# Spike: rmcp stateless Streamable HTTP (pf-twz8)

**Decision: GO.** rmcp `=3.7.0` serves stateless Streamable HTTP inside axum, speaks 2026-07-28 (`server/discover`, `Mcp-Method`/`Mcp-Name`) and still answers 2025-11-25 clients that `initialize`, without issuing a session id. All eight checks pass. Claude Code 2.1.310 and rmcp's own client both list and call the tools. No hand-written JSON-RPC fallback is needed.

## Question

Can the MCP adapter (pf-7qza) be a thin layer over rmcp 3.x: stateless, mounted at `/mcp` in our axum router, tools generated from the op registry, errors in band, auth done by our middleware before rmcp?

## Versions

| What | Version |
| --- | --- |
| rustc / cargo | 1.97.1 |
| rmcp, rmcp-macros | `=3.7.0` (released 2026-10-22), features `server`, `macros`, `transport-streamable-http-server`; tests add `client`, `transport-streamable-http-client-reqwest` |
| axum | 0.8.9 |
| schemars | 1.2.3 |
| tokio | 1.54.1 |
| reqwest (tests) | 0.13.5. It must be 0.13: rmcp's client impl is for reqwest 0.13, and a 0.12 client fails with `reqwest::Client: StreamableHttpClient` not satisfied |
| Claude Code | 2.1.310 |

## What was built

`spikes/spike-mcp` (about 400 lines with tests):
- A tiny op registry shaped like PLAN §4b: `trait Op { NAME, DESCRIPTION, HINTS, type Output; fn run }`. The input structs use `#[serde(deny_unknown_fields)]` + `JsonSchema`. `entry::<O>()` turns an op into an rmcp `Tool` and a type-erased runner.
- `ServerHandler` is implemented by hand (`list_tools`, `call_tool`, `list_resources`, `read_resource`, `get_info`), **not** with the `#[tool_router]` macros. The registry is the source of truth, and the macros would be a second one.
- Two tools: `part_get` (object output, a `not_found` error path) and `part_screenshot` (a PNG image block plus a non-object `u64` output, which gets wrapped).
- `StreamableHttpService` mounted with `nest_service("/mcp", ...)`, using `with_legacy_session_mode(false)`, `with_json_response(true)`, `with_sse_keep_alive(None)` and `with_allowed_hosts([...])`.
- An `axum::middleware::from_fn(auth)` layer in front of it. It returns 401 + `WWW-Authenticate` + a JSON body, and on success it inserts a `Principal` into the request extensions. rmcp copies `http::request::Parts` into `RequestContext.extensions`, so `call_tool` reads the principal with `ctx.extensions.get::<Parts>()?.extensions.get::<Principal>()`.
- `instructions` = `include_str!("docs/agent-contract.md")` (about 44 KB of markdown).
- A `part://demo` resource whose read runs `PartGet` and renders its output: a projection, not a separate code path.
- Logs: tracing JSON with `event="http.request" route_class="mcp" status=...` from the middleware, and `event="mcp.tool" outcome=ok|error`.

## The eight checks

