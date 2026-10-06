# /// script
# requires-python = ">=3.10"
# dependencies = ["daytona==0.220.0", "python-dotenv"]
# ///
"""Use the pinned real SDK against a local Toolbox API, without cloud resources."""
import sys
from pathlib import Path
HERE = Path(__file__).resolve().parent
# Avoid the neighboring daytona.py shadowing the installed SDK.
sys.path = [p for p in sys.path if Path(p or '.').resolve() != HERE]
sys.path.insert(0, str(HERE.parent))
import http.server, json, threading, unittest
import importlib.metadata
import httpx
from daytona._sync.process import Process
from daytona_toolbox_api_client import ApiClient, Configuration, ProcessApi
from cloud import test_hybrid as journey

assert importlib.metadata.version('daytona') == '0.220.0', 'run with uv run cloud/sdk_smoke.py'
assert importlib.metadata.version('daytona-toolbox-api-client') == '0.220.0'

original_init = journey.SDKProcess.__init__
original_exec = journey.SDKProcess.exec
original_teardown = journey.HybridTest.tearDown

def sdk_init(self, root):
    original_init(self, root)
    process = self
    class Toolbox(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_): pass
        def do_POST(self):
            assert self.path == '/process/execute'
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            assert body.get('envs') is None, 'host environment was exported'
            try:
                result = original_exec(process, body['command'], body['timeout'])
                data, status = {'exitCode':result.exit_code, 'result':result.result}, 200
            except Exception:
                data, status = {'message':'toolbox fixture failed'}, 500
            encoded = json.dumps(data).encode()
            self.send_response(status)
            self.send_header('Content-Type','application/json')
            self.send_header('Content-Length',str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)
    self.server = http.server.ThreadingHTTPServer(('127.0.0.1',0),Toolbox)
    self.thread = threading.Thread(target=self.server.serve_forever,daemon=True)
    self.thread.start()
    self.http = httpx.Client(trust_env=False)
    self.api = ApiClient(Configuration(host=f'http://127.0.0.1:{self.server.server_port}'))
    self.actual = Process('python', ProcessApi(self.api), self.http)

def sdk_exec(self, command, timeout):
    return self.actual.exec(command, timeout=timeout)

def teardown(self):
    try:
        self.process.server.shutdown()
        self.process.server.server_close()
        self.process.thread.join(timeout=2)
        self.process.api.rest_client.pool_manager.clear()
        self.process.http.close()
    finally:
        original_teardown(self)

journey.SDKProcess.__init__ = sdk_init
journey.SDKProcess.exec = sdk_exec
journey.HybridTest.tearDown = teardown
suite = unittest.defaultTestLoader.loadTestsFromTestCase(journey.HybridTest)
result = unittest.TextTestRunner(verbosity=1).run(suite)
raise SystemExit(not result.wasSuccessful())
