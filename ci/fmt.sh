#!/usr/bin/env bash
# Formatting: rustfmt over the whole workspace, check only.
source "$(dirname "$0")/lib.sh"
cargo fmt --all --check
