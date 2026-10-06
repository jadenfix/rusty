#!/usr/bin/env python3
"""Record one complete CLI session: edit/test, editable drafts, workers and review.
The local SSE script drives real Rusty tools; it is not a live-model quality demo.
"""
import argparse
import copy
import http.server
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import time
from capture import Provider as Modes

EDIT='Fix the route helper and run its tests.'
SUB='Inspect the route helper with a read-only worker.'
SWARM='Inspect routes, failures and tests in parallel.'
CAREFUL='Write a short route note and review the change carefully.'
HOLD='Explore another route design until I interrupt.'
SUMMARY='Summarize the route changes'
ALIASES={SUB:'ui-subagent',SWARM:'ui-swarm',CAREFUL:'ui-careful'}
OLD='if path == "/health" { "pending" } else { "unknown" }'
NEW='if path == "/health" { "ok" } else { "not found" }'
SOURCE='''fn route(path: &str) -> &str {
    if path == "/health" { "pending" } else { "unknown" }
}
#[test] fn health() { assert_eq!(route("/health"), "ok"); }
#[test] fn missing() { assert_eq!(route("/missing"), "not found"); }
'''

class Provider(Modes):
    requests=[]
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        self.requests.append(body)
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
        def send(delta,finish=None):
            delta=dict(delta)
            if delta.get('content')=='CAPTURE_COMPLETE\n':delta['content']='Inspection complete. The available reports are shown above.\n'
            payload={'choices':[{'delta':delta,'finish_reason':finish}]}
            self.wfile.write(('data: '+json.dumps(payload)+'\n\n').encode());self.wfile.flush()
        def tool(name,args):
            time.sleep(.7)
            send({'tool_calls':[{'index':0,'id':f'demo-{len(self.requests)}','type':'function','function':{'name':name,'arguments':json.dumps(args)}}]},'tool_calls')
        prompt=next(m['content'] for m in reversed(body['messages']) if m['role']=='user')
        try:
            alias=next((i for i in range(len(body['messages'])-1,-1,-1) if body['messages'][i]['role']=='user' and body['messages'][i]['content'] in ALIASES),None)
            if any(m['role']=='user' and (m['content'].startswith('UI_WORKER_') or m['content'].startswith('Check this proposed completion once')) for m in body['messages']):
                self.ui_case(body,prompt,send)
            elif alias is not None and not any(m['role']=='user' and m['content'] in [EDIT,SUMMARY,HOLD] for m in body['messages'][alias+1:]):
                scoped=copy.deepcopy(body)
                scoped['messages']=[body['messages'][0]]+scoped['messages'][alias:]
                scoped['messages'][1]['content']=ALIASES[body['messages'][alias]['content']]
                self.ui_case(scoped,prompt,send)
            elif prompt==EDIT:
                start=max(i for i,m in enumerate(body['messages']) if m['role']=='user' and m['content']==EDIT)
                count=sum(m['role']=='tool' for m in body['messages'][start:])
                actions=[('read_file',{'path':'router.rs'}),('bash',{'command':'test -f router.rs && rustc --test router.rs -o router-tests && ./router-tests'}),('edit_file',{'path':'router.rs','old_string':OLD,'new_string':NEW}),('bash',{'command':'test -f router.rs && rustc --test router.rs -o router-tests && ./router-tests'})]
                if count<len(actions):tool(*actions[count])
                else:
                    for line in ['The health route returns ok; missing routes return not found.','Both Rust tests pass. The earlier failing tests are fixed.']:
                        send({'content':line+'\n'});time.sleep(.6)
                    send({},'stop')
            elif prompt==HOLD:
                send({'content':'Exploring the next route design. You can steer me at any time.\n'})
                for _ in range(100):send({'reasoning_content':'Comparing route choices.\n'});time.sleep(.1)
                send({},'stop')
            else:
                time.sleep(.5)
                send({'content':'The helper now handles health and missing routes. Two tests passed.\n'},'stop')
            self.wfile.write(b'data: [DONE]\n\n');self.wfile.flush()
        except (BrokenPipeError,ConnectionResetError):pass

