#!/usr/bin/env bash
# CI-only Linux lane; the Mac observes the same test through tests/storage/gate.sh.
source "$(dirname "$0")/lib.sh"
export PF_BIN="${PF_BIN:-$repo/target/debug/partifactory}"
if [ "${PF_E2E_SKIP_BUILD:-0}" != 1 ]; then
  cargo build -p partifactory --features "$PF_FEATURES"
fi
(cd "$repo/tests/e2e" && npm ci --no-audit --no-fund --silent)
export PF_E2E_RUN="${PF_E2E_RUN:-pressure-$(date +%Y%m%d-%H%M%S)}"
export PF_RUN_DIR="$repo/target/e2e-runs/$PF_E2E_RUN"
tests/e2e/harness/build-browsers.sh
exec tests/storage/linux-gate.sh
