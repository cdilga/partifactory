#!/usr/bin/env bash
# Every harness client uses the shipped deployment plus the same explicit overlays.
set -euo pipefail
export PF_REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
h="$PF_REPO_ROOT/tests/e2e/harness"
export PF_RUN_DIR="${PF_RUN_DIR:-$PF_REPO_ROOT/target/e2e-runs/local}"
# Persist the selected image/mode for teardown and subprocess callers, without relying on
# whichever deployment-related variables happen to be in the caller's shell.
if [ -f "$PF_RUN_DIR/compose.env" ]; then
  set -a
  source "$PF_RUN_DIR/compose.env"
  set +a
fi
export PF_DOMAIN_APEX="${PF_APEX:-pf.test}"
case "$(uname -m)" in
  arm64|aarch64) export PF_HARNESS_NATIVE_PLATFORM=linux/arm64 ;;
  *) export PF_HARNESS_NATIVE_PLATFORM=linux/amd64 ;;
esac
if [ "${PF_E2E_RENDER:-0}" = 1 ]; then export PF_RENDER_ENABLED=true; else export PF_RENDER_ENABLED=false; fi
export PF_SITE_CONFIG="$PF_RUN_DIR/site.toml"
export PF_IMAGE="${PF_HARNESS_IMAGE:-pf-harness-dev:local}"
export PF_TLS_CERT="$PF_RUN_DIR/certs/cert.pem" PF_TLS_KEY="$PF_RUN_DIR/certs/key.pem"
export PF_HTTPS_PORT="${PF_HTTPS_PORT:-18443}" PF_OPS_PORT=19090
export PF_SESSION_KEY_FILE="$PF_RUN_DIR/secrets/session_key"
export PF_CAP_HMAC_KEY_FILE="$PF_RUN_DIR/secrets/cap_hmac_key"
export PF_METRICS_TOKEN_FILE="$PF_RUN_DIR/secrets/metrics_token"
export PF_IDP_CLIENT_SECRET_FILE="$PF_RUN_DIR/secrets/idp_client_secret"
files=(-f "$PF_REPO_ROOT/deploy/compose.yaml" -f "$h/compose.shipped-overlay.yaml")
if [ "${PF_HARNESS_RELEASE:-0}" != 1 ]; then files+=(-f "$h/compose.dev.yaml"); fi
if [ "${PF_HARNESS_MODE:-$( [ "$(uname -s)" = Darwin ] && echo host || echo container )}" = host ]; then
  files+=(-f "$h/compose.native-overlay.yaml")
fi
exec docker compose --project-name pf-harness "${files[@]}" "$@"
