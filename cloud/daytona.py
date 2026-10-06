# /// script
# requires-python = ">=3.10"
# dependencies = ["daytona", "python-dotenv"]
# ///
"""Run rusty in a Daytona cloud sandbox.

    uv run cloud/daytona.py prepare                      # once per rusty build
    uv run cloud/daytona.py run --repo https://github.com/you/project \
        --goal "get the test suite green"
    uv run cloud/daytona.py status|logs|export|stop <run-id>
    uv run cloud/daytona.py list
    uv run cloud/daytona.py prune [--yes]                # old rusty snapshots

`prepare` builds a Daytona snapshot from a digest-pinned Debian image plus
your exact rusty binary (`docker build --target bin -o out .`), named after
their hash, so every run starts from the same prepared environment.

`run` starts rusty in the background and follows its output. Ctrl-C only
stops following; the run keeps going and `logs`/`status` pick it up again.
When it finishes, rusty's patch, new files, logs, trajectory and session
state are downloaded to cloud-runs/<run-id>/ before the sandbox is deleted.
If the export fails, the sandbox is kept.

Needs DAYTONA_API_KEY plus your model keys (NVIDIA_API_KEY, ...) in the
environment or .env. Keys reach the sandbox as environment variables only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
import shlex
import sys
import time
from pathlib import Path

BASE_IMAGE = "debian:bookworm-slim@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251"
PACKAGES = ["ca-certificates", "curl", "git", "make", "python3", "python3-venv", "ripgrep"]
KEY_PREFIXES = ("NVIDIA_API_KEY", "RUSTY_")
RUNS = Path("cloud-runs")
SNAPSHOT_NAME = re.compile(r"^rusty-[0-9a-f]{12}$")
# Paths inside the sandbox; the shell expands $HOME.
WORK = "$HOME/work"
RUN = "$HOME/rusty-run"
EXPORTS = ["patch.diff", "new-files.txt", "new-files.tgz", "out.log", "stderr.log", "trajectory.json", "session.tgz", "memory.json.gz", "exit"]


# ----------------------------------------------------------------- sandbox


class Remote:
    """The few things the launcher needs from a sandbox."""

    def __init__(self, sandbox):
        self.sandbox = sandbox
        self.id = sandbox.id
        self._home = None

    def sh(self, cmd: str, timeout: int = 600) -> tuple[int, str]:
        r = self.sandbox.process.exec(f"bash -lc {shlex.quote(cmd)}", timeout=timeout)
        return r.exit_code, r.result

    def start(self, cmd: str) -> str:
        from daytona import SessionExecuteRequest

        try:
            self.sandbox.process.create_session("rusty")
        except Exception:
            pass  # already there after a reconnect
        req = SessionExecuteRequest(command=f"bash -lc {shlex.quote(cmd)}", run_async=True)
        return self.sandbox.process.execute_session_command("rusty", req).cmd_id

    def path(self, p: str) -> str:
        if self._home is None:
            self._home = self.sh("printf %s $HOME", timeout=30)[1].strip()
        return p.replace("$HOME", self._home)

    def upload(self, local: str, path: str) -> None:
        self.sandbox.fs.upload_file(local, self.path(path))

    def download(self, path: str) -> bytes | None:
        try:
            return self.sandbox.fs.download_file(self.path(path))
        except Exception:
            return None


def daytona_client():
    from daytona import Daytona

    return Daytona()


# ------------------------------------------------------------------- state


def state_path(run_id: str) -> Path:
    return RUNS / run_id / "run.json"


def save(state: dict) -> None:
    p = state_path(state["id"])
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(state, indent=2) + "\n")


def load(run_id: str) -> dict:
    p = state_path(run_id)
    if not p.exists():
        sys.exit(f"no run {run_id} in {RUNS}/ (see `list`)")
    return json.loads(p.read_text())


def model_env() -> dict:
    return {k: v for k, v in os.environ.items() if k.startswith(KEY_PREFIXES) and k != "RUSTY_HOME"}


# ------------------------------------------------------------- environment


def snapshot_name(binary: Path) -> str:
    h = hashlib.sha256()
    h.update(BASE_IMAGE.encode())
    h.update(" ".join(PACKAGES).encode())
    h.update(binary.read_bytes())
    advisor = binary.with_name("rusty-memoryd")
    if advisor.is_file():
        h.update(advisor.read_bytes())
    return f"rusty-{h.hexdigest()[:12]}"


def find_binary(arg: str | None) -> Path:
    binary = Path(arg or "out/rusty")
    if not binary.is_file():
        sys.exit(f"{binary} not found. Build the static Linux binary first:\n  docker build --target bin -o out .")
    return binary


def cmd_prepare(args) -> int:
    from daytona import CreateSnapshotParams, Image

    binary = find_binary(args.binary)
    advisor = binary.with_name("rusty-memoryd")
    if not advisor.is_file():
        sys.exit("rusty-memoryd missing; rebuild both binaries with docker build --target bin -o out .")
    name = snapshot_name(binary)
    d = daytona_client()
    try:
        d.snapshot.get(name)
        print(f"☾ {name} is already prepared")
        return 0
    except Exception:
        pass
    image = (
        Image.base(BASE_IMAGE)
        .run_commands(
            "apt-get update -qq && apt-get install -y -qq --no-install-recommends "
            + " ".join(PACKAGES)
            + " && rm -rf /var/lib/apt/lists/*"
        )
        .add_local_file(str(binary), "/usr/local/bin/rusty")
        .add_local_file(str(advisor), "/usr/local/bin/rusty-memoryd")
        .run_commands("chmod 755 /usr/local/bin/rusty /usr/local/bin/rusty-memoryd && rusty --version")
    )
    print(f"☾ preparing {name} from {BASE_IMAGE.split('@')[0]} and {binary}…", file=sys.stderr)
    d.snapshot.create(CreateSnapshotParams(name=name, image=image), on_logs=lambda line: print(line, file=sys.stderr))
    print(f"☾ prepared {name}")
    return 0


def cmd_prune(args) -> int:
    d = daytona_client()
    keep = {args.keep} if args.keep else set()
    if Path("out/rusty").is_file():
        keep.add(snapshot_name(Path("out/rusty")))
    page = d.snapshot.list()
    items = getattr(page, "items", page)
    old = [s.name for s in items if SNAPSHOT_NAME.match(s.name or "") and s.name not in keep]
    if not old:
        print("nothing to prune")
        return 0
    for name in old:
        if args.yes:
            # Re-check the name right before deleting; only rusty's own snapshots.
            snap = d.snapshot.get(name)
            if SNAPSHOT_NAME.match(snap.name):
                d.snapshot.delete(snap)
                print(f"deleted {name}")
        else:
            print(f"would delete {name}")
    if not args.yes:
        print("(dry run; pass --yes to delete)")
    return 0


# --------------------------------------------------------------------- run


def setup_and_start(remote, state: dict, task: list[str]) -> None:
    """Clones the project, records where it started, and starts rusty."""
    ref = state.get("ref") or ""
    checkout = f" && git checkout -q {shlex.quote(ref)}" if ref else ""
    code, out = remote.sh(
        f"rm -rf {WORK} {RUN} && mkdir -p {RUN} && git clone -q {shlex.quote(state['repo'])} {WORK} && "
        f"cd {WORK}{checkout} && git rev-parse HEAD",
        timeout=900,
    )
    if code != 0:
        raise RuntimeError(f"clone failed:\n{out}")
    state["base_commit"] = out.strip().splitlines()[-1]
    memory = state.get("memory_mode", "legacy")
    project_id = state.get("project_id") or state["repo"]
    prefix = f"RUSTY_PROJECT_ID={shlex.quote(project_id)} "
    if memory in ("on", "deep"):
        code, _ = remote.sh("command -v rusty-memoryd", timeout=30)
        if code != 0:
            raise RuntimeError("snapshot lacks rusty-memoryd; rebuild and prepare it")
    if state.get("memory_input"):
        remote.upload(state["memory_input"], f"{RUN}/memory-input.json.gz")
        code, _ = remote.sh(f"cd {WORK} && {prefix}rusty-memoryd import {RUN}/memory-input.json.gz", timeout=30)
        if code != 0:
            raise RuntimeError("memory import failed; check explicit project identity and snapshot scope")
    words = ["rusty", "--yolo", "--stats", "--mode", state["mode"], "--agents", state["agents"], "--memory", memory]
    rusty = shlex.join([*words, "--trajectory", "TRAJECTORY", *task]).replace("TRAJECTORY", f"{RUN}/trajectory.json")
    state["command"] = rusty
    state["cmd_id"] = remote.start(
        f"cd {WORK} && {prefix}NO_COLOR=1 {rusty} </dev/null >{RUN}/out.log 2>{RUN}/stderr.log; echo $? >{RUN}/exit"
    )
    state["status"] = "running"
    save(state)


def exit_code(remote) -> int | None:
    code, out = remote.sh(f"cat {RUN}/exit 2>/dev/null", timeout=60)
    out = out.strip()
    return int(out) if code == 0 and out.lstrip("-").isdigit() else None


def follow(remote, state: dict, poll: float = 5.0) -> int | None:
    """Streams new output until rusty exits. Ctrl-C stops following only."""
    offset = state.get("log_offset", 0)

    def drain():
        nonlocal offset
        _, chunk = remote.sh(f"tail -c +{offset + 1} {RUN}/out.log 2>/dev/null", timeout=60)
        if chunk:
            sys.stdout.write(chunk)
            sys.stdout.flush()
            offset += len(chunk.encode())

    try:
        while True:
            drain()
            done = exit_code(remote)
            if done is not None:
                # Output written between the read and the exit check.
                drain()
                state["log_offset"] = offset
                return done
            time.sleep(poll)
    except KeyboardInterrupt:
        state["log_offset"] = offset
        save(state)
        print(
            f"\n☾ still running in {state['sandbox']}. Pick it up with:\n"
            f"  uv run cloud/daytona.py logs {state['id']}\n"
            f"  uv run cloud/daytona.py stop {state['id']}   (exports first)",
            file=sys.stderr,
        )
        return None


def export(remote, state: dict) -> bool:
    """Downloads the patch, new files, logs and session state. True if the
    essentials (patch and log) arrived."""
    memory_ok = True
    if state.get("memory_mode") in ("on", "deep"):
        prefix = f"RUSTY_PROJECT_ID={shlex.quote(state.get('project_id') or state['repo'])} "
        code, _ = remote.sh(f"cd {WORK} && rm -f {RUN}/memory.json.gz && {prefix}rusty-memoryd export {RUN}/memory.json.gz", timeout=30)
        memory_ok = code == 0
    base = shlex.quote(state["base_commit"])
    code, out = remote.sh(
        f"cd {WORK} && git add -A && git diff --cached --binary {base} >{RUN}/patch.diff && "
        f"git diff --cached --name-only --diff-filter=A {base} >{RUN}/new-files.txt && "
        f"COPYFILE_DISABLE=1 tar czf {RUN}/new-files.tgz -T {RUN}/new-files.txt && "
        # Session state without any key files.
        f"{{ COPYFILE_DISABLE=1 tar czf {RUN}/session.tgz -C $HOME --exclude='.env' --exclude='*.env' --exclude='.config/rusty/memory' .config/rusty 2>/dev/null || true; }}",
        timeout=600,
    )
    if code != 0:
        print(f"☾ export failed in the sandbox:\n{out}", file=sys.stderr)
        return False
    dest = RUNS / state["id"]
    dest.mkdir(parents=True, exist_ok=True)
    manifest = {}
    for name in EXPORTS:
        data = remote.download(f"{RUN}/{name}")
        if data is None:
            continue
        (dest / name).write_bytes(data)
        manifest[name] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    (dest / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    ok = "patch.diff" in manifest and "out.log" in manifest and memory_ok
    if state.get("memory_mode") in ("on", "deep"):
        ok = ok and "memory.json.gz" in manifest
    state["exported"] = ok
    state["export_dir"] = str(dest)
    save(state)
    return ok


def finish(d, remote, state: dict, keep: bool) -> int:
    code = exit_code(remote)
    state["exit_code"] = code
    state["status"] = "finished"
    save(state)
    ok = export(remote, state)
    dest = RUNS / state["id"]
    if ok:
        print(f"\n☾ exported to {dest}/ (apply with: git apply {dest}/patch.diff)", file=sys.stderr)
    if keep or not ok:
        reason = "kept (--keep)" if keep else "kept because the export failed"
        print(f"☾ sandbox {state['sandbox']} {reason}", file=sys.stderr)
        return code if code is not None else 1
    delete_sandbox(d, remote, state)
    return code if code is not None else 1


def delete_sandbox(d, remote, state: dict) -> None:
    sandbox = remote.sandbox
    labels = getattr(sandbox, "labels", None) or {}
    # Only ever delete the sandbox this run created.
    if labels.get("app") == "rusty" and labels.get("rusty-run") == state["id"]:
        d.delete(sandbox)
        state["status"] = "deleted"
        save(state)
        print(f"☾ sandbox {state['sandbox']} deleted", file=sys.stderr)
    else:
        print(f"☾ not deleting {state['sandbox']}: its labels don't match run {state['id']}", file=sys.stderr)


def cmd_run(args) -> int:
    from daytona import CreateSandboxFromSnapshotParams, Resources

    env = model_env()
    if "NVIDIA_API_KEY" not in env and "RUSTY_API_KEY" not in env:
        sys.exit("set NVIDIA_API_KEY (or RUSTY_API_KEY) in the environment or .env")
    if args.memory_input and args.memory_mode not in ("on", "deep"):
        sys.exit("--memory-input requires --memory-mode on or deep")
    if args.memory_input and not Path(args.memory_input).is_file():
        sys.exit("memory input file not found")
    snapshot = args.snapshot or snapshot_name(find_binary(args.binary))
    d = daytona_client()
    try:
        d.snapshot.get(snapshot)
    except Exception:
        sys.exit(f"snapshot {snapshot} isn't prepared yet. Run:\n  uv run cloud/daytona.py prepare")
    run_id = time.strftime("%Y%m%d-%H%M%S-") + secrets.token_hex(2)
    state = {
        "id": run_id,
        "repo": args.repo,
        "ref": args.ref,
        "snapshot": snapshot,
        "mode": args.mode,
        "agents": args.agents,
        "memory_mode": args.memory_mode,
        "memory_input": str(Path(args.memory_input).resolve()) if args.memory_input else None,
        "project_id": args.project_id or args.repo,
        "task": args.goal or args.prompt,
        "started": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "status": "creating",
    }
    print(f"☾ run {run_id}: creating a sandbox from {snapshot}…", file=sys.stderr)
    resources = Resources(cpu=args.cpu, memory=args.memory) if (args.cpu or args.memory) else None
    sandbox = d.create(
        CreateSandboxFromSnapshotParams(
            snapshot=snapshot,
            env_vars=env,
            labels={"app": "rusty", "rusty-run": run_id},
            # A run may be followed from nowhere; don't stop it for being quiet.
            auto_stop_interval=0,
            **({"resources": resources} if resources else {}),
        ),
        timeout=300,
    )
    state["sandbox"] = sandbox.id
    save(state)
    remote = Remote(sandbox)
    task = ["--goal", args.goal] if args.goal else [args.prompt]
    try:
        setup_and_start(remote, state, task)
    except Exception as e:
        print(f"☾ {e}", file=sys.stderr)
        delete_sandbox(d, remote, state)
        return 1
    print(f"☾ running: {state['command']}", file=sys.stderr)
    if follow(remote, state) is None:
        return 130
    return finish(d, remote, state, args.keep)


def reconnect(run_id: str):
    state = load(run_id)
    d = daytona_client()
    return d, Remote(d.get(state["sandbox"])), state


def cmd_status(args) -> int:
    state = load(args.run)
    line = f"{state['id']}  {state['status']}  {state.get('task', '')[:60]}"
    if state["status"] == "running":
        _, remote, _ = reconnect(args.run)
        code = exit_code(remote)
        line += "  (still working)" if code is None else f"  (rusty exited {code}; run `logs` or `stop` to export)"
    print(line)
    return 0


def cmd_logs(args) -> int:
    d, remote, state = reconnect(args.run)
    if follow(remote, state) is None:
        return 130
    return finish(d, remote, state, args.keep)


def cmd_export(args) -> int:
    _, remote, state = reconnect(args.run)
    ok = export(remote, state)
    print(f"☾ {'exported to ' + state['export_dir'] if ok else 'export failed'}", file=sys.stderr)
    return 0 if ok else 1


def cmd_stop(args) -> int:
    d, remote, state = reconnect(args.run)
    if exit_code(remote) is None:
        remote.sh("pkill -INT -x rusty; sleep 3; pkill -x rusty; true", timeout=60)
    return finish(d, remote, state, args.keep)


def cmd_list(args) -> int:
    if not RUNS.is_dir():
        print("no runs yet")
        return 0
    for p in sorted(RUNS.glob("*/run.json")):
        s = json.loads(p.read_text())
        print(f"{s['id']}  {s['status']:<9}  {s.get('task', '')[:70]}")
    return 0


def main(argv=None) -> int:
    try:
        from dotenv import load_dotenv

        load_dotenv()
    except ImportError:
        pass
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("prepare", help="build the pinned snapshot for your rusty binary")
    p.add_argument("--binary", help="static Linux rusty binary (default out/rusty)")
    p = sub.add_parser("run", help="start a run")
    p.add_argument("--repo", required=True, help="git URL (or path) of the project")
    p.add_argument("--ref", help="branch, tag or commit to start from")
    task = p.add_mutually_exclusive_group(required=True)
    task.add_argument("--goal", help="objective for a goal run")
    task.add_argument("--prompt", help="a single prompt instead of a goal")
    p.add_argument("--mode", default="auto", choices=["auto", "careful", "standard", "vibe"])
    p.add_argument("--agents", default="off", choices=["off", "sub", "swarm", "auto"])
    p.add_argument("--snapshot", help="use this prepared snapshot instead of the one for out/rusty")
    p.add_argument("--binary", help="static Linux rusty binary the snapshot was built from")
    p.add_argument("--cpu", type=int)
    p.add_argument("--memory", type=int, help="GiB")
    p.add_argument("--memory-mode", default="legacy", choices=["legacy", "off", "on", "deep"])
    p.add_argument("--memory-input", help="scoped gzip export from rusty-memoryd")
    p.add_argument("--project-id", help="same RUSTY_PROJECT_ID used locally; defaults to repo URL")
    p.add_argument("--keep", action="store_true", help="keep the sandbox after exporting")
    for name, help_ in [
        ("status", "is it still working?"),
        ("logs", "follow the output, then export"),
        ("export", "download the results now"),
        ("stop", "stop rusty, export, delete"),
    ]:
        p = sub.add_parser(name, help=help_)
        p.add_argument("run")
        p.add_argument("--keep", action="store_true", help="keep the sandbox after exporting")
    sub.add_parser("list", help="runs started from here")
    p = sub.add_parser("prune", help="delete old rusty-* snapshots (dry run unless --yes)")
    p.add_argument("--yes", action="store_true")
    p.add_argument("--keep", help="also keep this snapshot")
    args = ap.parse_args(argv)
    return {
        "prepare": cmd_prepare,
        "run": cmd_run,
        "status": cmd_status,
        "logs": cmd_logs,
        "export": cmd_export,
        "stop": cmd_stop,
        "list": cmd_list,
        "prune": cmd_prune,
    }[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
