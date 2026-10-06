"""Offline tests for cloud/daytona.py.

A stand-in sandbox runs the launcher's real shell commands on this machine,
with a fake rusty on PATH, so tracking, detaching, export and cleanup are
tested end to end without a Daytona account.

    python3 -m unittest cloud/test_daytona.py
"""

import importlib.util
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

spec = importlib.util.spec_from_file_location("launcher", Path(__file__).with_name("daytona.py"))
launcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launcher)

FAKE_RUSTY = r"""#!/bin/bash
# Pretends to be rusty: edits a file, adds one, leaves session state behind.
traj=""
while [ $# -gt 0 ]; do [ "$1" = --trajectory ] && traj=$2; shift; done
echo "working on it"
sleep "${FAKE_RUSTY_SLEEP:-0}"
echo 'print("fixed")' > app.py
echo 'print("new")' > new_module.py
mkdir -p "$HOME/.config/rusty/sessions"
echo '{"messages": []}' > "$HOME/.config/rusty/sessions/last.json"
echo 'NVIDIA_API_KEY=nvapi-secret' > "$HOME/.config/rusty/.env"
echo '{"agent": "rusty"}' > "$traj"
echo '{"requests": 3}' >&2
echo "done"
"""


class FakeSandbox:
    def __init__(self, run_id):
        self.id = "sbx-1"
        self.labels = {"app": "rusty", "rusty-run": run_id}


class FakeRemote:
    """Runs commands locally with HOME pointing at a scratch directory."""

    def __init__(self, root: Path, run_id: str):
        self.home = root / "home"
        self.home.mkdir()
        bin_dir = root / "bin"
        bin_dir.mkdir()
        (bin_dir / "rusty").write_text(FAKE_RUSTY)
        (bin_dir / "rusty").chmod(0o755)
        (bin_dir / "rusty-memoryd").write_text('#!/bin/bash\nif [ "$1" = export ]; then printf compressed-snapshot > "$2"; fi\n')
        (bin_dir / "rusty-memoryd").chmod(0o755)
        self.env = {**os.environ, "HOME": str(self.home), "PATH": f"{bin_dir}:{os.environ['PATH']}"}
        self.sandbox = FakeSandbox(run_id)
        self.id = self.sandbox.id
        self.started = []

    def sh(self, cmd, timeout=600):
        r = subprocess.run(["bash", "-c", cmd], env=self.env, capture_output=True, text=True, timeout=timeout)
        return r.returncode, r.stdout + r.stderr

    def start(self, cmd):
        self.started.append(subprocess.Popen(["bash", "-c", cmd], env=self.env))
        return "cmd-1"

    def upload(self, local, path):
        Path(path.replace("$HOME", str(self.home))).write_bytes(Path(local).read_bytes())

    def download(self, path):
        p = Path(path.replace("$HOME", str(self.home)))
        return p.read_bytes() if p.exists() else None


class FakeDaytona:
    def __init__(self):
        self.deleted = []

    def delete(self, sandbox):
        self.deleted.append(sandbox.id)


class LauncherTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.old_cwd = os.getcwd()
        os.chdir(self.root)
        repo = self.root / "project"
        repo.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
        (repo / "app.py").write_text('print("broken")\n')
        git = ["git", "-c", "user.email=t@t", "-c", "user.name=t"]
        subprocess.run([*git, "add", "-A"], cwd=repo, check=True)
        subprocess.run([*git, "commit", "-qm", "init"], cwd=repo, check=True)
        self.state = {"id": "run-1", "repo": str(repo), "ref": None, "mode": "careful", "agents": "off", "sandbox": "sbx-1"}
        self.remote = FakeRemote(self.root, "run-1")

    def tearDown(self):
        for p in self.remote.started:
            p.wait(timeout=10)
        os.chdir(self.old_cwd)
        self.tmp.cleanup()

    def run_quietly(self, fn, *args, **kw):
        out = io.StringIO()
        with redirect_stdout(out), redirect_stderr(io.StringIO()):
            result = fn(*args, **kw)
        return result, out.getvalue()

    def test_a_run_is_tracked_exported_then_deleted(self):
        launcher.setup_and_start(self.remote, self.state, ["--goal", "fix app.py"])
        saved = json.loads(Path("cloud-runs/run-1/run.json").read_text())
        self.assertEqual(saved["status"], "running")
        self.assertEqual(saved["cmd_id"], "cmd-1")
        self.assertIn("--mode careful", saved["command"])
        code, out = self.run_quietly(launcher.follow, self.remote, self.state, poll=0.05)
        self.assertEqual(code, 0)
        self.assertIn("working on it", out)
        self.assertIn("done", out)
        d = FakeDaytona()
        code, _ = self.run_quietly(launcher.finish, d, self.remote, self.state, False)
        self.assertEqual(code, 0)
        dest = Path("cloud-runs/run-1")
        patch = (dest / "patch.diff").read_text()
        self.assertIn('+print("fixed")', patch)
        self.assertIn("new_module.py", patch)
        self.assertEqual((dest / "new-files.txt").read_text().split(), ["new_module.py"])
        with tarfile.open(dest / "new-files.tgz") as t:
            self.assertEqual(t.getnames(), ["new_module.py"])
        with tarfile.open(dest / "session.tgz") as t:
            names = t.getnames()
        self.assertIn(".config/rusty/sessions/last.json", names)
        self.assertFalse(any(n.endswith(".env") for n in names), "a key file left the sandbox")
        self.assertEqual(json.loads((dest / "trajectory.json").read_text()), {"agent": "rusty"})
        self.assertIn('"requests": 3', (dest / "stderr.log").read_text())
        manifest = json.loads((dest / "manifest.json").read_text())
        self.assertGreater(manifest["patch.diff"]["bytes"], 0)
        self.assertEqual(d.deleted, ["sbx-1"])
        self.assertEqual(json.loads((dest / "run.json").read_text())["status"], "deleted")
        # The patch applies cleanly to a fresh checkout.
        check = self.root / "check"
        subprocess.run(["git", "clone", "-q", self.state["repo"], str(check)], check=True)
        subprocess.run(["git", "apply", str(dest.resolve() / "patch.diff")], cwd=check, check=True)
        self.assertEqual((check / "new_module.py").read_text(), 'print("new")\n')

    def test_advisor_cloud_import_export_and_private_db_exclusion(self):
        incoming = self.root / "incoming.gz"
        incoming.write_bytes(b"scoped-memory")
        self.state.update(memory_mode="deep", memory_input=str(incoming), project_id="my-project")
        launcher.setup_and_start(self.remote, self.state, ["fix the file"])
        self.assertIn("--memory deep", self.state["command"])
        self.assertEqual((self.remote.home / "rusty-run/memory-input.json.gz").read_bytes(), b"scoped-memory")
        self.run_quietly(launcher.follow, self.remote, self.state, poll=0.05)
        private = self.remote.home / ".config/rusty/memory"
        private.mkdir()
        (private / "memory.sqlite-wal").write_bytes(b"do not archive raw live WAL")
        self.assertTrue(self.run_quietly(launcher.export, self.remote, self.state)[0])
        dest = Path("cloud-runs/run-1")
        self.assertEqual((dest / "memory.json.gz").read_bytes(), b"compressed-snapshot")
        self.assertEqual(dest.stat().st_mode & 0o777, 0o700)
        for item in dest.iterdir():
            self.assertEqual(item.stat().st_mode & 0o777, 0o600, str(item))
        with tarfile.open(dest / "session.tgz") as t:
            self.assertFalse(any("/memory/" in n for n in t.getnames()))

    def test_missing_trajectory_preserves_memory_sandbox(self):
        self.state["memory_mode"] = "on"
        launcher.setup_and_start(self.remote, self.state, ["x"])
        self.run_quietly(launcher.follow, self.remote, self.state, poll=.05)
        (self.remote.home / "rusty-run/trajectory.json").unlink()
        d = FakeDaytona()
        self.run_quietly(launcher.finish, d, self.remote, self.state, False)
        self.assertEqual(d.deleted, [])
        self.assertFalse(self.state["exported"])

    def test_unknown_exit_preserves_memory_sandbox(self):
        self.state["memory_mode"] = "on"
        self.remote.env["FAKE_RUSTY_SLEEP"] = "1"
        launcher.setup_and_start(self.remote, self.state, ["x"])
        d = FakeDaytona()
        self.run_quietly(launcher.finish, d, self.remote, self.state, False)
        self.assertEqual(d.deleted, [])
        self.assertEqual(self.state["status"], "running")

    def test_missing_memory_export_preserves_cloud_sandbox(self):
        self.state["memory_mode"] = "on"
        launcher.setup_and_start(self.remote, self.state, ["x"])
        self.run_quietly(launcher.follow, self.remote, self.state, poll=0.05)
        (self.root / "bin/rusty-memoryd").unlink()
        d = FakeDaytona()
        self.run_quietly(launcher.finish, d, self.remote, self.state, False)
        self.assertEqual(d.deleted, [])
        self.assertFalse(self.state["exported"])

    def test_ctrl_c_only_stops_following(self):
        self.remote.env["FAKE_RUSTY_SLEEP"] = "1"
        launcher.setup_and_start(self.remote, self.state, ["--goal", "fix app.py"])
        real_sleep = time.sleep
        calls = []

        def interrupt_once(s):
            if not calls:
                calls.append(s)
                raise KeyboardInterrupt
            real_sleep(s)

        launcher.time.sleep = interrupt_once
        try:
            code, first = self.run_quietly(launcher.follow, self.remote, self.state, poll=0.05)
            self.assertIsNone(code, "following stops, the run doesn't")
            self.assertEqual(json.loads(Path("cloud-runs/run-1/run.json").read_text())["status"], "running")
            code, second = self.run_quietly(launcher.follow, self.remote, self.state, poll=0.05)
        finally:
            launcher.time.sleep = real_sleep
        self.assertEqual(code, 0)
        both = first + second
        self.assertEqual(both.count("working on it"), 1, "output was repeated after reconnecting")
        self.assertIn("done", both)

    def test_a_failed_export_keeps_the_sandbox(self):
        launcher.setup_and_start(self.remote, self.state, ["--goal", "x"])
        self.run_quietly(launcher.follow, self.remote, self.state, poll=0.05)
        self.state["base_commit"] = "0" * 40  # unknown commit: the diff fails
        d = FakeDaytona()
        self.run_quietly(launcher.finish, d, self.remote, self.state, False)
        self.assertEqual(d.deleted, [])
        self.assertFalse(self.state["exported"] if "exported" in self.state else False)

    def test_only_this_runs_sandbox_is_ever_deleted(self):
        self.remote.sandbox.labels = {"app": "something-else"}
        d = FakeDaytona()
        self.run_quietly(launcher.delete_sandbox, d, self.remote, self.state)
        self.assertEqual(d.deleted, [])

    def test_snapshot_name_follows_the_binary(self):
        a, b = self.root / "a", self.root / "b"
        a.write_bytes(b"one")
        b.write_bytes(b"two")
        self.assertRegex(launcher.snapshot_name(a), launcher.SNAPSHOT_NAME)
        self.assertEqual(launcher.snapshot_name(a), launcher.snapshot_name(a))
        self.assertNotEqual(launcher.snapshot_name(a), launcher.snapshot_name(b))
        old = launcher.snapshot_name(a)
        (a.parent / "rusty-memoryd").write_bytes(b"advisor-v2")
        self.assertNotEqual(old, launcher.snapshot_name(a))
        self.assertIn("@sha256:", launcher.BASE_IMAGE, "the base image must stay pinned by digest")

