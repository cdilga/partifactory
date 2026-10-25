#!/usr/bin/env bash
# Real signed-out Mobile Safari paint/freeze, paired with Chromium's admin observer.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo"
[[ "$(uname -s)" = Darwin ]] || { echo 'pixel guest requires Xcode on macOS' >&2; exit 2; }
scripts/disk-guard.sh
export PF_APEX="${PF_APEX:-localtest.me}" PF_HTTPS_PORT="${PF_HTTPS_PORT:-443}" PF_E2E_RENDER=1
[[ "$PF_APEX" = localtest.me ]] || { echo 'pixel guest uses localtest.me without hosts edits' >&2; exit 2; }
owned=0
udid=
admin_pid=
nonce="$(openssl rand -hex 8)"
admin_container="pf-ios-pixel-$nonce"
cleanup(){
  docker rm -f "$admin_container" >/dev/null 2>&1 || true
  [[ -z "$admin_pid" ]] || wait "$admin_pid" >/dev/null 2>&1 || true
  if [[ -n "$udid" ]]; then
    xcrun simctl shutdown "$udid" >/dev/null 2>&1 || true
    xcrun simctl delete "$udid" >/dev/null 2>&1 || true
  fi
  if [[ -n "${out:-}" && -d "$out/result.xcresult" && ! -d "$out/screenshots" ]]; then
    xcrun xcresulttool export attachments --path "$out/result.xcresult" \
      --output-path "$out/screenshots" --filter '*.png' > "$out/attachments.log" 2>&1 || true
  fi
  if [[ "$owned" = 1 ]]; then tests/e2e/harness/down.sh "$PF_RUN_DIR"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM
if [[ -z "${PF_RUN_DIR:-}" ]]; then
  # Keep the native macOS Unix-domain dev socket below SUN_LEN.
  export PF_E2E_RUN="${PF_E2E_RUN:-ipx-$(date +%H%M%S)}"
  export PF_RUN_DIR="$repo/target/e2e-runs/$PF_E2E_RUN"
  if [[ "${PF_HARNESS_RELEASE:-0}" = 1 ]]; then
    : "${PF_HARNESS_IMAGE:?requires the immutable release image}"
    export PF_HARNESS_MODE=container
  else
    export PF_HARNESS_MODE=host PF_BIN="${PF_BIN:-$repo/target/debug/partifactory}"
  fi
  owned=1
  tests/e2e/harness/up.sh >/dev/null
fi
set -a; source "$PF_RUN_DIR/env"; set +a
[[ "$PF_APEX" = localtest.me ]] || { echo 'running harness must use localtest.me' >&2; exit 2; }
signin_ready=0
for attempt in {1..12}; do
  status="$(curl -sS --max-time 15 --cacert "$PF_CA" \
    --resolve "parts.$PF_APEX:$PF_HTTPS_PORT:127.0.0.1" \
    -o /dev/null -w '%{http_code}' "https://parts.$PF_APEX:$PF_HTTPS_PORT/signin?return=/" || true)"
  if [[ "$status" = 303 ]]; then signin_ready=1; break; fi
  sleep 1
done
[[ "$signin_ready" = 1 ]] || { echo 'pixel guest: OIDC discovery did not become ready' >&2; exit 1; }
out="$repo/target/e2e-logs/ios-sim/${1:-pixel-guest}/$PF_E2E_RUN"
mkdir -p "$out"
chmod 700 "$out"
runtime="${PF_IOS_RUNTIME:-$(xcrun simctl list runtimes -j | python3 -c 'import json,sys; rs=[r for r in json.load(sys.stdin)["runtimes"] if r.get("isAvailable") and r["identifier"].startswith("com.apple.CoreSimulator.SimRuntime.iOS")]; print(sorted(rs,key=lambda r:[int(x) for x in r["version"].split(".")])[-1]["identifier"])')}"
udid="$(xcrun simctl create "pf-pixel-$nonce" com.apple.CoreSimulator.SimDeviceType.iPhone-16 "$runtime")"
xcrun simctl boot "$udid"
xcrun simctl bootstatus "$udid" -b > "$out/boot.log" 2>&1
xcrun simctl keychain "$udid" add-root-cert "$PF_CA"
xcodebuild build-for-testing -project tests/device/SimulatorTests.xcodeproj -scheme PartifactoryDevice \
  -destination "platform=iOS Simulator,id=$udid" -derivedDataPath target/device-xctest \
  CODE_SIGNING_ALLOWED=NO > "$out/build.log" 2>&1
# This Chromium flow creates the board with a real scoped device token and creates the
# contribute link through Share after a real passkey ceremony. No database or dev API setup.
tests/e2e/harness/compose.sh --profile browsers run --rm --name "$admin_container" \
  -e "PF_IOS_PIXEL=$nonce" browsers harness/ios-pixel-guest.spec.ts --project chromium-desktop \
  > "$out/admin.log" 2>&1 &
admin_pid=$!
python3 - "$PF_RUN_DIR" "$out" "$nonce" "$admin_pid" <<'PY'
import json,os,pathlib,plistlib,sys,time
run,out,nonce,pid=pathlib.Path(sys.argv[1]),pathlib.Path(sys.argv[2]),sys.argv[3],int(sys.argv[4])
source=next(pathlib.Path('target/device-xctest/Build/Products').glob('PartifactoryDevice_*.xctestrun'))
deadline=time.monotonic()+150
while True:
    try:
        guest=json.loads((run/'device-pixel/guest.json').read_text())
        if guest['nonce']==nonce: break
    except (OSError,ValueError): pass
    try: os.kill(pid,0)
    except ProcessLookupError: sys.exit('Chromium setup failed; inspect admin.log')
    if time.monotonic()>deadline: sys.exit('Chromium setup barrier timed out; inspect admin.log')
    time.sleep(.1)
value=plistlib.loads(source.read_bytes())
env=value['PartifactoryDevice']['EnvironmentVariables']
env.pop('PF_DEVICE_PIXEL',None)
env.update(PF_DEVICE_PIXEL=guest['url'],PF_DEVICE_REPORT='https://blobs-part.localtest.me/__harness/report',PF_DEVICE_NONCE=nonce)
configured=source.with_name('PartifactoryDevice-pixel.xctestrun')
configured.write_bytes(plistlib.dumps(value));configured.chmod(0o600)
PY
xcodebuild test-without-building -xctestrun target/device-xctest/Build/Products/PartifactoryDevice-pixel.xctestrun \
  -destination "platform=iOS Simulator,id=$udid" -parallel-testing-enabled NO \
  -only-testing:PartifactoryDevice/SimulatorDeviceTests/testMobileSafariPixelGuest \
  -collect-test-diagnostics never -resultBundlePath "$out/result.xcresult" > "$out/test.log" 2>&1
wait "$admin_pid"
admin_pid=
xcrun xcresulttool export attachments --path "$out/result.xcresult" \
  --output-path "$out/screenshots" --filter '*.png' > "$out/attachments.log" 2>&1
curl -fsS --cacert "$PF_CA" --resolve "blobs-part.$PF_APEX:$PF_HTTPS_PORT:127.0.0.1" \
  "https://blobs-part.$PF_APEX/__harness/reports?run=$nonce" > "$out/collector.json"
python3 - "$out" "$PF_RUN_DIR" "$runtime" <<'PY'
import json,os,pathlib,sys
out,run=pathlib.Path(sys.argv[1]),pathlib.Path(sys.argv[2])
rows=json.loads((out/'collector.json').read_text())
required={'admin_ready','guest_painted','admin_live_under_1s','admin_frozen','guest_refused','pixel_paint','pixel_freeze'}
assert {r['item'] for r in rows if r['outcome']=='pass'}==required,rows
assert all(r.get('os') for r in rows if r['lane']=='ios-simulator'),rows
admin=json.loads((run/'device-pixel/admin-report.json').read_text());assert admin['pass']
screenshots=sorted(str(p.relative_to(out)) for p in (out/'screenshots').glob('*.png'))
assert len(screenshots)==2,screenshots
(out/'report.json').write_text(json.dumps({'pass':True,'runtime':sys.argv[3],'harness':'release' if os.environ.get('PF_HARNESS_RELEASE')=='1' else 'dev','image':os.environ.get('PF_HARNESS_IMAGE'),'checks':{item:True for item in required},'responseToComputedStyleMs':admin['responseToComputedStyleMs'],'reports':rows,'screenshots':screenshots},indent=2))
print('ios pixel guest: actual Safari paint, admin update <1s, Freeze refusal/revert PASS; '+str(out))
PY
