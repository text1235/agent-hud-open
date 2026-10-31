//! Read completed Desktop replies from Hermes' own database; never replay history.
use agent_hud_agents::HookEvent;
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub(crate) struct Reader {
    cursors: HashMap<PathBuf, i64>,
}

impl Reader {
    fn read(&mut self, path: &Path) -> rusqlite::Result<Vec<HookEvent>> {
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let maximum: i64 =
            db.query_row("SELECT COALESCE(MAX(id),0) FROM messages", [], |r| r.get(0))?;
        let Some(previous) = self.cursors.get(path).copied().filter(|id| *id <= maximum) else {
            self.cursors.insert(path.to_owned(), maximum);
            return Ok(vec![]);
        };
        if previous == maximum {
            return Ok(vec![]);
        }
        let lease = if db.table_exists(None, "session_turn_leases")? {
            "EXISTS(SELECT 1 FROM session_turn_leases l WHERE l.conversation_id=m.session_id AND l.expires_at>unixepoch('now'))"
        } else {
            "0"
        };
        let sql = format!("SELECT m.id,m.session_id,m.timestamp,substr(m.content,1,4000),s.title,s.cwd,s.model,
            (SELECT MAX(u.timestamp) FROM messages u WHERE u.session_id=m.session_id AND u.role='user' AND u.active=1 AND u.id<m.id),{lease}
            FROM messages m JOIN sessions s ON s.id=m.session_id
            WHERE m.id>?1 AND m.id<=?2 AND m.role='assistant' AND m.finish_reason IN ('stop','end_turn','completed')
            AND (m.tool_calls IS NULL OR trim(m.tool_calls) IN ('','[]'))
            AND m.active=1 AND m.compacted=0 AND m._compressed_summary=0
            AND s.source='desktop' AND s.parent_session_id IS NULL AND s.archived=0 AND s.hidden=0
            AND NOT EXISTS(SELECT 1 FROM messages later WHERE later.session_id=m.session_id AND later.active=1 AND later.id>m.id)
            ORDER BY m.id");
        let mut query = db.prepare(&sql)?;
        let mut rows = query.query([previous, maximum])?;
        let mut events = Vec::new();
        let mut cursor = maximum;
        while let Some(row) = rows.next()? {
            let message_id: i64 = row.get(0)?;
            if row.get::<_, bool>(8)? {
                // A final message may be committed before the turn lease is released.
                // Keep it eligible for the next existing collector pass.
                cursor = cursor.min(message_id - 1);
                continue;
            }
            let id: String = row.get(1)?;
            let ended: f64 = row.get(2)?;
            let message: Option<String> = row.get(3)?;
            let started: Option<f64> = row.get(7)?;
            let Some(started) = started.filter(|start| start.is_finite() && *start <= ended) else {
                continue;
            };
            let Some(message) = message.filter(|s| !s.trim().is_empty()) else {
                continue;
            };
            if !ended.is_finite() {
                continue;
            }
            events.push(HookEvent {
                source: "hermes".into(), session_id: format!("hermes:{id}"), kind: "completion".into(),
                at_ms: (ended * 1000.) as i64,
                data: json!({"turnId":format!("hermes-message:{message_id}"),"startedAtMs":(started*1000.) as i64,
                    "title":row.get::<_,Option<String>>(4)?,"workspace":row.get::<_,Option<String>>(5)?,
                    "model":row.get::<_,Option<String>>(6)?,"message":message,
                    "evidence":"native-session-database","navigationTarget":{"kind":"hermes-session","id":id}}),
            });
        }
        self.cursors.insert(path.to_owned(), cursor);
        Ok(events)
    }
}

impl crate::AgentHudCore {
    pub(crate) fn read_hermes_completions(&mut self) {
        if self.demo || !self.enabled.get("hermes").copied().unwrap_or(true) {
            return;
        }
        let paths: Vec<_> = self
            .provider_roots
            .iter()
            .filter(|(_, provider)| *provider == "hermes")
            .map(|(path, _)| path.join("state.db"))
            .filter(|path| path.is_file())
            .collect();
        for path in paths {
            if let Ok(events) = self.hermes_completions.read(&path) {
                self.hook_events
                    .ingest_native_hermes(&events, chrono::Utc::now().timestamp_millis());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn database(path: &Path) -> Connection {
        let db = Connection::open(path).unwrap();
        db.execute_batch("CREATE TABLE sessions(id TEXT PRIMARY KEY,source TEXT,title TEXT,cwd TEXT,model TEXT,parent_session_id TEXT,archived INTEGER,hidden INTEGER);
            CREATE TABLE messages(id INTEGER PRIMARY KEY,session_id TEXT,role TEXT,timestamp REAL,content TEXT,tool_calls TEXT,finish_reason TEXT,active INTEGER,compacted INTEGER,_compressed_summary INTEGER);
            INSERT INTO sessions VALUES('20261031_test','desktop','test','D:\\ExampleProject','mercury-3',NULL,0,0);
            INSERT INTO messages VALUES(1,'20261031_test','user',100,'question',NULL,NULL,1,0,0);
            INSERT INTO messages VALUES(2,'20261031_test','assistant',101,'old reply',NULL,'stop',1,0,0);").unwrap();
        db
    }
    #[test]
    fn startup_dedup_and_restart_never_replay_a_reply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let db = database(&path);
        let mut reader = Reader::default();
        assert!(reader.read(&path).unwrap().is_empty());
        db.execute_batch("INSERT INTO messages VALUES(3,'20261031_test','user',102,'new question',NULL,NULL,1,0,0);
            INSERT INTO messages VALUES(4,'20261031_test','assistant',103,'new reply',NULL,'stop',1,0,0);").unwrap();
        let events = reader.read(&path).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data["message"], "new reply");
        assert_eq!(events[0].at_ms, 103000);
        assert!(reader.read(&path).unwrap().is_empty());
        assert!(Reader::default().read(&path).unwrap().is_empty());
    }
    #[test]
    fn tools_incomplete_truncated_compacted_and_non_desktop_messages_do_not_complete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let db = database(&path);
        let mut reader = Reader::default();
        reader.read(&path).unwrap();
        db.execute_batch("INSERT INTO messages VALUES(3,'20261031_test','assistant',102,'tool call','[{}]','stop',1,0,0);
            INSERT INTO messages VALUES(4,'20261031_test','assistant',103,'partial',NULL,NULL,1,0,0);
            INSERT INTO messages VALUES(5,'20261031_test','assistant',104,'truncated',NULL,'length',1,0,0);
            INSERT INTO messages VALUES(6,'20261031_test','assistant',105,'summary',NULL,'stop',1,1,1);
            UPDATE sessions SET source='weixin';").unwrap();
        assert!(reader.read(&path).unwrap().is_empty());
        db.execute_batch("UPDATE sessions SET source='desktop';
            INSERT INTO messages VALUES(7,'20261031_test','assistant',106,'tool call','[{}]','stop',1,0,0);
            INSERT INTO messages VALUES(8,'20261031_test','assistant',107,'partial',NULL,NULL,1,0,0);
            INSERT INTO messages VALUES(9,'20261031_test','assistant',108,'truncated',NULL,'length',1,0,0);
            INSERT INTO messages VALUES(10,'20261031_test','assistant',109,'summary',NULL,'stop',1,1,1);").unwrap();
        assert!(reader.read(&path).unwrap().is_empty());
    }
    #[test]
    fn a_new_native_reply_reaches_the_real_snapshot_event_tracker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let db = database(&path);
        let now = chrono::Utc::now().timestamp_millis();
        db.execute_batch("ALTER TABLE sessions ADD COLUMN started_at REAL;")
            .unwrap();
        db.execute(
            "UPDATE sessions SET started_at=?1",
            [(now - 20_000) as f64 / 1000.],
        )
        .unwrap();
        let mut core = crate::AgentHudCore::new(
            vec![crate::SourceRoot {
                path: dir.path().to_owned(),
                provider: "hermes",
            }],
            dir.path().join("hud"),
        );
        let first = core.request("snapshot", json!({})).unwrap();
        assert!(!first["islandEvents"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["source"] == "hermes"));
        // The tracker intentionally suppresses replies from before HUD startup.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let now = chrono::Utc::now().timestamp_millis();
        db.execute("INSERT INTO messages VALUES(3,'20261031_test','user',?1,'new question',NULL,NULL,1,0,0)",[(now-1000) as f64/1000.]).unwrap();
        db.execute("INSERT INTO messages VALUES(4,'20261031_test','assistant',?1,'new native reply',NULL,'stop',1,0,0)",[now as f64/1000.]).unwrap();
        let next = core.request("snapshot", json!({})).unwrap();
        let event = next["islandEvents"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["sessionId"] == "hermes:20261031_test")
            .expect("Hermes completion must reach the visible event tracker");
        assert_eq!(event["message"], "new native reply");
        assert_eq!(event["navigationTarget"]["id"], "20261031_test");
        let repeated = core.request("snapshot", json!({})).unwrap();
        assert_eq!(next["islandEvents"], repeated["islandEvents"]);
    }

    #[test]
    fn an_active_turn_waits_for_release_and_a_later_tool_suppresses_an_intermediate_reply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let db = database(&path);
        db.execute_batch("CREATE TABLE session_turn_leases(conversation_id TEXT,expires_at REAL);")
            .unwrap();
        let mut reader = Reader::default();
        reader.read(&path).unwrap();
        db.execute_batch("INSERT INTO messages VALUES(3,'20261031_test','assistant',102,'intermediate',NULL,'stop',1,0,0);
            INSERT INTO messages VALUES(4,'20261031_test','tool',103,'tool result',NULL,NULL,1,0,0);").unwrap();
        assert!(reader.read(&path).unwrap().is_empty());
        db.execute_batch("INSERT INTO messages VALUES(5,'20261031_test','assistant',104,'finished',NULL,'stop',1,0,0);
            INSERT INTO session_turn_leases VALUES('20261031_test',unixepoch('now')+60);").unwrap();
        assert!(reader.read(&path).unwrap().is_empty());
        db.execute_batch("DELETE FROM session_turn_leases").unwrap();
        let events = reader.read(&path).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data["message"], "finished");
        assert!(reader.read(&path).unwrap().is_empty());
    }
}
