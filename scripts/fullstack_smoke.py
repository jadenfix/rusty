#!/usr/bin/env python3
"""Real Rust CLI + file/Bash tools + local API + browser JS contract.
The offline provider supplies a fixed repair plan. This qualifies execution,
not a model's ability to discover bugs. Gold checks remain outside the workspace.
"""
import argparse
import collections
import http.server
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import time
from urllib.request import Request, urlopen
from urllib.error import HTTPError
from memory_smoke import rpc, stop

ROOT=Path(__file__).resolve().parents[1]
PROMPT='Fix the local task board API and UI. Check whitespace validation, item rendering and persistence; run the tests. Keep the answer short.'
STYLE='Response style: concise, warm, three bullets.'
PLAN=[
    ('read_file',{'path':'app.py'}),
    ('read_file',{'path':'web/app.js'}),
    ('edit_file',{'path':'app.py','old_string':"self.send_json({'tasks':self.server.tasks})",'new_string':"self.send_json({'items':self.server.tasks})"}),
    ('edit_file',{'path':'app.py','old_string':'        title = title  # TODO: trim whitespace and reject an empty title',
                  'new_string':"        title = title.strip()\n        if not title:\n            self.send_json({'error':'title required'},400)\n            return"}),
    ('edit_file',{'path':'web/app.js','old_string':'payload.entries','new_string':'payload.items'}),
    ('bash',{'command':'python3 -m unittest discover -s tests -v'}),
]

class Provider(http.server.BaseHTTPRequestHandler):
    def log_message(self,*_): pass
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if not body.get('stream'):
            # Deep teacher may run in the background; no speculative intervention.
            data=json.dumps({'choices':[{'message':{'content':'{"next_step":"","evidence_ids":[]}'}}]}).encode()
            self.send_response(200);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data);return
        self.server.requests.append(body)
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Connection','close');self.end_headers()
        def send(delta, finish=None):
            event={'choices':[{'delta':delta,'finish_reason':finish}],'usage':{'prompt_tokens':100,'completion_tokens':20}}
            self.wfile.write(('data: '+json.dumps(event)+'\n\n').encode());self.wfile.flush()
        count=sum(m['role']=='tool' for m in body['messages'])
        if count<len(PLAN):
            time.sleep(self.server.pace)
            name,args=PLAN[count]
            send({'tool_calls':[{'index':0,'id':f'fullstack-{count}','type':'function','function':{'name':name,'arguments':json.dumps(args)}}]},'tool_calls')
        else:
            for line in ['- API: trims titles, rejects blanks, and preserves tasks.\n',
                         '- UI: reads items and displays saved tasks safely.\n',
                         '- Checks: 3 API tests passed. Thanks — ready to try locally.\n','FULLSTACK_COMPLETE\n']:
                send({'content':line});time.sleep(self.server.pace)
            send({},'stop')
        self.wfile.write(b'data: [DONE]\n\n');self.wfile.flush()


def verify(project):
    spec=importlib.util.spec_from_file_location('taskboard',project/'app.py')
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    store=project/'heldout-tasks.json';server=module.make_server(data=store)
    thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
    url=f'http://127.0.0.1:{server.server_port}'
    try:
        def post(title):
            return urlopen(Request(url+'/api/tasks',json.dumps({'title':title}).encode(),{'Content-Type':'application/json'}))
        with post('  Browser to disk  ') as r: created=json.load(r);assert r.status==201
        assert created['title']=='Browser to disk'
        with urlopen(url+'/api/tasks') as r: assert json.load(r)['items']==[created]
        try: post('   ');raise AssertionError('blank accepted')
        except HTTPError as e: assert e.code==400
        with post('<script>alert(1)</script>') as r: assert json.load(r)['title']=='<script>alert(1)</script>'
        for asset in ['/','/app.js','/style.css']:
            with urlopen(url+asset) as r: assert r.status==200
    finally: server.shutdown();server.server_close();thread.join()
    restarted=module.make_server(data=store)
    try: assert len(restarted.tasks)==2 and restarted.tasks[0]==created
    finally: restarted.server_close()
    # Exercise the shipped JS with a minimal DOM/fetch host. Actual browser QA
    # is a separate receipt; this check catches contract and unsafe-rendering bugs.
    js="""
const fs=require('fs'),vm=require('vm'),assert=require('assert');
const title='<script>alert(1)</script>',list={children:[],replaceChildren(){this.children=[]},append(x){this.children.push(x)}};
const status={},form={addEventListener(){}};
const document={querySelector(s){return {'#tasks':list,'#status':status,'#task-form':form}[s]},createElement(){return {}}};
const context={document,fetch:async()=>({json:async()=>({items:[{title}]})})};
vm.runInNewContext(fs.readFileSync('web/app.js','utf8'),context);
setTimeout(()=>{assert.equal(list.children.length,1);assert.equal(list.children[0].textContent,title);assert.equal(status.textContent,undefined);console.log('JS contract passed')},30);
"""
    p=subprocess.run(['node','-e',js],cwd=project,capture_output=True,text=True,timeout=5)
    assert p.returncode==0,p.stderr
    return dict(api_roundtrip=True,blank_rejected=True,persistence_restart=True,assets=True,js_render_contract=True,untrusted_title_as_text=True)


