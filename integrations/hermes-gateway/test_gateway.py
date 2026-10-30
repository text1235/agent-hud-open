import tempfile
import unittest
import sqlite3
import time
import threading
from unittest.mock import patch
from contextlib import closing
from pathlib import Path
from gateway import Gateway
from observations import observe


class PersistenceTests(unittest.TestCase):
    def test_delivery_wait_is_event_driven_durable_and_scoped(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as folder:
            g = Gateway(Path(folder)/'state')
            project = g.dispatch('gateway_ensure_project', {'workspace':folder})
            def submit(key):
                return g.dispatch('gateway_submit', dict(project_id=project['id'],objective='x',boundary='x',acceptance='x',request_key=key))
            task, other = submit('ours'), submit('other')
            outputs = []
            started = threading.Event()
            def wait():
                started.set()
                outputs.append(g.wait_task(task['id'], timeout_seconds=3))
            thread = threading.Thread(target=wait)
            thread.start()
            try:
                assert started.wait(2)
                g.update(other['id'], 'completed', result='not ours')
                g.update(task['id'], 'completed', result='delivered')
                thread.join(3)
                assert not thread.is_alive()
                assert outputs[0]['ready'] is True
                assert outputs[0]['task']['result'] == 'delivered'
                cursor = outputs[0]['event']['seq']
                assert not g.wait_task(task['id'], cursor, 0)['ready']
                g.db.close()
                g = Gateway(Path(folder)/'state')
                assert g.wait_task(task['id'], timeout_seconds=0)['ready']
                with self.assertRaises(ValueError):
                    g.wait_task(task['id'], timeout_seconds=61)
            finally:
                g.stop.set()
                with g.changed:
                    g.changed.notify_all()
                thread.join(3)
                g.db.close()

    def test_unknown_blocks_own_project_but_not_independent_queue(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as folder:
            g = Gateway(Path(folder)/'state')
            try:
                root=Path(folder)
                (root/'a').mkdir();(root/'b').mkdir()
                pa=g.dispatch('gateway_ensure_project',{'workspace':str(root/'a')})
                pb=g.dispatch('gateway_ensure_project',{'workspace':str(root/'b')})
                def submit(project,key):
                    return g.dispatch('gateway_submit',dict(project_id=project['id'],objective='x',boundary='x',acceptance='x',request_key=key))
                old=submit(pa,'old');g.update(old['id'],'uncertain',remote='lost-id')
                blocked=submit(pa,'same-project');independent=submit(pb,'independent')
                self.assertEqual(g.next_task()['id'],independent['id'])
                self.assertEqual(g.task(old['id'])['status'],'uncertain')
                self.assertEqual(g.task(old['id'])['remote'],'lost-id')
                g.update(independent['id'],'running',remote='live-id')
                self.assertEqual(g.next_task()['id'],independent['id'])
                g.update(independent['id'],'completed',result='done')
                self.assertEqual(g.next_task()['id'],old['id'])
                g.update(old['id'],'completed',result='recovered')
                self.assertEqual(g.next_task()['id'],blocked['id'])
            finally:
                g.db.close()

    def test_old_timeout_is_unknown_and_cannot_be_reworked(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as folder:
            g = Gateway(Path(folder)/'state')
            project = g.dispatch('gateway_ensure_project', {'workspace':folder})
            args = dict(project_id=project['id'], objective='x', boundary='x', acceptance='x', request_key='x')
            task = g.dispatch('gateway_submit', args)
            g.update(task['id'], 'failed', remote='remote-id', result='[agent did not reply in time]')
            g.db.close()
            g = Gateway(Path(folder)/'state')
            try:
                assert g.task(task['id'])['status'] == 'uncertain'
                with self.assertRaises(ValueError):
                    g.dispatch('gateway_submit', dict(args, request_key='retry', parent_id=task['id']))
            finally:
                g.db.close()

    def test_recover_result_without_resubmission(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as folder:
            g = Gateway(Path(folder)/'state')
            project = g.dispatch('gateway_ensure_project', {'workspace':folder})
            task = g.dispatch('gateway_submit', dict(project_id=project['id'], objective='x', boundary='x', acceptance='x', request_key='x'))
            g.update(task['id'], 'uncertain', remote='remote-id')
            try:
                with patch('gateway.observe', return_value={'state':'completed', 'session_id':'s', 'result':'late answer'}):
                    got = g.dispatch('gateway_get_task', {'task_id':task['id']})
                assert got['status'] == 'completed'
                assert got['result'] == 'late answer'
                assert got['remote'] == 'remote-id'
            finally:
                g.db.close()

    def test_observation_is_bound_to_exact_task_and_turn(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as folder:
            path = Path(folder)/'sessions.db'
            with closing(sqlite3.connect(path)) as db:
                db.executescript('CREATE TABLE sessions(id,source); CREATE TABLE messages(id INTEGER PRIMARY KEY,session_id,role,content,tool_calls,tool_name,timestamp,finish_reason);')
                db.execute("INSERT INTO sessions VALUES('s','a2a')")
                rows = [('s','user','【任务ID】ours',None,None,time.time()-10,None),
                        ('s','tool','progress',None,'terminal',time.time(),None),
                        ('s','user','different task',None,None,time.time(),None),
                        ('s','assistant','other result',None,None,time.time(),'stop')]
                db.executemany('INSERT INTO messages(session_id,role,content,tool_calls,tool_name,timestamp,finish_reason) VALUES(?,?,?,?,?,?,?)', rows)
                db.commit()
            assert observe('ours', path)['state'] == 'recent_activity'
            assert observe('not-ours', path)['state'] == 'unknown'
            with closing(sqlite3.connect(path)) as db:
                db.execute('DELETE FROM messages WHERE id>=3')
                db.execute("INSERT INTO messages(session_id,role,content,timestamp,finish_reason) VALUES('s','assistant','our final',?,'stop')", (time.time(),))
                db.execute("INSERT INTO messages(session_id,role,content,timestamp) VALUES('s','session_meta','bookkeeping',?)", (time.time(),))
                db.commit()
            recovered = observe('ours', path)
            assert recovered['state'] == 'completed'
            assert recovered['result'] == 'our final'

    def test_idempotency_rework_and_review(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as folder:
            g = Gateway(Path(folder)/'state')
            project = g.dispatch('gateway_ensure_project',{'workspace':folder})
            args = dict(project_id=project['id'],objective='write test',boundary='only project',
                        acceptance='exact content',request_key='first')
            first = g.dispatch('gateway_submit',args)
            self.assertEqual(first['id'],g.dispatch('gateway_submit',args)['id'])
            with self.assertRaises(ValueError):
                g.dispatch('gateway_submit',dict(args,objective='different'))
            with self.assertRaises(ValueError):
                g.dispatch('gateway_review',dict(task_id=first['id'],decision='accepted',evidence='read file'))
            g.update(first['id'],'completed',result='done')
            g.dispatch('gateway_review',dict(task_id=first['id'],decision='rejected',evidence='missing line'))
            child = g.dispatch('gateway_submit',dict(args,request_key='rework',parent_id=first['id']))
            self.assertEqual(child['parent'],first['id'])
            g.update(child['id'],'submitting')
            g.db.close()
            g = Gateway(Path(folder)/'state')
            self.assertEqual(g.task(child['id'])['status'],'uncertain')
            self.assertEqual(g.task(first['id'])['review'],'rejected')
            g.db.close()


if __name__=='__main__':
    unittest.main()
