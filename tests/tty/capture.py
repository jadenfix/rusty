#!/usr/bin/env python3
"""Paced SSE provider for the real expect/PTY display capture.
Uses the actual file and Bash tools, then streams a long answer.
"""
import argparse
import http.server
import json
from pathlib import Path
import subprocess
import threading
import time

class Provider(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        prompt = next(m['content'] for m in reversed(body['messages']) if m['role'] == 'user')
        self.send_response(200)
        self.send_header('Content-Type','text/event-stream')
        self.send_header('Connection','close')
        self.end_headers()
        def send(delta,finish=None):
            data={'choices':[{'delta':delta,'finish_reason':finish}]}
            try:self.wfile.write(('data: '+json.dumps(data)+'\n\n').encode());self.wfile.flush()
            except BrokenPipeError:pass
        if self.ui_case(body,prompt,send):
            pass
        elif 'Unix' in prompt:
            for _ in range(60):
                time.sleep(.15);send({'content':'A long streamed reply for interruption.\n'})
        elif 'pong' in prompt:
            send({'content':'pong\n'},'stop')
        elif prompt=='approval':
            time.sleep(.4)
            if not any(m['role']=='tool' for m in body['messages']):
                send({'tool_calls':[{'index':0,'id':'approval','type':'function','function':{'name':'write_file','arguments':json.dumps({'path':'approved.txt','content':'approved'})}}]},'tool_calls')
            else:send({'content':'CAPTURE_COMPLETE\n'},'stop')
        else:
            count=sum(m['role']=='tool' for m in body['messages'])
            time.sleep(.35)
            if count<12:
                name='write_file' if count==0 else 'bash'
                args={'path':'capture-note.txt','content':'verified\n'} if count==0 else {'command': (
                    "test -f capture-note.txt && sleep 1 && printf 'checked file\\n'" if count==1 else
                    "printf 'controlled failure\\n'; exit 2" if count==4 else
                    f"printf 'step {count:02d} verified\\n'"
                )}
                send({'tool_calls':[{'index':0,'id':f'call{count}','type':'function','function':{'name':name,'arguments':json.dumps(args)}}]},'tool_calls')
            else:
                for i in range(18):
                    send({'content':f'Captured line {i:02d}: output flows above the geometric panel.\n'})
                    time.sleep(.1)
                send({'content':'CAPTURE_COMPLETE\n'},'stop')
        try:self.wfile.write(b'data: [DONE]\n\n');self.wfile.flush()
        except BrokenPipeError:pass

    def ui_case(self,body,prompt,send):
        """Script responses; exercise the real worker runner and one checker."""
        count=sum(m['role']=='tool' for m in body['messages'])
        def tool(name,args):
            send({'tool_calls':[{'index':0,'id':f'ui-{count}','type':'function','function':{'name':name,'arguments':json.dumps(args)}}]},'tool_calls')
        worker_prompt=next((m['content'] for m in body['messages'] if m['role']=='user' and (m['content'].startswith('UI_WORKER_') or m['content'].startswith('Check this proposed completion once'))),'')
        checker=worker_prompt.startswith('Check this proposed completion once')
        if worker_prompt:
            if count==0:
                time.sleep(.2)
                tool('read_file',{'path':'capture-note.txt' if checker else 'proof.txt'})
            else:
                if worker_prompt.startswith('UI_WORKER_B'):
                    time.sleep(.3);send({'content':''},'stop')
                else:
                    time.sleep(2 if checker else 3 if worker_prompt.startswith('UI_WORKER_A') else 4)
                    send({'content':'Read the file; no actionable findings.\n'},'stop')
            return True
        original=next((m['content'] for m in body['messages'] if m['role']=='user' and m['content'] in ['ui-swarm','ui-subagent','ui-careful']),None)
        if not original:return False
        time.sleep(.3)
        if count==0:
            if original=='ui-careful':
                tool('write_file',{'path':'capture-note.txt','content':'verified\n'})
            elif original=='ui-subagent':
                tool('task',{'description':'inspect routes','prompt':'UI_WORKER_A'})
            else:
                tool('swarm',{'tasks':[{'description':label,'prompt':f'UI_WORKER_{key}'} for key,label in [('A','inspect routes'),('B','inspect failures'),('C','inspect tests')]]})
        elif original=='ui-careful' and not any('[checker\'s review]' in m.get('content','') for m in body['messages'] if m['role']=='user'):
            send({'content':'The file is updated.\n'},'stop')
        else:send({'content':'CAPTURE_COMPLETE\n'},'stop')
        return True


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    args=p.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Provider)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    root=Path(__file__).resolve().parent
    endpoint=f'http://127.0.0.1:{server.server_port}/v1'
    try:
        for width in [24,40,80,120]:
            d=args.output/f'width-{width}';d.mkdir(exist_ok=True)
            subprocess.run(['expect',str(root/'steady.exp'),str(args.binary.resolve()),endpoint,str(width),str(d.resolve())],cwd=d,check=True)
        d=args.output/'approval';d.mkdir(exist_ok=True)
        subprocess.run(['expect',str(root/'approval.exp'),str(args.binary.resolve()),endpoint,str(d.resolve())],cwd=d,check=True)
        assert (d/'approved.txt').read_text()=='approved'
        # Interactive Ctrl-C, recovery, and prompt exit use the same real PTY.
        env=dict(__import__('os').environ,RUSTY_BASE_URL=endpoint,NVIDIA_API_KEY='offline-test-key',RUSTY_NO_DOTENV='1',RUSTY_HOME=str((args.output/'interrupt-home').resolve()),TERM='xterm-256color')
        env.pop('NO_COLOR',None)
        with (args.output/'ctrl-c.txt').open('w') as log:
            subprocess.run(['expect',str(root/'ctrl_c.exp'),str(args.binary.resolve())],cwd=args.output,env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
    finally:server.shutdown();server.server_close()

if __name__=='__main__':main()