FLOW=r'''
proc type_line {text} {
    set prefix ""
    foreach character [split $text ""] {
        send -- $character
        append prefix $character
        watch "› $prefix"
        after 45
    }
}
proc local_command {text output} {
    send -- "$text\r"
    watch $output
    watch {PASTE_ON\r› \r}
}
local_command "/permissions auto" {permissions auto}
send -- "Fix the route helper and run its tests.\r"
watch {rusty /}
type_line "Summarize the route changes"
watch {Both Rust tests pass}
watch {draft kept}
watch {PASTE_ON\r› Summarize the route changes\r}
after 700
send -- "\r"
watch {Two tests passed}
watch {PASTE_ON\r› \r}
local_command "/agents auto" {agents auto}
send -- "Inspect the route helper with a read-only worker.\r"
watch {rusty / subagent}
watch {Inspection complete}
watch {PASTE_ON\r› \r}
local_command "/mode vibe" {execution vibe}
local_command "/agents swarm" {agents swarm}
send -- "Inspect routes, failures and tests in parallel.\r"
watch {rusty / swarm}
watch {Inspection complete}
watch {PASTE_ON\r› \r}
local_command "/mode careful" {execution careful}
local_command "/agents off" {agents off}
send -- "Write a short route note and review the change carefully.\r"
watch {rusty / checker}
watch {Inspection complete}
watch {PASTE_ON\r› \r}
local_command "/mode standard" {execution standard}
send -- "Explore another route design until I interrupt.\r"
watch {You can steer me at any time}
type_line "Summarize the route changes"
send -- "\r"
watch {stopped}
watch {Two tests passed}
watch {PASTE_ON\r› \r}
after 500
send -- "/exit\r"
expect {
    -re {.+} {binary scan [encoding convertto utf-8 $expect_out(0,string)] H* bytes;puts $events "[clock milliseconds]\t$bytes";exp_continue}
    eof {}
    timeout {error "session did not exit"}
}
close $events
close $metrics
set result [wait]
if {[lindex $result 3]!=0} {error "demo failed: $result"}
puts "PASS complete terminal demo"
'''

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);args=p.parse_args()
    binary=args.binary.resolve();dest=args.output.resolve();dest.mkdir(parents=True,exist_ok=True)
    if (dest/'home').exists():shutil.rmtree(dest/'home')
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Provider)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    prefix=Path(__file__).with_name('input.exp').read_text().split('switch -- $case {')[0]
    script=dest/'demo.exp';script.write_text(prefix+FLOW)
    try:
        with tempfile.TemporaryDirectory(prefix='rusty-cli-demo-') as work:
            Path(work,'router.rs').write_text(SOURCE);Path(work,'proof.txt').write_text('route tests and health endpoint\n')
            proc=subprocess.run(['expect',str(script),str(binary),f'http://127.0.0.1:{server.server_port}/v1',str(dest),'demo','120'],cwd=work,capture_output=True,text=True)
            (dest/'driver.txt').write_text(proc.stdout+proc.stderr)
            if proc.returncode:raise RuntimeError(proc.stdout+proc.stderr)
            assert NEW in Path(work,'router.rs').read_text()
            shutil.copyfile(Path(work,'router.rs'),dest/'router-result.rs')
        checker_requests=[r for r in Provider.requests if any(m['role']=='user' and m['content'].startswith('Check this proposed completion once') for m in r['messages'])]
        assert sum(not any(m['role']=='tool' for m in r['messages']) for r in checker_requests)==1, 'expected exactly one careful checker'
        rows=(dest/'events.tsv').read_text().splitlines();first=int(rows[0].split('\t')[0])
        with (dest/'full-session.cast').open('w') as cast:
            cast.write(json.dumps({'version':2,'width':120,'height':24,'timestamp':first//1000,'title':'Rusty: editing, drafts, workers and careful review'})+'\n')
            for line in rows:
                at,data=line.split('\t');cast.write(json.dumps([(int(at)-first)/1000,'o',bytes.fromhex(data).decode('utf-8')])+'\n')
        (dest/'requests.json').write_text(json.dumps(Provider.requests,indent=2)+'\n')
        print(proc.stdout.strip())
    finally:server.shutdown();server.server_close()

if __name__=='__main__':main()
