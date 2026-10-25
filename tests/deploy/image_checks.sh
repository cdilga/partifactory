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
container="$(docker create --platform linux/amd64 --label pf.harness=1 "$image" --help)"
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
docker run --rm --platform linux/amd64 --label pf.harness=1 --entrypoint /part "$image" --help > "$out/part-help.json"
docker run --rm --platform linux/amd64 --label pf.harness=1 "$image" config --help > "$out/config-help.txt"
python3 - "$out" "${PF_RELEASE_ASSETS:-}" <<'PY'
import json,pathlib,sys,tarfile
p=pathlib.Path(sys.argv[1]);help=(p/'help.txt').read_text()
for command in ['serve','export','import','migrate','healthcheck','deploy','config','admin']:
    assert any(line.startswith('  '+command+' ') for line in help.splitlines()),command
assert 'check' in (p/'config-help.txt').read_text()
ops=json.loads((p/'part-help.json').read_text())['ops']
assert 'part_create' in ops and 'part_link_for' in ops
if sys.argv[2]:
    with tarfile.open(pathlib.Path(sys.argv[2])/'part-linux-amd64.tar.gz') as archive:
        assert archive.extractfile('part').read()==(p/'partifactory').read_bytes(),'CLI asset differs from image binary'
(p/'cap').write_text('k'*32+'\n');(p/'cap').chmod(0o444)
PY
base_env=(-e PF_SITE_TOML=/nonexistent/site.toml -e PF_DOMAIN_APEX=pf.test \
  -e PF_SESSION_KEY=ssssssssssssssssssssssssssssssss -e PF_METRICS_TOKEN=mmmmmmmmmmmmmmmm)
docker run --rm --platform linux/amd64 --label pf.harness=1 "${base_env[@]}" \
  -e PF_CAP_HMAC_KEY=kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk "$image" config check > "$out/config-env.jsonl"
docker run --rm --platform linux/amd64 --label pf.harness=1 "${base_env[@]}" \
  --mount "type=bind,src=$out/cap,dst=/probe/cap,readonly" \
  -e PF_CAP_HMAC_KEY_FILE=/probe/cap "$image" config check > "$out/config-file.jsonl"
set +e
docker run --rm --platform linux/amd64 --label pf.harness=1 -e PF_SITE_TOML=/nonexistent/site.toml \
  "$image" config check > "$out/config-missing.jsonl" 2>&1
missing_code=$?
docker run --rm --platform linux/amd64 --label pf.harness=1 --entrypoint /part "$image" create \
  > "$out/part-invalid.stdout" 2> "$out/part-invalid.json"
part_code=$?
set -e
[ "$missing_code" = 2 ] && [ "$part_code" = 2 ] || { echo 'release refusal has wrong exit code' >&2; exit 1; }
python3 - "$out" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1])
for name in ['config-env.jsonl','config-file.jsonl']:
    raw=(p/name).read_text();rows=[json.loads(line) for line in raw.splitlines()]
    assert any(row.get('event')=='config.loaded' for row in rows),name
    assert 'k'*32 not in raw and 's'*32 not in raw and 'm'*16 not in raw,'secret in config logs'
rows=[json.loads(line) for line in (p/'config-missing.jsonl').read_text().splitlines()]
assert any(row.get('event')=='config.check' and row.get('outcome')=='missing_secret'
           and row.get('var')=='PF_SESSION_KEY' for row in rows)
assert json.loads((p/'part-invalid.json').read_text())['error']['code']=='invalid_input'
print('image: release server help, part argv0/help/error, missing-secret refusal and env/_FILE equivalence')
PY
set +e
docker run --rm --platform linux/amd64 --label pf.harness=1 -e PF_DEV_IDENTITY=/nonexistent/identity.toml \
  "$image" config check > "$out/release-refusal.jsonl" 2>&1
code=$?
set -e
[ "$code" = 2 ] || { echo "release fixture refusal exit: $code (expected 2)" >&2; exit 1; }
grep -q 'PF_DEV_IDENTITY' "$out/release-refusal.jsonl"
echo 'image: release has no dev subcommand and refuses dev identity configuration'
