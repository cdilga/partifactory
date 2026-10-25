#!/usr/bin/env python3
"""Observe a real Dex outage without making readiness depend on the IdP.

Run after the main browser suite has authenticated the pinned admin. The caller
owns harness teardown. This script restores Dex even when any assertion fails.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_dir", type=Path)
    args = parser.parse_args()
    run = args.run_dir.resolve()
    repo = Path(__file__).resolve().parents[2]
    compose = [str(repo / "tests/e2e/harness/compose.sh")]
    env = dict(os.environ, PF_RUN_DIR=str(run))
    logs = repo / "target/e2e-logs" / os.environ.get("PF_E2E_RUN", run.name)
    logs.mkdir(parents=True, exist_ok=True)
    # Establish the real OIDC browser session before taking the provider down.
    # This also makes the observer runnable on its own, without a prior full suite.
    if not (run / "browser-auth/admin.json").is_file():
        subprocess.run(compose + ["--profile", "browsers", "run", "--rm", "browsers",
                                  "health_page.spec.ts", "--project", "chromium-desktop",
                                  "--grep", "admin Health exposes"],
                       cwd=repo, env=env, check=True)
    subprocess.run(compose + ["stop", "dex"], env=env, check=True, capture_output=True)
    try:
        deadline = time.monotonic() + 80
        while True:
            with urllib.request.urlopen(os.environ.get("PF_OPS_URL", "http://127.0.0.1:19090") + "/readyz", timeout=5) as response:
                assert response.status == 200
                report = json.load(response)
            assert report["ready"] is True and report["status"] == "ready"
            if any(check["name"] == "idp" and not check["ok"] and check["kind"] == "degraded" for check in report["checks"]):
                break
            assert time.monotonic() < deadline, "background sampler did not observe the real Dex outage"
            time.sleep(1)
        (logs / "readyz-idp-down.json").write_text(json.dumps(report, indent=2) + "\n")
        result = subprocess.run(compose + ["--profile", "browsers", "run", "--rm", "-e", "PF_IDP_DOWN=1", "browsers", "health_page.spec.ts", "--project", "chromium-desktop", "--grep", "admin Health exposes"], cwd=repo, env=env)
        if (logs / "results.json").is_file():
            shutil.copy2(logs / "results.json", logs / "results-health-idp-down.json")
        result.check_returncode()
        log = (run / "server.jsonl").read_text()
        lines = [json.loads(line) for line in log.splitlines() if line.startswith("{")]
        assert any(line.get("event") == "health.check" and line.get("name") == "idp" and line.get("outcome") == "degraded" for line in lines)
        print("health: actual Dex stopped; /readyz HTTP200 with degraded IdP, authenticated admin Health screenshot PASS")
    finally:
        subprocess.run(compose + ["start", "dex"], env=env, check=True, capture_output=True)


if __name__ == "__main__":
    main()
