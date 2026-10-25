#!/usr/bin/env python3
"""Install the real release archive with cargo-binstall into a private test directory."""
import argparse
import hashlib
import http.server
import json
import os
import platform
from pathlib import Path
import subprocess
import tarfile
import tempfile
import threading


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binstall", default="cargo-binstall")
    parser.add_argument("--assets", type=Path, default=Path("target/release-assets"))
    parser.add_argument("--output", type=Path, default=Path("target/gate-evidence/binstall"))
    parser.add_argument("--report", type=Path, help="Also write the public JSON receipt here")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    assets = args.assets.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix="install-", dir=args.output)).resolve()
    sums = dict(line.split("  ", 1)[::-1] for line in (assets / "SHA256SUMS").read_text().splitlines())
    archive = assets / "part-linux-amd64.tar.gz"
    archive_sha = hashlib.sha256(archive.read_bytes()).hexdigest()
    assert sums[archive.name] == archive_sha, "release archive checksum mismatch"
    with tarfile.open(archive) as package:
        assert package.getnames() == ["part"], "CLI archive must contain exactly part"
        member = package.getmember("part")
        assert member.isfile() and member.mode & 0o111, "CLI must be executable"
        expected = package.extractfile(member).read()
    assert expected.startswith(b"\x7fELF"), "expected actual Linux release binary"
    requests = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_HEAD(self):
            requests.append(self.path)
            self.send_response(200 if self.path == "/" + archive.name else 404)
            self.send_header("Content-Length", str(archive.stat().st_size))
            self.end_headers()

        def do_GET(self):
            requests.append(self.path)
            if self.path != "/" + archive.name:
                self.send_error(404)
                return
            content = archive.read_bytes()
            self.send_response(200)
            self.send_header("Content-Length", str(len(content)))
            self.end_headers()
            try:
                self.wfile.write(content)
            except (BrokenPipeError, ConnectionResetError):
                # Binstall probes availability, then downloads in a new request.
                pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    env = os.environ.copy()
    # Do not inherit strategy/target overrides or transmit unrelated credentials.
    for key in list(env):
        if key.startswith("BINSTALL_") or key in ("CARGO_BUILD_TARGET", "GITHUB_TOKEN", "GH_TOKEN"):
            del env[key]
    command = [args.binstall, "--manifest-path", str(repo / "deploy/part-cli.Cargo.toml"),
               "--pkg-url", f"http://127.0.0.1:{server.server_port}/{archive.name}",
               "--allow-insecure-http", "--no-discover-github-token", "--disable-telemetry",
               "--settings", str(output / "binstall-settings.toml"), "--no-confirm"]
    report = {"archive_sha256": archive_sha, "binary_sha256": hashlib.sha256(expected).hexdigest(),
              "binstall": subprocess.check_output([args.binstall, "-V"], text=True).strip(), "targets": {}}
    try:
        for target in ("x86_64-unknown-linux-musl", "x86_64-unknown-linux-gnu",
                       "aarch64-unknown-linux-musl", "aarch64-apple-darwin",
                       "x86_64-apple-darwin", "x86_64-pc-windows-msvc"):
            destination = output / target
            destination.mkdir()
            before = len(requests)
            result = subprocess.run(command + ["--targets", target, "--install-path", str(destination),
                                              "partifactory-cli"], env=env, capture_output=True, text=True, timeout=60)
            (output / (target + ".log")).write_text(result.stdout + result.stderr)
            installed = destination / "part"
            supported = target in ("x86_64-unknown-linux-musl", "x86_64-unknown-linux-gnu")
            if supported:
                assert result.returncode == 0, f"supported install failed: {target}; see {output}"
                assert installed.read_bytes() == expected, "installed release bytes changed"
                assert os.access(installed, os.X_OK), "installed CLI is not executable"
                assert not (destination / "partifactory").exists(), "wrong argv0 selects server mode"
                assert len(requests) > before, "no real archive download occurred"
                if platform.system() == "Linux" and platform.machine() in ("x86_64", "amd64"):
                    help_result = subprocess.run([str(installed), "--help"], env=env,
                                                 capture_output=True, text=True, timeout=15)
                    assert help_result.returncode == 0 and "part_create" in help_result.stdout, "installed part did not enter agent CLI mode"
                    (output / (target + "-help.txt")).write_text(help_result.stdout)
            else:
                assert result.returncode != 0 and not installed.exists(), f"unsupported platform installed: {target}"
                assert len(requests) == before, f"unsupported target requested archive: {target}"
                assert "Fallback to cargo-install is disabled" in result.stdout + result.stderr, "fallback restriction not observed"
            report["targets"][target] = {"supported": supported, "exit_code": result.returncode,
                                          "archive_requests": len(requests) - before,
                                          "native_cli_executed": supported and platform.system() == "Linux"
                                          and platform.machine() in ("x86_64", "amd64")}
        report["ok"] = True
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({"ok": True, "report": str(output / "report.json")}, indent=2))
    finally:
        server.shutdown()
        server.server_close()
        worker.join()


if __name__ == "__main__":
    main()
