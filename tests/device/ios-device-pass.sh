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
release_mode="${PF_HARNESS_RELEASE:-0}"
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
  if [[ "$release_mode" = 1 ]]; then
    : "${PF_HARNESS_IMAGE:?ios release pass requires the immutable release image}"
    export PF_HARNESS_MODE=container
  else
    export PF_BIN="${PF_BIN:-$repo/target/debug/partifactory}" PF_HARNESS_MODE=host
  fi
  owned=1
  tests/e2e/harness/up.sh >/dev/null
  set -a; source "$PF_RUN_DIR/env"; set +a
  if [[ "$release_mode" = 1 ]]; then
    tests/e2e/harness/compose.sh --profile browsers run --rm browsers \
      harness/release-setup.spec.ts --project chromium-desktop
  else
    python3 tests/e2e/harness/seed-part.py
  fi
else
  set -a; source "$PF_RUN_DIR/env"; set +a
fi
release_mode="${PF_HARNESS_RELEASE:-$release_mode}"
export PF_IOS_HARNESS_KIND=dev
if [[ "$release_mode" = 1 ]]; then
  : "${PF_HARNESS_IMAGE:?ios release pass requires the immutable release image}"
  [[ "${PF_HARNESS_MODE:-}" = container ]] || { echo 'ios release pass requires the container harness' >&2; exit 2; }
  [[ -s "$PF_RUN_DIR/part-fixture.json" && -s "$PF_RUN_DIR/device-pass/guest-link.json" ]] || {
    echo 'ios release pass requires fixtures made by the real release-setup browser flow' >&2; exit 2;
  }
  export PF_IOS_HARNESS_KIND=release
fi
[[ "$PF_APEX" = localtest.me ]] || { echo 'ios device pass: the running harness must use localtest.me' >&2; exit 2; }
# /healthz deliberately ignores IdP availability. Wait for the application's own
# lazy OIDC discovery before opening the browser, just as the browser harness does.
signin_ready=0
for attempt in {1..12}; do
  status="$(curl -sS --max-time 15 --cacert "$PF_CA" \
    --resolve "parts.$PF_APEX:$PF_HTTPS_PORT:127.0.0.1" \
    -o /dev/null -w '%{http_code}' "https://parts.$PF_APEX:$PF_HTTPS_PORT/signin?return=/" || true)"
  if [[ "$status" = 303 ]]; then signin_ready=1; break; fi
  sleep 1
done
[[ "$signin_ready" = 1 ]] || { echo 'ios device pass: application OIDC discovery did not become ready' >&2; exit 1; }
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
# Release links come from the real Chromium passkey + browser API setup. The dev-only
# fixture remains available for the accumulated native-binary lane.
python3 - "$PF_RUN_DIR" "$PF_APEX" "$out" <<'PY'
import json,os,pathlib,plistlib,secrets,sys
run,apex,out=pathlib.Path(sys.argv[1]),sys.argv[2],pathlib.Path(sys.argv[3])
release=os.environ['PF_IOS_HARNESS_KIND']=='release'
if release:
    guest=json.loads((run/'device-pass/guest-link.json').read_text())['url']
    report=f'https://blobs-part.{apex}/__harness/report'
    reports=f'https://blobs-part.{apex}/__harness/reports'
else:
    import base64,hashlib,sqlite3,time
    token=base64.urlsafe_b64encode(secrets.token_bytes(16)).decode().rstrip('=')
    c=sqlite3.connect(run/'data/partifactory.db',timeout=30)
    with c:
        part,owner=c.execute("SELECT id,owner_user_id FROM parts WHERE name='notes'").fetchone()
        c.execute("INSERT INTO public_links(id,token_hash,part_id,mode,created_by,created_at) VALUES(?,?,?,'view',?,?)",('l_ios_'+secrets.token_hex(8),hashlib.sha256(token.encode()).digest(),part,owner,int(time.time()*1000)))
    c.close()
    guest=f'https://notes-part.{apex}/l/{token}'
    report=f'https://parts.{apex}/_dev/report'
    reports=f'https://parts.{apex}/_dev/reports'
source=next(pathlib.Path('target/device-xctest/Build/Products').glob('PartifactoryDevice_*.xctestrun'))
value=plistlib.loads(source.read_bytes())
env=value['PartifactoryDevice']['EnvironmentVariables']
nonce=secrets.token_hex(8)
env.update(PF_DEVICE_GUEST=guest,PF_DEVICE_STANDALONE=json.loads((run/'part-fixture.json').read_text())['notes']['url'],PF_DEVICE_SIGNIN=f'https://parts.{apex}/signin?return=/',PF_DEVICE_ROOT=f'https://notes-part.{apex}/',PF_DEVICE_REPORT=report,PF_DEVICE_NONCE=nonce)
configured=source.with_name('PartifactoryDevice-configured.xctestrun')
configured.write_bytes(plistlib.dumps(value))
configured.chmod(0o600)
(out/'nonce').write_text(nonce)
(out/'collector-url').write_text(reports)
print(configured)
PY
xcodebuild test-without-building -xctestrun target/device-xctest/Build/Products/PartifactoryDevice-configured.xctestrun \
  -destination "platform=iOS Simulator,id=$udid" -parallel-testing-enabled NO \
  -collect-test-diagnostics never \
  -resultBundlePath "$out/result.xcresult" > "$out/test.log" 2>&1
xcrun xcresulttool export attachments --path "$out/result.xcresult" \
  --output-path "$out/screenshots" --filter '*.png' > "$out/attachments.log" 2>&1
collector_host="parts.$PF_APEX"
if [[ "$release_mode" = 1 ]]; then collector_host="blobs-part.$PF_APEX"; fi
curl -fsS --cacert "$PF_CA" --resolve "$collector_host:$PF_HTTPS_PORT:127.0.0.1" \
  "$(cat "$out/collector-url")" > "$out/collector.json"
python3 - "$out" "$runtime" <<'PY'
import json,os,pathlib,sys
out=pathlib.Path(sys.argv[1]);nonce=(out/'nonce').read_text()
rows=[r for r in json.loads((out/'collector.json').read_text()) if r.get('run')==nonce and r.get('lane')=='ios-simulator']
required={'guest_link','standalone','sign_in','part_root_first_open','part_root_reload'}
assert {r['item'] for r in rows if r.get('outcome')=='pass'}==required,rows
screenshots=sorted(str(p.relative_to(out)) for p in (out/'screenshots').glob('*.png'))
assert len(screenshots)==len(required),screenshots
(out/'report.json').write_text(json.dumps({'pass':True,'runtime':sys.argv[2],'harness':os.environ['PF_IOS_HARNESS_KIND'],'image':os.environ.get('PF_HARNESS_IMAGE') if os.environ['PF_IOS_HARNESS_KIND']=='release' else None,'checks':{r['item']:True for r in rows},'reports':rows,'screenshots':screenshots},indent=2))
print('ios device pass: all five real Mobile Safari matrix checks passed; evidence '+str(out))
PY
