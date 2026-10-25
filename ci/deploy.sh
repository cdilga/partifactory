#!/usr/bin/env bash
# Reuse this workflow's exact release assets; observe deploy and a second-daemon restore.
source "$(dirname "$0")/lib.sh"
h="$repo/tests/e2e/harness"
export PF_RELEASE_ASSETS="${PF_RELEASE_ASSETS:-$repo/target/release-assets}"
export PF_E2E_RUN="${PF_E2E_RUN:-deploy-$(date +%Y%m%d-%H%M%S)}"
export PF_RUN_DIR="$repo/target/e2e-runs/$PF_E2E_RUN"
export PF_HARNESS_RELEASE=1 PF_HARNESS_MODE=container PF_E2E_RENDER=1
export PF_BIN="${PF_BIN:-$repo/target/debug/partifactory}"
export HOST_UID="$(id -u)" HOST_GID="$(id -g)"
logs="$repo/target/e2e-logs/$PF_E2E_RUN"
mkdir -p "$PF_RUN_DIR" "$logs/private" "$logs/public"
chmod 700 "$PF_RUN_DIR" "$logs/private"
private="$logs/private/driver.log"
touch "$private"; chmod 600 "$private"
registry=""; daemon=""; daemon_volume=""; harness_owned=0
own_images=()
cleanup() {
  result=$?
  if [ -n "$daemon" ]; then docker rm -fv "$daemon" >> "$private" 2>&1 || true; fi
  if [ -n "$daemon_volume" ]; then docker volume rm "$daemon_volume" >> "$private" 2>&1 || true; fi
  if [ "$harness_owned" = 1 ]; then "$h/down.sh" "$PF_RUN_DIR" >> "$private" 2>&1 || true; fi
  if [ -n "$registry" ]; then docker rm -fv "$registry" >> "$private" 2>&1 || true; fi
  for owned in ${own_images[@]+"${own_images[@]}"}; do docker image rm "$owned" >> "$private" 2>&1 || true; done
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# CI owns a new isolated source daemon. Local callers may explicitly use an existing
# daemon, but the resulting evidence records that this is not the fresh-context gate.
source_id="$(docker info --format '{{.ID}}')"
fresh=1
if [ -n "$(docker ps -aq)$(docker image ls -aq)$(docker volume ls -q)" ]; then fresh=0; fi
if [ "${PF_DEPLOY_REQUIRE_FRESH_SOURCE:-1}" = 1 ] && [ "$fresh" != 1 ]; then
  echo 'deploy gate: use a fresh caller-owned Docker daemon/context (or explicitly record a non-fresh local observation)' >&2
  exit 2
fi
if [ -n "$(docker ps -aq --filter label=com.docker.compose.project=pf-harness)" ]; then
  echo 'deploy gate: the shared harness is already owned by another run' >&2
  exit 2
fi
python3 - "$PF_RELEASE_ASSETS" "$logs/public/release.json" "$fresh" <<'PY'
import hashlib,json,pathlib,subprocess,sys
assets=pathlib.Path(sys.argv[1]); release=json.loads((assets/'RELEASE.json').read_text())
for line in (assets/'SHA256SUMS').read_text().splitlines():
    digest,name=line.split('  ',1)
    assert pathlib.Path(name).name==name and hashlib.file_digest((assets/name).open('rb'),'sha256').hexdigest()==digest
assert (assets/'digest.txt').read_text().strip()==release['image']
assert subprocess.run(['git','merge-base','--is-ancestor',release['commit'],'HEAD']).returncode==0
# main.rs excludes the entire dev module behind cfg(feature="dev"). Every
# release-compiled runtime source remains checked, including that cfg itself.
assert subprocess.run(['git','diff','--quiet',release['commit'],'HEAD','--','crates','Cargo.toml','Cargo.lock','Dockerfile','ui','vendor','docs/agent-contract.md','rust-toolchain.toml',':!crates/partifactory/src/dev.rs']).returncode==0,'release runtime source differs'
pathlib.Path(sys.argv[2]).write_text(json.dumps({**release,'gate_commit':subprocess.check_output(['git','rev-parse','HEAD']).decode().strip(),'fresh_source_daemon':sys.argv[3]=='1'},indent=2)+'\n')
PY
# Exercise the actual release CLI archive, not a rebuilt or placeholder binary.
# Keep both the installer and its installations in this run's private directory.
binstall_dir="$logs/private/binstall-tools"
mkdir -p "$binstall_dir"
curl -fLsS --proto '=https' --tlsv1.2 --connect-timeout 10 --max-time 120 \
  https://github.com/cargo-bins/cargo-binstall/releases/download/v1.26.1/cargo-binstall-x86_64-unknown-linux-musl.tgz \
  -o "$binstall_dir/cargo-binstall.tgz"
printf '%s  %s\n' 43d379fef8cb450961a14ff113e7bce7b5e1e52db709c776d186f928d8a20a40 \
  "$binstall_dir/cargo-binstall.tgz" | sha256sum --check
tar xzf "$binstall_dir/cargo-binstall.tgz" -C "$binstall_dir" cargo-binstall
python3 tests/deploy/binstall.py --binstall "$binstall_dir/cargo-binstall" \
  --assets "$PF_RELEASE_ASSETS" --output "$logs/private/binstall" \
  --report "$logs/public/binstall.json"
if [ "${PF_E2E_SKIP_BUILD:-0}" != 1 ]; then cargo build -j2 -p partifactory --features "$PF_FEATURES"; fi
registry="$(docker create --name "pf-deploy-registry-$PF_E2E_RUN" --label pf.harness=1 --memory=256m \
  --tmpfs /var/lib/registry:rw,nosuid,nodev,size=128m -p 127.0.0.1:25000:5000 \
  registry:3.0.0@sha256:6c5666b861f3505b116bb9aa9b25175e71210414bd010d92035ff64018f9457e 2>> "$private")"
docker start "$registry" >> "$private" 2>&1
for attempt in $(seq 1 30); do
  if curl -fsS http://127.0.0.1:25000/v2/ >/dev/null 2>&1; then break; fi
  sleep 1
done
expected_image="$(python3 - "$PF_RELEASE_ASSETS/RELEASE.json" <<'PY'
import json,pathlib,sys
print('127.0.0.1:25000/partifactory@'+json.loads(pathlib.Path(sys.argv[1]).read_text())['image'].rsplit('@',1)[1])
PY
)"
if ! docker image inspect "$expected_image" >/dev/null 2>&1; then own_images+=("$expected_image"); fi
python3 scripts/load-release.py --assets "$PF_RELEASE_ASSETS" --registry http://127.0.0.1:25000 \
  --repository partifactory --tag gate --pull --output "$PF_RUN_DIR/release-image" >> "$private" 2>&1
