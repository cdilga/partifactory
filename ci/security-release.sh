#!/usr/bin/env bash
# Actual release image, shipped compose and real browser/OAuth setup; no dev endpoints.
source "$(dirname "$0")/lib.sh"
h="$repo/tests/e2e/harness"
export PF_E2E_RUN="${PF_E2E_RUN:-release-$(date +%Y%m%d-%H%M%S)}"
export PF_RUN_DIR="$repo/target/e2e-runs/$PF_E2E_RUN"
export PF_HARNESS_RELEASE=1 PF_HARNESS_MODE=container PF_E2E_RENDER=1
export PF_BIN="${PF_BIN:-$repo/target/debug/partifactory}"
export HOST_UID="$(id -u)" HOST_GID="$(id -g)"
logs="$repo/target/e2e-logs/$PF_E2E_RUN"
mkdir -p "$logs" "$PF_RUN_DIR"
registry=""
cleanup() {
  result=$?
  "$h/down.sh" "$PF_RUN_DIR" || true
  if [ -n "$registry" ]; then docker rm -fv "$registry" >/dev/null 2>&1 || true; fi
  # Publish only public evidence, never live caps, cookies or token files.
  if [ -d "$PF_RUN_DIR/device-pass" ]; then
    mkdir -p "$logs/device-reports"
    for file in "$PF_RUN_DIR/device-pass/"*-report.json "$PF_RUN_DIR/device-pass/"*-guest.png; do
      [ ! -f "$file" ] || cp "$file" "$logs/device-reports/"
    done
  fi
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
if [ "${PF_E2E_SKIP_BUILD:-0}" != 1 ]; then
  cargo build -p partifactory -p pf-mcp --features "$PF_FEATURES" --bins --examples
fi
if [ -z "${PF_HARNESS_IMAGE:-}" ]; then
  scripts/release.sh --dry-run --tag "gate-$(git rev-parse --short HEAD)"
  registry="pf-security-registry-$(date +%s)"
  docker run -d --rm --name "$registry" --label pf.harness=1 \
    --tmpfs /var/lib/registry:rw,nosuid,nodev,size=128m \
    -p 127.0.0.1:25000:5000 registry:3.0.0@sha256:6c5666b861f3505b116bb9aa9b25175e71210414bd010d92035ff64018f9457e >/dev/null
  for attempt in $(seq 1 30); do
    if curl -fsS http://127.0.0.1:25000/v2/ >/dev/null 2>&1; then break; fi
    sleep 1
  done
  python3 scripts/load-release.py --assets target/release-assets \
    --registry http://127.0.0.1:25000 --repository partifactory \
    --tag "gate-$(git rev-parse --short HEAD)" --pull --output "$PF_RUN_DIR/release-image"
  export PF_HARNESS_IMAGE="$(cat "$PF_RUN_DIR/release-image")"
fi
tests/deploy/image_checks.sh "$PF_HARNESS_IMAGE" | tee "$logs/image-checks.txt"
(cd "$repo/tests/e2e" && npm ci --no-audit --no-fund --silent)
"$h/build-browsers.sh"
"$h/up.sh" >/dev/null
set -a
source "$PF_RUN_DIR/env"
set +a
ready=0
for attempt in $(seq 1 12); do
  code="$(curl -sS --max-time 15 --cacert "$PF_CA" \
    --resolve "parts.pf.test:$PF_HTTPS_PORT:127.0.0.1" -o /dev/null -w '%{http_code}' \
    "https://parts.pf.test:$PF_HTTPS_PORT/signin?return=/" || true)"
  if [ "$code" = 303 ]; then ready=1; break; fi
  sleep 1
done
[ "$ready" = 1 ] || { echo 'release gate: application OIDC discovery unavailable' >&2; exit 1; }
"$h/compose.sh" --profile browsers run --rm browsers harness/release-setup.spec.ts --project chromium-desktop
cp "$logs/results.json" "$logs/results-setup.json"
export PF_SECURITY_DEVICE_FILE=/run/pf/run/security-private/device.json
"$h/compose.sh" --profile security run --rm security-device-client \
  --base-url https://parts.pf.test --ca-cert /run/pf/run/certs/ca.pem \
  --connect-to 172.30.79.10:443 --prepare-device-file "$PF_SECURITY_DEVICE_FILE"
"$h/compose.sh" --profile browsers run --rm -e PF_SECURITY_DEVICE_FILE="$PF_SECURITY_DEVICE_FILE" \
  browsers security.spec.ts mcp.spec.ts
cp "$logs/results.json" "$logs/results-security.json"
python3 tests/agent_surfaces/harness.py "$PF_RUN_DIR" --binary "$PF_BIN" \
  --mcp-client "${PF_MCP_CLIENT:-$repo/target/debug/examples/gate_client}" \
  2>&1 | tee "$logs/conformance.txt"
python3 tests/render/isolation.py "$PF_RUN_DIR" | tee "$logs/summary.txt"
python3 deploy/proxy/test-runtime-logs.py | tee "$logs/proxy-runtime-log-gate.json"
guest_content="$(python3 - "$PF_RUN_DIR/release-private/guest-content.json" <<'PY'
import json,pathlib,sys,urllib.parse
url=json.loads(pathlib.Path(sys.argv[1]).read_text())['url']
parsed=urllib.parse.urlsplit(url)
assert parsed.scheme=='https' and parsed.path.startswith('/c/') and not parsed.username
print(url)
PY
)"
python3 tests/security/run.py --base-url https://parts.pf.test \
  --ca-cert "$PF_CA" --connect-to "127.0.0.1:$PF_HTTPS_PORT" \
  --fixture "$PF_RUN_DIR/part-fixture.json" --sibling-origin https://pixel-part.pf.test \
  --session-file "$PF_RUN_DIR/release-private/session.json" \
  --token-file "$PF_RUN_DIR/release-private/agent-token" \
  --guest-content-url "$guest_content" --view-content-url "$guest_content" \
  --logs-file "$PF_RUN_DIR/server.jsonl" --database-file "$PF_RUN_DIR/data/partifactory.db" \
  --metrics-url "$PF_OPS_URL/metrics" --metrics-token-file "$PF_METRICS_TOKEN_FILE" \
  --insights-db "$PF_RUN_DIR/data/insights.db" \
  --canary-values-file "$PF_RUN_DIR/release-private/redaction-canaries.json" \
  --canary-values-file "$PF_RUN_DIR/agent-surface-private/redaction-canaries.json" \
  --secret-file "$PF_RUN_DIR/secrets/session_key" --secret-file "$PF_RUN_DIR/secrets/cap_hmac_key" \
  --secret-file "$PF_RUN_DIR/secrets/idp_client_secret" --secret-file "$PF_RUN_DIR/secrets/metrics_token" \
  --playwright-report "$logs/results-security.json" --evidence-dir "$logs/security" \
  2>&1 | tee "$logs/security.txt"
