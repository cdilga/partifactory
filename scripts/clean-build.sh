#!/usr/bin/env bash
# Free build space without disturbing concurrent builds (AGENTS.md "Disk hygiene").
# Dry run by default: lists what it would remove. `--apply` removes it.
#
# Age-based only, so nothing a running build is using goes away. Never runs `cargo clean`.
# Outside the repo it touches only Docker objects labelled pf.harness=1, the Docker build
# cache above 10 GB, and Playwright browser builds other than the pinned one.
#
#   PF_TARGET_DIR      target dir (default <repo>/target)
#   PF_CLEAN_DAYS      age window for build artefacts (default 2)
#   PF_CLEAN_NO_DOCKER skip the Docker steps when set (fixture tests)
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${PF_TARGET_DIR:-$repo/target}"
days="${PF_CLEAN_DAYS:-2}"
apply=0
[ "${1:-}" = "--apply" ] && apply=1

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
  for profile_dir in "$target"/*/ "$target"/*/*/; do
    for sub in deps build .fingerprint examples; do
      d="$profile_dir$sub"
      [ -d "$d" ] || continue
      while IFS= read -r -d '' f; do
        act "$f ($(size_of "$f"))" rm -rf -- "$f"
      done < <(find "$d" -mindepth 1 -maxdepth 1 -mtime "+$((days - 1))" -print0)
    done
  done

  # 3. Incremental dirs older than a day.
  while IFS= read -r -d '' d; do
    act "$d ($(size_of "$d"))" rm -rf -- "$d"
  done < <(find "$target" -mindepth 3 -maxdepth 3 -type d -path '*/incremental/*' -mtime +0 -print0 2>/dev/null)

  # 4. Release and musl outputs whose gate is recorded (a gate run touches .pf-gate-recorded).
  for d in "$target/release" "$target"/*-musl; do
    if [ -d "$d" ] && [ -e "$d/.pf-gate-recorded" ]; then
      act "$d ($(size_of "$d")), gate recorded" rm -rf -- "$d"
    fi
  done

  # 5. Log dirs beyond the newest 5 runs.
  for logs in "$target/e2e-logs" "$target/test-logs"; do
    [ -d "$logs" ] || continue
    ls -1t "$logs" | tail -n +6 | while IFS= read -r run; do
      act "$logs/$run" rm -rf -- "${logs:?}/$run"
    done
  done
fi

# 6. Docker: harness objects and the build cache.
if [ -z "${PF_CLEAN_NO_DOCKER:-}" ] && command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  ids="$(docker ps -aq --filter label=pf.harness=1)"
  [ -n "$ids" ] && act "harness containers: $(echo "$ids" | wc -l | tr -d ' ')" docker rm -f $ids >/dev/null
  vols="$(docker volume ls -q --filter label=pf.harness=1)"
  [ -n "$vols" ] && act "harness volumes: $(echo "$vols" | wc -l | tr -d ' ')" docker volume rm $vols >/dev/null
  imgs="$(docker images -q --filter label=pf.harness=1 | sort -u)"
  [ -n "$imgs" ] && act "harness images: $(echo "$imgs" | wc -l | tr -d ' ')" docker rmi -f $imgs >/dev/null
  act "docker build cache above 10GB" docker builder prune -f --keep-storage 10GB >/dev/null
fi

# 7. Playwright browsers other than the pinned build (tests/e2e/playwright-version pins it).
pin_file="$repo/tests/e2e/playwright-browsers"
pw_cache="${PLAYWRIGHT_BROWSERS_PATH:-$HOME/Library/Caches/ms-playwright}"
[ -d "$pw_cache" ] || pw_cache="$HOME/.cache/ms-playwright"
if [ -f "$pin_file" ] && [ -d "$pw_cache" ]; then
  for b in "$pw_cache"/*/; do
    name="$(basename "$b")"
    grep -qx "$name" "$pin_file" || act "playwright $name ($(size_of "$b"))" rm -rf -- "$b"
  done
fi

[ "$apply" -eq 1 ] || echo "(dry run: pass --apply to remove)"
