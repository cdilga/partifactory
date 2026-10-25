#!/usr/bin/env bash
# Source from release/fixture scripts. Never changes the selected Docker context
# or global builder. The integrated docker driver cannot export OCI archives.
oci_builder_prepare(){
  PF_OCI_BUILDER=pf-release
  PF_OCI_BUILD_ARGS=(--progress=plain)
  local -a driver_args
  driver_args=(
    --driver-opt image=moby/buildkit:v0.33.1@sha256:cec9f139f45e93c5c69c60f8b07cfad9f43f4ef6b6a6cd917527fea5ff2e3dea
    --driver-opt memory=6g --driver-opt memory-swap=6g
    --driver-opt cpu-period=100000 --driver-opt cpu-quota=200000
  )
  # The CI runner itself is a container. Its nested default bridge cannot
  # resolve through the runner's DNS reliably; keep this exception in CI only.
  # Local builders retain their current network, snapshots and cache keys.
  if [ "${CI:-}" = true ] || [ "${GITHUB_ACTIONS:-}" = true ] || [ "${GITEA_ACTIONS:-}" = true ]; then
    driver_args+=(--driver-opt network=host --buildkitd-flags '--allow-insecure-entitlement network.host')
    PF_OCI_BUILD_ARGS+=(--network=host --allow=network.host)
  fi
  local inspected
  inspected="$(docker buildx inspect "$PF_OCI_BUILDER" 2>/dev/null)" || inspected=""
  if [ -z "$inspected" ]; then
    docker buildx create --name "$PF_OCI_BUILDER" --driver docker-container \
      "${driver_args[@]}" \
      --buildkitd-config scripts/buildkitd.toml >/dev/null
  elif ! printf '%s\n' "$inspected" | rg -q '^Driver:[[:space:]]+docker-container$'; then
    echo 'release: pf-release exists with an incompatible builder driver' >&2
    return 2
  fi
}

oci_builder_prune(){
  # Buildx33 calls the old --keep-storage bound --max-used-space. This targets
  # the actual release builder rather than pruning the unrelated default cache.
  docker buildx prune --builder "$PF_OCI_BUILDER" --force --max-used-space 10GB >/dev/null 2>&1 || true
}