export PF_HARNESS_IMAGE="$(cat "$PF_RUN_DIR/release-image")"
[ "$PF_HARNESS_IMAGE" = "$expected_image" ] || { echo 'release image reference changed' >&2; exit 1; }
tests/deploy/image_checks.sh "$PF_HARNESS_IMAGE" | tee "$logs/public/image-checks.txt"
(cd tests/e2e && npm ci --no-audit --no-fund --silent)
"$h/build-browsers.sh" >> "$private" 2>&1
harness_owned=1
"$h/up.sh" >> "$private" 2>&1
set -a; source "$PF_RUN_DIR/env"; set +a
for attempt in $(seq 1 12); do
  status="$(curl -sS --max-time 15 --cacert "$PF_CA" --resolve "parts.pf.test:$PF_HTTPS_PORT:127.0.0.1" \
    -o /dev/null -w '%{http_code}' "https://parts.pf.test:$PF_HTTPS_PORT/signin?return=/" || true)"
  [ "$status" != 303 ] || break
  sleep 1
done
[ "$status" = 303 ] || { echo 'deploy gate: real OIDC discovery unavailable' >&2; exit 1; }
"$h/compose.sh" --profile browsers run --rm browsers harness/release-setup.spec.ts --project chromium-desktop >> "$private" 2>&1
"$h/compose.sh" --profile container --profile proxy --profile render config --format json > "$PF_RUN_DIR/deploy-compose.raw.json"
python3 - "$PF_RUN_DIR" "$logs" <<'PY'
import json,pathlib,re,sys
run,logs=map(pathlib.Path,sys.argv[1:]); doc=json.loads((run/'deploy-compose.raw.json').read_text())
runtime={'partifactory','proxy','partifactory-render','dex','dex2','security-canary'}
doc['services']={k:v for k,v in doc['services'].items() if k in runtime}
for service in doc['services'].values():service.pop('profiles',None)
for network in doc['services']['partifactory']['networks'].values():
    if isinstance(network,dict):network.pop('ipv4_address',None)
