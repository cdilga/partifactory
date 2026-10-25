#!/usr/bin/env bash
# Use a disposable registry reachable by the target engine; both fixture images are digest-pinned.
set -euo pipefail
cd "$(dirname "$0")/../.."
good="${1:?usage: fault-image.sh GOOD_IMAGE@sha256:DIGEST STATIC_BUSYBOX@sha256:DIGEST REGISTRY/fixture}"
busybox="${2:?pinned static BusyBox image required}"
repository="${3:?disposable registry/repository required}"
for image in "$good" "$busybox"; do
  [[ "$image" =~ @sha256:[a-f0-9]{64}$ ]] || { echo 'fixture inputs must be pinned by digest' >&2; exit 2; }
done
scripts/disk-guard.sh
source scripts/oci-builder.sh
oci_builder_prepare
trap oci_builder_prune EXIT
mkdir -p target/deploy-fixtures
local_assets="${PF_RELEASE_ASSETS:-}"
context_args=()
base="$good"
if [ -n "$local_assets" ]; then
  layout="$(pwd)/target/deploy-fixtures/release-layout"
  digest="$(python3 scripts/load-release.py --assets "$local_assets" --layout "$layout")"
  [ "${good##*@}" = "$digest" ] || { echo 'fixture base differs from release assets' >&2; exit 2; }
  context_args=(--build-context "release-base=oci-layout://$layout@$digest")
  base=release-base
fi
for mode in immediate delayed; do
  tag="$repository:$mode"
  outputs=(--push)
  if [ -n "$local_assets" ]; then
    outputs=(--output "type=oci,dest=target/deploy-fixtures/$mode.oci.tar")
  fi
  docker buildx build --builder "$PF_OCI_BUILDER" "${PF_OCI_BUILD_ARGS[@]}" --platform linux/amd64 --provenance=false --sbom=false \
    ${context_args[@]+"${context_args[@]}"} \
    --build-arg "GOOD_IMAGE=$base" --build-arg "BUSYBOX_IMAGE=$busybox" \
    --build-arg "FAULT_MODE=$mode" --label pf.harness=1 \
    --metadata-file "target/deploy-fixtures/$mode.json" "${outputs[@]}" --tag "$tag" \
    -f tests/deploy/fault.Dockerfile tests/deploy
  if [ -n "$local_assets" ]; then
    # The builder's bridge cannot reach a host loopback registry. Transfer the
    # exact exported bytes from the host instead; no builder recreation needed.
    digest="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["containerimage.digest"])' "target/deploy-fixtures/$mode.json")"
    python3 scripts/load-release.py --archive "target/deploy-fixtures/$mode.oci.tar" \
      --digest "$digest" --registry "${PF_FIXTURE_REGISTRY_SCHEME:-http}://${repository%%/*}" \
      --repository "${repository#*/}" --tag "$mode" >&2
  fi
done
python3 - "$repository" <<'PY'
import json,pathlib,sys
repository=sys.argv[1]
for mode in ['immediate','delayed']:
    digest=json.loads(pathlib.Path(f'target/deploy-fixtures/{mode}.json').read_text())['containerimage.digest']
    assert digest.startswith('sha256:') and len(digest)==71
    print(f'{mode}={repository}@{digest}')
PY
