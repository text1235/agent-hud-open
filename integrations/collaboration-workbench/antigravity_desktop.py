"""Native Antigravity Desktop transport. Never launches the standalone agy CLI."""
import json, os, re, struct, subprocess, sys, time, uuid
from pathlib import Path
from urllib.parse import unquote, urlparse
from urllib.request import Request, build_opener, ProxyHandler
from urllib.error import HTTPError

DEFAULT_MODEL='gemini-4-argon-high'

def canonical(path):
    return os.path.normcase(str(Path(path).resolve(strict=True)))

def uri_path(uri):
    parsed=urlparse(uri)
    if parsed.scheme!='file' or parsed.netloc not in ('','localhost'):
        raise ValueError('只接受本机项目目录')
    return os.path.normcase(str(Path(unquote(parsed.path).lstrip('/')).resolve()))

def project_paths(project):
    return [uri_path(x['folderUri']) for x in project.get('projectResources',{}).get('resources',[]) if x.get('folderUri')]

def office_paths(workspace, project_root, task_id):
    path=Path(workspace).resolve(strict=True)
    if project_root:
        root=Path(project_root).resolve(strict=True)
    else:
        root=path
        for marker in ('协作办公区','协同办公区'):
            if marker in path.parts:
                root=Path(*path.parts[:path.parts.index(marker)])
                break
    if not path.is_dir() or not root.is_dir() or not path.is_relative_to(root):
        raise ValueError('办公区必须位于所属项目根目录内')
    office=root/'协作办公区'/'Antigravity'/task_id if path==root else path
    if not office.resolve().is_relative_to(root):
        raise ValueError('办公区目录联接指向了其他项目')
    return str(root),str(office)

