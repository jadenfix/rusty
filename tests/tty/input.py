#!/usr/bin/env python3
"""Input ownership, editing, steering and recovery with real expect/PTY sessions.
The loopback provider is a fixture: no model credentials or remote calls.
"""
import argparse
import http.server
import json
import os
import shutil
from pathlib import Path
import subprocess
import threading
import time
from capture import Provider as ModeProvider

class Provider(ModeProvider):
    requests=[]
    def log_message(self,*args):pass
    def do_GET(self):
        time.sleep(2)
        try:
            self.send_response(200);self.end_headers();self.wfile.write(b'{"data":[]}')
        except (BrokenPipeError,ConnectionResetError):pass
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        self.requests.append(body)
        prompt=next(m['content'] for m in reversed(body['messages']) if m['role']=='user')
        if prompt=='broken':
            self.send_response(400);self.end_headers();self.wfile.write(b'controlled provider error');return
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
        def send(delta,finish=None):
            data={'choices':[{'delta':delta,'finish_reason':finish}]}
            self.wfile.write(('data: '+json.dumps(data)+'\n\n').encode());self.wfile.flush()
        try:
            if prompt!='instant' and self.ui_case(body,prompt,send):
                pass
            elif prompt=='approve' and not any(m['role']=='tool' for m in body['messages']):
                time.sleep(.7)
                send({'tool_calls':[{'index':0,'id':'approval','type':'function','function':{'name':'write_file','arguments':json.dumps({'path':'approved.txt','content':'approved'})}}]},'tool_calls')
            elif prompt=='bashhold' and not any(m['role']=='tool' for m in body['messages']):
                send({'tool_calls':[{'index':0,'id':'bash','type':'function','function':{'name':'bash','arguments':json.dumps({'command':'sleep 2; printf done'})}}]},'tool_calls')
            else:
                if prompt in ['slow','hold'] or 'New goal: hold-goal' in prompt:
                    if prompt=='hold':send({'content':'HOLD_READY\n'})
                    for i in range(30 if prompt=='slow' else 150):
                        send({'reasoning_content':'Checking the route.\n'})
                        if prompt=='slow':send({'content':f'Output {i:02d} scrolls above the draft.\n'})
                        time.sleep(.06)
                time.sleep(.1);send({'content':'DONE\n'},'stop')
            self.wfile.write(b'data: [DONE]\n\n');self.wfile.flush()
        except (BrokenPipeError,ConnectionResetError):pass

def run(binary, output, selected=None):
    output.mkdir(parents=True,exist_ok=True)
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Provider)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    endpoint=f'http://127.0.0.1:{server.server_port}/v1'
    cases=['command','file','unicode','multiline-cancel','history','unsent','escape','steer','slash','bash','bash-escape','bash-steer','reduced','verbose','verbose-flow','resize','approval','approval-empty','approval-stop','plain','error','models','routes','loop','subagent','swarm','careful','short','busy-unicode','goal','plain-stop']
    if selected:cases=[case for case in cases if case in selected]
    reports=[]
    try:
        for case in cases:
            widths=[24,40,80,120] if case in ['unsent','unicode'] else [80]
            for width in widths:
                dest=output/f'{case}-{width}';dest.mkdir(exist_ok=True)
                if (dest/'home').exists():shutil.rmtree(dest/'home')
                (dest/'approved.txt').unlink(missing_ok=True)
                (dest/'proof.txt').write_text('fixture\n')
                start=len(Provider.requests);t=time.monotonic()
                proc=subprocess.run(['expect',str(Path(__file__).with_suffix('.exp').resolve()),str(binary.resolve()),endpoint,str(dest.resolve()),case,str(width)],cwd=dest,capture_output=True,text=True)
                (dest/'driver.txt').write_text(proc.stdout+proc.stderr)
                if proc.returncode:raise AssertionError(f'{case}-{width}: {proc.stdout} {proc.stderr}')
                requests=Provider.requests[start:]
                prompts=[next(m['content'] for m in reversed(r['messages']) if m['role']=='user') for r in requests]
                if case=='multiline-cancel':assert prompts==['instant'],prompts
                if case=='file':assert prompts==['@proof.txt'],prompts
                if case=='unicode':assert prompts==['hello\n!'],prompts
                if case in ['command','routes']:assert not prompts,prompts
                if case=='slash':assert prompts==['hold'],prompts
                if case=='history':assert prompts==['instant','instant'],prompts
                if case in ['unsent','escape','steer','slash','bash','bash-escape','bash-steer','reduced','verbose','verbose-flow','resize','models']:
                    if case!='slash':assert prompts[-1]=='instant',prompts
                    if case=='unsent':assert prompts==['slow','instant'],prompts
                if case in ['subagent','swarm','careful','loop','short','goal','plain-stop','busy-unicode']:assert 'instant' in prompts,prompts
                if case.startswith('approval'):
                    assert 'unsent-note' not in prompts,prompts
                    assert (dest/'approved.txt').exists()==(case=='approval')
                    for stored in (dest/'home').rglob('*'):
                        if stored.is_file():assert b'unsent-note' not in stored.read_bytes(),stored
                latency=dict((name,int(value)) for name,value in (line.split('\t') for line in (dest/'latency.tsv').read_text().splitlines()))
                assert latency.get('feedback_ms',0)<500,latency
                assert latency.get('interrupt_ms',0)<1000,latency
                assert latency.get('steer_ms',0)<1000,latency
                report={'latency':latency,'case':case,'width':width,'passed':True,'requests':len(requests),'last_prompts':prompts,'seconds':round(time.monotonic()-t,3)}
                reports.append(report)
                (output/'checks.json').write_text(json.dumps({'passed':True,'journeys':reports},indent=2)+'\n')
                print(f'PASS {case}-{width}: {len(requests)} requests',flush=True)
                # Timed asciinema recording from the actual PTY, for visual review.
                lines=(dest/'events.tsv').read_text().splitlines()
                if lines:
                    first=int(lines[0].split('\t')[0])
                    with (dest/'terminal.cast').open('w') as cast:
                        cast.write(json.dumps({'version':2,'width':width,'height':24,'timestamp':first//1000})+'\n')
                        for line in lines:
                            at,data=line.split('\t');cast.write(json.dumps([(int(at)-first)/1000,'o',bytes.fromhex(data).decode('utf-8')])+'\n')
    finally:server.shutdown();server.server_close()
    return reports

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--case',action='append');args=p.parse_args()
    run(args.binary,args.output,args.case)
