//! Cursor's completion hook, in its version-1 settings.
use crate::hooks::{Completed, Completion, Handler, Hooks, Layout, POSIX_ONLY};
use crate::text;
use crate::{Location, Usage};
use serde_json::Value;

pub static HOOKS: Hooks = Hooks {
    settings: Some(Location::home(&[], &[".cursor"], "hooks.json")),
    aliases: &[],
    layout: Layout::Handlers(Handler {
        command_key: "command",
        typed: false,
        timeout_key: "timeout",
    }),
    permission: None,
    completion: Some(Completion {
        event: "stop",
        timeout: 5,
        read: completed,
        enrich: None,
        reply: None,
    }),
    origin: None,
    attention: false,
    compaction: None,
    prefixed_sessions: true,
    launch: POSIX_ONLY,
};

/// A completed generation: the conversation and the generation that ended.
fn completed(payload: &Value, _now_ms: i64) -> Option<Completed> {
    if payload["hook_event_name"] != "stop" || payload["status"] != "completed" {
        return None;
    }
    let (Some(id), Some(turn)) = (
        text(&payload["conversation_id"]),
        text(&payload["generation_id"]),
    ) else {
        return None;
    };
    Some(Completed {
        session: id.into(),
        turn: turn.into(),
        workspace: payload["workspace_roots"][0].as_str().map(str::to_owned),
        model: payload["model_id"]
            .as_str()
            .or(payload["model"].as_str())
            .map(str::to_owned),
        transcript_path: None,
        usage: usage(payload),
    })
}

/// The generation's tokens, which Cursor 3.27 and later put in the `stop`
/// payload and keeps nowhere else on disk (its messages store 0/0).
/// `input_tokens` counts every prompt token, cache reads included: a turn of
/// about twenty calls whose context ended near 63K reported 740,409 input
/// with 639,616 of them read from the cache, so the fresh input is the
/// difference. Cache writes are taken as reported.
fn usage(payload: &Value) -> Option<Usage> {
    let count = |key: &str| payload[key].as_u64();
    let input = count("input_tokens")?;
    let output = count("output_tokens")?;
    let read = count("cache_read_tokens").unwrap_or(0);
    let write = count("cache_write_tokens").unwrap_or(0);
    if read > input {
        return None;
    }
    let usage = Usage {
        input: input - read,
        output,
        read,
        write,
        reasoning: 0,
    };
    (usage.total() > 0).then_some(usage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stop(extra: Value) -> Value {
        let mut payload = json!({
            "conversation_id": "ca086626-b368-46b5-8492-2c8b493f7e7a",
            "generation_id": "f89c40c7-ff67-435b-9cf3-a395582dd7e6",
            "model": "grok-4.7-high-fast",
            "model_id": "grok-4.7",
            "status": "completed",
            "hook_event_name": "stop",
            "workspace_roots": ["/Users/me/project"],
        });
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        payload
    }

    #[test]
    fn a_stop_with_tokens_keeps_the_fresh_input_apart_from_cache_reads() {
        let done = completed(
            &stop(json!({
                "input_tokens": 740_409,
                "output_tokens": 17_649,
                "cache_read_tokens": 639_616,
                "cache_write_tokens": 0,
            })),
            0,
        )
        .unwrap();
        let usage = done.usage.unwrap();
        assert_eq!(
            (usage.input, usage.output, usage.read, usage.write),
            (100_793, 17_649, 639_616, 0)
        );
        assert_eq!(done.model.as_deref(), Some("grok-4.7"));
        assert_eq!(done.workspace.as_deref(), Some("/Users/me/project"));
    }

    #[test]
    fn a_stop_without_tokens_or_with_impossible_ones_reports_none() {
        assert!(completed(&stop(json!({})), 0).unwrap().usage.is_none());
        assert!(completed(
            &stop(json!({"input_tokens": 10, "output_tokens": 1, "cache_read_tokens": 11})),
            0
        )
        .unwrap()
        .usage
        .is_none());
        assert!(
            completed(&stop(json!({"input_tokens": 0, "output_tokens": 0})), 0)
                .unwrap()
                .usage
                .is_none()
        );
    }
}
