# /// script
# requires-python = ">=3.10"
# dependencies = ["daytona==0.220.0", "python-dotenv"]
# ///
"""A real Daytona/NVIDIA swarm journey. Verifies exported results outside
Rusty's workspace; keeps the sandbox if export fails. No raw logs in receipt.

uv run cloud/swarm_smoke.py --snapshot rusty-... --ref <commit> --report receipt.json
"""
import argparse
import importlib.util
import json
import os
import re
from pathlib import Path
import sys
import time

# Avoid cloud/daytona.py shadowing the installed SDK.
HERE = Path(__file__).resolve().parent
sys.path = [p for p in sys.path if Path(p or '.').resolve() != HERE]
spec = importlib.util.spec_from_file_location('launcher', HERE / 'daytona.py')
launcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launcher)

PROMPT = '''Use the swarm tool once with exactly three parallel read-only workers:
1. description "agents": read src/config.rs and list the four AgentsMode variants.
2. description "signals": read src/signal.rs and explain what reset() does to INTERRUPTED.
3. description "modes": read src/execution.rs and list the three ExecutionMode variants.
Each worker must inspect its file with read_file and give a concise factual report. No worker edits.
After collecting all three reports, use write_file to create swarm-report.md with three sections titled agents, signals, modes. Include the variants and the reset fact.
Use Bash to verify all three sections exist, then printf verified > swarm-proof.txt. Do not edit any existing files. Report completion briefly. The swarm call is explicitly requested, even though individual reads are small. No other investigation is needed.'''


def verify(dest: Path) -> dict:
    trace_path = dest / 'trajectory.json'
    trace = json.loads(trace_path.read_text()) if trace_path.is_file() else {}
    calls = [c for m in trace.get('messages', []) for c in m.get('tool_calls') or []]
    swarms = [c for c in calls if c['function']['name'] == 'swarm']
    jobs = [json.loads(c['function']['arguments']).get('tasks', []) for c in swarms]
    reports = [m.get('content', '') for m in trace.get('messages', []) if m.get('role') == 'tool' and m.get('tool_call_id') in {c['id'] for c in swarms}]
    patch = (dest / 'patch.diff').read_text() if (dest / 'patch.diff').is_file() else ''
    manifest = json.loads((dest / 'manifest.json').read_text()) if (dest / 'manifest.json').is_file() else {}
    state = json.loads((dest / 'run.json').read_text())
    checks = {
        'exit': state.get('exit_code') == 0,
        'one_three_worker_swarm': len(jobs) == 1 and len(jobs[0]) == 3,
        'all_workers_done': len(reports) == 1 and reports[0].count('(done)') == 3 and '(failed)' not in reports[0],
        'worker_tools': (dest / 'out.log').is_file() and len(re.findall(r'· [1-9][0-9]* calls', (dest / 'out.log').read_text())) >= 3,
        'lead_write_and_bash': {'write_file', 'bash'} <= {c['function']['name'] for c in calls},
        'report_and_proof': 'diff --git a/swarm-report.md b/swarm-report.md' in patch and '+verified' in patch,
        'expected_facts': all(s in patch.lower() for s in ['agents', 'signals', 'modes', 'swarm', 'careful', 'standard', 'vibe', 'false']),
        'exports': all(s in manifest for s in ['patch.diff', 'out.log', 'stderr.log', 'trajectory.json', 'session.tgz', 'memory.json.gz', 'exit']),
        'memory_hooks': (trace.get('memory') or {}).get('hooks', 0) > 0 and (trace.get('memory') or {}).get('timeouts', 0) == 0,
        'manifest_integrity': all((dest / name).is_file() and launcher.hashlib.sha256((dest / name).read_bytes()).hexdigest() == meta['sha256'] for name,meta in manifest.items()),
        'sandbox_deleted_after_export': state.get('status') == 'deleted' and state.get('exported') is True,
    }
    return dict(passed=all(checks.values()), checks=checks, run=state['id'], sandbox=state['sandbox'], snapshot=state['snapshot'], base_commit=state['base_commit'], model=trace.get('model'), totals=trace.get('totals'), memory=trace.get('memory'), exports=manifest)


