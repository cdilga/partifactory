#!/usr/bin/env bash
# Mobile Safari in the iOS Simulator against the e2e harness (pf-kurh). A local lane: it runs
# on the Mac (or a macOS runner), is never a required CI check, and beads whose gate names it
# attach its output to their `verified` report.
#
#   tests/device/ios-sim.sh self-test        open the harness self-test page, collect its POST
#   tests/device/ios-sim.sh <spec> [path]    open https://parts.pf.test:<port><path>?spec=<spec>
#
# What it does: brings the harness up (unless PF_RUN_DIR names a running one), boots a dedicated
# simulator device on the installed iOS runtime, trusts the run's CA in the simulator keychain
# (`simctl keychain add-root-cert`; no certificate-error flags), opens the page with
# `simctl openurl`, waits for the page's POST to the harness collector (`/_dev/report`), takes
# `simctl io screenshot`, and writes target/e2e-logs/ios-sim/<spec>/{report.json,screenshot.png,
# lane.json}. Exits non-zero when the POST never arrives.
#
# One-time setup (needs the admin, because /etc/hosts is root's): the simulator resolves names
# through the Mac, so add
#   127.0.0.1 parts.pf.test notes-part.pf.test blobs-part.pf.test dex.pf.test dex2.pf.test
# to /etc/hosts.
#
#   PF_IOS_RUNTIME   simulator runtime id (default: the newest installed iOS runtime)
#   PF_IOS_DEVICE    device type id (default com.apple.CoreSimulator.SimDeviceType.iPhone-16)
#   PF_IOS_WAIT      seconds to wait for the POST (default 60)
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
h="$repo/tests/e2e/harness"
spec="${1:-self-test}"
page="${2:-/_dev/selftest}"
[ "$spec" = self-test ] && page=/_dev/selftest

[ "$(uname -s)" = Darwin ] || { echo "ios-sim.sh: needs macOS with Xcode" >&2; exit 2; }
command -v xcrun >/dev/null || { echo "ios-sim.sh: needs Xcode (xcrun)" >&2; exit 2; }
if ! grep -Eq '^[^#]*\bparts\.pf\.test\b' /etc/hosts; then
  echo "ios-sim.sh: /etc/hosts has no parts.pf.test entry. Ask the admin to add:" >&2
  echo "  127.0.0.1 parts.pf.test notes-part.pf.test blobs-part.pf.test dex.pf.test dex2.pf.test" >&2
  exit 2
fi

out="$repo/target/e2e-logs/ios-sim/$spec"
mkdir -p "$out"

own_harness=0
if [ -z "${PF_RUN_DIR:-}" ]; then
  "$repo/scripts/disk-guard.sh"
  PF_RUN_DIR="$("$h/up.sh")"
  own_harness=1
fi
# shellcheck disable=SC1091
set -a; source "$PF_RUN_DIR/env"; set +a

udid=""
cleanup() {
  [ -n "$udid" ] && xcrun simctl shutdown "$udid" >/dev/null 2>&1 || true
  if [ "$own_harness" = 1 ]; then "$h/down.sh" "$PF_RUN_DIR" || true; fi
}
trap cleanup EXIT

runtime="${PF_IOS_RUNTIME:-$(xcrun simctl list runtimes -j | python3 -c '
import json, sys
rs = [r for r in json.load(sys.stdin)["runtimes"] if r.get("isAvailable") and r["identifier"].startswith("com.apple.CoreSimulator.SimRuntime.iOS")]
print(sorted(rs, key=lambda r: [int(x) for x in r["version"].split(".")])[-1]["identifier"] if rs else "")')}"
[ -n "$runtime" ] || { echo "ios-sim.sh: no iOS simulator runtime installed" >&2; exit 2; }
devtype="${PF_IOS_DEVICE:-com.apple.CoreSimulator.SimDeviceType.iPhone-16}"
name="pf-harness-iphone"

# A dedicated device, so the lane never disturbs simulators other sessions use.
udid="$(xcrun simctl list devices -j | python3 -c '
import json, sys
j = json.load(sys.stdin)
for d in j["devices"].get(sys.argv[1], []):
    if d["name"] == sys.argv[2]: print(d["udid"]); break' "$runtime" "$name")"
if [ -z "$udid" ]; then
  udid="$(xcrun simctl create "$name" "$devtype" "$runtime" 2>/dev/null || xcrun simctl create "$name" com.apple.CoreSimulator.SimDeviceType.iPhone-15 "$runtime")"
fi
xcrun simctl boot "$udid" 2>/dev/null || true
xcrun simctl bootstatus "$udid" -b >/dev/null
xcrun simctl keychain "$udid" add-root-cert "$PF_CA"

runtime_name="$(xcrun simctl list runtimes -j | python3 -c '
import json, sys
for r in json.load(sys.stdin)["runtimes"]:
    if r["identifier"] == sys.argv[1]: print(r["name"] + " (" + r["buildversion"] + ")")' "$runtime")"
device_name="$(xcrun simctl list devices -j | python3 -c '
import json, sys
for ds in json.load(sys.stdin)["devices"].values():
    for d in ds:
        if d["udid"] == sys.argv[1]: print(d["deviceTypeIdentifier"].rsplit(".", 1)[-1])' "$udid")"
printf '{"lane":"ios-sim","browser":"Mobile Safari (WebKit)","runtime":"%s","device":"%s","macos":"%s","udid":"%s"}\n' \
  "$runtime_name" "$device_name" "$(sw_vers -productVersion)" "$udid" > "$out/lane.json"

nonce="$(openssl rand -hex 8)"
port="$PF_HTTPS_PORT"
url="https://parts.pf.test:$port$page?spec=$spec&nonce=$nonce"
echo "ios-sim: $device_name on $runtime_name opening $url"
xcrun simctl openurl "$udid" "$url"

C=(curl -fsS --cacert "$PF_CA" --resolve "parts.pf.test:$port:127.0.0.1")
wait_s="${PF_IOS_WAIT:-60}"
got=""
for _ in $(seq 1 "$wait_s"); do
  got="$("${C[@]}" "https://parts.pf.test:$port/_dev/reports" | python3 -c '
import json, sys
for r in json.load(sys.stdin):
    if sys.argv[1] in r.get("href", ""): print(json.dumps(r)); break' "$nonce" || true)"
  [ -n "$got" ] && break
  sleep 1
done
sleep 1
xcrun simctl io "$udid" screenshot "$out/screenshot.png" >/dev/null 2>&1 || true
if [ -z "$got" ]; then
  echo "ios-sim: FAIL no POST from the page within ${wait_s}s (screenshot: $out/screenshot.png)" >&2
  exit 1
fi
echo "$got" > "$out/report.json"
echo "ios-sim: ok, the page reported: $got"
echo "ios-sim: output in $out"
