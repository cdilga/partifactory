#!/usr/bin/env bash
# pf-6qma sandbox spike, Playwright lanes: chromium-desktop, chromium-mobile, webkit-desktop,
# webkit-mobile, firefox-desktop. Starts the rig server (and the item 12 proxies in Docker when
# available), drives every item on every lane, then renders target/sandbox-spike/results.md.
#   ./run.sh                     everything (item 12 holds 75 s)
#   ./run.sh --lanes webkit-desktop --items 1,2 --hold 10
# Env: SPIKE_PROXIES=0 skips Docker; SPIKE_KEEP=1 appends to results.jsonl instead of starting fresh.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
trap spike_teardown EXIT

spike_deps
[[ "${SPIKE_KEEP:-0}" == 1 ]] || : >"$SPIKE_OUT/results.jsonl"
spike_server_up
spike_proxies_up
args=("$@")
if [[ "${SPIKE_PROXIES:-1}" == 0 ]]; then args+=(--proxies 0); fi
status=0
node "$HERE/run.mjs" "${args[@]}" || status=$?
lanes="chromium-desktop,chromium-mobile,webkit-desktop,webkit-mobile,firefox-desktop"
for ((i = 0; i < ${#args[@]}; i++)); do [[ "${args[$i]}" == --lanes ]] && lanes="${args[$((i + 1))]}"; done
node "$HERE/render.mjs" --lanes "$lanes" || status=$?
grep -h '"event":"spike.result"' "$SPIKE_OUT/results.jsonl" | wc -l | xargs echo "spike: result lines:"
exit $status