"$h/compose.sh" --profile browsers run --rm -e PF_DEVICE_PASS=1 \
  browsers device_pass.spec.ts --project chromium-mobile --project webkit-mobile --project firefox-desktop \
  2>&1 | tee "$logs/devices.txt"
cp "$logs/results.json" "$logs/results-devices.json"
# A skipped or failed project cannot be hidden by a successful report from another project.
python3 - "$logs/results-devices.json" "$PF_RUN_DIR/device-pass" <<'PY'
import json,pathlib,sys
report=json.loads(pathlib.Path(sys.argv[1]).read_text())
required={'chromium-mobile','webkit-mobile','firefox-desktop'};passed=set()
def visit(suites):
    for suite in suites:
        for spec in suite.get('specs',[]):
            for test in spec.get('tests',[]):
                results=test.get('results',[])
                if test.get('expectedStatus')=='passed' and test.get('status')=='expected' and results and all(r.get('status')=='passed' for r in results):passed.add(test['projectName'])
        visit(suite.get('suites',[]))
visit(report.get('suites',[]));assert required<=passed,sorted(required-passed)
items={'standalone','sign_in','part_root_first_open','part_root_reload','guest_link'}
for lane in required:
    rows=json.loads((pathlib.Path(sys.argv[2])/(lane+'-report.json')).read_text())
    seen={r['item'] for r in rows if r.get('outcome')=='pass' and r.get('lane')==lane and r.get('browser')}
    assert items<=seen,(lane,sorted(items-seen))
    if lane=='chromium-mobile':assert 'passkey_step_up' in seen
    print(json.dumps({'event':'device.matrix','lane':lane,'outcome':'pass','items':sorted(seen)}))
PY
