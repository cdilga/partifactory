#!/usr/bin/env bash
# Entry point of the harness's Playwright container: trust this run's CA the way each browser
# needs (no certificate-error overrides), then run Playwright with the given arguments.
#   WebKit:   the system store (update-ca-certificates)
#   Firefox:  an NSS profile holding the CA (made by up.sh; PF_FIREFOX_PROFILE), because
#             Playwright's Firefox ignores enterprise policies
#   Chromium: the CA's SPKI hash (PF_CA_SPKI, passed by playwright.config.ts)
#   Node (APIRequestContext, https): NODE_EXTRA_CA_CERTS
set -euo pipefail
ca="${PF_CA_IN_CONTAINER:-/run/pf/ca.pem}"

cp "$ca" /usr/local/share/ca-certificates/pf-harness.crt
update-ca-certificates >/dev/null 2>&1

export NODE_EXTRA_CA_CERTS="$ca"
cd /repo/tests/e2e
status=0
npx playwright test "$@" || status=$?
# Hand the artifacts back to the invoking user (Linux bind mounts keep root ownership).
if [ -n "${HOST_UID:-}" ]; then
  chown -R "$HOST_UID:${HOST_GID:-$HOST_UID}" /repo/target/e2e-logs 2>/dev/null || true
fi
exit "$status"
