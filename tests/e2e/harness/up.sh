#!/usr/bin/env bash
# Bring the harness up for one run: per-run CA, Dex configs from the identity fixture, secrets
# as files, site.toml, Caddy + Dex in compose, and partifactory (host process on macOS, container
# on Linux). Prints the run dir; `down.sh <run dir>` tears it all down (ci/e2e.sh does both,
# with the teardown in a trap).
#
#   PF_HARNESS_MODE  host | container   (default: host on macOS, container elsewhere)
#   PF_BIN           the partifactory binary (default target/debug/partifactory, built
#                    with --features dev)
#   PF_HTTPS_PORT    host port for Caddy's 443 (default 18443; the browsers reach Caddy on
#                    443 inside the harness network, so their URLs carry no port)
#   PF_RUN_DIR       reuse/choose the run dir (default target/e2e-runs/<timestamp>)
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
h="$repo/tests/e2e/harness"

case "$(uname -s)" in Darwin) default_mode=host ;; *) default_mode=container ;; esac
mode="${PF_HARNESS_MODE:-$default_mode}"
export PF_HARNESS_RELEASE="${PF_HARNESS_RELEASE:-0}"
if [ "$PF_HARNESS_RELEASE" = 1 ]; then
  mode=container
  : "${PF_HARNESS_IMAGE:?Release harness requires an immutable release image}"
fi
export PF_HARNESS_MODE="$mode"
export PF_APEX="${PF_APEX:-pf.test}"
export PF_HTTPS_PORT="${PF_HTTPS_PORT:-18443}"
# A nested, caller-owned remote controller may publish through its own outer loopback port.
export PF_HARNESS_PUBLISH_BIND="${PF_HARNESS_PUBLISH_BIND:-127.0.0.1}"
export PF_BIN="${PF_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/partifactory}"
export PF_RUN_DIR="${PF_RUN_DIR:-$repo/target/e2e-runs/$(date +%Y%m%d-%H%M%S)}"
run="$PF_RUN_DIR"
mkdir -p "$run"/{certs,dex,secrets,data,render-public}
export PF_RENDER_GATEWAY="${PF_RENDER_GATEWAY:-partifactory:8443}"
cleanup_failed_start() {
  local rc=$?
  if [ "$rc" -ne 0 ]; then
    "$h/down.sh" "$run" >&2 || true
  fi
}
trap cleanup_failed_start EXIT
trap 'exit 130' INT TERM

if [ "$PF_HARNESS_RELEASE" != 1 ]; then
  [ -x "$PF_BIN" ] || { echo "up.sh: no binary at $PF_BIN (cargo build -p partifactory --features dev)" >&2; exit 1; }
fi

"$h/ca.sh" "$run/certs" >/dev/null
python3 - "$repo/tests/fixtures/identity.toml" "$run/identity.toml" "$PF_APEX" <<'PYFIX'
import pathlib, sys
pathlib.Path(sys.argv[2]).write_text(pathlib.Path(sys.argv[1]).read_text().replace("pf.test", sys.argv[3]))
PYFIX
python3 "$h/gen-dex.py" "$run/identity.toml" "$run/dex" >&2
admin_sub="$(python3 -c 'import sys,tomllib; print(next(u["sub"] for u in tomllib.load(open(sys.argv[1],"rb"))["users"] if u["username"]=="admin"))' "$run/identity.toml")"
chmod 644 "$run/dex/"*.yaml

for s in session_key cap_hmac_key metrics_token api_token; do
  openssl rand -hex 32 | tr -d '\n' > "$run/secrets/$s"
done
printf '%s' 'harness-client-secret' > "$run/secrets/idp_client_secret"
chmod 644 "$run/secrets/"*

if [ "$mode" = host ]; then
  export PF_RENDER_GATEWAY=render-host-relay:8443
  # Docker Desktop delivers container -> host.docker.internal connections from loopback.
  trusted='["127.0.0.1", "::1"]'
  export PF_UPSTREAM_WORKSPACE="host.docker.internal:18080"
  export PF_UPSTREAM_CONTENT="host.docker.internal:18081"
  mirror_addr="127.0.0.1:$PF_HTTPS_PORT"
  ops_url="http://127.0.0.1:19090"
else
  trusted='["172.30.77.10"]'
  mirror_addr="172.30.77.10:443"
  export PF_UPSTREAM_WORKSPACE="partifactory:8080"
  export PF_UPSTREAM_CONTENT="partifactory:8081"
  ops_url="http://127.0.0.1:19090"
