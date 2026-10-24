#!/usr/bin/env bash
# RustSec advisories against Cargo.lock.
source "$(dirname "$0")/lib.sh"
cargo audit --deny warnings
