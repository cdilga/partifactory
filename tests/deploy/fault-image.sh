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
for mode in immediate delayed; do
  tag="$repository:$mode"
  docker buildx build --builder "$PF_OCI_BUILDER" "${PF_OCI_BUILD_ARGS[@]}" --platform linux/amd64 --provenance=false --sbom=false \
    --build-arg "GOOD_IMAGE=$good" --build-arg "BUSYBOX_IMAGE=$busybox" \
    --build-arg "FAULT_MODE=$mode" --label pf.harness=1 \
    --metadata-file "target/deploy-fixtures/$mode.json" --push --tag "$tag" \
    -f tests/deploy/fault.Dockerfile tests/deploy
done
python3 - "$repository" <<'PY'
import json,pathlib,sys
repository=sys.argv[1]
for mode in ['immediate','delayed']:
    digest=json.loads(pathlib.Path(f'target/deploy-fixtures/{mode}.json').read_text())['containerimage.digest']
    assert digest.startswith('sha256:') and len(digest)==71
    print(f'{mode}={repository}@{digest}')
PY
