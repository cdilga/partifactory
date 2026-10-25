#!/usr/bin/env python3
"""Observed deploy gates against a caller-owned disposable compose stack.

No mocked engine or readiness response is accepted. The caller tears down its stack.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time
from urllib.parse import urlsplit, urlunsplit

REPO = Path(__file__).resolve().parents[2]


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def pinned(value):
    return bool(re.fullmatch(r"[^\s]+@sha256:[a-f0-9]{64}", value))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["smoke", "logs", "rollback"])
    parser.add_argument("--compose", type=Path, default=REPO / "deploy/compose.yaml")
    parser.add_argument("--env-file", type=Path)
    parser.add_argument("--host", help="Docker endpoint or context name")
    parser.add_argument("--evidence-dir", type=Path, required=True)
    parser.add_argument("--base-url")
    parser.add_argument("--part-url", help="Stable public restored-part URL, followed through redirects")
    parser.add_argument("--part-marker", help="Expected response body text from the public part")
    parser.add_argument("--part-entry", default="index.html", help="Content entry fetched after a stable public link's real go flow")
    parser.add_argument("--ca-cert", type=Path)
    parser.add_argument("--connect-to", help="Actual Caddy address:port; preserves hostname/SNI")
    parser.add_argument("--binary", type=Path, default=REPO / "target/debug/partifactory")
    parser.add_argument("--good-image")
    parser.add_argument("--bad-image")
    parser.add_argument("--delayed-image")
    parser.add_argument("--deploy-image", help="Smoke also performs and observes a successful good-image deployment")
    args = parser.parse_args()
    args.evidence_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    private = args.evidence_dir / "private.log"
    private.touch(mode=0o600)
    engine = ["docker"]
    if args.host:
        engine += ["--host" if "://" in args.host else "--context", args.host]
    compose = engine + ["compose", "-f", str(args.compose.resolve())]
    if args.env_file:
        compose += ["--env-file", str(args.env_file.resolve())]

    def command(argv, timeout=60, check=True):
        result = subprocess.run(argv, capture_output=True, timeout=timeout, cwd=REPO)
        with private.open("ab") as output:
            output.write(result.stdout + result.stderr)
        if check:
            require(result.returncode == 0, "command failed; inspect private evidence log")
        return result

    def service_id():
        value = command(compose + ["ps", "-q", "partifactory"]).stdout.decode().strip()
        require(value and len(value.splitlines()) == 1, "one running application is required")
        return value

    def inspect(container):
        return json.loads(command(engine + ["inspect", container]).stdout)[0]

    def request(url, follow=False, method="GET", headers=None):
        parsed = urlsplit(url)
        require(parsed.scheme == "https" and parsed.hostname and not parsed.username, "TLS URL required")
        argv = ["curl", "--silent", "--show-error", "--connect-timeout", "1", "--max-time", "3",
                "--proto", "=https", "--proto-redir", "=https", "--write-out", "\n%{http_code}", "--noproxy", "*"]
        if follow:
            argv += ["--location", "--max-redirs", "5"]
        if method != "GET":
            argv += ["--request", method, "--data", ""]
        for name, value in (headers or {}).items():
            require(not any(c in name + value for c in "\r\n"), "invalid request header")
            argv += ["--header", name + ": " + value]
        if args.ca_cert:
            argv += ["--cacert", str(args.ca_cert.resolve())]
        if args.connect_to:
            address, port = args.connect_to.rsplit(":", 1)
            argv += ["--connect-to", f"::{address}:{port}"]
        result = command(argv + [url], timeout=5, check=False)
        body, _, status = result.stdout.rpartition(b"\n")
        return result.returncode, int(status or b"0"), body

    def observed_part():
        url = args.part_url
        parsed = urlsplit(url)
        if re.fullmatch(r"/l/[^/]+/?", parsed.path):
            # Deploy/import bump the boot epoch. Mint through the real public
            # link each time, rather than testing wrapper text or a stale cap.
            origin = urlunsplit((parsed.scheme, parsed.netloc, "", "", ""))
            go = origin + parsed.path.rstrip("/") + "/go"
            code, status, body = request(go, method="POST", headers={"Origin": origin})
            if code or status != 200:
                return False
            try:
                cap = json.loads(body)["cap"]
                require(re.fullmatch(r"[A-Za-z0-9_.-]+", cap), "unexpected guest capability encoding")
            except (ValueError, KeyError, TypeError):
                return False
            require(args.part_entry and not args.part_entry.startswith("/") and
                    ".." not in args.part_entry.split("/"), "relative content entry required")
            url = origin + "/c/" + cap + "/" + args.part_entry
        code, status, body = request(url, follow=True)
        return code == 0 and status == 200 and args.part_marker.encode() in body

    def log_limits():
        # Validate inactive profiles too, then inspect every actually running service.
        config = json.loads(command(compose + ["--profile", "proxy", "--profile", "render", "config", "--format", "json"]).stdout)
        for name, service in config["services"].items():
            logging = service.get("logging", {})
            require(logging.get("driver") == "json-file", f"{name}: wrong logging driver")
            require(logging.get("options", {}).get("max-size") == "10m", f"{name}: wrong max-size")
            require(str(logging.get("options", {}).get("max-file")) == "3", f"{name}: wrong max-file")
        proxy = config["services"].get("proxy", {})
        # The pinned Caddy binary carries cap_net_bind_service=ep. With ALL dropped
        # and no capability restored, Linux rejects exec before Caddy can even log.
        require(set(proxy.get("cap_drop", [])) == {"ALL"}
                and {value.removeprefix("CAP_") for value in proxy.get("cap_add", [])} == {"NET_BIND_SERVICE"},
                "proxy must restore only the capability required by the actual Caddy binary")
        require(proxy.get("read_only") is True and
                any(value.startswith("no-new-privileges") for value in proxy.get("security_opt", [])),
                "proxy hardening differs from the shipped contract")
        ids = command(compose + ["--profile", "proxy", "--profile", "render", "ps", "-q"]).stdout.decode().split()
        require(ids, "no running compose services")
        observed = []
        running_services = set()
        for container in ids:
            doc = inspect(container)
            host = doc["HostConfig"]
            logging = host["LogConfig"]
            require(logging["Type"] == "json-file" and logging["Config"].get("max-size") == "10m"
                    and logging["Config"].get("max-file") == "3", "running container log limits differ from compose")
            service = doc["Config"]["Labels"].get("com.docker.compose.service")
            if doc["State"]["Running"]:
                running_services.add(service)
            row = {"id": doc["Id"], "service": service, "logging": logging}
            if service == "proxy":
                require(set(host.get("CapDrop") or []) == {"ALL"}
                        and {value.removeprefix("CAP_") for value in host.get("CapAdd") or []} == {"NET_BIND_SERVICE"},
                        "running proxy capability set differs from the shipped contract")
                require(host["ReadonlyRootfs"] and any(value.startswith("no-new-privileges") for value in host.get("SecurityOpt") or []),
                        "running proxy lost read-only or no-new-privileges protection")
                row["security"] = {"cap_drop": host["CapDrop"], "cap_add": host["CapAdd"],
                                   "read_only": host["ReadonlyRootfs"], "security_opt": host["SecurityOpt"]}
            observed.append(row)
        require({"partifactory", "proxy"} <= running_services, "application and actual Caddy proxy must both be running")
        (args.evidence_dir / "log-limits.json").write_text(json.dumps(observed, indent=2) + "\n")

    if args.mode in ("smoke", "rollback"):
        require(args.base_url and args.part_url and args.part_marker, "base URL, public part URL and marker are required")
        require(observed_part(), "public part does not serve expected restored bytes through Caddy")
    if args.mode == "logs":
        log_limits()
    elif args.mode == "smoke":
        successful = None
        if args.deploy_image:
            require(pinned(args.deploy_image), "good deployment image must be pinned by digest")
            before_id = service_id()
            previous = inspect(before_id)["Config"]["Image"]
            journal = args.evidence_dir / "good-deploy-journal.json"
            argv = [str(args.binary.resolve()), "deploy", "--image", args.deploy_image,
                    "--compose", str(args.compose.resolve()), "--journal", str(journal.resolve())]
            if args.env_file:
                argv += ["--env-file", str(args.env_file.resolve())]
            if args.host:
                endpoint = args.host
                if "://" not in endpoint:
                    endpoint = json.loads(command(["docker", "context", "inspect", endpoint]).stdout)[0]["Endpoints"]["docker"]["Host"]
                argv += ["--host", endpoint]
            began = time.monotonic()
            output = command(argv, timeout=240)
            (args.evidence_dir / "good-deploy.jsonl").write_bytes(output.stdout + output.stderr)
            successful = json.loads(journal.read_text())
            require(successful["ok"] and successful["phase"] == "complete" and successful["previous"] == previous,
                    "good deployment did not complete its journal")
            require(time.monotonic() - began >= 120, "good deployment skipped its two-minute readiness watch")
            after_id = service_id()
            require(after_id != before_id and inspect(after_id)["Config"]["Image"] == args.deploy_image,
                    "good deployment did not switch the service")
            require(command(engine + ["inspect", before_id], check=False).returncode != 0, "old container was not removed")
            require(observed_part(), "part stopped serving after good deployment")
        current = inspect(service_id())
        require(pinned(current["Config"]["Image"]), "running release image must be digest-pinned")
        require(current["Config"]["User"] == "65532:65532", "application must be non-root")
        require(current["HostConfig"]["ReadonlyRootfs"], "application rootfs must be read-only")
        for port, bindings in current["NetworkSettings"]["Ports"].items():
            if port in ("8080/tcp", "8081/tcp", "8443/tcp"):
                require(not bindings, "application listener was published")
            if port == "9090/tcp":
                require(all(b["HostIp"] in ("127.0.0.1", "::1") for b in bindings or []), "ops was exposed publicly")
        command(engine + ["exec", current["Id"], "/partifactory", "healthcheck"])
        for suffix in ("/readyz", "/livez", "/metrics"):
            _, status, _ = request(args.base_url.rstrip("/") + suffix)
            require(status in (401, 403, 404), "proxy exposed an ops route")
        log_limits()
        (args.evidence_dir / "smoke.json").write_text(json.dumps({"ok": True, "image": current["Config"]["Image"],
                                                                 "part_served": True, "successful_deploy": successful}) + "\n")
    else:
        require(all(pinned(value or "") for value in (args.good_image, args.bad_image, args.delayed_image)), "all three test images must be pinned by digest")
        require(inspect(service_id())["Config"]["Image"] == args.good_image, "start with the specified good image")
        reports = []
        for name, image, phase in (("immediate", args.bad_image, "candidate_failed"), ("delayed", args.delayed_image, "rolled_back")):
            journal = args.evidence_dir / f"{name}-journal.json"
            log_path = args.evidence_dir / f"{name}-deploy.jsonl"
            argv = [str(args.binary.resolve()), "deploy", "--image", image, "--compose", str(args.compose.resolve()),
                    "--journal", str(journal.resolve()), "--ready-timeout-seconds", "20"]
            if args.env_file:
                argv += ["--env-file", str(args.env_file.resolve())]
            # Deploy accepts endpoints; context callers use their context's resolved endpoint.
            if args.host:
                host = args.host
                if "://" not in host:
                    host = json.loads(command(["docker", "context", "inspect", host]).stdout)[0]["Endpoints"]["docker"]["Host"]
                argv += ["--host", host]
            begin, samples = time.monotonic(), []
            with log_path.open("wb") as output:
                process = subprocess.Popen(argv, stdout=output, stderr=subprocess.STDOUT, cwd=REPO)
                try:
                    while process.poll() is None:
                        samples.append({"s": round(time.monotonic() - begin, 3), "served": observed_part()})
                        require(time.monotonic() - begin < 120, "rollback exceeded two minutes")
                        time.sleep(.2)
                    code = process.wait(timeout=5)
                finally:
                    if process.poll() is None:
                        process.terminate()
                        try:
                            process.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait(timeout=5)
            elapsed = time.monotonic() - begin
            require(code != 0 and elapsed < 120, "broken deployment must fail and recover within two minutes")
            report = json.loads(journal.read_text())
            require(report["previous"] == args.good_image and report["phase"] == phase and not report["ok"], "wrong deployment journal outcome")
            if name == "delayed":
                require(elapsed >= 50 and report.get("rollback_ready"), "late failure was not observed after switch")
            require(inspect(service_id())["Config"]["Image"] == args.good_image, "previous digest was not restored")
            require(observed_part(), "previous digest no longer serves through Caddy")
            events = []
            for line in log_path.read_text().splitlines():
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                fields = event.get("fields", event)
                if fields.get("event") == "deploy.rollback":
                    events.append(fields)
            require(any(e.get("reason") == "readyz_failed" and e.get("outcome") == "ok" for e in events), "successful readyz rollback log missing")
            # The candidate must never interrupt the old service. The switched late
            # failure may show only the bounded compose service replacement gap.
            outages, opened = [], None
            for sample in samples + [{"s": elapsed, "served": True}]:
                if not sample["served"] and opened is None:
                    opened = sample["s"]
                elif sample["served"] and opened is not None:
                    outages.append(sample["s"] - opened)
                    opened = None
            require(not outages if name == "immediate" else max(outages, default=0) <= 10 and sum(outages) <= 20,
                    "part availability regressed beyond the bounded service switch")
            reports.append({"case": name, "rto_s": round(elapsed, 3), "samples": samples, "outage_s": outages,
                            "journal": report, "events": events})
        (args.evidence_dir / "rollback.json").write_text(json.dumps(reports, indent=2) + "\n")
    print(json.dumps({"event": "deploy.check", "check": args.mode, "outcome": "pass"}))


if __name__ == "__main__":
    main()
