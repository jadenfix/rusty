"""Real Rust CLI + Rust tool endpoint + SDK bridge, all on localhost.

No Daytona resource or real model is used. Different controller/sandbox
directories expose accidental local execution. Scripted swarms exercise real
threads, HTTP, permissions, editing, Bash and optional local memory hooks.
"""

import argparse
import http.server
import http.client
import importlib.util
import json
import os
import shlex
import subprocess
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request
from pathlib import Path
from unittest.mock import patch

from cloud.tools_bridge import bridge, remote_rpc

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target/debug/rusty"
spec = importlib.util.spec_from_file_location("hybrid_launcher", ROOT / "cloud/daytona.py")
launcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launcher)


class SDKProcess:
    """Same process.exec interface as the pinned Daytona SDK, real Rust tools."""
    def __init__(self, root):
        self.root = root
        self.calls = []
        self.active = 0
        self.peak = 0
        self.lock = threading.Lock()
        self.fail = False
        self.env = {"PATH": f"{BIN.parent}:/usr/bin:/bin", "HOME": str(root / "remote-home"), "RUSTY_NO_DOTENV": "1"}
        (root / "remote-home").mkdir()

    def exec(self, command, timeout):
        if self.fail:
            raise RuntimeError("SDK exception with credential-canary-never-log")
        inner = shlex.split(command)[2]
        parts = shlex.split(inner)
        request = json.loads(parts[parts.index("%s") + 1])
        with self.lock:
            self.calls.append(request)
        overlap = request.get("op") == "execute" and request.get("name") == "read_file"
        if overlap:
            with self.lock:
                self.active += 1
                self.peak = max(self.peak, self.active)
            time.sleep(.15)
        try:
            proc = subprocess.run(["bash", "-c", inner], env=self.env, capture_output=True, text=True, timeout=timeout)
            return argparse.Namespace(exit_code=proc.returncode, result=proc.stdout + proc.stderr)
        finally:
            if overlap:
                with self.lock:
                    self.active -= 1


