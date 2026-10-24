#!/usr/bin/env bash
# pf-6qma sandbox spike, Mobile Safari in the iOS Simulator (macOS only).
# Boots an iPhone on the iOS runtime (SPIKE_IOS_RUNTIME, default iOS 27.0), trusts the rig CA in
# the simulator keychain (no certificate-error flags), opens one runner URL per item with
# `simctl openurl`, waits for the runner's done report, and screenshots each item.
# Hosts: https://*.localtest.me (public DNS for 127.0.0.1), because /etc/hosts on the Mac needs
# root; the rig cert covers both pf.test and localtest.me. Output: target/sandbox-spike/ios-sim/.
#   ./ios-sim.sh                   all items
#   ./ios-sim.sh 1,2,10            some items
set -euo pipefail
source "$(dirname "$0")/lib.sh"
trap spike_teardown EXIT
[[ "$(uname)" == Darwin ]] || { echo "ios-sim.sh needs macOS with Xcode" >&2; exit 2; }

SIM_OUT="$SPIKE_OUT/ios-sim"
mkdir -p "$SIM_OUT"
RUNTIME="${SPIKE_IOS_RUNTIME:-com.apple.CoreSimulator.SimRuntime.iOS-27-0}"
if [[ "${1:-}" == --delete-device ]]; then
  udid="$(xcrun simctl list devices -j | node -e '
    const j = JSON.parse(require("fs").readFileSync(0, "utf8"));
    for (const ds of Object.values(j.devices)) for (const d of ds) if (d.name === (process.env.SPIKE_SIM_NAME || "pf-spike-iphone")) console.log(d.udid);')"
  for u in $udid; do xcrun simctl shutdown "$u" 2>/dev/null || true; xcrun simctl delete "$u"; done
  exit 0
fi
ITEMS="${1:-login,1,1x,1h,2,2top,3,4,5,6,6src,7,8,8c,9a,9b,9c,L,dl,10,12,13}"
HOLD="${SPIKE_HOLD:-75}"

spike_deps
[[ "${SPIKE_KEEP:-0}" == 1 ]] || : >"$SIM_OUT/results.jsonl"
if [[ "$SPIKE_TLS_PORT" != 443 ]]; then echo "ios-sim.sh needs the rig on :443" >&2; exit 2; fi
spike_server_up
spike_proxies_up

# A dedicated device, so the spike never disturbs simulators other sessions use.
# ios-sim.sh --delete-device removes it afterwards.
SIM_NAME="${SPIKE_SIM_NAME:-pf-spike-iphone}"
UDID="${SPIKE_SIM_UDID:-$(xcrun simctl list devices -j | node -e '
  const j = JSON.parse(require("fs").readFileSync(0, "utf8"));
  const d = (j.devices[process.argv[1]] || []).find((d) => d.name === process.argv[2]);
  if (d) console.log(d.udid);' "$RUNTIME" "$SIM_NAME")}"
if [[ -z "$UDID" ]]; then
  UDID="$(xcrun simctl create "$SIM_NAME" "${SPIKE_SIM_TYPE:-com.apple.CoreSimulator.SimDeviceType.iPhone-18-Pro}" "$RUNTIME")"
fi
DEVICE_NAME="$(xcrun simctl list devices -j | node -e '
  const j = JSON.parse(require("fs").readFileSync(0, "utf8"));
  for (const ds of Object.values(j.devices)) for (const d of ds) if (d.udid === process.argv[1]) console.log(d.name);' "$UDID")"
echo "ios-sim: device $DEVICE_NAME ($UDID) on $RUNTIME"
xcrun simctl boot "$UDID" 2>/dev/null || true
xcrun simctl bootstatus "$UDID" -b >/dev/null
# add-root-cert can hang when the CA is already in the keychain, so do it once per device and CA.
CA_MARK="$SIM_OUT/.ca-added-$UDID-$(openssl x509 -in "$SPIKE_OUT/certs/ca.pem" -noout -fingerprint -sha256 | tr -dc 'A-F0-9' | cut -c1-16)"
if [[ ! -f "$CA_MARK" ]]; then
  xcrun simctl keychain "$UDID" add-root-cert "$SPIKE_OUT/certs/ca.pem"
  touch "$CA_MARK"
fi

RUNTIME_NAME="$(xcrun simctl list runtimes -j | node -e '
  const j = JSON.parse(require("fs").readFileSync(0, "utf8"));
  const r = j.runtimes.find((r) => r.identifier === process.argv[1]);
  console.log(r ? `${r.name} (${r.buildversion})` : process.argv[1]);' "$RUNTIME")"
node -e '
  const [file, device, runtime, mac] = process.argv.slice(1);
  require("fs").writeFileSync(file, JSON.stringify({ lane: "ios-sim", browser: "Mobile Safari (WebKit)",
    version: "from UA", os: runtime + " simulator on macOS " + mac, device }, null, 2));' \
  "$SIM_OUT/lane.json" "$DEVICE_NAME" "$RUNTIME_NAME" "$(sw_vers -productVersion)"

proxies=1
[[ "${SPIKE_PROXIES:-1}" == 0 ]] && proxies=0
IFS=',' read -r -a items <<<"$ITEMS"
for item in "${items[@]}"; do
  run="sim-$item-$(date +%s)"
  url="https://spike.localtest.me/run?lane=ios-sim&items=$item&run=$run&wa=sim&actwait=3&hold=$HOLD&proxies=$proxies"
  limit=90
  [[ "$item" == 12 ]] && limit=$((HOLD + 90))
  echo "ios-sim: item $item"
  # openurl can block until Safari finishes loading (or time out on a cold Safari), so detach it;
  # completion is detected through the collector instead.
  (xcrun simctl openurl "$UDID" "$url" >/dev/null 2>&1 &)
  shot=0
  for ((t = 0; t < limit; t++)); do
    if curl -s "http://127.0.0.1:$SPIKE_HTTP_PORT/_spike/done?run=$run" | grep -q '"done":true'; then break; fi
    # Mid-flight evidence for the top-level items (wrapper flow, passkey sheet).
    if [[ $t == 6 && ( "$item" == 10 || "$item" == 13 ) ]]; then
      xcrun simctl io "$UDID" screenshot "$SIM_OUT/$item-midway.png" >/dev/null 2>&1 && shot=1
    fi
    sleep 1
  done
  if ((t >= limit)); then echo "ios-sim: item $item did not report done within ${limit}s" | tee -a "$SIM_OUT/timeouts.log"; fi
  sleep 1
  xcrun simctl io "$UDID" screenshot "$SIM_OUT/$item.png" >/dev/null 2>&1 || true
done
grep -c '"event":"spike.result"' "$SIM_OUT/results.jsonl" | xargs echo "ios-sim: result lines:"
node "$HERE/render.mjs" || true
