#!/usr/bin/env bash
# pf-twz8 spikes: build and test spike-compiler, spike-mcp and spike-ui in their
# own workspace (spikes/, excluded from the root one), using the shared target.
# Cleanup is handled by clean-build.sh; never cargo clean a shared checkout.
# The esbuild-backed tests run when PF_ESBUILD (or esbuild on PATH) is set and
# are skipped otherwise. Delete this job together with spikes/.
source "$(dirname "$0")/lib.sh"
cargo test --manifest-path spikes/Cargo.toml --target-dir "$repo/target" -p spike-compiler -p spike-mcp -p spike-ui
