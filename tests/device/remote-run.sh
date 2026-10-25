#!/usr/bin/env bash
# Read actual run artifacts from an isolated remote harness through its tunneled Docker API.
# The caller owns the SSH tunnel, the source harness, and this reader's lifecycle.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
: "${PF_RUN_DIR:?requires the actual remote run at the same absolute path}"
[[ "${DOCKER_HOST:-}" =~ ^tcp://127\.0\.0\.1:[0-9]+$ ]] || {
  echo 'remote-run: requires the loopback SSH tunnel to the private source daemon' >&2;exit 2;
}
case "$PF_RUN_DIR" in
  "$repo"/target/e2e-runs/*) ;;
  *) echo 'remote-run: run must be inside this checkout target/e2e-runs' >&2;exit 2 ;;
esac
[[ "${PF_RUN_DIR#"$repo"/target/e2e-runs/}" =~ ^[a-zA-Z0-9_-]+$ ]] || exit 2
verify(){
  : "${PF_REMOTE_READER:?requires the test-owned reader container}"
  [[ "$(docker inspect --format '{{index .Config.Labels "pf.remote-run"}}' "$PF_REMOTE_READER")" = "$PF_RUN_DIR" ]] || {
    echo 'remote-run: reader belongs to a different run' >&2;exit 2;
  }
}
case "${1:-}" in
  start)
    docker run -d --name "pf-ios-reader-$(openssl rand -hex 6)" \
      --label pf.harness=1 --label "pf.remote-run=$PF_RUN_DIR" \
      --network none --read-only --cap-drop ALL --security-opt no-new-privileges \
      --memory 128m --pids-limit 32 --mount "type=bind,src=$PF_RUN_DIR,dst=/run/pf/run,readonly" \
      --entrypoint python3 pf-harness-browsers:1.64.1 -c 'import threading;threading.Event().wait()'
    ;;
  pull)
    verify
    relative="${2:?requires an allowed artifact path}"
    case "$relative" in
      env|compose.env|certs/ca.pem|part-fixture.json|device-pass/guest-link.json|device-pixel/guest.json|device-pixel/admin-report.json|device-pixel/admin-after-freeze.png) ;;
      *) echo 'remote-run: artifact is not in the explicit copy list' >&2;exit 2 ;;
    esac
    destination="$PF_RUN_DIR/$relative"
    mkdir -p "$(dirname "$destination")"
    chmod 700 "$PF_RUN_DIR" "$(dirname "$destination")"
    temporary="$(mktemp "$destination.XXXXXX")"
    trap 'rm -f "$temporary"' EXIT
    docker cp "$PF_REMOTE_READER:/run/pf/run/$relative" "$temporary" >/dev/null
    chmod 600 "$temporary"
    mv "$temporary" "$destination"
    ;;
  prepare)
    verify
    for relative in env compose.env certs/ca.pem;do "$0" pull "$relative";done
    # compose.env must retain the actual remote inner port for Docker operations.
    # Native Safari and curl reach that listener through canonical local TLS443.
    printf '\nPF_HTTPS_PORT=443\n' >> "$PF_RUN_DIR/env"
    ;;
  stop) verify;docker rm -f "$PF_REMOTE_READER" >/dev/null ;;
  *) echo 'usage: remote-run.sh start|prepare|pull <artifact>|stop' >&2;exit 2 ;;
esac
