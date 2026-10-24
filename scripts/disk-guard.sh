#!/usr/bin/env bash
# Refuse to build when the disk is nearly full or target/ has grown past its cap
# (AGENTS.md "Disk hygiene"). Every ci/*.sh and the e2e harness call this first.
#
#   PF_DISK_MIN_FREE_GB  free-space floor for the volume holding the repo (default 20)
#   PF_TARGET_MAX_GB     cap on target/ (default 30); decimals allowed
#   PF_TARGET_DIR        the target dir to measure (default <repo>/target)
#   PF_DISK_PATH         the path whose volume is checked (default <repo>)
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
min_free_gb="${PF_DISK_MIN_FREE_GB:-20}"
max_target_gb="${PF_TARGET_MAX_GB:-30}"
target="${PF_TARGET_DIR:-$repo/target}"
disk_path="${PF_DISK_PATH:-$repo}"

free_kb="$(df -Pk "$disk_path" | awk 'NR==2 {print $4}')"
target_kb=0
if [ -d "$target" ]; then
  target_kb="$(du -sk "$target" 2>/dev/null | awk '{print $1}')"
fi

gb() { awk -v kb="$1" 'BEGIN { printf "%.2f", kb / 1048576 }'; }
below() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a < b) }'; }
kb_of_gb() { awk -v g="$1" 'BEGIN { printf "%d", g * 1048576 }'; }

status=0
if below "$free_kb" "$(kb_of_gb "$min_free_gb")"; then
  echo "disk-guard: only $(gb "$free_kb") GiB free on $disk_path (floor ${min_free_gb} GiB)." >&2
  status=1
fi
if below "$(kb_of_gb "$max_target_gb")" "$target_kb"; then
  echo "disk-guard: $target is $(gb "$target_kb") GiB (cap ${max_target_gb} GiB)." >&2
  status=1
fi

if [ "$status" -ne 0 ]; then
  echo "disk-guard: refusing to build. Run scripts/clean-build.sh --apply first; it would free:" >&2
  PF_TARGET_DIR="$target" "$repo/scripts/clean-build.sh" >&2 || true
  echo "disk-guard: if that isn't enough, stop and tell the admin. Never delete outside the repo." >&2
  exit 1
fi
echo "disk-guard: ok ($(gb "$free_kb") GiB free, target $(gb "$target_kb") GiB)"