def run_scripted(args):
    from daytona import CreateSandboxFromSnapshotParams
    d=launcher.daytona_client()
    run_id=time.strftime("%Y%m%d-%H%M%S-")+launcher.secrets.token_hex(2)
    state=dict(id=run_id,repo=args.repo,ref=args.ref,snapshot=args.snapshot,target=os.environ.get('DAYTONA_TARGET'),mode='standard',agents='swarm',memory_mode='on',status='creating',task='hosted deterministic swarm')
    sandbox=d.create(CreateSandboxFromSnapshotParams(snapshot=args.snapshot,env_vars={'RUSTY_API_KEY':'offline-test-key','RUSTY_BASE_URL':'http://127.0.0.1:8123/v1','RUSTY_NO_DOTENV':'1'},labels={'app':'rusty','rusty-run':run_id},auto_stop_interval=10),timeout=120)
    state['sandbox']=sandbox.id;launcher.save(state)
    remote=launcher.Remote(sandbox)
    remote.upload(str(HERE/'swarm_provider.py'),'$HOME/swarm-provider.py')
    remote.start('python3 $HOME/swarm-provider.py >$HOME/swarm-provider.log 2>&1', session='provider')
    code,_=remote.sh('for attempt in $(seq 1 20); do curl -fsS http://127.0.0.1:8123/health && exit 0; sleep .1; done; exit 1',timeout=15)
    if code!=0:
        launcher.delete_sandbox(d,remote,state)
        raise RuntimeError('scripted provider did not start')
    launcher.setup_and_start(remote,state,[PROMPT])
    done=launcher.follow(remote,state,poll=.2)
    if done is None:return 130
    data=remote.download('$HOME/swarm-metrics.json')
    launcher.private_write(launcher.RUNS/run_id/'mock-metrics.json',data or b'{}')
    return launcher.finish(d,remote,state,False)


def main():
    from dotenv import load_dotenv
    load_dotenv()
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--snapshot', required=True)
    p.add_argument('--ref', required=True)
    p.add_argument('--repo', default='https://github.com/jadenfix/rusty.git')
    p.add_argument('--report', type=Path, required=True)
    p.add_argument('--scripted-provider', action='store_true', help='real Daytona journey with a deterministic loopback provider')
    args = p.parse_args()
    previous = {s.parent.name for s in launcher.RUNS.glob('*/run.json')}
    start = time.monotonic()
    rc = run_scripted(args) if args.scripted_provider else launcher.main(['run', '--snapshot', args.snapshot, '--repo', args.repo, '--ref', args.ref, '--mode', 'standard', '--agents', 'swarm', '--memory-mode', 'on', '--prompt', PROMPT])
    new = [s.parent for s in launcher.RUNS.glob('*/run.json') if s.parent.name not in previous]
    if len(new) != 1:
        raise RuntimeError('could not identify exactly one new run; inspect cloud-runs before retrying')
    receipt = verify(new[0])
    receipt.update(version=1, hosted=True, live=not args.scripted_provider, provider='scripted' if args.scripted_provider else os.environ.get('RUSTY_BASE_URL','https://integrate.api.nvidia.com/v1'), launcher_exit=rc, wall_seconds=round(time.monotonic() - start, 3))
    if args.scripted_provider:
        metrics=json.loads((new[0]/'mock-metrics.json').read_text())
        receipt['scripted_metrics']=metrics
        receipt['checks']['parallel_read_only_workers']=metrics.get('peak_parallel')==3 and metrics.get('denied_writes')==3 and metrics.get('reads')==3
        receipt['passed']=all(receipt['checks'].values())
    receipt['passed'] = receipt['passed'] and rc == 0
    args.report.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'w') as f:
        json.dump(receipt, f, indent=2)
        f.write('\n')
    print(json.dumps(receipt))
    return 0 if receipt['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
