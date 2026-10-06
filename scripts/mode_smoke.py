#!/usr/bin/env python3
"""Drive the labeled routing corpus through the real CLI and inspect requests.
Tests starting-profile precision/recall, overrides, and Auto across successive
turns. Risky-command escalation and single-checker coverage live in tests/cli.rs.
"""
import argparse
import http.server
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading

ROOT=Path(__file__).resolve().parents[1]
class Provider(http.server.BaseHTTPRequestHandler):
    def log_message(self,*_): pass
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        self.server.requests.append(body)
        response={'choices':[{'delta':{'content':'Reviewed the request.'},'finish_reason':'stop'}]}
        data=('data: '+json.dumps(response)+'\n\ndata: [DONE]\n\n').encode()
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary',type=Path,default=Path('target/debug/rusty'));p.add_argument('--report',type=Path,required=True)
    a=p.parse_args();binary=a.binary.resolve()
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Provider);server.requests=[]
    threading.Thread(target=server.serve_forever,daemon=True).start()
    env={k:v for k,v in os.environ.items() if not k.startswith(('RUSTY_','NVIDIA_API_KEY','AWS_','TF_','CLOUDSDK_','KUBE'))}
    env.update(NVIDIA_API_KEY='offline-test-key',RUSTY_NO_DOTENV='1',RUSTY_BASE_URL=f'http://127.0.0.1:{server.server_port}/v1',NO_COLOR='1')
    cases=json.loads((ROOT/'tests/fixtures/mode-cases.json').read_text());runs=[]
    try:
        with tempfile.TemporaryDirectory(prefix='modes-') as tmp:
            env['RUSTY_HOME']=tmp
            def run(args,stdin=None):
                before=len(server.requests)
                out=subprocess.run([str(binary),'--memory','off','--agents','off',*args],input=stdin,env=env,cwd=tmp,capture_output=True,text=True,timeout=10)
                assert out.returncode==0,out.stderr
                return server.requests[before:]
            def mode(body):
                system=body['messages'][0]['content']
                found=[m for m in ['careful','standard','vibe'] if f'Execution: {m}.' in system]
                assert len(found)==1;return found[0]
            for c in cases:
                requests=run(['--mode','auto',c['request']]);assert len(requests)==1
                actual=mode(requests[0])
                runs.append(dict(**c,actual=actual,passed=actual==c['mode']))
            overrides={m:mode(run(['--mode',m,'deploy to production'])[0])==m for m in ['standard','careful','vibe']}
            successive=run([], '/mode auto\nplan the database migration\nexplain the migration\nsketch a landing page\nfix a parser\n/exit\n')
            successive_modes=[mode(b) for b in successive]
            settings=json.loads((Path(tmp)/'settings.json').read_text())
            checks=dict(corpus=all(r['passed'] for r in runs),explicit_overrides=all(overrides.values()),
                        reselects_each_turn=successive_modes==['careful','standard','vibe','standard'],saved_setting_stays_auto=settings['execution_mode'] is None)
    finally:server.shutdown();server.server_close()
    def metrics(label):
        tp=sum(r['actual']==label and r['mode']==label for r in runs)
        selected=sum(r['actual']==label for r in runs);gold=sum(r['mode']==label for r in runs)
        return dict(precision=tp/selected if selected else None,recall=tp/gold if gold else None,cases=gold)
    result=dict(version=1,passed=all(checks.values()),checks=checks,runs=runs,classes={m:metrics(m) for m in ['standard','careful','vibe']},successive_modes=successive_modes,
                qualification='42 hand-labeled prompts, real CLI, scripted local provider; not an independent natural-language benchmark')
    a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
    return 0 if result['passed'] else 1
if __name__=='__main__':raise SystemExit(main())
