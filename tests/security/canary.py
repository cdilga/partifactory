#!/usr/bin/env python3
"""Harness-only sibling endpoint. Report cookie presence without storing cookie values."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
import threading
from urllib.parse import parse_qs, urlsplit

observations = {}
lock = threading.Lock()


class Canary(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def check(self):
        parsed = urlsplit(self.path)
        if parsed.path not in ("/__sec_canary", "/__sec_canary/observations"):
            self.send_error(404)
            return
        size = int(self.headers.get("Content-Length", "0"))
        if size > 4096:
            self.send_error(413)
            return
        self.rfile.read(size)
        nonce = parse_qs(parsed.query).get("nonce", [""])[0]
        key = (self.headers.get("Host", ""), nonce)
        with lock:
            if parsed.path == "/__sec_canary/observations":
                methods = list(observations.get(key, []))
                value = {"count": len(methods), "methods": methods}
            else:
                if nonce:
                    observations.setdefault(key, []).append(self.command)
                value = {"cookie_present": bool(self.headers.get("Cookie")),
                         "method": self.command, "origin": self.headers.get("Origin")}
        data = json.dumps(value).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Access-Control-Allow-Origin", "null")
        self.send_header("Access-Control-Allow-Credentials", "true")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = check
    do_POST = check


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", int(os.environ.get("PORT", "8080"))), Canary).serve_forever()
