#!/usr/bin/env python3
"""External ingress security probes. TLS verification and hostname/SNI are never bypassed."""
from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import secrets
import socket
import sqlite3
import ssl
import subprocess
import sys
import time
from urllib.parse import quote, urlencode, urljoin, urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parent))
from coverage import CHECKLIST, check_mapping, observations  # noqa: E402


class Unobserved(Exception):
    """A prerequisite is unavailable; this is not passing evidence."""


def need(value, label):
    if not value:
        raise Unobserved(label)
    return value


def require(condition, message):
    if not condition:
        raise AssertionError(message)


class Client:
    def __init__(self, ca_cert=None, connect_to=None):
        self.context = ssl.create_default_context(cafile=ca_cert)
        self.connect_to = connect_to

    def request(self, url, method="GET", headers=None, body=b""):
        parsed = urlsplit(url)
        require(parsed.scheme == "https" and parsed.hostname and not parsed.username,
                "probe targets must be HTTPS URLs without userinfo")
        require(not parsed.fragment, "fragments must not be sent to the server")
        destination = (parsed.hostname, parsed.port or 443)
        if self.connect_to:
            host, port = self.connect_to.rsplit(":", 1)
            destination = (host.strip("[]"), int(port))
        path = parsed.path or "/"
        if parsed.query:
            path += "?" + parsed.query
        outgoing = {"Host": parsed.netloc, "Connection": "close", "Accept-Encoding": "identity"}
        outgoing.update(headers or {})
        if body:
            outgoing["Content-Length"] = str(len(body))
        # Build the wire request explicitly, so redirects cannot silently forward credentials.
        wire = f"{method} {path} HTTP/1.1\r\n"
        for key, value in outgoing.items():
            require(not any(c in key + value for c in "\r\n"), "invalid request header")
            wire += f"{key}: {value}\r\n"
        with socket.create_connection(destination, timeout=15) as plain:
            with self.context.wrap_socket(plain, server_hostname=parsed.hostname) as stream:
                stream.sendall(wire.encode("ascii") + b"\r\n" + body)
                response = http.client.HTTPResponse(stream)
                response.begin()
                data = response.read(8 * 1024 * 1024 + 1)
                require(len(data) <= 8 * 1024 * 1024, "response exceeded probe limit")
                pairs = response.getheaders()
                # Preserve duplicates for the no-Set-Cookie assertion.
                return response.status, {k.lower(): v for k, v in pairs}, data


