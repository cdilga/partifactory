#!/usr/bin/env bash
# Real, bounded 128MiB filesystem pressure in the CI job's private mount namespace.
# No fake disk probe, loop device, host mount or physical device is used.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo"
[ "$(uname -s)" = Linux ] || { echo 'Linux storage gate needs Linux; use gate.sh on macOS' >&2; exit 2; }
scripts/disk-guard.sh
export PF_E2E_RUN="${PF_E2E_RUN:-pressure-$(date +%Y%m%d-%H%M%S)}"
export PF_RUN_DIR="$repo/target/e2e-runs/$PF_E2E_RUN"
export PF_BIN="${PF_BIN:-$repo/target/debug/partifactory}"
export PF_HARNESS_MODE=container PF_E2E_RENDER=0 PF_STORAGE_GATE=1
export HOST_UID="$(id -u)" HOST_GID="$(id -g)"
logs="$repo/target/e2e-logs/$PF_E2E_RUN"
mkdir -p "$PF_RUN_DIR/data" "$logs"
mounted=0
seed_pid=
cleanup() {
  result=$?
  if [ -n "$seed_pid" ]; then kill "$seed_pid" 2>/dev/null || true; wait "$seed_pid" 2>/dev/null || true; fi
  "$repo/tests/e2e/harness/down.sh" "$PF_RUN_DIR" || true
  if [ "$mounted" = 1 ]; then
    # Containers and the seed writer are gone before unmount; never lazily detach live data.
    umount "$PF_RUN_DIR/data" || result=1
  fi
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
mount -t tmpfs -o size=128m,nosuid,nodev pf-storage-gate "$PF_RUN_DIR/data"
mounted=1
python3 - "$PF_RUN_DIR/data" <<'PY'
import os,sys
v=os.statvfs(sys.argv[1]); budget=v.f_blocks*v.f_frsize
assert 120*1024*1024 <= budget <= 128*1024*1024, budget
print('pressure gate actual filesystem budget:',budget)
PY
"$repo/tests/e2e/harness/up.sh" >/dev/null
set -a
source "$PF_RUN_DIR/env"
set +a
python3 tests/e2e/harness/seed-part.py
python3 tests/storage/seed-pressure.py "$PF_RUN_DIR/data" > "$logs/stores.txt" 2>&1 &
seed_pid=$!
tests/e2e/harness/compose.sh --profile browsers run --rm \
  -e PF_STORAGE_GATE=1 -e PF_STORAGE_METRICS_URL=http://partifactory:9090/metrics \
  -e "PF_STORAGE_GATE_DATA=/repo/target/e2e-runs/$PF_E2E_RUN/data" \
  browsers disk_full.spec.ts --project chromium-desktop 2>&1 | tee "$logs/storage.txt"
wait "$seed_pid"
seed_pid=
python3 - "$PF_RUN_DIR/data/partifactory.db" <<'PY'
import pathlib,sqlite3,sys
c=sqlite3.connect(sys.argv[1])
rows=c.execute("SELECT principal,kind,count(*) FROM quota_hits WHERE kind='space_parts' GROUP BY principal,kind").fetchall()
assert len(rows)==1 and rows[0][2]==1,rows
print('quota diagnostic: exactly one principal/kind/day row')
i=sqlite3.connect(str(pathlib.Path(sys.argv[1]).with_name('insights.db')))
for store in ['events','runtime_errors','audit','idempotency','quota_hits','build_logs']:
    row=i.execute('SELECT items,freed_bytes,size_after,cap_bytes,error FROM janitor_runs WHERE store=? ORDER BY id DESC LIMIT 1',(store,)).fetchone()
    assert row and row[0]>0 and row[1]>0 and row[2]<=row[3] and row[4] is None,(store,row)
    print('janitor',store,'items/freed/size/cap/error',row)
PY
