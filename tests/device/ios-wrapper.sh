#!/usr/bin/env bash
# Actual Mobile Safari first-open and silent regrant after a real release-service restart.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo"
[[ "$(uname -s)" = Darwin ]] || { echo 'ios wrapper requires Xcode on macOS' >&2;exit 2; }
[[ "${PF_HARNESS_RELEASE:-0}" = 1 ]] || { echo 'ios wrapper requires the actual release-image lane' >&2;exit 2; }
: "${PF_HARNESS_IMAGE:?requires the immutable release image}"
scripts/disk-guard.sh
export PF_APEX="${PF_APEX:-localtest.me}" PF_HTTPS_PORT="${PF_HTTPS_PORT:-443}" PF_E2E_RENDER=0
[[ "$PF_APEX" = localtest.me ]] || { echo 'ios wrapper uses localtest.me without hosts edits' >&2;exit 2; }
owned=0
if [[ -n "${PF_RUN_DIR:-}" ]]; then
  [[ "${PF_IOS_PREPARED_FRESH:-0}" = 1 && -n "${PF_REMOTE_READER:-}" ]] || {
    echo 'ios wrapper requires its own fresh harness or an explicitly prepared fresh remote source' >&2;exit 2;
  }
  : "${PF_E2E_RUN:?requires the actual prepared remote run identifier}"
else
  export PF_E2E_RUN="${PF_E2E_RUN:-iwr-$(date +%H%M%S)}" PF_HARNESS_MODE=container
  export PF_RUN_DIR="$repo/target/e2e-runs/$PF_E2E_RUN"
  owned=1
