"""Local two-executor workbench. Existing Hermes gateway remains authoritative."""
import json, os, secrets, sqlite3, sys, threading, time, uuid
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import urllib.request
from antigravity_desktop import Desktop, DEFAULT_MODEL, office_paths

ROOT=Path(__file__).resolve().parent
STATE=ROOT/'state'
STATE.mkdir(exist_ok=True)
LOCK=threading.RLock()
DB=sqlite3.connect(STATE/'workbench.db',check_same_thread=False)
DB.row_factory=sqlite3.Row
DB.executescript('''CREATE TABLE IF NOT EXISTS tasks(id TEXT PRIMARY KEY, workspace TEXT,
 objective TEXT,boundary TEXT,acceptance TEXT,model TEXT,status TEXT,result TEXT,created REAL,
 request_key TEXT UNIQUE,review TEXT);
 CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY,task TEXT,kind TEXT,text TEXT,created REAL);''')
if 'parent' not in [r[1] for r in DB.execute('PRAGMA table_info(tasks)')]:
    DB.execute('ALTER TABLE tasks ADD COLUMN parent TEXT')
for column in ('project_root','transport','conversation_id','native_project_id','native_model','prompt_state'):
    if column not in [r[1] for r in DB.execute('PRAGMA table_info(tasks)')]:
        DB.execute('ALTER TABLE tasks ADD COLUMN '+column+' TEXT')
DB.execute("UPDATE tasks SET status='uncertain' WHERE status='running'")
DB.commit()
ACTIVE={}
AUTH=STATE/'access.token'
if not AUTH.exists():AUTH.write_text(secrets.token_hex(32))
TOKEN=AUTH.read_text().strip()

def fields(task,**values):
    with LOCK:
        DB.execute('UPDATE tasks SET '+','.join(k+'=?' for k in values)+' WHERE id=?',(*values.values(),task))
        DB.commit()

def hermes(name,**arguments):
    token=Path(r'Z:\Hermes\project-gateway\state\gateway.token').read_text().strip()
    req=urllib.request.Request('http://127.0.0.1:18493',json.dumps(dict(name=name,arguments=arguments)).encode(),
        {'Authorization':'Bearer '+token,'Content-Type':'application/json'})
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req,timeout=15) as r:
        data=json.load(r)
    if 'error' in data:raise ValueError(data['error'])
    return data['result']

def change(task,status,result=None):
    with LOCK:
        DB.execute('UPDATE tasks SET status=?,result=COALESCE(?,result) WHERE id=?',(status,result,task))
        DB.commit()

def event(task,kind,text):
    with LOCK:
        DB.execute('INSERT INTO events(task,kind,text,created) VALUES(?,?,?,?)',(task,kind,text,time.time()))
        DB.commit()

def run(task,recover=False):
    attempted=recover
    try:
        client=Desktop(STATE,lambda kind,text:event(task['id'],kind,text))
        if recover:
            cid=task['conversation_id'];native_model=task['native_model']
            if not cid or not native_model:raise RuntimeError('恢复记录缺失；未重新发送任务')
            event(task['id'],'reconnect','仅恢复桌面任务观察，不重发提示词')
        else:
            model=client.model(task['model'])
            project=client.ensure_project(task['project_root'])
            native_model=model['native_model'];cid=task['conversation_id']
            fields(task['id'],native_project_id=project['id'],native_model=native_model)
            event(task['id'],'desktop_binding',json.dumps(dict(project=project,workspace=task['workspace'],
                      conversation_id=cid,model=model),ensure_ascii=False))
            client.start(cid,project,model,'Codex派工 · '+Path(task['project_root']).name+' · '+task['objective'][:32])
            prompt=(f"用户授权任务。所属项目根目录：{task['project_root']}\n本次办公区及工作目录：{task['workspace']}\n目标：{task['objective']}\n"
                f"边界：{task['boundary']}\n验收：{task['acceptance']}\n"
                '所有新增成果写入本次办公区；先将命令工作目录切换到该办公区。原始资料只在明确授权范围内就地读取，不复制整个项目。'
                '保留桌面原生审批，不自行扩大授权。只执行本任务，不读取密钥，不发送外部消息，不转委托，不强制结束进程。'
                '使用简体中文报告实际成果和未完成项，完成不代表已通过Codex验收。')
            fields(task['id'],prompt_state='attempting')
            attempted=True
            client.send(cid,model,prompt)
            fields(task['id'],prompt_state='sent')
            event(task['id'],'desktop_sent','任务已发送到Antigravity桌面会话 '+cid)
        seen={}
        while True:
            progress=client.progress(cid,native_model)
            for step in progress['events']:
                key=step['index'];signature=(step['status'],step['text'])
                if seen.get(key)!=signature:
                    event(task['id'],step['kind'],step['text']);seen[key]=signature
            change(task['id'],progress['status'],progress['result'] or None)
            if progress['status'] in ('completed','failed'):
                event(task['id'],'desktop_finished',json.dumps({k:v for k,v in progress.items() if k not in ('events','result')},ensure_ascii=False))
                break
            time.sleep(3)
    except Exception as exc:
        change(task['id'],'uncertain' if attempted else 'failed',str(exc))
    finally:
        with LOCK:ACTIVE.pop(task['id'],None)

