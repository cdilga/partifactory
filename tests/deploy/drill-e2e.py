#!/usr/bin/env python3
"""Real restored-host checks, fresh OIDC/device consent and post-canary sink capture."""
from __future__ import annotations

import argparse
import http.client
import http.cookiejar
import http.server
from html.parser import HTMLParser
import json
import os
from pathlib import Path
import re
import socket
import ssl
import subprocess
import sys
import tarfile
import threading
import time
from urllib.error import HTTPError
from urllib.parse import urlencode, urljoin, urlsplit
from urllib.request import HTTPSHandler, HTTPCookieProcessor, ProxyHandler, Request, build_opener

ROOT = Path(__file__).resolve().parents[2]


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def private_json(path, data):
    path = Path(path)
    path.write_text(json.dumps(data) + "\n")
    path.chmod(0o600)


def command(argv, log, timeout=90):
    result = subprocess.run(argv, capture_output=True, timeout=timeout)
    Path(log).touch(mode=0o600)
    Path(log).chmod(0o600)
    with Path(log).open("ab") as output:
        output.write(result.stdout + result.stderr)
    require(result.returncode == 0, "recovery command failed; inspect private evidence")
    return result.stdout


def engine(host):
    return ["docker", "--host" if "://" in host else "--context", host]


def capture(config_path):
    config = json.loads(Path(config_path).read_text())
    target = engine(config["host"])
    out = Path(config["out"])
    compose = target + ["compose", "-f", config["compose"], "--profile", "proxy"]
    app = command(compose + ["ps", "-q", "partifactory"], out / "capture.log").decode().strip()
    require(app, "restored app missing")
    remote_archive = "/tmp/security-sinks-" + os.urandom(6).hex() + ".tar.zst"
    command(target + ["exec", app, "/partifactory", "export", "--include-insights", "--output", remote_archive], out / "capture.log")
    archive = out / "sinks.tar.zst"
    command(target + ["cp", app + ":" + remote_archive, str(archive)], out / "capture.log")
    archive.chmod(0o600)
    process = subprocess.Popen(["zstd", "--decompress", "--stdout", str(archive)], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        with tarfile.open(fileobj=process.stdout, mode="r|") as tar:
            for member in tar:
                if member.name in ("db.sqlite", "insights.sqlite"):
                    destination = out / member.name
                    with destination.open("wb") as output:
                        output.write(tar.extractfile(member).read())
                    destination.chmod(0o600)
        require(process.wait(timeout=30) == 0, "sink export decompression failed")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
    require((out / "db.sqlite").is_file() and (out / "insights.sqlite").is_file(), "consistent sink snapshots missing")
    logs = command(compose + ["logs", "--no-color", "--no-log-prefix"], out / "capture.log")
    (out / "server.jsonl").write_bytes(logs)
    (out / "server.jsonl").chmod(0o600)


class LoginForm(HTMLParser):
    def __init__(self):
        super().__init__()
        self.action, self.fields = None, {}

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "form":
            self.action = attrs.get("action", "")
        elif tag == "input" and attrs.get("name"):
            self.fields[attrs["name"]] = attrs.get("value", "")


class Browser:
    """TLS-verified real HTTP form/redirect client; no sessions are fabricated."""
    def __init__(self, base, ca, target, idp):
        self.base = base
        self.cookies = http.cookiejar.CookieJar()
        context = ssl.create_default_context(cafile=ca)
        workspace = urlsplit(base).hostname

        class RoutedConnection(http.client.HTTPSConnection):
            def connect(self):
                endpoint = target if self.host == workspace else idp
                require(self.host == workspace or self.host.startswith("dex."), "unexpected auth redirect host")
                address, port = endpoint.rsplit(":", 1)
                plain = socket.create_connection((address, int(port)), timeout=20)
                self.sock = context.wrap_socket(plain, server_hostname=self.host)

        class Handler(HTTPSHandler):
            def https_open(self, request):
                return self.do_open(RoutedConnection, request)

        self.opener = build_opener(ProxyHandler({}), Handler(context=context), HTTPCookieProcessor(self.cookies))

    def request(self, path, data=None, form=False, headers=None):
        headers = dict(headers or {})
        body = None
        if data is not None:
            body = urlencode(data).encode() if form else json.dumps(data).encode()
            headers["Content-Type"] = "application/x-www-form-urlencoded" if form else "application/json"
            if not form:
                headers["Origin"] = self.base
                headers["Sec-Fetch-Site"] = "same-origin"
        request = Request(urljoin(self.base, path), data=body, headers=headers)
        try:
            response = self.opener.open(request, timeout=30)
        except HTTPError as error:
            response = error
        with response:
            return response.status, response.geturl(), response.read()

    def signin(self, username):
        status, url, html = self.request("/signin?return=/")
        require(status == 200 and urlsplit(url).hostname.startswith("dex."), "actual OIDC login page unavailable")
        form = LoginForm()
        form.feed(html.decode())
        require("login" in form.fields and "password" in form.fields, "actual Dex login form missing")
        form.fields.update(login=username + "@example.com", password="password")
        action = urljoin(url, form.action or "")
        require(urlsplit(action).hostname == urlsplit(url).hostname, "unexpected IdP form destination")
        status, returned, _ = self.request(action, form.fields, form=True)
        require(status == 200 and urlsplit(returned).hostname == urlsplit(self.base).hostname, "real OIDC callback did not finish")
        status, _, body = self.request("/api/session")
        require(status == 200, "fresh OIDC session not authenticated")
        return json.loads(body)

    def state(self):
        return {"cookies": [{"name": cookie.name, "value": cookie.value, "domain": cookie.domain,
                              "secure": cookie.secure} for cookie in self.cookies]}


def observe(args):
    base, target = os.environ["PF_BASE_URL"], os.environ["PF_CONNECT_TO"]
    out = Path(os.environ["PF_DRILL_EVIDENCE"])
    config = json.loads(args.config.read_text())
    target_engine = engine(os.environ["PF_DRILL_HOST"])
    compose = target_engine + ["compose", "-f", os.environ["PF_DRILL_COMPOSE"], "--profile", "proxy"]
    private = out / "e2e-private.log"
    private.touch(mode=0o600)
    sys.path.insert(0, str(ROOT / "tests/security"))
    from run import Client
    client = Client(os.environ["PF_CA"], target)
    require(client.request(config["old_cap"])[0] == 404, "pre-import content cap survived boot epoch bump")
    link = config["public_link"].rstrip("/")
    parsed = urlsplit(link)
    origin = f"{parsed.scheme}://{parsed.netloc}"
    status, _, body = client.request(link + "/go", "POST", {"Origin": origin, "Content-Length": "0"})
    require(status == 200, "restored public link cannot mint genuine guest cap")
    guest = origin + "/c/" + json.loads(body)["cap"] + "/index.html"
    status, _, body = client.request(guest)
    require(status == 200 and config["marker"].encode() in body, "restored part bytes differ")

    admin = Browser(base, os.environ["PF_CA"], target, config["idp_connect_to"])
    admin.signin("admin")
    private_json(out / "admin-session.json", admin.state())
    member = Browser(base, os.environ["PF_CA"], target, config["idp_connect_to"])
    member.signin("member-a")
    scopes = ["parts:read", "parts:write", "data:read", "data:write"]
    status, _, body = member.request("/oauth/device_authorization", {"client_id": "part-cli", "agent_name": "Restored host device check", "scope": " ".join(scopes)}, form=True)
    require(status == 200, "fresh device authorization failed")
    device = json.loads(body)
    status, _, body = member.request("/api/agents/consent?" + urlencode({"code": device["user_code"]}))
    require(status == 200, "real member device consent unavailable")
    consent = json.loads(body)
    require(consent["spaces"], "fresh member has no authorized space")
    space = consent["spaces"][0]
    status, _, _ = member.request("/api/agents/consent", {"code": device["user_code"], "scopes": scopes, "spaces": [space["id"]], "lifetime_days": 30, "allow": True})
    require(status == 200, "real member device consent failed")
    status, _, body = member.request("/oauth/token", {"client_id": "part-cli", "grant_type": "urn:ietf:params:oauth:grant-type:device_code", "device_code": device["device_code"]}, form=True)
    require(status == 200, "fresh device token not issued")
    token = json.loads(body)["access_token"]
    token_path = out / "agent-token"
    token_path.write_text(token)
    token_path.chmod(0o600)
    # Consent selects space IDs; the part_create contract addresses spaces by slug.
    require(isinstance(space.get("slug"), str) and space["slug"], "authorized space has no create slug")
    status, _, body = client.request(base + "/api/v1/part_create", "POST", {"Authorization": "Bearer " + token, "Content-Type": "application/json"}, json.dumps({"name": "drill-member", "space": space["slug"], "title": "Recovered member device", "kind": "html", "files": {"index.html": "<h1>Fresh recovered device</h1>"}, "request_id": "drill-fresh-member"}).encode())
    if status != 200:
        private_json(out / "scoped-create-failure.json", {"status": status, "body": body.decode("utf-8", errors="replace")})
    require(status == 200, f"fresh scoped device create failed with HTTP {status}; inspect private scoped-create-failure.json")
    proxy = command(compose + ["ps", "-q", "proxy"], private).decode().strip()

    class Metrics(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path != "/metrics":
                self.send_error(404)
                return
            argv = target_engine + ["exec", proxy, "wget", "-S", "-qO-"]
            authorization = self.headers.get("Authorization")
            if authorization:
                argv += ["--header", "Authorization: " + authorization]
            result = subprocess.run(argv + ["http://partifactory:9090/metrics"], capture_output=True, timeout=20)
            statuses = re.findall(rb"HTTP/\S+ (\d{3})", result.stderr)
            status = int(statuses[-1]) if statuses else 502
            self.send_response(status)
            self.end_headers()
            if status == 200 and result.returncode == 0:
                self.wfile.write(result.stdout)

        def log_message(self, *_):
            pass

    metrics = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Metrics)
    thread = threading.Thread(target=metrics.serve_forever, daemon=True)
    thread.start()
    capture_config = out / "capture.json"
    private_json(capture_config, {"host": os.environ["PF_DRILL_HOST"], "compose": os.environ["PF_DRILL_COMPOSE"], "out": str(out)})
    try:
        capture(capture_config)
        argv = [sys.executable, str(ROOT / "tests/security/run.py"), "--base-url", base,
                "--ca-cert", os.environ["PF_CA"], "--connect-to", target,
                "--content-url", guest, "--part-url", origin + "/", "--private-asset-path", "data.json",
                "--sibling-origin", origin.replace("notes-part.", "pixel-part."),
                "--guest-content-url", guest, "--view-content-url", guest, "--public-link-url", link,
                "--session-file", str(out / "admin-session.json"), "--token-file", str(token_path),
                "--logs-file", str(out / "server.jsonl"), "--database-file", str(out / "db.sqlite"),
                "--insights-db", str(out / "insights.sqlite"), "--metrics-url", f"http://127.0.0.1:{metrics.server_port}/metrics",
                "--metrics-token-file", config["metrics_token_file"], "--evidence-dir", str(out / "security")]
        for secret in config["secret_files"]:
            argv += ["--secret-file", secret]
        argv += ["--capture-sinks-command", sys.executable, str(Path(__file__).resolve()), "capture", str(capture_config)]
        command(argv, private, timeout=180)
        report = json.loads((out / "security/report.json").read_text())
        require(all(row["outcome"] == "pass" for row in report["probe_results"].values()), "restored external security probe missing or failed")
    finally:
        metrics.shutdown()
        metrics.server_close()
        thread.join(timeout=5)
    status, _, html = admin.request("/admin/health")
    require(status == 200 and b'data-health-row="Last drill"' in html, "fresh admin cannot open restored Health drill row")
    public = {"event": "drill.e2e", "outcome": "pass", "old_cap_refused": True,
              "restored_content": True, "fresh_oidc": ["admin", "member"], "fresh_device_consent": True,
              "scoped_create": True, "external_probes": report["probe_results"], "health_drill_row": True,
              "scope": "restored-host external probes; full accumulated browser suite remains a separate gate"}
    (out / "e2e.json").write_text(json.dumps(public, indent=2) + "\n")
    print(json.dumps(public))


def source_report(args):
    config = json.loads(args.config.read_text())
    expected = args.expected
    token = Path(config["metrics_token_file"]).read_text().strip()
    connection = urlsplit(config["ops_url"])
    deadline = time.monotonic() + 75
    data = b""
    while time.monotonic() < deadline:
        conn = http.client.HTTPConnection(connection.hostname, connection.port, timeout=10)
        try:
            conn.request("GET", "/metrics", headers={"Authorization": "Bearer " + token})
            response = conn.getresponse()
            data = response.read()
            require(response.status == 200, "source authenticated metrics unavailable")
        finally:
            conn.close()
        if re.search(rb"^pf_last_drill_ok " + str(expected).encode() + rb"(?:\.0)?$", data, re.M):
            break
        time.sleep(1)
    else:
        raise AssertionError("source drill metric did not observe actual report")
    age = re.search(rb"^pf_last_drill_age_s ([0-9.]+)$", data, re.M)
    require(age and float(age.group(1)) < 900, "source drill age is not fresh")
    admin = Browser(config["base_url"], config["ca"], config["connect_to"], config["connect_to"])
    admin.signin("admin")
    status, _, html = admin.request("/admin/health")
    row = re.search(rb'<tr data-health-row="Last drill">.*?</tr>', html, re.S)
    require(status == 200 and row and (b"Passed" if expected else b"Failed") in row.group(), "source Health does not show actual drill outcome")
    report = {"event": "drill.source", "outcome": "pass", "pf_last_drill_ok": expected,
              "pf_last_drill_age_s": float(age.group(1)), "health_drill_row": "Passed" if expected else "Failed"}
    destination = Path(config["public_out"]) / ("source-drill-" + str(expected) + ".json")
    destination.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("observe", "capture", "source-report"))
    parser.add_argument("config", type=Path)
    parser.add_argument("--expected", type=int, choices=(0, 1))
    args = parser.parse_args()
    if args.mode == "capture":
        capture(args.config)
    elif args.mode == "observe":
        observe(args)
    else:
        require(args.expected is not None, "expected source report outcome required")
        source_report(args)


if __name__ == "__main__":
    main()
