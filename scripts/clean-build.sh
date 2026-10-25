#!/usr/bin/env bash
# Free build space without disturbing concurrent builds (AGENTS.md "Disk hygiene").
# Dry run by default: lists what it would remove. `--apply` removes it.
#
# Age-based only, so nothing a running build is using goes away. Never runs `cargo clean`.
# Docker cleanup is limited to stopped harness objects. Filesystem cleanup stays in this repo.
#
#   PF_TARGET_DIR      target dir (default <repo>/target)
#   PF_CLEAN_DAYS      age window for build artefacts (default 2)
#   PF_CLEAN_NO_DOCKER skip the Docker steps when set (fixture tests)
#   PF_CLEAN_INCREMENTAL drop completed incremental state while holding Cargo's locks
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${PF_TARGET_DIR:-${CARGO_TARGET_DIR:-$repo/target}}"
days="${PF_CLEAN_DAYS:-2}"
apply=0
[ "${1:-}" = "--apply" ] && apply=1

# Directory mtimes do not indicate whether Cargo is using a dependency. Hold Cargo's own
# profile locks throughout deletion, or defer immediately when a build has one open.
if [ "$apply" -eq 1 ] && [ -z "${PF_CLEAN_LOCKED:-}" ]; then
  exec python3 - "$repo" "$target" "$0" "$@" <<'PY'
import fcntl, os, pathlib, subprocess, sys
repo, target = (pathlib.Path(p).resolve() for p in sys.argv[1:3])
if not target.is_relative_to(repo):
    sys.exit("clean-build: target must be inside the repository")
locks = []
profiles = {target / "debug", target / "release"}
if target.exists():
    profiles.update(p.parent for p in target.rglob(".cargo-lock"))
try:
    for profile in sorted(profiles):
        profile.mkdir(parents=True, exist_ok=True)
        lock = (profile / ".cargo-lock").open("a+")
        locks.append(lock)
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:
    print("clean-build: deferred; Cargo is using this target directory")
    sys.exit(0)
sys.exit(subprocess.call(["bash", *sys.argv[3:]], env={**os.environ, "PF_CLEAN_LOCKED": "1"}))
PY
fi

act() { # act <description> <command...>
  local what="$1"; shift
  if [ "$apply" -eq 1 ]; then
    echo "remove: $what"
    "$@"
  else
    echo "would remove: $what"
  fi
}

size_of() { du -sh "$1" 2>/dev/null | awk '{print $1}'; }

if [ -d "$target" ]; then
  # 1. cargo sweep on the real workspace (age-based, by access time).
  if [ "$target" = "$repo/target" ] && command -v cargo-sweep >/dev/null 2>&1; then
    if [ "$apply" -eq 1 ]; then
      (cd "$repo" && cargo sweep --time "$days") || true
    else
      (cd "$repo" && cargo sweep --time "$days" --dry-run) || true
    fi
  fi

  # 2. The same rule for any target dir (fixtures, other profiles): top-level artefacts in
  # deps/, build/, .fingerprint/ and examples/ last modified before the window.
  # cargo-sweep tracks dependency use; a second mtime sweep would delete reused old files.
  if [ "$target" != "$repo/target" ] || ! command -v cargo-sweep >/dev/null 2>&1; then
  for profile_dir in "$target"/*/ "$target"/*/*/; do
    for sub in deps build .fingerprint examples; do
      d="$profile_dir$sub"
      [ -d "$d" ] || continue
      while IFS= read -r -d '' f; do
        act "$f ($(size_of "$f"))" rm -rf -- "$f"
      done < <(find "$d" -mindepth 1 -maxdepth 1 -mtime "+$((days - 1))" -print0)
    done
  done
  fi

  # 3. Incremental dirs older than a day.
  while IFS= read -r -d '' d; do
    act "$d ($(size_of "$d"))" rm -rf -- "$d"
  done < <(find "$target" -mindepth 3 -maxdepth 3 -type d -path '*/incremental/*' -mtime +0 -print0 2>/dev/null)
  if [ "${PF_CLEAN_INCREMENTAL:-0}" = 1 ]; then
    while IFS= read -r -d '' d; do
      act "$d ($(size_of "$d")), completed incremental state" rm -rf -- "$d"
    done < <(find "$target" -mindepth 3 -maxdepth 4 -type d -path '*/incremental/*' -prune -print0 2>/dev/null)
  fi

  # 4. Release and musl outputs whose gate is recorded (a gate run touches .pf-gate-recorded).
  for d in "$target/release" "$target"/*-musl; do
    if [ -d "$d" ] && [ -e "$d/.pf-gate-recorded" ]; then
      act "$d ($(size_of "$d")), gate recorded" rm -rf -- "$d"
    fi
  done

  # 5. Log dirs beyond the newest 5 runs.
  for logs in "$target/e2e-logs" "$target/test-logs"; do
    [ -d "$logs" ] || continue
    find "$logs" -mindepth 1 -maxdepth 1 -type d -name 'run-*' -print | sort -r | tail -n +6 | while IFS= read -r path; do
      run="$(basename "$path")"
      act "$logs/$run" rm -rf -- "${logs:?}/$run"
    done
  done
fi

# 6. Docker: harness objects and the build cache.
if [ -z "${PF_CLEAN_NO_DOCKER:-}" ] && command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  ids="$(docker ps -aq --filter label=pf.harness=1 --filter status=exited)"
  [ -n "$ids" ] && act "stopped harness containers: $(echo "$ids" | wc -l | tr -d ' ')" docker rm $ids >/dev/null
  vols="$(docker volume ls -q --filter label=pf.harness=1)"
  [ -n "$vols" ] && act "harness volumes: $(echo "$vols" | wc -l | tr -d ' ')" docker volume rm $vols >/dev/null
  imgs="$(docker images -q --filter label=pf.harness=1 --filter dangling=true | sort -u)"
  [ -n "$imgs" ] && act "unused harness images: $(echo "$imgs" | wc -l | tr -d ' ')" docker rmi $imgs >/dev/null
fi

[ "$apply" -eq 1 ] || echo "(dry run: pass --apply to remove)"
