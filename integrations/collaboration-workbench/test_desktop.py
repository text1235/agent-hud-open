"""Offline dispatch contracts; never connects to an agent or starts a process."""
import importlib.util,json,pathlib,sqlite3,sys,tempfile,unittest
from unittest.mock import patch
ROOT=pathlib.Path(__file__).resolve().parent
sys.path.insert(0,str(ROOT))
from antigravity_desktop import Desktop,office_paths

class DesktopContracts(unittest.TestCase):
    def setUp(self):
        (ROOT/'state').mkdir(exist_ok=True)
        self.temp=tempfile.TemporaryDirectory(prefix='desktop-contract-',dir=ROOT/'state')
        self.project=pathlib.Path(self.temp.name)
        self.db=sqlite3.connect(':memory:',check_same_thread=False)
        spec=importlib.util.spec_from_file_location('server_under_test',ROOT/'server.py')
        self.server=importlib.util.module_from_spec(spec)
        with patch('sqlite3.connect',return_value=self.db):spec.loader.exec_module(self.server)
        self.args={'executor':'antigravity','workspace':str(self.project),'objective':'test',
                   'boundary':'only test','acceptance':'real check later','request_key':'unique-test'}
    def tearDown(self):
        self.db.close();self.temp.cleanup()
    def submit(self,**overrides):
        with patch.object(self.server.threading,'Thread'):
            return self.server.dispatch('submit',{**self.args,**overrides})
    def test_project_office_and_idempotency(self):
        task=self.submit()
        self.assertEqual(task['project_root'],str(self.project))
        self.assertEqual(pathlib.Path(task['workspace']),self.project/'协作办公区/Antigravity'/task['id'])
        self.assertEqual(task['transport'],'antigravity-desktop')
        self.assertEqual(self.submit()['id'],task['id'])
        with self.assertRaises(ValueError):self.submit(boundary='different')
    def test_reject_cross_project_office(self):
        (self.project/'协作办公区').mkdir()
        with self.assertRaises(ValueError):office_paths(str(self.project),str(self.project/'协作办公区'),'x')
    def test_selected_model_does_not_break_request_idempotency(self):
        task=self.submit(model='auto')
        self.server.fields(task['id'],model='gemini-3.8-flash-high')
        self.assertEqual(self.submit(model='auto')['id'],task['id'])
        with self.assertRaises(ValueError):self.submit(model='gemini-4-argon-high')
    def test_native_status_and_model_checks(self):
        client=object.__new__(Desktop)
        state={'status':'CASCADE_RUN_STATUS_IDLE','trajectory':{'steps':[{'type':'CORTEX_STEP_TYPE_USER_INPUT','status':'CORTEX_STEP_STATUS_DONE'}]}}
        client.request=lambda *args:state
        self.assertEqual(client.progress('own','MODEL_A')['status'],'running')
        state['trajectory']['steps'].append({'type':'CORTEX_STEP_TYPE_PLANNER_RESPONSE','status':'CORTEX_STEP_STATUS_DONE',
                                           'metadata':{'generatorModel':'MODEL_A'},'plannerResponse':{'response':'done','thinkingSignature':'secret'}})
        self.assertEqual(client.progress('own','MODEL_A')['status'],'completed')
        self.assertNotIn('secret',json.dumps(client.progress('own','MODEL_A')))
        self.assertEqual(client.progress('own','MODEL_B')['status'],'failed')
        state['trajectory']['steps'].append({'status':'CORTEX_STEP_STATUS_WAITING','requestedInteraction':{'type':'approval'}})
        self.assertEqual(client.progress('own','MODEL_A')['status'],'waiting_approval')
    def test_uncertain_send_never_replayed(self):
        task=self.submit();counts={'sends':0,'starts':0}
        class Fake:
            def __init__(self,*args):pass
            def model(self,x):return {'id':'selected-model','native_model':'MODEL_A'}
            def ensure_project(self,x):return {'id':'native-project','root':x}
            def start(self,*args):counts['starts']+=1
            def send(self,*args):
                counts['sends']+=1
                raise RuntimeError('transport uncertain after sending')
            def progress(self,*args):return {'status':'completed','events':[],'result':'done','models':['MODEL_A']}
        with patch.object(self.server,'Desktop',Fake):
            self.server.run(task)
            row=dict(self.db.execute('SELECT * FROM tasks').fetchone())
            self.assertEqual(row['status'],'uncertain')
            self.assertEqual(row['prompt_state'],'attempting')
            with self.assertRaises(ValueError):self.submit(request_key='second-task')
            self.server.run(row,recover=True)
        self.assertEqual(counts,{'sends':1,'starts':1})
        self.assertEqual(self.db.execute('SELECT status FROM tasks').fetchone()[0],'completed')
    def test_offline_desktop_fails_without_cli_fallback(self):
        task=self.submit()
        with patch.object(self.server,'Desktop',side_effect=RuntimeError('desktop unavailable')):
            self.server.run(task)
        row=self.db.execute('SELECT status,prompt_state FROM tasks').fetchone()
        self.assertEqual(tuple(row),('failed','not_sent'))
        self.assertNotIn('AGY=',(ROOT/'server.py').read_text('utf-8'))
    def test_live_selection_is_persisted_and_no_quota_never_sends(self):
        task=self.submit(model='gemini-4-argon-high');sent=[]
        configs=[{'modelId':'gemini-4-argon-high','modelOrAlias':{'model':'ARGON'},'quotaInfo':{'remainingFraction':.10}},
                 {'modelId':'gemini-3.8-flash-high','modelOrAlias':{'model':'FLASH'},'quotaInfo':{'remainingFraction':1}}]
        class Fake(Desktop):
            def __init__(self,*args):pass
            def request(self,*args):return {'clientModelConfigs':configs}
            def ensure_project(self,root):return {'id':'native-project','root':root}
            def start(self,*args):pass
            def send(self,cid,model,prompt):sent.append(model['native_model'])
            def progress(self,*args):return {'status':'completed','events':[],'result':'done','models':['FLASH']}
        with patch.object(self.server,'Desktop',Fake):self.server.run(task)
        row=dict(self.db.execute('SELECT * FROM tasks').fetchone())
        self.assertEqual((row['model'],row['native_model'],row['requested_model']),
                         ('gemini-3.8-flash-high','FLASH','gemini-4-argon-high'))
        self.assertEqual(sent,['FLASH'])
        selection=json.loads(self.db.execute("SELECT text FROM events WHERE kind='model_selection'").fetchone()[0])
        self.assertEqual(selection['reason'],'requested_low_or_unavailable')
        # A separate request with every model at zero must fail before start/send.
        for config in configs:config['quotaInfo']['remainingFraction']=0
        task=self.submit(request_key='all-empty')
        with patch.object(self.server,'Desktop',Fake):self.server.run(task)
        row=self.db.execute('SELECT status,prompt_state FROM tasks WHERE id=?',(task['id'],)).fetchone()
        self.assertEqual(tuple(row),('failed','not_sent'))
        self.assertEqual(sent,['FLASH'])

if __name__=='__main__':unittest.main()
