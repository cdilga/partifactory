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

The accumulated Playwright suite imports `sandbox_cookies.spec.ts` through `tests/e2e/security.spec.ts`. The sibling canary is a real HTTP service at `blobs-part/__sec_canary`, explicitly proxied by the harness; no application response is mocked. The control sends a seeded cookie to prove the canary sees cookies, then opaque sandbox GET/POST must reach it without the Lax cookie. Workspace fetches are separately checked for actual CSP violations. Cookie write/storage/parent isolation runs on all four browser projects.

The device warning gate needs a live code issued from the external client's network before browser execution:

```sh
python3 tests/security/run.py --base-url https://parts.pf.test \
  --ca-cert "$run/certs/ca.pem" --connect-to 127.0.0.1:18443 \
  --prepare-device-file "$run/security-private/device.json"
```

Pass the mounted private path as `PF_SECURITY_DEVICE_FILE` to Playwright. The file is mode 0600, contains a live code and must not be published as an artifact. The browser source must differ by IPv4 /24 or IPv6 /56 from the raw client, using addresses derived by the trusted ingress. The gate waits real elapsed time to observe the stale warning, observes the different-network banner, then verifies the sixth manual lookup fails. Network comparisons store only temporary keyed fingerprints; missing provenance and fingerprints from an earlier process are unknown. The server never trusts a caller-supplied network assertion.
