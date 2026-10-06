#!/usr/bin/env python3
"""Offline service quality checks; measure recall, silence, tone, and latency.
This is a labeled fixture corpus, not a claim of general semantic recall.
"""
import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time
from memory_smoke import rpc, stop

# Saved text and queries are intentionally distinct. No model produces gold labels.
CASES = [
    ('monorepo', 'Our monorepo GitHub repository is https://github.com/example/workspace; inspect its manifest first.',
     ['find the monorepo', 'search our GitHub repository', 'where is the workspace manifest']),
    ('test', 'Parser tests use python3 -m unittest; inspect parser.py before editing.',
     ['fix the parser tests', 'what command checks parser.py', 'verify parser with unittest']),
    ('api', 'Task board API returns items from /api/tasks; preserve the JSON response contract.',
     ['inspect task board API', 'check the JSON response contract', 'fix /api/tasks items']),
    ('build', 'Frontend builds use npm run build; inspect package.json before changing dependencies.',
     ['build frontend', 'check package.json dependencies', 'what runs frontend builds']),
    ('storage', 'SQLite migrations require a backup and dry run before changing the schema.',
     ['plan SQLite migrations', 'backup before schema changes', 'dry run the migrations']),
    ('alias', 'The main repository is a monorepo at https://github.com/example/workspace.',
     ['where is our source tree', 'open the main repository']),
]
NEGATIVE = ['tell me a joke', 'explain orbital mechanics', 'draw a sunflower', 'what is lunch today', 'explain photosynthesis']


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, default=Path('target/debug/rusty-memoryd'))
    p.add_argument('--report', type=Path, required=True)
    a = p.parse_args()
    env = {k:v for k,v in os.environ.items() if not k.startswith(('RUSTY_', 'NVIDIA_API_KEY'))}
    # macOS Unix sockets need a short path, independent of TMPDIR.
    with tempfile.TemporaryDirectory(prefix='rq-', dir='/tmp') as tmp:
        home = Path(tmp)
        daemon = subprocess.Popen([str(a.binary.resolve()), '--home', tmp], env=env,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            end = time.monotonic()+3
            while not (home/'memory/advisor.sock').exists():
                if daemon.poll() is not None or time.monotonic()>end: raise RuntimeError('startup')
                time.sleep(.01)
            ids = {}
            for label, text, _ in CASES:
                r = rpc(home, 'remember', {'kind':'fact','text':text})
                assert not r['error']; ids[label] = r['id']
            # A saturated store includes 506 unrelated lessons. Query labels never
            # reach the daemon, only the same begin/advice protocol used by rusty.
            for i in range(512-len(CASES)):
                assert not rpc(home, 'remember', {'kind':'fact','text':f'Unrelated catalog entry q{i}: violet acoustic telescope.'})['error']
            runs=[]; latencies=[]
            for label, _, queries in CASES:
                for q in queries:
                    session=f'query-{len(runs)}'
                    assert not rpc(home,'begin',{'text':q},seq=1,session=session)['error']
                    start=time.perf_counter_ns(); r=rpc(home,'advice',seq=1,session=session)
                    latencies.append((time.perf_counter_ns()-start)/1e6)
                    # Repository aliases may select either equivalent repository record.
                    expected={ids[label]} if label not in ('monorepo','alias') else {ids['monorepo'],ids['alias']}
                    runs.append(dict(query=q,kind='recall',hit=r['id'] in expected,selected=r['id']))
            silence=[]
            for i,q in enumerate(NEGATIVE):
                session=f'negative-{i}'
                rpc(home,'begin',{'text':q},seq=1,session=session)
                silence.append(dict(query=q,silent=not rpc(home,'advice',seq=1,session=session)['text']))
            # Separate project for explicit presentation preferences, avoiding L2 eviction.
            # Seed through the same project used by the public helper, after recall measurements.
            tone = rpc(home,'remember',{'kind':'preference','text':'Response style: concise, warm, three bullets.'})['id']
            rpc(home,'begin',{'text':'fix a serializer'},seq=1,session='tone')
            styled = rpc(home,'advice',seq=1,session='tone')['id']==tone
            rpc(home,'finish',{'complete':True},seq=2,session='tone')
            cleared=not rpc(home,'advice',seq=2,session='tone')['text']
            status=rpc(home,'status')['data']
            size=sum(f.stat().st_size for f in (home/'memory').iterdir() if f.is_file())
            # Real L2 survive restart; L1 does not.
            stop(daemon)
            daemon=subprocess.Popen([str(a.binary.resolve()),'--home',tmp],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
            end=time.monotonic()+3
            while True:
                try: after=rpc(home,'status')['data']; break
                except (OSError, ValueError):
                    if time.monotonic()>end: raise
                    time.sleep(.01)
            checks=dict(no_irrelevant_advice=all(x['silent'] for x in silence),response_style_selected=styled,
                        finished_turn_cleared=cleared,restart_preserves_l2=after['lessons']==status['lessons'],
                        restart_clears_l1=after.get('l1') is None,p95_under_20ms=sorted(latencies)[int(.95*(len(latencies)-1))]<20,
                        storage_under_20mib=size<20*1024*1024)
            result=dict(version=1,qualification='real memory daemon, labeled offline corpus; no model calls',
                        recall_hits=sum(x['hit'] for x in runs),recall_cases=len(runs),
                        recall=sum(x['hit'] for x in runs)/len(runs),queries=runs,silence=silence,
                        p50_ms=round(statistics.median(latencies),3),p95_ms=round(sorted(latencies)[int(.95*(len(latencies)-1))],3),
                        max_ms=round(max(latencies),3),storage_bytes=size,lessons=status['lessons'],checks=checks,
                        # Misses are reported honestly, not silently dropped from the denominator.
                        passed=all(checks.values()) and all(x['hit'] for x in runs))
            a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(result,indent=2)+'\n')
            print(json.dumps(result));return 0 if result['passed'] else 1
        finally: stop(daemon)

if __name__=='__main__': raise SystemExit(main())
