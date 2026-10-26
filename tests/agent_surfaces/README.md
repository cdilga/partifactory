# Agent surface conformance

`scenario.yaml` is JSON-compatible YAML. It covers all Slice 1 registry operations and checks
an independent stale-base edit, a conflicting anchor, person-only share-sheet navigation, data,
a render check, and restoring v1. Adding a registry operation without a scenario step fails.

The native suite (`cargo test -p partifactory -p pf-mcp --features partifactory/dev`) runs the scenario through a real rmcp
client, an HTTP client whose route table comes from the served OpenAPI document, the actual CLI
entry point in a child process, and a client deriving its endpoint from the served skill. Each
runs with a real OAuth authorization-code access token and a real device-grant access token. These native tests use a
small screenshot source; the harness pass below uses the actual render sidecar.

Against an already running native e2e harness:

1. Build the examples with the normal unified feature set:
   `CARGO_INCREMENTAL=0 cargo build -p partifactory -p pf-mcp --features partifactory/dev --examples`.
2. Run `mcp.spec.ts --project chromium-desktop` through the harness browser container. It performs
   browser consent, verifies MCP discovery/image/screenshot ownership and writes mode600 tokens
   under the private run directory (outside uploaded logs).
3. Run `tests/agent_surfaces/harness.py RUN_DIR --binary target/debug/partifactory`.

The runner uses the existing CLI-login CONNECT proxy to keep normal CA verification and SNI
without modifying `/etc/hosts`. It observes every real build going live, invokes the renderer,
compares results and audit rows across all eight lanes, tests skill installation, and saves
non-secret JSONL under `target/e2e-logs/RUN/conformance/`. The caller owns normal harness teardown.

The native deliberate-divergence test changes an error code and checks that the comparator
prints both versions and the failing lane.
