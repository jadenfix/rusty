import json
from pathlib import Path
import tempfile
import threading
import unittest
from urllib.request import Request, urlopen
from urllib.error import HTTPError
from app import make_server

class API(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.server=make_server(data=Path(self.tmp.name)/'tasks.json')
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.url=f'http://127.0.0.1:{self.server.server_port}'
    def tearDown(self):
        self.server.shutdown();self.server.server_close();self.thread.join();self.tmp.cleanup()
    def post(self,title):
        with urlopen(Request(self.url+'/api/tasks',json.dumps({'title':title}).encode(),{'Content-Type':'application/json'})) as r:
            return json.load(r)
    def test_roundtrip(self):
        self.assertEqual(self.post('  ship it  ')['title'],'ship it')
        with urlopen(self.url+'/api/tasks') as r: self.assertEqual(json.load(r)['items'][0]['title'],'ship it')
    def test_blank_rejected(self):
        with self.assertRaises(HTTPError) as r: self.post('   ')
        self.assertEqual(r.exception.code,400)
    def test_page_and_assets(self):
        for route in ['/','/app.js','/style.css']:
            with urlopen(self.url+route) as r: self.assertEqual(r.status,200)
