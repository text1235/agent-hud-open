//! Antigravity JSONL transcript reducer (`brain/<id>/.system_generated/logs/transcript.jsonl`),
//! turn boundaries, and conversation items.
use crate::transcript::{
    at,
    items::{Items, LineItem, LineKind, LineRole, ToolCall},
    live::{last_paragraph, newer, Call, Work},
    metering, n, text,
    turns::{has, TurnStart, Turns},
};
use crate::{Log, Usage};
use serde_json::{json, Value};
use std::path::Path;

/// Infers the Antigravity client surface (`Desktop`, `IDE`, or `CLI`) from a
/// storage path (`~/.gemini/antigravity`, `~/.gemini/antigravity-ide`, or
/// `~/.gemini/antigravity-cli`).
pub fn client_from_path(path: &Path) -> Option<String> {
    for part in path.components() {
        let name = part.as_os_str().to_str()?;
        match name {
            "antigravity-cli" => return Some("CLI".into()),
            "antigravity-ide" => return Some("IDE".into()),
            "antigravity" => return Some("Desktop".into()),
            _ => {}
        }
    }
    None
}

/// Extracts the conversation UUID from an Antigravity `brain/<id>/...` or
/// `conversations/<id>.db` path.
pub fn session_id_from_path(path: &Path) -> Option<String> {
    let mut parts = path.components();
    while let Some(part) = parts.next() {
        if part.as_os_str() == "brain" {
            if let Some(id) = parts.next().and_then(|c| c.as_os_str().to_str()) {
                if !id.is_empty() {
                    return Some(id.to_owned());
                }
            }
        }
    }
    path.file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && *s != "transcript" && *s != "transcript_full")
        .map(str::to_owned)
}

/// Extracts the user's request text from Antigravity's `<USER_REQUEST>` wrapper
/// and strips `<ADDITIONAL_METADATA>` / `<USER_SETTINGS_CHANGE>` blocks.
pub fn clean_user_content(raw: &str) -> String {
    if let Some((_, rest)) = raw.split_once("<USER_REQUEST>") {
        if let Some((inside, _)) = rest.split_once("</USER_REQUEST>") {
            return inside.trim().to_owned();
        }
    }
    let mut cleaned = raw.to_owned();
    for (open, close) in [
        ("<ADDITIONAL_METADATA>", "</ADDITIONAL_METADATA>"),
        ("<USER_SETTINGS_CHANGE>", "</USER_SETTINGS_CHANGE>"),
        ("<CONTEXT_SUMMARY>", "</CONTEXT_SUMMARY>"),
    ] {
        while let Some(start) = cleaned.find(open) {
            if let Some(end_rel) = cleaned[start..].find(close) {
                let end = start + end_rel + close.len();
                cleaned.replace_range(start..end, "");
            } else {
                break;
            }
        }
    }
    cleaned.trim().to_owned()
}

fn tool_work(name: &str) -> Work {
    match name {
        "run_command" | "send_command_input" | "command_status" => Work::Shell,
        "edit_file"
        | "write_file"
        | "replace_file_content"
        | "multi_replace_file_content"
        | "write_to_file" => Work::Edit,
        "read_file" | "view_file" | "view_file_outline" | "view_code_item" | "grep_search"
        | "find_by_name" | "list_dir" | "read_url_content" | "search_web" => Work::Read,
        "invoke_subagent" | "define_subagent" | "manage_subagents" | "send_message" => Work::Agents,
        "ask_question" => Work::Question,
        _ => Work::Command,
    }
}

fn tool_name(call: &Value) -> Option<&str> {
    text(&call["name"]).or_else(|| text(call))
}