def dispatch(name,a):
    if name=='state':
        try:projects=hermes('gateway_list_projects');tasks=hermes('gateway_list_tasks');online=True
        except Exception:projects=[];tasks=[];online=False
        for t in tasks:t['executor']='hermes'
        with LOCK:tasks += [dict(r,executor='antigravity') for r in DB.execute('SELECT * FROM tasks ORDER BY created DESC LIMIT 100')]
        for t in tasks:
            path=t.get('project_root') or t.get('workspace')
            if path and not any(os.path.normcase(p['path'])==os.path.normcase(path) for p in projects):
                projects.append(dict(id='agy-'+t['id'],path=path))
        return dict(projects=projects,tasks=sorted(tasks,key=lambda t:t['created'],reverse=True),
                    executors=[dict(id='codex',name='Codex',role='派工与独立验收',state='当前聊天'),
                    dict(id='hermes',name='大肥鱼',role='本地执行',state='已连接' if online else '未连接'),
                    dict(id='antigravity',name='Antigravity',role='桌面原生会话 · Gemini 4 Argon High',state='桌面默认；派工时实时核验',transport='antigravity-desktop')])
    if name=='task':
        if a['executor']=='hermes':return hermes('gateway_get_task',task_id=a['task_id'])
        with LOCK:
            row=DB.execute('SELECT * FROM tasks WHERE id=?',(a['task_id'],)).fetchone()
            if not row:raise ValueError('任务不存在')
            if row['status'] in ('running','waiting_approval','uncertain') and row['transport']=='antigravity-desktop' and row['conversation_id'] and row['prompt_state'] in ('attempting','sent') and row['id'] not in ACTIVE:
                ACTIVE[row['id']]=True
                threading.Thread(target=run,args=(dict(row),True)).start()
            return dict(row,events=[dict(r) for r in DB.execute('SELECT * FROM events WHERE task=? ORDER BY seq',(a['task_id'],))])
    if name=='submit':
        path=Path(a['workspace']).resolve(strict=True)
        if not path.is_dir() or not Path(a['workspace']).is_absolute():raise ValueError('选择已存在的项目绝对路径')
        for k in ('objective','boundary','acceptance','request_key'):
            if not isinstance(a.get(k),str) or not a[k].strip() or len(a[k])>20000:raise ValueError('请填写'+k)
        if a['executor']=='hermes':
            project=hermes('gateway_ensure_project',workspace=str(path))
            return hermes('gateway_submit',project_id=project['id'],**{k:a[k] for k in ('objective','boundary','acceptance','request_key')})
        if a['executor']!='antigravity':raise ValueError('执行端无效')
        with LOCK:
            old=DB.execute('SELECT * FROM tasks WHERE request_key=?',(a['request_key'],)).fetchone()
            if old:
                root,office=office_paths(str(path),a.get('project_root'),old['id']) if old['transport'] else (str(path),str(path))
                if any(old[k]!=a[k] for k in ('objective','boundary','acceptance')) or old['workspace']!=office or old['parent']!=a.get('parent_id') or (a.get('model') and old['model']!=a['model']):raise ValueError('重复标识对应不同任务')
                return dict(old)
            if ACTIVE:raise ValueError('Antigravity 正在执行任务，完成后再派工')
            if DB.execute("SELECT 1 FROM tasks WHERE transport='antigravity-desktop' AND status IN ('running','waiting_approval','uncertain') LIMIT 1").fetchone():
                raise ValueError('存在未确认结束的桌面任务，先读取其状态，不重复派工')
            tid=uuid.uuid4().hex
            root,office=office_paths(str(path),a.get('project_root'),tid)
            if a.get('parent_id'):
                parent=DB.execute('SELECT * FROM tasks WHERE id=?',(a['parent_id'],)).fetchone()
                if not parent or (parent['project_root'] or parent['workspace'])!=root or parent['status'] not in ('completed','failed','blocked'):
                    raise ValueError('返工须关联本项目已经结束的任务')
            model=a.get('model') or DEFAULT_MODEL
            Path(office).mkdir(parents=True,exist_ok=True)
            DB.execute('INSERT INTO tasks(id,workspace,objective,boundary,acceptance,model,status,created,request_key,review,parent,project_root,transport,conversation_id,prompt_state) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
                       (tid,office,a['objective'],a['boundary'],a['acceptance'],model,'running',time.time(),a['request_key'],'pending',a.get('parent_id'),root,'antigravity-desktop',str(uuid.uuid4()),'not_sent'))
            DB.commit()
            task=dict(DB.execute('SELECT * FROM tasks WHERE id=?',(tid,)).fetchone())
            # Reserve before spawning to keep concurrent requests serialized.
            ACTIVE[tid]=None
            threading.Thread(target=run,args=(task,)).start()
            return task
    if name=='review':
        with LOCK:
            task=DB.execute('SELECT * FROM tasks WHERE id=?',(a['task_id'],)).fetchone()
            if not task or task['status'] not in ('completed','failed','blocked') or a.get('decision') not in ('accepted','rejected') or not a.get('evidence') or (a.get('decision')=='accepted' and task['status']!='completed'):
                raise ValueError('需已交付任务、验收决定和实际核验依据')
            DB.execute('UPDATE tasks SET review=? WHERE id=?',(a['decision'],a['task_id']));DB.commit()
            event(a['task_id'],'review',a['decision']+': '+a['evidence'])
            return dict(DB.execute('SELECT * FROM tasks WHERE id=?',(a['task_id'],)).fetchone())
    raise ValueError('未知操作')

