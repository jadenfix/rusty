#!/usr/bin/env python3
"""Real CLI/PTY regression and local overhead benchmark; no model inference.
Gold checks run outside the agent workspace. Scripted tool plans measure the
executor, not model intelligence. Every process uses isolated fake credentials.
"""
import argparse
import contextlib
import http.server
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]


class Provider(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        messages = body['messages']
        last = max(i for i, m in enumerate(messages) if m['role'] == 'user')
        prompt = messages[last]['content']
        results = [m for m in messages[last+1:] if m['role'] == 'tool']
        self.server.requests.append(body)
        plan = self.server.plans.get(prompt, [])
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Connection', 'close')
        self.end_headers()

        def emit(delta, finish=None):
            data = {'choices': [{'delta': delta, 'finish_reason': finish}]}
            self.wfile.write(('data: '+json.dumps(data)+'\n\n').encode())
            self.wfile.flush()

        try:
            if prompt.startswith('approval ') and not results:
                name, args = 'write_file', {'path': prompt.replace(' ', '-')+'.txt', 'content': 'approved'}
                emit({'tool_calls': [{'index': 0, 'id': 'approval', 'type': 'function',
                      'function': {'name': name, 'arguments': json.dumps(args)}}]}, 'tool_calls')
            elif prompt.startswith('approval '):
                emit({'content': 'APPROVAL_RESIZE_READY\n'})
                time.sleep(.15)
                emit({'content': 'BENCH_DONE\n'}, 'stop')
            elif prompt == 'stream interrupt':
                emit({'content': 'STREAM_READY\n'})
                for _ in range(30):
                    time.sleep(.1)
                    emit({'content': 'still streaming\n'})
                emit({'content': 'BENCH_DONE\n'}, 'stop')
            elif len(results) < len(plan):
                name, args = plan[len(results)]
                arguments = args if isinstance(args, str) else json.dumps(args)
                emit({'tool_calls': [{'index': 0, 'id': f'call-{len(results)}', 'type': 'function',
                      'function': {'name': name, 'arguments': arguments}}]}, 'tool_calls')
            else:
                emit({'content': 'BENCH_DONE\n'}, 'stop')
            self.wfile.write(b'data: [DONE]\n\n')
        except (BrokenPipeError, ConnectionResetError):
            pass


def percentile(values, fraction):
    return sorted(values)[max(0, int(len(values)*fraction+.999)-1)]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, default=Path('target/debug/rusty'))
    p.add_argument('--report', type=Path, required=True)
    p.add_argument('--trials', type=int, default=10)
    a = p.parse_args()
    binary = a.binary.resolve()
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Provider)
    server.requests, server.plans = [], {}
    threading.Thread(target=server.serve_forever, daemon=True).start()
    env = {k: v for k, v in os.environ.items() if not k.startswith(
        ('RUSTY_', 'NVIDIA_API_KEY', 'AWS_', 'TF_', 'KUBE', 'CLOUDSDK_'))}
    env.update(NVIDIA_API_KEY='offline-test-key', RUSTY_NO_DOTENV='1',
               RUSTY_BASE_URL=f'http://127.0.0.1:{server.server_port}/v1', NO_COLOR='1')
    runs, terminals, timings = [], [], {}
    try:
        with tempfile.TemporaryDirectory(prefix='rsb-', dir='/tmp') as tmp, contextlib.ExitStack() as cleanup:
            root = Path(tmp).resolve()
            def stop_advisors():
                for sock in root.glob('**/memory/advisor.sock'):
                    subprocess.run([str(binary.with_name('rusty-memoryd')), '--home', str(sock.parent.parent), 'stop'],
                                   env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=3)
            cleanup.callback(stop_advisors)
            project = root/'project'
            project.mkdir()
            home = root/'home'
            home.mkdir()
            env.update(HOME=str(home), RUSTY_HOME=str(root/'config'))
            env.update(PWD=str(project), OLDPWD=str(home))
            outside = home/'rusty/demo space λ'
            outside.mkdir(parents=True)
            (project/'inside').mkdir()
            (project/'escape').symlink_to(outside, target_is_directory=True)

            def run(label, plan, verify, permissions='yolo', timeout=8):
                server.plans[label] = plan
                before = len(server.requests)
                started = time.perf_counter()
                out = subprocess.run([str(binary), '--tools', 'local', '--memory', 'off',
                                      '--agents', 'off', '--mode', 'standard', '--permissions', permissions,
                                      label], cwd=project, env=env, capture_output=True, text=True, timeout=timeout)
                elapsed = time.perf_counter()-started
                requests = server.requests[before:]
                tools = [m['content'] for b in requests[-1:] for m in b['messages'] if m['role'] == 'tool']
                passed = out.returncode == 0 and 'BENCH_DONE' in out.stdout and verify(tools)
                runs.append(dict(case=label, passed=passed, seconds=round(elapsed, 4), requests=len(requests)))
                if not passed:
                    print(json.dumps(dict(case=label, tool_results=tools, error=out.stderr)))

            for location in [str(outside), 'inside', str(project/'inside')]:
                expected = (project/location).resolve()
                run('cwd '+location, [('bash', {'command': "pwd; printf 'verified' > result.txt", 'cwd': location}),
                                     ('bash', {'command': 'cat result.txt; pwd', 'cwd': location})],
                    lambda ts, d=expected: len(ts) == 2 and str(d) in ts[1] and 'verified' in ts[1]
                    and (d/'result.txt').read_text() == 'verified')
            run('cd stays shell-local', [('bash', {'command': 'cd inside && pwd'}), ('bash', {'command': 'pwd'})],
                lambda ts: len(ts) == 2 and str(project/'inside') in ts[0] and
                'stdout:\n'+str(project)+'\n' in ts[1])
            run('exports stay shell-local', [('bash', {'command': 'export RUSTY_BENCH_SENTINEL=exists'}),
                                            ('bash', {'command': 'printf "%s" "${RUSTY_BENCH_SENTINEL-unset}"'})],
                lambda ts: len(ts) == 2 and 'unset' in ts[1])
            for label, args, match in [
                ('missing cwd', {'command': 'pwd', 'cwd': 'missing'}, 'cwd does not exist'),
                ('invalid cwd type', {'command': 'pwd', 'cwd': 7}, 'cwd must be a string'),
                ('failed cd', {'command': 'cd missing && touch should-not-exist'}, 'exit code: 1'),
                ('shell error', {'command': 'exit 23'}, 'exit code: 23'),
                ('stderr', {'command': "printf 'error-marker' >&2"}, 'error-marker'),
                ('large output', {'command': "python3 -c 'print(\"x\"*200000)'"}, 'exit code: 0'),
            ]:
                run(label, [('bash', args)], lambda ts, m=match: len(ts) == 1 and m in ts[0])
            for path in [str(outside), 'escape']:
                run('outside denied '+path, [('bash', {'command': 'touch forbidden', 'cwd': path})],
                    lambda ts: len(ts) == 1 and 'denied' in ts[0] and not (outside/'forbidden').exists(),
                    permissions='read-only')
                run('outside asks '+path, [('bash', {'command': 'touch forbidden', 'cwd': path})],
                    lambda ts: len(ts) == 1 and 'approval' in ts[0] and not (outside/'forbidden').exists(),
                    permissions='auto')
            run('literal cwd is not shell code', [('bash', {'command': 'pwd', 'cwd': '$(touch injected)'})],
                lambda ts: len(ts) == 1 and 'cwd does not exist' in ts[0] and not (project/'injected').exists())
            run('file edit journey', [('write_file', {'path': 'notes.txt', 'content': 'first λ\n'}),
                                     ('read_file', {'path': 'notes.txt'}),
                                     ('edit_file', {'path': 'notes.txt', 'old_string': 'first', 'new_string': 'second'}),
                                     ('bash', {'command': 'cat notes.txt'})],
                lambda ts: len(ts) == 4 and 'second λ' in ts[-1] and (project/'notes.txt').read_text() == 'second λ\n')
            run('malformed arguments', [('bash', '{invalid-json')], lambda ts: len(ts) == 1 and 'not valid JSON' in ts[0])
            run('unknown tool', [('not_a_tool', {})], lambda ts: len(ts) == 1 and 'unknown tool' in ts[0])
            run('read-only denies writes', [('write_file', {'path': 'forbidden.txt', 'content': 'no'})],
                lambda ts: len(ts) == 1 and 'denied' in ts[0] and not (project/'forbidden.txt').exists(), permissions='read-only')
            child_marker = project/'survived-timeout'
            run('child process timeout', [('bash', {'command': 'sleep 20 & wait', 'timeout_secs': 1}),
                                          ('bash', {'command': "printf 'recovered'"})],
                lambda ts: len(ts) == 2 and 'timed out after 1s' in ts[0] and 'recovered' in ts[1], timeout=4)
            run('background pipe holder timeout', [('bash', {'command': 'sleep 20 &', 'timeout_secs': 1})],
                lambda ts: len(ts) == 1 and 'timed out after 1s' in ts[0], timeout=4)
            run('timeout kills delayed writer', [('bash', {'command': f"(sleep 2; touch '{child_marker}') & wait", 'timeout_secs': 1})],
                lambda ts: len(ts) == 1 and 'timed out' in ts[0], timeout=4)
            time.sleep(1.2)
            runs.append(dict(case='no delayed write after timeout', passed=not child_marker.exists()))

            # Real interactive REPL, repeated approvals, dynamic resize and Ctrl-C.
            for memory in ['off', 'on', 'deep']:
                d = root/('tty-'+memory)
                d.mkdir()
                started = time.perf_counter()
                out = subprocess.run(['expect', str(ROOT/'tests/tty/session.exp'), str(binary),
                                      env['RUSTY_BASE_URL'], str(d), memory], cwd=d, env=env,
                                     capture_output=True, text=True, timeout=45)
                terminal = (d/'session.ansi').read_bytes()
                files = list(d.glob('approval-accepted-*.txt'))
                passed = out.returncode == 0 and len(files) == 8 and not list(d.glob('approval-[0-9].txt'))
                terminals.append(dict(memory=memory, passed=passed, approvals=16, resizes=8,
                                      seconds=round(time.perf_counter()-started, 3),
                                      result=out.stdout.strip(), panic=b'panicked' in terminal))
                if not passed:
                    print(out.stdout, out.stderr)
                    print(terminal[-4000:].decode(errors='replace'))

            # Each trial starts a process and runs three successive turns. No
            # fixed pause in this provider; latency includes startup + memory.
            for memory in ['off', 'on', 'deep']:
                values, counts = [], []
                for trial in range(a.trials):
                    env['RUSTY_HOME'] = str(root/('bench-'+memory))
                    before = len(server.requests)
                    started = time.perf_counter()
                    out = subprocess.run([str(binary), '--tools', 'local', '--memory', memory,
                                          '--permissions', 'ask', '--agents', 'off', '--mode', 'auto'],
                                         input='explain this folder\nexplain migrations\nhello\n/exit\n',
                                         cwd=project, env=env, capture_output=True, text=True, timeout=10)
                    values.append((time.perf_counter()-started)*1000)
                    reqs = server.requests[before:]
                    counts.append(len(reqs))
                    passed = out.returncode == 0 and len(reqs) == 3 and out.stdout.count('BENCH_DONE') == 3
                    passed = passed and all('Execution: standard.' in b['messages'][0]['content'] for b in reqs)
                    runs.append(dict(case=f'{memory} steady Auto session {trial}', passed=passed))
                timings[memory] = dict(trials=a.trials, turns_per_trial=3, requests=counts,
                                      process_and_three_turns_ms=dict(p50=round(statistics.median(values), 3),
                                          p95=round(percentile(values, .95), 3), max=round(max(values), 3)))
    finally:
        server.shutdown()
        server.server_close()
    result = dict(version=1, passed=all(r['passed'] for r in runs+terminals),
                  cases=len(runs), runs=runs, terminals=terminals, benchmarks=timings,
                  qualification='Real Rust CLI, Bash/files, expect PTY, scripted localhost provider; no live-model or hosted Daytona score',
                  paid_model_calls=0, cloud_resources_created=0)
    a.report.parent.mkdir(parents=True, exist_ok=True)
    a.report.write_text(json.dumps(result, indent=2)+'\n')
    print(json.dumps(dict(passed=result['passed'], cases=len(runs), terminals=terminals, benchmarks=timings)))
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
