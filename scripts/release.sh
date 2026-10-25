#!/usr/bin/env bash
# CI-independent release assets. Dry runs build the same bytes without publishing them.
set -euo pipefail
# A build can run while other agents edit this checkout. Bash otherwise reads
# a script incrementally and can combine old/new bytes after a long command.
# Execute one immutable in-memory snapshot, preserving this script's $0/path.
if [ "${PF_INTERNAL_RELEASE_SNAPSHOT:-}" != 1 ]; then
  export PF_INTERNAL_RELEASE_SNAPSHOT=1
  exec bash -c "$(cat "$0")" "$0" "$@"
fi
unset PF_INTERNAL_RELEASE_SNAPSHOT
cd "$(dirname "$0")/.."
scripts/disk-guard.sh
dry=0
tag=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run) dry=1; shift ;;
    --tag) tag="${2:?tag required}"; shift 2 ;;
    *) echo 'usage: scripts/release.sh [--dry-run] --tag VERSION' >&2; exit 2 ;;
  esac
done
tag="${tag:-$(git describe --tags --always)}"
case "$tag" in *[!a-zA-Z0-9._-]*|'') echo 'invalid release tag' >&2; exit 2 ;; esac
image="${PF_RELEASE_IMAGE:-registry.example.com/partifactory}"
if [ "$dry" = 0 ] && [ "$image" = registry.example.com/partifactory ]; then
  echo 'Set PF_RELEASE_IMAGE to the destination registry/repository' >&2; exit 2
fi
if [ "$dry" = 0 ]; then
  git diff-index --quiet HEAD -- || { echo 'Commit the release source before publishing' >&2; exit 2; }
  if [ -n "$(git ls-files --others --exclude-standard)" ]; then
    echo 'Commit or ignore untracked release source before publishing' >&2; exit 2
  fi
fi
out="$PWD/target/release-assets"
mkdir -p "$out"
export CARGO_INCREMENTAL=0 SOURCE_COMMIT="$(git rev-parse HEAD)"
# A stable epoch also keeps BuildKit WORKDIR/COPY metadata cacheable across commits.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-0}"
outputs=(--output "type=oci,dest=$out/partifactory-linux-amd64.oci.tar,rewrite-timestamp=true")
if [ "$dry" = 0 ]; then outputs+=(--output type=image,push=true); fi
# A single BuildKit cache, bounded after each build, including a failed one.
source scripts/oci-builder.sh
oci_builder_prepare
trap oci_builder_prune EXIT
docker buildx build --builder "$PF_OCI_BUILDER" "${PF_OCI_BUILD_ARGS[@]}" --platform linux/amd64 --provenance=false --sbom=false \
  --build-arg "SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH" \
  --label pf.harness=1 --label "org.opencontainers.image.revision=$(git rev-parse HEAD)" \
  --tag "$image:$tag" --metadata-file "$out/build.json" "${outputs[@]}" .
package_args=(--assets "$out" --image "$image" --tag "$tag")
if [ "$dry" = 0 ]; then package_args+=(--published); fi
python3 scripts/package-release.py "${package_args[@]}"
if [ "$dry" = 0 ]; then
  # The forge is optional plumbing. The artifact set above is always available to a laptop.
  python3 - "$out" "$tag" <<'PY'
import json, os, pathlib, sys, urllib.request, urllib.parse, uuid
root=pathlib.Path(sys.argv[1]); tag=sys.argv[2]
base=os.environ.get('PF_RELEASE_API','').rstrip('/')
token=os.environ.get('PF_RELEASE_TOKEN','')
if not base or not token:
    raise SystemExit('Image published; set PF_RELEASE_API (repository API URL) and PF_RELEASE_TOKEN to upload forge assets')
def request(path,data,mime):
    req=urllib.request.Request(base+path,data=data,headers={'Authorization':'token '+token,'Content-Type':mime,'User-Agent':'partifactory-release'})
    return json.load(urllib.request.urlopen(req,timeout=120))
release=request('/releases',json.dumps({'tag_name':tag,'name':tag,'draft':False,'body':'Digest-pinned image, offline recovery image, CLI and checksums.'}).encode(),'application/json')
for p in sorted(root.iterdir()):
    if not p.is_file() or p.name=='build.json': continue
    boundary='pf-'+uuid.uuid4().hex
    head=f'--{boundary}\r\nContent-Disposition: form-data; name="attachment"; filename="{p.name}"\r\nContent-Type: application/octet-stream\r\n\r\n'.encode()
    request(f'/releases/{release["id"]}/assets?name='+urllib.parse.quote(p.name),head+p.read_bytes()+f'\r\n--{boundary}--\r\n'.encode(),'multipart/form-data; boundary='+boundary)
print('Forge release assets uploaded')
PY
fi
