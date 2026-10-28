//! Cursor's sessions, from its own database (`state.vscdb`): the session
//! headers Cursor keeps in `composerHeaders`, with each session's name,
//! times, folder, archive state and the session that started a sub-agent.
//! Cursor stores no tokens there (its messages count 0/0); a session's
//! tokens come from its turn log (`hooks::turn_log`).
use crate::grok::cache::{agent_id, hash};
use crate::transcript::records::{Page, Record};
use crate::Usage;
use rusqlite::Connection;
use serde_json::{json, Map, Value};

/// Sessions last active longer ago than this are left out: Agent HUD keeps
/// tokens 30 days and lists sessions 7.
const KEPT_MS: i64 = 30 * 86_400_000;

/// The sessions new or changed since `cursor`, the watermark of the last
/// read, which remembers each kept session as it was.
pub fn read(db: &Connection, cursor: &Value) -> rusqlite::Result<Page> {
    read_at(db, cursor, chrono::Utc::now().timestamp_millis())
}

pub(crate) fn read_at(db: &Connection, cursor: &Value, now_ms: i64) -> rusqlite::Result<Page> {
    let mut statement = db.prepare(
        "SELECT composerId, CASE WHEN json_valid(value) THEN json_extract(value,
            '$.name', '$.createdAt', '$.lastUpdatedAt',
            '$.conversationCheckpointLastUpdatedAt', '$.isArchived', '$.isDraft',
            '$.workspaceIdentifier.uri.fsPath', '$.subagentInfo.parentComposerId') END
         FROM composerHeaders",
    )?;
    let mut rows = statement.query([])?;
    let seen = cursor["seen"].as_object();
    let mut kept = Map::new();
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        let Some(id) = crate::transcript::sql_text(row, 0).filter(|id| agent_id(id)) else {
            continue;
        };
        let Some(fields) = crate::transcript::sql_text(row, 1)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            continue;
        };
        let Some(fields) = fields.as_array().filter(|f| f.len() == 8) else {
            continue;
        };
        let name = fields[0].as_str().map(str::trim).filter(|s| !s.is_empty());
        let checkpoint = fields[3].as_i64();
        let truthy = |v: &Value| v.as_bool().unwrap_or(false) || v.as_i64() == Some(1);
        // A draft, or a session nothing was ever said in.
        if truthy(&fields[5]) || (name.is_none() && checkpoint.is_none()) {
            continue;
        }
        let Some(at) = [&fields[1], &fields[2], &fields[3]]
            .iter()
            .filter_map(|v| v.as_i64())
            .filter(|at| *at > 0)
            .max()
        else {
            continue;
        };
        if at < now_ms - KEPT_MS {
            continue;
        }
        let archived = truthy(&fields[4]);
        let cwd = fields[6].as_str().filter(|s| !s.is_empty());
        let parent = fields[7].as_str().filter(|p| agent_id(p) && *p != id);
        let print = hash(&[
            name.unwrap_or(""),
            &at.to_string(),
            if archived { "1" } else { "0" },
            cwd.unwrap_or(""),
            parent.unwrap_or(""),
        ])[..16]
            .to_owned();
        let unchanged = seen
            .and_then(|seen| seen.get(&id))
            .is_some_and(|old| old.as_str() == Some(print.as_str()));
        kept.insert(id.clone(), json!(print));
        if unchanged {
            continue;
        }
        records.push(Record {
            id: format!("cursor:{id}"),
            title: name.map(str::to_owned),
            title_fallback: None,
            cwd: cwd.map(str::to_owned),
            model: "Unknown".into(),
            key: "header".into(),
            at,
            usage: Usage::default(),
            parent: parent.map(|p| format!("cursor:{p}")),
            archived: Some(archived),
            updated: None,
            duration_ms: None,
        });
    }
    Ok(Page {
        records,
        cursor: None,
        watermark: Some(json!({ "seen": kept })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_792_993_809_155;

    fn database(rows: &[(&str, Value)]) -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE composerHeaders(composerId TEXT PRIMARY KEY, value TEXT)")
            .unwrap();
        for (id, value) in rows {
            db.execute(
                "INSERT INTO composerHeaders VALUES(?1, ?2)",
                [*id, value.to_string().as_str()],
            )
            .unwrap();
        }
        db
    }

    fn header(name: Option<&str>, at: i64) -> Value {
        json!({
            "type": "head",
            "name": name,
            "createdAt": at - 5_000,
            "lastUpdatedAt": at - 5_000,
            "conversationCheckpointLastUpdatedAt": at,
            "workspaceIdentifier": {"uri": {"fsPath": "/Users/me/project"}},
        })
    }

    #[test]
    fn sessions_carry_their_name_folder_archive_and_parent() {
        let mut archived = header(Some("Old idea"), NOW - 60_000);
        archived["isArchived"] = json!(true);
        let mut child = header(Some("Verify CPU"), NOW - 30_000);
        child["subagentInfo"] = json!({"parentComposerId": "parent-1"});
        let mut draft = header(None, NOW);
        draft["isDraft"] = json!(true);
        let db = database(&[
            ("parent-1", header(Some("Code quality review"), NOW)),
            ("old-1", archived),
            ("child-1", child),
            ("draft-1", draft),
            ("empty-1", json!({"createdAt": NOW})),
            ("stale-1", header(Some("Last month"), NOW - 31 * 86_400_000)),
        ]);
        let page = read_at(&db, &Value::Null, NOW).unwrap();
        let mut ids = page
            .records
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, ["cursor:child-1", "cursor:old-1", "cursor:parent-1"]);
        let parent = page
            .records
            .iter()
            .find(|r| r.id == "cursor:parent-1")
            .unwrap();
        assert_eq!(parent.title.as_deref(), Some("Code quality review"));
        assert_eq!(parent.cwd.as_deref(), Some("/Users/me/project"));
        assert_eq!(parent.at, NOW);
        assert_eq!(parent.archived, Some(false));
        assert_eq!(parent.usage.total(), 0);
        let old = page
            .records
            .iter()
            .find(|r| r.id == "cursor:old-1")
            .unwrap();
        assert_eq!(old.archived, Some(true));
        let child = page
            .records
            .iter()
            .find(|r| r.id == "cursor:child-1")
            .unwrap();
        assert_eq!(child.parent.as_deref(), Some("cursor:parent-1"));
    }

    #[test]
    fn a_later_read_passes_on_only_the_sessions_that_changed() {
        let db = database(&[
            ("a", header(Some("First"), NOW - 10_000)),
            ("b", header(Some("Second"), NOW - 20_000)),
        ]);
        let first = read_at(&db, &Value::Null, NOW).unwrap();
        assert_eq!(first.records.len(), 2);
        let watermark = first.watermark.unwrap();

        let again = read_at(&db, &watermark, NOW).unwrap();
        assert!(again.records.is_empty());

        let mut renamed = header(Some("First, renamed"), NOW - 10_000);
        renamed["isArchived"] = json!(1);
        db.execute(
            "UPDATE composerHeaders SET value=?1 WHERE composerId='a'",
            [renamed.to_string()],
        )
        .unwrap();
        let changed = read_at(&db, &again.watermark.unwrap(), NOW).unwrap();
        assert_eq!(changed.records.len(), 1);
        assert_eq!(changed.records[0].title.as_deref(), Some("First, renamed"));
        assert_eq!(changed.records[0].archived, Some(true));
    }
}
