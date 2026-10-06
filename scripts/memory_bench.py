#!/usr/bin/env python3
"""Measure real socket advice latency with a full 512-lesson local store."""
import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time
from memory_smoke import rpc, stop


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary',type=Path,default=Path('target/release/rusty-memoryd'))
    p.add_argument('--report',type=Path,default=Path('memory-bench.json'))
    args=p.parse_args(); binary=args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix='rmb-') as root:
        home=Path(root)
        env={k:v for k,v in os.environ.items() if not k.startswith(('NVIDIA_API_KEY','RUSTY_'))}
        daemon=subprocess.Popen([str(binary),'--home',str(home)],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
        try:
            deadline=time.monotonic()+3
            while not (home/'memory/advisor.sock').exists():
                if time.monotonic()>deadline: raise RuntimeError('startup deadline')
                time.sleep(.01)
            for i in range(512):
                r=rpc(home,'remember',dict(kind='fact',text=f'Workspace package {i} requires inspect manifest then run Python checks.',evidence='observed result '*100))
                assert not r['error'],r['text']
            assert rpc(home,'status')['data']['lessons']==512
            # Cold load then 100 fresh decisions. begin writes real checkpoints;
            # advice times include the socket, selection, JSON and response.
            times=[];seq=0
            for _ in range(101):
                seq+=1; assert not rpc(home,'begin',{'text':'inspect workspace package manifest Python checks'},seq=seq)['error']
                start=time.perf_counter_ns();r=rpc(home,'advice',seq=seq);elapsed=(time.perf_counter_ns()-start)/1e6
                assert r['id'], 'no advice selected from full store'
                times.append(elapsed)
            p95=sorted(times[1:])[94]
            size=sum(f.stat().st_size for f in (home/'memory').iterdir() if f.is_file())
            result=dict(lessons=512,cold_ms=round(times[0],3),p50_ms=round(statistics.median(times[1:]),3),p95_ms=round(p95,3),max_ms=round(max(times),3),storage_bytes=size,binary_bytes=binary.stat().st_size,passed=p95<20 and size<20*1024*1024)
            args.report.parent.mkdir(parents=True,exist_ok=True);args.report.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
            return 0 if result['passed'] else 1
        finally:stop(daemon)
if __name__=='__main__':raise SystemExit(main())
