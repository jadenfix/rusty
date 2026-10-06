#!/usr/bin/env python3
"""Eight concurrent synthetic RPC clients, real memory daemon, no model calls."""
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time
from memory_smoke import rpc, stop


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, default=Path('target/debug/rusty-memoryd'))
    p.add_argument('--report', type=Path, required=True)
    p.add_argument('--rounds', type=int, default=100)
    a = p.parse_args()
    env = {k:v for k,v in os.environ.items() if not k.startswith(('RUSTY_', 'NVIDIA_API_KEY'))}
    with tempfile.TemporaryDirectory(prefix='rms-', dir='/tmp') as tmp:
        home = Path(tmp)
        daemon = subprocess.Popen([str(a.binary.resolve()), '--home', tmp], env=env,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            deadline = time.monotonic()+3
            while not (home/'memory/advisor.sock').exists():
                if daemon.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError('daemon startup failed')
                time.sleep(.01)
            lesson = rpc(home, 'remember', {'kind':'fact', 'text':'The monorepo repository manifest uses cargo test for parser checks.'})['id']
            for i in range(511):
                assert not rpc(home, 'remember', {'kind':'fact', 'text':f'Unrelated record q{i}: cobalt acoustic telescope.'})['error']

            def worker(index):
                samples, selected, failures = [], 0, 0
                session = f'worker-{index}'
                for turn in range(a.rounds):
                    seq = turn*4+1
                    begin = rpc(home, 'begin', {'text':'check the monorepo repository manifest'}, session=session, seq=seq)
                    start = time.perf_counter_ns()
                    reply = rpc(home, 'advice', session=session, seq=seq)
                    samples.append((time.perf_counter_ns()-start)/1e6)
                    selected += reply['id'] == lesson
                    failures += begin['error'] or reply['error']
                    rpc(home, 'event', {'tool':'read_file', 'args':'manifest', 'result':'verified'}, session=session, seq=seq+1)
                    rpc(home, 'finish', session=session, seq=seq+2)
                return dict(samples=samples, hits=selected, errors=failures)

            started = time.perf_counter()
            with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
                workers = list(pool.map(worker, range(8)))
            elapsed = time.perf_counter()-started
            times = sorted(t for w in workers for t in w['samples'])
            status = rpc(home, 'status')['data']
            size = sum(f.stat().st_size for f in (home/'memory').iterdir() if f.is_file())
            hits = sum(w['hits'] for w in workers)
            errors = sum(w['errors'] for w in workers)
            p95 = times[int(.95*(len(times)-1))]
            result = dict(version=1, workers=8, advice_requests=len(times), total_hook_requests=4*len(times),
                          hits=hits, errors=errors, seconds=round(elapsed,3),
                          advice_p50_ms=round(statistics.median(times),3), advice_p95_ms=round(p95,3),
                          advice_max_ms=round(max(times),3), lessons=status['lessons'], storage_bytes=size,
                          passed=hits==len(times) and errors==0 and p95<50 and size<20*1024*1024,
                          qualification='8 synthetic RPC clients; real daemon; not model-backed agent swarms')
            a.report.parent.mkdir(parents=True,exist_ok=True)
            a.report.write_text(json.dumps(result,indent=2)+'\n')
            print(json.dumps(result))
            return 0 if result['passed'] else 1
        finally:
            stop(daemon)


if __name__ == '__main__':
    raise SystemExit(main())
