#!/usr/bin/env bash
# Secret scanning over the git history with .gitleaks.toml (the pf_at_/pf_rt_/pf_pat_ shapes).
# `ci/gitleaks.sh <repo-dir>` scans another checkout (the self-test uses it).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
target="${1:-$here}"
exec gitleaks git --no-banner --redact --config "$here/.gitleaks.toml" "$target"
