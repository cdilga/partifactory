#!/usr/bin/env python3
"""Run the four real agent drivers against an existing native harness and render sidecar.

First run mcp.spec.ts (real browser consent writes its mode600 private tokens). The caller
owns harness teardown. TLS stays verified through the same local CONNECT proxy as CLI login;
no host DNS changes or elevated privileges are needed. Never prints tokens.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import ssl
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request


def normalise(value, name, key=""):
    volatile = {"request_id", "docs", "id", "part_id", "expires_at", "created_at", "updated_at", "taken_at", "updated_by", "actor", "actor_conn", "user_id", "conn_id", "at", "manifest_hash", "screenshot_hash", "timing"}
    if isinstance(value, dict):
        return {k: normalise(v, name, k) for k, v in value.items() if k not in volatile}
    if isinstance(value, list):
        return [normalise(v, name, key) for v in value]
    if isinstance(value, str):
        value = value.replace(name, "scenario-part")
        if key == "url" and "/api/v1/screenshots/" in value:
            return "https://parts.pf.test/api/v1/screenshots/generated"
        if value.startswith(("conn:", "user:")):
            return "principal"
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_dir", type=Path)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/partifactory"))
    parser.add_argument("--mcp-client", type=Path, default=Path("target/debug/examples/gate_client"))
    parser.add_argument("--https-port", type=int, default=18443)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    run = args.run_dir.resolve()
    tokens = json.loads((run / "agent-surface-private/tokens.json").read_text())
    assert set(tokens) == {"pat", "device"}
    scenario = json.loads((repo / "tests/agent_surfaces/scenario.yaml").read_text())
    # The external security observer runs after all eight live lanes and scans their actual
    # private inputs across origin logs, metrics and Insights. Never publish this file.
    private_inputs = []
    def collect_inputs(value):
        if isinstance(value, dict):
            for key, item in value.items():
                if key in {"content", "title", "old_str", "new_str"} and isinstance(item, str) and len(item) >= 8:
                    private_inputs.append(item)
                collect_inputs(item)
        elif isinstance(value, list):
            for item in value:
                collect_inputs(item)
    collect_inputs(scenario["steps"])
    private = run / "agent-surface-private/redaction-canaries.json"
    fd = os.open(private, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as stream:
        json.dump({"tokens": list(tokens.values()), "part_inputs": sorted(set(private_inputs))}, stream)
    os.chmod(private, 0o600)
    spec = importlib.util.spec_from_file_location("cli_proxy", repo / "tests/device/cli-login.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    directory = repo / "target/e2e-logs" / run.name / "conformance"
    directory.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(f"file:{run / 'data/partifactory.db'}?mode=ro", uri=True)
    with module.Server(("127.0.0.1", 0), module.Proxy) as proxy, tempfile.TemporaryDirectory(prefix="pf-surfaces-") as temp:
        proxy.upstream_port = args.https_port
        thread = threading.Thread(target=proxy.serve_forever, daemon=True)
        thread.start()
        part = Path(temp) / "part"
        part.symlink_to(args.binary.resolve())
        proxy_url = f"http://127.0.0.1:{proxy.server_address[1]}"
        context = ssl.create_default_context(cafile=str(run / "certs/ca.pem"))
        # ProxyHandler obeys NO_PROXY; this child alone clears it. The caller's environment
        # remains untouched, and TLS verification still uses the harness CA.
        os.environ["NO_PROXY"] = os.environ["no_proxy"] = ""
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({"https": proxy_url}), urllib.request.HTTPSHandler(context=context))
        origin = "https://parts.pf.test"
        def http(path, token=None, payload=None):
            headers = {"Accept": "text/markdown" if path == "/" else "application/json"}
            if token:
                headers["Authorization"] = f"Bearer {token}"
            if payload is not None:
                headers["Content-Type"] = "application/json"
            request = urllib.request.Request(origin + path, data=None if payload is None else json.dumps(payload).encode(), headers=headers)
            try:
                with opener.open(request, timeout=90) as response:
                    body = response.read()
                    try:
                        return json.loads(body)
                    except json.JSONDecodeError:
                        return body.decode()
            except urllib.error.HTTPError as error:
                return json.loads(error.read())
        try:
            document = http("/openapi.json")
            routes = {v["post"]["operationId"]: path for path, v in document["paths"].items() if "post" in v}
            skill = http("/skill.md")
            full = http("/llms-full.txt")
            template = skill.split("POST $PARTIFACTORY_URL", 1)[1].split("`", 1)[0]
            for endpoint in ["/", "/llms.txt", "/llms-full.txt", "/agents.md", "/.well-known/mcp/server.json"]:
                assert http(endpoint), endpoint
            baseline = None
            for driver in ["mcp", "openapi", "cli", "skill"]:
                for kind, token in tokens.items():
                    lane = f"{driver}-{kind}"
                    name = f"surface-{lane}-{time.time_ns()}"
                    env = dict(os.environ, PARTIFACTORY_URL=origin, PARTIFACTORY_TOKEN=token, PARTIFACTORY_CA_CERT=str(run / "certs/ca.pem"), PF_MCP_URL=origin + "/mcp", HTTPS_PROXY=proxy_url)
                    for key in ["https_proxy", "ALL_PROXY", "all_proxy"]:
                        env.pop(key, None)
                    if driver == "skill":
                        # Exercise the real public installer without any agent credential.
                        install_env = dict(env)
                        install_env.pop("PARTIFACTORY_TOKEN", None)
                        destination = Path(temp) / "installed-SKILL.md"
                        subprocess.run([str(part), "skill", "install", "--path", str(destination)], env=install_env, capture_output=True, text=True, check=True, timeout=30)
                        assert destination.read_text() == skill
                        agents = Path(temp) / "AGENTS.md"
                        agents.write_text("Existing project instructions.\n")
                        for _ in range(2):
                            subprocess.run([str(part), "skill", "install", "--codex"], env=install_env, cwd=temp, capture_output=True, text=True, check=True, timeout=30)
                        assert agents.read_text().startswith("Existing project instructions.\n")
                        assert agents.read_text().count("<!-- partifactory -->") == 1
                    transcript = []
                    start = db.execute("SELECT COALESCE(max(id),0) FROM audit").fetchone()[0]
                    for step in scenario["steps"]:
                        op = step["op"]
                        payload = json.loads(json.dumps(step["input"]).replace("$name", name).replace("$rid", f"{lane}-{step['id']}-{time.time_ns()}"))
                        if driver == "mcp":
                            completed = subprocess.run([str(args.mcp_client.resolve())], input=json.dumps([{"op": op, "input": payload}]), env=env, capture_output=True, text=True, timeout=100)
                            if completed.returncode:
                                raise RuntimeError(f"{lane}/{step['id']}: rmcp client failed: {completed.stderr}")
                            response = json.loads(completed.stdout)
                            result = response["result"]
                            if op == "part_check" and not response["is_error"]:
                                assert response["images"] == 1
                        elif driver == "cli":
                            file = Path(temp) / "input.json"
                            file.write_text(json.dumps(payload))
                            completed = subprocess.run([str(part), op, "--json", f"@{file}"], env=env, capture_output=True, text=True, timeout=100)
                            result = json.loads(completed.stdout if completed.returncode == 0 else completed.stderr)
                        else:
                            if driver == "skill":
                                assert f"<!-- pf:op {op} -->" in skill and f"### {op}" in full
                                route = template.replace("<op>", op)
                            else:
                                route = routes[op]
                            result = http(route, token, payload)
                        if "error" in step:
                            assert result["error"]["code"] == step["error"], (lane, step["id"], result)
                            assert result["error"]["hint"]
                        else:
                            assert "error" not in result, (lane, step["id"], result)
                        if step["id"] == "stale-rebase":
                            assert result["rebased_from"] == 1
                        if step["id"] == "get-restored":
                            assert result["content"]["index.md"] == "- passports\n- tickets\n"
                        transcript.append({"step": step["id"], "result": normalise(result, name)})
                        # Await the real worker between mutations. This keeps queued build
                        # responses comparable and observes actual publication before reads.
                        if op in {"part_create", "part_edit"} and "error" not in result:
                            deadline = time.monotonic() + 30
                            while True:
                                current = http(routes["part_get"], token, {"name": name})
                                if current.get("build", {}).get("status") == "ok" and current["head"] == current["live"]:
                                    break
                                if time.monotonic() > deadline:
                                    raise RuntimeError(f"{lane}: build did not go live")
                                time.sleep(.1)
                    audit = db.execute("SELECT action,target_kind,before,after FROM audit WHERE id>? ORDER BY id", (start,)).fetchall()
                    transcript.append({"audit": audit})
                    # All supported ops leave outward sharing unchanged for these new parts.
                    assert db.execute("SELECT count(*) FROM public_links l JOIN names n ON n.part_id=l.part_id WHERE n.name=?", (name,)).fetchone()[0] == 0
                    (directory / f"{lane}.jsonl").write_text("".join(json.dumps(row, sort_keys=True) + "\n" for row in transcript))
                    if baseline is None:
                        baseline = transcript
                    elif transcript != baseline:
                        import difflib
                        diff = "".join(difflib.unified_diff(json.dumps(baseline, indent=2, sort_keys=True).splitlines(True), json.dumps(transcript, indent=2, sort_keys=True).splitlines(True), fromfile="baseline", tofile=lane))
                        raise AssertionError("Cross-surface divergence:\n" + diff)
                    print(f"PASS {lane}: create/get/rebase/conflict/person-link/data/check/restore; real renderer", flush=True)
        finally:
            proxy.shutdown()
            thread.join(timeout=5)
            db.close()


if __name__ == "__main__":
    main()
