#!/usr/bin/env bash
# The store-durability job (pf-lpvh gate): build the dev binary, then run the kill -9 durability
# script against `partifactory serve`. Needs jq.
source "$(dirname "$0")/lib.sh"
cargo build -p partifactory --features "$PF_FEATURES"
"$repo/tests/e2e/store/durability.sh" "${CARGO_TARGET_DIR:-$repo/target}/debug/partifactory"
