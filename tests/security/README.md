Run the probes from a client outside the deployment, through its configured HTTPS ingress:

```sh
python3 tests/security/coverage.py
python3 tests/security/run.py --base-url https://parts.example.com \
  --part-url https://notes-part.example.com/ --content-url "$CONTENT_URL" \
  --sibling-origin https://another-part.example.com \
  --evidence-dir target/e2e-logs/security
```

`--ca-cert FILE` trusts a private CA. `--connect-to ADDRESS:PORT` changes only the socket destination: the URL hostname, Host header, SNI and certificate hostname validation remain intact. Redirects are inspected without following them. No DNS or hosts-file changes are required.

`checklist.yaml` is JSON-compatible YAML and retains every complete PLAN §7 bullet under a stable `SEC-001`–`SEC-050` ID. Components state precisely what each reference proves; absent later features have owning phase beads. The real-phone check belongs only to the non-blocking human bead. Source references prove structural coverage, **not execution**.

The runner records pass/fail/unobserved for every existing component, plus pending/manual for deferred components, in `<evidence-dir>/SEC-xxx/result.json` and `report.json`. It prints `sec.check` lines without secret URLs or request bodies. Unavailable inputs are unobserved. `--require-complete` fails on unobserved existing checks; ordinary external runs still fail every observed failure. An ordinary browser lane skipping the disk/device gate never satisfies its checklist entry.

Supply accumulated execution evidence with repeated `--native-junit FILE` and `--playwright-report FILE`. Native JUnit cases must be crate-qualified (or carry their source file). Browser evidence must contain the required projects, with actual passed results and no skipped/flaky/expected failures. For a shell/Python observer such as renderer isolation, `--script-evidence FILE` accepts `{"observations":[{"file":"tests/render/isolation.py","test":"renderer_isolation","outcome":"pass","command":"<actual command>","artifact":"<captured output path>"}]}`. Create that record only after observing the command succeed.

Optional inputs are `--fixture` (the harness part fixture), `--session-file` (private Playwright storage state for authenticated grant fuzz), `--database-file` (read-only schema observation), and `--logs-file`/`--logs-url`, `--metrics-url` with `--metrics-token-file`, `--insights-db`, repeated `--secret-file` (values to scan, never copied into reports). `--expected-csp-file` or repeated `--embedder` supplies configured CSP additions. `--forbidden-listener ADDRESS:PORT` checks externally unreachable origin/ops listeners. Guest probes can additionally take private `--guest-content-url` and `--view-content-url`; the browser guest test independently observes those walls and revokes its links in teardown.

The blob observer fetches the actual shared prelude and a known private asset, checks their cache and cookie policies, and verifies that the private asset's content hash returns 404 on the shared host. Supply `--private-asset-path` relative to the content URL; the harness fixture defaults to `data.json`.

Redaction requires the actual canary request IDs to appear in the log evidence, so a stale or empty log cannot pass. Metrics are collected after the canaries; an explicitly supplied loopback ops URL may use HTTP, while public ingress remains verified HTTPS. The live Insights database is scanned read-only. Missing metrics or Insights input remains unobserved. Repeated `--canary-values-file` accepts private JSON string values from actual agent-surface runs (the harness writes mode-0600 `agent-surface-private/redaction-canaries.json` after preparing the eight lanes). The scanner checks plain, JSON-escaped and SQLite BLOB/SQL-escaped forms of these values, and the configured secrets supplied with `--secret-file`.

The accumulated Playwright suite imports `sandbox_cookies.spec.ts` through `tests/e2e/security.spec.ts`. The sibling canary is a real HTTP service at `blobs-part/__sec_canary`, explicitly proxied by the harness; no application response is mocked. The control sends a seeded cookie to prove the canary sees cookies, then opaque sandbox GET/POST must reach it without the Lax cookie. Workspace fetches are separately checked for actual CSP violations. Cookie write/storage/parent isolation runs on all four browser projects.

The device warning gate needs a live code issued from the external client's network before browser execution:

```sh
python3 tests/security/run.py --base-url https://parts.pf.test \
  --ca-cert "$run/certs/ca.pem" --connect-to 127.0.0.1:18443 \
  --prepare-device-file "$run/security-private/device.json"
```

Pass the mounted private path as `PF_SECURITY_DEVICE_FILE` to Playwright. The file is mode 0600, contains a live code and must not be published as an artifact. The browser source must differ by IPv4 /24 or IPv6 /56 from the raw client, using addresses derived by the trusted ingress. The gate waits real elapsed time to observe the stale warning, observes the different-network banner, then verifies the sixth manual lookup fails. Network comparisons store only temporary keyed fingerprints; missing provenance and fingerprints from an earlier process are unknown. The server never trusts a caller-supplied network assertion.

`ci/devices.sh` runs the emulated shipping matrix on a fresh harness, using `PF_DEVICE_PASS=1`. `ci/storage-pressure.sh` runs the accumulated pressure test against an actual 128 MiB tmpfs in the CI job's private mount namespace; it needs the CI job's `SYS_ADMIN` capability. It tests real filesystem accounting and writes through the normal server guards. The Mac uses `tests/storage/gate.sh` with its APFS image. These are dedicated lanes; no ordinary skipped test is counted as pressure/device evidence.

After the native, full browser, device and pressure jobs finish, `ci/security-evidence.sh` combines artifacts from that same workflow. `aggregate.py` checks the external report's checklist hash and merges actual native JUnit and browser project results, the renderer's final assertion line, and Caddy's real runtime-warning positive-control report. Observed failures remain failures even if another input passes. Missing expected evidence fails `--require-complete`.

The generic CI report uses `--platform linux`: its output explicitly lists the Mobile Safari Simulator component as `outside_platform`, with the required `macos` platform. That is a scoped Linux result, not all-platform completion. The bead's full gate additionally requires the separately observed Mac simulator artifacts. To combine them, pass the observed simulator command/artifact record with `--script-evidence FILE` and select `--platform all` (the default). A missing simulator observation then remains unobserved and fails `--require-complete`.
