#!/usr/bin/env bash
# pf-twz8 spikes: build and test spike-compiler, spike-mcp and spike-ui in their
# own workspace (spikes/, excluded from the root one), using the shared target.
# Cleanup is handled by clean-build.sh; never cargo clean a shared checkout.
# ci/install-tools.sh installs the checksummed esbuild pin. Missing tooling is a
# failure, so the bundling tests can never silently pass without running.
# Delete this job together with spikes/.
source "$(dirname "$0")/lib.sh"
export PF_ESBUILD="${PF_ESBUILD:-$(command -v esbuild)}"
[ "$("$PF_ESBUILD" --version)" = 0.28.2 ] || { echo 'spikes: esbuild 0.28.2 is required' >&2; exit 2; }
cargo test -j2 --manifest-path spikes/Cargo.toml --target-dir "$repo/target" -p spike-compiler -p spike-mcp -p spike-ui
