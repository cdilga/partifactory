#!/usr/bin/env bash
# No harness: combine observations from this same workflow's native and external gates.
source "$(dirname "$0")/lib.sh"
python3 tests/security/coverage.py
python3 tests/security/test_coverage.py
python3 tests/security/aggregate.py --artifact-root "${1:-target/security-inputs}" \
  --platform linux --require-complete --evidence-dir target/e2e-logs/security-aggregate
