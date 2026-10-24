#!/usr/bin/env bash
# Shared by run.sh and ios-sim.sh: start/stop the rig server and the Docker proxies.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
export SPIKE_OUT="${SPIKE_OUT:-$REPO/target/sandbox-spike}"
mkdir -p "$SPIKE_OUT"
# macOS lets unprivileged processes bind :443 on the wildcard address; Linux CI uses 8443 and
# the rig's CONNECT proxy maps 443 to it for Playwright.
if [[ -z "${SPIKE_TLS_PORT:-}" ]]; then
  if [[ "$(uname)" == Darwin ]]; then SPIKE_TLS_PORT=443; else SPIKE_TLS_PORT=8443; fi
fi
export SPIKE_TLS_PORT
export SPIKE_HTTP_PORT="${SPIKE_HTTP_PORT:-8080}"
SERVER_PID=""
STARTED_PROXIES=0

spike_deps() {
  if [[ ! -d "$HERE/node_modules/playwright" ]]; then
    (cd "$HERE" && npm ci --no-audit --no-fund)
  fi
  "$HERE/certs.sh"
}

spike_server_up() {
  if curl -s --max-time 1 "http://127.0.0.1:$SPIKE_HTTP_PORT/_spike/ping" >/dev/null 2>&1; then
    echo "spike: reusing the rig server already running on :$SPIKE_HTTP_PORT"
    return
  fi
  node "$HERE/server.mjs" >"$SPIKE_OUT/server.log" 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 1 50); do
    if curl -s --max-time 1 "http://127.0.0.1:$SPIKE_HTTP_PORT/_spike/ping" >/dev/null 2>&1; then return; fi
    sleep 0.2
  done
  echo "spike: server did not start; see $SPIKE_OUT/server.log" >&2
  cat "$SPIKE_OUT/server.log" >&2
  return 1
}

spike_proxies_up() {
  if [[ "${SPIKE_PROXIES:-1}" == 1 ]] && command -v docker >/dev/null && docker info >/dev/null 2>&1; then
    "$HERE/proxies.sh" up && STARTED_PROXIES=1
  else
    echo "spike: Docker unavailable or SPIKE_PROXIES=0; item 12 runs direct only"
    export SPIKE_PROXIES=0
  fi
}

spike_teardown() {
  if [[ -n "$SERVER_PID" ]]; then kill "$SERVER_PID" 2>/dev/null || true; fi
  if [[ "$STARTED_PROXIES" == 1 ]]; then "$HERE/proxies.sh" down || true; fi
}
