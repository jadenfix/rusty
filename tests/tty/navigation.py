#!/usr/bin/env python3
"""Settings, cancellation and exit through real Rusty PTYs and a local provider."""
import argparse
import http.server
import json
from pathlib import Path
import shutil
import subprocess
import threading
from input import Provider

CASES=['settings','resume-settings','corrupt-settings','completion-escape','completion-cancel','completion-back','completion-cycle','settings-after-stop','mode-navigation','quit','quit-short','eof','double-cancel','exit','busy-quit','busy-eof','approval-cancel']

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--case',action='append',choices=CASES)
    args=parser.parse_args();root=args.output.resolve();root.mkdir(parents=True,exist_ok=True)
    scripts=Path(__file__).resolve().parent
    prefix=(scripts/'input.exp').read_text().split('switch -- $case {')[0]
    prefix=prefix.replace('set cols [lindex $argv 4]','set cols [lindex $argv 4]\nset rows [lindex $argv 5]')
    prefix=prefix.replace('stty rows 24 columns $cols','stty rows $rows columns $cols')
    # The shell retains the slave PTY long enough to compare the actual termios
    # after exit, rather than inferring restoration from ANSI cursor bytes.
    state_script=root/'tty-state.py'
    state_script.write_text("import json,termios\nt=termios.tcgetattr(0)\nt[3]&=~getattr(termios,'PENDIN',0)\nt[6]=[v.hex() if isinstance(v,bytes) else v for v in t[6]]\nprint(json.dumps(t))\n")
    # PENDIN is maintained by the macOS kernel when input is reprocessed; all
    # actual input/output/local flags, control bytes and speeds must match.
    prefix=prefix.replace('spawn -noecho $bin', 'spawn -noecho /bin/sh -c {stty sane; state=$1; shift; before=$(python3 "$state"); "$@"; code=$?; after=$(python3 "$state"); if [ "$before" = "$after" ]; then printf "\\nTTY_RESTORED\\n"; else printf "\\nTTY_DIRTY %s %s\\n" "$before" "$after"; fi; exit "$code"} rusty-tty [file join [file dirname $dest] tty-state.py] $bin')
    prefix=prefix.replace('spawn -noecho /bin/sh', 'set nav_args [list --permissions $perms --mode $mode --agents $agents --memory off]\nif {$case eq "resume-settings" || $case eq "corrupt-settings"} {set nav_args [list --memory off]}\nspawn -noecho /bin/sh')
    prefix=prefix.replace('$bin --permissions $perms --mode $mode --agents $agents --memory off', '$bin {*}$nav_args')
    driver=root/'navigation.exp';driver.write_text(prefix+(scripts/'navigation.exp').read_text())
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Provider)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    reports=[]
    expected_journeys=len(args.case or CASES)*4
    def receipt(passed):
        (root/'checks.json').write_text(json.dumps({'passed':passed,'expected_journeys':expected_journeys,'journeys':reports},indent=2)+'\n')
    receipt(False)
    try:
        for case in args.case or CASES:
            for cols,rows in [(24,8),(40,12),(80,24),(120,24)]:
                dest=root/f'{case}-{cols}x{rows}';dest.mkdir(exist_ok=True)
                if (dest/'home').exists():shutil.rmtree(dest/'home')
                (dest/'proof.txt').write_text('fixture\n')
                if case in ['resume-settings','corrupt-settings']:
                    home=dest/'home';home.mkdir(exist_ok=True)
                    value=json.dumps({'theme':'amber','font':'minimal','view':'adhd','suggestions':False,'execution_mode':'careful','agents':{'mode':'off'}}) if case=='resume-settings' else '{not valid json'
                    (home/'settings.json').write_text(value)

                start=len(Provider.requests)
                proc=subprocess.run(['expect',str(driver),str(args.binary.resolve()),f'http://127.0.0.1:{server.server_port}/v1',str(dest),case,str(cols),str(rows)],cwd=dest,capture_output=True,text=True)
                (dest/'driver.txt').write_text(proc.stdout+proc.stderr)
                if proc.returncode:raise AssertionError(f'{case}-{cols}x{rows}: {proc.stdout}{proc.stderr}')
                prompts=[next(m['content'] for m in reversed(r['messages']) if m['role']=='user') for r in Provider.requests[start:]]
                expected={'settings':['instant'],'settings-after-stop':['hold','hold','instant'],'mode-navigation':['instant\n\n(Loop run 1; this prompt repeats every 30s.)'],'busy-quit':['hold'],'busy-eof':['hold'],'approval-cancel':['approve']}.get(case,[])
                assert prompts==expected,(case,prompts)
                if case in ['settings','settings-after-stop']:
                    saved=json.loads((dest/'home/settings.json').read_text())
                    assert saved['theme']=='calm',saved
                    if case=='settings':
                        assert saved['font']=='minimal' and saved['view']=='default' and saved['tools']=='local' and not saved['suggestions'],saved
                lines=(dest/'events.tsv').read_text().splitlines();first=int(lines[0].split('\t')[0])
                with (dest/'terminal.cast').open('w') as cast:
                    cast.write(json.dumps({'version':2,'width':cols,'height':rows})+'\n')
                    for line in lines:
                        at,data=line.split('\t');cast.write(json.dumps([(int(at)-first)/1000,'o',bytes.fromhex(data).decode('utf-8')])+'\n')
                reports.append({'case':case,'width':cols,'height':rows,'passed':True,'terminal_restored':True,'prompts':prompts})
                receipt(False)
                print(proc.stdout.strip(),flush=True)
    finally:server.shutdown();server.server_close()
    assert len(reports)==expected_journeys
    receipt(True)

if __name__=='__main__':main()
