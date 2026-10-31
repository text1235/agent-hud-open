"""Read-only execution evidence for this gateway's exact task, never another chat."""
import sqlite3
import time
from contextlib import closing
from pathlib import Path


def observe(task_id, database=Path(r'Z:\Hermes\data\state.db'), session_id=None):
    if not database.exists():
        return {'state': 'unknown', 'reason': 'session database unavailable'}
    try:
        with closing(sqlite3.connect(database.as_uri() + '?mode=ro', uri=True, timeout=2)) as db:
            db.row_factory = sqlite3.Row
            if session_id:
                anchor = db.execute(
                    "SELECT id,session_id,timestamp FROM messages WHERE session_id=? "
                    "AND role='user' AND instr(content,?)>0 ORDER BY id LIMIT 1",
                    (session_id, '【任务ID】' + task_id)).fetchone()
            else:
                anchor = db.execute(
                    "SELECT m.id,m.session_id,m.timestamp FROM messages m JOIN sessions s ON s.id=m.session_id "
                    "WHERE s.source='a2a' AND m.role='user' AND instr(m.content,?)>0 ORDER BY m.id LIMIT 1",
                    ('【任务ID】' + task_id,)).fetchone()
            if anchor is None:
                return {'state': 'unknown', 'reason': 'no matching execution session'}
            next_user = db.execute("SELECT min(id) FROM messages WHERE session_id=? AND role='user' AND id>?",
                                   (anchor['session_id'], anchor['id'])).fetchone()[0]
            row = db.execute(
                'SELECT role,content,tool_calls,tool_name,timestamp,finish_reason FROM messages '
                "WHERE session_id=? AND id>=? AND role IN ('user','assistant','tool') "
                'AND (? IS NULL OR id<?) ORDER BY id DESC LIMIT 1',
                (anchor['session_id'], anchor['id'], next_user, next_user)).fetchone()
            evidence = {'state': 'unknown', 'session_id': anchor['session_id'],
                        'last_activity_at': row['timestamp'], 'last_role': row['role'],
                        'last_tool': row['tool_name'], 'source': 'Hermes session transcript'}
            if row['role'] == 'assistant' and not row['tool_calls'] and row['finish_reason'] == 'stop' and row['content']:
                evidence.update(state='completed', result=row['content'])
            elif row['timestamp'] > anchor['timestamp'] and time.time() - row['timestamp'] <= 180:
                evidence['state'] = 'recent_activity'
            return evidence
    except (sqlite3.Error, OSError, ValueError) as exc:
        return {'state': 'unknown', 'reason': str(exc)}
