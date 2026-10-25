#!/usr/bin/env bash
# The e2e harness job (pf-kurh): build the binary with the dev feature, bring the harness up
# (Caddy with the shipped Caddyfile + per-run CA, two Dex, partifactory), then:
#   1. deploy/proxy/smoke.sh --dev through Caddy (Host routing, 8 MiB / 8 MiB + 1, 60 s SSE);
#   2. the running-binary gate: /metrics 401 / 200, a spoofed X-Forwarded-For ignored in the
#      access line, the rate-limit self-test route 429 + Retry-After at threshold + 1;
#   3. Playwright on chromium-desktop, chromium-mobile, webkit-mobile, firefox-desktop;
#   4. log assertions on the server JSONL (http.request lines with request_id and client_ip,
#      exactly one ratelimit.trip);
#   5. partifactory stopped: the maintenance page (smoke.sh --origin-down and @origin-down).
# Teardown (`docker compose down -v --remove-orphans`) runs in a trap, so it also runs on
# failure or Ctrl-C. Artifacts land in target/e2e-logs/<run>/; passing runs keep only the
# summary, and only the newest 5 runs are kept.
#
#   ci/e2e.sh [extra playwright args, e.g. --project chromium-desktop]
#   PF_E2E_SKIP_BUILD=1  reuse target/debug/partifactory
source "$(dirname "$0")/lib.sh"

h="$repo/tests/e2e/harness"
export PF_E2E_RENDER="${PF_E2E_RENDER:-1}"
export PF_E2E_RUN="run-$(date +%Y%m%d-%H%M%S)"
logs="$repo/target/e2e-logs/$PF_E2E_RUN"
mkdir -p "$logs"
export HOST_UID="$(id -u)" HOST_GID="$(id -g)"

if [ "${PF_E2E_SKIP_BUILD:-0}" != 1 ]; then
  cargo build -p partifactory -p pf-mcp --features "$PF_FEATURES" --bins --examples
fi
export PF_BIN="${PF_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/partifactory}"
( cd "$repo/tests/e2e" && npm ci --no-audit --no-fund --silent )
PF_RUN_DIR="$repo/target/e2e-runs/$PF_E2E_RUN" \
  docker compose -f "$h/compose.yaml" --profile browsers build browsers

run=""
status=0
teardown() {
  local rc=$?
  [ "$status" = 0 ] && status=$rc
  if [ -n "$run" ]; then
    cp "$run"/server.jsonl "$run"/server.stderr "$logs/" 2>/dev/null || true
    "$h/down.sh" "$run" || true
    cp "$run"/caddy.log "$run"/dex.log "$logs/" 2>/dev/null || true
    if [ "$status" = 0 ]; then
      # Passing: keep only the summary and the results file.
      find "$logs" -type f ! -name 'results*.json' ! -name summary.txt ! -name '*.png' ! -name report.json -delete 2>/dev/null || true
      find "$logs" -depth -type d -empty -delete 2>/dev/null || true
      rm -rf "$run"
    fi
  fi
  # Newest 5 runs only, for both the artifacts and the run dirs.
  for d in "$repo/target/e2e-logs" "$repo/target/e2e-runs"; do
    [ -d "$d" ] && ls -1dt "$d"/run-*/ 2>/dev/null | tail -n +6 | xargs rm -rf 2>/dev/null || true
  done
  echo "e2e.sh: $( [ "$status" = 0 ] && echo PASS || echo "FAIL (artifacts in $logs)")"
  exit "$status"
}
trap teardown EXIT
trap 'status=130; exit 130' INT TERM

run="$repo/target/e2e-runs/$PF_E2E_RUN"
export PF_RUN_DIR="$run"
"$h/up.sh" >/dev/null
set -a
# shellcheck disable=SC1091
source "$run/env"
set +a
export PF_RUN_DIR="$run"
summary="$logs/summary.txt"
note() { echo "$*" | tee -a "$summary"; }

caddy_v="$(docker compose -f "$h/compose.yaml" exec -T caddy caddy version 2>/dev/null | head -1)"
note "caddy: $caddy_v"
note "dex: ghcr.io/dexidp/dex:v2.45.1; playwright: mcr.microsoft.com/playwright:v1.64.1-noble"
note "mode: $PF_HARNESS_MODE; binary: $(git -C "$repo" rev-parse --short HEAD)"

# 1. Proxy smoke.
note "--- proxy smoke"
"$repo/deploy/proxy/smoke.sh" --dev --cacert "$PF_CA" --resolve-to 127.0.0.1 --port "$PF_HTTPS_PORT" | tee -a "$summary"

# 2. The running-binary gate.
note "--- running binary"
tok="$(cat "$PF_METRICS_TOKEN_FILE")"
code="$(curl -s -o /dev/null -w '%{http_code}' "$PF_OPS_URL/metrics")"
[ "$code" = 401 ] || { note "FAIL metrics without token: $code"; exit 1; }
curl -fsS -H "Authorization: Bearer $tok" "$PF_OPS_URL/metrics" > "$logs/metrics.txt"
grep -q '^pf_http_requests_total{route_class=' "$logs/metrics.txt" || { note "FAIL no pf_http_requests_total"; exit 1; }
note "ok   /metrics 401 without the token, 200 with pf_http_requests_total"

W=(--cacert "$PF_CA" --resolve "parts.pf.test:$PF_HTTPS_PORT:127.0.0.1")
spoof_rid="$(curl -fsS "${W[@]}" -o /dev/null -D - -H 'X-Forwarded-For: 6.6.6.6' -H 'X-Request-Id: spoofed-request-id' \
  "https://parts.pf.test:$PF_HTTPS_PORT/healthz" | awk -F': ' 'tolower($1)=="x-request-id"{print $2}' | tr -d '\r')"
