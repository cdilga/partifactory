# partifactory — plan history

The change logs of `PLAN.md`, moved here in round 3 so the plan holds only the current design. Each round is kept as it was recorded. Later rounds supersede earlier ones where they conflict, and `PLAN.md` is always the current word.

> Task ids from the retired task table have been removed; tasks live in beads.
> Where a sentence named a task, it now names the feature. The merge tables' "Where it landed" column keeps its section references and drops the task ids.
> Review finding ids (GLM `S1`–`S12`, `M1`–`M3`, the Opus `O*` and Fable `F*` items, the final review's `F01`–`F33`) are the reviewers' own labels, not task ids, so they stay.

### Round 1: the fresh-eyes review (2026-10-22)

Kept as it was recorded. Round 2 supersedes several of these outcomes (the cookie hand-off, `preview:mint`, the `/s/` card links, R2 and Litestream backups, P1b order); the round-2 table below says where each one went.


- **Grant hand-off**: one-time 30 s codes replace `?cap=` tokens in URLs. `SameSite=Lax` replaces `SameSite=None; Partitioned`, which wasn't needed because everything is same-site. `/grant` carries a path only, which closes the open redirect.
- **Blob privacy**: split shared and part blobs. Private parts' files are no longer on an edge-cached, CORS-`*`, URL-is-the-key host, where revocation couldn't reach them.
- **Third-party code**: mirror-on-save replaces the CDN allowlist, so CSP has no third-party origins and §1 and §2 no longer contradict each other.
- **Agent auth**: static admin-minted tokens → OAuth 2.1 per the MCP spec, with consent, scopes, spaces, delegation (`act`), device flow for headless agents, and DCR abuse limits.
- **Agent sharing contradiction**: resolved with a narrow `preview:mint` scope (≤ 24 h, view-only, one version). Everything else is `share:request`.
- **Admin step-up**: native WebAuthn in partifactory instead of trusting IdP `max_age`/`amr` claims.
- **Link shares**: view-only by default. Comments from link viewers are opt-in and labelled as untrusted for agents (prompt injection).
- **New**: agent discoverability (notes, outlines, resources, `part_recent`), `part_edit` with auto-rebase, in-place editing, self-publishing parts.
- **New**: operations (logs, metrics, health, alerts, analytics, abuse limits, quotas, blob GC, retention, migrations, dependency hygiene, doctor, scale envelope).
- **Fixed**: stale Cloudflare Access references, inconsistent `form-action`, duplicated P1 "Done when" lines, `partifactory-enrollment` vs `partifactory-invite`,
  CI perf gating on noisy runners, unencrypted backups, and the missing offline copy of the backup key.

**Merged from the GLM 5.3 adversarial review** (`docs/reviews/2026-10-22-glm-5.3.md`, run via ntm/omp on 2026-10-22, against the earlier draft):

| GLM finding | Outcome |
| --- | --- |
| S1 partitioned cookies / Safari / `/grant` in iframes (critical) | Already fixed in the rewrite (same-site `SameSite=Lax`). **Also adopted**: path-mode fallback + header-only `_rt`, so nothing depends on cookies |
| S2 caps in URLs | Already fixed (one-time 30 s codes, `no-referrer`). Rename 301s strip the query |
| S3 same-site cookie tossing | **Adopted as an open decision** with a recommendation: buy a separate domain for parts before P1. Honest CSP wording on navigation |
| S4 invite races / zombie accounts | **Adopted**: multi-use invites with a count, "new passkey link" for existing users, cleanup list |
| S5 provisioner RBAC granularity / `is_superuser` | **Adopted as a P1b design gate** with explicit fallbacks. The containment claim now matches what's verified |
| S6 no break-glass; pin is a DR liability | **Adopted**: console break-glass, pin + site config backed up off-host |
| S7 agent sharing contradiction, no cascade | Already fixed (`preview:mint` vs `share:request`). **Adopted**: `minted_by` cascade revocation |
| S8 CSRF on `_rt` | **Adopted**: `X-PF-Cap` header + `Sec-Fetch-Site` check |
| S9 CDN allowlist ≈ `*`; CSP gaps | Already fixed (mirror-on-save). **Adopted**: full directive list (`default-src 'none'`, `base-uri`, `object-src`…) |
| S10 open redirect via `/grant` | Already fixed (path-only, host derived from the part name) |
| S11 blob host = unrevocable | Already fixed (shared vs part blobs; part blobs never edge-cached) |
| S12 revocation claims inconsistent | **Adopted**: in-memory grant epoch |
| C2 Telegram webview / unfurlers | **Adopted**: content-free card at `/s/<token>`. Telegram stays preview-only |
| C3 conditional UI / `max_age` assumptions | Admin step-up is now native WebAuthn. Conditional UI is a P1 spike. Budgets count real interactions |
| C4 h2c, SSE timeouts | h2c claim dropped. SSE heartbeat 25 s, long-poll ≤ 50 s |
| C5 wildcard captures future names | **Adopted**: a metric on non-`-part` hits |
| C6 editing shared login flows | **Adopted**: partifactory gets its own cloned flows. Global changes only when explicitly wanted (the "stay signed in" removal was) |
| C7 oxc/rolldown risk, import maps | **Adopted**: pinned versions, golden compile corpus, import-map story. The `vite build` escape hatch stays prominent |
| C8 rename collisions | **Adopted**: used names stay reserved, renames are admin-confirmed |
| C9 hidden DB read on entry | **Adopted**: in-memory live-version cache |
| O1 IdP traits are YAGNI | **Partly rejected**: the admin asked for swappability. Kept thin, with one real adapter + a test fake |
| O2 io_uring + frankensqlite | **Adopted**: both cut (parked as ideas) |
| O3 webhook + poll | **Adopted**: poll only |
| O4 reconciler/adapter matrix too heavy | **Partly adopted**: an honest idempotent script, owned resources only, no plan mode in v1. Multi-host stays (the admin wants DR and portability) |
| O5 `manage` grant editing | **Adopted**: grants are admin-only in v1 |
| M1–M3 GC, quotas, rate limits, monitoring | Already in the rewrite (§5d) |
| P1 µs gates on shared runners | Already fixed (same-machine regression gates) |
| P2 sendfile claim | **Adopted**: claim softened, mmap + pread in v1 |
| P3 content-free cacheable HTML shells | **Rejected**: it would break non-JS parts and Claude compatibility. The 150 ms target is now reported honestly, not gated |
| P4 brotli + zstd | **Adopted**: brotli only |
| P5 targets exclude the TSX path | **Adopted**: a TSX edit target added |
| D1 IdP not in DR (critical) | **Adopted**: Authentik's DB in the site backup set + `export-grants` degraded mode |
| D2 blob/DB ordering | **Adopted**: blob acknowledged by R2 before the manifest commits |
| D3 RTO vs full blob download | **Adopted**: lazy restore |
| D4 two connectors after cutover | **Adopted**: rotate the connector token, probe via `cfargotunnel.com` |
| D5 circular DR dependencies | **Adopted**: laptop-driven drill, offline copy of site config |
| D6 key retention | **Adopted**: keys kept 25 h. Links are DB tokens, unaffected |
| U1–U6 click budgets | **Adopted**: real-interaction budgets, real-device passes, multi-use invites, admin-confirmed renames, Telegram preview-only |

### Round 2 (2026-10-23)

Inputs: the admin's decisions in chat (binding, and later messages overriding earlier ones on backups), an Opus review (O1–O17), a Fable review (parts A–H), a grounding check against primary sources (G), a self-containment, data-model and task-graph validation (V), and a design proposal (D).

**What changed.**
- **Sandbox, no part-host cookies**: every part document gets CSP `sandbox` without `allow-same-origin`. The `__Host-pf_cap` cookie, the one-time `?g=` codes, the cookie-tossing prelude and the separate-domain open decision are gone. Caps are in the path, deterministic per 12 h window, with a boot-epoch floor.
  `/_rt` is one text/plain POST (no preflights). The `Sec-Fetch-Site: same-origin` requirement is dropped: it would have rejected every legitimate call from an opaque origin. Three inconsistent CSPs became one canonical block.
- **Surfaces**: one operation registry renders MCP, the HTTPS API (OpenAPI 3.1), the CLI and `/skill.md`. Discovery, one error shape with status and exit maps, one event log with three deliveries, a cross-surface conformance suite. Tools consolidated to ten around one write op.
- **Links**: agent preview links (`preview:mint`, 24 h, version-pinned) are removed. One link to post, `/p/<name>`. Public links are admin-only, no default expiry, with open counts.
- **Agents**: one connection per (agent, person); service identities; inward sharing via `share:household`; outward only by request. Token audience fixed to `https://parts.example.com/`. MCP spec 2026-07-28 named, with its AS metadata, `iss`, CIMD SSRF guards and redirect-host display.
- **Spaces** defined: personal + `family`, live inheritance. A role × runtime matrix; viewers may write shared data by default. Step-up is one 10-minute window for admin actions only.
- **Previews**: in-binary branded cards decided by audience; an optional render sidecar; `part_check` as a P2 headline.
- **Core model**: build records keyed by `hash(manifest, toolchain)`, head vs live, import maps frozen per build, builds in a child process, mirror-on-build (source untouched), `request_id` on every write, ordered batch edits, a structured error catalogue, fork, scratch parts, assets, `render: "text"`, full content for small parts.
- **Backups**: the `/data` folder plus `export`/`import`, a three-layer table, a host contract and a moving checklist. Litestream (whose v0.5 dropped age encryption), R2 as a built-in, the blob-before-manifest ordering and the async sink outbox are all gone.
- **Search, history, bin** specified. **Stockroom** UI and **Jig** added as §4a. **Data model** (§5e) and **task graph** (§6a) added. Phases re-ordered, with "Done when" for every phase.
- **Grounding fixes**: Authentik invitations can't set groups and have no use count; Cloudflare Free has one rate-limit rule; Cloudflare caches by extension; oxc/rolldown Rust APIs aren't semver-stable; the wildcard CNAME to the tunnel needs verifying.

**Considered and rejected during round 2.**
- **A git parts archive** (each version a commit in a private Gitea repo, pushed from an outbox). the admin: no git or Gitea dependency for part content or backups. Versioning stays inside partifactory, and backup is the `/data` folder.
- **An async backup-sink outbox with storage-agnostic sinks** (local dir, S3 via `object_store`) on the core path. Replaced by `export` + host backups. An S3 upload of exports is optional and P5.
- **15-minute `VACUUM INTO` snapshots shipped off-box.** Replaced by an hourly local consistent copy for host tools and full exports.

**Merge table.**

| Source | Item | Verdict | Where it landed |
| --- | --- | --- | --- |
| the admin 1 | No cookies on part hosts; sandbox; no second domain | **Adopted** | §2 Serving, §3, §8.10, §7 |
| the admin 2 | Any AI, any surface; registry; discovery; PATs; one error shape; one event log; no MCP-only features; conformance | **Adopted** | §4, §4b, §3b, §8.11 |
| the admin 3 | Tier (a) cards always, by audience; tier (b) sidecar; `part_check` headline | **Adopted** | §4c, §6 P2, §8.16 |
| the admin 4 | No agent-minted preview links; admin-only public links, no default expiry, open counts | **Adopted** (round 2b: the link to post became the part's own host, and public links moved to `<name>-part/l/<token>`) | §2 link, §3a ladder, §5e `public_links` |
| the admin 4 | Telegram in-app browser + passkeys | **Open item**, tested in P1; person-bound fallback, never anonymous. *Superseded at the Slice 1 freeze:* the fallback link is **rejected**; a 5-minute real-phone test stays, and its fixes are Telegram's open-in-browser setting and an "open in browser" hint in Hermes' messages | §2, §6 P1 |
| the admin 4b | Inward yes (`share:household`), outward no; divergence from shartifacts; connecting = identity | **Adopted** | §3b, §1, §8.13 |
| the admin 5 → later messages | Storage-agnostic sinks + outbox, then a git archive, then **`/data` + export/import** | **Final: `/data` + export/import**; git archive and core-path sinks rejected | §5c, §10 rejected list |
| the admin 5 | Per-part data opt-in, expiry, keep the last one | **Adopted** as "keep data snapshots", for undo not DR (round 2b: on by default) | §5c, §5e |
| the admin 5 | Authentik's Postgres out of scope | **Adopted** | §5c, §9 |
| the admin 5 / F E1.5 | Image + CLI on the GitHub mirror for laptop drills | **Adopted** | §5b |
| the admin (later) | Host contract, backup layers, `doctor` reports layers, moving checklist | **Adopted** | §5b, §5c |
| the admin 6 | FTS5 search, filters, bm25 + recency, embeddings later | **Adopted** | §3e |
| the admin 7 | Versions labelled, diff any two, preview old, restore = new, keep all, 30-day bin | **Adopted** | §3e, §4a.6 |
| the admin 8 / D | Stockroom theme, Jig design system, mockup | **Adopted**, adjusted for `/p/<name>` (round 2b: the part view moved into the part host's wrapper), no previews, public links without expiry | §4a |
| Sandbox mechanics | Cap in path, windowed, boot floor, no `?g=`, `pf:regrant`, `/_rt` simple request, ACAO null, no CORP, canonical CSP, shim, escape hatch, per-part hosts kept, P0 spike | **Adopted** | §2, §5 spike list |
| O1 | Opaque-origin sandbox | **Adopted**, with ACAO `null` + `Vary` (not `*`) and no UA-mismatch re-grant | §2 |
| O2 / F B6 | Spaces first-class | **Adopted**, live inheritance (V) over copy-at-create (F) | §2, §3a |
| O3 | One token per (agent, person); family hub service principal | **Adopted** | §3b |
| O4 | Canonical smart link `/s/<token>` with fallbacks and agent previews | **Modified**: canonical `/p/<name>` + separate admin-only public link; `/s/` dropped. **Round 2b:** canonical is now the part's own host with a trusted wrapper; `/p/<name>` only redirects | §2 link |
| O5 | Agent inbox + live reload | **Partly**: live reload P2, inbox and steward P3 | §3c |
| O6 | Tap-to-edit text, point-and-comment | **Adopted for P4** | §3c |
| O7 / F E1 | Saves don't wait on backups; restore to newest complete | **Adopted in spirit**: no backup target on the write path at all | §3d, §5c |
| O8 | Role × `/_rt` matrix, viewer-writable data, no step-up for household | **Adopted**; step-up window fixed at **10 min** (O suggested 15) | §3a |
| O9 / F part F | Phase re-order | **Adopted**: sandbox + TSX in P0, agents before invites (P2 then P2b), bench comparison after P1, Done-when for every phase | §6 |
| O10 | Blob upload with have/need, idempotent edits, latest vs live | **Adopted** | §3d |
| O11 / F C1 / F H | Consolidate tools; part-level leases | **Adopted**: ten ops, one write op; part scope by default with optional file/block | §4, §3d |
| O12 | Notifications for people | **Adopted, minimal, P4**. **Cut in round 2b**: approvals are in-band; only an optional ops-alert webhook remains | §4 |
| O13 | Kinds table; fixed Tailwind build | **Kinds adopted**; Tailwind is a P3 spike choosing among three options on evidence | §2 Kinds, §6 P3 |
| O14 | Design system as a part; remix | **Adopted**: Jig is a part; fork via `part_create {from}` | §4a.8, §3d |
| O15 | Previews mechanics (read-only renderer, content-addressed og URLs, thumbnails via workspace, names not secret) | **Adopted** | §4c, §4 Names |
| O16 / V | Schema | **Adopted** as the full DDL | §5e |
| O17 | Agent surfaces | **Adopted** | §4b |
| O smaller | Leptos may lose to templates; undo vs immediate revoke | **Adopted**: a decision rule; revoke is immediate, undo re-grants | §5, §3a |
| O smaller | Per-link view count only for analytics | **Not adopted**: daily-salted uniques kept, plus per-link open counts | §5d |
| F A2 | `'unsafe-inline'`/`'unsafe-eval'`; CSP is egress policy | **Adopted** | §2 CSP |
| F A2 | `allow-pointer-lock` | **Not added**: not in the decided token list; revisit if a game needs it | — |
| F A1 | 1 h URL caps with a 10 min `/_rt` freshness | **Replaced** by 12 h deterministic windows + epochs | §2 |
| F A8 | Boot-epoch floor; per-part epochs for agent changes | **Adopted** | §2 |
| F B1/B2 | Build records, toolchain-keyed cache, frozen import maps, head vs live | **Adopted** | §2, §3d |
| F B3/B4/C2/C3 | `request_id` everywhere, ordered batches, error catalogue, `wait` shape | **Adopted** | §3d, §4b |
| F B5 | Token audience = `https://parts.example.com/` | **Adopted** | §3b |
| F B7/B8 | Assets, fork, scratch parts, `render: "text"` | **Adopted** (assets P3) | §3c, §3d |
| F D1 | Child-process compile, 10 s, memory cap | **Adopted** | §3d, §5 |
| F D2 | Mirror-on-build, unpinned resolution, fonts one level, jsDelivr `/npm/` only | **Adopted** | §2 |
| F E1.3 | Per-part data backup on by default | **Disagreed** in round 2 (opt-in + prompt + keep-last). **Adopted in round 2b**: on by default, daily keep 7, switchable off per part | §5c, §5e |
| F E1.4 | Same-disk backup is amber | **Adopted** (for exports) | §5c |
| F E2 | `/readyz` decoupled from the IdP; fail open on a failed poll | **Adopted** | §3a, §5d |
| F E3/E5 | Atomic blob writes, `storage_full` at 95 %, events kept 30 days | **Adopted** | §3d, §5d |
| F H1 | Manual client registration; resources and instructions as projections; provenance as JSON | **Adopted** | §3b, §3c, §4b |
| G A1 | Litestream v0.5 has no age encryption | **Adopted**: Litestream removed | §5c |
| G A2 | Authentik invitations can't set groups; no use count | **Adopted**: grants and counts in partifactory, membership by API after enrolment | §3a |
| G A3 | Cloudflare Free = one rate-limit rule | **Adopted**: `governor` is the control, one combined edge rule | §5d |
| G A4 | MCP 2026-07-28, CIMD SHOULD, DCR deprecated but kept, `iss`, AS metadata, redirect host | **Adopted** | §3b |
| G A5 | oxc/rolldown APIs unstable | **Adopted**: pin exactly, vendor, esbuild fallback | §5 |
| G A6 | Cloudflare caches by extension | **Adopted**: explicit cache rules; no `Set-Cookie` on shared blobs | §5c, §7 |
| G | Wildcard CNAME to the tunnel unverified; remaining Authentik items | **Adopted as gates** | §5 spike, §3a |
| G E | A2A card | **Disagreed**: partifactory is a tool server, not an agent | §4b |
| V 1.1–1.8 | Self-containment text: caps, mirror, rebase, block ids, build worker, provisioner, restore, device grant | **Adopted**, restore rewritten for `export`/`import` | §2, §3b, §3d, §5c |
| V 0.9/0.10/0.13/0.14 | `everyone-signed-in` = audience group; invite "opened" via `/i/<id>`; CIMD SSRF; ask for access | **Adopted** | §3a, §3b |
| V 0.12 | CF plan in config; analytics token at runtime | **Adopted**: separate read-only token or drop the metric | §5c, §5d |
| V §3 | Task graph | **Adopted**, re-cut for these decisions: 85 tasks, 6 milestones, acyclic, no orphans (90 tasks after round 2b) | §6a |
| V §4 | Justify Rust, Leptos, the bench, compile isolation, per-part hosts | **Adopted** | §5, §5a, §3d, §2 |

### Round 2b (2026-10-23)

Inputs: four decisions from the admin, the owner, made after round 2. They're binding, and where they conflict with round 2 they win.

**What changed.**
- **Portability: no strong dependency on the current stack.** An audit found couplings to Cloudflare (tunnel, DNS, cache rules, the rate-limit rule, the fallback Worker, health checks, `cf-ray` ids, country/ASN on device approval), Authentik (the only `Authn`/`Directory` adapter, the admin `sub` pin, invites, recovery, groups, cloned flows),
  Gitea (secrets only in Gitea Actions, the deploy pipeline, the registry, Renovate), TrueNAS (the default host), Telegram/Hermes (the default alert notifier, a notify channel, a `[hermes]` config section) and the family hub (hardcoded in `frame-ancestors`), plus `example.com` written into normative text.
  Each one is now an adapter behind config with a generic default: the ingress contract with Caddy/nginx/Traefik examples (§3), `provision` provider plugins with a manual mode (§5c), generic OIDC with a claims re-read directory and native invites on local passkey accounts (§3a), an OCI image + compose file + env contract with `_FILE` secrets (§5b),
  generic webhooks (§4), `[embed] frame_ancestors` (§2), and `<domain>` everywhere with example.com as the example deployment. New §5f has the table, and a **portability lane** runs the full e2e suite with Dex, Caddy and local exports, and no Cloudflare, Authentik or Gitea.
  Schema: `users` gains `auth_source` and `(idp_issuer, idp_sub)`, `groups` gains `source`, `admin_credentials` becomes `passkeys` with a `purpose`, sessions keep an encrypted IdP refresh token, `audit.cf_ray` becomes `request_id`.
  Tasks added: the ingress contract, built-in ACME (P5), the Cloudflare adapter, the Authentik adapter, the portability lane, and native invites with local accounts. Re-scoped: the OIDC client, the directory, `provision`, Health, the deliverable and the Authentik provisioner work. Decisions 22–24.
- **The link to post is the part's own host.** `https://<name>-part.<domain>/` serves a trusted, unsandboxed wrapper with the Stockroom chrome, framing the sandboxed content at `/c/<cap>/` on the same host. With no cap, the wrapper sends the browser to `/grant`, which returns with the cap in a `#pf=` fragment.
  The wrapper clears the fragment, keeps the cap in `sessionStorage` and sets the frame. Chrome actions use a credentialed same-site chrome API on the workspace, with CORS for exactly the sandboxed part-host origins. The part host still never sets or reads a cookie.
  `/p/<name>` only redirects, public links moved to `<name>-part.<domain>/l/<token>`, and the first open in a tab costs one redirect hop. Updated: §2 link section and capability hand-off, §3 topology, §4a.6 part view, §4a.7 budgets, §4c cards, §5 spike list, §7, the content listener, `/grant`, the part-host root and the workspace UI, decision 25.
  *One correction to the brief:* the "no cap → `/grant`" step has to be a client-side `location.replace`, not a server 302, because the server never sees the fragment and can't tell a cap-carrying load from a bare one.
  *Added beyond the brief:* the chrome API's CORS never admits an `unsandboxed` part's origin, because that part's code is same-origin with its wrapper and would otherwise ride the viewer's session. Its wrapper falls back to opening Share and Comments on the workspace. The wrapper's exposure to that part (its own cap only) is accepted.
- **Per-part data snapshots are on by default** (daily, keep 7), switchable off per part, with the 90-day idle stop and keep-the-last-one rule kept. Fable's round-2 suggestion is adopted. §5c, `data_snapshot_policies.mode DEFAULT 'daily'` (the prompt column is gone), the data-snapshots work, decision 26.
- **Notifications for people are cut.** Share and access requests are approve/deny chips in the Requests inbox and on the part's chrome bar. The asking agent tells the person in its own channel. Device approvals are started by the person. Agent inward shares are inbox and activity-feed entries.
  The only push left is an **optional ops-alert webhook** for the admin (disk ≥ 90 %, no export in 48 h, build worker crash-looping, IdP unreachable), off by default and mirrored on the Health page. Removed: web push, comment and mention notifications, per-person settings, the `notify_prefs` table and the notifications task. The event log and its webhooks for agents stay. Decision 27.

**Kept coupled, on purpose** (§5f): one registrable domain for workspace and part hosts (the cookie model needs same-site), WebAuthn passkeys, OCI + compose as the deployment unit, the default mirror CDNs (a Claude-compatibility target, and config), and the Telegram in-app browser in the real-device pass.

### Round 2c (2026-10-23)

Inputs: seven decisions from the admin, the owner, made after round 2b, plus one later override from him that removes the approval workflow. They're binding, and where they conflict with earlier rounds they win. Round 2b (portability adapters, the part host as the link, data snapshots on by default, notifications cut) was already in place and is unchanged.

**What changed.**
- **Backends in v1** (new §3f, phase P2a). A part is a page plus, optionally, containers: a Dockerfile built by the server, a registry image from an allowlist (pinned by digest), or an uploaded `docker save` tarball. Services use a documented compose subset; anything else fails the build by name.
  One container set per live version, gated by a health check, so a rollback rolls the backend back (volumes stay, and the plan says so). Scale to zero by default, always on per part. Routing is `/c/<cap>/api/*` behind the cap check, with cookies, caps, `pf_` tokens and `X-PF-*` stripped, a signed per-part `X-PF-Identity` JWT added, and WebSocket and SSE proxied.
  Only `partifactory-runner` holds the Docker Engine API (Docker or Podman), and it starts containers from one hardened template (no privileges, all caps dropped, read-only root, forced non-root, seccomp, limits, `json-file` 10m × 3, optional gVisor). Each part gets its own network: internet egress on, LAN, host and other parts dropped by explicit rules (`DOCKER-USER` on Docker, nftables on Podman).
  Volumes live under `/data/parts/<id>/vol/<svc>` and are in exports (export hook or stop-copy-start); data snapshots don't cover them. Quotas per part and globally; `backends:deploy` is off by default at consent. Removed: "v1 has no backends", the WASM-first line (WASM is now an optional P5 alternative), and "Per-part backends" from the non-goals. Decision 5 is marked superseded.
- **Server builds by default** (new §3g): frontends in the binary, backends with rootless BuildKit in the runner (network on behind the LAN block, 10 minutes, limits, 10 GiB LRU cache). Prebuilt files or images are optional. The why is in the plan: quick edits need a rebuild, many AI clients can't build, human edits in the UI need it, and source stays the diffable truth. `builds.method` records `server_build | prebuilt_files | prebuilt_image | uploaded_image` per target.
- **`text` kind**: as-is rendering in a readable shell with line anchors (§2 Kinds, `TXT` bin code, the kind renderers).
- **"Can contribute" public links** (§3a). Guests are web consumers only: they write shared data through `/_rt` data ops and call the backend as `role: guest`, and get `403` on every non-data `/_rt` op and on `/api` and `/mcp`. A signed guest id rides in the link cap (`g`, with `dw`), so there's still no cookie. Per-guest and per-IP limits, a shared-data cap, a snapshot on switching, roll back, reset and freeze. Share-sheet copy, the role matrix (new guest row and a backend column), the click budgets and §7 are updated.
  **The communal pixel editor** is the named CI test at the end of P2 (`communal_pixel_editor`) and, with a Postgres leaderboard, at the end of P2a (`communal_pixel_editor_backend`).
  *Moved to make that possible:* shared `window.storage` and live `data.changed` events moved from P3 (with `claude.use`) into P2 (shared data over `/_rt`), because the P2 acceptance test needs them.
- **Insights** (new §5g): admin-only analytics and observability in a separate `/data/insights.db` (DDL added, and `view_events`/`view_daily` moved out of the main schema into it). Raw events 30 days, rollups indefinitely, truncated and salted IPs, no content.
  Pages for Overview, Agents, Parts, People, Builds and Backends; request ids on every error and propagated to builds, `/_rt` and backends; tail-sampled traces with a waterfall; a per-part Debug tab that shows the decisions behind a 403; a per-agent call page; a log viewer with live tail; `/metrics` behind an admin token; an optional OTLP adapter (§5f row). A fuzz test keeps content out of logs and analytics. The P4 analytics overview is folded into the Insights pages.
- **Disk safety** (new §5h) and a **"Defaults are the safe choice"** principle at the top. Every growing store has a cap, a retention rule, rotation, an export path and a metric, in one table, run by one janitor that never deletes anything referenced. A global disk budget refuses new writes at 95 % and never refuses reads or deletes.
  Store sizes are on Health and in Insights, with an alert at 80 % of a cap. Turning a safe default off needs an `[unsafe]` key and shows on Health and in `doctor`. Runtime errors are now kept 1,000 per part for 30 days (the trigger was 20); the nightly export is on by default; alert thresholds changed from "disk ≥ 90 %" to the budget and per-store caps.
- **No approval workflow** (the admin's later override). Agents act directly within their scopes, including inward sharing, deploying backends and **setting secrets, which are write-only and per part**: nobody, the setting agent included, can read one back, and they never appear in versions, exports, logs or Insights. People can set them in the part's settings too.
  People-only actions (public links, invites, groups, LAN allowlist entries, admin) happen in the UI. An agent that thinks one is needed calls the new op **`part_link_for {action}`** and hands the person a deep link that opens the sheet pre-filled (`<name>-part.<domain>/#share=public`, `parts.<domain>/admin/backends/<part>#lan`).
  Removed: the `share_requests` table, the `share:request` scope, the Requests inbox and tray, approve/deny chips, `part_share {request}`, "request a secret", the `pending_approval` invite status and "share decisions" in `part_inbox`. "Ask for access" stays as a small person-to-person list (`access_asks`) in the share sheet with Add and Dismiss. Device-code connect stays: it's sign-in, not a queue.
  *Why:* approval queues cause fatigue and stall agents whenever the person is away, and they protect nothing the scopes don't. An agent setting a key already holds it, so the protection that matters is write-only storage and per-part scope.
- **Schema**: `parts` gains `text` in `kind`, `shared_data_cap`, `backend_lifecycle` and `api_prefix`; `builds` gains `target`, `method` and `log_hash`; `public_links` gains `mode` and `frozen_at`; new `access_asks`, `backend_images`, `container_sets`, `containers`, `part_volumes`, `part_secrets` and `lan_allow`; `share_requests`, `view_events` and `view_daily` are gone; the Insights file has `events_raw`, `rollups_daily`, `traces`, `spans`, `log_segments` (logs themselves are rotating zstd files, the decision's "ring file" option) and `janitor_runs`. Both blocks load with `sqlite3 :memory:`.
- **Tasks**: added for guests, shared data, the two pixel-editor tests, the runner, backend builds, lifecycle, routing, data and secrets, observability, retention and security, Insights store, traces, log pipeline, the fuzz test, janitor, disk budget, retention test, the Insights and debugging UIs, OTLP and deep links; the P4 analytics overview removed; and the kind renderers, logs and metrics, `/grant`, the workspace UI, backups, Health, the deliverable, the Authentik extras, agent sharing, the Hermes skill, blob GC, `doctor`, WASM backends, `claude.use` and the P1, P2 and P4 exits re-scoped. 112 tasks and 7 milestones, acyclic, no orphans.

**Interpretations, stated.**
- The decision's `https://<name>-part.<domain>/api/*` is what the part's code writes; on the wire it's `/c/<cap>/api/*`, because the prelude prefixes same-host URLs with the cap and the cap check comes first.
- "Guest id kept in the prelude's memory/sessionStorage shim" is implemented as a signed claim inside the link cap, which lives exactly there, so a guest can't pick another guest's id.
- The disk budget defaults to 80 % of the volume, and the 80 % warning and 95 % refusal are fractions of that budget.
- Guests get `403` on `user` and `errors.report` too (the decision says every non-data op). The prelude answers `claude.use("user")` locally for guests, and runtime errors from guest tabs aren't collected.
- "Approved by the admin" for LAN allowlist entries became "set by the admin in the admin UI", per the no-approvals override.

### Slice 1 freeze (2026-10-23)

Inputs: the admin's request to freeze the first shippable slice and write the agent contract, and his Telegram decision.

**What changed.**
- New section "Slice 1: what ships first" after "How to read this plan": scope, the 59 tasks by area, what's after, what moved, decisions for the slice, and the definition of done. A Slice 1 milestone in the task graph.
- **Splits along the slice line** (each remainder named "after Slice 1"): the kind renderers (`svg` and `code` after); tier (a) cards (the titled public card after); the workspace UI (search, diff, old-version preview, comments rail, agent activity, inward rungs, Move to Family, Ask for access and install after); Health (the ops-alert webhook after); the OAuth token service (authorization code, CIMD, DCR and manual clients after); the agent read path (notes, symbols, `render: "text"` and `part_find` after); the event log (`part_watch`, event SSE and webhooks after); deep links (`group`, `everyone` and `admin` after).
- **Moved to where they're needed**: service identities from the device grant to family hub embedding; the `/_rt` `db.*` ops from shared data to `claude.use`; the runner's compose service from the deliverable to the runner; Ask for access from `/grant` to the post-slice workspace UI; `family` and `system` creation made explicit in `provision`; the MCP server no longer waits on comments, the event feed or agent sharing, which add their tools when they land; the §7 suite no longer waits on the Cloudflare adapter, whose §7 lines run with that adapter; the janitor and the retention test no longer wait on the Insights store or the log pipeline, which plug their jobs in.
- **Telegram**: the open item is decided (§2, §6 P1, the real-device pass); the person-bound fallback is rejected.
- **`docs/agent-contract.md`** written (the canonical agent-facing doc, served as `/skill.md`); §4b points to it and adds the registry-match test; §4 gains the Slice 1 op field table.
- 120 tasks and 8 milestones, acyclic, no orphans. Critical path to Slice 1: 15 steps.

**Ambiguities resolved** (the contract needed one answer; the plan now says it):
1. `part_create` returned `{id, name, url, version: 0}` in §3d but the mutation shape in §4: it returns `{id, name, url, head, live, build}`; with files, v1 commits before the call returns and builds after.
2. `request_id` was required in one signature and optional in two: it's **required** on every write (1–128 characters); the CLI generates one. A replay returns `replayed: true`.
3. The error example said "edits[0] applied cleanly" while `part_edit` is all-or-none: the hint now says nothing was saved and the batch is resent.
4. The shared guide was `USAGE.md` and also the body of `/skill.md`: it's `docs/agent-contract.md`.
5. `runtime_errors` was listed as a `part_get` option: the latest 20 are always returned. `part_get`'s default version is `head`.
6. `part_check` defaults (`head`, `mobile`, 8000 ms), its behaviour on a running or failed build, and how base64 is asked for (`include_base64`) were unstated: now specified.
7. How page code hears `data.changed` was unstated: `partifactory.on("data.changed", fn)`. The `window.storage` signatures (Claude's) are written down.
8. `db.*` was in both the P2 shared-data work and the P3 `claude.use` work: it's in P3, with `claude.use("db")`.
9. `part_data` had no field list and no CAS error: fields as in §4, `if_rev`, and `if_match_failed` with `current_rev`.
10. `content` on `part_create` had no file name or kind rule: it needs `kind`, and `files` is never detected.
11. The CLI's `--json` was both an input and an output flag: `--json @file|-` is input, and any `--json` makes output JSON.
12. A part outside the caller's spaces: `not_found`, never `forbidden`.
13. Blob upload responses: `{hash, size}` and `{missing: [hash]}`.
14. The `family` space was "created by `provision`" while `provision` sits outside the slice: Slice 1 has personal spaces only.
15. Public links in Slice 1 get the content-free card in its public-link audience; the titled public card comes after Slice 1.

### Final round (2026-10-23)

Inputs: the steady-state review of `PLAN.md` and the contract (zero structural findings, 33 polish findings F01–F33, eight of them before-start), the dry run of the contract by an agent building the pixel board (22 guesses and ambiguities), and the coordinator's scope decisions. The scope decisions are **provisional, pending the admin** (§8 decisions 39–42).

**Findings applied** (all 33):
- **Before-start.** F01: under `/c/` only blob paths are DB-free; the entry document does one cached indexed read for `pf-boot`. F02: the entry document is templated at serve time (prelude tag, import map, `pf-boot`), so a prelude fix needs no rebuild. F03: mirroring runs in the parent before the child is spawned; the child has no network and its 10 s covers compile only (40 s wall clock). F04: WebAuthn `rp.id` is the apex, assertions are accepted from sandboxed part-host origins, step-up goes through `/api/chrome/v1/<part>/stepup`, and spike item 13 tests it. F06: `text` and `markdown` get a built-in `pf-shell.css`; Jig and block ids plug in later. F11: device approval takes a passkey when the person has one, otherwise the session; registering one is offered once native invites land. F14: the wrapper keeps a `#share=` fragment across the `/grant` hop. F23: the e2e harness (Dex, Caddy, an internal-CA cert for `pf.test`, host resolution per browser) is part of the generic OIDC work; the admin's steps run on Chromium with the virtual authenticator, the guest on WebKit.
- **The rest.** F05 (`/api/chrome/v1/<part>/grant`), F07 (session-only handlers behind the chrome API), F08 (quota defaults; NULL means the default), F09 (`blobs` reserved in code), F10 (daily `kid` by HKDF; `w` is a window end), F12 (snapshots cover shared docs only; on-demand snapshots are outside `keep`; a `reason` column), F13 (`/_pf/` defined, `/_b/` dropped, `refresh` defined, the navigation rule), F15 (`site.toml` with `PF_<SECTION>_<KEY>` overrides), F16 (`max(1, …)` pool), F17 (the manifest lists `part_id` and `n`; JCS), F18 (no FTS triggers in the schema migration), F19 (below), F20 (the generator owns each op section's field table and `Errors:` line), F21 (the library set listed, with `tsx`), F22 (screenshot URL and auth; `check` waits up to 15 s), F24 (`replayed?` on every write's output), F25 (`upload` rejected until assets land), F26 (the Insights Agents page counts deep links, not share requests), F27 (the event shape matches the DDL), F28 (live reload from the wrapper), F29 (find joins with `part_find`), F30 ("once the part has a live version", not "after publishing"), F31 (the §4 table is the full surface), F32 (`sessionStorage` is memory only), F33 (`claude.use("user")` for guests from P3).
- **F19, applied.** Roll back data, Reset data and Freeze moved from the Slice 1 workspace UI to Can-contribute links (the share-sheet controls) and data snapshots (the backend), so data snapshots no longer wait on the workspace UI. With the workspace UI feeding Can-contribute links instead, data snapshots are off the critical path.
- **Two adjustments to the review's fix text.** F12 asked for one `data.changed` carrying every key on a roll back; the contract's event is now per key (`{key, op, by}`), so a roll back emits one per affected key. F33 offered `partifactory.user()`, which no section defines; the guest answer waits for `claude.use("user")` in P3 instead.

**The contract's guesses, resolved** (in `docs/agent-contract.md`, and here where the plan needed it):
1. Tokens: a person connects every agent once; no unattended bootstrap; unattended agents and MCP clients other than Claude Code use a personal token in Slice 1; device-code lifetimes stated (1 h access, single-use refresh tokens, the family lives until the consented lifetime) (§3b).
2. Shared data: `parts.shared_data_writers` replaces `viewers_write_data`; the who-writes table is in §3a and the contract; switching to Can contribute sets `viewers`, and setting `editors` freezes a contribute link.
3. `part_check`: the real prelude over a read-only view of the real shared data (with `data:read`), a discarded overlay, `actions` (click, type, `wait_ms`, `eval_return`; ≤ 20), and `partifactory.ready()` documented (§2, §4c).
4. `window.storage`: `list` with cursor, limit ≤ 1000 and `values`; `getMany` ≤ 500; key, value and rate limits (§3a); `PfError` with catalogue codes and the new code `forbidden_view_only`; `data.changed {key, op, by}` (§3c).
5. `live: "always" | "after_check"`, with `versions.live_gate`, `builds.gate_check` and a `gate` render purpose (§3d).
6. `request_id`: only a committed write uses it up; the format is 1–128 characters of `[A-Za-z0-9._:-]` (§3d).
7. `part_data` `snapshot_list`, `snapshot_restore`, `reset` under `parts:write` and the `edit` role (§4, §5c).
8. The screenshot URL is `/api/v1/screenshots/<id>` with the same bearer (§4c).
9. The contract has a "what the page can do" note (sandbox, storage, network, `csp` errors).
10. What a non-contributor sees: the sign-in page with the content-free card; the admin must be signed in.
11. Nits: `<domain>` explained, `part-cli` is every device-code agent's client id, every op answers 200, `check_only` output, Rule 4 vs the deep link, viewports, `who: "admin"` for non-admin people, and every example written out in full.

**Scope (provisional, pending the admin).** `tsx` moves to right after Slice 1, with the compiler spike kept; the Slice 1 cards ship as two static PNGs and the resvg pipeline moves to the titled-card work; the retention chain stays in and sets Slice 1's date.
From the review's over-scope table, deferred because neither the pixel test nor the definition of done needs them: the versions rail and the bin view (from the Slice 1 workspace UI to the post-slice one) and the deliverable extras (split out of the deliverable and the ingress contract into new post-slice work: nginx and Traefik configs, arm64, SBOM, `cosign`, example CI workflows). Kept, because the definition of done needs them: export and import, the CLI, personal tokens and four-driver conformance, the three kinds, the fork/scratch/history/diff ops, the directory, the policy engine, the admin pin and the v0 placeholder.

**Graph.** 121 tasks and 8 milestones, acyclic, no orphans. Slice 1 is 58 tasks. Edges added: the shared blob host now feeds the prelude (the prelude is a shared blob); the workspace UI feeds Can-contribute links (the controls live in the share sheet); data snapshots feed shared data (the snapshot ops); `tsx` follows Slice 1; mirroring feeds Slice 1 directly (`tsx` no longer pulls it in); the deliverable and the ingress contract feed the deliverable extras, which feed the portability lane. Edges removed: workspace UI → data snapshots, and `tsx` → Slice 1. Critical path to Slice 1: 14 steps (workspace → SQLite layer → schema → manifests and versions → kind renderers → build worker → content listener → shared blob host → prelude → data snapshots → shared data → Can-contribute links → `communal_pixel_editor` → Slice 1), tied with the retention chain and conformance.

**Schema.** `parts.shared_data_writers` (replaces `viewers_write_data`), `versions.live_gate`, `builds.gate_check`, `data_snapshots.reason`, `render_jobs.request` and the `gate` purpose, and comments on quotas, passkeys and idempotency. Both DDL blocks load in `sqlite3 :memory:` (3.54).

### Round 3 (2026-10-23)

Inputs: a brief with two changes, under the admin's rules: tasks live only in beads, never in the plan, and beads are made only at conversion, never in a planning or polish pass; write in the plan's voice; lose no feature, decision or rationale.

**What changed.**
- **New §3h, Updates and advisories.** Three layers, kept apart:
  - partifactory itself, pointing at §5b and §5d, plus a nightly advisory run on main;
  - a part's frontend libraries: a per-build SBOM in the new `build_deps` table, filled from the mirror and the library set, with unparseable entries stored as `unknown` and inline code stated as undetectable;
  - a part's backend: an SBOM of the image at build from one pinned scanner, matched nightly without rescanning, with base-image tags re-resolved.
  Immutability stays: a live part never changes under you, and an update is always a new version.
- **Advisory data is a new adapter**, `AdvisorySource` (§5f). The default downloads the OSV bulk dumps daily, syncs them incrementally and matches locally. The OSV API is the alternative, and `none` is a Health warning. A DB more than 7 days stale turns Health amber, and builds never block on it.
- **Findings are records, not queues.** Severity is shown in context. A finding is an `advisories` row, surfaced as a chrome badge, counts on Health and in Insights, `part_get` `advisories`, a `dep_advisory` warning, a `part.advisory` event, and the ops webhook for critical backend and contribute-link cases. The fixes are `part_edit {rebuild: true, update}`, an Update libraries button (§4a.6, §4a.7), or ignoring it with a reason. Auto-update is `never | patch`, with provisional defaults.
- **The rest of the advisories change:**
  - DDL for `build_deps`, `advisories`, `base_image_checks` and `builds.sbom_hash` (§5e; both blocks still load in `sqlite3 :memory:`);
  - an `advisories/` folder under `/data`, and three §5h rows;
  - the advisories spike (§5);
  - §7 lines, decisions 44–49, and backend SBOMs in P2a;
  - the field-table entries and the contract note in §4.
- **Grounding** (checked 2026-10-23): OSV's per-ecosystem `all.zip`, `ecosystems.txt` and `modified_id.csv`; the licences, which vary by source; `querybatch`, with no auth and currently no rate limits; the Syft + Grype and Trivy SBOM workflows. The dump sizes and the matching speed are unmeasured, hence the spike.
- **Tasks are out of the markdown.**
  - **Checked before removal.** Before §6a was removed, every row's acceptance test (121 tasks and 8 milestones) was checked against its bead. 122 matched word for word, and six differed only in writing "the admin" as "the admin". One, the Insights UI's "a non-admin gets 403", was missing: that bead's acceptance criteria said 404 (and its own notes said 403). The row's test was appended to that bead's acceptance criteria.
  - **Rewritten:** §6a "Task graph" became "§6a Build order". "Slice 1: what ships first" is now by capability: the scope table without its Tasks column, the decisions, "after Slice 1" and the definition of done. "58 tasks", "What moved" and every task id are gone.
  - **Renamed:** every other task id and milestone id in the plan now names the feature or the phase. P0–P5 stay.
  - **Moved:** these change logs moved here, with task ids replaced by feature names. Decision 50.
- **Backend rollback is best effort** (the admin, 2026-10-23). The same model as shartifacts: volumes are shared by every version, rollback restores code only, and whatever a part runs in its containers is its agent's to manage. partifactory guarantees the exact code and the health gate, and runs no migrations or data rollbacks. Rejected: version-tagged volume snapshots and "restore code and data together", because they would only work for engines we special-case. The backend contract (migrate forward, additive changes, read per request, lock writes, back up before a non-additive change) is in §3f. Decision 51.