class Suite:
    def __init__(self, args):
        self.args = args
        self.client = Client(args.ca_cert, args.connect_to)
        self.base = args.base_url.rstrip("/")
        p = urlsplit(self.base)
        require(p.scheme == "https" and p.hostname and p.path in ("", "/") and
                not p.query and not p.fragment and not p.username, "--base-url must be an HTTPS origin")
        self.content = args.content_url
        self.part = args.part_url
        self.expired = None
        self.private_asset_path = args.private_asset_path
        if args.fixture:
            fixture = json.loads(Path(args.fixture).read_text())
            self.content = self.content or fixture["notes"]["url"]
            self.expired = fixture.get("expired", {}).get("url")
            self.private_asset_path = self.private_asset_path or "data.json"
        if self.content and not self.part:
            parsed = urlsplit(self.content)
            self.part = f"{parsed.scheme}://{parsed.netloc}/"
        self.part = self.part.rstrip("/") + "/" if self.part else None
        self.part_name = args.part_name
        if not self.part_name and self.part:
            self.part_name = urlsplit(self.part).hostname.split(".")[0].removesuffix("-part")
        self.blob_origin = args.blob_origin
        if not self.blob_origin and self.part:
            host = urlsplit(self.part).hostname
            self.blob_origin = "https://blobs-part." + host.split(".", 1)[1]
        self.canaries = ["pf_pat_" + secrets.token_hex(16),
                         "pf_at_" + secrets.token_hex(16),
                         "pf_rt_" + secrets.token_hex(16)]
        self.cookie_canary = "SEC_COOKIE_" + secrets.token_hex(16)
        self.content_canary = "SEC_CONTENT_" + secrets.token_hex(16)
        self.secret_canaries = [Path(p).read_bytes().strip() for p in args.secret_file]
        self.cookie = ""
        if args.session_file:
            state = json.loads(Path(args.session_file).read_text())
            self.cookie = "; ".join(f"{c['name']}={c['value']}" for c in state.get("cookies", [])
                                    if c["domain"].lstrip(".") == p.hostname and c.get("secure"))

    def get(self, path, **kwargs):
        return self.client.request(urljoin(self.base + "/", path), **kwargs)

    def content_headers(self):
        url = need(self.content, "--content-url or --fixture required")
        status, headers, body = self.client.request(url, headers={"Origin": "null"})
        require(status == 200, "authorized content must return 200")
        return headers, body

    def canonical_csp(self):
        headers, _ = self.content_headers()
        if self.args.expected_csp_file:
            expected = Path(self.args.expected_csp_file).read_text().strip()
        else:
            own = self.part.rstrip("/")
            blob = self.blob_origin
            ws = own.replace("https://", "wss://", 1)
            expected = (
                "sandbox allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-modals allow-downloads allow-top-navigation-by-user-activation; "
                f"default-src 'none'; script-src 'self' {own} 'unsafe-inline' 'unsafe-eval' blob: {blob}; "
                f"style-src 'self' {own} 'unsafe-inline' {blob}; img-src 'self' {own} data: blob: {blob}; "
                f"media-src 'self' {own} data: blob: {blob}; font-src 'self' {own} data: {blob}; "
                f"connect-src 'self' {own} {ws} {blob}; worker-src 'self' {own} blob:; "
                f"frame-src 'self' {own}; form-action 'self' {own}; base-uri 'none'; object-src 'none'; "
                f"frame-ancestors {own} {self.base} {' '.join(self.args.embedder)}").strip()
        require(headers.get("content-security-policy") == expected, "canonical CSP differs byte-for-byte")

    def no_part_cookies(self):
        url = need(self.content, "--content-url or --fixture required")
        a = self.client.request(url)
        b = self.client.request(url, headers={"Cookie": "__Host-pf_cap=SEC_OBSOLETE_COOKIE"})
        require(a[0] == b[0] == 200 and a[2] == b[2], "old cap cookie changed content")
        for response in (a, b):
            require("set-cookie" not in response[1], "part emitted Set-Cookie")

    def null_cors(self):
        headers, _ = self.content_headers()
        require(headers.get("access-control-allow-origin") == "null", "content null CORS missing")
        require("access-control-allow-credentials" not in headers, "credentialed content CORS")
        require("cross-origin-resource-policy" not in headers, "content emitted CORP")
        for path in ("/", "/api/v1/part_get", "/mcp"):
            _, h, _ = self.get(path, headers={"Origin": "null"})
            require(h.get("access-control-allow-origin") != "null", "workspace allowed null CORS")

    def blob_policies(self):
        headers, body = self.content_headers()
        require("private" in [value.strip() for value in headers.get("cache-control", "").split(",")], "private content cache policy missing")
        require(headers.get("cf-cache-status", "").upper() != "HIT", "private content was served from edge cache")
        match = re.search(rb'<script\s+src=["\'](https://[^"\']+/b/[a-f0-9]{64})["\']', body)
        require(match, "shared prelude URL missing from content")
        status, shared_headers, _ = self.client.request(match[1].decode())
        require(status == 200, "shared prelude unavailable")
        require(shared_headers.get("cache-control") == "public, max-age=31536000, immutable" and
                "set-cookie" not in shared_headers, "shared blob caching/cookie policy differs")
        asset = need(self.private_asset_path, "--private-asset-path or --fixture required for known-private hash test")
        status, private_headers, private_bytes = self.client.request(urljoin(self.content, asset))
        require(status == 200 and "private" in [value.strip() for value in private_headers.get("cache-control", "").split(",")],
                "known private asset unavailable or publicly cacheable")
        require(private_headers.get("cf-cache-status", "").upper() != "HIT", "private asset was served from edge cache")
        digest = hashlib.sha256(private_bytes).hexdigest()
        status, _, _ = self.client.request(self.blob_origin + "/b/" + digest)
        require(status == 404, "known private blob hash was served by shared host")

    def cap_rejection(self):
        url = need(self.content, "--content-url or --fixture required")
        parsed = urlsplit(url)
        match = re.search(r"/c/([^/]+)/", parsed.path)
        require(match, "content URL needs /c/<cap>/")
        cap = match[1]
        tampered = cap[:-1] + ("A" if cap[-1] != "A" else "B")
        invalid = url.replace("/c/" + cap + "/", "/c/" + tampered + "/", 1)
        require(self.client.request(invalid)[0] == 404, "bad MAC did not return 404")
        if self.args.sibling_origin:
            foreign = self.args.sibling_origin.rstrip("/") + parsed.path
            require(self.client.request(foreign)[0] == 404, "foreign-host cap accepted")
        else:
            raise Unobserved("--sibling-origin required to observe host binding")

    def runtime_origin(self):
        url = need(self.content, "--content-url or --fixture required")
        cap = re.search(r"/c/([^/]+)/", urlsplit(url).path)[1]
        rt = urljoin(self.part, "c/" + cap + "/_rt/v1")
        # Valid read-only operation, so rejection cannot be an unknown-operation error.
        for origin in (self.base, "https://foreign.example", self.part.rstrip("/")):
            body = json.dumps({"op": "storage.list", "args": {}}).encode()
            status, _, _ = self.client.request(rt, "POST", {
                "Origin": origin, "Sec-Fetch-Mode": "cors", "Content-Type": "text/plain"}, body)
            require(status == 403, "runtime accepted a non-null origin")

    def grant_redirects(self):
        name = need(self.part_name, "--part-name or --part-url required")
        evil = ["//evil.example", "\\\\evil.example", "%2f%2fevil.example", "@evil.example", "\r\nLocation: https://evil.example"]
        for value in evil:
            for key in ("part", "path", "return_to", "redirect_uri"):
                query = {"part": name, "mode": "wrap", key: value}
                headers = {"Cookie": self.cookie} if self.cookie else {}
                status, h, _ = self.get("/grant?" + urlencode(query), headers=headers)
                location = h.get("location", "")
                target = urlsplit(urljoin(self.base + "/", location))
                allowed = {urlsplit(self.base).netloc, urlsplit(self.part).netloc} if self.part else {urlsplit(self.base).netloc}
                require(target.netloc in allowed and "\r" not in location and "\n" not in location,
                        "grant redirected outside workspace/own part")
                require(status in (302, 303, 400, 403, 404), "unexpected grant response")
                require("/c/" not in location, "wrap grant exposed cap in path")
                if not self.cookie:
                    require(target.netloc == urlsplit(self.base).netloc, "signed-out grant returned part content")
        if self.cookie:
            status, h, _ = self.get("/grant?" + urlencode({"part": name, "mode": "wrap"}), headers={"Cookie": self.cookie})
            require(status in (302, 303), "authorized grant did not redirect")
            dest = urlsplit(h.get("location", ""))
            require(dest.netloc == urlsplit(self.part).netloc and dest.fragment.startswith("pf=") and
                    "/c/" not in dest.path, "wrap grant cap is not fragment-only")

    def wrapper_headers(self):
        part = need(self.part, "--part-url required")
        status, h, body = self.client.request(part)
        require(status == 200, "wrapper missing")
        csp = h.get("content-security-policy", "")
        directives = {p.strip().split()[0]: p.strip().split()[1:] for p in csp.split(";") if p.strip()}
        require("sandbox" not in directives and "'unsafe-inline'" not in csp and "'unsafe-eval'" not in csp,
                "wrapper CSP permits sandbox or inline/eval scripts")
        require(directives.get("frame-src") == ["'self'"], "wrapper frame-src differs")
        require(directives.get("connect-src") == [self.base], "wrapper connect-src differs")
        require(directives.get("script-src") == [self.blob_origin], "wrapper script-src differs")
        require("set-cookie" not in h, "wrapper emitted Set-Cookie")
        require(b"/c/" not in body and not re.search(rb"v1\.[0-9]+\.", body), "wrapper leaked a cap")

    def chrome_cors(self):
        name = need(self.part_name, "--part-name required")
        own = need(self.part, "--part-url required").rstrip("/")
        for route in ("summary", "links", "grant", "stepup"):
            path = f"/api/chrome/v1/{quote(name)}/{route}"
            for origin in ("null", "https://evil.example", self.blob_origin, own + ".evil.example", own.replace("https:", "http:")):
                _, h, _ = self.get(path, method="OPTIONS", headers={"Origin": origin,
                    "Access-Control-Request-Method": "POST", "Access-Control-Request-Headers": "content-type"})
                require("access-control-allow-origin" not in h and "access-control-allow-credentials" not in h,
                        "chrome CORS accepted foreign origin")
            status, h, _ = self.get(path, method="OPTIONS", headers={"Origin": own,
                "Access-Control-Request-Method": "POST", "Access-Control-Request-Headers": "content-type"})
            require(status == 204 and h.get("access-control-allow-origin") == own and
                    h.get("access-control-allow-credentials") == "true", "active part CORS missing")
            if route != "summary":
                for site in (None, "cross-site"):
                    headers = {"Origin": own, "Content-Type": "application/json"}
                    if site:
                        headers["Sec-Fetch-Site"] = site
                    require(self.get(path, method="POST", headers=headers, body=b"{}")[0] == 403,
                            "chrome mutation accepted missing/foreign Fetch Metadata")

    def no_referrer(self):
        urls = [need(self.part, "--part-url required"), need(self.content, "--content-url required")]
        for url in urls:
            require(self.client.request(url)[1].get("referrer-policy") == "no-referrer", "part referrer policy differs")

    def signed_out_cards(self):
        part = need(self.part, "--part-url required")
        a = self.client.request(part, headers={"User-Agent": "Mozilla/5.0"})
        b = self.client.request(part, headers={"User-Agent": "Slackbot"})
        require(a[0] == b[0] == 200 and a[2] == b[2], "root bot/browser card differs")
        require(not re.search(rb"v1\.[0-9]+\.|/c/|/go(?:[\"'/?])", a[2]), "root card leaked cap/hop")
        require(b"og:video" not in a[2], "private card exposes video")
        if self.args.private_marker:
            require(self.args.private_marker.encode() not in a[2], "private part bytes in root card")
        if self.args.public_link_url:
            a = self.client.request(self.args.public_link_url, headers={"User-Agent": "Mozilla/5.0"})
            b = self.client.request(self.args.public_link_url, headers={"User-Agent": "Slackbot"})
            require(a[0] == b[0] == 200 and a[2] == b[2], "public link bot/browser card differs")
            require(not re.search(rb"v1\.[0-9]+\.|/c/|/go(?:[\"'/?])", a[2]), "public card leaked cap/hop")

    def workspace_redirect(self):
        name = need(self.part_name, "--part-name required")
        status, h, body = self.get("/p/" + quote(name))
        require(status in (301, 302, 303, 307, 308), "workspace /p is not redirect-only")
        require(urlsplit(h.get("location", "")).netloc == urlsplit(self.part).netloc,
                "workspace /p redirected to wrong host")
        require(not re.search(rb"v1\.[0-9]+\.|/c/", body), "workspace /p exposed cap")

    def schema_boundaries(self):
        path = need(self.args.database_file, "--database-file needed for schema observation")
        db = sqlite3.connect(Path(path).resolve().as_uri() + "?mode=ro", uri=True)
        try:
            tables = {r[0] for r in db.execute("SELECT name FROM sqlite_schema WHERE type='table'")}
            require("parts" in tables, "not a partifactory database")
            require(not tables.intersection({"requests", "agent_requests", "share_requests"}), "agent request table exists")
        finally:
            db.close()

    def guest_boundaries(self):
        guest = need(self.args.guest_content_url, "--guest-content-url needed for non-data guest wall")
        view = need(self.args.view_content_url, "--view-content-url needed for view-cap write refusal")
        def runtime_url(url):
            parsed = urlsplit(url)
            match = re.search(r"/c/([^/]+)/", parsed.path)
            require(match, "guest content URL needs /c/<cap>/")
            return f"https://{parsed.netloc}/c/{match[1]}/_rt/v1"
        headers = {"Origin": "null", "Sec-Fetch-Mode": "cors", "Content-Type": "text/plain"}
        for op in ("comments.list", "comments.create", "artifact.publish", "assets.put", "user", "errors.report", "refresh", "history", "source", "share"):
            body = json.dumps({"op": op, "args": {}}).encode()
            require(self.client.request(runtime_url(guest), "POST", headers, body)[0] == 403,
                    "guest non-data operation was not403")
        # This should never mutate because a view cap carries no data-write authority.
        body = json.dumps({"op": "storage.set", "args": {"key": "SEC_VIEW_MUST_NOT_WRITE", "value": "denied", "shared": True}}).encode()
        require(self.client.request(runtime_url(view), "POST", headers, body)[0] == 403, "view cap wrote shared data")
        parsed = urlsplit(runtime_url(guest))
        cap = parsed.path.split('/')[2]
        # Change an authenticated claim byte; retaining the signature cannot authenticate it.
        parts = cap.split('.')
        require(len(parts) >= 3, "unexpected cap format")
        payload_index = len(parts) - 2
        payload = parts[payload_index]
        parts[payload_index] = ("A" if payload[0] != "A" else "B") + payload[1:]
        forged = runtime_url(guest).replace('/c/' + cap + '/', '/c/' + '.'.join(parts) + '/')
        require(self.client.request(forged, "POST", headers, b'{"op":"storage.list","args":{"shared":true}}')[0] == 404,
                "forged guest claim accepted")
        self.authentication_boundary()

    def prepare_device(self, destination):
        body = urlencode({"client_id": "part-cli", "agent_name": "Security network warning gate", "scope": "parts:read", "resource": self.base + "/"}).encode()
        status, _, response = self.get("/oauth/device_authorization", method="POST", headers={"Content-Type": "application/x-www-form-urlencoded"}, body=body)
        require(status == 200, "cannot prepare live device code")
        result = json.loads(response)
        result["issued_at_ms"] = int(time.time() * 1000)
        path = Path(destination)
        path.parent.mkdir(parents=True, exist_ok=True)
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as stream:
            json.dump(result, stream)
        os.chmod(path, 0o600)

    def authentication_boundary(self):
        for token in (None, self.canaries[0]):
            headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
            if token:
                headers["Authorization"] = "Bearer " + token
            for path in ("/api/v1/part_get", "/mcp"):
                body = b'{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' if path == "/mcp" else b'{"name":"sec-missing"}'
                status, h, _ = self.get(path, method="POST", headers=headers, body=body)
                require(status == 401, "auth failure was not HTTP 401")
                require("resource_metadata=" in h.get("www-authenticate", ""), "auth challenge missing resource metadata")

    def discovery(self):
        private = [b"pf_pat_", b"pf_at_", b"pf_rt_", b"__Host-pf_session=", self.content_canary.encode()]
        for path in ("/llms.txt", "/llms-full.txt", "/skill.md", "/openapi.json",
                     "/.well-known/mcp/server.json", "/.well-known/oauth-authorization-server",
                     "/.well-known/oauth-protected-resource"):
            status, _, body = self.get(path)
            require(status == 200 and body, "public discovery not available: " + path)
            # The guide documents token prefixes, so inspect concrete token-shaped values only.
            require(not re.search(rb"pf_(?:pat|at|rt)_[A-Z2-7]{32}", body), "token in discovery")
            for marker in private[3:]:
                require(marker not in body, "private marker in discovery")
            for secret in self.secret_canaries:
                require(not secret or secret not in body, "configured secret in discovery")
            if path.endswith(".json") or "well-known/oauth" in path:
                json.loads(body)

    def ingress_metrics(self):
        for path in ("/metrics", "/readyz", "/healthz"):
            status, _, _ = self.get(path)
            if path == "/metrics":
                require(status in (401, 403, 404), "public ingress exposed metrics")
        for address in self.args.forbidden_listener:
            host, port = address.rsplit(":", 1)
            try:
                with socket.create_connection((host.strip("[]"), int(port)), timeout=2):
                    raise AssertionError("direct listener reachable from external probe")
            except (ConnectionRefusedError, TimeoutError, OSError):
                pass

    def redaction_canaries(self):
        # Place each canary only in a sensitive input, never in a path/query/loggable field.
        request_ids = []
        for token in self.canaries:
            response = self.get("/api/v1/part_get", method="POST", headers={"Authorization": "Bearer " + token,
                "Cookie": "__Host-pf_session=" + self.cookie_canary, "Content-Type": "application/json"},
                body=json.dumps({"name": self.content_canary}).encode())
            if response[1].get("x-request-id"):
                request_ids.append(response[1]["x-request-id"])
            response = self.get("/mcp", method="POST", headers={"Authorization": "Bearer " + token,
                "Cookie": "__Host-pf_session=" + self.cookie_canary, "Content-Type": "application/json",
                "Accept": "application/json, text/event-stream"},
                body=json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "part_get", "arguments": {"name": self.content_canary}}}).encode())
            if response[1].get("x-request-id"):
                request_ids.append(response[1]["x-request-id"])
        live_token = Path(self.args.token_file).read_text().strip() if self.args.token_file else None
        if live_token:
            response = self.get("/api/v1/part_list", method="POST", headers={"Authorization": "Bearer " + live_token,
                "Content-Type": "application/json"}, body=b"{}")
            require(response[0] != 401, "supplied live token is not authenticated")
            if response[1].get("x-request-id"):
                request_ids.append(response[1]["x-request-id"])
        if self.content:
            response = self.client.request(self.content, headers={"Cookie": "__Host-pf_cap=" + self.cookie_canary})
            if response[1].get("x-request-id"):
                request_ids.append(response[1]["x-request-id"])
        if not (self.args.logs_file or self.args.logs_url):
            raise Unobserved("--logs-file or --logs-url needed to observe redaction")
        # Allow the asynchronous access line to land; this is bounded and intentionally short.
        time.sleep(0.3)
        if self.args.capture_sinks_command:
            captured = subprocess.run(self.args.capture_sinks_command, capture_output=True, timeout=90)
            require(captured.returncode == 0, "explicit sink capture command failed")
        sources = []
        for path in self.args.logs_file:
            data = Path(path).read_bytes()
            require(data, "log evidence is empty")
            sources.append(data)
        if self.args.logs_url:
            status, _, data = self.client.request(self.args.logs_url)
            require(status == 200 and data not in (b"", b"[]"), "log evidence unavailable")
            sources.append(data)
        log_bytes = b"\n".join(sources)
        require(request_ids and all(identifier.encode() in log_bytes for identifier in request_ids),
                "log evidence does not contain the actual canary request IDs")
        missing_sinks = []
        if self.args.metrics_url:
            token = Path(need(self.args.metrics_token_file, "metrics token file required")).read_text().strip()
            parsed = urlsplit(self.args.metrics_url)
            if parsed.scheme == "http":
                require(parsed.hostname in ("127.0.0.1", "::1", "localhost") and not parsed.username,
                        "plaintext metrics input must be an explicit loopback ops listener")
                connection = http.client.HTTPConnection(parsed.hostname, parsed.port or 80, timeout=10)
                try:
                    connection.request("GET", parsed.path or "/", headers={"Authorization": "Bearer " + token})
                    response = connection.getresponse()
                    status, data = response.status, response.read(8 * 1024 * 1024)
                finally:
                    connection.close()
            else:
                status, _, data = self.client.request(self.args.metrics_url, headers={"Authorization": "Bearer " + token})
            require(status == 200, "authenticated metrics unavailable")
            sources.append(data)
        else:
            missing_sinks.append("--metrics-url and --metrics-token-file")
        if self.args.insights_db:
            db = sqlite3.connect(Path(self.args.insights_db).resolve().as_uri() + "?mode=ro", uri=True)
            try:
                sources.append("\n".join(db.iterdump()).encode())
            finally:
                db.close()
        else:
            missing_sinks.append("--insights-db")
        needles = [s.encode() for s in self.canaries + [self.cookie_canary, self.content_canary]] + self.secret_canaries
        if live_token:
            needles.append(live_token.encode())
        def strings(value):
            if isinstance(value, str):
                return [value]
            if isinstance(value, list):
                return [s for item in value for s in strings(item)]
            if isinstance(value, dict):
                return [s for item in value.values() for s in strings(item)]
            return []
        for path in self.args.canary_values_file:
            needles.extend(value.encode() for value in strings(json.loads(Path(path).read_text())) if value)
        if self.content:
            needles.append(re.search(r"/c/([^/]+)/", self.content)[1].encode())
        variants = []
        for marker in needles:
            if not marker:
                continue
            variants.append(marker)
            # JSON logs escape newlines/quotes; SQLite dumps encode BLOBs as hex literals.
            variants.append(json.dumps(marker.decode("utf-8", errors="replace"), ensure_ascii=False)[1:-1].encode())
            variants.append(marker.hex().upper().encode())
            variants.append(marker.replace(b"'", b"''"))
        for data in sources:
            require(all(marker not in data for marker in variants), "sensitive canary survived redaction")
        if missing_sinks:
            raise Unobserved("missing live redaction sinks: " + ", ".join(missing_sinks))


