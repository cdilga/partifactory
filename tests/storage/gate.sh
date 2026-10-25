#!/usr/bin/env bash
# Actual volume pressure, on a private disk image inside this checkout. macOS attaches this
# user-owned image without sudo or system configuration changes. No physical disk is selected.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo"
[[ "$(uname -s)" = Darwin ]] || { echo 'storage gate: run on the Mac (hdiutil disk-image lane)' >&2; exit 2; }
scripts/disk-guard.sh
export PF_E2E_RUN="${PF_E2E_RUN:-storage-$(date +%Y%m%d-%H%M%S)}"
export PF_RUN_DIR="$repo/target/e2e-runs/$PF_E2E_RUN"
export PF_HARNESS_MODE=host PF_E2E_RENDER=0
# The quota loop is the subject of this isolated lane; its requests must reach the writer.
export PF_INGRESS_RATE_LIMITS=false PF_UNSAFE_RATE_LIMITS=off
# Scheduled exports are exercised in their own gate; don't allocate a second complete copy
# while intentionally filling this tiny volume.
export PF_BACKUP_EXPORT_SCHEDULE=off PF_UNSAFE_NIGHTLY_EXPORT=off
mkdir -p "$PF_RUN_DIR/data" "$repo/target/e2e-logs/$PF_E2E_RUN"
image="$PF_RUN_DIR/storage.dmg"
attached=0
seed_pid=
cleanup(){
  result=$?
  if [[ -n "$seed_pid" ]]; then kill "$seed_pid" 2>/dev/null || true; fi
  "$repo/tests/e2e/harness/down.sh" "$PF_RUN_DIR" || true
  if [[ "$attached" = 1 ]]; then
    if hdiutil detach "$PF_RUN_DIR/data" -quiet; then attached=0; fi
  fi
  if [[ "$attached" = 0 ]]; then rm -f "$image"; fi
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
hdiutil create -size 128m -fs APFS -volname pf-storage-gate -type UDIF "$image" -quiet
hdiutil attach "$image" -mountpoint "$PF_RUN_DIR/data" -nobrowse -quiet
attached=1
"$repo/tests/e2e/harness/up.sh" >/dev/null
python3 tests/storage/seed-pressure.py "$PF_RUN_DIR/data" > "$repo/target/e2e-logs/$PF_E2E_RUN/stores.txt" 2>&1 &
seed_pid=$!
# Browser containers already have an exact host resolver and CA; no host /etc/hosts edits.
docker compose -f tests/e2e/harness/compose.yaml --profile browsers run --rm \
  -e PF_STORAGE_GATE=1 \
  -e "PF_STORAGE_GATE_DATA=/repo/target/e2e-runs/$PF_E2E_RUN/data" \
  browsers disk_full.spec.ts --project chromium-desktop \
  | tee "$repo/target/e2e-logs/$PF_E2E_RUN/storage.txt"
wait "$seed_pid"
seed_pid=
# Query the actual database after the real API loop, not an application-side counter.
python3 - "$PF_RUN_DIR/data/partifactory.db" <<'PY'
import sqlite3,sys
c=sqlite3.connect(sys.argv[1])
rows=c.execute("SELECT principal,kind,count(*) FROM quota_hits WHERE kind='space_parts' GROUP BY principal,kind").fetchall()
assert len(rows)==1 and rows[0][2]==1, rows
print('quota diagnostic: exactly one principal/kind/day row')
i=sqlite3.connect(str(__import__('pathlib').Path(sys.argv[1]).with_name('insights.db')))
for store in ['events','runtime_errors','audit','idempotency','quota_hits','build_logs']:
    row=i.execute('SELECT items,freed_bytes,size_after,cap_bytes,error FROM janitor_runs WHERE store=? ORDER BY id DESC LIMIT 1',(store,)).fetchone()
    assert row and row[0]>0 and row[1]>0 and row[2]<=row[3] and row[4] is None,(store,row)
    print('janitor',store,'items/freed/size/cap/error',row)
PY