impl Log {
    pub fn antigravity(&mut self, v: &Value) {
        if self
            .path
            .file_name()
            .is_none_or(|name| name != "transcript.jsonl")
        {
            return;
        }
        let Some(raw) = session_id_from_path(&self.path) else {
            return;
        };
        self.own_id(&raw);
        if self.client.is_none() {
            self.client = client_from_path(&self.path);
        }
        let Some(at) = at(&v["created_at"]).or_else(|| at(&v["timestamp"])) else {
            return;
        };
        self.clock(at);
        if self.live.started_at.is_none() {
            self.live.started_at = Some(at);
        }
        let kind = text(&v["type"]).unwrap_or("");
        let step = n(&v["step_index"]);

        if kind == "USER_INPUT" && v["source"] != "SYSTEM" {
            let turn = format!("step-{step}");
            self.source_state["turn"] = json!(turn);
            if !self.live.is_subagent {
                self.mark_prompt(Some(at), None);
            }
            if let Some(raw_text) = text(&v["content"]) {
                let prompt = clean_user_content(raw_text);
                if !prompt.is_empty() {
                    self.title_from(&prompt);
                }
            }
            self.call_timing.request_at[0] = Some(at);
            newer(&mut self.live.last_message_at, at);
            self.live.observe_source(&turn, true, Some(at), at, None);
            self.live
                .activity
                .get_or_insert_with(Default::default)
                .start();
            return;
        }

        if matches!(kind, "GENERIC" | "TOOL_RESPONSE") {
            self.call_timing.request_at[0] = Some(at);
            if let Some(turn) = self.source_state["turn"].as_str().map(str::to_owned) {
                self.live.touch_source(&turn, at);
            }
            let failed = v["status"] == "ERROR";
            if let Some(activity) = &mut self.live.activity {
                if let Some(first) = activity.calls.first().map(|call| call.id.clone()) {
                    activity.close(&first, at, failed);
                }
            }
            return;
        }

        if kind != "PLANNER_RESPONSE" {
            return;
        }

        let turn = self.source_state["turn"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("step-{step}"));
        self.source_state["turn"] = json!(turn);
        if let Some(activity) = &mut self.live.activity {
            activity.calls.clear();
        }

        let mut tools = Vec::new();
        let mut calls = Vec::new();
        if let Some(list) = v["tool_calls"].as_array() {
            for (idx, item) in list.iter().enumerate() {
                if let Some(name) = tool_name(item) {
                    tools.push(name.to_owned());
                    let id = text(&item["id"])
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("step-{step}:call:{idx}"));
                    calls.push(Call {
                        id,
                        work: Some(tool_work(name)),
                        at,
                        request: None,
                    });
                }
            }
        }

        let status = text(&v["status"]).unwrap_or("");
        if !calls.is_empty() && !matches!(status, "ERROR" | "CANCELED") {
            self.live.observe_source(&turn, true, None, at, None);
            let activity = self.live.activity.get_or_insert_with(Default::default);
            for call in calls {
                activity.open(call);
            }
        } else {
            let outcome = match status {
                "DONE" => Some("completed"),
                "CANCELED" => Some("interrupted"),
                "ERROR" => Some("failed"),
                _ => None,
            };
            self.live
                .observe_source(&turn, outcome.is_none(), None, at, outcome);
        }

        if let Some(content) = text(&v["content"]) {
            newer(&mut self.live.last_message_at, at);
            if let Some(back) = self.live.turns.back_mut() {
                back.message = last_paragraph(content);
            }
        }

        if self.source_state["hasInference"] == true {
            return;
        }

        let input = n(&v["input_tokens"]);
        let read = n(&v["cache_read_tokens"]);
        let write = n(&v["cache_write_tokens"]);
        let out = n(&v["output_tokens"]);
        let reasoning = n(&v["thinking_tokens"]).min(out);
        if input + read + write + out == 0 {
            return;
        }
        let duration_ms = self.call_timing.since_request(0, at);
        self.call_timing.request_at[0] = Some(at);
        let key = format!("{}:step:{step}", self.id);
        let model = self.model.clone();
        self.count_timed_source(
            key.clone(),
            &model,
            at,
            Usage {
                input,
                output: out - reasoning,
                read,
                write,
                reasoning,
            },
            true,
            duration_ms,
        );
        let hash = metering::event_key(&key);
        if let Some(event) = self.meter_events.get_mut(&hash) {
            event.tools = tools;
        }
    }
}

/// An Antigravity turn starts at each explicit `USER_INPUT` step.
pub static TURNS: Turns = Turns {
    start: |value| {
        if value["type"] != "USER_INPUT" || value["source"] == "SYSTEM" {
            return None;
        }
        let step = value["step_index"].as_u64()?;
        Some(TurnStart::whole(
            Some(format!("step-{step}")),
            value.get("created_at").or_else(|| value.get("timestamp")),
        ))
    },
    may_name: |line| has(line, b"USER_INPUT"),
};

/// Visible user prompts, assistant thinking, assistant text, and tool calls in
/// an Antigravity `transcript.jsonl` step.
pub static ITEMS: Items = Items { read: read_items };