p=run/'deploy-compose.json';p.write_text(json.dumps(doc));p.chmod(0o600)
site=(run/'site.toml').read_text();assert 'trusted_proxies = ["172.30.77.10"]' in site
(run/'drill-site.toml').write_text(site.replace('trusted_proxies = ["172.30.77.10"]','trusted_proxies = ["172.31.87.10"]'))
(run/'drill-site.toml').chmod(0o600)
link=json.loads((run/'device-pass/guest-link.json').read_text())['url']
(run/'public-part-url').write_text(link);(run/'public-part-url').chmod(0o600)
PY
public_part="$(cat "$PF_RUN_DIR/public-part-url")"
connection=(--compose "$PF_RUN_DIR/deploy-compose.json" --base-url https://parts.pf.test \
  --part-url "$public_part" --part-marker 'Serving fixture' --ca-cert "$PF_CA" \
  --connect-to "127.0.0.1:$PF_HTTPS_PORT" --binary "$PF_BIN")
tests/deploy/compose_smoke.sh "${connection[@]}" --deploy-image "$PF_HARNESS_IMAGE" \
  --evidence-dir "$logs/private/smoke" | tee "$logs/public/smoke.txt"
cp "$logs/private/smoke/smoke.json" "$logs/private/smoke/log-limits.json" "$logs/public/"
docker compose -f "$PF_RUN_DIR/deploy-compose.json" stop proxy partifactory-render >> "$private" 2>&1
tests/deploy/log_limits.sh --compose "$PF_RUN_DIR/deploy-compose.json" --without-proxy \
  --evidence-dir "$logs/private/logs-app-only" | tee "$logs/public/logs-app-only.txt"
cp "$logs/private/logs-app-only/log-limits.json" "$logs/public/log-limits-app-only.json"
docker compose -f "$PF_RUN_DIR/deploy-compose.json" up -d --wait proxy partifactory-render >> "$private" 2>&1
PF_RELEASE_ASSETS="$PF_RELEASE_ASSETS" tests/deploy/fault-image.sh "$PF_HARNESS_IMAGE" \
  busybox:1.37.0-musl@sha256:dc88b80842580654294472735332ec7c7789a4010ec9a68b69f9b88c19abede6 \
  localhost:25000/partifactory-deploy-fixture > "$PF_RUN_DIR/fault-images" 2>> "$private"
bad="$(sed -n 's/^immediate=//p' "$PF_RUN_DIR/fault-images")"
delayed="$(sed -n 's/^delayed=//p' "$PF_RUN_DIR/fault-images")"
own_images+=("$bad" "$delayed")
tests/deploy/rollback.sh "${connection[@]}" --good-image "$PF_HARNESS_IMAGE" --bad-image "$bad" --delayed-image "$delayed" \
  --evidence-dir "$logs/private/rollback" | tee "$logs/public/rollback.txt"
cp "$logs/private/rollback/rollback.json" "$logs/public/"

# Refresh the old-cap refusal input after all source deploys, then take the newest
# complete release-CLI export. The destination import must invalidate this cap.
python3 - "$PF_RUN_DIR" "$PF_CA" "$PF_HTTPS_PORT" "$logs/public" <<'PY'
import json,pathlib,sys
sys.path.insert(0,'tests/security');from run import Client
run=pathlib.Path(sys.argv[1]);link=json.loads((run/'device-pass/guest-link.json').read_text())['url'];origin=link.split('/l/',1)[0]
client=Client(sys.argv[2],'127.0.0.1:'+sys.argv[3]);status,_,body=client.request(link+'/go','POST',{'Origin':origin,'Content-Length':'0'});assert status==200
old=origin+'/c/'+json.loads(body)['cap']+'/index.html';assert client.request(old)[0]==200
config={'public_link':link,'old_cap':old,'marker':'Serving fixture','idp_connect_to':'127.0.0.1:'+sys.argv[3],
        'metrics_token_file':str(run/'secrets/metrics_token'),'secret_files':[str(p) for p in (run/'secrets').iterdir() if p.name!='api_token'],
        'base_url':'https://parts.pf.test','ca':sys.argv[2],'connect_to':'127.0.0.1:'+sys.argv[3],
        'ops_url':'http://127.0.0.1:19090','public_out':sys.argv[4]}
p=run/'drill-input.json';p.write_text(json.dumps(config));p.chmod(0o600)
PY
docker compose -f "$PF_RUN_DIR/deploy-compose.json" exec -T partifactory \
  /partifactory export --include-insights --output /data/exports/deploy-gate.tar.zst >> "$private" 2>&1
