#!/usr/bin/env python3
"""Real native part login + browser approval against an already-running e2e harness.

Usage: tests/device/cli-login.py RUN_DIR [--project chromium-desktop|webkit-mobile]
The temporary CONNECT proxy preserves pf.test TLS verification without editing host DNS.
The caller owns the harness and must run its normal down.sh cleanup afterwards.
"""
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import select
import socket
import socketserver
import subprocess
import tempfile
import threading
import time


class Proxy(socketserver.StreamRequestHandler):
    def handle(self):
        request = self.rfile.readline(8192).decode("ascii")
        if request.strip() != "CONNECT parts.pf.test:443 HTTP/1.1":
            self.wfile.write(b"HTTP/1.1 403 Forbidden\r\n\r\n")
            return
        while self.rfile.readline(8192).strip():
            pass
        with socket.create_connection(("127.0.0.1", self.server.upstream_port), timeout=10) as upstream:
            self.wfile.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            self.wfile.flush()
            sockets = [self.connection, upstream]
            while True:
                ready, _, _ = select.select(sockets, [], [], 30)
                if not ready:
                    return
                for source in ready:
                    data = source.recv(65536)
                    if not data:
                        return
                    (upstream if source is self.connection else self.connection).sendall(data)


class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_dir", type=Path)
    parser.add_argument("--project", action="append", choices=["chromium-desktop", "webkit-mobile"])
    parser.add_argument("--binary", type=Path, default=Path("target/debug/partifactory"))
    parser.add_argument("--https-port", type=int, default=18443)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    run_dir = args.run_dir.resolve()
    compose = repo / "tests/e2e/harness/compose.yaml"
    with Server(("127.0.0.1", 0), Proxy) as proxy, tempfile.TemporaryDirectory(prefix="pf-cli-gate-") as temp:
        proxy.upstream_port = args.https_port
        thread = threading.Thread(target=proxy.serve_forever, daemon=True)
        thread.start()
        part = Path(temp) / "part"
        part.symlink_to(args.binary.resolve())
        env = dict(os.environ, PARTIFACTORY_URL="https://parts.pf.test", PARTIFACTORY_CA_CERT=str(run_dir / "certs/ca.pem"), HTTPS_PROXY=f"http://127.0.0.1:{proxy.server_address[1]}", NO_PROXY="", no_proxy="", PF_RUN_DIR=str(run_dir))
        for key in ["PARTIFACTORY_TOKEN", "ALL_PROXY", "all_proxy", "https_proxy"]:
            env.pop(key, None)
        try:
            for project in args.project or ["chromium-desktop", "webkit-mobile"]:
                config = Path(temp) / f"{project}.json"
                env["PARTIFACTORY_CONFIG"] = str(config)
                public = run_dir / f"cli-approval-{project}.txt"
                stderr = Path(temp) / "login.stderr"
                process = None
                try:
                    with stderr.open("w") as log:
                        process = subprocess.Popen([str(part), "login", "--no-browser", "--agent-name", f"Native CLI {project}"], env=env, stdout=subprocess.PIPE, stderr=log, text=True)
                        deadline = time.monotonic() + 30
                        approval = None
                        while time.monotonic() < deadline:
                            approval = next((line for line in stderr.read_text().splitlines() if line.startswith("https://parts.pf.test/device?code=")), None)
                            if approval or process.poll() is not None:
                                break
                            time.sleep(.1)
                        if not approval:
                            raise RuntimeError("CLI did not produce a device approval URL: " + stderr.read_text())
                        public.write_text(approval + "\n")
                        subprocess.run(["docker", "compose", "-f", str(compose), "--profile", "browsers", "run", "--rm", "browsers", "cli_login.spec.ts", "--project", project], cwd=repo, env=env, check=True)
                        stdout, _ = process.communicate(timeout=30)
                    if process.returncode != 0:
                        raise RuntimeError("CLI login failed: " + stderr.read_text())
                    result = json.loads(stdout)
                    assert result["signed_in"] is True
                    assert "pf_at_" not in stdout and "pf_rt_" not in stdout
                    assert config.stat().st_mode & 0o777 == 0o600
                    name = f"cli-native-{project}-{time.time_ns()}"
                    def command(*arguments):
                        completed = subprocess.run([str(part), *arguments], env=env, capture_output=True, text=True, check=True, timeout=40)
                        return json.loads(completed.stdout)
                    command("part_create", "--name", name, "--kind", "text", "--content", "Created by the real device-connected CLI.")
                    saved = json.loads(config.read_text())
                    old_refresh = saved["refresh_token"]
                    saved["expires_at"] = 0
                    config.write_text(json.dumps(saved))
                    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                        results = list(pool.map(lambda _: command("get", name), range(2)))
                    assert len(results) == 2
                    assert json.loads(config.read_text())["refresh_token"] != old_refresh
                    print(f"PASS {project}: real part login, browser consent, mode600, part get, concurrent refresh family survives", flush=True)
                finally:
                    public.unlink(missing_ok=True)
                    if process is not None and process.poll() is None:
                        process.terminate()
                        process.wait(timeout=5)
        finally:
            proxy.shutdown()
            thread.join(timeout=5)


if __name__ == "__main__":
    main()
