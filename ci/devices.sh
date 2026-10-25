#!/usr/bin/env bash
# Device gates share one actual release image and stack with the external security lane.
set -euo pipefail
exec "$(dirname "$0")/security-release.sh" "$@"
