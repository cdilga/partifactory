#!/usr/bin/env bash
# Tests: nextest over the workspace with the one feature set. NEXTEST_PROFILE=ci in CI keeps
# JUnit output (target/nextest/ci/junit.xml).
source "$(dirname "$0")/lib.sh"
if cargo nextest --version >/dev/null 2>&1; then
  cargo nextest run --workspace --features "$PF_FEATURES" \
    --profile "${NEXTEST_PROFILE:-default}"
else
  echo "test.sh: cargo-nextest not installed (scripts/bootstrap-dev.sh installs it); using cargo test" >&2
  cargo test --workspace --features "$PF_FEATURES"
fi
echo "test.sh: target/ is now $(du -sh target | awk '{print $1}')"
