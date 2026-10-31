"""stdio adapter for Codex to direct the Antigravity execution lane."""
import json,sys
from pathlib import Path
from urllib.request import Request,build_opener,ProxyHandler
ROOT=Path(__file__).resolve().parent
SPECS={
 'workbench_status':('查询双执行端工作台状态、项目和任务，不执行业务。','state',{}),
 'antigravity_get_task':('读取本工作台派给Antigravity桌面会话的任务、办公区、会话ID及执行动态。','task',{'task_id':{'type':'string'}}),
 'antigravity_submit':('默认派发到Antigravity桌面原生会话，禁止回退CLI。workspace填实际项目根目录，自动在项目内分配办公区；显式办公区可配project_root。提供范围、验收和唯一请求键，完成后独立核验，禁止递归转派。','submit',
    {k:{'type':'string'} for k in ['workspace','objective','boundary','acceptance','request_key','model','parent_id','project_root']}),
 'antigravity_review':('实际检查文件和验证成果后记录独立验收依据；返工关联原任务。','review',{k:{'type':'string'} for k in ['task_id','decision','evidence']}),
}
sys.stdin.reconfigure(encoding='utf-8');sys.stdout.reconfigure(encoding='utf-8')
for line in sys.stdin:
 try:
  r=json.loads(line)
  if 'id' not in r:continue
  if r['method']=='initialize':result=dict(protocolVersion='2024-11-05',capabilities={'tools':{}},serverInfo=dict(name='collaboration-workbench',version='0.2.0'))
  elif r['method']=='tools/list':result={'tools':[dict(name=n,description=s[0],inputSchema=dict(type='object',properties=s[2],required=[k for k in s[2] if k not in ('model','parent_id','project_root')],additionalProperties=False)) for n,s in SPECS.items()]}
  elif r['method']=='tools/call':
   p=r['params']; spec=SPECS[p['name']];a=p.get('arguments',{})
   if spec[1] in ('task','submit'):a['executor']='antigravity'
   req=Request('http://127.0.0.1:18500/api',json.dumps(dict(name=spec[1],arguments=a)).encode(),{'Content-Type':'application/json','Authorization':'Bearer '+(ROOT/'state/access.token').read_text().strip()})
   with build_opener(ProxyHandler({})).open(req,timeout=20) as response:reply=json.load(response)
   result={'content':[dict(type='text',text=json.dumps(reply,ensure_ascii=False))],'isError':'error' in reply}
  elif r['method']=='ping':result={}
  else:raise ValueError('未知方法')
  reply=dict(jsonrpc='2.0',id=r['id'],result=result)
 except Exception as exc:reply=dict(jsonrpc='2.0',id=locals().get('r',{}).get('id'),error=dict(code=-32603,message=str(exc)))
 print(json.dumps(reply,ensure_ascii=False),flush=True)
