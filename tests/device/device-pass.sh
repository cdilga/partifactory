#!/usr/bin/env bash
# One isolated device matrix; temporary links never affect another browser run.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo"
owned=0
cleanup(){ if [[ "$owned" = 1 ]]; then tests/e2e/harness/down.sh "$PF_RUN_DIR"; fi; }
trap cleanup EXIT
if [[ -z "${PF_RUN_DIR:-}" ]]; then
  scripts/disk-guard.sh
  export PF_E2E_RUN="${PF_E2E_RUN:-device-$(date +%Y%m%d-%H%M%S)}"
  export PF_RUN_DIR="$repo/target/e2e-runs/$PF_E2E_RUN"
  export PF_BIN="${PF_BIN:-$repo/target/debug/partifactory}" PF_HARNESS_MODE=host
  owned=1
  tests/e2e/harness/up.sh >/dev/null
  set -a; source "$PF_RUN_DIR/env"; set +a
  python3 tests/e2e/harness/seed-part.py
else
  set -a; source "$PF_RUN_DIR/env"; set +a
fi
mkdir -p "$PF_RUN_DIR/device-pass"
rm -f "$PF_RUN_DIR/device-pass/guest-link.json"
tests/e2e/harness/compose.sh --profile browsers run --rm \
  -e PF_DEVICE_PASS=1 browsers device_pass.spec.ts \
  --project chromium-mobile --project webkit-mobile --project firefox-desktop