def run(a,mode,endpoint,requests):
    with tempfile.TemporaryDirectory(prefix='rf-',dir='/tmp') as tmp:
        home=Path(tmp)/'home';project=Path(tmp)/'project'
        shutil.copytree(ROOT/'demo/fullstack/files',project)
        env={k:v for k,v in os.environ.items() if not k.startswith(('RUSTY_','NVIDIA_API_KEY','AWS_','CLOUDSDK_','TF_','KUBE'))}
        env.update(RUSTY_HOME=str(home),RUSTY_NO_DOTENV='1',RUSTY_PROJECT_ID='memory-smoke',RUSTY_BASE_URL=endpoint,NVIDIA_API_KEY='offline-test-key')
        daemon=subprocess.Popen([str(a.binary.with_name('rusty-memoryd')),'--home',str(home)],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
        try:
            end=time.monotonic()+3
            while not (home/'memory/advisor.sock').exists():
                if daemon.poll() is not None or time.monotonic()>end: raise RuntimeError('startup')
                time.sleep(.01)
            assert not rpc(home,'remember',{'kind':'preference','text':STYLE})['error']
            trace=Path(tmp)/'trajectory.json';before=len(requests);start=time.monotonic()
            args=['--model','offline-scripted','--mode','auto','--agents','off','--memory',mode,'--yolo','--stats','--trajectory',str(trace)]
            if a.cast and mode=='on':
                recording=dict(env,RUSTY_BIN=str(a.binary),RUSTY_RECORD_TYPE_DELAY='0.008')
                proc=subprocess.run(['python3',str(ROOT/'scripts/record.py'),'fullstack',str(a.cast.resolve()),'--',*args],cwd=project,env=recording,capture_output=True,text=True,timeout=60)
            else:
                env['NO_COLOR']='1'
                proc=subprocess.run([str(a.binary),*args,PROMPT],cwd=project,env=env,capture_output=True,text=True,timeout=30)
            assert proc.returncode==0,proc.stderr
            wall=time.monotonic()-start;t=json.loads(trace.read_text())
            calls=[c for m in t['messages'] for c in (m.get('tool_calls') or [])]
            bodies=requests[before:]
            styles=[STYLE in b['messages'][0]['content'] for b in bodies]
            metrics=t.get('memory') or {}
            checks=verify(project)
            checks.update(tool_budget=len(calls)==6,no_repeated_calls=len({json.dumps(c['function'],sort_keys=True) for c in calls})==6,
                          stayed_standard=all('Execution: standard.' in b['messages'][0]['content'] for b in bodies),
                          no_checker=len(bodies)==7,tone_guidance_every_step=all(styles) if mode!='off' else not any(styles),
                          no_hook_timeouts=metrics.get('timeouts',0)==0,hook_deadline=metrics.get('max_hook_us',0)<150000,
                          final_answer_present=any('FULLSTACK_COMPLETE' in str(m.get('content','')) for m in t['messages']))
            if a.workspace and mode=='on':
                assert not a.workspace.exists(),'refuse to replace an existing demo workspace'
                shutil.copytree(project,a.workspace)
            return dict(memory_mode=mode,passed=all(checks.values()),checks=checks,wall_seconds=round(wall,3),tool_calls=len(calls),model_requests=len(bodies),memory=metrics,
                        qualification='scripted provider; real CLI/file/Bash/API/JS; no paid inference')
        finally: stop(daemon)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary',type=Path,default=Path('target/debug/rusty'));p.add_argument('--report',type=Path,required=True)
    p.add_argument('--modes',nargs='+',choices=['off','on','deep'],default=['off','on','deep'])
    p.add_argument('--cast',type=Path);p.add_argument('--workspace',type=Path)
    a=p.parse_args();a.binary=a.binary.resolve()
    if a.cast:a.cast.parent.mkdir(parents=True,exist_ok=True)
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Provider);server.requests=[];server.pace=.5 if a.cast else .02
    threading.Thread(target=server.serve_forever,daemon=True).start()
    runs=[]
    with tempfile.TemporaryDirectory(prefix='rf-base-') as baseline:
        shutil.copytree(ROOT/'demo/fullstack/files',Path(baseline)/'project')
        before=subprocess.run(['python3','-m','unittest','discover','-s','tests'],cwd=Path(baseline)/'project',capture_output=True,text=True,timeout=10)
        assert before.returncode!=0,'fixture must reproduce the API failures before repair'
    try:
        for mode in a.modes: runs.append(run(a,mode,f'http://127.0.0.1:{server.server_port}/v1',server.requests))
    finally:server.shutdown();server.server_close()
    result=dict(version=1,passed=all(r['passed'] for r in runs),runs=runs,live_model=False,baseline_api_tests_fail=True)
    a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
    return 0 if result['passed'] else 1

if __name__=='__main__':raise SystemExit(main())
