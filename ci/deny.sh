#!/usr/bin/env bash
# Licences, bans, sources and advisories (deny.toml).
source "$(dirname "$0")/lib.sh"
cargo deny --all-features check
