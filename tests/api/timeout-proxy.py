#!/usr/bin/env python3
"""Withhold a committed response to observe a real CLI timeout and replay."""
import http.server
import json
import os
import ssl
import threading
import time
import urllib.request

upstream = os.environ['PARTIFACTORY_UPSTREAM']
context = ssl.create_default_context(cafile=os.environ['PARTIFACTORY_CA_CERT'])
lock = threading.Lock()
seen = False


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        global seen
        body = self.rfile.read(int(self.headers.get('Content-Length', '0')))
        headers = {k: v for k, v in self.headers.items()
                   if k.lower() in ('authorization', 'content-type', 'x-part-surface')}
        req = urllib.request.Request(upstream + self.path, data=body, headers=headers, method='POST')
        with urllib.request.urlopen(req, context=context) as response:
            result, status = response.read(), response.status
        with lock:
            drop = self.path == '/api/v1/part_edit' and not seen
            if drop:
                seen = True
        if drop:
            print('timeout-proxy: upstream committed; withholding first response', flush=True)
            time.sleep(28)
        elif self.path == '/api/v1/part_edit':
            assert json.loads(result).get('replayed') is True, 'retry must replay'
            print('timeout-proxy: retry returned replayed=true', flush=True)
        try:
            self.send_response(status)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(result)))
            self.end_headers()
            self.wfile.write(result)
        except (BrokenPipeError, ConnectionResetError):
            pass


server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
with open(os.environ['PF_TIMEOUT_PROXY_PORT'], 'w') as out:
    out.write(str(server.server_port))
server.serve_forever()