class Model(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.server.requests.append(body)
        messages = body["messages"]
        prompt = next(m["content"] for m in reversed(messages) if m["role"] == "user")
        steps = sum(m["role"] == "tool" for m in messages)
        worker = "research worker" in messages[0]["content"]
        if worker:
            assert '"write_file"' not in json.dumps(body["tools"])
            if prompt.startswith("WORKER-"):
                if steps == 0:
                    with self.server.condition:
                        self.server.entered += 1
                        self.server.condition.notify_all()
                        assert self.server.condition.wait_for(lambda: self.server.entered == 3, timeout=5), "workers did not overlap"
                    action = ("write_file", {"path": "worker-forbidden.txt", "content": "forged"})
                elif steps == 1:
                    assert "denied" in messages[-1]["content"]
                    action = ("read_file", {"path": "source.py"})
                else:
                    assert "return a - b" in messages[-1]["content"]
                    action = None
            else:  # Careful's single checker reads the changed remote file.
                self.server.checker_requests += steps == 0
                if steps == 0:
                    action = ("read_file", {"path": "source.py"})
                else:
                    assert "return a + b" in messages[-1]["content"]
                    action = None
        else:
            assert "REMOTE-INSTRUCTIONS" in messages[0]["content"]
            assert "HOST-INSTRUCTIONS" not in messages[0]["content"]
            plan = [
                ("swarm", {"tasks": [{"description": f"inspect {i}", "prompt": f"WORKER-{i}"} for i in range(3)]}),
                ("read_file", {"path": "source.py"}),
                ("outline", {"path": "source.py"}),
                ("search", {"pattern": "return", "path": "source.py"}),
                ("glob", {"pattern": "*.py"}),
                ("list_files", {"path": "."}),
                ("edit_file", {"path": "source.py", "old_string": "return a - b", "new_string": "return a + b"}),
                ("write_file", {"path": "note.txt", "content": "done"}),
                ("bash", {"command": "test \"$(cat note.txt)\" = done && printf checked > result.txt"}),
            ]
            if any(m["role"] == "user" and m.get("content", "").startswith("INFRA") for m in messages):
                if steps in (1, 5) and messages[-1]["role"] == "tool":
                    assert "blocked by careful mode" in messages[-1]["content"], messages[-1]["content"]
                if steps == 3:
                    assert "[verified:" in messages[-1]["content"]
                    assert "pre-change snapshot" in messages[-1]["content"], messages[-1]["content"]
                plan = [
                    ("bash", {"command": "kubectl apply -f deployment.yaml"}),
                    ("bash", {"command": "kubectl diff -f deployment.yaml"}),
                    ("bash", {"command": "kubectl apply -f deployment.yaml"}),
                    ("edit_file", {"path": "deployment.yaml", "old_string": "replicas: 1", "new_string": "replicas: 2"}),
                    ("bash", {"command": "kubectl apply -f deployment.yaml"}),
                ]
            action = plan[steps] if steps < len(plan) else None
        if action:
            name, args = action
            delta = {"tool_calls": [{"index": 0, "id": f"call-{steps}", "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}]}
            finish = "tool_calls"
        else:
            delta, finish = {"content": "Inspected and verified the remote workspace."}, "stop"
        frames = [{"choices": [{"delta": delta, "finish_reason": None}]},
                  {"choices": [{"delta": {}, "finish_reason": finish}], "usage": {"prompt_tokens": 10, "completion_tokens": 5}}]
        encoded = ("".join("data: " + json.dumps(f) + "\n\n" for f in frames) + "data: [DONE]\n\n").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


class HybridTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="rusty-hybrid-")
        self.root = Path(self.tmp.name)
        self.host = self.root / "controller"
        self.remote = self.root / "sandbox"
        self.home = self.root / "home"
        for p in (self.host, self.remote, self.home):
            p.mkdir()
        (self.host / "AGENTS.md").write_text("HOST-INSTRUCTIONS")
        (self.remote / "AGENTS.md").write_text("REMOTE-INSTRUCTIONS")
        (self.host / "source.py").write_text("HOST-CANARY")
        (self.remote / "source.py").write_text("def add(a,b):\n    return a - b\n")
        self.process = SDKProcess(self.root)
        self.sandbox = argparse.Namespace(id="offline-sandbox", state="started", process=self.process)
        self.transport = launcher.Remote(self.sandbox)
        self.daemon = None

    def tearDown(self):
        if self.daemon:
            self.daemon.terminate()
            try:
                self.daemon.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.daemon.kill()
                self.daemon.wait(timeout=3)
        self.tmp.cleanup()

    def env(self):
        return {"PATH": os.environ["PATH"], "HOME": str(self.home), "RUSTY_HOME": str(self.home), "RUSTY_NO_DOTENV": "1", "NVIDIA_API_KEY": "offline-model-key", "NO_COLOR": "1"}

    def rpc(self, request):
        return remote_rpc(self.transport, str(self.remote), request)

    def test_rpc_needs_no_model_key_and_checks_remote_scripts_and_rechecks_approval(self):
        policy = {"mode": "auto", "allow": [], "deny": []}
        # Same script name, different bytes in the two workspaces.
        (self.host / "script.sh").write_text("echo harmless\n")
        (self.remote / "script.sh").write_text("git push origin main\n")
        req = {"op": "decide", "name": "bash", "args": {"command": "bash script.sh"}, "policy": policy}
        first = self.rpc(req)
        self.assertTrue(first["ok"])
        self.assertIn("Ask", first["data"])
        req.update(op="execute", approval="Allow")
        self.assertFalse(self.rpc(req)["ok"], "an old or forged approval bypassed remote inspection")
        req.update(name="write_file", args={"path": "forbidden.txt", "content": "no"}, policy={"mode": "read-only"})
        self.assertFalse(self.rpc(req)["ok"])
        self.assertFalse((self.remote / "forbidden.txt").exists())
        # Check the tool process, not just its parent, has no model credentials.
        req.update(name="bash", args={"command": "env"}, policy={"mode": "yolo"})
        env = self.rpc(req)["data"]
        self.assertNotIn("NVIDIA_API_KEY", env)
        self.assertNotIn("RUSTY_TOOL_BRIDGE_TOKEN", env)

    def test_bridge_rejects_missing_auth_and_oversized_requests(self):
        with bridge(self.transport, str(self.remote)) as env:
            request = urllib.request.Request(env["RUSTY_TOOL_BRIDGE_URL"] + "/rpc", data=b'{}')
            with self.assertRaises(urllib.error.HTTPError) as error:
                urllib.request.urlopen(request)
            self.assertEqual(error.exception.code, 403)
            conn = http.client.HTTPConnection(env["RUSTY_TOOL_BRIDGE_URL"].split("//", 1)[1])
            conn.request("POST", "/rpc", body=b'{}', headers={"Authorization": "Bearer " + env["RUSTY_TOOL_BRIDGE_TOKEN"], "Content-Length": str(1024 * 1024 + 1)})
            self.assertEqual(conn.getresponse().status, 413)
            conn.close()
        self.assertEqual(self.process.calls, [])

    def test_transport_failure_never_falls_back_or_logs_sdk_secrets(self):
        self.process.fail = True
        with bridge(self.transport, str(self.remote)) as env:
            out = subprocess.run([str(BIN), "--tools", "daytona", "--memory", "off", "probe"], cwd=self.host, env={**self.env(), **env}, capture_output=True, text=True, timeout=10)
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("no local fallback", out.stderr)
        self.assertNotIn("credential-canary", out.stderr + out.stdout)
        self.assertEqual((self.host / "source.py").read_text(), "HOST-CANARY")

    def test_missing_bridge_and_invalid_location_fail_before_model_calls(self):
        for args in (["--tools", "daytona"], ["--tools", "typo"]):
            out = subprocess.run([str(BIN), *args, "probe"], cwd=self.host, env=self.env(), capture_output=True, text=True, timeout=5)
            self.assertNotEqual(out.returncode, 0)
        for url in ("https://example.com", "http://127.0.0.1/path", "http://user:secret@127.0.0.1"):
            out = subprocess.run([str(BIN), "--tools", "daytona", "probe"], cwd=self.host, env={**self.env(), "RUSTY_TOOL_BRIDGE_URL": url}, capture_output=True, text=True, timeout=5)
            self.assertIn("localhost HTTP origin", out.stderr)
            self.assertNotIn("user:secret", out.stderr)

    def test_tool_endpoint_flag_after_double_dash_remains_a_user_prompt(self):
        env = self.env()
        env.pop("NVIDIA_API_KEY")
        out = subprocess.run([str(BIN), "--", "--tool-rpc"], input='{"op":"describe"}', cwd=self.host, env=env, capture_output=True, text=True, timeout=5)
        self.assertNotEqual(out.returncode, 0)
        self.assertNotIn('"protocol"', out.stdout)

    def test_saved_tool_default_only_changes_future_sessions(self):
        out = subprocess.run([str(BIN), "--tools", "local"], input="/tools daytona\n/settings\n/exit\n", cwd=self.host, env=self.env(), capture_output=True, text=True, timeout=5)
        self.assertEqual(out.returncode, 0)
        self.assertIn("takes effect next session", out.stdout)
        self.assertIn("current tools local", out.stdout)
        self.assertEqual(json.loads((self.home / "settings.json").read_text())["tools"], "daytona")
        out = subprocess.run([str(BIN)], input="/exit\n", cwd=self.host, env=self.env(), capture_output=True, text=True, timeout=5)
        self.assertNotEqual(out.returncode, 0)
        out = subprocess.run([str(BIN), "--tools", "local"], input="/exit\n", cwd=self.host, env=self.env(), capture_output=True, text=True, timeout=5)
        self.assertEqual(out.returncode, 0)

    def test_launcher_attaches_only_and_never_creates_wakes_or_deletes(self):
        args = argparse.Namespace(local_binary=str(BIN), sandbox="offline-sandbox", workspace=str(self.remote), mode="standard", agents="swarm", swarm_max=3, memory_mode="off", permissions="auto", model=None, stats=False, trajectory=None, goal=None, prompt=None)
        sdk = argparse.Namespace(get=lambda _: self.sandbox)
        with patch.object(launcher, "daytona_client", return_value=sdk), patch.object(launcher.subprocess, "call", return_value=0) as call:
            self.assertEqual(launcher.cmd_hybrid(args), 0)
            command = call.call_args.args[0]
            self.assertIn("--tools", command)
            self.assertIn("daytona", command)
            self.assertIn("--swarm-max", command)
            self.assertIn("RUSTY_TOOL_BRIDGE_TOKEN", call.call_args.kwargs["env"])
            self.sandbox.state = "stopped"
            with self.assertRaisesRegex(RuntimeError, "never creates or wakes"):
                launcher.cmd_hybrid(args)
            self.assertEqual(call.call_count, 1)
        self.assertEqual(self.process.calls, [])

    def test_full_cloud_env_never_copies_the_local_sdk_connection(self):
        with patch.dict(os.environ, {"NVIDIA_API_KEY": "model-canary", "RUSTY_TOOL_BRIDGE_URL": "http://127.0.0.1:1", "RUSTY_TOOL_BRIDGE_TOKEN": "bridge-canary", "RUSTY_TOOLS": "daytona", "RUSTY_HOME": "local-home"}, clear=True):
            self.assertEqual(launcher.model_env(), {"NVIDIA_API_KEY": "model-canary"})

    def test_infra_gate_snapshot_target_and_verification_use_remote_environment(self):
        bindir = self.root / "remote-bin"
        bindir.mkdir()
        kubectl = bindir / "kubectl"
        kubectl.write_text('''#!/bin/bash
echo "$*" >> "$HOME/kube-calls"
case "$1" in
  config) printf 'dev-cloud|test' ;;
  diff) exit 1 ;;
  apply) printf applied ;;
  get) case "$*" in
    *'-o yaml'*) printf 'kind: Deployment\\nmetadata:\\n  name: api\\n' ;;
    *) printf 'deployment.apps/api\\n' ;;
  esac ;;
  rollout) printf 'deployment api successfully rolled out' ;;
esac
''')
        kubectl.chmod(0o755)
        self.process.env["PATH"] = f"{bindir}:{self.process.env['PATH']}"
        (self.remote / "deployment.yaml").write_text("replicas: 1\n")
        (self.remote / "source.py").write_text("def add(a,b):\n    return a + b\n")
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model)
        server.requests, server.entered, server.checker_requests = [], 0, 0
        server.condition = threading.Condition()
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        trace_path = self.root / "infra.json"
        try:
            with bridge(self.transport, str(self.remote)) as bridge_env:
                out = subprocess.run([str(BIN), "--tools", "daytona", "--mode", "careful", "--memory", "off", "--permissions", "yolo", "--trajectory", str(trace_path), "INFRA verify the gate"], cwd=self.host, env={**self.env(), **bridge_env, "RUSTY_BASE_URL": f"http://127.0.0.1:{server.server_port}/v1"}, capture_output=True, text=True, timeout=20)
            self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)
        calls = (self.root / "remote-home/kube-calls").read_text().splitlines()
        self.assertEqual(calls.count("apply -f deployment.yaml"), 1)
        self.assertTrue(any(line.startswith("rollout status") for line in calls))
        self.assertIn("dev-cloud/test", out.stdout)
        self.assertIn("dry run is stale", out.stdout)
        snapshots = list((self.root / "remote-home/.config/rusty/snapshots").glob("*/rollback.yaml"))
        self.assertEqual(len(snapshots), 1)
        self.assertIn("kind: Deployment", snapshots[0].read_text())
        self.assertEqual(snapshots[0].stat().st_mode & 0o777, 0o600)
        self.assertFalse((self.remote / ".rusty/snapshots").exists())
        self.assertFalse((self.host / "deployment.yaml").exists())

    def test_three_modes_run_parallel_swarms_with_all_remote_tools_and_local_memory(self):
        receipts = []
        for mode, memory in (("standard", "off"), ("vibe", "off"), ("careful", "on")):
            with self.subTest(mode=mode):
                (self.remote / "source.py").write_text("def add(a,b):\n    return a - b\n")
                self.process.calls.clear()
                self.process.peak = 0
                server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model)
                server.requests, server.entered, server.checker_requests = [], 0, 0
                server.condition = threading.Condition()
                thread = threading.Thread(target=server.serve_forever, daemon=True)
                thread.start()
                trajectory = self.root / f"{mode}.json"
                if memory == "on":
                    self.daemon = subprocess.Popen([str(BIN.with_name("rusty-memoryd")), "--home", str(self.home)], env=self.env(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    deadline = time.monotonic() + 3
                    while not (self.home / "memory/advisor.sock").exists():
                        self.assertLess(time.monotonic(), deadline, "memory daemon did not start")
                        time.sleep(.01)
                started = time.monotonic()
                try:
                    with bridge(self.transport, str(self.remote)) as bridge_env:
                        env = {**self.env(), **bridge_env, "RUSTY_BASE_URL": f"http://127.0.0.1:{server.server_port}/v1"}
                        out = subprocess.run([str(BIN), "--tools", "daytona", "--mode", mode, "--agents", "swarm", "--swarm-max", "3", "--memory", memory, "--stats", "--trajectory", str(trajectory), "Inspect with three workers then fix the function and verify."], cwd=self.host, env=env, capture_output=True, text=True, timeout=25)
                    self.assertEqual(out.returncode, 0, out.stderr + out.stdout)
                finally:
                    server.shutdown()
                    server.server_close()
                    thread.join(timeout=2)
                trace = json.loads(trajectory.read_text())
                self.assertGreaterEqual(self.process.peak, 3)
                self.assertEqual((self.remote / "note.txt").read_text(), "done")
                self.assertEqual((self.remote / "result.txt").read_text(), "checked")
                self.assertIn("return a + b", (self.remote / "source.py").read_text())
                self.assertEqual((self.host / "source.py").read_text(), "HOST-CANARY")
                self.assertFalse((self.host / "note.txt").exists())
                self.assertFalse((self.remote / "worker-forbidden.txt").exists())
                tools = {r.get("name") for r in self.process.calls if r.get("op") == "execute"}
                self.assertTrue({"read_file", "outline", "search", "glob", "list_files", "edit_file", "write_file", "bash"} <= tools)
                self.assertNotIn("offline-model-key", json.dumps(self.process.calls))
                self.assertEqual(server.checker_requests, int(mode == "careful"))
                self.assertEqual(trace["memory_mode"], memory)
                if memory == "on":
                    self.assertGreater(trace["memory"]["hooks"], 0)
                    self.assertEqual(trace["memory"]["timeouts"], 0)
                self.assertFalse((self.root / "remote-home/.config/rusty/memory").exists())
                receipts.append({"mode": mode, "memory": memory, "passed": True, "provider": "scripted-local", "sandbox": "local-SDK-stand-in", "peak_parallel_remote_reads": self.process.peak, "model_requests": len(server.requests), "checker_attempts": server.checker_requests, "wall_seconds": round(time.monotonic() - started, 3), "remote_tool_calls": sum(r.get("op") == "execute" for r in self.process.calls), "memory_metrics": trace["memory"]})
        print("HYBRID_RECEIPTS " + json.dumps(receipts), flush=True)


if __name__ == "__main__":
    unittest.main()
