//! A client's turn log: the tokens each completed turn spent, one JSON line a
//! turn. The completion hook appends it for a client whose callback reports
//! a turn's tokens and that keeps them nowhere else on disk (Cursor). Agent
//! HUD keeps it in its own data folder, `clients/<client>/turns-<YYYY-MM>.jsonl`,
//! and reads it like a client's log: only the lines past where it stopped.
use crate::transcript::records::{Page, Record};
use crate::{HookEvent, Usage};
use chrono::{DateTime, Datelike, Months, Utc};
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

/// The folder in Agent HUD's data folder that holds each client's turn log.
pub const FOLDER: &str = "clients";
/// The most a read takes in one page.
const PAGE: u64 = 4 * 1024 * 1024;

/// The file a turn completed at `at_ms` goes to: one a month, in UTC.
pub fn file_name(at_ms: i64) -> Option<String> {
    let at = DateTime::<Utc>::from_timestamp_millis(at_ms)?;
    Some(format!("turns-{:04}-{:02}.jsonl", at.year(), at.month()))
}

/// Whether `name` is a month's turn log older than the month before
/// `now_ms`'s, past the 30 days tokens are kept.
pub fn expired(name: &str, now_ms: i64) -> bool {
    let Some(now) = DateTime::<Utc>::from_timestamp_millis(now_ms) else {
        return false;
    };
    let Some(previous) = now
        .date_naive()
        .with_day(1)
        .and_then(|first| first.checked_sub_months(Months::new(1)))
    else {
        return false;
    };
    let oldest = format!("turns-{:04}-{:02}.jsonl", previous.year(), previous.month());
    is_month_file(name) && name < oldest.as_str()
}

fn is_month_file(name: &str) -> bool {
    let Some(month) = name
        .strip_prefix("turns-")
        .and_then(|rest| rest.strip_suffix(".jsonl"))
    else {
        return false;
    };
    let bytes = month.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || b.is_ascii_digit())
}

/// Whether `path` is a turn log: a month's file in a client's folder of the
/// turn logs folder.
pub fn accepts(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_month_file)
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|name| name == FOLDER)
}

/// The line a completion appends, when it reports the turn's tokens.
pub fn line(event: &HookEvent) -> Option<String> {
    if event.kind != "completion" || event.session_id.is_empty() {
        return None;
    }
    let usage: Usage = serde_json::from_value(event.data.get("usage")?.clone()).ok()?;
    let turn = event.data["turnId"].as_str().filter(|s| !s.is_empty())?;
    let mut line = json!({
        "session": event.session_id,
        "turn": turn,
        "at": event.at_ms,
        "model": event.data["model"],
        "workspace": event.data["workspace"],
        "usage": usage,
    })
    .to_string();
    line.push('\n');
    Some(line)
}

/// The turns a turn log holds past `cursor`, a page at a time. A line still
/// being written waits for the next read.
pub fn read(path: &Path, cursor: &Value) -> rusqlite::Result<Page> {
    let failed = |_| rusqlite::Error::InvalidQuery;
    let mut file = File::open(path).map_err(failed)?;
    let length = file.metadata().map_err(failed)?.len();
    let mut after = cursor["after"].as_u64().unwrap_or(0);
    if after > length {
        after = 0;
    }
    file.seek(SeekFrom::Start(after)).map_err(failed)?;
    let mut bytes = Vec::new();
    file.take(PAGE).read_to_end(&mut bytes).map_err(failed)?;
    let whole = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |end| end + 1);
    let mut records = Vec::new();
    for line in bytes[..whole].split(|b| *b == b'\n') {
        if let Some(record) = serde_json::from_slice::<Value>(line)
            .ok()
            .and_then(|v| record(&v))
        {
            records.push(record);
        }
    }
    let next = after + whole as u64;
    let more = whole > 0 && bytes.len() as u64 == PAGE && next < length;
    Ok(Page {
        records,
        cursor: more.then(|| json!({ "after": next })),
        watermark: (!more).then(|| json!({ "after": next })),
    })
}

fn record(v: &Value) -> Option<Record> {
    let id = v["session"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512)?;
    let key = v["turn"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512)?;
    let at = v["at"].as_i64().filter(|at| *at > 0)?;
    let usage: Usage = serde_json::from_value(v["usage"].clone()).ok()?;
    Some(Record {
        id: id.into(),
        title: None,
        title_fallback: None,
        cwd: v["workspace"].as_str().map(str::to_owned),
        model: v["model"].as_str().unwrap_or("Unknown").into(),
        key: format!("turn:{key}"),
        at,
        usage,
        parent: None,
        archived: None,
        updated: None,
        duration_ms: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn completion(turn: &str, at: i64, usage: Value) -> HookEvent {
        HookEvent {
            source: "cursor".into(),
            session_id: "cursor:c1".into(),
            at_ms: at,
            kind: "completion".into(),
            data: json!({"turnId": turn, "model": "grok-4.7", "workspace": "/p", "usage": usage}),
        }
    }

    #[test]
    fn a_month_file_is_named_in_utc_and_expires_after_the_month_before() {
        // 2026-10-26T05:50Z and 2026-10-31T23:30Z.
        assert_eq!(file_name(1_792_993_809_155).unwrap(), "turns-2026-10.jsonl");
        assert_eq!(file_name(1_793_489_400_000).unwrap(), "turns-2026-10.jsonl");
        let now = 1_792_993_809_155;
        assert!(!expired("turns-2026-10.jsonl", now));
        assert!(!expired("turns-2026-09.jsonl", now));
        assert!(expired("turns-2026-08.jsonl", now));
        assert!(expired("turns-2025-12.jsonl", now));
        assert!(!expired("notes.jsonl", now));
        assert!(accepts(Path::new("/d/clients/cursor/turns-2026-10.jsonl")));
        assert!(!accepts(Path::new("/d/clients/cursor/turns-2026-1.jsonl")));
        assert!(!accepts(Path::new("/d/other/cursor/turns-2026-10.jsonl")));
    }

    #[test]
    fn turns_are_read_from_where_the_last_read_stopped_and_a_partial_line_waits() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("turns-2026-10.jsonl");
        let usage = json!({"input": 100, "output": 7, "read": 600, "write": 0, "reasoning": 0});
        let first = line(&completion("g1", 1_000, usage.clone())).unwrap();
        let second = line(&completion("g2", 2_000, usage)).unwrap();
        assert!(line(&completion("g3", 3_000, Value::Null)).is_none());
        let mut file = File::create(&path).unwrap();
        file.write_all(first.as_bytes()).unwrap();
        file.write_all(&second.as_bytes()[..10]).unwrap();
        file.flush().unwrap();

        let page = read(&path, &Value::Null).unwrap();
        assert_eq!(page.records.len(), 1);
        let record = &page.records[0];
        assert_eq!(
            (record.id.as_str(), record.key.as_str()),
            ("cursor:c1", "turn:g1")
        );
        assert_eq!((record.usage.input, record.usage.read), (100, 600));
        assert_eq!(record.cwd.as_deref(), Some("/p"));
        assert!(record.archived.is_none());
        assert!(page.cursor.is_none());
        let watermark = page.watermark.unwrap();
        assert_eq!(watermark["after"], first.len());

        file.write_all(&second.as_bytes()[10..]).unwrap();
        file.flush().unwrap();
        let page = read(&path, &watermark).unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].key, "turn:g2");
        assert_eq!(page.watermark.unwrap()["after"], first.len() + second.len());
    }
}
