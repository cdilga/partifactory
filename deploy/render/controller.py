#!/usr/bin/env python3
"""Internal-only Chrome lifecycle controller. Health checks never keep Chrome awake."""
import glob
import http.server
import os
from pathlib import Path
import socket
import socketserver
import select
import subprocess
import threading
import time

guard = threading.Lock()
browser = None
last_use = 0.0
pin_used = None


def stop():
    global browser
    if browser is not None and browser.poll() is None:
        browser.terminate()
        try:
            browser.wait(timeout=3)
        except subprocess.TimeoutExpired:
            browser.kill()
            browser.wait()
    browser = None


def start():
    global browser, last_use, pin_used
    pin = Path(os.environ.get("PF_RENDER_SPKI_FILE", "/run/pf-render/spki")).read_text().strip()
    if not pin or len(pin) > 128:
        raise RuntimeError("missing certificate pin")
    if pin != pin_used:
        stop()
    if browser is None or browser.poll() is not None:
        executables = (glob.glob("/ms-playwright/chromium_headless_shell-*/chrome-headless-shell-linux*/chrome-headless-shell")
                       + glob.glob("/ms-playwright/chromium_headless_shell-*/chrome-linux*/headless_shell"))
        if not executables:
            raise RuntimeError("pinned headless shell not installed")
        target = os.environ.get("PF_RENDER_GATEWAY", "partifactory")
        host, _, port = target.partition(":")
        address = socket.gethostbyname(host)
        target = address + (":" + port if port else "")
        apex = os.environ.get("PF_RENDER_APEX", "pf.test")
        args = [executables[0], "--remote-debugging-address=127.0.0.1", "--remote-debugging-port=9224",
                "--user-data-dir=/tmp/pf-chrome", "--disable-gpu", "--no-first-run",
                "--disable-extensions", "--disable-background-networking", "--disable-sync",
                "--mute-audio", "--deny-permission-prompts", "--no-sandbox",
                "--ignore-certificate-errors-spki-list=" + pin,
                "--host-resolver-rules=MAP *-part." + apex + " " + target + ", EXCLUDE localhost"]
        browser = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL)
        pin_used = pin
    last_use = time.monotonic()
    for _ in range(100):
        if browser.poll() is not None:
            raise RuntimeError("headless shell exited")
        try:
            with socket.create_connection(("127.0.0.1", 9224), timeout=0.1):
                return
        except OSError:
            time.sleep(0.05)
    raise RuntimeError("headless shell did not become ready")


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, body):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        with guard:
            running = browser is not None and browser.poll() is None
        self.reply(200 if self.path == "/health" and running else 503, b'{"running":' + (b'true}' if running else b'false}'))

    def do_POST(self):
        if self.path != "/start":
            self.reply(404, b'{}')
            return
        try:
            with guard:
                start()
            self.reply(200, b'{"running":true}')
        except (OSError, RuntimeError):
            self.reply(503, b'{"error":"browser unavailable"}')


def reap():
    while True:
        time.sleep(1)
        with guard:
            if browser is not None and time.monotonic() - last_use >= 300:
                stop()


threading.Thread(target=reap, daemon=True).start()

# Recent headless-shell versions bind CDP to loopback regardless of its address flag.
# Expose only that fixed local endpoint on the internal render network.
class Cdp(socketserver.BaseRequestHandler):
    def handle(self):
        try:
            with socket.create_connection(("127.0.0.1", 9224), timeout=2) as remote:
                remote.settimeout(None)
                peers = {self.request: remote, remote: self.request}
                while True:
                    ready, _, _ = select.select(list(peers), [], [], 30)
                    if not ready:
                        return
                    for source in ready:
                        data = source.recv(65536)
                        if not data:
                            return
                        peers[source].sendall(data)
        except OSError:
            pass


class CdpServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


cdp = CdpServer(("0.0.0.0", 9222), Cdp)
threading.Thread(target=cdp.serve_forever, daemon=True).start()
try:
    http.server.ThreadingHTTPServer(("0.0.0.0", 9223), Handler).serve_forever()
finally:
    with guard:
        stop()