fi

cat > "$run/site.toml" <<EOF
# Written by tests/e2e/harness/up.sh for one harness run.
[domain]
apex = "$PF_APEX"

[ingress]
client_ip_header = "X-Forwarded-For"
request_id_header = "X-Request-Id"
trusted_proxies = $trusted

[idp]
issuer = "https://dex.$PF_APEX/dex"
client_id = "partifactory"
groups_claim = "groups"
audience_group = "household"
household_group = "household"

[admin]
subject = "oidc:https://dex.$PF_APEX/dex#$admin_sub"


EOF

if [ "$PF_HARNESS_RELEASE" != 1 ]; then
  cat >> "$run/site.toml" <<EOF
[mirror.test_resolve]
"dex.$PF_APEX" = "$mirror_addr"
"dex2.$PF_APEX" = "$mirror_addr"
"cdnjs.cloudflare.com" = "$mirror_addr"
"cdn.jsdelivr.net" = "$mirror_addr"
"unpkg.com" = "$mirror_addr"
"esm.sh" = "$mirror_addr"
"fonts.googleapis.com" = "$mirror_addr"
"fonts.gstatic.com" = "$mirror_addr"
EOF
fi

if [ "${PF_BUILD_GATE:-0}" = 1 ] && [ "$PF_HARNESS_RELEASE" != 1 ]; then
  # Deliberately allow the literal address through the host policy, without a dev DNS
  # override: the running worker must reject its actual private address before dialing.
  printf '\n[mirror]\nallowlist = ["cdnjs.cloudflare.com", "cdn.jsdelivr.net", "unpkg.com", "esm.sh", "fonts.googleapis.com", "fonts.gstatic.com", "127.0.0.1"]\n' >> "$run/site.toml"
fi

if [ "${PF_STORAGE_GATE:-0}" = 1 ]; then
  # The disposable small-volume lane tests quotas and physical pressure, so requests must
  # reach those guards rather than the unrelated ingress limiter. Avoid allocating a second
  # complete scheduled export while intentionally filling a 128MiB test filesystem.
  python3 - "$run/site.toml" <<'PYSTORAGE'
import pathlib,sys
p=pathlib.Path(sys.argv[1])
s=p.read_text().replace('[ingress]\n','[ingress]\nrate_limits = false\n')
s+='\n[backup]\nexport_schedule = "off"\n\n[unsafe]\nrate_limits = "off"\nnightly_export = "off"\n'
p.write_text(s)
PYSTORAGE
fi

if [ "${PF_E2E_RENDER:-0}" = 1 ]; then
  printf '\n[render]\nenabled = true\n' >> "$run/site.toml"
fi

# The application keeps the shipped non-root user; only disposable bind directories
# are writable. Config and secret files remain read-only mounts.
if [ "$mode" = container ]; then chmod 777 "$run/data" "$run/render-public"; fi
export PF_HARNESS_DEV_IMAGE="${PF_HARNESS_DEV_IMAGE:-pf-harness-dev:$(git -C "$repo" rev-parse --short HEAD)}"
if [ "$mode" = container ] && [ "$PF_HARNESS_RELEASE" != 1 ]; then
  mkdir -p "$run/dev-runtime"
  cp "$PF_BIN" "$run/dev-runtime/partifactory"
  docker build --label pf.harness=1 --label "org.opencontainers.image.revision=$(git -C "$repo" rev-parse HEAD)" \
    -t "$PF_HARNESS_DEV_IMAGE" -f "$h/Dockerfile.dev" "$run/dev-runtime" >&2
  rm -rf "$run/dev-runtime"
fi
: > "$run/compose.env"
for name in PF_APEX PF_HARNESS_MODE PF_HARNESS_RELEASE PF_HARNESS_DEV_IMAGE PF_HARNESS_IMAGE PF_HTTPS_PORT PF_HARNESS_PUBLISH_BIND PF_RENDER_GATEWAY PF_UPSTREAM_WORKSPACE PF_UPSTREAM_CONTENT; do
  printf '%s=%q\n' "$name" "${!name:-}" >> "$run/compose.env"
done
chmod 600 "$run/compose.env"

