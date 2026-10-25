#!/usr/bin/env bash
# Observe the real content listener and prelude in Mobile Safari without editing /etc/hosts.
# Docker publishes HTTPS on 443 so canonical part/blob origins resolve via localtest.me.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo"
scripts/disk-guard.sh
cargo build -p partifactory --features dev
export PF_APEX="${PF_APEX:-localtest.me}" PF_HTTPS_PORT="${PF_HTTPS_PORT:-443}"
export PF_BIN="$repo/target/debug/partifactory"
export PF_RUN_DIR="$repo/target/e2e-runs/ios-part-host-$(date +%Y%m%d-%H%M%S)"
trap 'tests/e2e/harness/down.sh "$PF_RUN_DIR"' EXIT
tests/e2e/harness/up.sh >/dev/null
set -a
source "$PF_RUN_DIR/env"
set +a
python3 tests/e2e/harness/seed-part.py
page="$(python3 - "$PF_RUN_DIR/part-fixture.json" <<'PY'
import json, sys, urllib.parse
url = json.load(open(sys.argv[1]))['device']['url']
print('/_dev/selftest?' + urllib.parse.urlencode({'frame': url}))
PY
)"
for spec in content_listener_sandbox prelude_storage; do
  tests/device/ios-sim.sh "$spec" "$page"
done
