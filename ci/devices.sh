#!/usr/bin/env bash
# A fresh disposable harness: real sign-in and guest open on the shipping browser projects.
source "$(dirname "$0")/lib.sh"
h="$repo/tests/e2e/harness"
export PF_E2E_RUN="${PF_E2E_RUN:-devices-$(date +%Y%m%d-%H%M%S)}"
export PF_RUN_DIR="$repo/target/e2e-runs/$PF_E2E_RUN"
export PF_E2E_RENDER=0 PF_DEVICE_PASS=1
export PF_BIN="${PF_BIN:-$repo/target/debug/partifactory}"
export HOST_UID="$(id -u)" HOST_GID="$(id -g)"
logs="$repo/target/e2e-logs/$PF_E2E_RUN"
mkdir -p "$logs" "$PF_RUN_DIR"
cleanup() {
  result=$?
  "$h/down.sh" "$PF_RUN_DIR" || true
  # Copy only public evidence. The run also contains live URLs, cookies and credentials.
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
  cargo build -p partifactory --features "$PF_FEATURES"
fi
(cd "$repo/tests/e2e" && npm ci --no-audit --no-fund --silent)
docker compose -f "$h/compose.yaml" --profile browsers build browsers
"$h/up.sh" >/dev/null
set -a
source "$PF_RUN_DIR/env"
set +a
# Exercise app-side OIDC discovery before parallel sign-ins, preserving TLS verification.
ready=0
for attempt in 1 2 3; do
  code="$(curl -sS --max-time 15 --cacert "$PF_CA" \
    --resolve "parts.pf.test:$PF_HTTPS_PORT:127.0.0.1" -o /dev/null -w '%{http_code}' \
    "https://parts.pf.test:$PF_HTTPS_PORT/signin?return=/" || true)"
  if [ "$code" = 303 ]; then ready=1; break; fi
  sleep 2
done
[ "$ready" = 1 ] || { echo 'device gate: application OIDC discovery unavailable' >&2; exit 1; }
python3 "$h/seed-part.py"
docker compose -f "$h/compose.yaml" --profile browsers run --rm \
  -e PF_DEVICE_PASS=1 browsers device_pass.spec.ts \
  --project chromium-mobile --project webkit-mobile --project firefox-desktop \
  2>&1 | tee "$logs/devices.txt"
python3 - "$logs/results.json" "$PF_RUN_DIR/device-pass" <<'PY'
import json, pathlib, sys
report=json.loads(pathlib.Path(sys.argv[1]).read_text())
required={'chromium-mobile','webkit-mobile','firefox-desktop'}
passed=set()
def visit(suites):
    for suite in suites:
        for spec in suite.get('specs',[]):
            for test in spec.get('tests',[]):
                results=test.get('results',[])
                if test.get('expectedStatus')=='passed' and test.get('status')=='expected' and results and all(r.get('status')=='passed' for r in results):
                    passed.add(test['projectName'])
        visit(suite.get('suites',[]))
visit(report.get('suites',[]))
assert required <= passed, ('device projects not observed passing',sorted(required-passed))
items={'standalone','sign_in','part_root_first_open','part_root_reload','guest_link'}
for lane in required:
    rows=json.loads((pathlib.Path(sys.argv[2])/(lane+'-report.json')).read_text())
    observed={r['item'] for r in rows if r.get('outcome')=='pass' and r.get('lane')==lane and r.get('browser')}
    assert items <= observed,(lane,sorted(items-observed))
    if lane=='chromium-mobile':assert 'passkey_step_up' in observed,lane
    print(json.dumps({'event':'device.matrix','lane':lane,'outcome':'pass','items':sorted(observed)}))
PY
