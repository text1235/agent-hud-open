import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from gateway import Gateway
from native_desktop import NativeTransport, key
from observations import observe

class Client:
    backend={'pid':123,'port':12345}
    def __init__(self, gateway, root, bad_cwd=False, uncertain=False):
        self.gateway=gateway; self.root=root; self.bad_cwd=bad_cwd; self.uncertain=uncertain
        self.calls=[]; self.closed=False; self.project={'id':'native-project','primary_path':str(root)}
    def rpc(self, method, params):
        self.calls.append((method,params))
        if method=='projects.list':return {'projects':[]}
        if method=='projects.create':return {'project':self.project}
        if method=='projects.for_cwd':return {'project':self.project}
        if method=='session.create':
            self.office=params['cwd'];return {'session_id':'runtime','stored_session_id':'stored'}
        if method in ('session.activate','session.resume'):
            return {'running':True,'info':{'cwd':str(self.root) if self.bad_cwd else self.office,'model':'agnes-3.0-flash','project':self.project}}
        if method=='prompt.submit':
            row=self.gateway.db.execute('SELECT * FROM native_bindings').fetchone()
            assert row and self.gateway.task(row['task'])['remote']=='desktop:stored'
            if self.uncertain:raise RuntimeError('reply lost')
            return {'status':'streaming','user_row_id':7}
        raise AssertionError(method)
    def close(self):self.closed=True

class NativeTests(unittest.TestCase):
    def setup_gateway(self, folder, **flags):
        root=Path(folder)/'project'; root.mkdir()
        g=Gateway(Path(folder)/'state')
        (g.state/'native-desktop.json').write_text(json.dumps({'projects':[str(root)],'model':'agnes-3.0-flash','provider':'relay'}),'utf-8')
        p=g.dispatch('gateway_ensure_project',{'workspace':str(root)})
        task=g.dispatch('gateway_submit',dict(project_id=p['id'],objective='proof',boundary='office only',acceptance='file',request_key='test-native'))
        client=Client(g,root,**flags)
        g.native=NativeTransport(g,lambda *args:client)
        g.native.evidence=lambda task:{'state':'unknown'}
        return g,root,p,task,client
    def test_binds_before_single_prompt_and_uses_project_office(self):
        with tempfile.TemporaryDirectory() as folder:
            g,root,p,t,c=self.setup_gateway(folder)
            try:
                result=g.native.submit(t,p,'【任务ID】'+t['id'])
                binding=g.native.binding(t['id'])
                self.assertTrue(Path(binding['cwd']).resolve().is_relative_to(root.resolve()))
                self.assertEqual(result['id'],'desktop:stored')
                self.assertEqual([m for m,_ in c.calls].count('prompt.submit'),1)
                self.assertTrue(g.native.enabled(root))
                self.assertFalse(g.native.enabled(Path(folder)))
            finally:g.native.close();g.db.close()
    def test_wrong_native_cwd_stops_before_prompt(self):
        with tempfile.TemporaryDirectory() as folder:
            g,root,p,t,c=self.setup_gateway(folder,bad_cwd=True)
            try:
                with self.assertRaisesRegex(ValueError,'mismatch'):g.native.submit(t,p,'test')
                self.assertNotIn('prompt.submit',[m for m,_ in c.calls])
                self.assertIsNone(g.native.binding(t['id']))
            finally:g.native.close();g.db.close()
    def test_uncertain_submit_recovers_by_reading_same_session_without_resend(self):
        with tempfile.TemporaryDirectory() as folder:
            g,root,p,t,c=self.setup_gateway(folder,uncertain=True)
            try:
                with self.assertRaisesRegex(RuntimeError,'reply lost'):g.native.submit(t,p,'test')
                binding=g.native.binding(t['id'])
                self.assertEqual(binding['stored'],'stored')
                for _ in range(2):g.native.poll(g.task(t['id']))
                self.assertEqual([m for m,_ in c.calls].count('prompt.submit'),1)
            finally:g.native.close();g.db.close()
    def test_completed_native_reply_closes_transport(self):
        with tempfile.TemporaryDirectory() as folder:
            g,root,p,t,c=self.setup_gateway(folder)
            try:
                g.native.submit(t,p,'test')
                old_rpc=c.rpc
                def idle(method,params):
                    data=old_rpc(method,params)
                    if method=='session.resume':data['running']=False
                    return data
                c.rpc=idle
                g.native.evidence=lambda task:{'state':'completed','result':'actual result'}
                result=g.native.poll(g.task(t['id']))
                self.assertEqual(result['status']['state'],'completed')
                self.assertTrue(c.closed)
            finally:g.native.close();g.db.close()
    def test_only_reject_failed_task_never_accept_it(self):
        with tempfile.TemporaryDirectory() as folder:
            g,root,p,t,c=self.setup_gateway(folder)
            try:
                g.update(t['id'],'failed',result='preparation error')
                with self.assertRaises(ValueError):g.dispatch('gateway_review',dict(task_id=t['id'],decision='accepted',evidence='wrong'))
                result=g.dispatch('gateway_review',dict(task_id=t['id'],decision='rejected',evidence='no prompt sent'))
                self.assertEqual(result['review'],'rejected')
            finally:g.native.close();g.db.close()
    def test_native_evidence_cannot_read_a_different_session(self):
        with tempfile.TemporaryDirectory() as folder:
            dbfile=Path(folder)/'history.db';db=sqlite3.connect(dbfile)
            db.executescript('CREATE TABLE messages(id INTEGER PRIMARY KEY,session_id TEXT,timestamp REAL,role TEXT,content TEXT,tool_calls TEXT,tool_name TEXT,finish_reason TEXT);')
            db.executemany('INSERT INTO messages VALUES(?,?,?,?,?,?,?,?)',[(1,'other',1,'user','【任务ID】same',None,None,None),(2,'other',2,'assistant','other-private-result',None,None,'stop'),(3,'ours',3,'user','【任务ID】same',None,None,None),(4,'ours',4,'assistant','ours-result',None,None,'stop')])
            db.commit();db.close()
            result=observe('same',dbfile,session_id='ours')
            self.assertEqual(result['session_id'],'ours');self.assertEqual(result['result'],'ours-result')

if __name__=='__main__':unittest.main()

