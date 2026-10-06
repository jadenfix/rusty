"""A session-scoped, authenticated localhost bridge to the Daytona SDK.

The SDK owns remote transport; Rust owns tools, permissions and the infra
harness. No model keys, dotenv files or host environment are sent remotely.
"""

import hmac
import http.server
import json
import secrets
import shlex
import threading
from contextlib import contextmanager

MAX_RPC = 1024 * 1024


def remote_rpc(remote, workspace, request):
    encoded = json.dumps(request, separators=(",", ":"))
    if len(encoded.encode()) > MAX_RPC:
        raise ValueError("tool request exceeds 1 MiB")
    args = request.get("args") or {}
    timeout = (args.get("timeout_secs", 120) if request.get("op") == "execute"
               else request.get("timeout", 120) if request.get("op") == "shell" else 20)
    timeout = max(1, min(int(timeout), 600)) + 10
    # stdin carries JSON. It is quoted as data, never interpreted as shell code.
    command = (f"cd -- {shlex.quote(workspace)} && printf '%s' {shlex.quote(encoded)} | "
               "env RUSTY_NO_DOTENV=1 rusty --tool-rpc")
    code, out = remote.sh(command, timeout=timeout)
    if code != 0:
        raise RuntimeError("sandbox tool endpoint failed; rebuild the snapshot with tool protocol v1")
    if len(out.encode()) > MAX_RPC:
        raise ValueError("tool response exceeds 1 MiB")
    try:
        response = json.loads(out)
    except ValueError:
        raise RuntimeError("sandbox did not return tool protocol JSON; rebuild its Rusty snapshot") from None
    if request.get("op") == "describe" and response.get("ok"):
        response["data"]["sandbox"] = remote.id
    return response


@contextmanager
def bridge(remote, workspace):
    if not workspace.startswith("/") or "\x00" in workspace:
        raise ValueError("--workspace must be an absolute sandbox path")
    token = secrets.token_hex(32)
    slots = threading.BoundedSemaphore(9)  # lead plus eight workers

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        def log_message(self, *_):
            pass  # No commands, tokens or tool results in HTTP logs.

        def do_POST(self):
            self.connection.settimeout(5)
            if self.path != "/rpc" or not hmac.compare_digest(self.headers.get("Authorization", ""), f"Bearer {token}"):
                self.send_error(403)
                return
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 < length <= MAX_RPC:
                    self.send_error(413)
                    return
                if not slots.acquire(blocking=False):
                    self.send_error(429)
                    return
                try:
                    body = self.rfile.read(length)
                    if len(body) != length:
                        raise ValueError("incomplete request")
                    request = json.loads(body)
                    if not isinstance(request, dict):
                        raise ValueError("request must be an object")
                    response = remote_rpc(remote, workspace, request)
                finally:
                    slots.release()
            except Exception:
                # SDK exception strings may contain credentials or URLs; keep
                # transport errors generic and never retry an uncertain write.
                response = {"ok": False, "error": "Daytona tool transport failed; remote outcome may be unknown. Inspect the sandbox before retrying; no local fallback."}
            encoded = json.dumps(response).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            try:
                self.wfile.write(encoded)
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield {"RUSTY_TOOL_BRIDGE_URL": f"http://127.0.0.1:{server.server_port}", "RUSTY_TOOL_BRIDGE_TOKEN": token}
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)
