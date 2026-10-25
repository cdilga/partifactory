#!/usr/bin/env bash
# Genuine Mobile Safari UI tests, with an ephemeral test-owned simulator and trusted TLS.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo"
[[ "$(uname -s)" = Darwin ]] || { echo 'ios device pass requires Xcode on macOS' >&2; exit 2; }
scripts/disk-guard.sh
export PF_APEX="${PF_APEX:-localtest.me}" PF_HTTPS_PORT="${PF_HTTPS_PORT:-443}"
[[ "$PF_APEX" = localtest.me ]] || { echo 'ios device pass: use a fresh localtest.me harness; no hosts edits are needed' >&2; exit 2; }
owned=0
udid=
cleanup(){
  if [[ -n "$udid" ]]; then
    xcrun simctl shutdown "$udid" >/dev/null 2>&1 || true
    xcrun simctl delete "$udid" >/dev/null 2>&1 || true
  fi
  if [[ "$owned" = 1 ]]; then tests/e2e/harness/down.sh "$PF_RUN_DIR"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM
if [[ -z "${PF_RUN_DIR:-}" ]]; then
  export PF_E2E_RUN="${PF_E2E_RUN:-ios-device-$(date +%Y%m%d-%H%M%S)}"
  export PF_RUN_DIR="$repo/target/e2e-runs/$PF_E2E_RUN"
  export PF_BIN="${PF_BIN:-$repo/target/debug/partifactory}" PF_HARNESS_MODE=host
  owned=1
  tests/e2e/harness/up.sh >/dev/null
  set -a; source "$PF_RUN_DIR/env"; set +a
  python3 tests/e2e/harness/seed-part.py
else
  set -a; source "$PF_RUN_DIR/env"; set +a
fi
[[ "$PF_APEX" = localtest.me ]] || { echo 'ios device pass: the running harness must use localtest.me' >&2; exit 2; }
out="$repo/target/e2e-logs/ios-sim/device-pass/$PF_E2E_RUN"
mkdir -p "$out"
chmod 700 "$out"
runtime="${PF_IOS_RUNTIME:-$(xcrun simctl list runtimes -j | python3 -c 'import json,sys; rs=[r for r in json.load(sys.stdin)["runtimes"] if r.get("isAvailable") and r["identifier"].startswith("com.apple.CoreSimulator.SimRuntime.iOS")]; print(sorted(rs,key=lambda r:[int(x) for x in r["version"].split(".")])[-1]["identifier"])')}"
udid="$(xcrun simctl create "pf-shipping-$PF_E2E_RUN" com.apple.CoreSimulator.SimDeviceType.iPhone-16 "$runtime")"
xcrun simctl boot "$udid"
xcrun simctl bootstatus "$udid" -b > "$out/boot.log" 2>&1
xcrun simctl keychain "$udid" add-root-cert "$PF_CA"
xcodebuild build-for-testing -project tests/device/SimulatorTests.xcodeproj -scheme PartifactoryDevice \
  -destination "platform=iOS Simulator,id=$udid" -derivedDataPath target/device-xctest \
  CODE_SIGNING_ALLOWED=NO > "$out/build.log" 2>&1
# Link creation/authorization is independently tested with the Chromium authenticator. This
# view-only fixture isolates Safari guest rendering from simulator passkey hardware support.
python3 - "$PF_RUN_DIR" "$PF_APEX" "$out" <<'PY'
import base64,hashlib,json,pathlib,plistlib,secrets,sqlite3,sys,time
run,apex,out=pathlib.Path(sys.argv[1]),sys.argv[2],pathlib.Path(sys.argv[3])
token=base64.urlsafe_b64encode(secrets.token_bytes(16)).decode().rstrip('=')
c=sqlite3.connect(run/'data/partifactory.db',timeout=30)
with c:
    part,owner=c.execute("SELECT id,owner_user_id FROM parts WHERE name='notes'").fetchone()
    c.execute("INSERT INTO public_links(id,token_hash,part_id,mode,created_by,created_at) VALUES(?,?,?,'view',?,?)",('l_ios_'+secrets.token_hex(8),hashlib.sha256(token.encode()).digest(),part,owner,int(time.time()*1000)))
c.close()
source=next(pathlib.Path('target/device-xctest/Build/Products').glob('PartifactoryDevice_*.xctestrun'))
value=plistlib.loads(source.read_bytes())
env=value['PartifactoryDevice']['EnvironmentVariables']
nonce=secrets.token_hex(8)
env.update(PF_DEVICE_GUEST=f'https://notes-part.{apex}/l/{token}',PF_DEVICE_STANDALONE=json.loads((run/'part-fixture.json').read_text())['notes']['url'],PF_DEVICE_SIGNIN=f'https://parts.{apex}/signin?return=/',PF_DEVICE_ROOT=f'https://notes-part.{apex}/',PF_DEVICE_REPORT=f'https://parts.{apex}/_dev/report',PF_DEVICE_NONCE=nonce)
configured=source.with_name('PartifactoryDevice-configured.xctestrun')
configured.write_bytes(plistlib.dumps(value))
configured.chmod(0o600)
(out/'nonce').write_text(nonce)
print(configured)
PY
xcodebuild test-without-building -xctestrun target/device-xctest/Build/Products/PartifactoryDevice-configured.xctestrun \
  -destination "platform=iOS Simulator,id=$udid" -parallel-testing-enabled NO \
  -resultBundlePath "$out/result.xcresult" > "$out/test.log" 2>&1
curl -fsS --cacert "$PF_CA" --resolve "parts.$PF_APEX:$PF_HTTPS_PORT:127.0.0.1" \
  "https://parts.$PF_APEX:$PF_HTTPS_PORT/_dev/reports" > "$out/collector.json"
python3 - "$out" "$runtime" <<'PY'
import json,pathlib,sys
out=pathlib.Path(sys.argv[1]);nonce=(out/'nonce').read_text()
rows=[r for r in json.loads((out/'collector.json').read_text()) if r.get('run')==nonce and r.get('lane')=='ios-simulator']
required={'guest_link','standalone','sign_in','part_root_first_open','part_root_reload'}
assert {r['item'] for r in rows if r.get('outcome')=='pass'}==required,rows
(out/'report.json').write_text(json.dumps({'pass':True,'runtime':sys.argv[2],'checks':{r['item']:True for r in rows},'reports':rows},indent=2))
print('ios device pass: all five real Mobile Safari matrix checks passed; evidence '+str(out))
PY
