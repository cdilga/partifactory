# Part runtime protocol

The sandboxed document sends messages only to its parent. The trusted wrapper accepts
messages only when `event.source` is its own content iframe; an opaque origin's `null`
origin alone is never an identity check. Messages are objects with a `type` string.

| Type | Direction | Meaning |
| --- | --- | --- |
| `pf:scroll` | document → wrapper | The current `y` scroll offset; the wrapper hides its phone bar while scrolling down. |
| `pf:ready` | document → wrapper | The document's first `partifactory.ready()` call completed. |
| `pf:regrant` | document → wrapper | A capability expired. Refresh through the credentialed workspace chrome API, then replace the frame's capability path. Fall back to a top-level `/grant` navigation if renewal fails. |
| `pf:version` | document → wrapper | Reload the frame after a live version change. |
| `pf:hello` | document → wrapper | Prelude boot announcement with `basePath` and protocol `v: 1`. |
| `pf:ping` | wrapper → document | Ask whether the document has called `ready()`. |
| `pf:pong` | document → wrapper | Reply to a ping with the boolean `ready` field. |

The capability arrives at the wrapper in `#pf=`. The wrapper clears that fragment before
creating the iframe, keeps the capability in that host's sessionStorage, and places it in
the iframe's `/c/<cap>/` path. Other URL fragments are preserved. Capabilities never appear
in runtime error reports or access logs.

The document calls `POST /c/<cap>/_rt/v1` with a JSON `{op, args}` body and
`Content-Type: text/plain`, `credentials: omit`, and CORS mode. This is a simple CORS
request and requires no OPTIONS preflight. Success is `{ok: value}`; failure is
`{error: {code, message, ...}}`. The runtime checks the host-bound capability, Origin,
Fetch Metadata, operation and role before committing a write.

The localStorage shim seeds from the serve-time `pf-boot` JSON, batches changes for 300 ms,
and flushes remaining changes on `pagehide` with a CORS `fetch` using `keepalive: true`.
The flush uses the same runtime guard; a `sendBeacon` text/plain request uses `no-cors`
and is rejected. Public-link, guest and render documents keep local state in memory.

`GET /c/<cap>/_rt/events` delivers `data.changed`, `pf:version` and `resync` events.
The prelude omits a writer's own `data.changed` notification using its per-tab identifier.
Expired or revoked capabilities end the event stream and require regranting.