| # | Check | Result | Notes |
| --- | --- | --- | --- |
| 1 | Stateless: no session id; `server/discover`; `Mcp-Method`/`Mcp-Name` (2026-07-28) | **pass** | `legacy_session_mode(false)` makes every request stateless, and 2026-07-28 is always stateless whatever the setting. `server/discover` returns versions, capabilities and instructions. A 2026-07-28 request without `Mcp-Method` gets JSON-RPC `-32020 missing required Mcp-Method header`. A 2025-11-25 `initialize` is answered with **no** `Mcp-Session-Id`, and later 2025-11-25 calls work without one |
| 2 | `inputSchema`/`outputSchema` from schemars; non-object outputs wrapped | **pass** | `rmcp::handler::server::common::{schema_for_input, schema_for_output}` (draft 2020-12, top-level title and description stripped). `deny_unknown_fields` shows as `additionalProperties: false`. rmcp 3.7 **no longer requires** an object output root (SEP-2106), so we wrap non-objects ourselves as `{"result": ...}`: 2025-11-25 clients, Claude Code among them, expect an object `structuredContent` |
| 3 | Tool annotations | **pass** | `ToolAnnotations::new().read_only().destructive().idempotent().open_world(false)` straight from the op's `HINTS` |
| 4 | Errors as `isError: true` + `structuredContent` + one text line | **pass** | Built by hand: `CallToolResult::error(vec![text(line)])`, then set `structured_content = Some({"error": ...})`. (`CallToolResult::structured_error` puts the whole JSON in the text, which isn't what we want.) Serde input errors (`unknown field`) are mapped to `unknown_field`/`invalid_input` in band, never a JSON-RPC error. Claude Code shows the line even though the error object doesn't match the tool's `outputSchema` |
| 5 | Auth failure = HTTP 401 + `WWW-Authenticate: Bearer resource_metadata=...` from middleware, before rmcp | **pass** | The transcript is below. rmcp never sees the request, and a wrong token adds `error="invalid_token"` |
| 6 | Image content blocks (PNG ≤ 1 MB) | **pass** | `ContentBlock::image(b64, "image/png")` next to the structured output. Claude Code received and described a 64x32 PNG, a 400x300 PNG (247 KB) and an **800x410 PNG (661 KB, 882 KB base64)**. Our op refuses anything over 1 MB with `too_large` |
| 7 | Long markdown `instructions` | **pass** | 44 KB `agent-contract.md` in `initialize` and in `server/discover`, unmodified |
| 8 | `part://` resources as projections | **pass** | `resources/list` and `resources/read` (`part://demo`) both work over the stateless path, and the read reuses the op |

## Transcripts

Run with `cd spikes && cargo run -p spike-mcp` (listens on `127.0.0.1:8080`, `SPIKE_MCP_ADDR` overrides).

Use `127.0.0.1`, not `localhost`: on the admin's Mac another process listens on `*:8080` over IPv6, so `localhost` resolved to it and answered `404 unknown host`.

Token-less call:
```
$ curl -si -X POST http://127.0.0.1:8080/mcp -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
HTTP/1.1 401 Unauthorized
content-type: application/json
www-authenticate: Bearer resource_metadata="http://localhost:8080/.well-known/oauth-protected-resource"

{"error":{"code":"unauthenticated","docs":"http://localhost:8080/llms.txt","hint":"Create a personal token in the workspace and send it as `Authorization: Bearer <token>`, or run `part login` (device code).","message":"This is the partifactory MCP endpoint and it needs a bearer token.","retryable":true}}
```
Log line: `{"level":"INFO","event":"http.request","route_class":"mcp","status":401,...}`

Stateless 2026-07-28 call (no initialize, no session):
```
$ curl -s -X POST http://127.0.0.1:8080/mcp -H 'Authorization: Bearer test' -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' -H 'MCP-Protocol-Version: 2026-07-28' \
    -H 'Mcp-Method: tools/call' -H 'Mcp-Name: part_get' \
    -d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"curl","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}},"name":"part_get","arguments":{"name":"missing"}}}'
{"jsonrpc":"2.0","id":4,"result":{"resultType":"complete","content":[{"type":"text","text":"not_found: No part named \"missing\" in your spaces. Hint: Check the name; part_list shows what exists."}],"structuredContent":{"error":{"code":"not_found","hint":"Check the name; part_list shows what exists.","message":"No part named \"missing\" in your spaces.","retryable":false}},"isError":true}}
```

Claude Code:
```
$ claude mcp add --transport http spike http://127.0.0.1:8080/mcp --header "Authorization: Bearer test"
$ claude mcp list   ->  spike: http://127.0.0.1:8080/mcp (HTTP) - ✔ Connected
$ claude -p "List the tools ... call part_get demo, part_get missing, part_screenshot 400x300 ..." \
    --allowedTools mcp__spike__part_get,mcp__spike__part_screenshot
unknown format "uint64" ignored in schema at path "#/properties/version"   (x4, stderr noise)
The spike server has two tools: mcp__spike__part_get and mcp__spike__part_screenshot.
- part_get demo: {"files":["index.html"],"name":"demo","owner":"a family member","version":3}
- part_get missing: error not_found: No part named "missing" in your spaces. Hint: ...
- part_screenshot 400x300: returned a PNG, structured output {"result":247116} ... a smooth diagonal colour gradient ...
```
(The spike ran `mcp add` from a scratch directory with local scope, then `claude mcp remove spike -s local`. The 661 KB image run used `--mcp-config` with `--strict-mcp-config`.)

rmcp client and tests: `cargo test --manifest-path spikes/Cargo.toml -p spike-mcp`, 5 passed:
`tools_list_schemas_match_schemars` (Discover lifecycle on 2026-07-28, plus a resource read), `rmcp_client_legacy_initialize_also_works` (auto lifecycle, image block), `missing_token_is_http_401_not_tool_result` (no token and a wrong token, plus the `http.request` 401 log assertion), `stateless_request_without_session_id_succeeds` (raw POSTs on 2026-07-28 and 2025-11-25), `error_is_iserror_with_structured_content`.

## Findings and risks

- **rmcp moves fast.** It went from 3.3 to 3.7 in six weeks (2026-09-10 to 2026-10-22), and patch releases change stateless behaviour, e.g. 3.6.0 "answer a session-less non-initialize POST with 400" and "reject requests to 2026-07-28 clients". Pin exactly (`=3.7.0`), and let the conformance suite in pf-7qza gate every upgrade.
- **schemars `format: uint64/uint32`** makes Claude Code's validator print `unknown format "uint64" ignored` on stderr. It's harmless, but pf-ops should strip non-standard `format` values (a schemars transform), or use `u32`/`i64` with explicit `minimum`.
- **`allowed_hosts` defaults to loopback only** (DNS-rebinding guard). Production must set it to `parts.<domain>` (and the internal hostnames). Otherwise every request gets rmcp's host rejection. `allowed_origins` should be enforced too (`enforce_origin_validation()`).
- `stateless_protocol_metadata_required(true)` would reject 2025-11-25 clients' ordinary calls. Leave it **off** while Claude Code et al. still initialize on 2025-11-25.
- `json_response(true)` gives plain `application/json` for simple calls. rmcp falls back to SSE by itself only if a handler emits a notification first, and our ops never do.
- Error `structuredContent` doesn't match the tool's `outputSchema`. The spec allows this for `isError` results, and Claude Code accepted it. Keep a conformance test on it.
- rmcp's server-side OAuth: none, as expected. Bearer extraction is ours, in middleware, as planned.

## What the dependent beads must do

- **pf-7qza (MCP)**:
  - Pin `rmcp = "=3.7.0"` with `server, transport-streamable-http-server` (no `macros` needed).
  - Implement `ServerHandler` by hand over the pf-ops registry, following `spikes/spike-mcp/src/lib.rs` (`entry::<O>()`, `error_result`, `principal()`), with config `legacy_session_mode(false)`, `json_response(true)`, `sse_keep_alive(None)`, `allowed_hosts = [parts host]`, origin validation on.
  - Advertise `[2026-07-28, 2025-11-25, 2025-06-18]` and leave `stateless_protocol_metadata_required` off.
  - Do auth as axum middleware in front of `nest_service("/mcp", ...)`, and pass the principal via request extensions, which reach `ctx.extensions::<http::request::Parts>`.
  - Wrap non-object outputs as `{"result": ...}`, and strip schemars' non-standard `format`s.
  - Port the five spike tests into the conformance suite.
  - Test clients need reqwest 0.13.
- **pf-1q1u (builds), pf-wmcw (wrapper), pf-8ssu (tsx)**: nothing from this spike.
