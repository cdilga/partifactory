#!/usr/bin/env python3
"""Create the browser fixture through the running server, then wait for its real build."""
import json
import os
from pathlib import Path
import subprocess
import time

REPO = Path(__file__).resolve().parents[3]
RUN = Path(os.environ["PF_RUN_DIR"])
if os.environ["PF_HARNESS_MODE"] == "host":
    COMMAND = [os.environ["PF_BIN"], "dev", "--socket", str(RUN / "data/run/dev.sock")]
else:
    COMMAND = ["docker", "compose", "-f", str(REPO / "tests/e2e/harness/compose.yaml"),
               "--profile", "container", "exec", "-T", "partifactory",
               "/usr/local/bin/partifactory", "dev"]


def call(op, args):
    result = subprocess.run(COMMAND + ["request", op], input=json.dumps(args),
                            text=True, capture_output=True, timeout=50, check=True)
    return json.loads(result.stdout)


call("sign-in", {"username": "admin"})
html = """<!doctype html><html lang="en"><head><!-- <head> is not a real head -->
<script>
window.firstAuthorScript = {
  storage: typeof window.storage, local: localStorage.getItem('initial'),
  cookie: document.cookie, indexedDB: typeof indexedDB, caches: typeof caches
};
</script><meta charset="utf-8"><title>Serving fixture</title></head>
<body><h1>Serving fixture</h1><div id="result">ready</div></body></html>"""
call("put", {"name": "notes", "kind": "html", "request_id": "e2e-serving-fixture",
             "files": {"index.html": html, "data.json": '{"answer":42}',
                       "worker.js": 'self.onmessage = function () { postMessage("worker-ok"); };'}})
deadline = time.monotonic() + 45
while True:
    part = call("get", {"name": "notes"})
    status = (part.get("build") or {}).get("status")
    if status == "ok":
        break
    if status in ("failed", "timeout", "superseded"):
        raise SystemExit(f"seed-part: fixture build ended {status}")
    if time.monotonic() > deadline:
        raise SystemExit(f"seed-part: fixture did not build; status={status}")
    time.sleep(0.1)
fixtures = {}


def seed(name, kind, files):
    call("put", {"name": name, "kind": kind, "request_id": "e2e-" + name, "files": files})
    deadline = time.monotonic() + 45
    while True:
        result = call("get", {"name": name})
        status = (result.get("build") or {}).get("status")
        if status == "ok":
            break
        if status in ("failed", "timeout", "superseded") or time.monotonic() > deadline:
            raise SystemExit(f"seed-part: {name} build failed: {result.get('build')}")
        time.sleep(0.1)
    fixtures[name] = call("cap", {"name": name})


fixtures["notes"] = call("cap", {"name": "notes"})
fixtures["expired"] = call("cap", {"name": "notes", "at_ms": int(time.time() * 1000) - 13 * 3600 * 1000})
seed("device", "html", {"index.html": """<!doctype html><meta charset="utf-8"><h1>Part host device probe</h1><pre id="status">checking…</pre>
<script>
(async function () {
  const url = new URL(location.href), nonce = url.searchParams.get('nonce') || 'device', key = 'probe-' + nonce;
  if (!url.searchParams.has('read')) {
    localStorage.setItem(key, 'saved-before-navigation');
    url.searchParams.set('read', '0');
    location.replace(url.href);
    return;
  }
  if (localStorage.getItem(key) !== 'saved-before-navigation' && Number(url.searchParams.get('read')) < 20) {
    url.searchParams.set('read', String(Number(url.searchParams.get('read')) + 1));
    setTimeout(() => location.replace(url.href), 100);
    return;
  }
  const checks = {opaque: window.origin === 'null', secure: isSecureContext, cookie: document.cookie === '',
    storage: localStorage.getItem(key) === 'saved-before-navigation', indexedDB: typeof indexedDB === 'undefined',
    caches: typeof caches === 'undefined', prelude: !!window.partifactory};
  try {
    checks.fetch = (await (await fetch('/data.json')).json()).answer === 42;
    const worker = new Worker('/worker.js');
    checks.worker = await new Promise(resolve => {
      const timer = setTimeout(() => resolve(false), 5000);
      worker.onmessage = e => {clearTimeout(timer); resolve(e.data === 'worker-ok');};
      worker.onerror = () => {clearTimeout(timer); resolve(false);};
      worker.postMessage('run');
    });
    worker.terminate();
  } catch (error) { checks.network = false; }
  const result = {pass: Object.values(checks).every(Boolean), checks};
  document.getElementById('status').textContent = JSON.stringify(result, null, 2);
  parent.postMessage({type: 'pf:harness', result}, '*');
})();
</script>""", "data.json": '{"answer":42}',
      "worker.js": 'self.onmessage = function () { postMessage("worker-ok"); };'})
seed("markdown", "markdown", {"index.md": "# A working Markdown part\n\n~~gone~~ and **bold**\n\n| A | B |\n| - | - |\n| 1 | 2 |\n"})
seed("text", "text", {"index.txt": "\ufeff<tag>\r\n& second\n"})
seed("bundle", "files", {
    "index.html": '<!doctype html><h1>Bundle</h1><script type="module" src="/assets/app.js"></script>',
    "assets/app.js": 'import { value } from "./chunk.js"; document.body.dataset.bundle=value;',
    "assets/chunk.js": 'export const value="loaded";',
})
seed("mirrored", "html", {"index.html": '<!doctype html><h1 id="mirrored">Mirrored libraries</h1><script type="module" src="https://unpkg.com/widget@latest/main.js"></script><link rel="stylesheet" href="https://unpkg.com/widget@1.2.3/style.css"><script src="https://unpkg.com/unknown.js"></script>'})
# The executable contract is the shared-data fixture, not a separately maintained copy.
contract = REPO.joinpath("docs/agent-contract.md").read_text()
board = contract.split('Hermes writes `board.html`:', 1)[1].split('```html\n', 1)[1].split('\n```', 1)[0]
seed("pixel", "html", {"index.html": board})
seed("public-pixel", "html", {"index.html": board})
for browser in ("chromium-desktop", "chromium-mobile", "webkit-mobile", "firefox-desktop"):
    seed("pixel-" + browser, "html", {"index.html": board})
    seed("pixel-undo-" + browser, "html", {"index.html": board})
RUN.joinpath("part-fixture.json").write_text(json.dumps(fixtures))
print("seed-part: all four kinds built; mirrored CDN graph and capabilities ready")
