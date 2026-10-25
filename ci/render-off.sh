#!/usr/bin/env bash
# A fresh harness without the render profile observes optional-renderer publishing.
source "$(dirname "$0")/lib.sh"
h="$repo/tests/e2e/harness"
if [ "${PF_E2E_SKIP_BUILD:-0}" != 1 ]; then
  cargo build -p partifactory --features "$PF_FEATURES" -j 2
fi
export PF_BIN="${PF_BIN:-$repo/target/debug/partifactory}"
export PF_E2E_RENDER=0 PF_BUILD_GATE=1
export PF_E2E_RUN="render-off-$(date +%Y%m%d-%H%M%S)"
export PF_RUN_DIR="$repo/target/e2e-runs/$PF_E2E_RUN"
logs="$repo/target/e2e-logs/$PF_E2E_RUN"
mkdir -p "$logs"
teardown() {
  local rc=$?
  "$h/down.sh" "$PF_RUN_DIR" || true
  cp "$PF_RUN_DIR/server.jsonl" "$PF_RUN_DIR/server.stderr" "$logs/" 2>/dev/null || true
  echo "render-off.sh: exit=$rc; evidence=$logs"
  exit "$rc"
}
trap teardown EXIT
trap 'exit 130' INT TERM
"$h/up.sh" > "$logs/up.txt"
set -a
source "$PF_RUN_DIR/env"
set +a
running="$("$h/compose.sh" --profile render ps --services --status running)"
if printf '%s\n' "$running" | grep -q '^partifactory-render$'; then
  echo 'render-off.sh: renderer unexpectedly running' >&2; exit 1
fi
python3 "$repo/tests/build/running_gate.py" --case "${PF_RUNNING_GATE_CASE:-renderer-off}" --out "$logs"
