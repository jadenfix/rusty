"""Deterministic provider for the hosted swarm smoke, never used by production.
Lives outside the project and checks real tool results before advancing.
"""
import http.server
import json
from pathlib import Path
import threading
import time

PATHS=['src/config.rs','src/signal.rs','src/execution.rs']
DESCS=['agents','signals','modes']
FACTS=['AgentsMode: Off, Sub, Swarm, Auto.','reset() sets INTERRUPTED to false.','ExecutionMode: Careful, Standard, Vibe.']
CONDITION=threading.Condition()
ENTERED=0
ACTIVE=0
PEAK=0
DENIED=0
READS=0
REQUESTS=0
METRICS=Path.home()/'swarm-metrics.json'


def metrics():
    METRICS.write_text(json.dumps(dict(entered=ENTERED,peak_parallel=PEAK,denied_writes=DENIED,reads=READS,requests=REQUESTS)))


class Provider(http.server.BaseHTTPRequestHandler):
    def log_message(self,*_):pass
    def do_GET(self):
        self.send_response(200);self.end_headers();self.wfile.write(b'ready')
    def do_POST(self):
        global ENTERED,ACTIVE,PEAK,DENIED,READS,REQUESTS
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        messages=body['messages']
        prompt=next(m['content'] for m in reversed(messages) if m['role']=='user')
        steps=sum(m['role']=='tool' for m in messages)
        with CONDITION:REQUESTS+=1
        def tool(name,args):
            return {'tool_calls':[{'index':0,'id':f'call-{steps}','type':'function','function':{'name':name,'arguments':json.dumps(args)}}]},'tool_calls'
        if prompt.startswith('WORKER-'):
            i=int(prompt[7])
            assert 'write_file' not in {t['function']['name'] for t in body['tools']}
            if steps==0:
                with CONDITION:
                    ENTERED+=1;ACTIVE+=1;PEAK=max(PEAK,ACTIVE);CONDITION.notify_all()
                    assert CONDITION.wait_for(lambda:ENTERED==3,timeout=5),'workers did not overlap'
                time.sleep(.2)
                with CONDITION:ACTIVE-=1;metrics()
                delta,finish=tool('write_file',{'path':'swarm-worker-must-not-write.txt','content':'forbidden'})
            elif steps==1:
                assert 'denied' in messages[-1]['content']
                with CONDITION:DENIED+=1;metrics()
                delta,finish=tool('read_file',{'path':PATHS[i]})
            else:
                expected=['Swarm','INTERRUPTED','Careful'][i]
                assert expected in messages[-1]['content'],'file was not read'
                with CONDITION:READS+=1;metrics()
                delta,finish={'content':FACTS[i]},'stop'
        else:
            if steps==0:delta,finish=tool('swarm',{'tasks':[{'description':DESCS[i],'prompt':f'WORKER-{i}: read {PATHS[i]} and report the fact.'} for i in range(3)]})
            elif steps==1:
                report=messages[-1]['content'];assert report.count('(done)')==3
                assert all(fact in report for fact in FACTS)
                delta,finish=tool('write_file',{'path':'swarm-report.md','content':'\n\n'.join(f'## {DESCS[i]}\n{FACTS[i]}' for i in range(3))+'\n'})
            elif steps==2:delta,finish=tool('bash',{'command':'test -s swarm-report.md && test ! -e swarm-worker-must-not-write.txt && printf verified > swarm-proof.txt'})
            else:
                assert 'exit code: 0' in messages[-1]['content']
                delta,finish={'content':'Done and verified.'},'stop'
        reply={'choices':[{'delta':delta,'finish_reason':finish}],'usage':{'prompt_tokens':10,'completion_tokens':5}}
        data=('data: '+json.dumps(reply)+'\n\ndata: [DONE]\n\n').encode()
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)


if __name__=='__main__':http.server.ThreadingHTTPServer(('127.0.0.1',8123),Provider).serve_forever()