def serve():
    class Handler(BaseHTTPRequestHandler):
        def log_message(self,*a):pass
        def send(self,code,data,kind='application/json'):
            raw=data if isinstance(data,bytes) else json.dumps(data,ensure_ascii=False).encode()
            self.send_response(code);self.send_header('Content-Type',kind+'; charset=utf-8')
            self.send_header('Cache-Control','no-store');self.send_header('X-Content-Type-Options','nosniff')
            self.send_header('Content-Length',str(len(raw)));self.end_headers();self.wfile.write(raw)
        def do_GET(self):
            if self.headers.get('Host') not in ('127.0.0.1:18500','localhost:18500'):
                return self.send(403,{'error':'本机工作台'})
            if self.path!='/':return self.send(404,{'error':'not found'})
            # Local-only first version. Public access must add a login layer before publishing.
            page=(ROOT/'index.html').read_text(encoding='utf-8').replace('__ACCESS_TOKEN__',TOKEN)
            self.send(200,page.encode(),'text/html')
        def do_POST(self):
            if not secrets.compare_digest(self.headers.get('Authorization',''),'Bearer '+TOKEN):
                return self.send(401,{'error':'需要本机授权'})
            try:
                size=int(self.headers.get('Content-Length',0))
                if not 0<size<=100000:raise ValueError('请求大小无效')
                a=json.loads(self.rfile.read(size))
                if a['name']=='_shutdown':
                    if ACTIVE:raise ValueError('执行中，请等待正常结束')
                    threading.Thread(target=server.shutdown).start()
                    result={'stopping':True}
                else:result=dispatch(a['name'],a.get('arguments',{}))
                self.send(200,{'result':result})
            except Exception as exc:self.send(400,{'error':str(exc)})
    server=ThreadingHTTPServer(('127.0.0.1',18500),Handler)
    (STATE/'process.json').write_text(json.dumps(dict(pid=os.getpid(),path=sys.executable,started=time.time(),task='用户保留的双执行端工作台',keep_running=True)))
    try:server.serve_forever()
    finally:server.server_close()

if __name__=='__main__':serve()