archive="$(python3 - "$PF_RUN_DIR/data/exports" <<'PY'
import pathlib,sys
print(max(pathlib.Path(sys.argv[1]).glob('*.tar.zst'),key=lambda p:p.stat().st_mtime_ns))
PY
)"
volume_name="pf-drill-data-$PF_E2E_RUN"
if docker volume inspect "$volume_name" >/dev/null 2>&1; then echo 'drill volume name is already owned' >&2; exit 1; fi
docker volume create --label pf.harness=1 --driver local --opt type=tmpfs --opt device=tmpfs \
  --opt o=size=2g,mode=700 "$volume_name" >> "$private" 2>&1
daemon_volume="$volume_name"
# Privileged Docker defaults to a shared cgroup namespace. Keep the recovery
# daemon's children inside this bounded container's delegated subtree instead.
daemon="$(docker create --name "pf-drill-daemon-$PF_E2E_RUN" --label pf.harness=1 --privileged --cgroupns=private \
  --cpus=2 --memory=2g --memory-swap=2g --pids-limit=512 \
  --network pf-harness_ingress --mount "type=volume,src=$daemon_volume,dst=/var/lib/docker" \
  -e DOCKER_TLS_CERTDIR= -p 127.0.0.1:23751:2375 -p 127.0.0.1:19443:19443 \
  docker:28.5.2-dind@sha256:2a232a42256f70d78e3cc5d2b5d6b3276710a0de0596c145f627ecfae90282ac \
  --storage-driver=vfs 2>> "$private")"
docker start "$daemon" >> "$private" 2>&1
for attempt in $(seq 1 60); do
  if docker --host tcp://127.0.0.1:23751 info >/dev/null 2>&1; then break; fi
  sleep 1
done
drill=(--host tcp://127.0.0.1:23751 --source-daemon-id "$source_id" --assets "$PF_RELEASE_ASSETS" \
  --export "$archive" --site "$PF_RUN_DIR/drill-site.toml" --secrets-dir "$PF_RUN_DIR/secrets" \
  --tls-cert "$PF_RUN_DIR/certs/cert.pem" --tls-key "$PF_RUN_DIR/certs/key.pem" --ca-cert "$PF_CA" \
  --address 127.0.0.1 --https-port 19443 --report-dir "$PF_RUN_DIR/data/backup/drills" \
  --ingress-subnet 172.31.87.0/24 --proxy-address 172.31.87.10 --extra-host dex.pf.test=172.30.77.10)
if scripts/drill.sh "${drill[@]}" --e2e-command python3 -c 'raise SystemExit(19)' > "$PF_RUN_DIR/drill-failure.json" 2>> "$private"; then
  echo 'deploy gate: deliberate failing recovery test was accepted' >&2; exit 1
fi
python3 - "$PF_RUN_DIR/drill-failure.json" "$logs/public/drill-failure.json" <<'PY'
import json,pathlib,sys
report=json.loads(pathlib.Path(sys.argv[1]).read_text());assert not report['ok'] and report['failures']==['external_e2e_failed'],report
pathlib.Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
PY
python3 tests/deploy/drill-e2e.py source-report "$PF_RUN_DIR/drill-input.json" --expected 0 | tee "$logs/public/source-failure.txt"
scripts/drill.sh "${drill[@]}" --e2e-command python3 tests/deploy/drill-e2e.py observe "$PF_RUN_DIR/drill-input.json" \
  > "$PF_RUN_DIR/drill-success.json" 2>> "$private"
python3 - "$PF_RUN_DIR" "$logs/public" <<'PY'
import json,pathlib,shutil,sys
run,out=map(pathlib.Path,sys.argv[1:]);report=json.loads((run/'drill-success.json').read_text());assert report['ok'] and report['rto_s']<900 and not report['failures']
(out/'drill-success.json').write_text(json.dumps(report,indent=2)+'\n')
evidence=run/'data/backup/drills'/(str(report['at'])+'-evidence')
shutil.copyfile(evidence/'e2e.json',out/'drill-e2e.json');shutil.copyfile(evidence/'security/report.json',out/'drill-security.json')
PY
python3 tests/deploy/drill-e2e.py source-report "$PF_RUN_DIR/drill-input.json" --expected 1 | tee "$logs/public/source-success.txt"
echo 'event=deploy.check check=release_deploy_and_second_daemon_drill outcome=pass'
