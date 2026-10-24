#!/usr/bin/env bash
# The nightly advisory run on main: cargo audit and cargo deny's advisory check.
source "$(dirname "$0")/lib.sh"
cargo audit --deny warnings
cargo deny --all-features check advisories
