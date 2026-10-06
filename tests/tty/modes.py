#!/usr/bin/env python3
"""Real PTY journeys for delegation, partial failure and the single careful checker.
No remote model or credentials: the local SSE fixture drives real Rusty workers.
"""
import argparse
import http.server
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import threading

from capture import Provider


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--case',choices=['subagent','swarm','careful'])
    parser.add_argument('--reduced-only',action='store_true')
    args=parser.parse_args()
    args.output.mkdir(parents=True,exist_ok=True)
    scripts=Path(__file__).resolve().parent
    spec=importlib.util.spec_from_file_location('screen',scripts/'inspect.py')
    screen=importlib.util.module_from_spec(spec);spec.loader.exec_module(screen)
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Provider)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    endpoint=f'http://127.0.0.1:{server.server_port}/v1'
    reports=[]
    try:
        for case,mode,agents in [('subagent','standard','auto'),('swarm','vibe','swarm'),('careful','careful','off')]:
            if args.case and args.case!=case:continue
            for width in ([80] if args.reduced_only else [24,40,80,120]):
                dest=(args.output/f'{case}-{width}').resolve();dest.mkdir(exist_ok=True)
                (dest/'proof.txt').write_text('route and test evidence\n')
                env=dict(os.environ);env.pop('RUSTY_NO_ANIM',None)
                if args.reduced_only:env['RUSTY_NO_ANIM']='1'
                subprocess.run(['expect',str(scripts/'steady.exp'),str(args.binary.resolve()),endpoint,str(width),str(dest),f'ui-{case}',mode,agents],cwd=dest,env=env,check=True)
                result,frames,_=screen.inspect(dest/f'terminal-{width}.ansi',width,require_scroll=False)
                text=(dest/f'terminal-{width}.ansi').read_text()
                stage='checker' if case=='careful' else case
                assert frames,(case,width,'no strip captured')
                assert any(stage in row and ('rusty /' in row if width>=31 else f'{mode}/' in row) for frame in frames for row in frame),(case,width,'stage missing from strip')
                if args.reduced_only:
                    shapes=set()
                    for frame in frames:
                        heading=next((i for i,row in enumerate(frame) if f'rusty / {stage}' in row),None)
                        if heading is not None:shapes.add('\n'.join(row[2:18] for row in frame[heading-1:heading+5]))
                    assert len(shapes)==1,(case,'reduced motion changed geometry',len(shapes))
                if width>=60:
                    assert 'read-only' in text
                    assert mode in text
                    if case=='swarm':
                        assert '1 failed' in text and '2×' in text
                        assert '3/3 returned' in text
                    elif case=='careful':
                        trajectory=json.loads((dest/f'trajectory-{width}.json').read_text())
                        # Count real main-agent checker reports; it must still check once.
                        history=trajectory.get('messages',trajectory.get('history',[]))
                        reviews=sum(m.get('role')=='user' and "[checker's review]" in str(m.get('content','')) for m in history)
                        assert reviews==1,(case,'extra or missing checker',reviews)
                result.update(case=case,mode=mode,stage=stage,reduced_motion=args.reduced_only)
                reports.append(result)
    finally:
        server.shutdown();server.server_close()
    (args.output/'modes-checks.json').write_text(json.dumps({'passed':True,'checks':reports},indent=2)+'\n')
    print(json.dumps({'passed':True,'journeys':len(reports)}))


if __name__=='__main__':main()
