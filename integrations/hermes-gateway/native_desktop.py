"""Hermes Desktop transport for the existing project Gateway; no new service."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
from observations import observe


def key(path):
    return os.path.normcase(str(Path(path).resolve(strict=True)))


class NativeClient:
    def __init__(self, state, config):
        self.state = Path(state)
        publication = Path(config['publication'])
        record = json.loads(publication.read_text('utf-8'))
        if record.get('role') != 'desktop-serve' or record.get('protocolVersion') != 1 or record.get('host') != '127.0.0.1':
            raise ValueError('Unsupported Hermes Desktop rendezvous')
        if config.get('profile', 'default') not in record.get('profiles', []):
            raise ValueError('Hermes profile does not match this connection')
        owners = json.loads(Path(config['ownership']).read_text('utf-8'))['backends']
        pid, port = int(record['pid']), int(record['port'])
        script = ("$p=Get-CimInstance Win32_Process -Filter 'ProcessId="+str(pid)+"'; "
            "$listeners=@(Get-NetTCPConnection -State Listen -LocalPort "+str(port)+" -ErrorAction SilentlyContinue); "
            "[PSCustomObject]@{parent=$p.ParentProcessId;created=([DateTimeOffset]$p.CreationDate).ToUnixTimeMilliseconds();listeners=@($listeners.OwningProcess)} | ConvertTo-Json -Compress")
        shell = shutil.which('pwsh') or 'powershell.exe'
        probe = subprocess.Popen([shell, '-NoProfile', '-Command', script], stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, encoding='utf-8', creationflags=subprocess.CREATE_NO_WINDOW)
        probe_file = self.state / ('native-probe-'+str(probe.pid)+'.json')
        info = dict(task='Hermes native backend identity', path=shell, pid=probe.pid, startedAt=time.time())
        probe_file.write_text(json.dumps(info), 'utf-8')
        try:
            out, err = probe.communicate()
        finally:
            probe.wait()
            info.update(exited=True, exitCode=probe.returncode, endedAt=time.time())
            probe_file.write_text(json.dumps(info), 'utf-8')
        actual = json.loads(out) if probe.returncode == 0 else {}
        if actual.get('parent') not in [o['pid'] for o in owners] or pid not in actual.get('listeners', []) or abs(actual.get('created', 0)/1000-record.get('createTime', 0)) > 1:
            raise ValueError('Hermes Desktop backend identity or listener changed')
        self.backend = dict(pid=pid, port=port)
        node = config.get('node') or shutil.which('node')
        if not node:
            raise RuntimeError('Node 24 is required for the existing desktop WebSocket API')
        helper = Path(__file__).with_name('desktop_rpc.cjs')
        self.process = subprocess.Popen([node, str(helper), '--endpoint', 'http://127.0.0.1:'+str(port)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, encoding='utf-8', bufsize=1, creationflags=subprocess.CREATE_NO_WINDOW)
        self.sequence = 0
        self.log_file = self.state / ('native-rpc-'+str(self.process.pid)+'.json')
        self.log = dict(task='Hermes native Gateway transport', path=node, helper=str(helper), pid=self.process.pid, startedAt=time.time(), backend=self.backend)
        self.log_file.write_text(json.dumps(self.log), 'utf-8')

    def rpc(self, method, params):
        self.sequence += 1
        self.process.stdin.write(json.dumps(dict(id=self.sequence, method=method, params=params), ensure_ascii=False)+'\n')
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError('Hermes native connection closed; operation was not replayed')
        reply = json.loads(line)
        if reply.get('id') != self.sequence:
            raise RuntimeError('Unexpected Hermes native response id')
        if 'error' in reply:
            raise RuntimeError('Hermes native RPC rejected: '+str(reply['error']))
        return reply['result']

    def close(self):
        if self.process.stdin and not self.process.stdin.closed:
            self.process.stdin.close()
        try:
            self.process.wait(10)
        except subprocess.TimeoutExpired:
            self.log['residual'] = True
        self.log.update(exited=self.process.poll() is not None, exitCode=self.process.poll(), endedAt=time.time())
        self.log_file.write_text(json.dumps(self.log), 'utf-8')
        if self.process.stdout:
            self.process.stdout.close()


class NativeTransport:
    def __init__(self, gateway, client_factory=NativeClient):
        self.gateway = gateway
        self.client_factory = client_factory
        self.clients = {}
        gateway.db.execute('CREATE TABLE IF NOT EXISTS native_bindings(task TEXT PRIMARY KEY, stored TEXT, runtime TEXT, project TEXT, cwd TEXT, backend TEXT)')
        gateway.db.commit()

    def config(self):
        path = self.gateway.state / 'native-desktop.json'
        return json.loads(path.read_text('utf-8')) if path.exists() else {}

    def enabled(self, root):
        config = self.config()
        return any(key(root) == key(p) for p in config.get('projects', []))

    def binding(self, task_id):
        with self.gateway.lock:
            row = self.gateway.db.execute('SELECT * FROM native_bindings WHERE task=?', (task_id,)).fetchone()
            return dict(row) if row else None

    def client(self, task_id):
        if task_id not in self.clients:
            self.clients[task_id] = self.client_factory(self.gateway.state, self.config())
        return self.clients[task_id]

    def release(self, task_id):
        client = self.clients.pop(task_id, None)
        if client:
            client.close()

    def submit(self, task, project, message):
        root = Path(project['path']).resolve(strict=True)
        office = root / '协作办公区' / '大肥鱼' / ('hud-'+task['request_key'])
        office.mkdir(parents=True, exist_ok=True)
        if not office.resolve().is_relative_to(root):
            raise ValueError('Hermes office escaped its business project')
        client = self.client(task['id'])
        projects = client.rpc('projects.list', {})['projects']
        matches = [p for p in projects if p.get('primary_path') and key(p['primary_path']) == key(root)]
        if len(matches) > 1:
            raise ValueError('Multiple native Hermes projects match this root')
        native_project = matches[0] if matches else client.rpc('projects.create', dict(name=root.name, folders=[str(root)], primary_path=str(root), use=False))['project']
        resolved = client.rpc('projects.for_cwd', dict(cwd=str(office)))
        if not resolved.get('project') or resolved['project']['id'] != native_project['id']:
            raise ValueError('Native Hermes office belongs to a different project')
        config = self.config()
        created = client.rpc('session.create', dict(cwd=str(office), cwd_explicit=True, source='desktop',
            close_on_disconnect=False, title='Codex派工 · '+root.name+' · '+task['objective'][:32],
            idempotency_key='project-gateway-'+task['id'], model=config['model'], provider=config['provider']))
        live = client.rpc('session.activate', dict(session_id=created['session_id'], omit_messages=True))
        info = live['info']
        if key(info['cwd']) != key(office) or (info.get('project') or {}).get('id') != native_project['id'] or info['model'] != config['model']:
            raise ValueError('Hermes native session cwd, project or model mismatch; prompt not submitted')
        binding = dict(task=task['id'], stored=created['stored_session_id'], runtime=created['session_id'], project=native_project['id'], cwd=str(office), backend=json.dumps(client.backend))
        with self.gateway.lock:
            self.gateway.db.execute('INSERT INTO native_bindings VALUES(:task,:stored,:runtime,:project,:cwd,:backend)', binding)
            self.gateway.update(task['id'], 'submitting', remote='desktop:'+binding['stored'])
            self.gateway.event(task['id'], 'native_binding', json.dumps(binding, ensure_ascii=False))
            self.gateway.db.commit()
        # Durable binding precedes the only model submission. Unknown replies are never replayed.
        ack = client.rpc('prompt.submit', dict(session_id=binding['runtime'], text=message+'\n【所属项目根目录】'+str(root)+'\n【本次办公区与真实工作目录】'+str(office)))
        if ack.get('status') != 'streaming' or not ack.get('user_row_id'):
            raise RuntimeError('Hermes did not confirm a new native input; inspect session, do not resend')
        with self.gateway.lock:
            self.gateway.event(task['id'], 'native_prompt_accepted', str(ack['user_row_id']))
            self.gateway.db.commit()
        return self.poll(dict(task, remote='desktop:'+binding['stored']))

    def evidence(self, task):
        binding = self.binding(task['id'])
        if not binding:
            return dict(state='unknown', reason='native binding missing')
        evidence = observe(task['id'], session_id=binding['stored'])
        evidence.update(transport='hermes-desktop', cwd=binding['cwd'], native_project_id=binding['project'])
        return evidence

    def poll(self, task):
        binding = self.binding(task['id'])
        if not binding:
            raise RuntimeError('Native binding missing; no prompt was replayed')
        client = self.client(task['id'])
        snapshot = client.rpc('session.resume', dict(session_id=binding['stored'], lazy=True, omit_messages=True, inline_images=False, close_on_disconnect=False))
        info = snapshot['info']
        config = self.config()
        if key(info['cwd']) != key(binding['cwd']) or (info.get('project') or {}).get('id') != binding['project'] or info['model'] != config['model']:
            raise RuntimeError('Native session binding changed; inspect before continuing')
        evidence = self.evidence(task)
        inflight = snapshot.get('inflight') or {}
        waiting = snapshot.get('pending_approval') or snapshot.get('open_requests')
        running = snapshot.get('running', info.get('running', False))
        state = 'input-required' if waiting else 'working'
        result = ''
        if not running and inflight.get('error'):
            state, result = 'failed', inflight['error']
        elif not running and evidence.get('state') == 'completed':
            state, result = 'completed', evidence['result']
        if state in ('completed', 'failed'):
            self.release(task['id'])
        return dict(id=task['remote'], status=dict(state=state, message=dict(parts=[dict(kind='text', text=result)])))

    def close(self):
        for task_id in list(self.clients):
            self.release(task_id)
