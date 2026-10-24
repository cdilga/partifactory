# Sourced by every ci/*.sh: the disk guard first, then the shared build settings.
# One target/ and one feature set for every job, so dependencies build once (AGENTS.md).
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"
"$repo/scripts/disk-guard.sh"
export CARGO_INCREMENTAL=0
export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
# The one documented feature set for builds and tests.
PF_FEATURES="partifactory/dev"