sleep 0.5
line="$(grep -F "\"request_id\":\"$spoof_rid\"" "$PF_SERVER_LOG" | grep '"event":"http.request"' | head -1)"
if [ -z "$line" ] || grep -q '6.6.6.6' <<<"$line" || [ "$spoof_rid" = spoofed-request-id ]; then
  note "FAIL spoofed X-Forwarded-For / request id honoured: $line"; exit 1
fi
note "ok   spoofed X-Forwarded-For and X-Request-Id ignored (client_ip $(sed -E 's/.*"client_ip":"([^"]+)".*/\1/' <<<"$line"))"

for i in 1 2 3 4 5; do
  code="$(curl -s "${W[@]}" -o /dev/null -w '%{http_code}' "https://parts.pf.test:$PF_HTTPS_PORT/_dev/ratelimit")"
  [ "$code" = 200 ] || { note "FAIL rate-limit request $i: $code"; exit 1; }
done
hdrs="$(curl -s "${W[@]}" -o /dev/null -D - "https://parts.pf.test:$PF_HTTPS_PORT/_dev/ratelimit")"
grep -q '^HTTP/[0-9.]* 429' <<<"$hdrs" && grep -qi '^retry-after: [0-9]' <<<"$hdrs" \
  || { note "FAIL threshold + 1 did not return 429 + Retry-After"; exit 1; }
note "ok   rate-limit self-test: 429 with $(grep -i '^retry-after' <<<"$hdrs" | tr -d '\r') at threshold + 1"

# Build fixture pages through the running server, then mint their capabilities.
python3 "$h/seed-part.py"
if [ "${PF_E2E_RENDER:-0}" = 1 ]; then
  python3 "$repo/tests/render/isolation.py" "$run" | tee -a "$summary"
fi

# 3. Playwright, all four projects.
note "--- playwright"
docker compose -f "$h/compose.yaml" --profile browsers run --rm browsers --grep-invert @origin-down "$@" \
  2>&1 | tee "$logs/playwright.txt"
pw="${PIPESTATUS[0]}"
if [ -f "$logs/results.json" ]; then cp "$logs/results.json" "$logs/results-main.json"; fi
grep -E '^\s+[0-9]+ (passed|failed|skipped|flaky)' "$logs/playwright.txt" | tee -a "$summary" || true
[ "$pw" = 0 ] || { note "FAIL playwright"; exit 1; }

# Exercise native CLI device authorization and refresh through verified TLS without DNS edits.
note "--- CLI device login"
python3 "$repo/tests/device/cli-login.py" "$run" --binary "$PF_BIN" --https-port "$PF_HTTPS_PORT" 2>&1 | tee "$logs/cli-login.txt"

note "--- agent surface conformance"
python3 "$repo/tests/agent_surfaces/harness.py" "$run" --binary "$PF_BIN" \
  --mcp-client "$repo/target/debug/examples/gate_client" 2>&1 | tee "$logs/conformance.txt"
note "--- CLI export/import recovery"
python3 "$repo/tests/backup/recovery.py" "$run" --binary "$PF_BIN" \
  --https-port "$PF_HTTPS_PORT" 2>&1 | tee "$logs/recovery.txt"
note "--- IdP outage readiness and Health"
python3 "$repo/tests/backup/health.py" "$run" 2>&1 | tee "$logs/health-idp-down.txt"

# 4. Log assertions.
note "--- logs"
n_access="$(grep '"event":"http.request"' "$PF_SERVER_LOG" | grep '"request_id":"' | grep -c '"client_ip":"')"
n_trip="$(grep '"event":"ratelimit.trip"' "$PF_SERVER_LOG" | grep -c '"route":"dev_self_test"' || true)"
[ "$n_access" -gt 10 ] || { note "FAIL only $n_access access lines with request_id and client_ip"; exit 1; }
[ "$n_trip" = 1 ] || { note "FAIL expected exactly one dev_self_test ratelimit.trip, got $n_trip"; exit 1; }
if grep -E 'pf_(at|rt|pat)_[A-Za-z0-9]|/c/[A-Za-z0-9_-]{8,}' "$PF_SERVER_LOG" | grep -v '\[redacted\]\|\[cap\]' | grep -q .; then
  note "FAIL an unredacted token or cap in the server log"; exit 1
fi
note "ok   $n_access http.request lines with request_id + client_ip; 1 dev_self_test ratelimit.trip; accumulated feature rate gates checked by their tests; no unredacted caps or tokens"

# 5. Origin down: the maintenance page.
note "--- origin down"
if [ "$PF_HARNESS_MODE" = host ]; then
  kill "$(cat "$run/pf.pid")"; rm -f "$run/pf.pid"
else
  docker compose -f "$h/compose.yaml" --profile container stop partifactory >/dev/null
fi
sleep 1
"$repo/deploy/proxy/smoke.sh" --origin-down --cacert "$PF_CA" --resolve-to 127.0.0.1 --port "$PF_HTTPS_PORT" | tee -a "$summary"
PF_ORIGIN_DOWN=1 docker compose -f "$h/compose.yaml" --profile browsers run --rm -e PF_ORIGIN_DOWN=1 browsers \
  --grep @origin-down 2>&1 | tee "$logs/playwright-origin-down.txt"
[ "${PIPESTATUS[0]}" = 0 ] || { note "FAIL maintenance page in the browsers"; exit 1; }
note "ok   maintenance page served with the origin stopped"