if [ "$mode" = host ]; then
  PF_SITE_TOML="$run/site.toml" PF_DATA_DIR="$run/data" \
  PF_DEV_API_TOKEN_FILE="$run/secrets/api_token" PF_DEV_IDENTITY="$run/identity.toml" \
  PF_IDP_CLIENT_SECRET_FILE="$run/secrets/idp_client_secret" \
  PF_SESSION_KEY_FILE="$run/secrets/session_key" \
  PF_CAP_HMAC_KEY_FILE="$run/secrets/cap_hmac_key" \
  PF_METRICS_TOKEN_FILE="$run/secrets/metrics_token" \
  PF_RENDER_PUBLIC_DIR="$run/render-public" PF_LISTEN_RENDER=127.0.0.1:18445 \
  PF_RENDER_CDP=http://127.0.0.1:19222 PF_RENDER_CONTROL=http://127.0.0.1:19223 \
  PF_LISTEN_WORKSPACE=127.0.0.1:18080 PF_LISTEN_CONTENT=127.0.0.1:18081 PF_LISTEN_OPS=127.0.0.1:19090 \
  PF_LOG="${PF_LOG:-info}" PF_DEV_LOG_RING=20000 SSL_CERT_FILE="$run/certs/ca.pem" \
    nohup "$PF_BIN" serve > "$run/server.jsonl" 2> "$run/server.stderr" &
  echo $! > "$run/pf.pid"
  compose_profiles=()
else
  compose_profiles=(--profile container)
fi

touch "$run/server.jsonl"
"$h/compose.sh" "${compose_profiles[@]}" up -d --quiet-pull proxy dex dex2 \
  $([ "$mode" = container ] && echo partifactory) >&2
if [ "${PF_E2E_RENDER:-0}" = 1 ]; then
  render_services=(partifactory-render)
  if [ "$mode" = host ]; then render_services+=(render-host-relay); fi
  "$h/compose.sh" --profile render up -d --quiet-pull "${render_services[@]}" >&2
fi
if [ "$mode" = container ]; then
  # Follow the server's stdout into the run's JSONL, as host mode writes it directly.
  nohup "$h/compose.sh" --profile container logs -f --no-color --no-log-prefix partifactory \
    > "$run/server.jsonl" 2>/dev/null &
  echo $! > "$run/logs.pid"
fi

cat > "$run/env" <<EOF
PF_RUN_DIR=$run
PF_APEX=$PF_APEX
PF_HARNESS_MODE=$mode
PF_HARNESS_RELEASE=$PF_HARNESS_RELEASE
PF_HARNESS_IMAGE=${PF_HARNESS_IMAGE:-}
PF_HARNESS_DEV_IMAGE=$PF_HARNESS_DEV_IMAGE
PF_E2E_RENDER=${PF_E2E_RENDER:-0}
PF_HTTPS_PORT=$PF_HTTPS_PORT
PF_OPS_URL=$ops_url
PF_CA=$run/certs/ca.pem
PF_METRICS_TOKEN_FILE=$run/secrets/metrics_token
PF_SERVER_LOG=$run/server.jsonl
EOF

# Wait for the whole chain: browser-side TLS through Caddy to partifactory, and both Dex.
resolve=(--resolve "parts.$PF_APEX:$PF_HTTPS_PORT:127.0.0.1" --resolve "dex.$PF_APEX:$PF_HTTPS_PORT:127.0.0.1" --resolve "dex2.$PF_APEX:$PF_HTTPS_PORT:127.0.0.1")
for i in $(seq 1 60); do
  if curl -fsS --cacert "$run/certs/ca.pem" "${resolve[@]}" "https://parts.$PF_APEX:$PF_HTTPS_PORT/healthz" >/dev/null 2>&1 &&
     curl -fsS --cacert "$run/certs/ca.pem" "${resolve[@]}" "https://dex.$PF_APEX:$PF_HTTPS_PORT/dex/.well-known/openid-configuration" >/dev/null 2>&1 &&
     curl -fsS --cacert "$run/certs/ca.pem" "${resolve[@]}" "https://dex2.$PF_APEX:$PF_HTTPS_PORT/dex/.well-known/openid-configuration" >/dev/null 2>&1; then
    echo "up.sh: harness ready ($mode mode) in $run" >&2
    echo "$run"
    exit 0
  fi
  sleep 1
done
echo "up.sh: harness did not become ready; see $run/server.jsonl and docker compose logs" >&2
"$h/compose.sh" --profile container --profile render logs --no-color --tail 50 >&2 || true
tail -20 "$run/server.jsonl" "$run/server.stderr" >&2 2>/dev/null || true
echo "$run"
exit 1
