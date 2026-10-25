#!/usr/bin/env bash
# CI-independent release assets. Dry runs build the same bytes without publishing them.
set -euo pipefail
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
export SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)"
outputs=(--output "type=oci,dest=$out/partifactory-linux-amd64.oci.tar,rewrite-timestamp=true")
if [ "$dry" = 0 ]; then outputs+=(--output type=image,push=true); fi
# A single BuildKit cache, bounded after each build, including a failed one.
source scripts/oci-builder.sh
oci_builder_prepare
trap oci_builder_prune EXIT
docker buildx build --builder "$PF_OCI_BUILDER" --platform linux/amd64 --provenance=false --sbom=false \
  --build-arg "SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH" \
  --label pf.harness=1 --label "org.opencontainers.image.revision=$(git rev-parse HEAD)" \
  --tag "$image:$tag" --metadata-file "$out/build.json" "${outputs[@]}" .
python3 - "$out" "$image" "$tag" "$dry" <<'PY'
import gzip, hashlib, io, json, os, pathlib, sys, tarfile
root=pathlib.Path(sys.argv[1]); image,tag,dry=sys.argv[2:]
metadata=json.loads((root/'build.json').read_text())
digest=metadata['containerimage.digest']
if not digest.startswith('sha256:') or len(digest)!=71: raise SystemExit('invalid image digest')
(root/'digest.txt').write_text(f'{image}@{digest}\n')
with tarfile.open(root/'partifactory-linux-amd64.oci.tar') as archive:
    def blob(d):
        raw=archive.extractfile('blobs/sha256/'+d.split(':',1)[1]).read()
        if hashlib.sha256(raw).hexdigest()!=d.split(':',1)[1]:
            raise SystemExit('OCI blob checksum mismatch')
        return raw
    def manifest_for(d):
        doc=json.loads(blob(d))
        if 'layers' in doc: return doc
        candidates=[x for x in doc.get('manifests',[]) if x.get('platform',{}).get('architecture')=='amd64' and x.get('platform',{}).get('os')=='linux']
        if len(candidates)!=1: raise SystemExit('OCI index must contain exactly one Linux amd64 image')
        return manifest_for(candidates[0]['digest'])
    manifest=manifest_for(digest)
    binary=None
    for layer in manifest['layers']:
        raw=blob(layer['digest'])
        with tarfile.open(fileobj=io.BytesIO(raw),mode='r:*') as files:
            for member in files:
                if member.name.lstrip('./')=='partifactory' and member.isfile():
                    binary=files.extractfile(member).read()
    if binary is None: raise SystemExit('image contains no binary')
with (root/'part-linux-amd64.tar.gz').open('wb') as out:
    with gzip.GzipFile(fileobj=out,mode='wb',filename='',mtime=0) as zipped:
        with tarfile.open(fileobj=zipped,mode='w') as asset:
            member=tarfile.TarInfo('part'); member.mode=0o755; member.size=len(binary); member.mtime=0
            asset.addfile(member,io.BytesIO(binary))
(root/'RELEASE.json').write_text(json.dumps({'tag':tag,'image':f'{image}@{digest}','published':dry=='0','commit':os.environ['SOURCE_COMMIT']},indent=2)+'\n')
files=sorted(p for p in root.iterdir() if p.is_file() and p.name not in ('SHA256SUMS','build.json'))
(root/'SHA256SUMS').write_text(''.join(f'{hashlib.file_digest(p.open("rb"),"sha256").hexdigest()}  {p.name}\n' for p in files))
print(f'Release assets: {root}; image {digest}; published={dry=="0"}')
PY
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
