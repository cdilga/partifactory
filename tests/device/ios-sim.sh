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
# Without a pf.test hosts entry, use localtest.me (public loopback DNS), like the sandbox
# spike. The per-run CA covers that apex; Safari still verifies TLS normally.
#   PF_APEX          override the harness apex (default pf.test with hosts entry, localtest.me otherwise)
#   PF_IOS_RUNTIME   simulator runtime id (default: the newest installed iOS runtime)
#   PF_IOS_DEVICE    device type id (default com.apple.CoreSimulator.SimDeviceType.iPhone-16)
#   PF_IOS_NAME      dedicated test device name (default pf-harness-iphone)
#   PF_IOS_BOOT_WAIT seconds allowed for boot (default 180)
#   PF_IOS_WAIT      seconds to wait for the POST (default 60)
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
h="$repo/tests/e2e/harness"
spec="${1:-self-test}"
if [ "$spec" = device-pass ]; then exec "$repo/tests/device/ios-device-pass.sh"; fi
page="${2:-/_dev/selftest}"
[ "$spec" = self-test ] && page=/_dev/selftest

[ "$(uname -s)" = Darwin ] || { echo "ios-sim.sh: needs macOS with Xcode" >&2; exit 2; }
command -v xcrun >/dev/null || { echo "ios-sim.sh: needs Xcode (xcrun)" >&2; exit 2; }
if [ -z "${PF_APEX:-}" ]; then
  if grep -Eq '^[^#]*parts\.pf\.test' /etc/hosts; then
    export PF_APEX=pf.test
  else
    export PF_APEX=localtest.me
  fi
fi
# The served part uses canonical HTTPS origins for its blob and runtime URLs.
# Docker publishes 443 through its existing port helper; no host file or DNS settings change.
export PF_HTTPS_PORT="${PF_HTTPS_PORT:-443}"

out="${PF_IOS_EVIDENCE_ROOT:-$repo/target/e2e-logs/ios-sim}/$spec"
mkdir -p "$out"
# Evidence from an earlier attempt must never make a failed run look successful.
rm -f "$out/report.json" "$out/screenshot.png"

own_harness=0
if [ -z "${PF_RUN_DIR:-}" ]; then
  "$repo/scripts/disk-guard.sh"
  PF_RUN_DIR="$("$h/up.sh")"
  own_harness=1
fi
# shellcheck disable=SC1091
set -a; source "$PF_RUN_DIR/env"; set +a

udid=""
open_pid=""
cleanup() {
  [ -n "$open_pid" ] && kill "$open_pid" >/dev/null 2>&1 || true
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
name="${PF_IOS_NAME:-pf-harness-iphone}"

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
python3 - "$udid" "$out/boot.log" "${PF_IOS_BOOT_WAIT:-180}" <<'PY'
import subprocess, sys
with open(sys.argv[2], "w") as log:
    try:
        subprocess.run(["xcrun", "simctl", "bootstatus", sys.argv[1], "-b"],
                       stdout=log, stderr=subprocess.STDOUT, timeout=int(sys.argv[3]), check=True)
    except subprocess.TimeoutExpired:
        sys.exit("ios-sim.sh: simulator boot timed out; see " + sys.argv[2])
PY
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
sep='?'
[[ "$page" == *\?* ]] && sep='&'
url="https://parts.$PF_APEX:$port$page${sep}spec=$spec&nonce=$nonce"
# A frame query contains a capability: keep it out of command logs.
echo "ios-sim: $device_name on $runtime_name opening https://parts.$PF_APEX:$port/_dev/selftest?spec=$spec&nonce=$nonce"
# On a cold Safari, openurl may time out even after handing the URL to the browser.
# The page's collector POST, rather than this launch command, decides the outcome.
xcrun simctl openurl "$udid" "$url" >"$out/openurl.log" 2>&1 &
open_pid=$!

wait_s="${PF_IOS_WAIT:-60}"
got="$(python3 - "$nonce" "$wait_s" "$PF_CA" "$PF_APEX" "$port" <<'PY'
import json, subprocess, sys, time
nonce, wait, ca, apex, port = sys.argv[1:]
deadline = time.monotonic() + int(wait)
while time.monotonic() < deadline:
    remaining = deadline - time.monotonic()
    try:
        response = subprocess.run([
            "curl", "-fsS", "--max-time", str(min(5, remaining)), "--cacert", ca,
            "--resolve", f"parts.{apex}:{port}:127.0.0.1",
            f"https://parts.{apex}:{port}/_dev/reports"
        ], capture_output=True, timeout=min(6, remaining), check=True)
        report = next((r for r in json.loads(response.stdout) if nonce in r.get("href", "")), None)
        if report:
            print(json.dumps(report))
            break
    except (subprocess.SubprocessError, ValueError):
        pass
    time.sleep(min(1, max(0, deadline - time.monotonic())))
PY
)"
python3 - "$udid" "$out/screenshot.png" <<'PY'
import subprocess, sys
try:
    subprocess.run(["xcrun", "simctl", "io", sys.argv[1], "screenshot", sys.argv[2]],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
except subprocess.TimeoutExpired:
    print("ios-sim: screenshot timed out", file=sys.stderr)
PY
if [ -z "$got" ]; then
  echo "ios-sim: FAIL no POST from the page within ${wait_s}s (screenshot: $out/screenshot.png)" >&2
  exit 1
fi
echo "$got" > "$out/report.json"
python3 - "$out/report.json" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
if report.get("pass") is False:
    sys.exit("ios-sim: FAIL page assertions: " + json.dumps(report.get("checks", {})))
PY
echo "ios-sim: ok, the page reported: $got"
echo "ios-sim: output in $out"
