"""Small local task board. Three intentional bugs for the full-stack demo."""
import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).parent
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def send_json(self, value, code=200):
        data=json.dumps(value).encode()
        self.send_response(code);self.send_header('Content-Type','application/json')
        self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
    def do_GET(self):
        if self.path=='/api/tasks':
            self.send_json({'tasks':self.server.tasks})
            return
        files={'/':'web/index.html','/app.js':'web/app.js','/style.css':'web/style.css'}
        if self.path not in files: self.send_error(404);return
        path=ROOT/files[self.path];data=path.read_bytes()
        kind={'.html':'text/html','.js':'text/javascript','.css':'text/css'}[path.suffix]
        self.send_response(200);self.send_header('Content-Type',kind)
        self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
    def do_POST(self):
        if self.path!='/api/tasks': self.send_error(404);return
        try:
            n=int(self.headers.get('Content-Length','0'))
            if n>4096: self.send_json({'error':'too large'},413);return
            title=json.loads(self.rfile.read(n)).get('title','')
            if not isinstance(title,str): raise ValueError('invalid title')
        except (ValueError,TypeError): self.send_json({'error':'invalid request'},400);return
        title = title  # TODO: trim whitespace and reject an empty title
        task={'id':len(self.server.tasks)+1,'title':title}
        self.server.tasks.append(task)
        self.server.data.write_text(json.dumps(self.server.tasks))
        self.send_json(task,201)

def make_server(port=0, data=None):
    server=ThreadingHTTPServer(('127.0.0.1',port),Handler)
    server.data=Path(data or ROOT/'tasks.json')
    server.tasks=json.loads(server.data.read_text()) if server.data.exists() else []
    return server

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--port',type=int,default=8765);p.add_argument('--data')
    a=p.parse_args();server=make_server(a.port,a.data)
    print(f'Task board: http://127.0.0.1:{server.server_port}',flush=True)
    try: server.serve_forever()
    except KeyboardInterrupt: pass
    finally: server.server_close()
