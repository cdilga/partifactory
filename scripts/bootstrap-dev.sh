#!/usr/bin/env bash
# Set up a laptop the way CI is set up: the cargo tools the ci/*.sh scripts use, and the hooks.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
for tool in cargo-nextest cargo-audit cargo-deny cargo-sweep; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    if command -v cargo-binstall >/dev/null 2>&1; then
      cargo binstall -y --locked "$tool"
    else
      cargo install --locked "$tool"
    fi
  fi
done
command -v gitleaks >/dev/null 2>&1 || echo "bootstrap-dev: install gitleaks (https://github.com/gitleaks/gitleaks)" >&2
"$here/scripts/install-hooks.sh"
