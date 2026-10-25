#!/usr/bin/env bash
# Entry point of the harness's Playwright container: trust this run's CA the way each browser
# needs (no certificate-error overrides), then run Playwright with the given arguments.
#   WebKit:   the system store (update-ca-certificates)
#   Firefox:  an NSS profile holding the CA (PF_FIREFOX_PROFILE), because
#             Playwright's Firefox ignores enterprise policies
#   Chromium: the same NSS trust database in its Linux user store
#   Node (APIRequestContext, https): NODE_EXTRA_CA_CERTS
set -euo pipefail
ca="${PF_CA_IN_CONTAINER:-/run/pf/ca.pem}"

cp "$ca" /usr/local/share/ca-certificates/pf-harness.crt
update-ca-certificates >/dev/null 2>&1

# Chromium reads local trust anchors from the user's NSS store. Create the
# database in this container with its own NSS version.
for trust_dir in "$HOME/.pki/nssdb" "$HOME/.local/share/pki/nssdb" "${PF_FIREFOX_PROFILE:?}"; do
  mkdir -p "$trust_dir"
  certutil -N --empty-password -d "sql:$trust_dir"
  certutil -A -n pf-harness-ca -t "C,," -i "$ca" -d "sql:$trust_dir"
done

export NODE_EXTRA_CA_CERTS="$ca"
cd /repo/tests/e2e
status=0
npx playwright test "$@" || status=$?
# Hand the artifacts back to the invoking user (Linux bind mounts keep root ownership).
if [ -n "${HOST_UID:-}" ]; then
  chown -R "$HOST_UID:${HOST_GID:-$HOST_UID}" /repo/target/e2e-logs 2>/dev/null || true
fi
exit "$status"
