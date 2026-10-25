#!/usr/bin/env bash
# Fresh shipped-compose harness; real native rmcp + real device/browser pixel acceptance.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo"
scripts/disk-guard.sh
export PF_E2E_RUN="${PF_E2E_RUN:-cpx-$(date +%H%M%S)}"
export PF_APEX="${PF_APEX:-pf.test}" PF_HTTPS_PORT="${PF_HTTPS_PORT:-443}" PF_E2E_RENDER=1
export PF_BIN="${PF_BIN:-$repo/target/debug/partifactory}"
client="${PF_MCP_CLIENT:-$repo/target/debug/examples/gate_client}"
[[ -x "$client" ]] || { echo 'communal-pixel: build existing gate_client with the standard dev feature set first' >&2; exit 2; }
owned=0
broker=
cleanup(){
  if [[ -n "$broker" ]]; then kill "$broker" >/dev/null 2>&1 || true;wait "$broker" >/dev/null 2>&1 || true; fi
  if [[ "$owned" = 1 ]]; then tests/e2e/harness/down.sh "$PF_RUN_DIR"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM
if [[ -z "${PF_RUN_DIR:-}" ]]; then
  export PF_RUN_DIR="$repo/target/e2e-runs/$PF_E2E_RUN"
  owned=1
  tests/e2e/harness/up.sh >/dev/null
fi
set -a; source "$PF_RUN_DIR/env"; set +a
out="$repo/target/e2e-logs/$PF_E2E_RUN/communal_pixel_editor"
mkdir -p "$out"
ready=0
for attempt in {1..12}; do
  code="$(curl -sS --max-time 15 --cacert "$PF_CA" --resolve "parts.$PF_APEX:$PF_HTTPS_PORT:127.0.0.1" \
    -o /dev/null -w '%{http_code}' "https://parts.$PF_APEX:$PF_HTTPS_PORT/signin?return=/" || true)"
  if [[ "$code" = 303 ]]; then ready=1;break;fi
  sleep 1
done
[[ "$ready" = 1 ]] || { echo 'communal-pixel: OIDC discovery not ready' >&2;exit 1; }
python3 tests/device/pixel-agent-broker.py "$PF_RUN_DIR" --client "$client" --apex "$PF_APEX" --port "$PF_HTTPS_PORT" \
  > "$out/broker.log" 2>&1 &
broker=$!
python3 - "$PF_RUN_DIR/communal-private/ready" "$broker" <<'PY'
import os,pathlib,sys,time
marker=pathlib.Path(sys.argv[1]);deadline=time.monotonic()+10
while not marker.exists():
    os.kill(int(sys.argv[2]),0)
    if time.monotonic()>deadline:sys.exit('Native rmcp bridge did not start')
    time.sleep(.05)
PY
tests/e2e/harness/compose.sh --profile browsers run --rm -e PF_COMMUNAL_ACCEPTANCE=1 browsers \
  communal_pixel_editor.spec.ts --project chromium-desktop --project chromium-mobile --workers 1 \
  2>&1 | tee "$out/browser.log"
python3 - "$out" <<'PY'
import json,pathlib,sys
out=pathlib.Path(sys.argv[1]);reports=[json.loads((out/name/'report.json').read_text()) for name in ('chromium-desktop','chromium-mobile')]
assert all(r['pass'] and r['p100Ms']<1000 and r['guestRefusals']==3 and r['rollbackEmptyTabs']==4 for r in reports)
(out/'report.json').write_text(json.dumps({'pass':True,'checks':{r['project']:True for r in reports},'p100Ms':max(r['p100Ms'] for r in reports)},indent=2))
print('communal_pixel_editor: BOTH agent surfaces, BOTH admin profiles, THREE guest browsers, exact budgets, rollback and Freeze PASS')
PY