class SafetyAndCompatibilityTest(unittest.TestCase):
    setUp = LauncherTest.setUp
    tearDown = LauncherTest.tearDown
    run_quietly = LauncherTest.run_quietly
    def test_direct_cli_does_not_shadow_the_sdk(self):
        sdk = self.root / 'sdk'
        sdk.mkdir()
        (sdk / 'daytona.py').write_text('class CreateSnapshotParams: pass\nclass Image: pass\nclass Resources: pass\nclass DaytonaNotFoundError(Exception): pass\n')
        script = Path(launcher.__file__).resolve()
        p = subprocess.run([sys.executable, str(script), 'prepare', '--binary', str(self.root/'missing')], env={**os.environ, 'PYTHONPATH': str(sdk)}, capture_output=True, text=True)
        self.assertEqual(p.returncode, 1)
        self.assertIn('not found. Build the static Linux binary', p.stderr)
        self.assertNotIn('ImportError', p.stderr)

    def test_snapshot_resources_change_identity(self):
        binary = self.root / 'rusty'
        binary.write_bytes(b'runtime')
        self.assertNotEqual(launcher.snapshot_name(binary, 2, 4), launcher.snapshot_name(binary, 4, 8))

    def test_missing_trajectory_preserves_sandbox(self):
        launcher.setup_and_start(self.remote, self.state, ['x'])
        self.run_quietly(launcher.follow, self.remote, self.state, poll=.05)
        (self.remote.home / 'rusty-run/trajectory.json').unlink()
        d = FakeDaytona()
        self.run_quietly(launcher.finish, d, self.remote, self.state, False)
        self.assertEqual(d.deleted, [])
        self.assertFalse(self.state['exported'])

    def test_local_exports_are_private(self):
        launcher.setup_and_start(self.remote, self.state, ['x'])
        self.run_quietly(launcher.follow, self.remote, self.state, poll=.05)
        self.run_quietly(launcher.export, self.remote, self.state)
        dest = Path('cloud-runs/run-1')
        self.assertEqual(dest.stat().st_mode & 0o777, 0o700)
        for p in dest.iterdir():
            self.assertEqual(p.stat().st_mode & 0o777, 0o600, str(p))

    def test_running_sandbox_is_not_deleted_without_exit(self):
        self.remote.env['FAKE_RUSTY_SLEEP'] = '1'
        launcher.setup_and_start(self.remote, self.state, ['x'])
        d = FakeDaytona()
        self.run_quietly(launcher.finish, d, self.remote, self.state, False)
        self.assertEqual(d.deleted, [])
        self.assertEqual(self.state['status'], 'running')


if __name__ == "__main__":
    unittest.main()
