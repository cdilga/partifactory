#!/usr/bin/env bash
# Inspect the actual release image without relying on a shell inside FROM scratch.
set -euo pipefail
cd "$(dirname "$0")/../.."
image="${1:?usage: tests/deploy/image_checks.sh IMAGE@sha256:DIGEST}"
[[ "$image" =~ @sha256:[a-f0-9]{64}$ ]] || { echo 'image must be pinned by digest' >&2; exit 2; }
out="$(mktemp -d "$PWD/target/image-checks.XXXXXX")"
container=""
cleanup(){
  code=$?
  if [ -n "$container" ]; then docker rm -f "$container" >/dev/null 2>&1 || true; fi
  rm -rf "$out"
  exit "$code"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
docker image inspect "$image" > "$out/inspect.json"
container="$(docker create --label pf.harness=1 "$image" --help)"
docker export "$container" > "$out/rootfs.tar"
python3 - "$out" <<'PY'
import json,pathlib,sys,tarfile
p=pathlib.Path(sys.argv[1]);info=json.loads((p/'inspect.json').read_text())[0]
assert info['Architecture']=='amd64' and info['Os']=='linux'
c=info['Config'];assert c['User']=='65532:65532', c['User']
assert c['Entrypoint']==['/partifactory']
assert c['Healthcheck']['Test']==['CMD','/partifactory','healthcheck']
with tarfile.open(p/'rootfs.tar') as archive:
    files={m.name.removeprefix('./'):m for m in archive}
    for name in ['partifactory','etc/ssl/certs/ca-certificates.crt','part']:
        assert name in files,name
    assert files['part'].issym() and files['part'].linkname=='/partifactory'
    assert not any(name in files for name in ['bin/sh','bin/bash','usr/bin/sh','usr/bin/env'])
    assert not any('.so' in pathlib.PurePosixPath(name).name for name in files),'unexpected dynamic library'
    binary=archive.extractfile(files['partifactory']).read()
    assert binary[:4]==b'\x7fELF'
    (p/'partifactory').write_bytes(binary)
print('image: Linux amd64, non-root, scratch filesystem, CA, CLI symlink, native healthcheck')
PY
docker start -a "$container" > "$out/help.txt"
if grep -Eq '^  dev[[:space:]]' "$out/help.txt"; then echo 'release image exposes dev subcommands' >&2; exit 1; fi
set +e
docker run --rm --label pf.harness=1 -e PF_DEV_IDENTITY=/nonexistent/identity.toml \
  "$image" config check > "$out/release-refusal.jsonl" 2>&1
code=$?
set -e
[ "$code" = 2 ] || { echo "release fixture refusal exit: $code (expected 2)" >&2; exit 1; }
grep -q 'PF_DEV_IDENTITY' "$out/release-refusal.jsonl"
echo 'image: release has no dev subcommand and refuses dev identity configuration'