PROBES = {name: getattr(Suite, name) for name in (
    "canonical_csp", "no_part_cookies", "null_cors", "blob_policies", "cap_rejection", "runtime_origin",
    "grant_redirects", "wrapper_headers", "chrome_cors", "no_referrer", "signed_out_cards",
    "workspace_redirect", "schema_boundaries", "guest_boundaries", "authentication_boundary", "discovery", "ingress_metrics", "redaction_canaries")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    for name in ("ca-cert", "connect-to", "fixture", "part-url", "content-url", "part-name", "blob-origin",
                 "sibling-origin", "session-file", "token-file", "expected-csp-file", "private-marker",
                 "public-link-url", "logs-url", "metrics-url", "metrics-token-file", "insights-db"):
        parser.add_argument("--" + name)
    for name in ("embedder", "logs-file", "secret-file", "forbidden-listener", "native-junit", "playwright-report", "script-evidence"):
        parser.add_argument("--" + name, action="append", default=[])
    parser.add_argument("--evidence-dir", default="target/e2e-logs/security")
    parser.add_argument("--require-complete", action="store_true", help="fail on unobserved existing checks; skipped tests never pass")
    parser.add_argument("--database-file")
    parser.add_argument("--private-asset-path")
    parser.add_argument("--canary-values-file", action="append", default=[], help="private JSON string values from executed agent-surface scenarios to scan across sinks")
    parser.add_argument("--guest-content-url")
    parser.add_argument("--view-content-url")
    parser.add_argument("--capture-sinks-command", nargs=argparse.REMAINDER, default=[],
                        help="Explicit argv to refresh protected logs/DB snapshots after actual canaries; no shell is used")
    parser.add_argument("--prepare-device-file", help="only create a live code from this client's network; private browser input, not an artifact")
    args = parser.parse_args()
    suite = Suite(args)
    if args.prepare_device_file:
        suite.prepare_device(args.prepare_device_file)
        print("Prepared live device code in private file; no code/token printed.")
        return 0
    errors, checklist = check_mapping()
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    evidence = observations(args.native_junit, args.playwright_report, args.script_evidence)
    out = Path(args.evidence_dir)
    out.mkdir(parents=True, exist_ok=True)
    probe_results = {}
    for name, probe in PROBES.items():
        try:
            probe(suite)
            probe_results[name] = {"outcome": "pass"}
        except Unobserved as error:
            probe_results[name] = {"outcome": "unobserved", "reason": str(error)}
        except Exception as error:
            # Exception messages can contain request targets/caps. Keep those out of artifacts.
            safe_reason = str(error) if isinstance(error, AssertionError) else type(error).__name__
            probe_results[name] = {"outcome": "fail", "reason": safe_reason}
    results = []
    for item in checklist["checks"]:
        components = []
        for component in item["components"]:
            if component["kind"] in ("pending", "manual"):
                result = {"outcome": component["kind"], "owner": component["owner"]}
            elif component["kind"] == "external":
                result = probe_results[component["probe"]].copy()
            else:
                key = (component["kind"], component["file"], component["test"])
                required = component.get("projects", [])
                seen = evidence.get(key, set())
                passed = all(p in seen for p in required) if required else bool(seen)
                result = {"outcome": "pass" if passed else "unobserved"}
                if not passed:
                    result["reason"] = "passing execution evidence required (source references are not execution)"
            components.append({"proves": component["proves"], **result})
        outcomes = [c["outcome"] for c in components]
        outcome = "fail" if "fail" in outcomes else "unobserved" if "unobserved" in outcomes else "pass"
        if all(o in ("pending", "manual") for o in outcomes):
            outcome = "pending" if "pending" in outcomes else "manual"
        record = {"event": "sec.check", "id": item["id"], "outcome": outcome, "components": components}
        folder = out / item["id"]
        folder.mkdir(exist_ok=True)
        (folder / "result.json").write_text(json.dumps(record, indent=2) + "\n")
        print(json.dumps({k: record[k] for k in ("event", "id", "outcome")}))
        results.append(record)
    report = {"schema": 1, "checklist_hash": hashlib.sha256(CHECKLIST.read_bytes()).hexdigest(),
              "base_origin": suite.base, "probe_results": probe_results, "checks": results,
              "complete": not any(r["outcome"] in ("fail", "unobserved") for r in results)}
    (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return int(any(r["outcome"] == "fail" or (args.require_complete and r["outcome"] == "unobserved") for r in results))


if __name__ == "__main__":
    raise SystemExit(main())
