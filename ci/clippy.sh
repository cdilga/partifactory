#!/usr/bin/env bash
# Lints: clippy on every target, warnings are errors.
source "$(dirname "$0")/lib.sh"
cargo clippy --workspace --all-targets --features "$PF_FEATURES" --profile ci -- -D warnings
