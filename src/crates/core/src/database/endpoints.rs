//! Where each session's calls go, as its client's hook reported it
//! (docs/proposals/no-plan-usage.md §1): kept so a log read later, or read
//! again, still records its calls under their route.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

/// Keeps `endpoint` for the session. A report that did not probe the same
/// host keeps what the session's start found out about it.
pub(crate) fn record(
    db: &Connection,
    source: &str,
    session_id: &str,
    endpoint: &Value,
    at_ms: i64,
) -> rusqlite::Result<bool> {
    let mut endpoint = endpoint.clone();
    if let Some(old) = find(db, source, session_id)? {
        if old["host"] == endpoint["host"] && endpoint["server"].is_null() {
            endpoint["server"] = old["server"].clone();
            endpoint["contextWindow"] = old["contextWindow"].clone();
        }
        if old == endpoint {
            return Ok(false);
        }
    }
    db.prepare_cached(
        "INSERT INTO session_endpoint(source, session_id, endpoint, updated_at_ms)
         VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(source, session_id) DO UPDATE SET
           endpoint = excluded.endpoint, updated_at_ms = excluded.updated_at_ms",
    )?
    .execute(params![source, session_id, endpoint.to_string(), at_ms])?;
    Ok(true)
}

/// The endpoint kept for the session; None when its hook reported none.
pub(crate) fn find(
    db: &Connection,
    source: &str,
    session_id: &str,
) -> rusqlite::Result<Option<Value>> {
    let text: Option<String> = db
        .prepare_cached(
            "SELECT endpoint FROM session_endpoint WHERE source = ?1 AND session_id = ?2",
        )?
        .query_row(params![source, session_id], |row| row.get(0))
        .optional()?;
    Ok(text.and_then(|text| serde_json::from_str(&text).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_prompts_report_keeps_what_the_start_probed() {
        let db = Connection::open_in_memory().unwrap();
        crate::database::schema(&db).unwrap();
        let started = json!({"host": "localhost:11434", "platform": null, "server": "ollama", "contextWindow": 32768});
        assert!(record(&db, "claude", "s1", &started, 1).unwrap());
        let prompt = json!({"host": "localhost:11434", "platform": null});
        assert!(
            !record(&db, "claude", "s1", &prompt, 2).unwrap(),
            "nothing new"
        );
        assert_eq!(find(&db, "claude", "s1").unwrap(), Some(started));
        let moved = json!({"host": "relay.example.com", "platform": null});
        assert!(record(&db, "claude", "s1", &moved, 3).unwrap());
        assert_eq!(find(&db, "claude", "s1").unwrap(), Some(moved));
        assert_eq!(find(&db, "claude", "other").unwrap(), None);
    }
}
