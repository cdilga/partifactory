#!/usr/bin/env bash
# Item 12 proxies in Docker: Caddy (9443), nginx tuned (9444), nginx naive (9445), Traefik (9446).
# All forward to the rig's plain-HTTP listener on the host (:8080). Containers carry
# pf.harness=1 and pf.spike=1 so the cleanup finds them.  Usage: proxies.sh up|down
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
CERTS="${SPIKE_OUT:-$HERE/../../target/sandbox-spike}/certs"
CERTS="$(cd "$CERTS" && pwd)"
CADDY_IMAGE=caddy:2.10.0-alpine
NGINX_IMAGE=nginx:1.27.5-alpine
TRAEFIK_IMAGE=traefik:v3.4.1
COMMON=(--detach --label pf.harness=1 --label pf.spike=1 --add-host host.docker.internal:host-gateway -v "$CERTS:/certs:ro")

down() {
  local ids
  ids="$(docker ps -aq --filter label=pf.spike=1)"
  if [[ -n "$ids" ]]; then
    # shellcheck disable=SC2086
    docker rm --force $ids >/dev/null
  fi
}

up() {
  down
  docker run "${COMMON[@]}" --name pf-spike-caddy -p 9443:443 \
    -v "$HERE/proxies/Caddyfile:/etc/caddy/Caddyfile:ro" "$CADDY_IMAGE" >/dev/null
  docker run "${COMMON[@]}" --name pf-spike-nginx -p 9444:443 -p 9445:8443 \
    -v "$HERE/proxies/nginx.conf:/etc/nginx/nginx.conf:ro" "$NGINX_IMAGE" >/dev/null
  docker run "${COMMON[@]}" --name pf-spike-traefik -p 9446:443 \
    -v "$HERE/proxies/traefik.yml:/etc/traefik/traefik.yml:ro" \
    -v "$HERE/proxies/dynamic.yml:/etc/traefik/dynamic.yml:ro" "$TRAEFIK_IMAGE" >/dev/null
  for port in 9443 9444 9445 9446; do
    for _ in $(seq 1 30); do
      if curl -sk --max-time 2 "https://127.0.0.1:$port/_spike/ping" >/dev/null; then break; fi
      sleep 1
    done
  done
  docker ps --filter label=pf.spike=1 --format '{{.Names}} {{.Image}} {{.Status}}'
}

case "${1:-}" in
  up) up ;;
  down) down ;;
  *) echo "usage: $0 up|down" >&2; exit 2 ;;
esac
