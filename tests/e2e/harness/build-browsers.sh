#!/usr/bin/env bash
# Only the image build inherits the job's DNS/network. Runtime browsers stay isolated.
# Compose Bake does not expose BuildKit's required network.host entitlement flag.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
task_builder="$(docker context show)"
docker buildx build --builder "$task_builder" --network host --allow network.host --load \
  --label pf.harness=1 --tag pf-harness-browsers:1.64.1 \
  --file "$repo/tests/e2e/harness/Dockerfile.browsers" "$repo/tests/e2e/harness"