fn read_items(value: &Value, offset: u64) -> Vec<LineItem> {
    let timestamp = value["created_at"]
        .as_str()
        .or_else(|| value["timestamp"].as_str())
        .map(str::to_owned);
    let base_id = value["step_index"]
        .as_u64()
        .map(|s| format!("step-{s}"))
        .unwrap_or_else(|| format!("o{offset}"));
    let mut rows = Vec::new();
    match value["type"].as_str() {
        Some("USER_INPUT") if value["source"] != "SYSTEM" => {
            if let Some(raw) = text(&value["content"]) {
                let prompt = clean_user_content(raw);
                if !prompt.is_empty() {
                    rows.push(LineItem {
                        id: base_id,
                        parent: None,
                        role: LineRole::User,
                        kind: LineKind::Text,
                        text: Some(prompt),
                        tool: None,
                        tool_call_id: None,
                        timestamp,
                        is_prompt: true,
                        origin: Some("human".into()),
                    });
                }
            }
        }
        Some("PLANNER_RESPONSE") => {
            if let Some(thinking) = text(&value["thinking"]) {
                rows.push(LineItem {
                    id: format!("{base_id}:thinking"),
                    parent: None,
                    role: LineRole::Assistant,
                    kind: LineKind::Thinking,
                    text: Some(thinking.trim().to_owned()),
                    tool: None,
                    tool_call_id: None,
                    timestamp: timestamp.clone(),
                    is_prompt: false,
                    origin: None,
                });
            }
            if let Some(content) = text(&value["content"]) {
                rows.push(LineItem {
                    id: format!("{base_id}:text"),
                    parent: None,
                    role: LineRole::Assistant,
                    kind: LineKind::Text,
                    text: Some(content.trim().to_owned()),
                    tool: None,
                    tool_call_id: None,
                    timestamp: timestamp.clone(),
                    is_prompt: false,
                    origin: None,
                });
            }
            if let Some(calls) = value["tool_calls"].as_array() {
                for (idx, call) in calls.iter().enumerate() {
                    let Some(name) = tool_name(call) else {
                        continue;
                    };
                    let call_id = text(&call["id"])
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("{base_id}:call:{idx}"));
                    let arguments = call
                        .get("args")
                        .or_else(|| call.get("arguments"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    rows.push(LineItem {
                        id: format!("{base_id}:tool:{idx}"),
                        parent: None,
                        role: LineRole::Assistant,
                        kind: LineKind::Tool,
                        text: None,
                        tool: Some(ToolCall {
                            id: Some(call_id.clone()),
                            name: name.to_owned(),
                            arguments,
                        }),
                        tool_call_id: Some(call_id),
                        timestamp: timestamp.clone(),
                        is_prompt: false,
                        origin: None,
                    });
                }
            }
        }
        _ => {}
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn cleans_user_request_tags_and_metadata() {
        let raw = "<USER_REQUEST>\nFix the tests\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nnow\n</ADDITIONAL_METADATA>";
        assert_eq!(clean_user_content(raw), "Fix the tests");
        let without_wrapper = "Hello world\n<ADDITIONAL_METADATA>\nnow\n</ADDITIONAL_METADATA>";
        assert_eq!(clean_user_content(without_wrapper), "Hello world");
    }

    #[test]
    fn reduces_transcript_lifecycle_tools_and_items() {
        let path = PathBuf::from(
            "/Users/test/.gemini/antigravity/brain/3e144c9b-bb25-4894-af49-508ecf3540c3/.system_generated/logs/transcript.jsonl",
        );
        let mut log = Log::new(path, "antigravity");
        let step0 = json!({
            "step_index": 0,
            "source": "USER_EXPLICIT",
            "type": "USER_INPUT",
            "status": "DONE",
            "created_at": "2026-10-26T05:58:56Z",
            "content": "<USER_REQUEST>\nInspect the workspace\n</USER_REQUEST>"
        });
        log.parse(&step0);
        assert_eq!(log.id, "antigravity:3e144c9b-bb25-4894-af49-508ecf3540c3");
        assert_eq!(log.client.as_deref(), Some("Desktop"));
        assert!(log.live.turns.back().unwrap().running);

        let step1 = json!({
            "step_index": 1,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "created_at": "2026-10-26T05:59:00Z",
            "input_tokens": 1000,
            "cache_read_tokens": 500,
            "output_tokens": 120,
            "thinking": "Reading files first.",
            "tool_calls": [{"name": "read_file", "args": {"Path": "/tmp/a"}}]
        });
        log.parse(&step1);
        assert!(log.live.turns.back().unwrap().running);
        assert_eq!(log.usage.input, 1000);
        assert_eq!(log.usage.read, 500);
        assert_eq!(log.usage.output, 120);

        let step2 = json!({
            "step_index": 2,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "created_at": "2026-10-26T05:59:10Z",
            "input_tokens": 1200,
            "cache_read_tokens": 1000,
            "output_tokens": 80,
            "content": "All done."
        });
        log.parse(&step2);
        let turn = log.live.turns.back().unwrap();
        assert!(!turn.running);
        assert_eq!(turn.outcome.as_deref(), Some("completed"));
        assert_eq!(turn.message.as_deref(), Some("All done."));

        let items0 = (ITEMS.read)(&step0, 0);
        assert_eq!(items0.len(), 1);
        assert!(items0[0].is_prompt);
        assert_eq!(items0[0].text.as_deref(), Some("Inspect the workspace"));

        let items1 = (ITEMS.read)(&step1, 100);
        assert_eq!(items1.len(), 2);
        assert_eq!(items1[0].kind, LineKind::Thinking);
        assert_eq!(items1[1].kind, LineKind::Tool);
    }
}
