#!/usr/bin/env bash
# The `spike` CI job (pf-6qma): the opaque-origin sandbox rig on the five Playwright projects
# (Chromium desktop and mobile emulation, WebKit desktop and mobile, Firefox). Keeps
# target/sandbox-spike/results.jsonl, results.md and screens/ as the job's artifacts.
# Needs Node 22+; installs only the three Playwright browsers the rig pins (once, cached by
# Playwright in ~/.cache/ms-playwright). Item 12's proxies run when Docker is available.
# The iOS Simulator lane (tests/sandbox-spike/ios-sim.sh) runs only on a Mac.
source "$(dirname "$0")/lib.sh"
cd "$repo/tests/sandbox-spike"
npm ci --no-audit --no-fund
if [[ "$(uname)" == Linux ]]; then
  npx playwright install --with-deps chromium webkit firefox
else
  npx playwright install chromium webkit firefox
fi
./run.sh "$@"