class Desktop:
    def __init__(self,state,event=lambda kind,text:None):
        self.state=Path(state);self.event=event
        self.opener=build_opener(ProxyHandler({}))
        self.exe=Path(os.environ['LOCALAPPDATA'])/'Programs/antigravity/resources/bin/language_server.exe'
        query=("Get-CimInstance Win32_Process -Filter \"name='language_server.exe'\" -ErrorAction Stop | "
               "Where-Object { $_.ExecutablePath -eq '"+str(self.exe).replace("'","''")+"' -and "
               "$_.CommandLine -match '--app_data_dir antigravity' -and $_.CommandLine -match '--subclient_type hub' } | "
               "Select-Object ProcessId,CommandLine | ConvertTo-Json -Compress")
        shell=Path(sys.executable).parent.parent/'native/powershell/pwsh.exe'
        if not shell.exists():shell=Path(os.environ['SystemRoot'])/'System32/WindowsPowerShell/v1.0/powershell.exe'
        proc=subprocess.Popen([str(shell),'-NoProfile','-Command',query],stdin=subprocess.DEVNULL,
             stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,encoding='utf-8',errors='replace',
             creationflags=subprocess.CREATE_NO_WINDOW)
        self.event('connection_probe',json.dumps({'pid':proc.pid,'path':str(shell),'started':time.time(),
                                                'task':'Antigravity本机连接发现'}))
        probe_record=self.state/('desktop-probe-'+str(proc.pid)+'.json')
        probe_info={'pid':proc.pid,'path':str(shell),'started':time.time(),'task':'Antigravity本机连接发现'}
        probe_record.write_text(json.dumps(probe_info),'utf-8')
        try:out,err=proc.communicate()
        finally:
            proc.wait()
            probe_info.update(exit_code=proc.returncode,exited=time.time())
            probe_record.write_text(json.dumps(probe_info),'utf-8')
            self.event('connection_probe_exit',str(proc.returncode))
        if proc.returncode or not out.strip():
            raise RuntimeError('Antigravity桌面未运行或本机连接发现不可用；未回退CLI')
        found=json.loads(out)
        if isinstance(found,list):
            raise RuntimeError('发现多套Antigravity桌面服务，请先明确目标；未自动选择其他实例')
        match=re.search(r'--csrf_token\s+(\S+)',found['CommandLine'])
        if not match:raise RuntimeError('桌面连接参数缺失')
        self.token=match.group(1);self.pid=found['ProcessId']
        log=Path(os.environ['APPDATA'])/'Antigravity/logs/language_server.log'
        ports=re.findall(r'listening on \w+ port at (\d+) for HTTP\b',log.read_text('utf-8',errors='replace'))
        if not ports:raise RuntimeError('未找到桌面本机服务端口')
        self.base='http://127.0.0.1:'+ports[-1]+'/exa.language_server_pb.LanguageServerService/'

    def request(self,method,body,stream=False):
        data=json.dumps(body,ensure_ascii=False).encode('utf-8')
        headers={'Content-Type':'application/json','x-codeium-csrf-token':self.token}
        if stream:
            data=b'\0'+struct.pack('>I',len(data))+data
            headers['Content-Type']='application/connect+json'
            headers['Connect-Protocol-Version']='1'
        try:
            with self.opener.open(Request(self.base+method,data,headers),timeout=25) as response:
                if stream:
                    frame=response.read(5)
                    if len(frame)!=5:raise RuntimeError('桌面项目列表未返回有效数据')
                    size=struct.unpack('>I',frame[1:])[0]
                    result=json.loads(response.read(size))
                    if frame[0]&2:raise RuntimeError('桌面项目列表流提前结束')
                    return result
                return json.load(response)
        except HTTPError as error:
            detail=error.read().decode('utf-8','replace').replace(self.token,'[redacted]')
            raise RuntimeError(method+' HTTP '+str(error.code)+': '+detail[:1000]) from None

    def model(self,requested):
        models=self.request('GetCascadeModelConfigData',{}).get('clientModelConfigs',[])
        matches=[m for m in models if m.get('modelId')==requested]
        if len(matches)!=1 or not matches[0].get('modelOrAlias',{}).get('model'):
            raise ValueError('桌面模型目录中找不到精确模型 '+requested+'；未静默替换')
        m=matches[0]
        return {'id':requested,'native_model':m['modelOrAlias']['model'],'label':m.get('label',requested)}

    def ensure_project(self,root):
        root=Path(root).resolve(strict=True)
        ids=self.request('ProjectUpdatesStream',{},stream=True).get('projectList',{}).get('projectIds')
        if ids is None:raise RuntimeError('桌面项目列表缺失，停止派工')
        projects=self.request('ReadProjects',{'ids':ids}).get('projects',[]) if ids else []
        matches=[]
        for candidate in projects:
            try:
                if project_paths(candidate)==[canonical(root)]:matches.append(candidate)
            except (ValueError,OSError):continue
        if len(matches)>1:raise ValueError('同一路径对应多个桌面项目，需要明确绑定，未使用最近项目')
        if matches:project=matches[0]
        else:
            pid=str(uuid.uuid5(uuid.NAMESPACE_URL,root.as_uri().lower()))
            self.request('CreateProject',{'project':{'id':pid,'name':root.name,
                         'projectResources':{'resources':[{'folderUri':root.as_uri()}]}}})
            project=self.request('ReadProject',{'id':pid}).get('project',{})
        verified=self.request('ReadProject',{'id':project['id']}).get('project',{})
        if project_paths(verified)!=[canonical(root)]:raise ValueError('桌面项目目录与任务所属项目不一致')
        return {'id':verified['id'],'name':verified.get('name',root.name),'root':str(root)}

    def start(self,cid,project,model,title):
        response=self.request('StartCascade',{'cascadeId':cid,'requestedModel':model['native_model'],
          'source':'CORTEX_TRAJECTORY_SOURCE_AGENT_API','trajectoryType':'CORTEX_TRAJECTORY_TYPE_CASCADE',
          'projectEnvConfig':{'projectId':project['id'],'defaultProjectEnvironment':{}},
          'customAgentSpec':{'codingAgent':{'googleMode':True},
              'cascadeConfig':{'plannerConfig':{'requestedModel':{'model':model['native_model']}}}}})
        if response.get('cascadeId')!=cid:raise RuntimeError('桌面返回了不同的会话ID；未发送任务')
        self.verify_conversation(cid,project)
        self.request('UpdateConversationAnnotations',{'cascadeIds':[cid],'mergeAnnotations':True,
                                                      'annotations':{'title':title}})

    def verify_conversation(self,cid,project):
        meta=self.request('GetConversationMetadata',{'conversationId':cid}).get('metadata',{})
        if meta.get('projectId')!=project['id'] or [uri_path(x) for x in meta.get('workspaceUris',[])]!=[canonical(project['root'])]:
            raise RuntimeError('桌面会话项目归属核验失败；停止派工')
        return meta

    def send(self,cid,model,prompt):
        return self.request('SendUserCascadeMessage',{'cascadeId':cid,'items':[{'text':prompt}],
             'cascadeConfig':{'plannerConfig':{'requestedModel':{'model':model['native_model']}}}})

    def progress(self,cid,expected_model):
        data=self.request('GetCascadeTrajectory',{'cascadeId':cid,'trajectoryVerbosity':'CLIENT_TRAJECTORY_VERBOSITY_PROD_UI'})
        steps=data.get('trajectory',{}).get('steps',[])
        events=[];answers=[];errors=[];models=set();waiting=False
        for index,step in enumerate(steps):
            meta=step.get('metadata',{})
            actual=meta.get('generatorModel') or meta.get('modelUsage',{}).get('model')
            if actual and step.get('plannerResponse'):models.add(actual)
            if step.get('requestedInteraction') and step.get('status') not in ('CORTEX_STEP_STATUS_DONE','CORTEX_STEP_STATUS_CANCELED'):
                waiting=True
            response=step.get('plannerResponse',{}).get('response') or ''
            if response:answers.append(response)
            notify=step.get('notifyUser',{}).get('notificationContent') or ''
            if notify:answers.append(notify)
            failure=step.get('errorMessage',{}).get('error') or step.get('error')
            if failure:errors.append(json.dumps(failure,ensure_ascii=False))
            kind=step.get('type','activity');status=step.get('status','')
            # Only UI-level response/activity. Never return signatures, raw thinking or debug prompts.
            text=response or notify or kind.replace('CORTEX_STEP_TYPE_','')+' · '+status.replace('CORTEX_STEP_STATUS_','')
            events.append({'index':index,'kind':kind,'status':status,'text':text[:20000]})
        if models and models!={expected_model}:
            errors.append('实际执行模型与指定模型不一致：'+','.join(sorted(models)))
        run_status=data.get('status','unknown')
        terminal=run_status=='CASCADE_RUN_STATUS_IDLE' and not waiting and bool(answers or errors or any(s.get('status')=='CORTEX_STEP_STATUS_CANCELED' for s in steps))
        if terminal and answers and not models:errors.append('桌面未返回实际执行模型，不能确认指定模型已执行')
        status=('failed' if errors else 'completed' if answers else 'failed') if terminal else ('waiting_approval' if waiting else 'running')
        result='\n\n'.join(answers)
        if terminal and errors:result+='\n\n错误：'+'\n'.join(errors)
        if terminal and not answers and not errors:result='桌面任务已停止，但未返回交付结果，请在桌面检查。'
        return {'status':status,'result':result,'events':events,'models':sorted(models),'native_status':run_status}
