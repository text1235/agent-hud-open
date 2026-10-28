//! Appends a completed turn's tokens to its client's turn log
//! (`agent_hud_agents::hooks::turn_log`). The completion hook does it, so a
//! turn is kept while Agent HUD is closed; the engine reads the log like a
//! client's own.
use agent_hud_agents::hooks::turn_log;
use agent_hud_agents::HookEvent;
use std::{fs, io::Write, path::Path};

pub(super) fn append(data: &Path, event: &HookEvent) -> Result<(), String> {
    let Some(line) = turn_log::line(event) else {
        return Ok(());
    };
    if agent_hud_agents::by_id(&event.source).is_none() {
        return Err("Turn logs are kept for known clients only".into());
    }
    let name = turn_log::file_name(event.at_ms).ok_or("The turn has no date")?;
    let folders = data.join(turn_log::FOLDER);
    crate::os::private_directory(&folders)?;
    let folder = folders.join(&event.source);
    crate::os::private_directory(&folder)?;
    // One write of a whole line, in append mode: two turns ending together
    // each keep theirs.
    crate::os::private_append(&folder.join(&name))
        .and_then(|mut file| file.write_all(line.as_bytes()))
        .map_err(|_| "The turn could not be kept".to_owned())?;
    // Months past the 30 days tokens are kept go.
    if let Ok(entries) = fs::read_dir(&folder) {
        for entry in entries.flatten() {
            let old = entry.file_name();
            if old
                .to_str()
                .is_some_and(|old| turn_log::expired(old, event.at_ms))
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn completion(at_ms: i64, usage: serde_json::Value) -> HookEvent {
        HookEvent {
            source: "cursor".into(),
            session_id: "cursor:c1".into(),
            at_ms,
            kind: "completion".into(),
            data: json!({"turnId": "g1", "model": "grok-4.7", "workspace": "/p", "usage": usage}),
        }
    }

    #[test]
    fn a_turn_with_tokens_is_appended_to_its_month_and_old_months_go() {
        let data = tempfile::tempdir().unwrap();
        // Made as the app makes them: on Windows a plain folder made by an
        // elevated process belongs to Administrators, which the owner check
        // rightly refuses.
        let folder = data.path().join("clients/cursor");
        crate::os::private_directory(&data.path().join("clients")).unwrap();
        crate::os::private_directory(&folder).unwrap();
        fs::write(folder.join("turns-2026-08.jsonl"), "{}\n").unwrap();
        fs::write(folder.join("turns-2026-09.jsonl"), "{}\n").unwrap();
        let usage = json!({"input": 100, "output": 7, "read": 600, "write": 0, "reasoning": 0});
        // 2026-10-26T05:50Z, twice.
        append(data.path(), &completion(1_792_993_809_155, usage.clone())).unwrap();
        append(data.path(), &completion(1_792_993_809_156, usage)).unwrap();
        append(
            data.path(),
            &completion(1_792_993_809_157, serde_json::Value::Null),
        )
        .unwrap();
        let text = fs::read_to_string(folder.join("turns-2026-10.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(!folder.join("turns-2026-08.jsonl").exists());
        assert!(folder.join("turns-2026-09.jsonl").exists());
    }
}
