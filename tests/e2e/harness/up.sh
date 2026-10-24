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
export PF_HTTPS_PORT="${PF_HTTPS_PORT:-18443}"
export PF_BIN="${PF_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/partifactory}"
export PF_RUN_DIR="${PF_RUN_DIR:-$repo/target/e2e-runs/$(date +%Y%m%d-%H%M%S)}"
run="$PF_RUN_DIR"
mkdir -p "$run"/{certs,dex,secrets,data}

[ -x "$PF_BIN" ] || { echo "up.sh: no binary at $PF_BIN (cargo build -p partifactory --features dev)" >&2; exit 1; }

"$h/ca.sh" "$run/certs" >/dev/null
python3 "$h/gen-dex.py" "$repo/tests/fixtures/identity.toml" "$run/dex"
chmod 644 "$run/dex/"*.yaml

# Firefox trusts the CA through an NSS profile (Playwright's Firefox ignores enterprise
# policies); needs certutil (libnss3-tools on Debian/Ubuntu, nss on Homebrew).
if command -v certutil >/dev/null; then
  mkdir -p "$run/firefox-profile"
  certutil -N --empty-password -d "sql:$run/firefox-profile"
  certutil -A -n pf-harness-ca -t "C,," -i "$run/certs/ca.pem" -d "sql:$run/firefox-profile"
  chmod -R a+rX "$run/firefox-profile"
else
  echo "up.sh: certutil not found; firefox-desktop will fail TLS (install libnss3-tools / nss)" >&2
fi

for s in session_key cap_hmac_key metrics_token; do
  openssl rand -hex 32 | tr -d '\n' > "$run/secrets/$s"
done
chmod 644 "$run/secrets/"*

if [ "$mode" = host ]; then
  # Docker Desktop delivers container -> host.docker.internal connections from loopback.
  trusted='["127.0.0.1", "::1"]'
  export PF_UPSTREAM_WORKSPACE="host.docker.internal:18080"
  export PF_UPSTREAM_CONTENT="host.docker.internal:18081"
  ops_url="http://127.0.0.1:19090"
else
  trusted='["172.30.77.10"]'
  export PF_UPSTREAM_WORKSPACE="partifactory:8080"
  export PF_UPSTREAM_CONTENT="partifactory:8081"
  ops_url="http://127.0.0.1:19090"
fi

cat > "$run/site.toml" <<EOF
# Written by tests/e2e/harness/up.sh for one harness run.
[domain]
apex = "pf.test"

[ingress]
client_ip_header = "X-Forwarded-For"
request_id_header = "X-Request-Id"
trusted_proxies = $trusted
EOF

if [ "$mode" = host ]; then
  PF_SITE_TOML="$run/site.toml" PF_DATA_DIR="$run/data" \
  PF_SESSION_KEY_FILE="$run/secrets/session_key" \
  PF_CAP_HMAC_KEY_FILE="$run/secrets/cap_hmac_key" \
  PF_METRICS_TOKEN_FILE="$run/secrets/metrics_token" \
  PF_LISTEN_WORKSPACE=127.0.0.1:18080 PF_LISTEN_CONTENT=127.0.0.1:18081 PF_LISTEN_OPS=127.0.0.1:19090 \
  PF_LOG="${PF_LOG:-info}" PF_DEV_LOG_RING=20000 SSL_CERT_FILE="$run/certs/ca.pem" \
    nohup "$PF_BIN" serve > "$run/server.jsonl" 2> "$run/server.stderr" &
  echo $! > "$run/pf.pid"
  compose_profiles=()
else
  compose_profiles=(--profile container)
fi

touch "$run/server.jsonl"
docker compose -f "$h/compose.yaml" "${compose_profiles[@]}" up -d --quiet-pull caddy dex dex2 \
  $([ "$mode" = container ] && echo partifactory) >&2
if [ "$mode" = container ]; then
  # Follow the server's stdout into the run's JSONL, as host mode writes it directly.
  nohup docker compose -f "$h/compose.yaml" --profile container logs -f --no-color --no-log-prefix partifactory \
    > "$run/server.jsonl" 2>/dev/null &
  echo $! > "$run/logs.pid"
fi

cat > "$run/env" <<EOF
PF_RUN_DIR=$run
PF_HARNESS_MODE=$mode
PF_HTTPS_PORT=$PF_HTTPS_PORT
PF_OPS_URL=$ops_url
PF_CA=$run/certs/ca.pem
PF_CA_SPKI=$(cat "$run/certs/ca.spki")
PF_METRICS_TOKEN_FILE=$run/secrets/metrics_token
PF_SERVER_LOG=$run/server.jsonl
EOF

# Wait for the whole chain: browser-side TLS through Caddy to partifactory, and both Dex.
resolve=(--resolve "parts.pf.test:$PF_HTTPS_PORT:127.0.0.1" --resolve "dex.pf.test:$PF_HTTPS_PORT:127.0.0.1" --resolve "dex2.pf.test:$PF_HTTPS_PORT:127.0.0.1")
for i in $(seq 1 60); do
  if curl -fsS --cacert "$run/certs/ca.pem" "${resolve[@]}" "https://parts.pf.test:$PF_HTTPS_PORT/healthz" >/dev/null 2>&1 &&
     curl -fsS --cacert "$run/certs/ca.pem" "${resolve[@]}" "https://dex.pf.test:$PF_HTTPS_PORT/dex/.well-known/openid-configuration" >/dev/null 2>&1 &&
     curl -fsS --cacert "$run/certs/ca.pem" "${resolve[@]}" "https://dex2.pf.test:$PF_HTTPS_PORT/dex/.well-known/openid-configuration" >/dev/null 2>&1; then
    echo "up.sh: harness ready ($mode mode) in $run" >&2
    echo "$run"
    exit 0
  fi
  sleep 1
done
echo "up.sh: harness did not become ready; see $run/server.jsonl and docker compose logs" >&2
docker compose -f "$h/compose.yaml" logs --no-color --tail 50 >&2 || true
tail -20 "$run/server.jsonl" "$run/server.stderr" >&2 2>/dev/null || true
echo "$run"
exit 1
