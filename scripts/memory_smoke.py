#!/usr/bin/env python3
"""Easy behavioral checks through the real CLI, offline or NVIDIA-backed.
Verifiers and receipts stay outside the agent's workspace. No raw model logs
are uploaded. A provider failure is recorded separately and still fails CI.
"""
import argparse
import collections
import http.server
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import threading
import time


def rpc(home, op, data=None, seq=0, mode='on', session='seed'):
    identity = 'memory-smoke'
    h = 0xcbf29ce484222325
    for b in identity.encode(): h = ((h ^ b) * 0x100000001b3) & ((1 << 64) - 1)
    request = dict(op=op, scope=f'project:{h:016x}', session=session, seq=seq, mode=mode, data=data)
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(2)
        s.connect(str(home / 'memory/advisor.sock'))
        s.sendall(json.dumps(request).encode() + b'\n')
        f = s.makefile('rb')
        return json.loads(f.readline(4 * 1024 * 1024))


class Mock(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if not body.get('stream'):
            board = json.loads(body['messages'][-1]['content'])
            result = {'next_step': 'Verify the calculator add function with the stated inputs.',
                      'evidence_ids': [board['observations'][-1]['id']]}
            reply = {'choices': [{'message': {'content': json.dumps(result)}}]}
            data = json.dumps(reply).encode()
            self.send_response(200); self.send_header('Content-Length', str(len(data))); self.end_headers(); self.wfile.write(data)
            return
        self.server.requests.append(body)
        tools = sum(m['role'] == 'tool' for m in body['messages'])
        edit = any('calculator.py' in str(m.get('content', '')) for m in body['messages'] if m['role'] == 'user')
        plan = [('read_file', {'path': 'calculator.py'}),
                ('edit_file', {'path': 'calculator.py', 'old_string': 'return a - b', 'new_string': 'return a + b'}),
                ('bash', {'command': "python3 -c 'from calculator import add; assert add(2,3)==5' && mkdir -p artifacts && printf verified > artifacts/check.txt"})] if edit else [
                ('write_file', {'path': 'note.txt', 'content': '42'}),
                ('bash', {'command': 'test "$(cat note.txt)" = 42 && mkdir -p artifacts && printf verified > artifacts/check.txt'})]
        if tools < len(plan):
            name, args = plan[tools]
            delta = {'tool_calls': [{'index': 0, 'id': f'call{tools}', 'type': 'function', 'function': {'name': name, 'arguments': json.dumps(args)}}]}
            finish = 'tool_calls'
        else: delta, finish = {'content': 'Done and verified.'}, 'stop'
        reply = {'choices': [{'delta': delta, 'finish_reason': finish}], 'usage': {'prompt_tokens': 10, 'completion_tokens': 5}}
        data = ('data: ' + json.dumps(reply) + '\n\ndata: [DONE]\n\n').encode()
        self.send_response(200); self.send_header('Content-Type', 'text/event-stream'); self.send_header('Content-Length', str(len(data))); self.end_headers(); self.wfile.write(data)


def stop(p):
    if p.poll() is None:
        os.killpg(p.pid, signal.SIGTERM)
    p.wait(timeout=5)


def run_case(args, mode, task, endpoint, requests):
    with tempfile.TemporaryDirectory(prefix='rms-') as tmp:
        root = Path(tmp); project = root / 'project'; home = root / 'home'
        project.mkdir(); home.mkdir()
        if task == 'edit': (project / 'calculator.py').write_text('SENTINEL = "keep me"\ndef add(a, b):\n    return a - b\n')
        env = {k:v for k,v in os.environ.items() if not k.startswith('RUSTY_') and not k.startswith('NVIDIA_API_KEY')}
        env.update(RUSTY_HOME=str(home), RUSTY_NO_DOTENV='1', RUSTY_PROJECT_ID='memory-smoke', NO_COLOR='1')
        if args.live:
            key = os.environ.get('RUSTY_API_KEY') or os.environ.get('NVIDIA_API_KEY')
            if not key: raise SystemExit('live check requires NVIDIA_API_KEY or RUSTY_API_KEY')
            env['NVIDIA_API_KEY'] = key
            if os.environ.get('RUSTY_BASE_URL'): env['RUSTY_BASE_URL'] = os.environ['RUSTY_BASE_URL']
        else: env.update(NVIDIA_API_KEY='offline-test-key', RUSTY_BASE_URL=endpoint)
        daemon = subprocess.Popen([str(args.binary.with_name('rusty-memoryd')), '--home', str(home)], env=env, stdout=subprocess.DEVNULL, stderr=(root/'daemon.log').open('w'), start_new_session=True)
        try:
            deadline = time.monotonic() + 3
            while not (home / 'memory/advisor.sock').exists():
                if daemon.poll() is not None or time.monotonic() > deadline: raise RuntimeError('daemon did not start')
                time.sleep(.01)
            lesson = 'For the add function, inspect calculator.py before editing and preserve SENTINEL.' if task == 'edit' else 'For creating note.txt, write exactly 42 and verify with Bash.'
            assert not rpc(home, 'remember', {'kind':'preference','text':lesson})['error']
            if task == 'edit':
                prompt = '''Fix calculator.py so add(a,b) adds a and b. Preserve SENTINEL. Read calculator.py directly with read_file, then use edit_file. Finally use the bash tool to run this exact command: python3 -c 'from calculator import add; assert add(2,3)==5 and add(-2,3)==1' && mkdir -p artifacts && printf verified > artifacts/check.txt
Create the marker through Bash, not write_file. Report the result briefly. This task needs no directory discovery or extra planning.'''
            else:
                prompt = '''Use write_file to create note.txt containing exactly the two ASCII bytes 42, without a newline. Then use the bash tool to run this exact command: test "$(cat note.txt)" = 42 && mkdir -p artifacts && printf verified > artifacts/check.txt
Create the marker through Bash, not write_file. Report the result briefly. This task needs no directory discovery or extra planning.'''
            trajectory = root / 'trajectory.json'
            command = [str(args.binary), '--yolo', '--mode', 'vibe', '--agents', 'off', '--memory', mode, '--stats', '--trajectory', str(trajectory), prompt]
            if args.model: command[1:1] = ['--model', args.model]
            before = len(requests); started = time.monotonic()
            p = subprocess.Popen(command, cwd=project, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
            timed_out = False
            try: stdout, stderr = p.communicate(timeout=args.timeout)
            except subprocess.TimeoutExpired:
                timed_out = True; stop(p); stdout, stderr = p.communicate()
            wall = time.monotonic() - started
            trace = json.loads(trajectory.read_text()) if trajectory.exists() else {}
            calls = [c for m in trace.get('messages', []) for c in (m.get('tool_calls') or [])]
            names = collections.Counter(c['function']['name'] for c in calls)
            repeated = sum(n-1 for n in collections.Counter(json.dumps(c['function'],sort_keys=True) for c in calls).values() if n>1)
            checks = {'no_unknown_tools':all(n in {'read_file','write_file','edit_file','bash','list_files','search','glob','outline','plan','remember','recall','forget'} for n in names), 'exit':p.returncode==0 and not timed_out, 'bash_artifact':(project/'artifacts/check.txt').exists() and (project/'artifacts/check.txt').read_text().rstrip('\n')=='verified', 'bash_used':names['bash']>0, 'tool_budget':bool(trace) and len(calls)<=12, 'repeat_budget':bool(trace) and repeated<=2}
            if task == 'create': checks['file_content'] = (project/'note.txt').exists() and (project/'note.txt').read_text()=='42'; checks['write_used']=names['write_file']>0
            else:
                code = 'from calculator import add,SENTINEL; assert add(2,3)==5 and add(-2,3)==1 and SENTINEL=="keep me"'
                v = subprocess.run(['python3','-c',code],cwd=project,env={'PATH':os.environ['PATH']},capture_output=True,timeout=5)
                checks['function_and_sentinel']=v.returncode==0; checks['edit_used']=names['edit_file']>0
            metrics = trace.get('memory') or {}
            if mode != 'off':
                checks['advisor_enabled']=trace.get('memory_mode')==mode
                checks['injected']=metrics.get('injections',0)>0
                checks['hook_deadline']=metrics.get('max_hook_us',0)<150000
                checks['no_hook_timeout']=metrics.get('timeouts',0)==0
            else: checks['memory_off']=trace.get('memory_mode')=='off' and not metrics
            if not args.live:
                bodies = requests[before:]
                seen = any('Memory advisor' in m.get('content','') for b in bodies for m in b['messages'] if m['role']=='system')
                checks['prompt_injection_matches_mode']=seen == (mode!='off')
            failure = 'timeout' if timed_out else ('provider' if p.returncode and any(s in stderr.lower() for s in ['429','502','503','504','unauthorized','rate limit']) else 'behavior')
            service = rpc(home, 'status')['data']
            storage = sum(f.stat().st_size for f in (home/'memory').iterdir() if f.is_file())
            checks['storage_budget']=storage<20*1024*1024
            report = dict(mode=mode,task=task,passed=all(checks.values()),checks=checks,wall_seconds=round(wall,3),trajectory_complete=bool(trace),tool_calls=len(calls) if trace else None,tools=dict(names),repeated_calls=repeated if trace else None,totals=trace.get('totals'),memory=metrics,storage_bytes=storage,service=service,artifact_bytes=(project/'artifacts/check.txt').stat().st_size if (project/'artifacts/check.txt').exists() else None)
            if not report['passed']:
                report['failure_class']=failure
                if trace:
                    diagnostic=args.report.with_name(args.report.stem+f'-{mode}-{task}-failure.json')
                    diagnostic.parent.mkdir(parents=True,exist_ok=True)
                    fd=os.open(diagnostic,os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o600)
                    with os.fdopen(fd,'w') as f: json.dump(trace,f)
                if not args.live: print('Offline failure:', stderr, 'daemon exit:', daemon.poll(), (root/'daemon.log').read_text())
            print(json.dumps(report),flush=True)
            return report
        finally: stop(daemon)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary',type=Path,default=Path('target/debug/rusty'))
    p.add_argument('--live',action='store_true'); p.add_argument('--model',default='nvidia/nemotron-3-super-120b-a12b'); p.add_argument('--timeout',type=int,default=180)
    p.add_argument('--modes',nargs='+',choices=['off','on','deep'],default=['off','on','deep'])
    p.add_argument('--report',type=Path,default=Path('memory-smoke.json'))
    args = p.parse_args(); args.binary=args.binary.resolve()
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Mock);server.requests=[]
    thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
    reports=[]
    args.report.parent.mkdir(parents=True,exist_ok=True)
    try:
        for mode in args.modes:
            for task in ['create','edit']:
                reports.append(run_case(args,mode,task,f'http://127.0.0.1:{server.server_port}/v1',server.requests))
                # Preserve completed attempts if a later provider call or job is interrupted.
                args.report.write_text(json.dumps(dict(version=1,task_revision="r4-two-byte-note",live=args.live,model=args.model,runs=reports,passed=False,complete=False),indent=2)+'\n')
    finally: server.shutdown();server.server_close()
    receipt=dict(version=1,task_revision="r4-two-byte-note",live=args.live,model=args.model,complete=True,runs=reports,passed=all(r['passed'] for r in reports))
    args.report.parent.mkdir(parents=True,exist_ok=True);args.report.write_text(json.dumps(receipt,indent=2)+'\n')
    return 0 if receipt['passed'] else 1

if __name__=='__main__': raise SystemExit(main())
