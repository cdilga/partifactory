#!/usr/bin/env bash
# pf-twz8 spikes: build and test spike-compiler, spike-mcp and spike-ui in their
# own workspace (spikes/, excluded from the root one), then give the space back.
# The target dir is pinned to spikes/target, never the shared cargo cache, so
# the cleanup below can't touch anyone else's artefacts.
# The esbuild-backed tests run when PF_ESBUILD (or esbuild on PATH) is set and
# are skipped otherwise. Delete this job together with spikes/.
source "$(dirname "$0")/lib.sh"
export CARGO_TARGET_DIR="$repo/spikes/target"
trap 'cargo clean --manifest-path spikes/Cargo.toml >/dev/null 2>&1 || true' EXIT
cargo test --manifest-path spikes/Cargo.toml -p spike-compiler -p spike-mcp -p spike-ui
