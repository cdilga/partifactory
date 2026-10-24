#!/usr/bin/env bash
# The core stays free of vendor adapters: no crate except the binary may depend on a
# pf-adapter-* crate (§5f). PF_CRATES_DIR points the check elsewhere (the self-test).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
crates="${PF_CRATES_DIR:-$here/crates}"
adapter_deps() { awk '/^\[/{deps = ($0 ~ /dependencies/)} deps && /^pf-adapter-/' "$1"; }
bad=0
for manifest in "$crates"/*/Cargo.toml; do
  name="$(basename "$(dirname "$manifest")")"
  case "$name" in pf-adapter-*|partifactory) continue ;; esac
  if [ -n "$(adapter_deps "$manifest")" ]; then
    echo "check-boundaries: $name depends on an adapter:" >&2
    adapter_deps "$manifest" >&2
    bad=1
  fi
done
[ "$bad" -eq 0 ] && echo "check-boundaries: ok (no core crate depends on an adapter)"
exit "$bad"
