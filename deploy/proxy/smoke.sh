#!/usr/bin/env bash
# Smoke-test a partifactory deployment through its proxy (the ingress contract, §3).
#
#   deploy/proxy/smoke.sh [--apex example.com] [--workspace parts] [--cacert ca.pem]
#                         [--resolve-to 127.0.0.1] [--port 443] [--dev] [--origin-down]
#
# Always: Host routing (workspace, a part host, a non-part host), /metrics not reachable
# through the proxy, a request id on every response, 8 MiB + 1 rejected by the proxy (413) and
# 8 MiB let through. With --dev (a binary built with the dev feature, as in the e2e harness):
# the 8 MiB body is echoed back, and an SSE stream is held for 60 s. With --origin-down: the
# maintenance page (503) on both hosts, for when partifactory is stopped.
set -euo pipefail
apex=pf.test ws=parts cacert= resolve_to= port=443 dev=0 down=0
while [ $# -gt 0 ]; do
  case "$1" in
    --apex) apex="$2"; shift ;;
    --workspace) ws="$2"; shift ;;
    --cacert) cacert="$2"; shift ;;
    --resolve-to) resolve_to="$2"; shift ;;
    --port) port="$2"; shift ;;
    --dev) dev=1 ;;
    --origin-down) down=1 ;;
    *) echo "smoke.sh: unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done

fail=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }
c() {
  local host="$1"; shift
  local args=(-sS --max-time "${MAXT:-15}")
  [ -n "$cacert" ] && args+=(--cacert "$cacert")
  [ -n "$resolve_to" ] && args+=(--resolve "$host:$port:$resolve_to")
  curl "${args[@]}" "$@"
}
base() { echo "https://$1:$port"; }
W="$ws.$apex"

if [ "$down" = 1 ]; then
  for h in "$W" "smoke-part.$apex"; do
    out="$(c "$h" -w '\n%{http_code}' "$(base "$h")/")"
    code="${out##*$'\n'}"
    if [ "$code" = 503 ] && grep -q "DOWN FOR MAINTENANCE" <<<"$out"; then ok "maintenance page on $h"; else bad "maintenance page on $h (got $code)"; fi
  done
  exit "$fail"
fi

code="$(c "$W" -o /dev/null -w '%{http_code}' "$(base "$W")/healthz")"
[ "$code" = 200 ] && ok "workspace /healthz 200" || bad "workspace /healthz ($code)"

rid="$(c "$W" -o /dev/null -D - "$(base "$W")/healthz" | awk -F': ' 'tolower($1)=="x-request-id"{print $2}' | tr -d '\r')"
[ -n "$rid" ] && ok "request id echoed ($rid)" || bad "no X-Request-Id on the response"

out="$(c "smoke-part.$apex" -w '\n%{http_code}' "$(base "smoke-part.$apex")/no-such-path")"
code="${out##*$'\n'}"
if [ "$code" = 404 ] && grep -q "request-id" <<<"$out"; then ok "part host reaches the content listener (404 page)"; else bad "part host ($code)"; fi

code="$(c "smoke-nonpart.$apex" -o /dev/null -w '%{http_code}' "$(base "smoke-nonpart.$apex")/")"
[ "$code" = 404 ] && ok "non-part host 404" || bad "non-part host ($code)"

code="$(c "$W" -o /dev/null -w '%{http_code}' "$(base "$W")/metrics")"
[ "$code" = 404 ] && ok "/metrics not routed through the proxy" || bad "/metrics through the proxy ($code)"
code="$(c "$W" -o /dev/null -w '%{http_code}' "https://$W:$port/readyz")"
[ "$code" = 404 ] && ok "/readyz not routed through the proxy" || bad "/readyz through the proxy ($code)"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
head -c $((8 * 1024 * 1024)) /dev/zero > "$tmp/8m"
head -c $((8 * 1024 * 1024 + 1)) /dev/zero > "$tmp/8m1"
path=/healthz
[ "$dev" = 1 ] && path=/_dev/echo
out="$(c "$W" -w '\n%{http_code}' -H 'content-type: application/octet-stream' --data-binary @"$tmp/8m" "$(base "$W")$path")"
code="${out##*$'\n'}"
if [ "$dev" = 1 ]; then
  grep -q '"bytes":8388608' <<<"$out" && ok "8 MiB accepted and echoed" || bad "8 MiB body ($code)"
else
  [ "$code" != 413 ] && ok "8 MiB let through ($code from the origin)" || bad "8 MiB rejected"
fi
code="$(c "$W" -o /dev/null -w '%{http_code}' -H 'content-type: application/octet-stream' --data-binary @"$tmp/8m1" "$(base "$W")$path")"
[ "$code" = 413 ] && ok "8 MiB + 1 rejected (413)" || bad "8 MiB + 1 ($code)"

if [ "$dev" = 1 ]; then
  start=$(date +%s)
  out="$(MAXT=90 c "$W" -N -H 'accept: text/event-stream' "$(base "$W")/_dev/sse?seconds=65" || true)"
  held=$(( $(date +%s) - start ))
  ticks="$(grep -c '^event: tick' <<<"$out" || true)"
  if [ "$held" -ge 60 ] && grep -q '^event: done' <<<"$out"; then ok "SSE held ${held}s ($ticks ticks)"; else bad "SSE held ${held}s, ticks $ticks"; fi
fi

exit "$fail"
