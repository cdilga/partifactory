#!/usr/bin/env bash
# Source from release/fixture scripts. Never changes the selected Docker context
# or global builder. The integrated docker driver cannot export OCI archives.
oci_builder_prepare(){
  PF_OCI_BUILDER=pf-release
  PF_OCI_BUILD_ARGS=(--progress=plain)
  local -a driver_args
  driver_args=(
    --driver-opt image=moby/buildkit:v0.33.1@sha256:cec9f139f45e93c5c69c60f8b07cfad9f43f4ef6b6a6cd917527fea5ff2e3dea
    --driver-opt cpu-period=100000 --driver-opt cpu-quota=200000
  )
  # The CI runner itself is a container. Its nested default bridge cannot
  # resolve through the runner's DNS reliably; keep this exception in CI only.
  # Local builders retain their current network, snapshots and cache keys.
  if [ "${CI:-}" = true ] || [ "${GITHUB_ACTIONS:-}" = true ] || [ "${GITEA_ACTIONS:-}" = true ]; then
    # Native snapshots on ZFS spend minutes fsyncing each Rust image layer.
    # Give this job's OCI worker its own bounded tmpfs and default overlayfs.
    # tmpfs pages count against the worker: the measured cold cook OOMs at 6 GiB.
    local state="buildx_buildkit_${PF_OCI_BUILDER}0_state" options
    if ! docker volume inspect "$state" >/dev/null 2>&1; then
      docker volume create --driver local --opt type=tmpfs --opt device=tmpfs \
        --opt o=size=6g,mode=0700 --label pf.harness=1 "$state" >/dev/null
    fi
    options="$(docker volume inspect --format '{{.Driver}}|{{index .Options "type"}}|{{index .Options "device"}}|{{index .Options "o"}}' "$state")"
    if [ "$options" != 'local|tmpfs|tmpfs|size=6g,mode=0700' ]; then
      echo 'release: CI builder state must be a bounded 6 GiB tmpfs; existing volume left unchanged' >&2
      return 2
    fi
    driver_args+=(--driver-opt memory=8g --driver-opt memory-swap=8g
      --driver-opt network=host --buildkitd-flags '--allow-insecure-entitlement network.host')
    PF_OCI_BUILD_ARGS+=(--network=host --allow=network.host)
  else
    driver_args+=(--driver-opt memory=6g --driver-opt memory-swap=6g)
  fi
  local inspected
  inspected="$(docker buildx inspect "$PF_OCI_BUILDER" 2>/dev/null)" || inspected=""
  if [ -z "$inspected" ]; then
    docker buildx create --name "$PF_OCI_BUILDER" --node "${PF_OCI_BUILDER}0" --driver docker-container \
      "${driver_args[@]}" \
      --buildkitd-config scripts/buildkitd.toml >/dev/null
  elif ! printf '%s\n' "$inspected" | grep -Eq '^Driver:[[:space:]]+docker-container$'; then
    echo 'release: pf-release exists with an incompatible builder driver' >&2
    return 2
  fi
}

oci_builder_prune(){
  # Buildx33 calls the old --keep-storage bound --max-used-space. This targets
  # the actual release builder rather than pruning the unrelated default cache.
  docker buildx prune --builder "$PF_OCI_BUILDER" --force --max-used-space 10GB >/dev/null 2>&1 || true
}
