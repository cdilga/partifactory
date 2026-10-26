#!/usr/bin/env python3
"""Exercise Caddy's real truncated-stream warning with synthetic credential canaries.

An unfiltered positive control must leak the path and custom-header canaries; the shipped config must
retain the warning while removing request URI and headers. Raw logs stay in memory.
Each owned container is removed, including on failure. No application harness needed.
"""
import http.server
import json
import pathlib
import re
import subprocess
import tempfile
import threading
import time
import uuid

REPO = pathlib.Path(__file__).resolve().parents[2]


def command(*args, **kwargs):
    return subprocess.run(args, check=True, capture_output=True, text=True, **kwargs).stdout


class BrokenStream(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Content-Length', '1048576')
        self.send_header('Connection', 'close')
        self.end_headers()
        self.wfile.write(b'data: truncated\n\n')
        self.wfile.flush()
        self.close_connection = True

    def log_message(self, *_):
        pass


def observe(source, directory, port, label):
    # A fresh file per lane: Docker Desktop can serve a stale, truncated view of a rewritten bind mount.
    caddyfile = directory / f'Caddyfile.{label}'
    caddyfile.write_text(source)
    name = 'pf-log-canary-' + uuid.uuid4().hex[:12]
    markers = [uuid.uuid4().hex for _ in range(4)]
    try:
        command('docker', 'run', '--detach', '--name', name, '--label', 'pf.harness=1',
                '--add-host', 'host.docker.internal:host-gateway',
                '--publish', '127.0.0.1::443',
                '--env', f'PF_UPSTREAM_WORKSPACE=host.docker.internal:{port}',
                '--mount', f'type=bind,src={caddyfile},dst=/etc/caddy/Caddyfile,readonly',
                '--mount', f'type=bind,src={directory}/certs,dst=/etc/caddy/certs,readonly',
                '--mount', f'type=bind,src={directory}/sites,dst=/etc/caddy/sites.d,readonly',
                '--mount', f'type=bind,src={REPO}/deploy/proxy,dst=/srv/pf-proxy,readonly',
                '--tmpfs', '/data', '--tmpfs', '/config', 'caddy:2.11.4-alpine')
        # Docker can report the container before its port mapping is published.
        for _ in range(40):
            ports = json.loads(command('docker', 'inspect', name))[0]['NetworkSettings']['Ports'] or {}
            if ports.get('443/tcp'):
                break
            time.sleep(.25)
        else:
            raise SystemExit(f'{name} never published 443: ' + subprocess.run(['docker', 'logs', name], capture_output=True, text=True).stderr[-2000:])
        published = ports['443/tcp'][0]['HostPort']
        for _ in range(40):
            result = subprocess.run(['curl', '--silent', '--show-error', '--max-time', '3',
                '--cacert', str(directory / 'certs/cert.pem'),
                '--resolve', f'parts.pf.test:{published}:127.0.0.1',
                '--header', 'Authorization: Bearer ' + markers[1],
                '--header', 'Cookie: pf_session=' + markers[2],
                '--header', 'X-Pf-Secret-Canary: ' + markers[3],
                f'https://parts.pf.test:{published}/_p/{markers[0]}/stream'], capture_output=True)
            if result.returncode not in (7, 35):
                break
            time.sleep(.25)
        time.sleep(.5)
        # Docker can put Caddy's JSON on stderr; combine without emitting it.
        captured = subprocess.run(['docker', 'logs', name], capture_output=True, text=True, check=True)
        logs = captured.stdout + captured.stderr
        rows = []
        for line in logs.splitlines():
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                pass
        warnings = [r for r in rows if r.get('level') in ('warn', 'error') and 'request' in r]
        assert warnings, f'{label}: truncated stream did not produce a request-bearing runtime warning'
        present = [m in logs for m in markers]
        if label == 'control':
            assert present[0] and present[3], 'positive control did not exercise URI and custom-header leakage'
        else:
            assert not any(present), 'shipped logger exposed a credential canary'
            assert all('uri' not in r['request'] and 'headers' not in r['request'] for r in warnings), 'request fields survived filtering'
        return {'lane': label, 'runtime_warnings': len(warnings), 'canaries_exposed': sum(present)}
    finally:
        subprocess.run(['docker', 'rm', '--force', name], capture_output=True)


def main():
    source = (REPO / 'deploy/proxy/Caddyfile').read_text()
    control, count = re.subn(r'\tlog \{\n\t\tformat filter \{.*?\n\t\}\n', '\tlog {\n\t\tformat json\n\t}\n', source, count=1, flags=re.S)
    assert count == 1, 'shipped runtime filter missing'
    upstream = http.server.ThreadingHTTPServer(('0.0.0.0', 0), BrokenStream)
    threading.Thread(target=upstream.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix='pf-log-canary-', dir=REPO / 'target') as tmp:
            directory = pathlib.Path(tmp)
            (directory / 'certs').mkdir()
            (directory / 'sites').mkdir()
            command('openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
                    '-subj', '/CN=parts.pf.test', '-addext', 'subjectAltName=DNS:parts.pf.test',
                    '-keyout', str(directory / 'certs/key.pem'), '-out', str(directory / 'certs/cert.pem'))
            observations = [observe(s, directory, upstream.server_port, label)
                            for label, s in [('control', control), ('shipped', source)]]
            print(json.dumps({'event': 'sec.check', 'id': 'pf-a72', 'outcome': 'pass', 'observations': observations}))
    finally:
        upstream.shutdown()
        upstream.server_close()


if __name__ == '__main__':
    main()