fi
out="$repo/target/e2e-logs/ios-sim/wrapper-regrant/$PF_E2E_RUN"
mkdir -p "$out"
udid= observer=
cleanup(){
  if [[ -n "$observer" ]]; then kill "$observer" >/dev/null 2>&1 || true;wait "$observer" >/dev/null 2>&1 || true;fi
  if [[ -n "$udid" ]]; then xcrun simctl shutdown "$udid" >/dev/null 2>&1 || true;xcrun simctl delete "$udid" >/dev/null 2>&1 || true;fi
  if [[ -d "$out/result.xcresult" && ! -d "$out/screenshots" ]]; then
    xcrun xcresulttool export attachments --path "$out/result.xcresult" --output-path "$out/screenshots" --filter '*.png' > "$out/attachments.log" 2>&1 || true
  fi
  if [[ "$owned" = 1 ]];then tests/e2e/harness/down.sh "$PF_RUN_DIR" || true;fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM
if [[ "$owned" = 1 ]];then tests/e2e/harness/up.sh >/dev/null;fi
set -a;source "$PF_RUN_DIR/env";set +a
if [[ "$owned" = 0 ]];then
  # Check the actual source process before any setup flow: no previously opened
  # wrapper or created fixture can be relabelled as a fresh first-open observation.
  tests/device/remote-run.sh pull compose.env
  python3 - "$out/fresh-source.json" <<'PY'
import json,pathlib,subprocess,sys
raw=subprocess.run(['tests/e2e/harness/compose.sh','--profile','container','logs','--no-color','--no-log-prefix','partifactory'],capture_output=True,text=True,check=True,timeout=20).stdout
rows=[json.loads(line) for line in raw.splitlines() if line.startswith('{')]
boots=[r['boot_epoch'] for r in rows if r.get('event')=='store.ready']
assert boots==[1],('source must be a fresh data directory and first process',boots)
assert not any(r.get('event') in ['op','part.write','wrapper.serve','chrome.grant','link.create'] or r.get('path')=='/grant' for r in rows), 'source has already been used'
pathlib.Path(sys.argv[1]).write_text(json.dumps({'fresh':True,'bootEpoch':1,'logRows':len(rows)}))
PY
fi
nonce="$(openssl rand -hex 8)"
ready=0
for attempt in {1..12}; do
  code="$(curl -sS --max-time 15 --cacert "$PF_CA" --resolve "parts.$PF_APEX:$PF_HTTPS_PORT:127.0.0.1" -o /dev/null -w '%{http_code}' "https://parts.$PF_APEX:$PF_HTTPS_PORT/signin?return=/" || true)"
  if [[ "$code" = 303 ]]; then ready=1;break;fi
  sleep 1
done
[[ "$ready" = 1 ]] || { echo 'ios wrapper: real OIDC discovery not ready' >&2;exit 1; }
tests/e2e/harness/compose.sh --profile browsers run --rm -e "PF_IOS_WRAPPER=$nonce" browsers harness/ios-wrapper-setup.spec.ts --project chromium-desktop > "$out/setup.log" 2>&1
runtime="${PF_IOS_RUNTIME:-$(xcrun simctl list runtimes -j | python3 -c 'import json,sys;rs=[r for r in json.load(sys.stdin)["runtimes"] if r.get("isAvailable") and r["identifier"].startswith("com.apple.CoreSimulator.SimRuntime.iOS")];print(sorted(rs,key=lambda r:[int(x) for x in r["version"].split(".")])[-1]["identifier"])')}"
udid="$(xcrun simctl create "pf-wrapper-$nonce" com.apple.CoreSimulator.SimDeviceType.iPhone-16 "$runtime")"
xcrun simctl boot "$udid"
xcrun simctl bootstatus "$udid" -b > "$out/boot.log" 2>&1
xcrun simctl keychain "$udid" add-root-cert "$PF_CA"
xcodebuild build-for-testing -project tests/device/SimulatorTests.xcodeproj -scheme PartifactoryDevice -destination "platform=iOS Simulator,id=$udid" -derivedDataPath target/device-xctest CODE_SIGNING_ALLOWED=NO > "$out/build.log" 2>&1
python3 - "$nonce" "$PF_APEX" <<'PY'
import pathlib,plistlib,sys
nonce,apex=sys.argv[1:]
source=next(pathlib.Path('target/device-xctest/Build/Products').glob('PartifactoryDevice_*.xctestrun'))
value=plistlib.loads(source.read_bytes());env=value['PartifactoryDevice']['EnvironmentVariables']
env.pop('PF_DEVICE_PIXEL',None)
env.update(PF_DEVICE_WRAPPER=f'https://ios-wrapper-part.{apex}/',PF_DEVICE_SIGNIN=f'https://parts.{apex}/signin?return=/',PF_DEVICE_REPORT=f'https://blobs-part.{apex}/__harness/report',PF_DEVICE_NONCE=nonce)
configured=source.with_name('PartifactoryDevice-wrapper.xctestrun');configured.write_bytes(plistlib.dumps(value));configured.chmod(0o600)
PY
python3 tests/device/wrapper-observe.py "$PF_RUN_DIR" "$out" "$nonce" > "$out/observer.log" 2>&1 &
observer=$!
xcodebuild test-without-building -xctestrun target/device-xctest/Build/Products/PartifactoryDevice-wrapper.xctestrun -destination "platform=iOS Simulator,id=$udid" -parallel-testing-enabled NO -only-testing:PartifactoryDevice/SimulatorDeviceTests/testMobileSafariWrapperRegrant -collect-test-diagnostics never -resultBundlePath "$out/result.xcresult" > "$out/test.log" 2>&1
wait "$observer";observer=
xcrun xcresulttool export attachments --path "$out/result.xcresult" --output-path "$out/screenshots" --filter '*.png' > "$out/attachments.log" 2>&1
python3 - "$out" "$runtime" <<'PY'
import json,pathlib,sys
out=pathlib.Path(sys.argv[1]);r=json.loads((out/'observer-report.json').read_text());rows=json.loads((out/'collector.json').read_text())
assert {v['item'] for v in rows if v.get('lane')=='ios-simulator' and v.get('outcome')=='pass'}=={'wrapper_first_open','wrapper_regrant'},rows
images=sorted(str(p.relative_to(out)) for p in (out/'screenshots').glob('*.png'));assert len(images)==2,images
r.update(runtime=sys.argv[2],reports=rows,screenshots=images)
(out/'report.json').write_text(json.dumps(r,indent=2));print('ios wrapper first-open + real boot-epoch silent regrant PASS; '+str(out))
PY
