# partifactory — plan

> A home for the things a household and its agents make: pages, React tools,
> docs, decks, diagrams, plain text, and small apps with their own backends. Each one is a **part**, with its own host and a name that means something (`photofinder-part.example.com`), and that host is the link you post.
> Every part runs sandboxed in an opaque origin with no cookies, every view is a grant, and every version is a manifest of hashes.
> Any AI can find and use it, through MCP, an HTTPS JSON API, a CLI or a skill file, each with its own scoped, revocable identity.
> Serving is built to be fast and is measured against nginx and HAProxy.

**Credit.** This project takes its idea from **[shartifacts](https://github.com/LaneBirmingham/shartifacts)
by Lane Birmingham (birmo)**, MIT-licensed. It's a reimagining for a different setup, not a fork.
Any code or text adapted from it keeps Lane's MIT notice in `NOTICE` and gets a credit in the README.

**Site config is private.** Everything specific to one deployment (IdP inventory, flow and stage names, group names, account facts, deploy target)
lives in a separate private repo (`partifactory-site`). Secrets come from environment variables or files, whichever the host or CI supplies (§5b). Every hostname is config (`[domain]`); examples use `<domain>` or `example.com`. Nothing site-specific is here, including names:
the owner is "the admin" (the only admin), and everyone else in the household is "a family member".

**Portable by design.** partifactory takes no hard dependency on the stack its first deployment happens to run (Cloudflare, Authentik, Gitea, TrueNAS, Telegram/Hermes, the family hub). Each of those is an **adapter behind config** with a generic, dependency-free default: a plain reverse proxy, generic OIDC, any CI or none, a folder for backups, a generic webhook.
Where this plan describes Cloudflare or Authentik behaviour, it's describing the adapter. §5f has the table, and CI proves it by running the full e2e suite with none of them (the portability lane).

**Defaults are the safe choice.** Every protection is on in the default config, with nothing to set: secure headers, rate limits, quotas, the container hardening template (§3f), a nightly export, the TLS assumptions of the ingress contract, log redaction, and a cap and retention rule on every store that grows (§5h).
Config can tighten or loosen each one within sane bounds. Turning one off takes an explicit config key, and it then shows as a warning on the Health page and in `doctor`.
- *Why:* a household server is set up once and then left alone. Whatever the defaults are is what runs for years, so they have to be the safe ones.

**Agents act; people decide the few things that leave the house.** There are no approval queues. An agent does what its consented scopes allow, directly: create, edit, check, share inward, deploy a backend, set a part's secrets.
A short list of actions is for people only, in the UI: public links, inviting new people, creating groups, LAN access for a backend, and admin. For those, an agent hands the person a deep link that opens the right sheet pre-filled (`part_link_for`, §4).
- *Why:* approval queues cause fatigue and stall agents whenever the person is away, and they protect nothing the scopes don't. An agent asking to set a key already holds the key.

**How to read this plan.** §1–§3e say what partifactory is and how its core works. §3f, §3g and §3h add backends, builds, and updates and advisories. §4–§4c are the surfaces people and agents touch. §5–§5h are the stack, operations, the data model, portability, Insights (analytics and observability) and disk safety.
§6 and §6a are the phases and the build order (the tasks themselves live in beads). §7 is the security checklist, §8 the decisions and §9 the non-goals; §10 points to the change logs in `docs/plan-history.md`. Every architectural choice has a **Why** next to it.
The section right below, "Slice 1", says which part of all that ships first. `docs/agent-contract.md` is the one page an agent reads (§4b).

## Slice 1: what ships first

**Frozen 2026-10-23.** Slice 1 is exactly what the `communal_pixel_editor` acceptance test (end of P2) needs, and nothing more. It cuts through P0, P1 and P2: the phases stay as the full roadmap, and Slice 1 is their first shippable cut.
Adding anything to it takes a decision from the admin. Work that straddled the line was split, so each piece is either in the slice or after it.
- *Why freeze a slice:* the plan is large, and the pixel board exercises every risky layer at once (sandbox, caps, builds, agents, guests, live data, rollback). Shipping that end to end first proves the core before anything is built on top of it.

**What's in it.**

| Area | What ships |
| --- | --- |
| Core store, versions, the bin | blob store, manifests, versions, names, personal spaces, create (incl. fork and scratch parts), history, diff, restore, the 30-day bin, idempotent writes |
| Builds | the build child process; kinds `html`, `markdown`, `text`, plus prebuilt `files`; mirror-on-build (in the parent process); the shared blob host. `tsx` comes right after Slice 1, and its compiler spike stays in |
| Sandboxed part host | path caps, the content listener with the canonical CSP and the serve-time entry document, the prelude and `/_rt`, the trusted wrapper with its chrome API, the content-free card (two static PNGs), gallery thumbnails from the latest render |
| Shared data, live | `window.storage` (personal and shared) over `/_rt`, `data.changed` on `/_rt/events`, `shared_data_writers`, `part_data` for agents (incl. snapshot list, restore and reset) |
| Sign-in | generic OIDC (Authentik is the example deployment's provider, used as plain OIDC) and the e2e harness (Dex, Caddy, a TLS test domain), the claims re-read directory, the policy engine and role × runtime matrix, `/grant`, the admin pin with passkey step-up |
| Agent connections | the token service, device code, Connected agents (personal agent tokens shipped here and were retired on 2026-10-26: short-lived OAuth credentials only) |
| Ops on every surface | the registry; MCP, the HTTPS API, the CLI; discovery and `/skill.md`; cross-surface conformance; `part_edit`; the `part_get` read path; `part_link_for` (`public_link`, `contribute`) |
| `part_check` | the render sidecar, `actions`, the `live: "after_check"` gate |
| Public links | admin-only links on the part host, Can view and Can contribute, guest caps and limits, rate limits, the share sheet's public rung |
| Data undo | data snapshots on by default, roll back and reset, and their share-sheet controls with freeze |
| Export and import | consistent DB copy, nightly export, `import` with the boot-epoch bump, the drill |
| Health | logs and metrics, `/healthz`, `/readyz`, the basic Health page |
| Retention | the janitor for the stores that exist, the disk budget, store caps, the retention test, blob GC, quotas |
| Shipping it | the ingress contract with the Caddy example, the amd64 OCI image + compose file + env contract, the §7 suite and the simulated device matrix (real phones are non-blocking human checks) |
| The acceptance test | `communal_pixel_editor` |

**The ops in Slice 1** are `part_create`, `part_edit`, `part_get`, `part_check`, `part_data` and `part_link_for`. Their exact fields, outputs and errors are in `docs/agent-contract.md` and the field table in §4.

**Decided for the slice.**
- **A person connects every agent once, by design.** There is no fully unattended bootstrap: browser sign-in and the device code both need a person's Allow. An unattended agent (cron, CI, a server) is connected once by device code and refreshes on its own.
- **Agents connect by device code or personal token. OAuth authorization code (with CIMD, DCR and manual clients) comes after Slice 1.** *Superseded (decision 55): authorization code shipped and personal tokens are retired.* Every MCP client that matters for the slice accepts a static `Authorization` header: Claude Code (`--header`), Codex, Cursor and Gemini CLI. Hermes and the CLI use the device grant.
  The pixel test drives MCP with a token, so nothing in it needs a browser OAuth flow. The authorization endpoint is the largest new attack surface in the plan (CIMD fetches behind the SSRF guard, DCR abuse limits, redirect-host display, consent for unknown clients), and none of it is exercised by the test.
  The clients that do need it are claude.ai and ChatGPT custom connectors, which accept only OAuth. They arrive with authorization code, which also brings "Claude Code connects with one passkey tap" (a line of P2's "Done when"). The cost in the slice is one pasted token for Claude Code.
  The slice's token service is built as the OAuth AS's token side, so authorization code adds an endpoint, not a redesign.
- **Personal spaces only.** `family` and `system` are created by `provision`, after Slice 1. A part in Slice 1 lives in its owner's personal space.
- **Unfurls are content-free in Slice 1, and previews come right after it** (decision 53). In Slice 1 a part with a public link unfurls with the content-free card in its public-link audience ("Anyone with the link", no title, name or thumbnail).
  Right after Slice 1, a public link's card shows what the part is: the titled card and an automatic screenshot poster (`og:image`), plus a short muted **`og:video`** clip when the agent chose to make one, on by default for every public link (the admin, 2026-10-23). The gallery shows the same previews to people who can open the part. A private part's unfurl stays content-free, because an unfurler can't open it.
- **Gallery thumbnails in Slice 1 are the latest render** (decision 53): a gallery card shows the screenshot of the newest passing `part_check` (or `after_check` gate) of the live version, kept as a part blob, and the "being made…" frame when there's none. Agent thumbnails, SVG thumbs and clips come right after Slice 1.
- **No search.** Agents address parts by name (the name is in the link they posted). FTS5 search and `part_find` come after Slice 1.
- **Ingress through the contract only.** The example deployment's Cloudflare tunnel runs as a plain proxy that meets the ingress contract (§3), configured by hand, and its two DNS records are made by hand. The Cloudflare adapter and `provision` come after.
- **`window.storage` is the part's data API.** `claude.use("db")` and the `/_rt` `db.*` ops come in P3, with the rest of `claude.use`.
- **Telegram in-app browser (the admin, 2026-10-23).** A 5-minute real-phone test of passkey sign-in in Telegram's in-app browser is in the human checks, done whenever, blocking nothing (decision 52). If it fails, the fixes are: (1) set Telegram to open links in the normal browser, and (2) Hermes' messages hint "open in browser". The person-bound-link fallback is rejected (`docs/plan-history.md`).
- **Final round (2026-10-23), provisional, pending the admin** (§8 decisions 39–42):
  - **`tsx` is right after Slice 1.** The oxc + rolldown kind moves off the Slice 1 path and runs right after it; the compiler spike stays in P0 and in the slice, so a rolldown or oxc problem still shows up first. In Slice 1 agents send `html`; React through the mirrored UMD build (a cdnjs or jsDelivr `<script>` tag) works.
  - **The preview card is two static PNGs.** In Slice 1 both card variants are content-free with fixed text, so the slice embeds two 1200×630 PNGs in the binary. The SVG → resvg pipeline moves to the titled card, where fields first vary.
  - **The retention chain stays in the slice, and it sets Slice 1's date.** The janitor → the disk budget → the fill-past-cap test is as long a chain as the pixel test's own path (§6a). Shipping uncapped stores to a box that's then left alone for years is the failure the plan exists to avoid, so Slice 1 waits for them.
  - **Deferred, because neither the pixel test nor the definition of done needs them:** the versions rail and the bin view in the wrapper (the ops stay: restore, delete and undelete work through `part_edit`); the nginx and Traefik example configs, arm64 images, SBOM, `cosign` and the example CI workflows (Caddy and an amd64 image ship).
- **Round 3 (2026-10-23), provisional, pending the admin** (§8 decision 48): **each build records its library list (`build_deps`, §3h) from the start.** It's nearly free, because the mirror already knows every library, so advisory history exists from day one. Matching, the advisory UI and `part_edit {update}` come after Slice 1.

**After Slice 1** (all of it stays in the plan, in its phase):
- **Backends** (P2a, all of it) and the backend pixel test.
- **Insights beyond Health**: the Insights store, traces, the log pipeline, the no-content fuzz test, the Insights and debugging UIs, the OTLP adapter.
- **Identity extras**: native invites and local passkey accounts, the Authentik spike, directory adapter and provisioner, cleanup lists, `provision`, the Cloudflare adapter, `doctor`, the portability lane.
- **Agent extras**: OAuth authorization code, comments, inward sharing by agents, leases, block ids, notes, `render: "text"` and `part_find`, `part_watch` and event SSE, the other `part_link_for` actions, Jig and `design_system`, the Hermes skill.
- **Workspace extras**: the versions rail and the bin view, search, diff, preview of old versions, comments rail, agent activity, inward sharing rungs, Move to Family, Ask for access, install as app; the source editor and the formatted view for Markdown and `text` parts; live reload; renames.
- **Notifications**: the ops-alert webhook and signed event webhooks for agents.
- **Previews** (right after Slice 1, pulled forward from P4 by decision 53): the card pipeline and the titled public card; automatic screenshot posters, and optional agent-chosen `og:video` clips, for public links (on by default) and the gallery; the `preview` fields and `part_check {record}`.
- **More kinds**: `tsx` (first after the slice), `svg` and `code`, `slides` and Mermaid.
- **Shipping extras**: nginx and Traefik example configs, arm64, SBOM, `cosign`, example CI workflows.
- **Updates and advisories** (§3h): advisory matching against the OSV data, the chrome badge and the counts, `part_edit {update}` and the Update libraries button, auto-update; backend SBOMs come with P2a.
- **Claude-compatibility extras** (P3): `claude.use` (`db`, `user`, `comments`, `artifact`), Tailwind and shadcn, viewer uploads, the agent inbox.
- **family-hub embedding and service identities**, family onboarding, phone editing (P4), and everything in P5.
- **The bench** and the nginx/HAProxy comparison.

**Definition of done.** Slice 1 is done when all of these hold:
1. **`communal_pixel_editor` passes in CI on every PR** (on the e2e harness: Dex and Caddy with an internal-CA wildcard cert for `pf.test`. The admin's sign-in, step-up, share, roll back and freeze run on Chromium with the virtual authenticator, and the signed-out guest runs on a mobile WebKit profile): an agent connected by device code creates the board in one `part_create`, runs `part_check`, posts `https://pixel-part.<domain>/`; the admin does Share → Anyone with the link → Can contribute → Copy (4 interactions + 1 biometric); a signed-out browser paints and the admin's open tab shows it within 1 s; the admin rolls back one snapshot and freezes the link, and the guest's next write is refused.
2. **The same flow passes on the device matrix** (decision 52): Playwright Chromium (mobile emulation), WebKit (mobile profile) and Firefox in CI, and Mobile Safari in the iOS Simulator, run locally on a Mac with its results attached as evidence.
3. **Every acceptance test of the work in Slice 1 passes** (they live in its beads), including cross-surface conformance over MCP, HTTP, the CLI and `/skill.md` with OAuth and device-code tokens.
4. **The §7 lines for what exists pass from outside**, the guest, agent and render-sidecar lines included. No agent token, scope or op can make a public link.
5. **Export and import round-trip**: the nightly export imported into a fresh container brings back every part, version, data document and snapshot, and every cap minted before it is dead.
6. **Every store that exists is capped** and passes its fill-past-cap case; the Health page shows each one, the backup layers and the disk budget; no `[unsafe]` key is set.
7. **`/skill.md` is `docs/agent-contract.md`**, and the test that regenerates its op reference from the registry passes.
8. **Not in the gate: real devices.** The by-hand run on the example deployment (a real agent, the admin on their phone, a signed-out phone on mobile data) and the Telegram passkey test sit in the human checks, which never block Slice 1 (decision 52). A failure there becomes a bug for the swarm.

---

## 1. What shartifacts gets right, and what doesn't fit here

I read the repo (Go server ~21k lines, plain JS UI, docs). The parts worth keeping:

| Keep (the good ideas) | Why |
| --- | --- |
| Agents connect over **MCP**. `edits` use `{old_str,new_str}` with a `base_version` compare-and-set | Same shape as Claude's artifact tools, so agents already know how to use it |
| **Immutable numbered versions**, restore = append | You never lose work, and you can say "what changed?" |
| **Back up the data folder**: one folder holds everything | Backup is whatever the host already does. No second system to run (§5c) |
| **Claude-artifact compatibility**: `window.storage`, `claude.use("db"/"user"/"comments"/"artifact")`, React/TSX + common libs | Anything Claude writes runs here unchanged, including state that persists and pages that save themselves |
| **Comments addressed to the agent** (`to_claude`) + `watch` | Lets you comment from your phone and have the agent fix it. partifactory adds live reload (P2) and an agent inbox (P3) |
| **Separate share surface** that only answers to tokens | A misconfigured edge can't expose the workspace |
| **Per-agent revocable tokens**, removed before anything reaches app code | Each agent can be cut off on its own |
| **Runtime errors reported back to the agent** (`runtime_errors`) | The agent finds out its page is broken without needing a browser. partifactory adds `part_check` screenshots (§4c) |
| `get_design_system` so agents make on-brand pages | Pages look consistent without extra prompting (Jig, §4a.8) |
| **Opaque-origin `sandbox`** for artifact documents | It's the one mechanism that takes cookies away from untrusted page code. partifactory keeps it, and serves it per part (§2) |

How shartifacts does agent auth, for reference: there's an optional owner password. With it on, each agent gets a static bearer token (`shx_…`)
made in the UI or with `shartifacts auth token NAME`, sent as `Authorization: Bearer` on a stateless MCP endpoint. The token can do
everything except manage the password and tokens, including making public share links (`manage_app` op `share`). With the password off, network position is the only check.
Tokens never expire, so you revoke them by hand. partifactory keeps "one token per agent, revocable" and replaces how the token is obtained and what it can do (§3b).

What doesn't fit the admin's setup:

| shartifacts assumes | The admin's reality | So partifactory… |
| --- | --- | --- |
| Trust comes from the **network** (LAN/tailnet IPs). Sign-in is optional | Everything comes in through a reverse proxy or tunnel (a **Cloudflare Tunnel** today), so every request comes from the proxy and IP trust means nothing | Trust comes from **identity**: an OIDC client of **any** provider (Authentik in the example deployment), a per-part policy engine and in-app invites (§3a) |
| One owner password, "Who are you?" is a name picker | The IdP already knows the household | Real principals: people from the IdP, agents acting *on behalf of* one person, each with its own connection (§3b) |
| Static all-powerful agent tokens, which can share publicly | Agents read untrusted web content and can be prompt-injected | **Scoped, consented, space-limited agent identities**. Agents act directly within their scopes, including sharing *inward* to people who already have accounts. They can never make public links or invite outsiders; for those they hand the person a deep link (§3b) |
| Anyone on the LAN sees everything | Reachable from the internet, with per-person audiences | **Sharing is a security boundary.** Audiences per part and per space, with a live sentence saying who can see it (§3a) |
| Opaque-origin `sandbox` iframes + postMessage bridge + hub-managed localStorage + HMAC app sessions, all on one host | `*.example.com` is covered by the free cert, and parts share a site with the IdP, home automation, the forge and other household services | **Each part gets its own hostname (`<name>-part.example.com`) *and* an opaque origin**. Every part document is served with CSP `sandbox` (no `allow-same-origin`). Part hosts never set or read cookies. The hostname scopes the CSP, the capability and the edge rules (§2) |
| MCP is the only agent door | The admin wants any AI, on any surface, to be able to use it | **One operation registry** rendered as MCP, an HTTPS JSON API with OpenAPI, the `part` CLI and a downloadable skill file (§4b) |
| JSON-file-per-artifact holding every revision, a hand-written search index | — | **SQLite** (metadata, FTS5, comments, db docs, events, audit) + a **content-addressed blob store**, both under `/data` |
| Full-stack apps with a Go runner sidecar + Landlock | The admin wants small apps with real backends ("Docker, whatever"), with guardrails | **Backends are in v1 (P2a), as containers.** A part is a page plus, optionally, containers it brings: a Dockerfile, a registry image or an uploaded image. Only a small runner sidecar holds the Docker/Podman Engine API, and it starts containers from one hardened template (§3f) |
| Every library vendored, offline | We want both: no third-party runtime loads, and no hand-maintained vendor list | **Mirror-on-build**: CDN URLs in a part are fetched once at build time, pinned by hash and served from our own blob host (§2) |

## 2. The core idea

Five concepts. Everything else is built from them.

1. **Blob**: bytes, named by `sha256`, stored once (`/data/blobs/ab/cd/<hash>`). Whether a blob is shared or private is a property of the **reference**, not of the blob:
   - **Shared blobs**: third-party libraries, fonts, the runtime prelude, the part-host wrapper and Jig. They're listed in `shared_blobs`, aren't private to anyone, are served `public, immutable` from `blobs-part.<domain>/b/<hash>` (so any CDN in front may cache them forever), and are cached once across all parts.
   - **Part blobs**: a part's own files, build output and media. They're served **only from that part's host**, under the part's capability, and only if the part's live (or requested) build references them.
     Browsers cache them `private, immutable`, and **no shared cache may hold them**, so revoking access really revokes it and nothing private sits in a CDN.
   - *Why per reference:* one hash can be both (an agent uploads its own copy of `react.production.min.js`). Serving rules then follow what refers to it, not an accident of upload order.
2. **Part version**: an immutable **source manifest** `{part_id, n, kind, entry, files: {path → {hash, size, mime}}, meta, parent, author, actor, created_at}`, canonicalised as JCS (RFC 8785).
   Its hash (so it covers the part id, `parent` and `n`) is the version id, and `n` is the readable counter agents use. A one-line edit to a 40-file part stores one new blob plus one tiny manifest.
   Each version has zero or more **build records** `{toolchain, status, output: {path → {hash, br_hash, type}}, import_map, warnings, errors}`, keyed by `hash(manifest, toolchain)`.
   **What's live is a pair** `(version n, build)`. `head` is the newest committed version, and `live` is the newest one with an `ok` build. They differ while a build runs or after a failed one.
   - *Why build records:* compile output depends on the toolchain. Keying on the manifest alone would serve stale output after an upgrade. A toolchain upgrade rebuilds live parts in the background, diffs the output against the golden corpus, and never switches what's live on its own.
3. **Grant**: who may do what to which part or space, decided by the policy engine (§3a). It covers people, groups, the audience group, public links and agents.
4. **Capability** ("cap"): a signed, host-bound token in the URL path that carries a decided grant to a part's host, so the content listener never needs the database to authorise a request (§2 Serving).
5. **Space**: a named container with grants of its own, a quota, and one owner. Every part is in exactly one space. Every person gets a personal space, and `provision` creates `family`.
   Effective access = the highest role from the part's own grants and its space's grants, **inherited live**. **Moving a part to Family is how most household sharing happens** (§3a).
   - *Why live inheritance:* "everything in Family is the family's" has to stay true when someone joins the family later. Copying grants at creation would quietly leave new members out.

### Mirror-on-build, instead of a CDN allowlist

When a part references `https://cdnjs.cloudflare.com/…`, jsDelivr `/npm/…`, unpkg, esm.sh or Google Fonts, the **build step** fetches it once, stores it as a shared blob and rewrites the URL **in the build output only**.
At runtime parts load **nothing from third parties**, so the CSP has no third-party origins, and Claude-written HTML with CDN `<script>` tags still works unchanged.

- *Why in the build, not the source:* the agent's next `old_str` anchor contains the original CDN URL. Rewriting the source would break it. The source version commits before any fetch, so a slow CDN never slows a save.
- **Rewritten**: HTML `script[src]`, `link[href]` (stylesheet, preload, modulepreload, icon), `img/source/video/audio[src|srcset]`, `import`/`export … from`/literal `import("…")` in inline and fetched module scripts,
  CSS `@import` and `url()` in inline and fetched CSS (recursively), and root-relative paths inside fetched files from the same CDN host. `sourceMappingURL` comments are stripped.
  URLs built at runtime (`fetch(cdn + x)`) aren't rewritten. CSP blocks them, and they show up in `runtime_errors` as `blocked_load`.
- **Allowlist** (site config): `cdnjs.cloudflare.com`, `cdn.jsdelivr.net` **`/npm/` paths only** (its `/gh/` paths serve any GitHub repo, which would make the edge-cached blob host into arbitrary file hosting), `unpkg.com`, `esm.sh`, `fonts.googleapis.com` + `fonts.gstatic.com`. https only.
- **Egress guard**, shared with CIMD fetches (§3b): we resolve DNS ourselves and refuse private, loopback and link-local addresses, allow ≤ 3 redirects and only within the allowlist, a 10 s timeout and ≤ 10 MiB per asset. Per build: ≤ 200 assets, ≤ 50 MiB, depth ≤ 8, 30 s total.
- **SRI**: when the source has `integrity=`, a mismatch fails the build. Otherwise the fetched hash is recorded in the build record.
- **Floating versions** (`react@18`, `@latest`, none): resolved to the pinned URL the CDN redirects to, recorded as `(requested → pinned → hash)` in `mirror_cache`, and reported as a `cdn_unpinned` warning.
  An exact version is cached forever. A floating one is re-resolved at most once per 24 h, and the resolution is written into the build, so every build is reproducible from its record.
- **Fonts**: Google Fonts CSS is fetched with a fixed modern User-Agent (so it returns woff2), then each `url()` it references is mirrored one level deep. KaTeX and Font Awesome webfonts come through the same pass.
- **One library set**: if a mirrored URL is a package in the bundled set with the same major version, the build maps it to the bundled shared blob, so CDN `react@18` and bare `react` resolve to one module. A different major is mirrored as is, with a `duplicate_library` warning.
  The set is `libset.toml` (its version is `library_set_version`): `react` and `react-dom` 18.3, `lucide-react`, `recharts`, `d3`, `lodash`, `papaparse`, `mathjs`, `three`, `chart.js`, `tone`, `plotly.js` and `xlsx`, i.e. shartifacts' compatibility matrix; shadcn and Tailwind join in P3. It arrives with `tsx` (after Slice 1); until then every mirrored URL is mirrored as is.
- **Import maps** for HTML parts with `<script type="module">` and for TSX bundles are **frozen into the build record**, so a later release never changes which library an old part loads. `part_edit {rebuild: true}` opts a part into the current set, and `rebuild` with `update` re-resolves its libraries within a range (§3h).
- **Failure**: the source version still commits, the build ends `failed` with `{code: "mirror_failed", url, reason: "not_allowlisted" | "private_address" | "too_large" | "timeout" | "integrity_mismatch" | "http_<status>"}`, and `live` doesn't move.

### Kinds

Detected from the entry file. `kind` overrides. **Every kind is built**, so `live` only ever moves through a successful build.

| kind | entry | build work | served as |
| --- | --- | --- | --- |
| `html` | `index.html` | mirror CDN refs, sandbox rewrites (root-relative → relative, `crossorigin` on `<img>`), brotli for every file but the entry | the HTML, with the prelude tag, import map and `pf-boot` inserted at serve time |
| `tsx` (right after Slice 1) | `App.tsx`/`.jsx` (default export) | oxc transform, rolldown bundle against the shared library set, a generated HTML shell (`#root`, Tailwind per §6 P3) | generated shell, with the prelude inserted at serve time |
| `markdown` | `*.md` | comrak render into the built-in shell (`pf-shell.css`, a shared blob); Jig styles and block ids (P2), Mermaid/KaTeX (P4) | rendered HTML shell |
| `slides` | `*.md` with `---` separators | as markdown + the slide runtime (block = slide) | slide shell |
| `svg` | `*.svg` | none beyond brotli (it's sandboxed like everything else) | the SVG as a sandboxed document |
| `text` | `*.txt` (or `kind: text`) | none beyond brotli, so it's instant | the text as is, in the built-in shell (`pf-shell.css`; Jig typography from P2) with a monospace toggle and line numbers for line-anchored comments and edits |
| `code` | any other text file | syntect highlight | read-only viewer shell |
| `files` | a built bundle (e.g. `vite build`) | sandbox rewrites only, no bundling | as uploaded |

`part_create {content}` is shorthand for a one-file part and needs `kind`; the file is named `index.html`, `App.tsx`, `index.md` or `index.txt`. `files` is never detected: a bundle is sent with `kind: "files"` and an `index.html` entry.
A part that needs real npm packages is built by the agent locally (`vite build`, with `base: './'`) and uploaded as `files`.
Any kind can also carry a **backend**: a `compose.yaml` or `Dockerfile` at the root, or an image reference (§3f). Who builds what is in §3g.
- *Why `text` is its own kind:* notes, lists and logs are the most common thing a chat agent makes. Rendering them as is, with line anchors, is faster and more faithful than wrapping them in Markdown.

### Serving a part: sandboxed, cookie-free, capability in the path

**No part host ever sets or reads a cookie.** Every part document (HTML entry, any navigable HTML file, an SVG served as a document) carries the canonical CSP below, including `sandbox` without `allow-same-origin`.
The document runs in an **opaque origin**: `document.cookie`, `localStorage`, `indexedDB`, `caches` and service workers are unavailable to it, and cookie tossing onto `.<domain>` is impossible at the platform level.
Cookies exist only on the workspace host (`parts.<domain>`; `parts.example.com` in the example deployment), for sessions.
- *Why:* agent-written JS is untrusted by default. The sandbox removes the cookie attacks without buying a second domain (§3, decision 10), and it's the same isolation Claude and shartifacts use for artifacts.

**The canonical part CSP.** One header, written once here. §3 and §7 refer to it. `<blob>` is `https://blobs-part.<domain>`, `<self-host>` is this part's own `https://<name>-part.<domain>` (where its wrapper lives), `<workspace>` is `https://parts.<domain>`,
and `<embedders>` is the site-config list `[embed] frame_ancestors` (empty by default; the example deployment lists the family hub).

```
Content-Security-Policy:
  sandbox allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-modals
          allow-downloads allow-top-navigation-by-user-activation;
  default-src 'none';
  script-src 'self' 'unsafe-inline' 'unsafe-eval' blob: <blob>;
  style-src 'self' 'unsafe-inline' <blob>;
  img-src 'self' data: blob: <blob>;
  media-src 'self' data: blob: <blob>;
  font-src 'self' data: <blob>;
  connect-src 'self' <blob>;
  worker-src 'self' blob:;
  frame-src 'self';
  form-action 'self';
  base-uri 'none';
  object-src 'none';
  frame-ancestors <self-host> <workspace> <embedders>
Referrer-Policy: no-referrer
X-Robots-Tag: noindex, nofollow
X-Content-Type-Options: nosniff
```

- *Why `'unsafe-inline'` and `'unsafe-eval'`:* the threat is the part itself, and its author is an agent. Blocking the author's inline script protects nobody and breaks nearly every Claude-written page.
  Here CSP is an **egress** policy: it keeps fetches, images, forms and frames on the part's own host and the shared blob host. The opaque origin is what keeps the viewer's credentials away from the page.
- *Why these sandbox tokens:* `allow-popups-to-escape-sandbox` lets a `target=_blank` link open a normal external site (sign-in pages, maps) instead of a broken, storage-less popup. That popup is another site anyway.
  `allow-top-navigation-by-user-activation` lets a tapped `target=_top` link work in a frame, while script-driven top navigation stays blocked.
- Responses to a request with `Origin: null` echo `Access-Control-Allow-Origin: null` and `Vary: Origin`, **never** `Access-Control-Allow-Credentials`.
  **Part responses never carry `Cross-Origin-Resource-Policy`**: it would block the part's own no-cors loads from its opaque origin. The workspace host never answers `Origin: null` with CORS headers.
- *Why `frame-ancestors` lists the part's own host explicitly:* the part's wrapper (below) frames it from that host. `'self'` in a sandboxed document's policy is a spike item (§5), so the host is spelled out.

**Capabilities live in the path.** Part content URLs look like `https://<name>-part.<domain>/c/<cap>/<path>`. Relative URLs inherit the prefix. The cap is never in a cookie. The part host's root is the wrapper page, not content (below).

- **Format**: `v1.<kid>.<payload>.<mac>`, all base64url, ≤ 300 bytes. `mac = HMAC-SHA256(key[kid], "v1." + kid + "." + payload)`, truncated to 16 bytes.
  `payload` is canonical CBOR: `{p: part_id, h: host_label, s: subject ("user:u_…" | "link:l_…" | "conn:c_…" | "render"), r: role (0 view … 3 manage), ro?: read_only, dw?: shared-data write (a "can contribute" link), g?: guest id,
  e: part_epoch, m: minter ("sess:<id>" | "link:<id>" | "conn:<id>" | "render"), me: minter_epoch, b: boot_epoch, w: window end (unix minutes), v?: pinned version_id, d?: draft id}`.
- **Keys**: `kid` is the UTC day number, and `key[kid] = HKDF-SHA256(PF_CAP_HMAC_KEY, info = "pf-cap/" ‖ kid)`. The verifier accepts `kid` and `kid − 1` (at least 25 h, longer than any 13 h cap). Rotating the root secret rotates every key, and a retired `kid` is never accepted again.
  Windows are 12 h for `user`, `link` and `conn` subjects and 60 s for `render`.
- **Deterministic and windowed**: there's no random nonce. The same subject, part, epochs and 12 h window give the **same cap**, so `private, immutable` part blobs stay in the browser cache across repeat opens.
  A cap is valid until its window ends. A cap minted in the last hour of a window uses the next window, so a fresh cap always has at least an hour left.
- **Verify** (no DB): known `kid` → MAC → `h` equals the request's host label → `e ≥ epoch[part]` → `me ≥ epoch[minter]` → `b ≥ boot_floor` → window not over.
- **Revocation stays instant** through in-memory epochs, persisted in SQLite: per part (any revoke or unshare), per minter (revoking a session, a link or an agent connection), and global.
  A **boot-epoch floor** is bumped on every start and every import, so a restored or rolled-back database can never resurrect a revoked cap. The cost is one silent re-grant per open tab after a deploy.
- **Hand-off**: `/grant?part=<name>&path=<path>&mode=wrap|direct` on the workspace host (path ≤ 2 KiB, must start with `/`; the host is derived from the name, so it **can't act as an open redirect**)
  authenticates with the workspace session (or sends the person to sign in), asks the policy engine and mints the cap. In `wrap` mode (the default, used by the wrapper) it 302s to `https://<name>-part.<domain><path>#pf=<cap>`.
  In `direct` mode (a standalone `/c/` URL that lost its cap) it 302s to `https://<name>-part.<domain>/c/<cap><path>`. There's no one-time code: it existed only to set a cookie.
- **Errors on the part host**: a top-level navigation to `/c/…` with an expired window or a stale epoch → 302 to `/grant?mode=direct` (silent if still allowed). A subresource → `401` with `X-PF-Reauth: 1`.
  A request is a navigation when `Sec-Fetch-Dest` is `document` or `iframe`, or, without `Sec-Fetch-*` headers, when `Accept` includes `text/html`.
  An unknown `kid` or a bad MAC → `404`, which looks the same as a part that doesn't exist.
- **Framed expiry**: a framed part never navigates itself to `/grant`. A subframe navigation from an opaque origin is cross-site for `SameSite=Lax`, so the session cookie wouldn't be sent and a login page would render inside the frame.
  The prelude posts `{type: "pf:regrant"}` to the parent. The part's wrapper answers it with a credentialed `POST https://parts.<domain>/api/chrome/v1/<part>/grant` (same-site, so the Lax session cookie goes along) and sets a freshly minted `src`, with no navigation.
  The workspace's own frames (editor preview) and an embedder's `pf-embed.js` do the same. Standalone documents reload through `/grant`, since top-level GET navigations do carry Lax cookies.
- **Leaked URLs**: the cap sits in the address bar and history only when someone opens `/c/…` standalone. A URL copied out deliberately gives its holder the copier's role on that one part until the window ends (≤ 13 h), unless an epoch is bumped first.
  The **default human view is the part host's root, `https://<name>-part.<domain>/`**, whose wrapper frames the content, so the address bar there never shows a cap. Logs redact `/c/<…>/` by route and never see the `#pf=` fragment.
- **Signing keys** rotate by `kid`, daily (see Keys above). Yesterday's key stays valid (≥ 25 h, longer than any cap's life), then drops out. Public links are random DB tokens, not HMAC-signed, so rotation never affects them.

**Runtime calls: `/_rt`, one simple request.** The prelude calls `POST /c/<cap>/_rt/v1` with `Content-Type: text/plain` and a JSON body `{op, args}`.
That's a CORS-simple request, so there's **no preflight round trip through the proxy**. Live events use `EventSource('/c/<cap>/_rt/events')`: `pf:version` for live reload and `data.changed` when anyone else writes the part's shared data, so communal pages update live.

- **Auth** is the cap in the path, bound to this host. The server accepts `Origin: null` (or the part's own origin for `unsandboxed` parts) and rejects any other non-null `Origin` before reading the body.
  If `Sec-Fetch-Mode` is present it must be `cors`. **There is no `Sec-Fetch-Site` check**: requests from an opaque origin are `cross-site` even to their own host, so `same-origin` would reject every legitimate call.
  CSRF is moot because there's no ambient credential to ride. A sibling part can't reach this host (`connect-src 'self'`), and its cap verifies only for its own host.
- `/_rt` ops: `storage.get/getMany/set/delete/list` (personal and shared), `db.*`, `comments.*`, `artifact.publish`, `user`, `errors.report`, `refresh`. The **data ops** are `storage.*` and `db.*`. `refresh` returns a fresh cap for the same subject (the credentialed regrant for standalone documents); it isn't a data op. What each role may do is the role × runtime matrix in §3a.
- A part with a backend is also reached under the cap: `/c/<cap>/api/*` (§3f).

**The entry document is templated at serve time.** The build output stores the entry HTML as written (sandbox rewrites applied, brotli for every *other* file). At serve time the content listener inserts `<script src="<blob>/b/<prelude-hash>">` first in `<head>`, the import map (with the cap prefix) and the `pf-boot` snapshot, then compresses on the fly.
So a prelude fix ships with a deploy and needs no rebuild: `toolchain` in the build cache key excludes the prelude. Every other HTML file a part serves gets the same prelude tag the same way.

**The prelude** is a shared blob, first in `<head>` of every served HTML document. It:
- installs `window.storage`, `claude.use(...)` and `window.partifactory`: `ready()` (call it when the page has finished its first render; `part_check` stops waiting then), `basePath`, and `on("data.changed", fn)`, which calls `fn({key, op: "set" | "delete", by: {kind: "person" | "agent" | "guest", name}})` once per changed key whenever another tab or person writes the part's shared data;
- redefines `localStorage` as a synchronous in-memory `Storage` seeded from the bootstrap snapshot (≤ 256 KiB of the person's state for this part, in `<script type="application/json" id="pf-boot">`), written behind to personal state on `/_rt` (debounced, plus a `pagehide` flush via `sendBeacon`);
  `sessionStorage` is memory only; `document.cookie` is an inert in-memory string; `indexedDB` and `caches` are `undefined`, so feature detection falls back. Public-link viewers (no person) get memory-only storage;
- wraps `fetch`, `XMLHttpRequest.open` and `EventSource` to add the `/c/<cap>/` prefix to same-host root-relative URLs, wraps `Worker`/`SharedWorker` to fetch the script and start it from a `blob:` URL, and sets `crossOrigin = "anonymous"` on script-created `Image`s;
- reports uncaught errors, unhandled rejections and blocked loads to `runtime_errors`, and handles `pf:regrant`, `pf:version` (live reload, §3c) and the wrapper's postMessage schema.

Root-relative URLs are covered four ways: the build rewrites root-relative `src`/`href`/`srcset`/CSS `url()` to relative form; the served import map adds `"/": "/c/<cap>/", "/c/": "/c/"`; the prelude prefixes fetches; and a build lint warns (`root_relative_url`) about string literals none of these can reach.
**Semantics change, stated plainly:** `localStorage` becomes "personal, synced across your devices", not "per device". That's what Claude-written code expects anyway, because claude.ai artifacts can't use `localStorage` either.

**Known losses under the sandbox**: service workers on part hosts, remembered device permissions (camera, microphone, geolocation), persistent IndexedDB, `<a download>` filenames for own-host URLs (served as `Content-Disposition` on `?dl=1` instead).
Installing a part on a home screen goes through its wrapper at the part host's root, which serves a per-part web app manifest (scope `/`) and frames the part. Each part is its own origin, so each installs as its own app.

**Escape hatch: `unsandboxed`.** An admin-only per-part flag, off by default, drops `sandbox` for that one host. The part gets a real origin: camera, geolocation, service workers, `new Worker("./w.js")`.
It still has no cookies (caps stay in the path), but it regains the ability to toss cookies onto `.<domain>`. It also becomes **same-origin with its own wrapper**, so its code can read the wrapper's DOM and sessionStorage, which hold only that part's cap.
That's accepted: an unsandboxed part already runs with its viewer's role on itself and nothing more. What it must not get is the viewer's workspace session, so the workspace's chrome CORS (below) **never allows an unsandboxed part's origin**; its wrapper falls back to opening Share and Comments on the workspace host.
The share sheet shows the flag in danger red, the audit log records it, and the admin area lists every such part.

**Why keep one hostname per part**, now that the sandbox, not the host, separates parts:
1. `connect-src 'self'` and `form-action 'self'` mean *this part only*. On one shared host, `'self'` would cover `/api`, `/mcp` and every other part's `/_rt`.
2. Caps are host-bound, and host → part is a pure function of the hostname, so the content listener needs no routing table.
3. Per-part cache rules, on any proxy or CDN: "bypass cache on `*-part` except `blobs-part`" is two host rules, not path patterns.
4. The `unsandboxed` escape hatch needs a real origin of its own, which a shared host can't give without handing over the workspace's origin.
5. `photofinder-part.example.com` is a name a person remembers and an agent can say in chat, and it's the link itself (below).

The cost that stays: the `*.<domain>` wildcard catches typos for future services, which is metered (§5d).

### The link to post: the part's own host

Agents and people post **one link per part: `https://<name>-part.<domain>/`** (`https://photofinder-part.example.com/` in the example deployment). It follows the live version.

**The part host's root is a small trusted wrapper page.** It's partifactory's own code, served by the content listener, and it is **not sandboxed**. It holds the thin Stockroom chrome (title, audience chip, Share, versions, comments, agent activity, §4a.6)
and frames the sandboxed part content at `/c/<cap>/…` on the same host. The wrapper's script and styles are shared blobs; its HTML is generated per part and carries no part content unless the part has a public link (below).
The wrapper is served at `/` and at any other path outside the reserved prefixes (`/c/`, `/l/`, `/_pf/`), and passes that path on to the frame, so deep links work. `/_pf/` is the wrapper's own static prefix (`/_pf/manifest.webmanifest`, icons).

**The flow on first open:**
1. The browser loads `https://<name>-part.<domain>/`. The wrapper looks for a cap in the `#pf=` fragment, then in its own `sessionStorage`. With none, it does `location.replace("https://parts.<domain>/grant?part=<name>&path=<path>&mode=wrap")`.
   Before hopping, the wrapper stores any non-`pf` fragment (`share=…`) in its `sessionStorage` under the part name and re-applies it after the cap is read, so an agent's deep link survives first open and sign-in.
   *This hop is client-side, by necessity:* the server never sees the fragment, so it can't tell a cap-carrying load from a bare one, and it serves the same wrapper HTML to both.
2. `/grant` runs where the session cookie lives. Signed in and allowed: it 302s to `https://<name>-part.<domain>/<path>#pf=<cap>`. Signed out: it shows the sign-in page with the content-free card. Signed in but not allowed: `403 · NOT SHARED WITH YOU` with **Ask for access**.
3. The wrapper reads the fragment, calls `history.replaceState` so the address bar shows the clean link, keeps the cap in its own `sessionStorage` (so a reload needs no hop), and sets the iframe `src` to `/c/<cap><path>`.
- **The fragment never reaches a server or a `Referer`**: browsers strip fragments from both, and the wrapper removes it before any other code runs.
- **Cost, stated:** the first open in a tab costs **one extra redirect hop** (wrapper → `/grant` → wrapper) compared with serving content straight away. Reloads in the same tab reuse the stored cap. Expiry and deploys re-grant over a credentialed fetch, with no navigation (§2 Serving).

**Chrome actions** (share, comments, versions, agent activity) call a small chrome API on the workspace, `https://parts.<domain>/api/chrome/v1/<part>/…`, with `fetch(…, {credentials: "include"})`.
The part host and the workspace are **same-site**, so the `SameSite=Lax` session cookie is sent. The workspace answers CORS for **exactly the part-host origins** and nothing else:
- `Access-Control-Allow-Origin` echoes the request's `Origin` only if it matches `https://<label>-part.<domain>`, `<label>` is an active part name (not `blobs`, not a redirecting old name), and that part is **not `unsandboxed`**. Otherwise no CORS headers at all.
- `Access-Control-Allow-Credentials: true`, `Vary: Origin`, methods `GET, POST`, headers `Content-Type`, preflight `max-age` 600. Never `*`, never `null`.
- **Origin binds to part**: a request from `https://photofinder-part.<domain>` may only touch `/api/chrome/v1/photofinder/…`. The chrome API carries no other ops, and every call still goes through the viewer's session and the policy engine.
  It's the wrapper's UI backend, like the workspace's own pages, not an agent surface: each chrome call is a thin view over a registry op (`part_share`, `part_comments`, `part_get`, `part_data`), or, for the people-only actions that have no op on purpose (public links, step-up), over the workspace's session-only handlers that the share sheet on `parts.<domain>` also uses. Nothing an agent may do exists only there.
- **CSRF**: chrome-API writes require `Sec-Fetch-Site: same-site` (or `same-origin`) and an allowed `Origin`; a `Content-Type: application/json` body forces a preflight, so a plain cross-site form can't reach them.
- *Why this is safe:* only the wrapper's own code can send a request whose `Origin` is a sandboxed part's host. The part's content runs in an opaque origin, so its requests carry `Origin: null`, which the workspace refuses (§2), and it can't script the wrapper.

**The part host still never sets or reads a cookie.** The wrapper's state is a `sessionStorage` entry on the part host, and the session cookie stays on the workspace host.
The wrapper has its own strict CSP (no `sandbox`): scripts and styles only from the shared blob host, `connect-src` only the workspace, `frame-src 'self'`, `frame-ancestors` = `<workspace> <embedders>`.

| Who opens it | What they get |
| --- | --- |
| Signed in, with access | the wrapper with their own role, the part framed inside it (one hop on first open) |
| Signed in, no access | `/grant` shows `403 · NOT SHARED WITH YOU` with **Ask for access** (a note to the owner, §3a). The title isn't revealed |
| Not signed in | `/grant` shows a sign-in page carrying the **content-free card** (§4c), then returns to the part |
| An unfurler or any client without JS | the wrapper HTML, whose og tags are the **content-free card**, or the **public card** if the part has an active public link, plus a `<noscript>` "Sign in to open" link |

- *Why one link:* every other shape (per-version previews, minted links) goes stale after the first fix, and agents would have to know which one to post.
  With the part host, the access decision happens when the link is opened, by whoever opens it, and the link is the name an agent already knows.
- **`/p/<name>` on the workspace is only a redirect** (302 to `https://<name>-part.<domain>/`), kept so old links and typed paths work. The workspace gallery links straight to part hosts.
- **Embedding**: an embedder in `[embed] frame_ancestors` frames the part host root (the wrapper, with its chrome) or `/c/` directly with a server-minted cap and `pf-embed.js`, which handles `pf:regrant` (§4).
- **Public links are separate and admin-only** (§3a): `https://<name>-part.<domain>/l/<token>`, on the part host. They're made once the part has a live version, have **no expiry by default** (optional expiry), show open count and last opened, and are revocable.
  The `/l/` page is the wrapper in public mode (title, no share or comments unless opted in) carrying the public card, and a JS hop: `POST /l/<token>/go` on the same host returns a cap with subject `link:<id>`, and that call is what counts an open.
  There's no `<meta refresh>` and no target URL in the HTML, so a crawler finds nothing to follow. The root `/` and `/l/` read the DB through a cache. Under `/c/`, blob paths are DB-free (cap check + manifest LRU); the **entry document** does one indexed read (the `data_docs` primary key for this person's `__storage` personal docs, cached per cap) to inline `pf-boot`, and `/_rt` reads and writes through the pool. For `link:`, `render` and guest subjects the snapshot is empty.
- **The response never depends on the User-Agent.** Bots and people get byte-identical HTML. What it reveals is decided by the audience: the root carries the content-free card unless the part has an active public link, in which case its title is already public and the root carries the public card; `/l/` always carries the public card.
  *Why:* unfurl-bot detection is unreliable. iMessage fetches from the sender's device, and several apps use browser-like UAs.
- **Telegram in-app browser and passkeys: decided (the admin, 2026-10-23).** A 5-minute test on a real phone, in the non-blocking human checks (decision 52), checks passkey sign-in in Telegram's in-app browser. If it fails, the fixes are (1) set Telegram to open links in the normal browser, and (2) Hermes' messages hint "open in browser".
  There is no fallback link: the person-bound link considered in round 2 is rejected (`docs/plan-history.md`).

## 3. Topology

```
                Ingress: any TLS-terminating reverse proxy (Caddy, nginx, Traefik) with a
                wildcard cert, OR the Cloudflare Tunnel adapter, OR (P5) the binary's own ACME
   ┌──────────────────────────────────────────────────────────────┐
   │ parts.<domain>          workspace UI, /p/<name> (redirect),   │
   │                         /api (+ /api/chrome), /mcp, /oauth/*, │
   │                         /device, /grant, /i/<id>             │
   │                         — the only host with cookies; the     │
   │                         only host that signs people in        │
   │ *.<domain> (wildcard) → content listener; answers only        │
   │                         names ending -part, else 404          │
   │   <name>-part.<domain>    one host per part: / = trusted      │
   │                           wrapper, /c/<cap>/ = sandboxed      │
   │                           content, /l/<token> = public link;  │
   │                           cookie-free, never in a shared cache│
   │   blobs-part.<domain>     shared blobs only, public immutable │
   │ optional, per adapter: edge cache for blobs-part /b/* and     │
   │ /og/*; one edge rate-limit rule as a coarse backstop          │
   └───────────────┬──────────────────────────────────────────────┘
                   │ plain HTTP (proxy on the same host or network),
                   │ or cloudflared next to the app (QUIC) with the tunnel adapter
                   ▼
   Any OCI host (Docker/Podman + compose): partifactory, one process
     :8080 workspace  — person session or agent token; policy engine decides
     :8081 content    — capability in the path; nothing else
     :9090 ops        — /metrics, /healthz, /readyz; local network only, never routed through the ingress
     /data            — partifactory.db (SQLite, WAL) + insights.db + blobs/ + logs/
                        + parts/<id>/vol/ + backup/ + exports/
   Optional sidecar: partifactory-render (chrome-headless-shell), internal network only (§4c)
   Backends sidecar: partifactory-runner, the only holder of the Docker/Podman Engine API;
     rootless BuildKit; one bridge network per part: internet yes, LAN/host/other parts no (§3f)
```

**The ingress contract** (the core's only requirement of whatever sits in front):
- route `parts.<domain>` to `:8080` and `*.<domain>` to `:8081`, preserving `Host`; terminate TLS with a cert covering both (one wildcard cert does);
- pass the client address in one header named in config (`[ingress] client_ip_header`, e.g. `X-Forwarded-For` or `CF-Connecting-IP`), trusted only from `[ingress] trusted_proxies`. Rate limits and analytics key on it;
- optionally pass a request id (`[ingress] request_id_header`); without one, partifactory makes its own. Logs and error pages show it;
- allow long-lived responses (SSE, long-poll ≤ 50 s) and request bodies up to 8 MiB.
Example Caddy, nginx and Traefik configs ship in `deploy/proxy/`, each exercised by the portability e2e lane (§5f). **Edge caching is an optimisation, never required for correctness**: every response already carries the right `Cache-Control`, and nothing breaks with no cache at all.

**Hostnames** (decided): the workspace and every part host live one label under one apex (`[domain] apex`, `workspace_host`, `part_suffix`), so one wildcard cert covers them (in the example deployment, Cloudflare's free universal cert for `example.com`).
DNS wildcards must be a whole label (`*-part` isn't valid), so routing is **one `*.<domain>` record + one `parts.<domain>` record**, created by `provision` through a DNS provider plugin or printed for you to create by hand (§5c).
Explicit records for other services always take precedence over the wildcard. Names that exist nowhere land on partifactory and get a 404 unless they end in `-part`.
There's no DNS token at runtime, and a new part is live the instant it's saved. *Cloudflare adapter only:* a wildcard CNAME to `<tunnel>.cfargotunnel.com` works in practice but isn't documented, so the adapter's spike verifies it.

**Isolation of agent-written JS, stated honestly.** Parts share a site with the rest of `<domain>` (in the example deployment, with the IdP, home automation, the forge and the family hub). The opaque origin (§2) closes the cookie attacks: a sandboxed document can't read or write cookies,
so cookie tossing onto `.<domain>`, cookie-jar flooding and reading other services' cookies are impossible. Requests a part's content initiates are cross-site for `SameSite`, so a part's fetch or form POST toward a sibling service (or the workspace) carries no `Lax`/`Strict` cookies
(verified in the P0 spike, and tested against a canary service in §7). What the sandbox does *not* close, and we accept:
1. **Top-level navigation and popups.** A standalone part can navigate itself anywhere, and the new document isn't sandboxed. A malicious part can send its viewer to a GET on another `<domain>` service, or to a phishing page.
   Passkey-only sign-in makes credential phishing inert. GETs on the other services must not mutate. Framed parts can't navigate the top frame without a tap.
2. **Exfiltration of what the part was given.** CSP confines fetches, images, forms and frames, but a URL with data in it can still be navigated to or opened. The viewer's credentials never leave with it, so the blast radius is the part's own data.

Revisit if parts ever render untrusted third-party content, for example an agent that renders scraped web pages, or if `unsandboxed` parts become common.

**Decided (2026-10-23): no separate registrable domain.** The sandbox removes the cookie-tossing risk that motivated buying one. The part suffix stays site config, so moving parts to their own domain later is a config change plus 301s, not a redesign.

## 3a. Identity and access: generic OIDC, optional provider extras, managed in-app

### Three layers, so the provider can be swapped

```
 ┌────────────── partifactory core (provider-neutral) ──────────────┐
 │ Policy engine: grants on parts/spaces → allow/deny + role        │
 │ Subjects: user:<stable-id>, group:<name>, everyone-signed-in,     │
 │           link:<id>, conn:<agent-connection-id>                   │
 │ Local passkey accounts (webauthn-rs): invitees, admin step-up     │
 └──────────────┬──────────────────┬──────────────────┬─────────────┘
          trait Authn        trait Directory     trait Provisioner (optional)
     (who is this person?) (users, groups,      (IdP-side invites, add by
                            membership)          email, IdP groups)
                ▼                  ▼                  ▼
          any OIDC IdP       groups claim +      none: native invite      ← default
          (discovery doc)    refresh re-read     links + local accounts
          + local passkeys   + local pf-* groups + local pf-* groups
          ─────────────      ─────────────       ─────────────
          (same)             Authentik API poll  Authentik API            ← adapter (example deployment)
          dev: fixture user  static file         fake                     ← dev/tests
```

Grants refer to provider-neutral subjects, so moving between Authentik, Pocket ID, Keycloak, Zitadel, Dex or any other OIDC provider changes config, not policy data.
- *Why generic OIDC as the baseline:* it's the one interface every provider speaks. Everything Authentik-specific (invitation objects, recovery links, group writes, cloned flows) is an **optional extra** behind the `Provisioner` trait, and the product works fully without it.
- v1 ships: the generic OIDC `Authn`, the generic `Directory` (claims + refresh re-read + local groups), native invites with local passkey accounts, the **Authentik `Directory` and `Provisioner` adapters** (used by the example deployment), and a test fake.

The example deployment reuses its existing Authentik pattern (per-audience groups, an enrollment flow per audience, passwordless sign-in, working email stages). The inventory is in site config.

### How people sign in

- **The workspace host is the only OIDC client** (confidential + PKCE, one redirect URI), with no per-part or wildcard redirect URIs. Configured from the provider's discovery document (`[idp] issuer`, `client_id`, client secret from env). It asks for implicit consent where the IdP supports it, so there's no consent screen for people.
- **Local passkey accounts** sign in on the workspace with a passkey that partifactory itself registered (`webauthn-rs`, discoverable credentials, conditional UI). They exist for invitees when no provisioner is configured (below), and make an **IdP-less deployment** possible: set no `[idp]`, and every person is a local account. The portability lane runs that variant too (§5f).
- **Group membership, generic directory**: the OIDC `groups` claim (claim name in config) at login, **re-read every 15 minutes** for each active session through an OIDC refresh-token grant. A refresh that returns `invalid_grant` (the person was disabled or removed) ends that session and bumps their minter epoch.
  An IdP that issues no refresh tokens gets 12-hour sessions instead of 30-day ones, so membership is re-read at least that often. Local `pf-*` groups live in partifactory's own tables and change instantly.
- **Group membership, Authentik adapter**: the same claim, plus a **60 s directory poll** over the Authentik API, so removing someone takes effect within about a minute.
- With either directory, a change bumps the epoch for affected parts. **A failed poll or refresh (IdP unreachable) keeps the last known membership**, marks it stale, never bumps an epoch, and raises an ops alert after 10 minutes.
  - *Why fail open on an unreachable IdP:* an IdP outage must not lock the household out of what they could already see. Removals still need the IdP to be up, which is the same window as before. A definite "this person is gone" (`invalid_grant`) is not an outage, and fails closed.
- **Admin step-up and IdP login are separate**, so the IdP's `max_age`/`prompt=login` behaviour isn't load-bearing for security (§ Admin).
- Workspace sessions: `__Host-pf_session`, `HttpOnly; Secure; SameSite=Lax`, stored hashed server-side, 30 days sliding, with "sign out everywhere" available. This is the only cookie partifactory sets.
- **`everyone-signed-in`** means members of the group named by site config `audience_group` (from the IdP's claim), plus every active local account, not every account on the IdP, which also serves other services. The live count shown is that set's size.

### Spaces

| Space | Created by | Grants | Who may write |
| --- | --- | --- | --- |
| Personal (`~admin`, one per person) | first sign-in | only its owner | the owner, and agents the owner connected |
| `family` | `provision` | `group:<household> → edit` | the household, and agents consented into it |
| `system` | `provision` | everyone signed in → view | the admin (holds Jig, the design-system part) |
| more | the admin | as set | as granted |

- A part is in exactly one space. `part_create` without `space` uses the creator's personal space. An agent acting for a person defaults to that person's personal space.
- **Effective access = max(part grants, space grants)**, inherited live. There's no per-part "restrict below the space" in v1.
- **Moving a part** needs write access to both spaces. "Move to Family" is the one-tap household share.
- Agent consent is limited to spaces. A connection for `~admin` + `family` can't see another person's personal space.

### Who can see a part: the audience ladder

Each part and each space has **grants** `{subject, role}`. Roles: `view` < `comment` < `edit` < `manage`.

| Audience | Subject | Who can set it | Notes |
| --- | --- | --- | --- |
| **Only me** (default in a personal space) | `user:<me>` | — | A part inherits its space's grants: private in `~me`, the household in `family` |
| Specific people | `user:<id>`… | the part's owner, the admin, agents with `share:household` | Picked from the directory. Inviting someone *new* is admin-only |
| A group | `group:<household>`, `group:pf-photofinder`… | the part's owner, the admin, agents with `share:household` | Any IdP group can be *granted*. partifactory can only *create/edit* its own `pf-*` groups (local, or IdP-side with a provisioner), and only the admin creates them |
| Everyone signed in | `everyone-signed-in` | the admin | Shown with a live count ("15 people") so it's never vague |
| **Public link** | `link:<id>` | **admin only**, once the part has a live version | No sign-in, unlisted, `noindex`, revocable, **no expiry by default**, open count and last opened. Two modes: **Can view** (default) and **Can contribute** (guests write the part's shared data, never its code; below). `comment` needs an explicit opt-in (spam and prompt-injection risk, §3c). Shown in amber as "the worst option" |

A part's **owner** is the person it was made for (the person a creating agent acted for). The admin can change it. Owners and `manage` grantees share inward; nobody but the admin shares outward.
There's no listed or indexable public mode. Effective access is the highest matching role. Every change goes to the `audit` table (who, actor, what, before → after).

- *Why owners can share inward without the admin:* sharing a part with someone who already has an account can't leak beyond the household, and gating it made the common case slow.
  Everything outward (public links, everyone-signed-in, inviting new people, creating groups) stays with the admin, because that's where data leaves.

**What each role can do at runtime** (enforced in `/_rt` and in every op):

| | personal state (`localStorage` shim, personal `storage`) | shared `storage` / `db` | comments | self-publish (`artifact.publish`) | source edit | assets upload | backend calls (§3f) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| public-link viewer (Can view) | memory only | read | only if opted in | no | no | no | as `role: public` |
| public-link guest (Can contribute) | memory only | read, **write** (rate-limited, size-capped) | no | no | no | no | as `role: guest` |
| `view` | read/write | read, **write unless the part is view only** (`shared_data_writers: "editors"`; the default is `"viewers"`, as in Claude) | no | no | no | no | as `role: view` |
| `comment` | read/write | as `view` | yes | no | no | yes | as `role: comment` |
| `edit` / `manage` | read/write | read/write | yes | yes | yes | yes | as `role: edit` / `manage` |

**Who may write shared data** is one per-part field, `shared_data_writers: "editors" | "viewers"` (default `"viewers"`), set by the owner in the share sheet ("Data: view only") or by an agent at create or in `meta`. So a poll or checklist works for everyone who can see it, while a dashboard can be read-only.

| Who | Reads shared data | Writes shared data |
| --- | --- | --- |
| `edit` / `manage` grantees and the owner | always | always, before any sharing |
| signed-in `view` and `comment` grantees | yes | yes, unless the part is `shared_data_writers: "editors"` |
| guests on a **Can view** public link | yes | never |
| guests on a **Can contribute** public link | yes | yes, within the guest limits |

Making a Can-contribute link is the only way guests write. Switching a link to Can contribute sets `shared_data_writers` to `"viewers"` (the sheet says so), and setting a part to `"editors"` while a contribute link is active freezes that link (back to Can view), so a guest never has more than a signed-in viewer.

`manage` in v1 means "edit, plus see the audit log and change inward grants". It never includes outward sharing.

### "Can contribute" public links: guests use the app, never change it

A public link has two modes, chosen in the share sheet: **Can view** (the default) and **Can contribute**. Someone using a part through a Can-contribute link is a **guest**, like a passer-by drawing on a shared whiteboard. Guests are **web consumers only**.
- **What a guest can do**: write the part's **shared data** through the `/_rt` data ops (`storage.*` shared, `db.*`), and call the part's backend, which sees them as `role: guest` (§3f).
- **What a guest can never do**: change code, files or versions; create, edit or delete parts; see history or source; share; comment as a person. Guests have no tokens, so MCP, the API and the CLI are closed to them.
  A guest cap calling any non-data `/_rt` op, or any `/api` or `/mcp` route, gets `403`. From P3 (when `claude.use` lands), the prelude answers `claude.use("user")` for a guest locally ("Guest 4f2a"). It doesn't send runtime errors from guest tabs.
- **Guest identity**: `POST /l/<token>/go` mints a cap with subject `link:<id>`, the `dw` claim and a random guest id `g`, shown as "Guest 4f2a". It lives where the cap lives (the wrapper's `sessionStorage`, the prelude's memory), so it's per tab, signed, and needs no cookie.
- **Limits, on by default**: per-guest and per-IP rate limits on `/_rt` writes (60 a minute per guest, 300 per IP), and a shared-data cap per part (5 MiB) that returns `quota_exceeded` past it.
- **Shared-data limits for everyone** (`window.storage`, §3c): a key is 1–512 bytes of UTF-8 with no control characters; a value is a string ≤ 256 KiB (the `data_docs` row cap); shared data ≤ 5 MiB per part; personal data ≤ 256 KiB per person per part (it's inlined as the `localStorage` seed). Writes: 60 a minute per guest and 300 per IP, 600 a minute per signed-in person per part. Reads: 600 requests a minute per viewer per part, where a `getMany` or a `list` page counts as one.
- **Undo and control (the admin)**: switching a link to Can contribute takes a data snapshot first, so there's always a "before the guests" point. From the share sheet they can **roll back** to any data snapshot, **Reset data** (snapshotted first), or **Freeze** (back to Can view; it bumps the link's minter epoch, so open guest tabs lose write on their next request).
  Rolling back and resetting are also agent ops (`part_data` `snapshot_list`, `snapshot_restore`, `reset`, with `parts:write`): undoing vandalism stays inside the part, so it's an inward action. Freeze stays the admin's.
- **Visibility**: guest writes are events (`data.changed`, actor `guest:<link>:<g>`), and the activity feed counts them ("14 guest edits today").
- **Share-sheet copy**: "Anyone with the link can use it. Only you and people you add can change it."
- *Why a separate mode, not a role:* a communal page (a pixel board, a sign-up sheet) is useless read-only, but nobody outside the house should touch its code. Splitting "use the data" from "change the part" gives both, and the snapshot taken at the switch makes a vandalised board one tap to undo.

### Ask for access

A signed-in person who opens a part they can't see gets `403 · NOT SHARED WITH YOU` with **Ask for access**. That's a simple note from one person to another: it adds the asker to an **access asks** list in the part's share sheet, where the owner (or the admin) taps **Add** (an ordinary inward grant) or **Dismiss**. Nobody is messaged, and it isn't an agent workflow: agents never appear in it.
The page never reveals the title. Public-link holders never see it: a revoked link is a plain `404`.

### Invitations, all minted in partifactory (admin only)

The link handed out is always **partifactory's own** `parts.<domain>/i/<id>`. Opening it records `opened_at` and counts the use. What happens next depends on whether an IdP provisioner is configured. Grants and use counts always live in partifactory's own tables.
- *Why grants in partifactory:* per-part access never depends on IdP group plumbing (Authentik invitations, for one, can't set groups). IdP groups are only needed for groups people manage elsewhere.

**Default: native invites with local passkey accounts (no provisioner). In scope for v1, P2b.**
1. **Passkey-only invite link** (share anywhere): `/i/<id>` → one field ("What should we call you?", username generated) → **passkey creation, required**, which partifactory registers itself (`webauthn-rs`) → a local account is created → land on the part with the invite's grants applied.
   - There's no email and no password. 7-day default expiry. The dialog offers **"for one person"** (single use) or **"for a group chat: up to N people"**, counted by partifactory.
     *Why multi-use:* single-use links pasted into family chats lose a race and confuse everyone else.
   - **Lost passkey** → the admin clicks "Send a new passkey link" on that *existing* person: a single-use `/i/<id>` of kind `recovery` bound to that account, which registers a new passkey and revokes the old ones. History and grants stay put, and no zombie account is created.
   - A local account can later be **linked to an IdP identity** (sign in with OIDC once while signed in locally), so a household that adopts an IdP keeps everyone's history.
   - *Why in scope:* without it, a deployment with plain OIDC and no provisioner couldn't invite anyone, and invites would quietly depend on Authentik. `webauthn-rs` is already in the binary for step-up, so the added surface is registration, passkey sign-in and recovery for local accounts.
2. **Add by email**: needs a provisioner (the IdP sends the email). Without one, the share sheet offers the invite link instead. partifactory doesn't send email.
3. **Existing user**: pick and grant (an inward share, the admin isn't needed if it's the owner's own part). It shows up in their "Shared with me" and activity feed.
4. **New group**: "Make a group for this" creates a local `pf-<name>` group in partifactory's tables.

**Adapter: the Authentik provisioner (the example deployment).** Authentik facts it's built on (checked 2026-10-23): invitations **can't set groups** (custom attributes become user attributes, groups come from the User Write stage), and there's **no use count**, only single-use or unlimited-until-expiry.
- `/i/<id>` 302s to an Authentik invitation bound to a dedicated **`partifactory-invite`** flow: invitation → one field → **passkey enrolment, required** → user write (into one holding group) → login → land on the part.
  After the first login, partifactory matches the new `sub` to the invite (carried in the login `state`), applies the invite's grants, and adds any `pf-*` memberships by API. The Authentik invitation is unlimited-until-expiry, and partifactory deletes it at the Nth enrolment.
- **Lost passkey** uses an IdP recovery link (`POST /api/v3/core/users/{id}/recovery/`) bound to the same user. **Add by email** creates the IdP user + `pf-*` groups, then triggers the IdP's "set up your account" email. **New groups** are IdP groups named `pf-<name>`, manageable from either side.
- Unverified Authentik items (object-level permission names, `is_superuser` protection, blueprint-over-API, recovery-link permission) are gates in the Authentik spike and the provisioner gate, not assumptions.

Either way: pending invites show their status (sent, opened, enrolled, expired) and have one-click revoke, and invitees who never enrolled, and people inactive for 180 days, are listed for one-click cleanup.

### UX: click budgets, tested end to end

Every flow has a budget in **real human interactions, including OS/browser prompts and the biometric**. The e2e suite (Chromium + virtual WebAuthn) enforces the page-level part,
and the simulated device matrix (decision 52: Chromium, WebKit and Firefox in Playwright, Mobile Safari in the iOS Simulator) gates every phase. A manual pass on a **real iPhone (Safari), an Android phone and the Telegram in-app browser** covers what simulation can't see, whenever the admin gets to it; it never blocks a phase, and a failure is filed as a bug. §4a.7 adds the UI-level budgets.

| Flow | Who | Budget |
| --- | --- | --- |
| Open a part I may see, already signed in | anyone | **0**: silent redirects only |
| Sign in after session expiry | returning user | **2 interactions**: pick the passkey from autofill + biometric. No "stay signed in?", no consent |
| Share with someone in the household | owner | Share → pick the person → **Done** (no step-up) |
| Share with the family | owner | Share → **Move to Family** (no step-up) |
| Make a public link | the admin | Share → "Anyone with the link" → **Copy** (+ 1 biometric if the 10-minute step-up window has lapsed. Step-up is in-page WebAuthn, not a redirect) |
| Mint a passkey invite link | the admin | Share → "Invite someone" → **Copy link** (+ step-up) |
| Accept a passkey invite | new person | open → type a name → **passkey tap** → on the part |
| Add someone by email (provisioner only) | the admin | Share → "Invite someone" → email → **Send** (+ step-up) |
| Connect a new agent (OAuth) | anyone | agent opens browser → passkey tap (if signed out) → **Allow** + passkey on the consent sheet |
| Pair yourself with a chat agent (Hermes in the example deployment) | household member | "connect me" in chat → open link → **Allow** + passkey |
| Make a public link guests can contribute to | the admin | Share → "Anyone with the link" → **Can contribute** → **Copy** (+ step-up if lapsed) |
| Act on an agent's deep link (e.g. "Tap to make it public") | the admin | tap the link → the sheet opens pre-filled → **confirm** (+ step-up if lapsed) |
| Answer an ask for access | owner | Share → **Add** or **Dismiss** |
| Revoke anything | owner or the admin | one click, with a 10 s undo toast instead of a confirmation dialog. Revocation applies at once (epoch bump), and undo re-grants |

**Authentik adapter: click reduction**. With a generic IdP, the operator configures implicit consent and passkey-first login in the IdP however it offers them, and the budgets above are what the e2e suite holds partifactory's own pages to.
With the Authentik adapter, shared flows belong to every service on the IdP, so partifactory **gets its own cloned authentication and authorization flows**,
which it owns, provisions and tunes. Shared flows are touched only for changes the admin explicitly wants globally. Global so far: no "Stay signed in?" prompt (applied 2026-10-22 to the
shared login stage at the admin's request, with the before-state in site config). partifactory-owned: implicit consent, passkey-first identification with conditional UI
(supported since Authentik 2025.12 when a WebAuthn validation stage is set on the identification stage; still verified in the Authentik spike), a no-prompt invalidation flow, and partifactory branding.

**Share sheet**: one sheet, with the audience ladder as a segmented control, a live sentence saying exactly who can see the part now, "Move to Family", the public link and its mode (the admin), pending invites,
the access-asks list (Add/Dismiss), agent-made grants labelled ("Hermes shared this with the Family group · Undo"), and copy buttons. It opens pre-filled from a deep link fragment (`#share=public`, `#share=invite`). It works on a phone (§4a.6).

### One admin, and only one

- **Pinned to the admin's immutable subject** in config (`[admin] subject = "oidc:<issuer>#<sub>"` or `"local:<id>"`), not a group. Being added to any IdP group (including the IdP's own admins) grants nothing here.
  A fresh deployment with no IdP bootstraps the admin with `partifactory admin bootstrap` on the host console, which prints a one-time link to register the admin's passkey as a local account.
- **Step-up is native WebAuthn in partifactory**, not trust in IdP claims. The admin registers an admin passkey in partifactory itself (`webauthn-rs`).
  WebAuthn `rp.id` is `[domain] apex`, so one passkey serves the workspace and every part-host wrapper; assertions are accepted from the workspace origin and from every active, sandboxed part-host origin (the same set the chrome CORS admits). The share sheet verifies via `POST /api/chrome/v1/<part>/stepup`, which sets `sessions.stepup_at`.
  **Admin actions need a passkey assertion within 10 minutes**: public links (and switching one to Can contribute), `everyone-signed-in`, invites, add-by-email, groups, users, LAN allowlist entries for backends, agent consent into spaces the consenting person doesn't own, `unsandboxed`, renames.
  Household sharing never needs step-up. *Why one 10-minute window:* short enough that a stolen session can't administer, long enough for a batch of admin changes. (Round 1 said 30 in one table and 10 in another. It's 10.)
- **Authentik adapter only: the IdP credential is least-privilege, and that's verified, not assumed** (the provisioner gate, P2b). With no provisioner, partifactory holds no IdP write credential at all. The intent: a service account `partifactory-provisioner` that can read users/groups,
  create invitations, send recovery links, and change membership of `pf-*` groups **only** (Authentik object-level permissions on each `pf-*` group).
  The gate tests, against the real IdP, that this account **cannot**: add anyone to a non-`pf-*` group, create a group with `is_superuser`, edit flows or stages,
  create users with elevated attributes, or read another application's provider secret. It also checks that `pf-*` groups have no privileged parent, because Authentik roles inherit down the group hierarchy.
  The core's own `pf-*` check is defence in depth only, because a compromised process would skip it.
  **If the IdP can't scope it that finely**: `pf-*` groups are created by `provision` (bootstrap token, run by the admin), never at runtime. The runtime account drops to
  invitations + recovery links + membership of pre-created groups. If even membership can't be object-scoped, add-by-email and new groups are done by the admin through `provision`, not at runtime.
  The result sets `[idp.provisioner] tier = "A" | "B" | "C"` in site config, which the runtime reads at start to hide the UI for anything that tier must do through `provision`.
- **Break-glass**: `partifactory admin break-glass`, run on the host console, prints a one-time admin login valid for 5 minutes. It needs local shell access, works when the IdP is down,
  and is logged and shown on the Health page and in the audit log. The admin subject pin lives in site config, which has an offline copy (§5c).

## 3b. Agents: their own identities, scoped to what they need

**Connecting is how an agent gets its own identity**, not just MCP convenience. Each connection is scoped, revocable, and labelled "Hermes for <person>".
The same token works on every surface: MCP, the HTTPS API and the CLI (§4b).

### One connection per (agent, person)

- **Delegation is explicit**: every write records `author = <person>, actor = <connection>`, which the UI shows as "Hermes for <person>". An agent can't see or do more than the person it acts for.
- **Shared agents hold one connection per person.** For example, Hermes (the example deployment's chat agent) keeps a map `telegram_user_id → token`. Each household member pairs once from Telegram ("Hermes, connect me to partifactory"):
  Hermes starts a device grant, and the person signs in and allows it at `parts.example.com/device` in their real browser (passkey + Allow). Unpaired chat users get nothing, and Hermes tells them to ask the admin for an invite.
  - *Why:* with one shared token, every family request was "Hermes for the admin", it landed in the admin's space, and Hermes could read the admin's private parts while serving someone else. That's the confused-deputy problem delegation exists to prevent.
- **Who may consent**: any person may connect an agent into their own personal space and the shared spaces they belong to. Only the admin (with step-up) may consent an agent into anyone else's space.
- **Service identities** for agents that serve the whole household with no single person, such as the family hub's server: `principal_kind = service`, spaces = `family` only, no human `sub`.
  Writes are labelled as the service. A caller-supplied `requested_by` is recorded and shown as unverified.

### How agents authenticate: OAuth 2.1, per the MCP authorization spec

Target revision **2026-07-28**, compatible with **2025-11-25**. partifactory is both the **resource server** (`/mcp`, `/api`) and a small **authorization server** that *federates* login to the IdP.
Slice 1 shipped the token side (the device grant, refresh, revocation, metadata, 401s); the authorization-code flow below followed right after it. **There are no long-lived agent credentials** (the admin, 2026-10-26): personal tokens were retired, and every agent holds a one-hour access token and a rotating refresh token.

```
claude mcp add --transport http partifactory https://parts.example.com/mcp
  → POST /mcp  → 401  WWW-Authenticate: Bearer resource_metadata="…/.well-known/oauth-protected-resource", scope="parts:read parts:write"
  → client reads Protected Resource Metadata (RFC 9728) → authorization server = parts.example.com
  → client identifies itself: Client ID Metadata Document (CIMD, preferred) or Dynamic Client Registration (RFC 7591, deprecated, kept)
  → browser: /oauth/authorize (PKCE S256, resource=https://parts.example.com/ per RFC 8707: one resource for /mcp, /api and the CLI)
       → IdP or local-passkey login if needed (1 passkey tap)
       → partifactory consent sheet: "Claude Code (redirect: localhost) wants to: read & edit parts in [~admin ▾].
          [ ] Share with people already in the household.  [ ] Deploy backends and set their secrets.  Expires: [90 days ▾]"   → passkey → Allow
  → authorization response carries iss (RFC 9207)
  → token: opaque, audience https://parts.example.com/, sub=<person>, act=conn:<id>, scopes + spaces
```

- **AS metadata** (RFC 8414 at `/.well-known/oauth-authorization-server`, also at `/.well-known/openid-configuration` for clients that only look there; it's not an OIDC provider, so no `id_token`):
  `client_id_metadata_document_supported: true`, `code_challenge_methods_supported: ["S256"]`, `authorization_response_iss_parameter_supported: true`, `registration_endpoint` (DCR), `device_authorization_endpoint`, `revocation_endpoint`, `scopes_supported`.
- **The consent sheet shows the redirect URI's host** (the spec says it MUST), with an extra warning for a localhost redirect from a CIMD client.
- **CIMD fetches** use the mirror egress guard (§2): https only, private/loopback/link-local addresses refused after DNS resolution, ≤ 5 KiB, 3 s, no redirects.
- **DCR** stays for Cursor and Gemini CLI, which don't do CIMD. It requires `application_type`, is rate-limited, and a registered client is inert until someone consents. Unconsented registrations expire after 1 hour.
- **Manual clients**: The admin can register a client by hand in the admin area (client id + secret + redirect URI) for clients that need a pre-registered one, such as GPT Actions.
- **Tokens**: opaque random strings with prefixes (`pf_at_`, `pf_rt_`), stored hashed, revocable instantly. Access tokens last 1 h. A refresh token is single use: each refresh returns a new access token and a new refresh token, and presenting a used one revokes the whole family.
  Lifetimes follow what other OAuth and MCP providers do, so there's no picker on the approval sheet: a refresh token expires after **30 days unused** (each refresh slides it, so an agent in use stays connected), and a connection ends **90 days** after consent however busy it is; then a person connects the agent again. Retired `pf_pat_` tokens never resolve, and any still stored are revoked at start.
  - *Why not JWTs:* no key distribution or clock skew, and revocation is a single row. The cost is a DB read per API call, which is fine because `/api` and `/mcp` aren't the hot path.
- **Auth failures are HTTP 401 with `WWW-Authenticate`**, on every surface, never an MCP tool result: that 401 is what triggers a client's OAuth flow. The 401 body explains the ways to get a token (§4b).
- **Swappable**: an `AuthorizationServer` trait. If the IdP gains CIMD + resource indicators, partifactory can switch to validating IdP-issued tokens as a pure resource server.
  - *Why a facade now:* Authentik's DCR (2026.x) needs an authenticated user and creates a provider + app per registration, and CIMD and RFC 8707 aren't documented. It can't serve MCP clients directly.

### Device grant (RFC 8628): the default for headless agents

*Why default:* there's no secret to copy, and approval happens on the person's phone with a passkey.

- **Client**: one pre-registered public client `part-cli`, used by every device-code agent (the CLI, Hermes, any other agent); there are no per-agent client ids in Slice 1. The agent names itself with `agent_name` and `software`. `POST /oauth/device_authorization` (form-encoded, JSON response) carries `client_id=part-cli`, `scope`, `agent_name` (required, ≤ 40 chars, e.g. "Hermes") and `software` (e.g. "hermes/1.4").
  Each approval creates its own `agent_connections` row, so each (agent, person) is revoked separately.
- **Codes**: `device_code` is 32 random bytes. `user_code` is 8 characters from `BCDFGHJKLMNPQRSTVWXZ`, shown as `XXXX-XXXX`. `expires_in` 600 s, `interval` 5 s. Polling faster gets `slow_down` (+5 s).
  `verification_uri = https://parts.example.com/device`, and `verification_uri_complete` adds `?code=XXXX-XXXX`. The CLI also prints a terminal QR.
- **Approval page** (any signed-in person, for their own connection): it shows "**This device says** it is *Hermes* (hermes/1.4)", labelled unverified; the request's coarse origin (country + network, from trusted proxy headers or a local GeoIP database; shown as "origin unknown" when neither is configured) and age ("requested 14 s ago");
  scopes as toggles; a space picker limited to spaces the person may consent; a lifetime picker (default 90 days); and **Allow / Deny**. Allow needs a partifactory passkey assertion when the person has one (the admin always does).
  A person with none, in Slice 1, approves with their signed-in session alone; once native invites land (P2b), the approval page offers to register a passkey for next time, and their registration flow makes one available to everyone.
- **Phishing defences**: when the request is over 2 minutes old, or (where origin is known) comes from a different network than the approving browser, a banner says "Only approve if you just asked an agent to connect".
  Typing a code by hand is limited to 5 attempts per minute per session. Codes are single use.
- **Token responses**: `authorization_pending`, `slow_down`, `access_denied`, `expired_token`. Success returns the same token shape as the auth-code flow.

**A person connects every agent once; there's no unattended bootstrap, by design.** An agent with no token can't get one alone: browser sign-in and the device grant both wait for a person's Allow. An unattended agent (cron, CI, a server) is connected once by device code and refreshes on its own.
*Personal agent tokens are retired* (the admin, 2026-10-26): long-lived static tokens are exactly what leaks from env files and CI logs, and every client that matters now does OAuth or the device grant.

### Scopes

| Scope | Allows |
| --- | --- |
| `parts:read` | find, get, history, diff, `part_check` |
| `parts:write` | create, edit, restore a version, fork; roll back or reset a part's shared data (`part_data` `snapshot_list`, `snapshot_restore`, `reset`, with the `edit` role) |
| `parts:delete` | soft delete to the 30-day bin, undelete |
| `data:read`, `data:write` | the part's `storage`/`db` documents and assets; with `data:read`, `part_check` renders against the real shared data |
| `comments:write` | add and resolve comments |
| `share:household` | **inward** sharing: grant view/comment/edit to people and groups that already have accounts, and move a part into a space the connection may write. Consent-time toggle, **off by default** for clients registered by CIMD, DCR or by hand |
| `backends:deploy` | create or change a part's backend (`compose.yaml`, a `Dockerfile`, an image, its lifecycle) and **set or replace its secrets** (write-only, §3f). Consent-time toggle, **off by default** for every client |

Agents act directly within these scopes; nothing they do waits on a person. Agents can never get the people-only actions: public links, invites, users, groups, `everyone-signed-in`, LAN allowlist entries, `unsandboxed`, admin.
For those, `part_link_for {action}` returns a deep link that opens the right sheet pre-filled, and the agent passes it on in its own channel ("Tap to make it public: …").

### Agent sharing: inward yes, outward no

- **Inward** (`share:household`): an agent may share with existing people and groups, or move a part into Family, within the spaces its connection allows.
  Every agent grant is labelled ("Hermes shared this with the Family group"), undoable in one tap, listed in the share sheet and in the activity feed (no message is sent).
  This is what makes "Hermes, make a packing list and share it with the family" one turn.
- **Outward, never agent-controlled**: public links, inviting new people and creating groups. There's no request to file. The agent calls `part_link_for {action: "public_link"}` and gives the person the link (`https://<name>-part.<domain>/#share=public`), which opens the share sheet on that rung. The person decides there, or doesn't.
- *Why:* an agent reading untrusted web content can be prompt-injected, and a public link or an outsider invite is the exfiltration channel. Inward grants can't leak beyond people who already have accounts.
- *Why a deep link, not a queue:* a queue adds a second place to look and stalls the agent until someone clears it. A deep link costs the person the same one tap, in the conversation they're already in, and partifactory stores nothing pending.
- **Divergence from shartifacts**: shartifacts trusts the LAN (anyone on the network sees everything), and an agent with the token can make public links itself. partifactory is reachable from the internet and has per-person audiences, so sharing is a security boundary.

### Revocation cascades

Revoking a connection kills its refresh family and its access tokens, and bumps its minter epoch, so every cap minted through it dies on the next request.
Grants it made stay (they're the person's choices too), but the connection page lists them with "Undo all".

| Agent | How it connects | Typical scopes |
| --- | --- | --- |
| Claude Code (Mac, other dev envs) | auth code + PKCE (CIMD or DCR), or the `part` CLI with device code | `parts:*`, `data:*`, `comments:write` in `~admin` (+ `share:household` and `backends:deploy` if the admin ticks them) |
| Hermes (VM, Telegram front door) | device code, **once per household member** | the same, in that person's space + `family`, with `share:household` |
| Family hub (`hub-api`) | device code once at setup, **service identity** | `parts:read`, `parts:write` in `family` |
| CI (provision/smoke tests), any CI system | device code once, refresh token in that CI's secret store (a person reconnects it every 90 days) | `parts:read` + a test space |
| A script or a function-calling app | the `part` CLI or device code | as consented |

## 3c. Agents editing parts: discoverable, quick, in place

The common case is "change that thing from last week", so finding a part and making a small edit must be cheap for an agent.

- **Discoverable**:
  - `part_find` (§3e) + recent-by-me (`part_find` with no query).
  - Each part has **agent notes**: a short "what this is / how it's built / gotchas" doc the agent keeps up to date, returned first by `part_get`.
    Notes are versioned on their own (`part_notes`), so updating them doesn't create a part version. The next agent starts from the notes, not from re-reading every file.
  - MCP resources (`part://recent`, `part://<name>`) are **projections** of `part_find`/`part_get`. Nothing is reachable only as a resource.
- **Read only what's needed**: `part_get` returns **full content when the part is ≤ 32 KiB in total** (the common single-file case), otherwise the outline (files, sizes, symbols) + notes. `file` + `range` fetch a slice.
  `part_get {render: "text"}` returns the rendered readable text the search index already has, so Hermes can answer "what's on the meal plan?" without parsing HTML.
- **Quick edits**: `part_edit` takes ordered `edits[]` across files, atomically, with auto-rebase (§3d). It returns the new version, build status and the part's link (its host). Target p99 < 10 ms without a compile.
- **Errors close the loop**: viewers' browsers report uncaught errors and blocked loads, which `part_get` returns as `runtime_errors` (the latest 20; the store keeps 1,000 per part for 30 days, §5h; `source: viewer | render`).
  `part_check` (§4c) lets the agent see its own render, screenshot included, before anyone opens it.
- **Live reload** (P2): when a new live version lands, open viewers get `pf:version` from the wrapper (or from the prelude's `EventSource` when standalone).
  A page with no unsaved personal state reloads in place; otherwise a toast says "Updated by Hermes · Reload". You watch the page change after asking.
- **Agent inbox** (P3): each part has a **steward** (the creating connection and its person, changeable in the share sheet). A `to_agent` comment goes to the steward.
  Push agents (Hermes) get a signed webhook. Pull agents get it via `part_inbox`, and the shared guide tells them to check it at session start. States: `open → claimed ("Hermes is on it") → answered → resolved`.
  A claimed request with no new version after 15 minutes shows "stalled".
- **In place, like Claude's artifacts**:
  - The workspace has a source editor (CodeMirror 6) with live preview from the draft, where saving creates a version. Drafts live server-side per (person, part), so they survive reloads and device switches.
  - **Persistent state**: `window.storage` (personal + shared) and `claude.use("db")` documents live in SQLite. `window.storage` has the same API as in Claude's artifacts: `get(key, shared?)` resolves to `{key, value, shared}` or `null`, `set(key, value, shared?)` takes a string value, `delete(key, shared?)`, `list(prefix?, shared?)` resolves to `{keys, prefix, shared}`; `shared` defaults to `false` (personal).
    partifactory adds paging and bulk reads: `list(prefix, shared?, {cursor?, limit? ≤ 1000 (default 1000), values?})` (the options object may also stand in for `shared`, as `{shared, cursor, limit, values}`) resolves to `{keys, prefix, shared, next_cursor, values?}`, where `values: true` adds `{key: value}` for the page and `next_cursor` is `null` at the end; `getMany(keys ≤ 500, shared?)` resolves to an array, in order, of `{key, value, shared}` or `null`.
    Failures reject with a `PfError {code, message, retryable, retry_after?}` whose `code` is from the error catalogue (§4b): `rate_limited`, `forbidden_view_only`, `quota_exceeded`, `too_large`, `storage_full`, `invalid_input`, `forbidden`. Limits are in §3a. Its documents are `part_data` collection `__storage`, with the key as `doc_id`. They're the part's state across versions and devices, agents read and write them (`part_data`), and they're in every copy of `/data` and every full export.
  - **Self-editing parts**: `claude.use("artifact").publish(...)` lets a part save a new version of itself, using compare-and-set against the version it's running, which must also be head. Viewers with `edit` can do it. Public-link viewers never can.
  - **Assets** (P3): viewers with `comment` or higher can add files from the page (`POST /_rt/v1 {op: "assets.put"}`, 25 MiB each, counted against the space quota). They're part blobs served at `/_a/<id>` behind the cap, listed and deletable by agents via `part_data {collection: "_assets"}`.
  - **Phone editing** (P4): "Edit text" mode lets you tap any text in the preview and change it. It's saved as an anchor edit (`old_str` = the original text), so it works whenever the text occurs exactly once in the source. Markdown and `text` parts use the formatted view instead (decision 54).
    Otherwise it offers "Ask the agent" with the text pre-filled. Long-press any element to comment with an anchor `{version, block_id | selector, text_quote, rect}`. Both run over the wrapper ↔ part `postMessage` channel, so the trusted UI stays outside the part.
- **Prompt-injection hygiene**: every comment, data document and asset carries `provenance: person | agent | link_viewer` in the JSON, and results carry `contains_untrusted: true` when any is from someone other than the connection's person.
  The MCP text rendering also wraps such content in `<untrusted_content>`. The shared guide tells agents to treat it as data, not instructions.

## 3d. Creation and concurrency: the contract

**What the others do, for reference.**
- *shartifacts*: optimistic concurrency. Every update sends `base_version`, and a stale one fails with "reload the latest version before saving". It holds an in-process lock per artifact,
  has cooperative **leases** (`acquire` with holder + TTL) on its `db` documents, a "conflict" notice when a browser draft is overtaken by an agent save, and two-step deletes with a confirmation token.
- *Claude's artifacts*: also optimistic, not locks. Edits are `{old_str, new_str}` pairs applied in order, where each `old_str` must occur exactly once and a batch applies all-or-none.
  A publish that loses a race is refused and handed the newer version to merge onto. Per-file compare-and-set uses `ifMatch` sha256s. Coordination between viewers uses `db` `acquire` leases.
  Claude Docs uses atomic operation batches with idempotency ids (`opId`) and `pending` placeholder blocks that are filled in later.
- Nobody uses pessimistic locks on source text, and partifactory doesn't either. **Optimistic by default, leases when coordination is worth it.**

**Creation contract: a link right away, robustly.**
- `part_create {name?, title, kind?, space?, files?, from?, expires_in?, live?, shared_data_writers?, request_id}` returns `{id, name, url, head, live, build}` **in < 10 ms**, before anything is built (`head: 0`, `live: null`, `build: null` with no files). The name is reserved by a unique constraint
  in the same transaction, and the part's host works immediately, its wrapper showing a "being made…" placeholder that live-updates when the first version lands. Optional `files` (or `content`) commit as v1 before the call returns and are built after it (`head: 1`, `build: {status: "queued"}`); `part_check` waits for that build.
  The agent can hand you the link *first* and build second.
- **Fork**: `from: <name>[@n]` starts a new part as a copy of that version: a new manifest, zero new blobs, with cross-part lineage in `forked_from`. "Copy last trip's packing list" is one call.
- **Scratch parts**: `expires_in: "7d"` soft-deletes the part to the bin when it expires, so chat-made scratch parts don't silt up the library. Shown with a badge, extendable.
- **Idempotent, every write**: every mutating op **requires** a client-chosen `request_id` (1–128 characters from `[A-Za-z0-9._:-]`; a UUID is ideal), scoped to the connection (or person) and kept 24 h. The CLI makes one per invocation. A retry with the same arguments returns the stored result.
  **Only a committed write uses up its id.** A write rejected before commit (any 4xx, such as `anchor_not_found` or `name_taken`, or a 5xx before commit) stores nothing, so the fixed retry may reuse the same id. A retry with different arguments after a committed write fails with `request_id_reused`. A timed-out `part_edit` retried after the network recovers returns the stored result with `replayed: true` ("already applied: version 12"), never `anchor_not_found`.
- **Durable when acknowledged**: a call returns only after the blobs are written temp → fsync → hash-verify → rename and the manifest commit is fsynced (SQLite WAL, `synchronous=FULL` on the single writer connection).
  "Acknowledged" means it survives a power cut. **No save ever waits on a backup target** (§5c).
- **Latest vs live**: every result carries `{head, live, build: {status, id, warnings, errors}}`. Edits apply to head. The live URL advances only to versions whose build is `ok`, so a broken save never replaces a working page. The agent sees the errors instead.
- **`live: "always" | "after_check"`** on `part_create` and `part_edit`. The server default is `"always"` (live moves on an `ok` build, for speed). With `"after_check"`, the server runs `part_check` (mobile) on the new version after its build, and `live` moves only if the check passes: no uncaught errors (`error`, `unhandledrejection`) and not blank.
  The check is folded into the result as `check` with `passed`, as with `check: true`. If the renderer is off or down, `"after_check"` behaves as `"always"` and the result carries the warning `check_skipped`. The agent contract recommends `"after_check"` (it's what keeps a runtime-broken page from going live between create and fix).
- **Big files**: `POST /api/v1/blobs` (raw body → sha256) and a have/need check (`POST /api/v1/blobs/missing` with `{path → hash}` returns `{missing: [hash]}`; `POST /api/v1/blobs` returns `{hash, size}`). Writes accept `{path: {blob: sha256}}`.
  `part put ./dist` uploads only missing hashes. MCP agents get a 10-minute upload URL from `part_create`/`part_edit {upload: [...]}` (after Slice 1, with assets in P3; the field is rejected until then), so they never base64 media through the model.

**The build worker.**
- **States**: `queued → running → ok | failed | timeout | superseded`. A queued build is `superseded` (and skipped) once a newer version of the same part is queued. A running build is never cancelled.
- **Isolation**: each build runs in a **child process of the same binary** (`partifactory build-worker`, re-exec'd), with length-prefixed CBOR over stdin/stdout, a **10 s deadline**, a memory cap (`RLIMIT_AS` 1 GiB) and `SIGKILL` on breach.
  A crash marks the build `failed` with `code: "builder_crashed"`. Pool size `max(1, min(2, cores/2))`, pre-forked.
- **Mirroring runs in the parent**, which is the only process with egress, before the child is spawned: it resolves and fetches every allowlisted URL (30 s budget, §2), stores the blobs, and hands the child the `{url → hash}` map.
  The child has no network, and its 10 s deadline covers compile and rewrite only. A build's wall-clock bound is therefore 40 s; `wait: true` still returns at 2 s with `status: running`.
  - *Why a child process:* oxc and rolldown are young. A hang can't be killed in-process (a `spawn_blocking` timeout only abandons the thread), and a panic under `panic = "abort"` would take the content listener down with it.
    Spawning a static binary costs 1–2 ms, so the TSX target survives, and the esbuild fallback becomes a drop-in for the same protocol.
- **Cache key**: `sha256(source_manifest_hash ‖ toolchain_version ‖ library_set_version ‖ mirror_resolution_hash)`. An identical key reuses the output.
- **Output**: a build manifest `{files: {path → {hash, br_hash?, type}}, import_map?, warnings[], errors[], mirrored: [{url, resolved, hash}]}`, stored as a blob and referenced from `builds.output_hash`.
- **Brotli**: quality 11 for text ≤ 1 MiB, 5 above that, skipped for already-compressed types.
- **Live switch**: on `ok`, inside the writer: `if version.n > live.n then live = version`, unless the version was saved with `live: "after_check"`, in which case the switch waits for its check to pass (or be skipped). Emit `build.finished` and, on a switch, `part.live_changed`.
- **`wait: true`** (the default for agents) blocks ≤ 2 s on `build.finished`; `check: true` or `live: "after_check"` extends the wait to the build plus the check, up to 15 s. If it's still running, the result says `build: {status: "running", id}`, and `part_get {build: id}` or the next `part_watch` event finishes the story. `wait: false` returns at once.

**Concurrency contract: several agents (and people) on one part.**
- **Linearisable writes**: one writer task serialises all commits, so every part has a strict version sequence `n, n+1, …` with no lost updates. Rebase runs inside that same critical section (read head → apply → commit).
- **`part_edit` request**: `{name, base_version?, edits: [Edit], message?, wait?, check?, live?, request_id}`, where `Edit` is one of:
  `{op: "replace", file?, old_str, new_str}` (`file` defaults to the entry) · `{op: "write", file, content | blob, if_match: sha256 | null}` (`null` = must not exist) ·
  `{op: "delete", file, if_match}` · `{op: "rename", from, to, if_match}` · block ops (below). Mode fields instead of edits: `restore: n`, `delete: true`, `undelete: true`, `rebuild: true` (with an optional `update` range, §3h), `meta: {title, tags, notes, description, shared_data_writers}`, `claim: {scope, ttl}`.
- **Semantics**: all or none. Edits apply **in order**, each against the text as left by the previous one. `old_str` is non-empty and must occur **exactly once** in the current working text of its file.
- **Rebase**: if `base_version` is absent or equals head, apply to head. Otherwise: (1) the base must exist and belong to the part, or `base_unknown`;
  (2) dry-run the edits on the base, and any failure there is the caller's bug (`invalid_against_base`); (3) apply the same list to head, where `write`/`delete`/`rename` compare `if_match` against *head*;
  (4) commit with `parent = head` and `rebased_from = base`. Two agents editing different regions of one file both succeed.
  An edit fails only when its anchor itself changed (0 or ≥ 2 occurrences at its turn). The error returns the failing `edit_index` and the current text of the best-matching region
  (normalised Levenshtein ≥ 0.6, otherwise the file's first 600 chars), ±3 lines, ≤ 2 KiB, with the line range, so the agent can redo just that edit.
- **Limits**: ≤ 64 edits per call, `old_str` ≤ 64 KiB, each resulting file ≤ 2 MiB, request ≤ 8 MiB.
- **Whole-file replacement** is compare-and-set per file (`if_match`, as in Claude's `publish`). It's never a blind overwrite.
- **Leases**: `part_edit {claim: {scope: part | file | block, ref?, ttl ≤ 10 min}}`, part scope by default. They're advisory, like Claude's `db.acquire`. Others' edits to a claimed scope get
  `claimed {by: "Hermes", until: "14:32"}`, and a person can always override. Leases auto-release on commit or expiry, and the UI shows them ("Hermes is editing *app.tsx*").
- **People and agents together**: the workspace editor keeps a server-side draft. When a new version lands mid-edit, the draft is merged with a 3-way `diff3` at line granularity (base, draft, head).
  Non-overlapping changes merge silently. Overlaps show a side-by-side resolve. Nobody's work is silently discarded, and "force" exists only for the person, never for agents.
- **Data** (`window.storage`/`db`) has per-document compare-and-set (`rev`) plus leases, exactly like Claude's `db`, and is separate from source versioning.

**Long documents** (a research write-up, a spec, a report).
- **Block** = a top-level comrak node: a heading, paragraph, list (the whole list), block quote, fenced code, table, HTML block or thematic break. In `slides`, block = slide.
  **Section** = a heading plus everything up to the next heading of the same or a higher level.
- **Sidecar**: the file `.pf/blocks.json` *inside the manifest*, so it's versioned atomically with the text: `{v: 1, file: "index.md", blocks: [{id, kind, start_byte, end_byte, hash}]}`.
  Ids are 8-character Crockford base32, random, unique within the part, and never reused.
- **Carrying ids over**: after *any* commit that touches a Markdown file (anchor edit, whole-file write, editor save), re-parse it and match new blocks to old ones in three passes:
  exact `hash` match in order (LCS); then remaining blocks of the same kind in the same gap with token Jaccard similarity ≥ 0.5; then everything else gets a fresh id.
- **Ops**: `{op: "replace_block", block, markdown, if_hash?}`, `{op: "insert_after", block | "start", markdown}`, `{op: "append_to_section", heading_block, markdown}`, `{op: "delete_block", block, if_hash?}`.
  Errors: `block_not_found`, `block_changed` (with the current text). Rebase resolves by id, not offset, so block edits on different blocks never conflict.
- **Outline-first, fill-in-parallel**, like Claude Docs' `pending` blocks: `<!-- pf:pending id=<id> intent="…" -->` on its own line renders as a placeholder, and `replace_block` on that id fills it.
  An agent creates the outline, shares the link at once, and one or more agents each claim a section (`scope: block`) and fill it in.
- `part_get` on a long doc returns the outline + block ids, so agents read the section they need rather than the whole document.

**Do we need CRDTs?** Not for v1, but live co-typing is deferred, not ruled out (decision 54). CRDTs (Yjs/`yrs`, Automerge) are production-ready, but they solve *simultaneous keystroke-level typing by humans*.
Agents submit whole edits, which versions, anchors, blocks and leases handle with far less machinery, and the formatted view (§4a.7) saves people's edits the same way.
If live co-typing is wanted, `yrs` runs *inside one formatted-view session* (presence, cursors, a `Y.Text` per block being edited), and the session's result commits as ordinary block ops and an ordinary version, so nothing below the editor changes.

## 3e. Search, history and the bin

**Search** is SQLite FTS5, the same index for the workspace search box and for `part_find`.
- **Indexed**: title, description, tags, agent notes, comments, and the **extracted readable text** of HTML and Markdown parts (text nodes after rendering, minus `<script>`/`<style>`). Code is not indexed as text, so a search for "map" doesn't match every `.map(` call.
  The index updates when a version goes live and when notes or comments change.
- **Filters**: `space:`, `kind:`, `by:` (an agent or a person, e.g. `by:hermes`, `by:me`), `updated:` (today/week/month or a date range), `has:data`, `has:comments`, `tag:`.
- **Ranking**: `bm25` with column weights (title 10, tags 5, description 4, notes 3, text 1, comments 2) multiplied by a recency factor `1 / (1 + age_days / 30)` on the last live change. No query means "recent, by me or this agent".
- **Workspace**: an instant search box (`/` to focus, results as you type, debounced 80 ms) with matches marked and "where it matched" (title, text, comment, agent notes).
- **Later, additive**: embeddings for semantic search alongside FTS5 (P5). Nothing in v1 depends on them.

**Versions and rollback, per part.**
- Every version is immutable and labelled with who and when ("Hermes for the admin · 14:02", "The admin · yesterday"), plus the agent's edit message or an auto summary ("app.tsx +14 −3").
- **History**: the full ledger. **Diff** between any two versions: a file list with `+/−` counts and unified (or side-by-side) diffs; binary files show size changes and before/after thumbnails for images.
- **Preview any old version**: the frame reloads with a cap pinned to that version (`v` claim). People with `view` can preview versions that were once live; drafts and never-live versions need `edit`.
- **Restore** = a new version whose manifest equals the old one. It's non-destructive, shows "Restored as REV 13 · Undo", and goes through the build like any other version.
- **Keep all versions.** Blobs are deduplicated by hash, so history is cheap. There's no version-pruning policy in v1.
- **Deleted parts go to a 30-day bin**: hidden from search and the gallery, restorable by the owner or the admin (`part_edit {undelete: true}`), purged after 30 days. A purged part's name stays reserved forever.

## 3f. Backends: a part can bring containers

**A part is a page plus, optionally, any containers it wants.** The page is served exactly as in §2. The containers run behind the part's own host and are reached only through partifactory, which checks the cap first. Backends land in P2a, right after agents work end to end.
- *Why in v1:* the admin wants small apps with real state and real services ("Docker, whatever") without a separate hosting story for each. The per-part `db` still covers most micro-apps. A backend is for the ones it doesn't: a leaderboard on Postgres, a scraper on a schedule, a websocket game.
- *Why containers, not WASM first:* every language and every database already ships as an OCI image, and agents already know how to write a Dockerfile. WASM components stay an optional later alternative (P5).

**Sources** (one per service):

| Source | How it arrives | Build method recorded |
| --- | --- | --- |
| A Dockerfile + context | files in the part (`Dockerfile`, or `build:` in `compose.yaml`) | `server_build` (rootless BuildKit in the runner, §3g) |
| A prebuilt image from a registry | `image: <ref>` in `compose.yaml`. The registry must be in `[backends] registries`. The tag is resolved to a digest at deploy, and the digest is recorded | `prebuilt_image` |
| An uploaded image | `part put --image app.tar` (a `docker save` tarball), streamed to `POST /api/v1/images` (≤ 2 GiB, counted against the part's quota) | `uploaded_image` |

**Services: a small compose subset.** A part's `compose.yaml` lists its services: usually one `main` HTTP service, plus dependencies (Postgres, Redis), a `worker` or a `cron` job. These keys are supported, and nothing else:
- per service: `image` or `build` (`context`, `dockerfile`, `args`), `command`, `entrypoint`, `environment`, `expose` (one port, `main` only), `healthcheck`, `depends_on` (with `condition: service_healthy`), `volumes` (named only), `secrets` (names of the part's secrets), and `x-pf` (`role: main | worker | cron | dependency`, `schedule` for cron, `export` hook, `limits`);
- top level: `services`, `volumes`, `secrets`.
Anything else (`ports`, `privileged`, `cap_add`, `network_mode`, `pid`, `devices`, bind mounts, `extra_hosts`, `sysctls`, `user: root`, …) fails the build with `compose_unsupported_key {key, service}`, and the error lists the supported set. A part with only a `Dockerfile` is one `main` service on its `EXPOSE`d port.
- *Why a subset:* every key we accept is a key the hardening template has to reason about. A short list is one the security suite can cover completely.

**Lifecycle.**
- **One container set per live version.** When a version builds, its set starts next to the current one. Its health check (the compose `healthcheck`, or a TCP connect to the main port) must pass within 60 s before `live` moves. A set that fails ends the build `failed` with `backend_unhealthy` and its last 50 log lines, and the old set keeps serving. The old set drains for 30 s, then stops.
- **Rollback rolls back backends' code, not their data.** Restoring version 9 starts version 9's images (kept for live + the previous 2 versions; older ones are rebuilt from source, re-pulled by digest, or need the tarball again).
  **Volumes belong to the part, not the version**, and every version shares them, as in shartifacts ("every version shares it… so migrate forward").
- **Rolling forward or back is best effort, and the part's own data is the part's business.** partifactory guarantees two things only:
  1. the restored or new version's code is exactly what it was built from;
  2. it never goes live unless its health check passes, so a version that can't run against today's data fails with `backend_unhealthy` and the current one keeps serving.
  It doesn't promise that old code understands data a newer version wrote, and it runs no migrations, down-migrations or data rollbacks for backends.
  Whatever a part puts in its containers (schemas, migrations, file formats), its agent manages.
  - *Why:* any "magic" here (automatic down-migrations, version-tagged volume snapshots) only works for the storage engines we'd special-case, and it would fail silently for the rest. The honest guarantee is the health gate. Recovery beyond it is the part's own export hook, or a folder backup or export (§5c).
- **The backend contract** (it goes into `docs/agent-contract.md` when backends ship):
  - migrate forward, with additive changes, so the previous version still runs against the new data;
  - versions run at the same time during the 30 s drain (and a restored version meets the newest data), so read from storage per request rather than caching in memory, and lock writes (a database, SQLite with WAL, or a file lock);
  - if a change can't be additive, take your own backup first (e.g. `pg_dump` into the volume) and say so in the version's edit message;
  - exit promptly on SIGTERM.
- **Scale to zero** is the default: the set starts on the first request (the proxy holds the request for up to 10 s; a cold start is typically 1–2 s) and stops after 15 minutes without one. **Always on** is a per-part setting for workers and websocket games, counted against the global quota. `cron` services run on schedule even while the rest is scaled to zero.
- **Logs** from every container are captured per part (ring segments, §5h), along with restarts, cold starts and resource use (§5g).

**Routing.** The part's own code calls its backend at `/api/*` (the prefix is set per part). The prelude turns that into `/c/<cap>/api/*`, like any same-host URL (§2). On that path, partifactory:
1. verifies the cap exactly as for content (host, epochs, window) and applies the role × runtime matrix (§3a), so a guest reaches the backend and a revoked cap doesn't;
2. strips the cap from the path, every `Cookie`, any `Authorization` carrying a `pf_` token, and every incoming `X-PF-*` header;
3. adds `X-PF-Identity`, a compact JWT (HS256 with a per-part key, 60 s expiry) carrying `sub` (`user:<id>`, `guest:<link>:<g>` or `link:<id>`), `name` (a display name, or "Guest 4f2a"), `role` (`view` · `comment` · `edit` · `manage` · `guest` · `public`), `part` and `exp`, plus `X-PF-Request-Id`;
4. proxies to the part's `main` service on its private network, **WebSocket upgrades and SSE included**, with the host's usual 50 s idle and 8 MiB body limits.
The backend checks the identity header with its key (mounted at `/run/secrets/pf_identity_key`) and **never sees a credential**. There's no unauthenticated route into a backend.
- *Why a signed header, not a session:* the backend learns who's asking and what they may do from the one place that decided it, without holding anything an attacker could replay elsewhere. A per-part key stops one part from minting identities for another.

**Data.** Each named volume belongs to one service and is a directory, `/data/parts/<id>/vol/<svc>`, so a folder backup of `/data` covers it (§5c). Volumes have a per-part quota (default 1 GiB).
`partifactory export` includes volumes consistently: for each service it runs its export hook if it has one (`x-pf.export`, e.g. `pg_dump`), otherwise it stops the service, copies the directory and starts it again.
**Data snapshots** (on by default, §5c) cover the part's `db`/`storage` documents only. **Backend volumes are covered by folder backups and exports, not by data snapshots**, and the part's Details sheet says so.

**The runner sidecar.** `partifactory-runner` is a separate container (the same binary, run as `partifactory runner`) and **the only process with the Docker Engine API**. Docker and Podman (whose API is Docker-compatible) both work.
partifactory talks to it over an internal network through a fixed, typed API (`build`, `deploy`, `start`, `stop`, `logs`, `stats`, `prune`), authenticated with `PF_RUNNER_KEY`.
- It creates containers **only from one template**: not privileged, `cap_drop: ALL`, `no-new-privileges`, a read-only root filesystem with a 64 MiB tmpfs `/tmp`, a non-root user (UID ≥ 10000, forced even if the image asks for root), the default seccomp profile, pids/memory/CPU/disk limits, `json-file` logging with `max-size=10m` and `max-file=3`, and optionally the gVisor runtime (`[backends] runtime = "runsc"`).
- No host mounts beyond the part's own volumes, never the Docker or Podman socket, no host network, no devices.
- *Why a sidecar:* the Engine API is root-equivalent on the host. Keeping it in a small process with a fixed API means a bug anywhere in partifactory's large surface can at worst ask for a hardened container, never a privileged one.

**Network.**
- Each part gets **its own bridge network** (`pf-part-<id>`), shared only by that part's services.
- **Internet egress is on by default** (APIs, package mirrors, outgoing webhooks). Bytes out are metered per part.
- **The LAN, the host and other parts are blocked by default.** The runner installs explicit drop rules for each part network: RFC 1918, CGNAT, link-local (including `169.254.169.254`) and IPv6 ULA/link-local ranges, the host's own addresses, and every other part network.
  On Docker the rules go in the `DOCKER-USER` chain. On Podman (netavark) they go in the runner's own nftables table. `docs/backends-network.md` documents both per host, and `doctor` checks that the rules are in place.
- **LAN allowlist**: a per-part entry (`host:port` or a CIDR, e.g. "may reach the home-automation server on :8123"). **Only a person sets it**, the admin in the admin UI (with step-up). An agent that needs one gives the admin the deep link `parts.<domain>/admin/backends/<part>#lan`, pre-filled. Denied attempts are counted per part (§5g).
- *Why egress on, LAN off:* the useful things a small app does are on the internet. The dangerous thing a prompt-injected or vulnerable backend could do is reach the household's own devices, so that's what's closed.

**Secrets: write-only, per part.**
- Agents with `backends:deploy` set or replace a part's secrets **directly** (`part_edit {backend: {secrets: {NAME: value}}}`). People with `edit` set them in the part's settings, which is how keys only the admin holds get in.
- **Nobody reads a secret back**: no op, surface or screen returns a value, not even to the agent that set it. Listing shows names, who set each one, and when.
- Secrets are injected as env vars or files under `/run/secrets/`, stored encrypted with `PF_SESSION_KEY`, scoped to one part, and **never appear in versions, exports, logs or Insights**. The one exception is an export run with `--include-secrets`, which requires `--encrypt`.
- *Why no approval:* the agent setting a key already holds it, so a gate adds nothing. What protects a secret is that it can't be read back out and can't reach another part.

**Permissions and quotas.**
- Changing a part's backend (a `compose.yaml`, a `Dockerfile`, an image, its lifecycle, its secrets) needs `edit` on the part, and for agents the **`backends:deploy` scope, off by default at consent**.
- Per-part limits (defaults): 1 CPU, 512 MiB of memory, 256 pids, 1 GiB of volumes, 2 GiB of images. Global limits: 20 running containers, 4 GiB of backend memory, 20 GiB of images. A deploy past a limit fails with `quota_exceeded {kind}`.
- **Agent ops**: no new tools for backends. `part_edit {backend: {lifecycle, secrets}}` changes settings, and `part_get {backend: true}` returns the container set's state, health, cold starts and recent log lines (`logs: {service, since, grep}`).

**The acceptance test** is the backend variant of the communal pixel editor (§6 P2a): the same board with a leaderboard service on Postgres.

## 3g. Builds: the server builds by default, prebuilt is optional

**Agents send source, and partifactory builds it.**
- **Frontends** build in the binary, in the build child process (§3d).
- **Backends** build with **rootless BuildKit inside the runner**: build network on (apt, cargo, npm) behind the same LAN and host block as running parts, a 10-minute timeout, CPU and memory limits (2 CPUs, 4 GiB by default), and a build cache capped at 10 GiB with LRU pruning.
- **Or agents send build output**: a `files` part (a `vite build` bundle) or a prebuilt image, from a registry or uploaded. partifactory serves or runs it as is.

*Why the server builds by default:*
1. A quick `part_edit` changes source, and the change means nothing until it's rebuilt.
2. Many AI clients can't run a build at all: claude.ai on the web and on mobile, ChatGPT, a chat agent in Telegram.
3. A person editing in the workspace UI, or tapping a typo on a phone, has no toolchain.
4. The stored source stays the diffable, resumable truth. The next agent edits what's there, not a minified bundle.

**Build method**, recorded on every build record (one per target, `frontend` or `backend`) for Insights (§5g): `server_build` · `prebuilt_files` · `prebuilt_image` · `uploaded_image`.
Backend build logs are attached to the build record (the last 20 builds per part are kept, §5h) and returned by `part_get {build: <id>}`.

## 3h. Updates and advisories

Parts are built once and then left alone, so something has to notice when a library or a base image a part carries gets a known vulnerability. partifactory notices, says so where people and agents already look, and makes the fix one call or one tap. It never changes a live part by itself.

**Three layers, kept apart.**
1. **partifactory itself.** Most of this is in §5b and §5d; it's listed here so the three layers sit side by side.
   - Renovate PRs, `cargo audit` + `cargo deny` in CI, a nightly advisory run on main, and base images pinned by digest (§5d).
   - Releases ship as an image through CI (§5b).
   - The prelude, `pf-shell.css`, the wrapper and the chrome are served at serve time (§2), so a platform security fix reaches every part on deploy, with no rebuild.
   - Toolchain upgrades (oxc, rolldown) never rebuild old parts (§2, build records).
2. **A part's frontend libraries.**
   - Every external library is already mirrored at build time and recorded in `mirror_cache` (requested → pinned → hash, §2).
   - Each build also records its library list as a small SBOM, in a new table `build_deps(build_id, ecosystem, name, version, source_url, blob_hash)` (§5e):
     - cdnjs, jsDelivr `/npm/`, unpkg and esm.sh URLs map to an npm `name@version`;
     - the TSX library set (`libset.toml`) maps directly;
     - anything it can't parse is stored as `unknown` and listed as such, so a part never claims a confidence it doesn't have.
   - **Code pasted inline into a page can't be detected.** A library copied into a `<script>` block has no URL and no version, and the part's advisory list says "inline code isn't checked".
   - *Why from the mirror:* the mirror already fetches and pins every external library, so the list costs one row per library, and it's exactly what the part loads.
3. **A part's backend.**
   - At build, the runner makes an SBOM of the image with one pinned scanner image (chosen in the spike; Syft is the default candidate) and stores it with the build, under a size cap (§5h).
   - Nightly matching runs against the stored SBOMs. Images aren't rescanned.
   - The runner also re-resolves each base-image tag and records "a newer digest exists".
   - *Why an SBOM once and matching nightly:* scanning an image is slow and touches the runtime, while matching a stored list against a refreshed advisory DB is a lookup. Syft makes the SBOM once and Grype rescans it against a fresh DB without touching the image again; Trivy makes and consumes SBOMs the same way, with an offline DB (about 50 MB, updated daily).

**Immutability stays the rule.**
- A live part never changes under you. Old builds keep their frozen import maps and image digests, so rollback stays exact.
- An update is always a new version, made by an agent or a person (or by the opt-in auto-update below), and it rolls back like any other version.
- *Why:* parts are throwaway and self-contained. A silent auto-upgrade breaks pages nobody is watching, and it breaks rollback.

**Advisory data is an adapter** (§5f portability), `AdvisorySource`, chosen with `[advisories] source`:
- **Default, `osv-bulk`:** a daily download of the OSV bulk dumps (`https://storage.googleapis.com/osv-vulnerabilities/<ECOSYSTEM>/all.zip`; the list is in `ecosystems.txt`) for the ecosystems in use: npm for frontends, plus Debian, Alpine, Ubuntu, crates.io, PyPI, Go and npm for backends. OSV exports continuously, and after the first download the sync is incremental from each ecosystem's `modified_id.csv` (newest first). Matching runs **locally**, against the copy in `/data/advisories/`.
  - *Why local:* only public data comes in. Nothing goes out about what the household runs.
  - The licences vary by source (CC-BY 4.0, CC0, MIT, Apache, and CC-BY-SA for Ubuntu). partifactory stores the data and shows each record's attribution wherever it shows the advisory; it doesn't redistribute it.
- **Alternative, `osv-api`:** the OSV API (`POST /v1/querybatch`, which has no auth and currently no rate limits). It sends package names and versions out, never part content, and the config comment says so.
- **Off, `none`:** no advisories. It shows as a warning on Health and in `doctor`, following "defaults are the safe choice".
- **It fails open.** If the advisory DB is more than 7 days stale, Health turns amber. Builds never block on it.

**Severity in context, stated honestly.**
- A frontend library runs in an opaque-origin sandbox with no cookies (§2). The worst case is attacker-controlled input exploiting it (an XSS in a markdown or sanitiser library, say) to act with that viewer's cap on that part's data.
- So frontend advisories are **shown, not alarmed**. They're raised one level when the part takes untrusted input: a public "Can contribute" link, or a backend.
- Backend advisories matter more: backends have internet egress and a reachable `/api` (§3f).
- The display is the OSV severity plus that context ("high · frontend · raised: the part has a Can-contribute link"), never just a CVSS number.

**What happens on a finding.** There are no approval queues and no notifications for people (the §4 rule stays).
- An `advisories` row is recorded (§5e): `part_id, build_id, ecosystem, package, version, osv_id, severity, fixed_in, first_seen, state (open | fixed | ignored), ignored_by, reason`.
- **Where it shows:**
  - a badge in the part's chrome, for the owner and editors (§4a.6);
  - a count on Health and in Insights (§4a.6 Admin, §5g);
  - an `advisories` list in `part_get` output (§4);
  - a `dep_advisory` warning in `part_check` and in `build.warnings` (§4b);
  - an event, `part.advisory`, in the event log, so a watching agent can act (§4 Events);
  - the optional ops-alert webhook, only for a critical advisory on a backend or on a part with a contribute link (§4).
- **Fix paths:**
  - **An agent:** `part_edit {rebuild: true, update: "patch" | "minor" | "latest"}`. It re-resolves the recorded libraries (and base-image tags) within that range and builds a new version, which goes live under the part's `live` setting (`after_check` runs `part_check` first, §3d).
  - **The admin or an editor:** an **Update libraries** button in the part's chrome does the same in one tap (§4a.6, §4a.7). Undo is the normal restore.
  - **Ignore, with a reason:** by a person, or by an agent with `parts:write`. It's audited.
- **Auto-update.** Site config `[advisories] auto_update = "never" | "patch"`, overridable per part.
  - **Provisional, pending the admin:** frontends default to `never`. Backends default to `patch`, only for an advisory with a known fix (`fixed_in`), through the `after_check` gate, with automatic rollback if the check fails (code only, best effort, like every backend rollback, §3f).
  - *Why:* backends are the exposed layer, and a patch-level base-image bump rarely breaks.
  - The action is recorded as done by `system`, never as the admin.

**Storage and retention** (rows in the §5h table):
- the advisory DB: the latest sync only, with a cap;
- stored SBOMs: kept while their build exists, with a size cap per build;
- `advisories` rows: deleted with their build.
The janitor covers all three.

**Phasing.** Slice 1 is frozen, and adding to it is the admin's call.
- **Provisional:** record `build_deps` in Slice 1. It's nearly free (the mirror already knows every library), so history exists from day one.
- Matching, the UI and `update` come after Slice 1.
- Backend SBOMs come with the backends phase (P2a).
- A spike comes first (§5): the OSV dump sizes, the matching speed against 10k `build_deps` rows, and the choice between Syft and Trivy in the runner. The dump sizes and the matching speed are unmeasured.

## 4. Operations: what agents and people can do

Every operation is defined once in the operation registry (§4b) and appears identically on MCP, the HTTPS API and the CLI. There are eleven tools around one write op, plus `part_inbox` in P3.
The table below is the full v1 surface; the Slice 1 subset and its exact fields follow it.

| Op | Does |
| --- | --- |
| `part_create` | reserve a name and get the part's link in < 10 ms, before building (§3d). `check_only: true` tests a name and returns ranked alternatives. Optional `files`/`content`, `kind`, `title`, `space`, `tags`, `notes`, `description`, `from` (fork), `expires_in`, `preview` (§4c), `request_id` |
| `part_edit` | **the one write op**: ordered anchor edits, per-file `write`/`delete`/`rename` with `if_match`, block ops, `meta`, `restore: n`, `delete`/`undelete`, `rebuild` (with `update: "patch" \| "minor" \| "latest"` to re-resolve libraries and base images, §3h), `claim`, `backend: {lifecycle, secrets}` (write-only, `backends:deploy`); `base_version` with auto-rebase; `wait` for the build; `check: true` runs `part_check` after the build; `live: "after_check"` holds `live` until that check passes (§3d, §3f) |
| `part_get` | full content when the part is ≤ 32 KiB, else outline + notes; `file`/`range`, `version` (default `head`), `history`, `diff_from`/`diff_to`, always the latest 20 `runtime_errors`, `render: "text"`, `build: <id>` (with its log), `backend: true` (state, health, recent logs; never secret values); always the open `advisories` (§3h) and the `preview` state (§4c) |
| `part_find` | search + filters (`kind:`, `tag:`, `space:`, `by:`, `has:data`, `has:comments`, `updated:`); no query = recent by me / this agent (§3e) |
| `part_share` | `list` who can see it (the live sentence + grants); `grant`/`revoke` inward (people and groups with accounts, `share:household`); `move` to a space. Always returns the link to post (the part's host) |
| `part_link_for` | `{name?, action}` → a deep link that opens the sheet for a **people-only** action, pre-filled: `public_link` (`https://<name>-part.<domain>/#share=public`), `contribute`, `invite`, `group`, `everyone`, `lan` (`https://parts.<domain>/admin/backends/<part>#lan`), `admin`. Read-only; the link grants nothing by itself. Slice 1 has `public_link` and `contribute`; `group`, `everyone` and `admin` come after it, `invite` with native invites (P2b) and `lan` with backends (P2a) |
| `part_check` | render a version (`head`, `live` or `n`) in the sandboxed renderer against a read-only view of the real shared data, optionally run up to 20 `actions` (click, type, wait, read back state), and return a screenshot (an image block in MCP) + `blank` + errors, console and failed loads; `record: true` also returns the preview clip it would make (§4c) |
| `part_comments` | list/add/resolve; `to_agent` threads; anchors; `provenance` per comment |
| `part_data` | the part's `db`/`storage` documents and `_assets` (Claude ArtifactData-shaped), with `rev` compare-and-set (`if_rev`) and leases (P3); data snapshots: `snapshot_list`, `snapshot_restore`, `reset` (with `parts:write`) |
| `part_watch` | long-poll the event log since a cursor, `wait` default 25 s, max 50 s (§4, Events) |
| `design_system` | Jig's rules and starter files, read from the Jig part (§4a.8) |
| `part_inbox` (P3) | work addressed to this connection across parts: `to_agent` comments and runtime-error spikes; claim/answer |

*Why so few:* agents choose tools by name and description, and near-duplicates (`part_put` vs `part_create`, `part_recent` vs `part_find`, preview vs share request) cause wrong picks and wasted turns. Every tool also costs context.
Every op carries hints (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint: false`), so clients can auto-approve reads.
Every mutation returns `{name, url, head, live, build: {id, status, warnings, errors}}`, so the agent never needs a second call to learn where things stand. `head` and `live` are version numbers (`live` is `null` until a build is `ok`), and `build` is `null` before v1.

**Slice 1 op fields.** The canonical, agent-facing version with outputs, errors and examples is `docs/agent-contract.md` (§4b); a test keeps the two and the registry in step. Required fields in **bold**, defaults in brackets.

| Op | Input | Output |
| --- | --- | --- |
| `part_create` | **`title`** (or `name` with `check_only`), `name` [from the title], `kind` (`html` \| `markdown` \| `text` \| `files`; `tsx` after Slice 1) [detected from the entry file; required with `content`], `content` (one entry file) or `files` (`{path: text \| {blob: sha256}}`), `space` [your personal space], `from` (`name` or `name@n`), `expires_in` (`"<n>h"` or `"<n>d"`), `tags`, `description`, `snapshots` (`daily` \| `hourly` \| `off`) [`daily`], `shared_data_writers` (`editors` \| `viewers`) [`viewers`], `live` (`always` \| `after_check`) [`always`], `check_only` [false], **`request_id`** (omitted with `check_only`) | the mutation shape plus `id` and `replayed?` (it returns before the build, so a `live: "after_check"` result arrives through `part_check`); with `check_only`, `{available, name, suggestions}` (`name` is the name tested) |
| `part_edit` | **`name`**, **`request_id`**, `edits` (≤ 64) or one mode field (`restore: n`, `delete: true`, `undelete: true`, `rebuild: true`, `meta: {title?, tags?, description?, shared_data_writers?}`), `update` (`patch` \| `minor` \| `latest`, with `rebuild: true`; after Slice 1, §3h), `base_version` [head], `message`, `wait` [true], `check` [false], `live` (`always` \| `after_check`) [`always`] | the mutation shape plus `rebased_from?`, `replayed?`, `check?` |
| `part_get` | **`name`**, `version` (`"head"` \| `"live"` \| n) [`"head"`], `file`, `range` (`[first, last]`, 1-based lines, needs `file`), `history` [false], `diff_from`, `diff_to` [head], `build` (a build id) | `{id, name, url, title, kind, space, version, head, live, build, files, content?, runtime_errors, history?, diff?}`; `advisories` after Slice 1 (§3h) |
| `part_check` | **`name`**, `version` [`"head"`], `viewport` (`mobile` \| `desktop`) [`mobile`], `wait_ms` (≤ 8000) [8000], `actions` (≤ 20 steps) [none], `include_base64` [false] | `{name, url, version, head, live, build, screenshot, blank, height, errors, console, failed_requests, actions, timing}` |
| `part_data` | **`name`**, **`action`** (`get` \| `list` \| `set` \| `delete` \| `snapshot_list` \| `snapshot_restore` \| `reset`), `scope` (`shared` \| `personal`) [`shared`], `collection` [`__storage`], `doc_id`, `data`, `if_rev` (`0` = must not exist), `prefix`, `cursor`, `limit` [100, ≤ 1000], `snapshot_id` (for `snapshot_restore`), `confirm` (`true`, for `reset`), `request_id` (required on `set`, `delete`, `snapshot_restore` and `reset`) | `{doc}`, `{docs, next_cursor}`, `{doc_id, rev}`, `{doc_id, deleted}`, `{snapshots, policy}`, `{restored, before_snapshot_id, docs}` or `{before_snapshot_id, deleted}`; reads carry `contains_untrusted`, writes `replayed?` |
| `part_link_for` | **`action`** (`public_link` \| `contribute` in Slice 1), `name` (required for both) | `{action, url, who}` |

Shapes inside those outputs: `files` is `{path: {hash, size, mime}}` and `content` is `{path: text}`; `runtime_errors` items are `{kind, message, stack, url, count, source, version, last_at}`; `history` items are `{n, author, actor, via, message, created_at, build_status}`; `diff` is `{from, to, files: [{path, status, added, removed, patch}]}`.
`part_check`'s `errors` items are `{kind, message, stack?, url?}` (the `runtime_errors` kinds), `console` items `{level, text}`, `failed_requests` items `{url, reason, status?}`, `actions` items `{i, do, ok, value?, error?}`, `timing` is `{ready_ms, total_ms}`, and over HTTP `screenshot` is `{url, expires_at, width, height, base64?}`.
The `check` folded into `part_create`/`part_edit` is `{version, passed, blank, height, errors, console, failed_requests, actions, timing, screenshot}`; `passed` is true when there's no `error` or `unhandledrejection` and `blank` is false.
An action is one of `{do: "click", selector}` or `{do: "click", x, y}`, `{do: "type", selector, text}` (≤ 1,000 characters), `{do: "wait_ms", ms}` (≤ 2,000) and `{do: "eval_return", expr}` (a JS expression, a promise awaited; its JSON value, ≤ 4 KiB, comes back as `value`).
In `part_check` output every `url` is a path within the part with the cap removed, and `timing.ready_ms` is `null` when the page never called `partifactory.ready()`. Times in every output are ISO 8601 UTC strings.
A `part_data` doc is `{collection, doc_id, data, rev, updated_at, updated_by, provenance}`, and a snapshot `{id, taken_at, reason, docs, bytes}` (`reason` is `scheduled`, `contribute`, `before_restore` or `before_reset`); `policy` is `{mode, keep}`. The snapshot ops act on shared data only and need `parts:write` and the `edit` role. `who` in `part_link_for` is `"admin"` for both Slice 1 actions.
`update` on `part_edit` and `advisories` on `part_get` arrive after Slice 1 with §3h; `docs/agent-contract.md` gains both sections in the same change that ships them (§4b).
The same goes for previews (§4c), right after Slice 1: a `preview` input on `part_create` and in `part_edit {meta}` is `{video?: path | "record" | "off" ["off"], poster?: path, actions? (≤ 20), seconds? (2–10) [6]}`, `part_check` gains `record` [false], `part_get` gains `preview`, and `preview_invalid` joins the error catalogue. An `advisories` item is `{ecosystem, package, version, osv_id, severity, context, fixed_in, state, attribution}`.

### Names: semantic, chosen by the agent

A part's name *is* its hostname, and its hostname is its link (`https://<name>-part.example.com/` in the example deployment).

- `part_create` takes an optional `name`. Without one, it's derived from the title ("Sleep chart" → `sleep-chart`).
- `part_create {check_only: true, name?, title?, description?}` → available? If not, ranked alternatives that are valid and free right now.
  The generator is deterministic, with no LLM: drop stop words, try 2- and 3-token subsets of the title and description, then the space as a prefix, then numeric suffixes last.
- A clash returns `name_taken` **with** suggestions, so the agent retries in one step.
- Rules: `[a-z0-9-]`, 1–58 chars, no leading/trailing/double hyphens. Reserved in code, regardless of config: `blobs` (the shared blob host). Reserved by site config: infrastructure words and names that mirror existing services.
- **Names are not secret.** A private part's name is in its URL, and unfurlers show URLs. The shared guide tells agents to pick a neutral name for sensitive parts.
- **Renames are rare and admin-confirmed**: the old hostname 301s (query string stripped) for good, and **a name that was ever used stays reserved**, so a future part can't collide with an old redirect.
  Home-screen installs live on the part host, whose old name 301s to the new one, and there's no device-local state to strand (personal state is server-side).

### `part` CLI

Generated from the registry: `part <op> [--flags | --json @file | -]` for any op, plus ergonomic aliases for the common ones:
`part login` (device code) · `part put ./dir --name …` · `part edit <name> --replace 'old' 'new'` · `part get <name> [--file f]` · `part find …` · `part check <name>` · `part watch` · `part open <name>` · `part skill install`.
`--json @file` (or `--json -` for stdin) sends that JSON as the op's input, and any `--json` makes the output JSON; without it, output is human text. `part help <op>` prints the description, schema and examples.
Credentials come from `PARTIFACTORY_TOKEN` if set, else the OS keychain, else `~/.config/partifactory/` (mode 600). The server comes from `PARTIFACTORY_URL`.
Binaries ship as release assets on any forge (the passive GitHub mirror's Releases in the example deployment) and through `cargo binstall`.

### Events: one log, three deliveries

An append-only `events` table, `{seq, at, part_id, space_id, type, actor, payload}` (the DDL; surfaces render `actor` as `{kind, id, on_behalf_of}`), retained **30 days**. A cursor older than that gets `cursor_expired {oldest}`. This is the agents' feed, and it stays as it is.
- **Long-poll** (every surface, MCP included): `part_watch {since, wait: 25, part?, kinds?}` or `GET /api/v1/events?since=…&wait=25` returns early on the first matching event.
- **SSE** (HTTP): the same URL with `Accept: text/event-stream`. `id:` is the event id, `Last-Event-ID` resumes, and a 25 s heartbeat stays under any proxy's idle timeout (Cloudflare's is 100 s). The workspace UI and agents' own servers use it.
- **Webhooks**: to admin-configured URLs only, the same event JSON, signed with HMAC (`X-PF-Signature`, documented so any consumer can verify it), retried with backoff, with the event id as the idempotency key. Targets are site config, never set by agents, which rules out SSRF.
  A chat agent (Hermes in the example deployment) is just one webhook consumer.
- *Why one log:* nothing depends on MCP notifications, which stateless Streamable HTTP doesn't have anyway. An MCP client that wants a push feel calls `part_watch` in a loop.

### No notifications for people, and no approval queues

partifactory sends people **no messages**: no web push, no mention or reply notifications, no per-person notification settings. And it keeps **no queue of things waiting for a person**. Everything that needs a person is where they already are:
- **People-only actions** reach the person as a deep link from the agent, in the agent's own channel: Hermes says "Tap to make it public: `https://pixel-part.<domain>/#share=public`" in Telegram. The link opens the sheet pre-filled; nothing is pending if they ignore it. partifactory never messages Telegram itself.
- **Asks for access** from signed-in people are a short list in the part's share sheet, with **Add** and **Dismiss**.
- **Device-code connect** is started by the person, so no notice is needed. It's sign-in, not a queue.
- **Agent-made inward shares** are an entry in the share sheet and the activity feed ("Hermes shared this with the Family group · Undo"), not a message.
- "Shared with you" shows up in the gallery's **Shared with me** filter and the activity feed.
- *Why:* every channel partifactory owns (web push, email, a chat bot) is another integration to run and another place where a title can leak. The household already talks to its agents, and the agents already have the event log.

**The only push: optional ops alerts for the admin.** Off by default. One generic webhook URL in config (`[alerts] webhook_url`, HMAC-signed JSON `{alert, severity, detail, at}`), which ntfy, Hermes or anything else can consume.
Conditions: the disk budget at 80 %, any store at 80 % of its cap (§5h), no export in 48 h, the build worker or a backend crash-looping, the IdP unreachable for 10 minutes, a critical advisory on a backend or on a part with a contribute link (§3h). The admin Health page shows the same things, so nothing depends on the webhook.

### Example consumers: a chat agent and an embedder

These are clients of the contracts above, not dependencies. The example deployment has two.
- **Hermes** (a chat agent on Telegram) uses MCP + the `partifactory` skill, with one connection per household member (§3b). It posts the part's **own host link** (and, for people-only actions, the `part_link_for` deep link), with `link_preview_options {prefer_small_media: true}`,
  so the unfurl is the small content-free card, plus inline buttons **Open** and **Ask for a change** (Hermes asks for the text and files a `to_agent` comment with that person's own connection).
  Before posting a new part, Hermes runs `part_check` and fixes what it finds. Whether screenshots go into chat is Hermes's own setting (owner's direct chat only, never a group), not partifactory's.
- **The family hub** (a household dashboard) is listed in `[embed] frame_ancestors` and embeds family parts: the part host root (the wrapper, with its chrome), or `/c/` directly with a server-minted `src` and `pf-embed.js`, which handles `pf:regrant`. It has its own service identity. The family hub's knowledge base stays separate from partifactory.

## 4a. Workspace UI and visual identity

The workspace is the workspace host (`parts.example.com` in the example deployment): gallery, search, share sheet, history, editor, comments, connected agents, admin and Insights. The **part view** lives on each part's own host, as the trusted wrapper (§2), and carries the same chrome. Its theme is **Stockroom** (approved by the admin).
Parts are not themed by it. Agents get a separate, optional design system, **Jig** (§4a.8).
The published mockup is at **https://claude.ai/artifact/7EMCHXHWt1gZxvTgbcSTQr** (static, self-contained, light and dark). Where it differs from this section, this section wins.

### 4a.1 What shartifacts looks like (the reference)

shartifacts is **warm editorial**: a magazine more than a tool. A greige paper canvas (`#f9f8f6`), cream surfaces, warm near-black ink, one terracotta primary (`#b5523b`);
**Cormorant Garamond** display serif, **Inter** body, **JetBrains Mono** for code; a 284 px left sidebar library; uppercase tracked eyebrows, pill badges, an eight-armed asterisk mark, soft radii.
Its design system is handed to agents (`get_design_system`) and used to render Markdown, slides and code.

Keep in spirit: calm paper surfaces, one confident accent, typography doing the work, a design system offered to agents. Change: the editorial serif, the pills, the sidebar-first layout and the cream monoculture, which would make partifactory read as a palette swap.

### 4a.2 The theme: Stockroom

**Concept.** A well-kept workshop parts cabinet. Every part is a **stocked item**: a drawer front with a **catalogue tag**, a **part number** (`P-0142`), a **revision** (`REV 12`) and a bin code for its kind (`TSX`, `MD`).
The workspace is the stockroom: you browse drawers, pull one out, and see who's working on it. shartifacts is an essay; partifactory is a catalogue.

**It stays tasteful because the metaphor lives in information design, not decoration:**

| Use | Never use |
| --- | --- |
| Part numbers, revisions and bin codes as real data, in mono | Wood grain, rivets, metal textures, hazard stripes, stencil fonts |
| One manila **tag** surface, the only warm colour, for labels | Skeuomorphic drawers, handles or cardboard |
| A clipped corner (the "tag notch") on cards and chips, 6 px | Rope, string, hole-punch icons |
| Hairline rules, registration **corner ticks** around the part frame | Drop-shadow-heavy floating cards |
| Condensed caps for labels, like a catalogue column head | Uppercase everywhere |
| A faint drafting grid on the "being made" placeholder and cards only | A grid behind everything |

**Colour concept.** Drafting-paper neutrals (slightly green-grey), graphite ink, **cutting-mat green** as the one signal colour, manila tag for labels.
**Amber is reserved** for "Public link" and nothing else, so amber always means "outside the house". Agents get **blueprint blue**, so "Hermes is editing" is never confused with an action you can take.

**Brand mark.** A drawer front: a rounded square with a label slot in the upper third and a short pull below it. One path, works at 16 px, monochrome by default, green only in the favicon.
**Wordmark.** `partifactory` in lowercase, Plex Sans Condensed SemiBold, tracking −0.01 em, mark on the left. On the sign-in page and cards: "Parts, made by you and your agents."

### 4a.3 Design tokens

The chrome uses the `--pf-*` namespace. Part content never sees these. Dark mode follows `prefers-color-scheme` unless the person picks one (stored per device in `localStorage` on the workspace origin, with a server-side preference as the default for new devices).

```css
:root {
  color-scheme: light;
  /* Surfaces */
  --pf-paper:        #f4f3ee;  /* canvas: drafting paper */
  --pf-raised:       #fbfbf8;  /* cards, sheets, bars */
  --pf-sunk:         #e9e8e1;  /* wells, inputs at rest, code, skeletons */
  --pf-rule:         #d6d4ca;  /* hairlines (decorative, not a boundary) */
  --pf-rule-strong:  #8e8b80;  /* input borders, focusable outlines: 3:1 */
  --pf-scrim:        rgb(20 22 24 / 40%);
  /* Ink */
  --pf-ink:          #191b1d;
  --pf-ink-2:        #3a3d41;
  --pf-ink-3:        #5c5f64;
  /* The tag: labels, part numbers, bin codes */
  --pf-tag:          #ecdcae;
  --pf-on-tag:       #3b3015;
  /* Signal: the one action colour (cutting-mat green) */
  --pf-signal:       #1c6a47;
  --pf-on-signal:    #ffffff;
  --pf-signal-text:  #1c6a47;
  --pf-signal-wash:  #dbeae1;
  --pf-on-wash:      #14503a;
  /* Public: amber is only ever "anyone with the link" */
  --pf-public:       #f0b43c;
  --pf-on-public:    #1f1606;
  --pf-public-text:  #8a5200;
  --pf-public-wash:  #fbecc8;
  /* Agents: blueprint blue */
  --pf-agent:        #1f5d8c;
  --pf-agent-wash:   #dde8f2;
  /* Status */
  --pf-danger:       #b02a20;
  --pf-danger-wash:  #f6dedb;
  --pf-diff-add:     #d9ecdf;
  --pf-diff-del:     #f5dcd8;
  --pf-focus:        0 0 0 2px var(--pf-paper), 0 0 0 4px var(--pf-signal);
}

/* Applied under :root[data-theme="dark"], and under prefers-color-scheme: dark unless data-theme="light" */
:root[data-theme="dark"] {
  color-scheme: dark;
  --pf-paper:        #121416;
  --pf-raised:       #1a1d20;
  --pf-sunk:         #0d0f10;
  --pf-rule:         #2b2f34;
  --pf-rule-strong:  #6b7078;
  --pf-scrim:        rgb(0 0 0 / 60%);
  --pf-ink:          #ecebe6;
  --pf-ink-2:        #c8c7c0;
  --pf-ink-3:        #9b9da1;
  --pf-tag:          #3e3722;
  --pf-on-tag:       #efe0b4;
  --pf-signal:       #62c495;  /* fill flips light, text on it flips dark */
  --pf-on-signal:    #0b1d14;
  --pf-signal-text:  #6fd0a1;
  --pf-signal-wash:  #173327;
  --pf-on-wash:      #a9e6c8;
  --pf-public:       #eab040;
  --pf-on-public:    #1f1606;
  --pf-public-text:  #f2c25e;
  --pf-public-wash:  #3a2e12;
  --pf-agent:        #86bde6;
  --pf-agent-wash:   #16293a;
  --pf-danger:       #ff8b80;
  --pf-danger-wash:  #3a1916;
  --pf-diff-add:     #15301f;
  --pf-diff-del:     #3a1b18;
}
```

**Contrast (WCAG 2.2, computed).** Every text pair is AA (4.5:1) or better. Most are AAA.

| Pair | Light | Dark |
| --- | --- | --- |
| `ink` on `paper` / `raised` / `sunk` | 15.6 / 16.7 / 14.1 | 15.5 / 14.2 / 16.1 |
| `ink-2` on `paper` / `raised` | 9.8 / 10.5 | 10.9 / 10.0 |
| `ink-3` on `paper` / `raised` / `sunk` | 5.8 / 6.2 / 5.2 | 6.8 / 6.2 / 7.1 |
| `on-tag` on `tag` | 9.5 | 9.0 |
| `on-signal` on `signal` (buttons) | 6.6 | 8.2 |
| `signal-text` on `paper` / `raised` | 5.9 / 6.3 | 9.9 / 9.0 |
| `on-wash` on `signal-wash` | 7.5 | 9.6 |
| `on-public` on `public` (amber chip) | 9.6 | 9.2 |
| `public-text` on `paper` / `public-wash` | 5.8 / 5.5 | 11.1 / 8.0 |
| `agent` on `paper` / `agent-wash` | 6.3 / 5.6 | 9.2 / 7.4 |
| `danger` on `paper` | 5.9 | 8.1 |
| `rule-strong` on `paper` (non-text UI, 3:1 needed) | 3.1 | 3.7 |

`--pf-rule` is decorative only. Anything a person must perceive as a boundary uses `--pf-rule-strong` or a fill. Colour is never the only signal: the audience chip always has an icon and words, diff lines have `+`/`−` gutters, and errors have an icon and text.

### 4a.4 Type

**Self-hosted IBM Plex** (SIL OFL 1.1, licence in `NOTICE` and at `/licenses/fonts`), Latin + Latin-Extended `woff2` subsets embedded in the binary, `font-display: swap`. **No third-party font CDN, ever.**

| Role | Family | Use |
| --- | --- | --- |
| `--pf-font-sans` | IBM Plex Sans (400, 500, 600) | Body, UI |
| `--pf-font-label` | IBM Plex Sans Condensed (500, 600) | Wordmark, titles, column heads, chips |
| `--pf-font-mono` | IBM Plex Mono (400, 500) | Part numbers, revisions, bin codes, hostnames, code, diff |

```css
--pf-font-sans:  "IBM Plex Sans", system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
--pf-font-label: "IBM Plex Sans Condensed", "Arial Narrow", "Roboto Condensed", system-ui, sans-serif;
--pf-font-mono:  "IBM Plex Mono", ui-monospace, "SF Mono", Menlo, Consolas, monospace;
```

Budget: about 160 KB for the five cuts, cached `immutable`. If that's too heavy for P1, ship **Plex Mono only** (the identity lives in the numbers) and system sans for the rest. That's the agreed fallback, not a regression.

| Token | Size / line | Font | Use |
| --- | --- | --- | --- |
| `--pf-t-display` | 32/36, −0.01em | label 600 | Empty-state headings, sign-in |
| `--pf-t-title` | 22/28 | label 600 | Page titles ("Parts") |
| `--pf-t-heading` | 17/24 | sans 600 | Sheet titles, section heads |
| `--pf-t-body` | 15/22 | sans 400 | Default |
| `--pf-t-small` | 13/18 | sans 400 | Meta, help text |
| `--pf-t-label` | 11.5/14, +0.08em, caps | label 600 | Column heads, chips |
| `--pf-t-mono` | 12.5/16, `tnum` | mono 500 | Part numbers, revisions, times |

Numbers are tabular everywhere. Relative times ("3 min ago") carry an absolute `title` and `datetime`.

### 4a.5 Spacing, radius, elevation, motion, icons

```css
:root {
  --pf-s-1: 4px;  --pf-s-2: 8px;  --pf-s-3: 12px; --pf-s-4: 16px;
  --pf-s-5: 24px; --pf-s-6: 32px; --pf-s-7: 48px; --pf-s-8: 64px;
  --pf-r-1: 3px;   /* chips, tags, kbd */
  --pf-r-2: 5px;   /* buttons, inputs */
  --pf-r-3: 8px;   /* cards, sheets */
  --pf-r-4: 14px;  /* phone bottom sheet top corners only */
  --pf-notch: 6px; /* the tag notch: clip-path on the top-right corner */
  --pf-e-1: 0 1px 0 var(--pf-rule);
  --pf-e-2: 0 1px 2px rgb(16 18 20 / 6%), 0 4px 12px rgb(16 18 20 / 6%);
  --pf-e-3: 0 12px 40px rgb(16 18 20 / 18%), 0 2px 6px rgb(16 18 20 / 8%);
  --pf-ease: cubic-bezier(.2, .7, .2, 1);   /* "drawer slide" */
  --pf-d-1: 120ms;  --pf-d-2: 200ms;  --pf-d-3: 320ms;
}
@media (prefers-reduced-motion: reduce) { :root { --pf-d-1: 0ms; --pf-d-2: 0ms; --pf-d-3: 0ms; } }
```

- **Layout**: gallery cards on an 8 px grid, `repeat(auto-fill, minmax(240px, 1fr))`, 16 px gaps; 24 px page gutters on desktop, 16 px on phones; max width 1280 px; the part view is full-bleed.
- **Elevation**: bars and cards use hairlines (`e-1`). Only sheets, menus and toasts float (`e-3`). Hover raises a card's border to `rule-strong` and nudges the thumbnail 2 px up.
- **Motion**: sheets slide 12 px and fade (`d-2`). A new version cross-fades the frame (`d-3`), never a flash of blank. The only looping motion is the agent activity dot (1.6 s opacity pulse), which stops under reduced motion.
- **Icons**: one inline SVG sprite, **1.5 px stroke on a 20 px grid**, square caps and mitred joins, `currentColor`. Hand-drawn, or adapted from **Lucide (ISC)** / **Tabler (MIT)** with the licence noted. About 40 glyphs.
  The agent glyph is a "node" (a circle with three spokes), not a robot head. Emoji are never used as icons.
- **Kind bin codes** replace file-type icons: a mono caps code on a tag chip (`HTML`, `TSX`, `MD`, `TXT`, `SVG`, `DECK`, `MMD`, `CODE`, `FILES`). A part with a backend adds a second chip, `SVC`.

### 4a.6 Screens

Shared chrome: a **48 px top bar**: mark + wordmark (home), search (`/`), theme toggle, avatar menu (connected agents, sign out, sign out everywhere, the admin page, for the admin).
Keyboard: `/` search, `g p` parts. There's no `n`: agents make parts. "Paste HTML…" lives in the menu.

**Gallery (`/`, "Parts")**
- Header: "Parts" + a mono count, a segmented filter `Recent · Mine · Shared with me · Family`, a kind filter, grid/list toggle (grid on desktop, list on phones).
- **Card = drawer front** (`raised`, `r-3`, hairline, tag notch): a 16:10 thumbnail; a label strip (tag chip with part number and bin code, title, name in mono); a foot (audience chip, `REV 12`, "Hermes · 3 min ago", open comment count);
  live state on the thumbnail (a blue "Hermes editing" pill while a lease is held, "being made…" on version 0, an expiry badge on scratch parts).
- Sections: "Pinned", then "Recent" grouped by day (`TODAY`, `YESTERDAY`, `THIS WEEK`, `EARLIER`) with caps column heads.
- List view: 56 px rows with a 72×45 thumbnail, title + name, bin code, audience chip, rev and time. The whole row is the hit target.
- **Thumbnails.** In Slice 1 the thumbnail is the newest passing render screenshot of the live version, or the "being made…" frame (decision 53). After Slice 1 a part with a preview clip plays it muted while its card is on screen (§4c). In order of preference: a screenshot from the render sidecar (if it's on, §4c); the agent's own (`meta.thumbnail`, PNG/WebP ≤ 256 KB); a server-drawn SVG spec-sheet thumb (title, first heading, kind, first lines, a colour strip from the part's CSS).
  So the grid never has holes. Each card links straight to its part host. Thumbnails are served by the workspace at `/api/v1/thumbs/<part>?v=<n>`, session- and grant-checked, `private, immutable`, never edge-cached.
- A **Bin** view lists deleted parts with days left and Restore.

**Search.** The top-bar field expands into a results sheet as you type. Each hit: thumbnail, title with matches marked (`signal-wash`), a 2-line snippet and where it matched.
Filters are typed or clicked as chips (`kind:tsx`, `space:family`, `by:hermes`, `has:data`, `updated:week`). "Did you mean …" for zero-result typos. An empty result offers "Ask an agent to make it", which copies a prompt.

**Part view (the wrapper at `https://<name>-part.<domain>/`).** The part is the page. One **44 px bar**, and everything else is a sheet or a rail that closes.
The wrapper is a small standalone bundle of the same Stockroom components (a shared blob, no framework hydration), talking to the workspace only through the chrome API (§2). The share sheet, history rail and comments rail open inside it; **Edit** and **‹ Parts** go to the workspace host.

```
┌───────────────────────────────────────────────────────────────────────────────────────────┐
│ [mark] ‹ Parts │ [P-0142 TSX] Photo finder  photofinder · REV 12 ▾ │ ● Hermes is editing app.tsx │ Family │ Share │ Edit │ Comments 3 │ ⋯ │
├───────────────────────────────────────────────────────────────────────────────────────────┤
│ ⌐                                                                                     ¬   │
│                    the part, full-bleed, sandboxed on its own host                        │
│ ∟                                                                                     ┘   │
└───────────────────────────────────────────────────────────────────────────────────────────┘
```

- **Left**: back, the tag (part number + bin code), the title (rename is admin-confirmed), the name in mono.
- **Version chip** `REV 12 ▾` opens the history rail. When viewing an older version it turns tag-coloured: `Viewing REV 9 of 12 · Back to latest`.
- **Agent activity**: a blue pulse dot + "Hermes is editing *app.tsx*" (from leases and in-flight edits), "Claude Code saved REV 12 · just now" for 30 s, or "Didn't build in REV 13: still showing REV 12" in danger colour, linking to the error.
- **Audience chip**: icon + words (`Only me`, `3 people`, `Family`, `Everyone signed in · 15`, `Public link`). Click opens Share.
- **Share**, **Edit** (hidden without `edit`), **Comments** with a count, and **⋯**: Open standalone ↗, Copy link (always `https://<name>-part.<domain>/`), Details (agent notes, runtime errors, analytics, files, data documents, data snapshots, and for a part with a backend its state, logs, secret names and the note that volumes are covered by exports, not snapshots), Debug (the admin, §5g), Download as zip, Pin, Delete (to the bin).
- **Frame**: an `<iframe>` with `sandbox="allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-modals allow-downloads allow-top-navigation-by-user-activation"` (belt and braces with the header) and a server-minted `/c/<cap>/` `src`.
  The wrapper (and the workspace's own preview frames) identify the frame's messages by `event.source === frame.contentWindow`, never by `event.origin` (which is `"null"`). Registration corner ticks sit just outside the frame: the boundary between our chrome and someone else's page.
- **Runtime errors**: a danger tick on ⋯ and "2 runtime errors" in Details, with "Send to agent" (a `to_agent` comment with the stack).
- **Advisories** (owner and editors, §3h): when the live build has an open advisory, a badge sits by ⋯ ("2 advisories"; danger colour only when the severity in context is raised, otherwise `ink-3`). It opens the list (package, version, OSV id with its attribution, severity in context, fixed in) with **Update libraries** (a new version, live under the part's `live` setting; undo is the normal restore) and **Ignore…** (asks for a reason). A line says inline code isn't checked.
- Rails (comments, versions, details) are 360 px on desktop and push the frame narrower rather than covering it.
- **Install**: the wrapper serves a per-part web app manifest (`id` and `scope` = `/` on the part host, name, icon, theme colour), so each part is its own home-screen app on its own origin.

**Share sheet (the audience ladder).** 480 px on desktop, a bottom sheet on phones. Top to bottom:
1. "Share *Photo finder*" with the tag chip.
2. **The ladder**, a five-step segmented control, narrow → wide: `Only me` · `People` · `A group` · `Everyone signed in` · `Anyone with the link`. The last two show only for the admin. The last is amber, with "The worst option: no sign-in, anyone who gets the link can open it."
3. **The live sentence**, in `t-heading`: "**You and 3 people** can see this. **A family member** can also edit." It updates before anything is saved.
4. **Move to Family**: one button when the part is in a personal space.
5. Rung detail: *People* (directory type-ahead with per-person role); *A group* (picker with live member counts; "Make a group for this" for the admin); *Everyone signed in* (count and role);
   *Anyone with the link* (the admin): mode **`Can view` (default) · `Can contribute`**, with "Anyone with the link can use it. Only you and people you add can change it." under Can contribute, and **Roll back data**, **Reset data** and **Freeze** while it's on (comments need the separate opt-in); **expiry picker `Never` (default) · 1 day · 7 days · 30 days**, the URL with **Copy**, **opened 14 times · last 2 h ago**, **Preview: Video · Image · Card only** (Video by default when a clip exists, with the poster shown; §4c), and **Revoke link**.
6. **Data: view only** switch (`shared_data_writers: "editors"`; off by default, so viewers can write), and the `unsandboxed` flag (the admin, danger red).
7. **Agent-made grants**: "Hermes shared this with the Family group · Undo".
8. **Access asks**: "A family member asked for access" with **Add** and **Dismiss**. Nothing from agents appears here.
9. **Pending invites** for this part, with status and revoke.
10. Footer: "Changes save as you go." Revocations show a 10 s undo toast instead of a confirm.

The sheet opens at a rung from a deep link (`#share=public`, `#share=contribute`, `#share=invite`, `#share=group`), with nothing saved until the person confirms. Step-up is in-sheet: "Confirm it's you" with a passkey button inline, then the action completes after the biometric, with no navigation.

**Invites** (the admin). Share → "Invite someone" (or Admin → Invites): two choices, `Invite link` (passkey-only: "For one person" / "For a group chat: up to [5] people", expiry 7 days, what they'll get, **Copy link** once, with a QR code) and `By email`.
The invitee's side (`/i/<id>`, then the partifactory-invite flow with partifactory branding): one centred card, "*You're invited to* Photo finder", "What should we call you?", **Create passkey**, then straight to the part.
A used or expired link says "This invite has been used. Ask whoever sent it for a new one." The invite list shows status chips, sent/opened times, "Send a new passkey link" and "Clean up".

**Version history, diff and restore.** The history rail opens from the `REV` chip: a vertical **revision ledger**, each row `REV 12` · "Hermes for the admin" · relative time · one-line summary, with a green `LIVE` tag on the live version and a danger `DIDN'T BUILD` tag on failed ones.
Clicking a row previews it in the frame. **Compare** picks any two rows (default: selected vs live): a file list with `+/−` counts and a unified diff (side-by-side ≥ 1100 px), hunks collapsed beyond 3 lines of context.
**Restore REV 9** appends REV 13 and shows "Restored as REV 13 · Undo". `[` and `]` step through versions.

**In-place editor (CodeMirror 6).** `Edit` adds **Preview · Split · Source**. Split = file tree + CodeMirror + live preview rendered from the **draft** (a cap with a `d` claim, never the live version).
Syntax in four hues (keyword: signal; string: agent blue; number/const: public-text amber; comment: `ink-3` italic), Plex Mono 13.5/20. Drafts autosave to the server.
When an agent lands a version mid-edit: "Hermes saved REV 13 while you were editing. Your changes were merged." Overlaps open **Resolve** (Keep mine / Keep theirs / Edit per hunk).
A lease shows a blue dot and "Hermes is editing this file until 14:32"; saving asks once. Markdown and slides get block-aware editing with pending placeholders. Phone: Source, plus the formatted view for Markdown and `text` parts (below), and tap-to-edit text for other kinds in P4 (§3c).

**Formatted view (Markdown and `text` parts, P2, decision 54).** `Edit` on a Markdown or `text` part adds **Formatted**, its default mode: you edit the page as it reads, like Claude Docs, in Jig typography.
A small toolbar (heading level, bold, italic, inline code, link, bulleted, numbered and task lists, quote, code block, table, divider), Markdown shortcuts as you type (`## `, `- `, `**…**`, `` ` ``), and pasted rich text converted to Markdown.
- **Every change is a block edit.** Each top-level block (§3d) is its own editing island, using ProseMirror with `prosemirror-markdown`, vendored as a shared blob beside CodeMirror. Only the blocks you touch are serialised again; every other block stays byte-identical, so a formatted save never reformats the document and Compare shows only what you changed.
  A block the view can't represent faithfully (raw HTML, front matter, footnote definitions, a table with merged or multi-line cells) opens in place as a small inline source box.
- **Saving.** Changes autosave to the server draft (the same per-person draft as Source, so switching modes loses nothing) and commit as one version, message "Edited in the page", on ⌘S, **Done**, leaving, or 30 s idle. The commit is `replace_block`, `insert_after` and `delete_block` ops with `if_hash`, so it rebases by block id.
- **Agents in the same document.** Agent commits to blocks you aren't in arrive in place over the event feed (`part_watch`), with a brief blue edge and "Hermes edited". A commit to the block under your cursor keeps your text and offers **Resolve** for that block only.
  A lease on a block or section shows "Hermes is writing this section", and `pending` blocks render as placeholders that fill in as the agent writes. Typing in a leased block asks once, and a person can always override.
- **Comments** start from a selection: select → **Comment**, anchored to the block id and the quote, in the same rail.
- **`text` parts**: the formatted view is the text shell made editable, with line numbers and line-anchored comments kept; saves are anchor edits.
- **Phones**: the same view, with the toolbar above the keyboard and 44 px targets. For Markdown and `text` it supersedes P4's tap-to-edit, which stays for HTML and the other kinds.
- **Who**: `edit` and up, as for Source; never guests or public-link viewers.
- **Not co-typing**: two people in the formatted view at once see each other's commits arrive block by block, not keystroke by keystroke (§3d).

**Comments.** A right rail, threads ordered by position then time, each with its anchor quote. **"For the agent"** toggle (the `to_agent` flag), shown as a blue chip. Then "Hermes is on it", then "Fixed in REV 14 · View diff".
Comments from public-link viewers or other people carry a small "untrusted for agents" marker for the owner. Resolve, reopen, "show resolved". Phone: a bottom sheet with the composer above the keyboard.

**Connecting an agent (`/device`).** Two ways in: a chat agent (Hermes) posts "Connect to partifactory: `parts.example.com/device?code=KMQT-4XRA`", or `part login` prints the URL and code. On a phone, single column, large type:
the code in big mono with "Make sure this matches what the agent shows"; **who** ("This device says it is Hermes · hermes/1.4", unverified, plus coarse origin and age); **what it wants** in plain words, each removable
("Read and edit parts", "Read and write part data", "Comment", "Share with people already in the household", "Deploy backends and set their secrets" — the last two off unless requested); **where** (spaces as toggles); the lifetime is fixed (30 days unused, 90 days at most) and stated on the sheet;
**Allow** (filled, full width, triggers the passkey) and **Deny**. Success: "Hermes is connected for you." The same sheet serves the browser OAuth flow, plus the redirect host.
**Connected agents** (avatar menu) lists each connection: "Hermes for you", scopes, spaces, last used, token type (`OAuth` / `device`), grants it made, with **Revoke** (undo toast; cascades per §3b).

**Admin (`/admin`, admin only).** **People · Groups · Invites · Agents · Public links · Backends · Audit · Health · Insights**. A persistent step-up chip: `Admin unlocked · 8 min` or `Locked` with a passkey button.
*Public links*: every live link, its part, created, expiry (usually "never"), **open count and last opened**, revoke, and a red **Revoke all public links** kill switch (hold to confirm, 1 s).
*Backends*: every part with containers, its state, limits and secret names (values are never shown), and the **LAN allowlist** per part (add with step-up; `/admin/backends/<part>#lan` opens it pre-filled).
*Audit*: a filterable ledger, "who · as whom · what · before → after · when", in mono. *Health*: the `/readyz` checks, degraded indicators (IdP, directory poll), backup layers (§5c), the disk budget and each store's size against its cap (§5h), the advisory DB's age and the open advisories by layer (§3h), any safe default switched off, version and last drill.
*Insights*: analytics and observability pages, the trace waterfall and the log viewer (§5g).

**Empty, loading and "being made…" states.**
- Empty gallery: the drawer mark at 64 px, "No parts yet. Parts are made by your agents. Connect one, then ask it for something." with **Connect an agent** and a copyable `claude mcp add --transport http partifactory https://parts.example.com/mcp`.
- Loading: skeleton drawer fronts (no shimmer). The frame shows the tag, title and corner ticks at once, and no spinner for the first 400 ms.
- **"Being made…"** (version 0): a **drafting sheet** with a faint 8 px grid and a title block bottom-right (`P-0215 · grocery-planner · REV 0 · Hermes for the admin`), and "Hermes is making this. It'll appear here as soon as the first version lands."
  It live-updates and cross-fades to REV 1. Long docs render pending blocks as tag-coloured placeholders that fill in live. If the first version doesn't build: "The first version didn't build. Hermes has the error."

**Error pages.** Paper, the mark, a mono status line like a catalogue code, one sentence, one action.

| Code | Line | Action |
| --- | --- | --- |
| `404 · NOT IN STOCK` | "There's no part called *grocery-planr*." + up to 3 close names you can see | Back to Parts |
| `403 · NOT SHARED WITH YOU` | "This part exists, but it isn't shared with you." (no title) | **Ask for access**, Sign in as someone else |
| `410 · LINK EXPIRED` | "This link expired on 12 Oct." (only for public links with an expiry) | Ask the sender for a new one |
| `410 · INVITE USED` | as under Invites | — |
| `429 · SLOW DOWN` | "Too many tries. Wait a minute and try again." | — |
| `500 · SOMETHING BROKE` | "partifactory hit an error. It's been logged as `req 8f1c…`." (the request id) | Try again |
| `503 · DOWN FOR MAINTENANCE` | a static page in the same style, inlined CSS, shipped in `deploy/proxy/` for the proxy to serve when the origin is down (the Cloudflare adapter serves it from a Worker) | — |

Errors inside a part stay inside its frame; we only add the Details tick.

**Mobile.** Phone-first, tested at 360 × 740 and on real devices. Gallery: a 56 px bar, list view by default, horizontally scrolling filter chips.
Part view: the part takes the whole screen below a **40 px bar** (back, tag + title, audience icon, ⋯), which hides on scroll down. Agent activity is a 2 px blue line under the bar.
⋯ opens a bottom sheet: Share, Versions, Comments, Edit, Open standalone, Copy link, Details. All sheets are bottom sheets with a grab handle, 85 % max height and the primary action in thumb reach. Safe-area insets respected. Hit targets ≥ 44 × 44. No hover-only affordances.
The workspace installs as a PWA, and each part installs from its own host.

### 4a.7 Click budgets (UI flows)

These extend §3a's identity budgets, counted the same way. The e2e suite asserts each one by counting `click`/`press`/WebAuthn events along the scripted path, so a regression fails CI.

| Flow | Path | Budget |
| --- | --- | --- |
| Open a recent part | Gallery → card | **1** |
| Find a part by name | `/` → type → Enter | **1 keystroke + type + 1** |
| Open a link posted in chat | tap link | **1** (0 more if signed in; the first open in a tab adds one silent redirect hop, not an interaction) |
| See who can see a part | glance at the audience chip | **0** |
| Share with the family | Share → Move to Family | **2** |
| Share with one person in the household | Share → People → pick | **3** |
| Make a public link + copy (the admin) | Share → Anyone with the link → Copy | **3** (+1 biometric) |
| Revoke a public link | Share → Revoke link | **2**, undo toast |
| Make a public link guests can contribute to (the admin) | Share → Anyone with the link → Can contribute → Copy | **4** (+1 biometric) |
| Freeze a contributed part (the admin) | Share → Freeze | **2**, undo toast |
| Roll back a part's data (the admin) | Share → Roll back data → Restore (newest snapshot preselected) | **3**, undo toast |
| Do what an agent's deep link asks (e.g. make it public) | tap the link → confirm in the pre-filled sheet | **2** (+1 biometric for admin actions) |
| Answer an ask for access | Share → Add (or Dismiss) | **2** |
| Find why someone got a 403 (the admin) | ⋯ → Debug → the request | **3** |
| Copy the link to post | ⋯ → Copy link, or copy the address bar (it's already clean) | **2**, or **0** |
| See what changed in the last version | `REV` chip → Compare | **2** |
| Restore an older version | `REV` chip → row → Restore | **3**, undo toast |
| Undelete from the bin | Bin → Restore | **2** |
| Update a part's libraries after an advisory | advisory badge → Update libraries | **2**, undo is the normal restore |
| Small text fix by hand | Edit → (type) → ⌘S | **2 + type** |
| Fix a sentence in a Markdown doc | Edit (opens Formatted) → click the sentence → (type) → Done | **3 + type** |
| Comment for the agent | Comments → type → "For the agent" → Post | **3 + type** |
| Connect a headless agent on the phone | tap link in chat → Allow → biometric | **3** |
| Connect Claude Code (signed in) | (browser opens) → Allow → biometric | **2** |
| Accept an invite | open → name → Create passkey → biometric | **3 + type** |
| Revoke an agent | avatar → Connected agents → Revoke | **3**, undo toast |

### 4a.8 Part content, and the optional Jig design system

**Part content is not themed by us.** A part is served from its own sandboxed host and gets exactly the HTML, CSS and JS its author wrote. No `--pf-*` token, font or stylesheet reaches it. Claude-written artifacts render unchanged.

**Jig** (a jig is the guide you use to make parts consistently) is an **optional** design system that agents opt into, returned by `design_system` with rules and a starter.
It's deliberately **distinct from the workspace chrome**: neutral white paper, system fonts by default, a blueprint-blue accent, softer radii. Parts should look like *their* content, not like partifactory.
We also use Jig to render the kinds we render ourselves (Markdown, slides, Mermaid, code). **Jig is itself a part** in the `system` space, so the admin edits it like anything else and parts record `meta.jig_version`.

**Delivery.** `jig.css` (≈ 8 KB brotli) is a shared blob at `https://blobs-part.example.com/b/<hash>/jig.css`, allowed by the part CSP. Opt in with one `<link>` and `data-jig` on `<html>`. The import map exposes `@partifactory/jig` for TSX parts (CSS import + a tiny `useJigTheme()` hook).

**Tokens** (`--jig-*`): five root tokens; everything else derives with `color-mix()` and `light-dark()`, so an agent can re-skin a part by setting one or two values.

```css
:root[data-jig] {
  color-scheme: light dark;                         /* follows the viewer; data-jig-theme="light|dark" pins it */
  /* The five you may override */
  --jig-accent:  light-dark(#2f54c9, #8eaaf5);      /* white on #2f54c9: 6.5:1; #0c1530 on #8eaaf5: 7.9:1 */
  --jig-paper:   light-dark(#ffffff, #161719);
  --jig-ink:     light-dark(#1c1d1f, #e9e9e6);      /* 16.9:1 / 14.8:1 */
  --jig-font:    system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
  --jig-radius:  8px;
  /* Derived */
  --jig-ink-2:     color-mix(in oklab, var(--jig-ink) 80%, var(--jig-paper));
  --jig-ink-3:     light-dark(#5f636a, #a3a6ab);    /* 6.0:1 on white, 7.4:1 on dark */
  --jig-surface:   color-mix(in oklab, var(--jig-ink) 4%, var(--jig-paper));
  --jig-surface-2: color-mix(in oklab, var(--jig-ink) 8%, var(--jig-paper));
  --jig-line:      color-mix(in oklab, var(--jig-ink) 14%, var(--jig-paper));
  --jig-line-strong: color-mix(in oklab, var(--jig-ink) 45%, var(--jig-paper));
  --jig-on-accent: light-dark(#ffffff, #0c1530);
  --jig-accent-wash: color-mix(in oklab, var(--jig-accent) 12%, var(--jig-paper));
  --jig-good: light-dark(#1d7a4a, #6fd0a1);
  --jig-warn: light-dark(#8a5200, #f2c25e);
  --jig-bad:  light-dark(#b42318, #ff8b80);
  --jig-mono: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  --jig-font-display: var(--jig-font);
  /* 8 categorical data colours, ordered for colour-blind separation */
  --jig-c1: #2f54c9; --jig-c2: #d97a1e; --jig-c3: #1d8a6b; --jig-c4: #c43d6b;
  --jig-c5: #7a5bc7; --jig-c6: #8a7a2a; --jig-c7: #2a8ab8; --jig-c8: #6b6f76;
  --jig-s1: 4px; --jig-s2: 8px; --jig-s3: 12px; --jig-s4: 16px; --jig-s5: 24px; --jig-s6: 40px; --jig-s7: 64px;
  --jig-t-xs: .8125rem; --jig-t-sm: .875rem; --jig-t-md: 1rem; --jig-t-lg: 1.25rem; --jig-t-xl: 1.75rem; --jig-t-2xl: 2.5rem;
}
```

Presets are one attribute: `data-jig-preset="paper"` (default), `"ledger"` (warm off-white, `ui-serif` display, green accent), `"slate"` (dark-first, cyan accent), `"kid"` (bigger type, rounder, high-saturation accent, for family tools).

| Group | Classes (plain CSS, no JS) |
| --- | --- |
| Layout | `.jig-page` (max 960), `.jig-wide` (1280), `.jig-stack`, `.jig-cluster`, `.jig-grid`, `.jig-split`, `.jig-sticky-bar` |
| Text | `.jig-prose` (Markdown body), `.jig-eyebrow`, `.jig-lede`, `.jig-muted`, `.jig-num` (tabular) |
| Surfaces | `.jig-card`, `.jig-card--flat`, `.jig-callout` (`--good/--warn/--bad`), `.jig-divider` |
| Controls | `.jig-btn` (`--primary/--quiet/--danger`), `.jig-input`, `.jig-select`, `.jig-check`, `.jig-switch`, `.jig-field`, `.jig-seg`, `.jig-tabs` |
| Data | `.jig-table`, `.jig-stat`, `.jig-meter`, `.jig-progress`, `.jig-badge`, `.jig-tag`, `.jig-kbd` |
| States | `.jig-empty`, `.jig-skeleton`, `.jig-toast`, `.jig-pending` (the `pending` block placeholder) |
| Docs and decks | `.jig-slide`, `.jig-deck` (16:9, keyboard nav), `.jig-code` (highlight.js token sets) |

Rules shipped with it: use the tokens, not hex; one accent per part; tables for data; `.jig-stat` for headline numbers; respect `prefers-color-scheme`; use `base: './'` and relative URLs;
use `window.storage` for state; and **never imitate partifactory's chrome** (no tag chips, part numbers or corner ticks), so a part can never be mistaken for the workspace.

### 4a.9 Copy voice and build notes

Plain, specific, short. Sentence case. Say who and what: "Hermes saved REV 13", not "Content updated". No exclamation marks, no "Oops". Amber copy is honest about risk. Numbers get units and real counts ("15 people", never "everyone").

- Leptos components (or templates, per the §5 spike) map one-to-one onto these screens. Tokens live in one `tokens.css` embedded in the binary, with the dark block generated from the same source.
- A contrast test in CI parses `tokens.css` and asserts every pair in §4a.3 (fails under 4.5:1 for text, 3:1 for `rule-strong`).
- Visual regression: Playwright screenshots of the gallery, part view, share sheet and `/device` in light, dark and at 360 px, compared per PR.

## 4b. Agent surfaces: any AI, any surface, one operation set

Any AI that can make an HTTPS request can find and use partifactory. MCP is one of four front ends, not the system.

### One operation registry, defined in Rust

A `pf-ops` crate. Each op is a typed input/output pair (`serde` + `schemars`, `deny_unknown_fields`) plus `NAME`, `SCOPE`, hints and **executable examples**:

```rust
#[derive(Deserialize, JsonSchema)] #[serde(deny_unknown_fields)]
/// Change an existing part. Prefer anchor edits; each `old_str` must occur exactly once.
pub struct PartEdit { pub name: String, pub edits: Vec<Edit>, pub base_version: Option<u64>, pub request_id: String, pub check: Option<bool> }

impl Op for PartEdit {
    const NAME: &'static str = "part_edit";
    const SCOPE: Scope = Scope::PartsWrite;
    const HINTS: Hints = Hints { read_only: false, destructive: false, idempotent: true /* with request_id */ };
    type Output = EditResult;
    fn examples() -> &'static [Example] { &[ex!("fix a typo", {...}, {...}), ex!("stale base, auto-rebased", ...)] }
    async fn run(ctx: &Ctx, input: Self) -> Result<Self::Output, OpError>;
}
```

- Doc comments become descriptions. `examples()` are run by the conformance suite, so the docs can't rot.
- A registry macro lists the ops. Every adapter iterates the registry, and no adapter defines an operation of its own, so nothing can exist on one surface only.
- **Versioning**: `v1` is additive only. A breaking change means a new op name. A checked-in `openapi.json` snapshot test fails CI on any non-additive diff.
- *Why `schemars` + `aide`:* `rmcp` uses `schemars` for tool schemas, and `aide` builds OpenAPI for axum on top of `schemars`, so there's one schema generator. `utoipa` would need a second derive that can drift.
  `rmcp`'s `outputSchema` root must be an object, so non-object outputs are wrapped.
- **No operation depends on an MCP-only feature**: no sampling, no elicitation, no notifications. MCP resources and server instructions are only projections of ops and of the shared guide.

### Adapters

| Surface | Shape | Generated from the registry |
| --- | --- | --- |
| **MCP** | Streamable HTTP at `/mcp`, stateless (spec 2026-07-28, 2025-11-25 compatible: `server/discover`, `Mcp-Method`/`Mcp-Name` headers). Tools = ops, with `inputSchema`/`outputSchema` and annotations. Resources `part://…` as projections. `instructions` = the shared guide | tool list, schemas, annotations, instructions |
| **HTTPS JSON API** | `POST /api/v1/{op}`, JSON in and out, one route per op. Read ops also get REST-style GET aliases (`GET /api/v1/parts/{name}`, `…/files/{path}`) for curl and noun-minded clients. `PUT /api/v1/parts/{name}/files/{path}` takes a raw body. `Authorization: Bearer` | routes, `/openapi.json` (OAS **3.1**, whose JSON Schema 2020-12 matches `schemars` output), `operationId` = op name |
| **`part` CLI** | `part <op> --json` + ergonomic aliases (§4) | `clap` subcommands built from the registry |
| **Skill file** | `GET /skill.md`: Agent Skills format, frontmatter `name: partifactory` and a `description` that triggers on "make/share/edit a page, tool, doc, deck". The body has the workflow, auth per surface, and a compact op reference with one example each. Also an `AGENTS.md` snippet at `/agents.md`. `part skill install [--claude \| --codex \| --path]` writes it to `~/.claude/skills/partifactory/SKILL.md` or appends the snippet to `AGENTS.md` | the op reference |

**The shared guide is `docs/agent-contract.md`**, the canonical agent-facing doc: one page that defines how any AI publishes to partifactory, written to be usable by an agent that has read nothing else (discovery, connecting per surface, every op with its fields, outputs, errors and an example, kinds, shared data, the rules, the error shape, a worked session).
It replaces the `USAGE.md` of earlier rounds. It's the MCP `instructions`, the body of `/skill.md`, the top of `/llms-full.txt`, and the OpenAPI `info.description`, so "make me a packing checklist in partifactory" is enough on any surface.
- **`/skill.md` is generated from the same registry and must match it.** The prose is hand-written. An op section runs from its `<!-- pf:op <name> -->` marker to the next marker or `## ` heading; the generator owns its field table and its `Errors:` line, and the description sentence, the `Output:` paragraph and the example are hand prose. A test rewrites the owned lines from the registry and fails on any diff in `docs/agent-contract.md` or the served `/skill.md`, and parses each example against the op's input schema; the contract's examples also run in the conformance suite.
- It documents only the ops that exist. When an op lands after Slice 1 (`part_find`, `part_comments`, `part_share`, `part_watch`, `design_system`, `part_inbox`), its section is added in the same change.
GPT Actions constraints are checked by a CI lint: every op has an `operationId`, descriptions are ≤ 300 chars (`x-pf-description` carries the rest), and there are ≤ 30 operations.

### Discovery: an agent told only "parts.example.com" can find everything

| Path | What |
| --- | --- |
| `GET /` with an `Accept` that isn't HTML | `text/markdown`: what partifactory is, the four ways to connect, and links to everything below (`Vary: Accept`; browsers get the UI) |
| `/llms.txt` | the llms.txt index: one paragraph, then links to `/skill.md`, `/openapi.json`, `/mcp`, `/llms-full.txt`. Cheap and harmless; no client relies on it |
| `/llms-full.txt` | the guide + every op's description, schema summary and examples, as one file |
| `/openapi.json` | OAS 3.1, ETag = build hash |
| `/.well-known/oauth-protected-resource` | RFC 9728 for the one resource `https://parts.example.com/`: `authorization_servers`, `scopes_supported`, `bearer_methods_supported: ["header"]`, `resource_documentation` → `/llms.txt` |
| `/.well-known/oauth-authorization-server` | RFC 8414 (§3b), also at `/.well-known/openid-configuration` |
| `/.well-known/mcp/server.json` | the MCP registry `server.json` shape (name, description, version, `remotes: [{type: streamable-http, url}]`). **Served, not published** to the public registry: this is a private instance, and the registry doesn't support private servers. The public repo ships a template |

- **No A2A agent card.** A2A describes agents that accept tasks. partifactory is a tool and resource server, not an agent. If Hermes ever speaks A2A, *Hermes* publishes the card and lists partifactory as a tool.
- Every `401` from `/api` or `/mcp` carries `WWW-Authenticate: Bearer resource_metadata="…"` **and** a JSON body explaining the ways to get a token (OAuth and device code), with links. An agent that hits the endpoint blind learns how to authenticate from the error alone.
- Discovery documents are unauthenticated and contain no site secrets or user data: they never list spaces, parts or people.

### Auth for each kind of client (one token model)

| Client | Flow | Token |
| --- | --- | --- |
| MCP clients (Claude Code, Codex: CIMD; Cursor, Gemini CLI: DCR) | auth code + PKCE | opaque access token + rotating refresh token |
| Headless (Hermes, the `part` CLI, CI, cron) | device grant (RFC 8628) | the same |
| GPT Actions and other pre-registered-only clients | auth code with a manual client | the same |

All of them resolve to the same `(person, connection, scopes, spaces)` principal, so the policy engine, audit and revocation are identical everywhere.

### Errors written for an LLM to self-correct

One shape on every surface:

```json
{"error": {
  "code": "anchor_not_found",
  "message": "edits[1].old_str was not found in app.tsx at v14 (you based on v12; v13 changed this region).",
  "hint": "Re-read the region with part_get {name, file: \"app.tsx\", range: [40, 60]} and resend the batch with a corrected edits[1]; edits[0] would apply, but nothing was saved (all or none).",
  "retryable": true,
  "details": {"edit_index": 1, "file": "app.tsx", "context": "…the closest matching region…", "lines": [44, 52], "similarity": 0.83},
  "docs": "https://parts.example.com/llms-full.txt#anchor_not_found"
}}
```

Rules, enforced by a test that every error variant has a hint and an example:
- Every message says **what was expected, what was found, and the next call to make**.
- Unknown fields get "did you mean": "unknown field `content`; did you mean `files`?" (`deny_unknown_fields` + edit distance).
- Wrong-surface mistakes are recognised: a JSON-RPC body POSTed to `/api/v1/…` gets "this is the HTTPS API; MCP lives at /mcp".
- A missing scope names the scope and how to get it. Quota and rate errors carry `retry_after`.
- A part the caller can't see is `not_found`, never `forbidden`, so an error never confirms what's behind a name; `forbidden` is for a missing scope (`scope_needed`), a space the connection may not write (`space`) or a role the person lacks (`role_needed`); `forbidden_view_only` is a shared-data write the part doesn't allow this person (`shared_data_writers: "editors"`) or this link (Can view).
- **HTTP**: every op answers `200` on success, `part_create` included; errors use the statuses below. `/oauth/*` takes `application/x-www-form-urlencoded` and answers JSON, per RFC 6749 and RFC 8628.

| Code | Details | HTTP | CLI exit |
| --- | --- | --- | --- |
| `invalid_input`, `invalid_edit`, `unknown_field` | `path`, `suggestion` | 400 | 2 |
| `unauthenticated` | how to get a token | 401 + `WWW-Authenticate` | 3 |
| `forbidden` | `scope_needed`, `space` or `role_needed` | 403 | 3 |
| `forbidden_view_only` | `scope` (`shared`) | 403 | 3 |
| `step_up_required` | — | 403 | 3 |
| `not_found`, `file_not_found`, `block_not_found` | — | 404 | 4 |
| `name_taken` | `suggestions` | 409 | 5 |
| `if_match_failed` | `file`, `current_sha256` (files); `collection`, `doc_id`, `current_rev` (`part_data`) | 409 | 5 |
| `file_exists`, `request_id_reused`, `version_conflict` | `head` | 409 | 5 |
| `claimed` | `by`, `until`, `scope` | 409 | 5 |
| `anchor_not_found`, `anchor_ambiguous`, `block_changed` | `edit_index`, `file`, `context`, `lines`, `count` | 422 | 5 |
| `base_unknown`, `invalid_against_base` | `base`, `failing edit` | 422 | 5 |
| `cursor_expired` | `oldest` | 410 | 4 |
| `quota_exceeded`, `rate_limited` | `kind`, `limit`, `retry_after` | 429 + `Retry-After` | 6 |
| `too_large` | `limit` | 413 | 2 |
| `storage_full` | — | 507 | 7 |
| `renderer_unavailable`, `backends_unavailable` | `hint` | 503 | 7 |
| `image_not_allowed` | `ref`, `allowed_registries` | 422 | 5 |
| `internal` | `request_id` | 500 | 7 |

`build_failed` isn't an HTTP error: the version *was* saved, so the call returns 200 with `build: {status: "failed", errors: [{file, line, col, message}], live}`. Backend build failures use the same shape, with codes `compose_unsupported_key`, `build_timeout` and `backend_unhealthy` (with the last log lines). Warnings (`cdn_unpinned`, `root_relative_url`, `duplicate_library`, `check_skipped` when `live: "after_check"` had no renderer, and, after Slice 1, `dep_advisory` for an open advisory in the build's libraries or image, §3h) ride in `build.warnings`; `part_check` carries `dep_advisory` too.
In MCP, errors are tool results with `isError: true` and the JSON as `structuredContent`, plus a one-line text rendering, **except authentication failures, which are HTTP 401**. The CLI prints `message` and the first actionable field, or the JSON with `--json`.
OpenAPI has one `Error` schema whose `code` is an enum generated from the catalogue.

### Conformance: one scenario, every surface

`tests/agent_surfaces/scenario.yaml` is one list of `(op, input, expect)` steps: create a part → get the link → edit with a stale base (auto-rebase) → a conflicting edit (expect `anchor_not_found` with a hint) → find it →
share inward with a group → get a deep link for a public link (`part_link_for`) → comment `to_agent` → write `db` → `part_check` → restore v1. In Slice 1 the scenario covers the Slice 1 ops (the find, comment and inward-share steps join when `part_find`, comments and agent sharing land), and every step's input is an example from `docs/agent-contract.md`. Four drivers run it against an ephemeral server:
1. MCP via the `rmcp` client.
2. HTTP via a client **generated from `/openapi.json`**, which also proves the spec is truthful.
3. The CLI via subprocess with `--json`.
4. **The skill file itself**: the example commands in `/skill.md` and `/llms-full.txt` are extracted and executed.

Normalised results (ids, timestamps) and audit rows must be identical across drivers and across the auth kinds (OAuth authorization code and device code). The suite also validates `/openapi.json`, runs the GPT Actions lint and fetches every discovery URL.
An optional nightly, not gating: a small model gets only `PARTIFACTORY_URL` + a device-code connection + "make a packing list page and give me the link", and must succeed via discovery.

## 4c. Previews and render checks

Two tiers. Tier (a) always runs. Tier (b) is an optional sidecar whose headline job is **`part_check`: agents screenshot and validate their own work**.

### Tier (a): branded cards, in the binary, always

*In Slice 1 both variants that exist are content-free with fixed text, so they ship as two static 1200 × 630 PNGs embedded in the binary, at content-addressed `/og/<sha256>.png`. The SVG pipeline below arrives with the titled card, after Slice 1, where fields first vary.*

- Rendered from an SVG template → `resvg`/`usvg` → PNG 1200 × 630 in a few ms, no browser. Fonts are bundled (`fontdb`; a `FROM scratch` image has no system fonts). Titles are wrapped to ≤ 2 lines and ellipsised in Rust (`rustybuzz` measures glyph advances), because resvg doesn't wrap.
  Output ≤ 150 KB, cached as a blob keyed by `sha256(template_version, fields)`, served at **content-addressed `/og/<sha256>.png`**, so a new title or version means a new URL and unfurl caches never go stale.
- **The card's inputs are fixed fields, never part content**: `{template_version, audience, title?, description?, kind_label?, rev?, thumbnail?}`. The designer owns the template; the mechanics own the inputs. That makes the content-free guarantee checkable: a test asserts the private variant's input has no part-derived field.
- All variants share the frame: `paper`, the 8 px drafting grid at 4 % ink, corner ticks 40 px in, the mark + wordmark top-left, and a catalogue-style title block bottom-right.

| Variant | Served on | Shows | Never shows |
| --- | --- | --- | --- |
| **Private** | the part host root (the wrapper's og tags) for every part without an active public link, sign-in pages, `/grant` | the mark, "A private part", "Sign in to open it", the workspace host name, a lock glyph | title, name, part number, thumbnail, description, author, audience, counts |
| **Public link** (after Slice 1; in Slice 1 a public link gets the private card's content-free layout with the audience line "Anyone with the link") | `https://<name>-part.<domain>/l/<token>`, and the part host root while the part has an active public link | title, kind bin code, `REV n`, a 2–3 line description, the thumbnail in a framed window, "Anyone with the link · view only" | comments, data, who else has access, agent notes |
| **Invite** | `/i/<id>` | "You're invited to a part on partifactory" (+ the inviter's display name only if they opt in) | the part title, until the invitee has enrolled |

Every card page carries `noindex`, `X-Robots-Tag: noindex, nofollow`, `og:title`/`og:description` that match the card exactly (so text tags never leak what the image hides), `twitter:card=summary_large_image`, and an absolute https `og:image`. A public link with a video preview adds the `og:video` tags (below).
Private and invite cards are generic, so `/og/*` is cacheable by any shared cache (`public, immutable`). Cards are served from the workspace host, and part-host pages reference them by absolute URL. **The audience decides the card, never the User-Agent** (§2).

### Tier (b): the render sidecar (optional)

- **Image**: `chrome-headless-shell` pinned by digest, plus fonts (Noto subset with CJK and emoji), in its own container `partifactory-render`. Non-root, read-only root fs, tmpfs `/tmp`, `mem_limit: 768m`, `pids_limit: 256`, `cpus: 1`, default seccomp.
  Flags: `--disable-gpu --no-first-run --disable-extensions --disable-background-networking --disable-sync --mute-audio --deny-permission-prompts`.
- **Network**: attached only to a Docker `internal: true` network shared with partifactory, so it has no route to the internet or the LAN. Chrome resolves `*-part.<domain>` and `blobs-part.<domain>` to the gateway via `--host-resolver-rules`,
  and partifactory serves the same handlers on an internal-only TLS listener (`:443` on that network, self-signed `*.<domain>` cert generated at start, trusted via `--ignore-certificate-errors-spki-list`). URLs, CSP and the secure context match production byte for byte.
  Parts load nothing third-party (mirror-on-build), so nothing else is needed.
- **Renderer caps**: minted for subject `render`, a 60 s window, **read-only**. The page runs with the real prelude against a **read-only view of the part's real shared data** (when the checking connection has `data:read`; otherwise shared storage starts empty), and personal storage starts empty.
  Writes made during a check go to an in-memory overlay that later reads in the same check see, and the overlay is discarded at the end, so the page behaves normally and nothing is persisted. It can reach only the part host.
- **Driving it**: CDP over the internal network (`chromiumoxide`). Each job gets a **fresh browser context**, a viewport (`mobile` 390×844 @2x or `desktop` 1280×800), and waits for `partifactory.ready()`, network idle for 500 ms, or 8 s, whichever is first. Then it runs the job's `actions` in order (click, type, `wait_ms`, `eval_return`; ≤ 20 steps; the first failing step stops the list) and takes the screenshot after the last one. Hard timeout 15 s, then the context is destroyed.
- **Collects**: uncaught exceptions, `console.error`/`warn` (first 50, truncated), failed and CSP-blocked requests, document height, timing, and a **blank flag** (body text empty *and* the screenshot ≥ 98 % one colour).
- **Queue**: `render_jobs`, **concurrency 1**, coalesced per part (a 5 s debounce after a live change, newest only), one retry, a circuit breaker. Chrome starts on demand and stops after 5 minutes idle, so it costs no RAM most of the day. **The renderer is never on the save path.**
- **Outputs**: `part_check` results; runtime errors tagged `source: render`, so a broken page is known before any person opens it; private gallery thumbnails (600×375 WebP part blobs, P4); and **previews**: an automatic poster per part, and a short `og:video` clip when the agent asks for one, used by public links (on by default, switchable per link) and the gallery (below).
- **If it's disabled or down**: `part_check` returns `renderer_unavailable` with a hint, thumbnails fall back to the agent's own or the SVG thumb, previews to the agent's own clip or the image card, and nothing else degrades.

### Previews: posters always, `og:video` when the agent wants one (right after Slice 1, decision 53)

A public link should show what the part is before anyone taps it. So its unfurl always carries a poster (a screenshot `og:image`), and it carries a short, muted clip (`og:video`) when the part has one. The gallery shows the same previews to people who can open the part. A private part's unfurl stays content-free: the unfurler can't open it, so it learns nothing.

- **Posters are automatic.** Whenever a part gets a new live version and the sidecar is on, a low-priority `preview` job (after every `check` and `gate` job, coalesced per part, newest only) screenshots the live version at 1200 × 630 in the read-only, discarded-overlay context `part_check` uses, after `preview.actions` if there are any. The result is a JPEG poster, a part blob.
  The agent's own `preview.poster` (PNG or JPEG, 1200 × 630) wins when it's set. With the sidecar off and no agent poster, the poster is the titled card. Nothing waits on a preview, and it's never on the save path.
- **A clip is the agent's choice.** Agents don't have to make one. The guide asks them to consider it for anything animated or interactive, where a still undersells it. Either:
  1. **the agent's own**: `preview.video` names an MP4 in the part's files: H.264, `yuv420p`, no audio track, ≤ 10 s, ≤ 1280 × 720, ≤ 8 MiB, `moov` first. The build child probes the container and codec headers (no network, no decoding) and fails the build with `preview_invalid`, naming what's wrong: codec, audio, duration, size or poster; or
  2. **recorded for it**: `preview.video: "record"` makes the same `preview` job also record `preview.seconds` (2–10, default 6) of CDP screencast frames at 1280 × 720 while it runs `preview.actions` (the `part_check` grammar, ≤ 20 steps), or, without actions, while it scrolls slowly through the page. `ffmpeg` in the render image (pinned by digest) encodes them to the same MP4 profile.
  The default is `"off"`. A failed or blank recording keeps the last good clip and records why. With the sidecar down, `"record"` waits for it and the poster is used meanwhile.
- **On by default for public links, off per link** (the admin, 2026-10-23). Every public link uses the part's clip when there is one, and the poster always. The share sheet's public rung shows the poster with **Preview: Video · Image · Card only** and says "Apps that unfurl the link keep their own copy." An agent can't change a link's preview: that's on the people-only list, like the link itself.
- **Served from the part's own host, never the workspace**: `https://<name>-part.<domain>/og/<sha256>.mp4` and `.jpg`. They're cookie-free, with the `Content-Type` fixed from the probe, `X-Content-Type-Options: nosniff`, `Accept-Ranges` with `206` (iMessage and Safari fetch ranges) and `Cache-Control: public, max-age=3600`.
  They answer **only while the part has an active public link whose preview allows them** (`video` for the clip; `video` or `image` for the poster), and otherwise return a 404 byte-identical to an unknown path. Revoking the link or switching its preview off stops them at the origin at once. Unfurler caches keep what they already fetched, and the sheet says so.
- **Tags**, on the root and `/l/` while that holds: `og:image` set to the poster, and, with a clip, `og:video`, `og:video:secure_url`, `og:video:type` (`video/mp4`), `og:video:width` and `og:video:height`.
  `twitter:card` stays `summary_large_image`, because X's `player` card needs an embeddable player page. Bots and browsers still get byte-identical HTML (§2).
- **In the workspace.** The poster is the gallery thumbnail. A card for a part with a clip plays it muted and looped while the card is on screen: on hover on desktop, in view on phones, and never under `prefers-reduced-motion`.
  It's served from `/api/v1/thumbs/<part>?v=<n>&media=video`, session- and grant-checked and `private, immutable`, like thumbnails. A viewer who can't open the part never gets it.
- **For agents.** `docs/agent-contract.md` gains these in the change that ships them:
  - `preview` on `part_create`, and inside `meta` on `part_edit`.
  - `part_check {record: true}` makes the recording the server would make, and stores nothing. Over HTTP it returns `video: {url, expires_at, width, height, seconds, bytes}`; in MCP it returns the poster as an image block plus a link to the clip.
  - `part_get` returns `preview: {source, state, video?, poster?, error?, public}`.
  - `/skill.md` tells agents that a poster is made for them. For anything animated or interactive, they should consider a clip: set `preview.video: "record"` with `preview.actions` that show it working (for the pixel board, a few taps that paint), watch it with `part_check {record: true}`, and mention that the link will unfurl with it.
- **Capped like every store** (§5h): previews are part blobs, only the newest poster and clip per part are referenced, and the store has a cap (2 GiB by default). While the store is full, recording stops, and the last poster or the titled card is used.
### `part_check`

`part_check {name, version?: head | live | n, viewport?: mobile | desktop, wait_ms? ≤ 8000, actions? (≤ 20), include_base64?}` (defaults `head`, `mobile`, 8000, none, false) returns `{screenshot, blank, errors[], console[], failed_requests[], actions[], timing}` plus `{name, url, version, head, live, build, height}`.
`actions` lets an agent exercise the page and read back what happened: for a pixel board, `[{do: "click", selector: "#b i"}, {do: "eval_return", expr: "getComputedStyle(document.querySelector('#b i')).backgroundColor"}]` proves a tap paints, without saving anything.
If the version's build is still running, `part_check` waits for it inside its 15 s limit; if the build isn't `ok`, there's nothing to render, so it returns `screenshot: null`, `blank: null` and the build's errors in `build`.
In MCP the screenshot is an **image content block** (PNG, ≤ 1 MB, downscaled), so multimodal agents can look at it. Over HTTP it's `{url, expires_at, width, height}`: `url` is `https://parts.<domain>/api/v1/screenshots/<id>`, fetched with the same bearer token by the same connection, valid 10 minutes; `include_base64: true` adds `base64`.
It can check `head` before it goes live, so an agent can verify a change before it ever replaces a working page. `part_edit {check: true}` runs it after the build and folds the result in (about 1–3 s; the call then waits up to 15 s). `live: "after_check"` (§3d) uses the same check as the gate for `live`; a `part_check` on a version whose gate check is running waits for that check and returns it.
Rate-limited to 30 per hour per connection. It's a **P2 headline feature**: Hermes runs it before posting a link, and the shared guide tells every agent to.

## 5. Stack: Rust, all the way down

One Cargo workspace builds **one static binary** (`partifactory`, with `part` as a CLI alias) that has every asset embedded. The build worker is the same binary re-exec'd.

| Concern | Choice |
| --- | --- |
| HTTP | `axum` + `tokio` + `tower` (three listeners: workspace, content, ops; plus the internal TLS listener when the render sidecar is on) |
| Storage | SQLite via `rusqlite` (bundled, FTS5, WAL). No storage-backend abstraction until there's a second backend |
| Blobs | `BlobStore` trait: a local directory under `/data/blobs` in v1 |
| TSX/JSX → JS | `oxc` (umbrella crate, `transformer` feature; transform + minify) + `rolldown` (bundle), **in the build child process**, cached by `hash(manifest, toolchain)`. **Exact versions pinned and vendored**: oxc is 0.x with breaking releases, and rolldown's semver promise covers its JS API, not the Rust crate. A **golden compile corpus** (shartifacts' compat-matrix artifacts + Claude-written samples) runs on every upgrade. esbuild-as-subprocess is the fallback, behind the same child protocol |
| Markdown / code | `comrak` + `syntect` server-side; Mermaid and KaTeX client-side |
| Operation registry | `serde` + `schemars` types; `aide` for OpenAPI 3.1 |
| MCP | `rmcp` 3.x (official Rust SDK), Streamable HTTP, stateless, spec 2026-07-28 |
| People auth | `openidconnect` (generic OIDC client), `webauthn-rs` 0.5 (admin step-up, local passkey accounts: registration, sign-in, recovery) |
| Agent auth | OAuth 2.1 AS/RS: auth code + PKCE, device grant, CIMD/DCR/manual clients, RFC 9728/8414/8707/9207. Hand-written over well-tested primitives (rmcp's OAuth support is client-side only), with a conformance suite |
| Capabilities | compact HMAC-signed tokens (`ring`), versioned keys (`kid`), CBOR payloads |
| Cards | `resvg` + `usvg` + `fontdb` (bundled fonts) + `rustybuzz` for wrapping (with the titled card; Slice 1 embeds two static PNGs) |
| Render checks (optional sidecar) | `chrome-headless-shell` by digest, driven over CDP with `chromiumoxide`; `ffmpeg` (by digest, in the same image) encodes preview clips (§4c) |
| Export | `tar` + `zstd`, `age` (encrypt to a recipient public key) |
| Workspace UI | Leptos 0.8 (SSR + hydration), or `askama`/`maud` templates + small vanilla-JS islands if the spike says so (below). CodeMirror 6 for the source editor (the one JS dependency, vendored as a shared blob) |
| Backends | the runner sidecar: `bollard` (Docker Engine API client; Docker or Podman), rootless BuildKit (`buildkitd`) for image builds, nftables/iptables rules it applies itself, optional gVisor (`runsc`) |
| Observability | `tracing` (JSON logs, spans into the trace store), `metrics` + Prometheus exporter, a separate `insights.db` (§5g), optional OTLP adapter |
| Tests | `cargo nextest`, golden tests (manifests, diffs, builds), property tests (cap for A never reads B), OAuth conformance, cross-surface conformance, browser e2e with virtual WebAuthn against `pf.test` (Dex + Caddy, internal-CA wildcard cert), a sandbox compat corpus (30 real Claude-written artifacts and 10 Vite builds rendered under the sandbox in CI with zero uncaught errors) |

**Why Rust, not Go** (the reference is 21k lines of Go):
one static `FROM scratch` binary with every asset embedded; `rmcp`, `oxc`, `rolldown` and `resvg` in the same binary instead of sidecars; `webauthn-rs` for native step-up; memory safety on an internet-facing authorization server; and the owner's preference.
The cost, stated: porting shartifacts' Go compatibility layer would reach P3 sooner. **Bend 2** was considered and set aside: this service is I/O-bound, and Bend's strength is parallel pure computation.

**Why Leptos, and when it loses.** One language end to end, typed request/response structs shared with the server, and SSR pages that work before any JS loads.
The risks are hydration bundle size on phones, API churn (0.9 is in release candidates; pin 0.8.x) and CodeMirror interop.
**Decision rule for the P0 spike**: if building the share sheet costs more than a day of framework friction, or the hydrated bundle for the part view exceeds 300 KB brotli, use `askama`/`maud` server templates plus small vanilla-JS islands (share sheet, editor, SSE). It's still Rust.

**Why a child process for builds**: see §3d. In short, a hang or panic in young compiler code costs one build, not the server.

**Maturity check before P0 freezes**: one-day spikes on rolldown/oxc as libraries, rmcp stateless Streamable HTTP, Leptos SSR vs templates, and the **opaque-origin sandbox on every browser engine**. The sandbox spike checks, on Chromium, WebKit and Firefox (Playwright) and on Mobile Safari in the iOS Simulator (`xcrun simctl`, a self-reporting test page that posts its results back), with no human in the loop:
1. `'self'` in `script-src`/`connect-src` still matches the part host from a sandboxed document (fallback: list the host explicitly);
2. `Origin` (`null`) and `Sec-Fetch-Site` (`cross-site`) values on a fetch to the part's own host;
3. `fetch("./x.json")`, `<script type="module">` and `import()` with `ACAO: null` and no credentials;
4. `Object.defineProperty(window, "localStorage", …)` before any page script runs, visible to later inline scripts;
5. `new Worker(blobURL)` and the wrapper around a relative URL (WebKit has open bugs here);
6. popups with `allow-popups-to-escape-sandbox`, clipboard, `navigator.share`, fullscreen with user activation;
7. the Cookie Store API (`cookieStore.set`) and `document.cookie` writes both fail;
8. a part's fetch/POST to a sibling `<domain>` service, and to the workspace, carries no `SameSite=Lax` cookies;
9. CSP `sandbox` is honoured from the header (not `Report-Only`, not `<meta>`);
10. the wrapper flow: a `#pf=` fragment in a 302 `Location` survives to the part host; `history.replaceState` clears it before any other script; an explicit part-host `frame-ancestors` admits the same-host wrapper; and a credentialed same-site `fetch` from the part host to the workspace carries the `Lax` session cookie (Safari's tracking prevention included);
11. *Cloudflare adapter only:* the wildcard CNAME to `<tunnel>.cfargotunnel.com` serves a new `*-part` name;
12. a WebSocket from a sandboxed document to its own host (`Origin: null` in the handshake) and SSE under `/c/<cap>/api/`, both through each example proxy;
13. a WebAuthn assertion from the wrapper origin with `rp.id` = the apex, with the virtual authenticator on Chromium, the only Playwright engine that has one (WebKit and Firefox record `not_assertable`), and in the iOS Simulator where WebKit allows it. The Telegram webview is approximated by the WebKit lanes and confirmed on a real phone in the human checks.
The sandbox spike runs in the first week, before the content listener starts, since items 10, 12 and 13 decide the wrapper flow.
A separate runner spike (P2a, part of the runner work) checks rootless BuildKit in a container and the firewall rules on Docker (`DOCKER-USER`) and on Podman (netavark + nftables).
An advisories spike (before matching ships, §3h) measures the OSV dump sizes for the ecosystems in use and the matching speed against 10k `build_deps` rows, and chooses between Syft and Trivy as the runner's pinned scanner image.

## 5a. Performance: numbers, measured honestly

partifactory handles a **reduced subset** of HTTP: a fixed route table, fixed responses (immutable, precompressed, ETag = hash) and no upstream hop.
Doing less work per request is how it can beat general-purpose proxies on *this* workload. On raw static files nginx's `sendfile` is near the syscall floor, so the target there is parity.
**Most repeat requests never reach the origin**: shared libraries are `public, immutable`, so the browser keeps them, and an edge cache (when the ingress adapter has one) serves them to new devices. Neither is needed for correctness.

- **Why the nginx/HAProxy comparison exists**: it's a regression guard that keeps the hot path DB-free and allocation-light. Parity with nginx is the sanity bar, not a product requirement. It runs **after P1, on the real content path** (cap check, `/c/` prefix, brotli), because that's the number worth knowing. P0 has only a same-machine regression bench.
- **Work at build time**: compile, render, minify and precompress (**brotli only**: every browser supports it, and zstd isn't worth doubling the blob store), so serving is negotiation + a file read.
- **Lean hot path**: cap check = one HMAC verify plus four integer compares (part, minter, boot epochs; window), then a lookup of `path → hash` in an LRU of hot build manifests filled from the events stream. 304s are answered from the ETag alone. Hot small blobs sit in an mmap'd LRU.
- **SQLite for reads**: WAL, mmap, a read pool + a single writer task, cached statements, in-memory SSE broadcast.
- **One runtime: tokio.** A thread-per-core io_uring runtime is parked, to be revisited only if a profile shows the runtime is the bottleneck.
- **Honest about zero-copy**: hyper's default body path copies through userspace. v1 uses mmap'd LRU + `pread`, which is plenty.
- **Serving a part is two steps**: an in-memory `part → live (version, build)` cache resolves the entry without a DB read, then the build manifest LRU resolves the path.
- **Repeat opens**: deterministic 12 h caps keep part blobs in the browser cache. They re-download when the window rolls, after a revocation, and after a deploy (the boot epoch). Shared libraries never do.

**How it's gated, so the numbers mean something**: absolute targets are *tracked*. Release gates compare against the previous release **on the same machine**
(the deploy host's bench job runs off-hours), failing on a >10 % regression. Shared CI runners are too noisy to gate on microseconds.

| Metric | Target (tracked) |
| --- | --- |
| Part blob from origin, p99 server time (warm) | < 200 µs (a hypothesis until measured) |
| `/grant` decision + 302, p99 server time | < 1 ms |
| `part_find` over 10k parts, p99 | < 5 ms |
| `part_edit` small edit, no compile, p99 | < 10 ms |
| `part_edit` TSX part incl. compile + bundle + brotli, p50 / p99 | < 150 ms / < 1 s |
| Card render (tier a, from the titled card), p99 | < 20 ms |
| Repeat open of a part from a phone, end to end (the wrapper and entry HTML always reach the origin; shared libs from cache) | < 150 ms on a good uplink, measured from real devices and reported, not gated |
| Static-file throughput vs nginx and HAProxy on the same box and files | ≥ parity |

## 5b. Builds and deploys

- **Frontends need no build infrastructure.** TSX/CSS/Markdown compile in the build child at save time, CDN references are mirrored at build time, and a part that needs real npm packages
  is built by the agent locally (`vite build`) and uploaded as `files`. **Backends** build in the runner with rootless BuildKit and run as containers it starts (§3f, §3g).
- **The deliverable is three things, none tied to a CI vendor**: an **OCI image**, a **compose file**, and a **documented env contract**.
  - **Image**: `cargo-chef` layer caching → static musl binary → `FROM scratch` image (~20 MB), amd64 + arm64, with an SBOM, signed with `cosign`, pushed to any OCI registry and **deployed by digest**, never `:latest`.
    Every tag also publishes the image and the `part` CLI as release assets (the passive GitHub mirror's Releases in the example deployment), so a laptop drill doesn't depend on the CI host. A registry holds only images, which CI can rebuild from git, so backing it up is optional.
  - **Compose file** (`deploy/compose.yaml`): partifactory, the `/data` volume, the runner behind the `backends` profile (on in the shipped file; it's the only service that mounts the Engine API socket), the optional render sidecar behind a profile, and an optional proxy service (Caddy by default, behind a profile) for hosts that have none.
    Every service sets `json-file` logging with `max-size=10m` and `max-file=3`, so the daemon's own logs can't fill the disk. Example proxy configs for Caddy, nginx and Traefik live in `deploy/proxy/`.
  - **Env contract** (`docs/env.md`, generated from the config loader so it can't drift): `site.toml` (path `PF_SITE_TOML`, default `/data/site.toml`) is the config, and every key is also settable as `PF_<SECTION>_<KEY>`, env winning. Secrets are env or `_FILE` only, never in the file: every secret also accepts `PF_*_FILE` (a path, which is how Docker/compose secrets and most secret stores mount them). Required: `PF_SESSION_KEY`, `PF_CAP_HMAC_KEY`, `PF_DOMAIN_APEX`, `PF_METRICS_TOKEN`, the OIDC client secret if `[idp]` is set, and `PF_RUNNER_KEY` if the runner is on.
    Optional per adapter: `PF_DNS_API_TOKEN`, `PF_TUNNEL_TOKEN`, `PF_IDP_PROVISIONER_TOKEN`, `PF_ALERTS_WEBHOOK_SECRET`, `PF_CDN_ANALYTICS_TOKEN`. `partifactory config check` validates a set and names any missing variable.
- **Deploying is `docker compose pull && docker compose up -d`.** That works by hand, from cron, or from any CI. `partifactory deploy` wraps it with a health gate: start the new container next to the old one, wait for `/readyz`, switch, and **roll back automatically** if health fails within 2 minutes.
  Schema migrations run before the switch and must be forward-compatible with the previous binary.
- **CI is an example, not a dependency.** The repo ships `examples/ci/gitea-actions.yml` (what the example deployment runs: build, sign, push, then `partifactory deploy` over SSH from a protected runner) and `examples/ci/github-actions.yml`. Secrets come from whatever the CI or host provides, as env vars or files. Nothing reads a CI vendor's API.
- **The host contract.** partifactory never calls ZFS, TrueNAS or any host API. A host must offer:
  1. an OCI runtime with compose (Docker or Podman), and, for backends, its Engine API socket mounted into the runner only;
  2. one data volume mounted at `/data` (SQLite, blobs, backups, exports). There's no other state anywhere;
  3. env vars or secret files per the env contract;
  4. inbound HTTP from the proxy on the same host or network, or outbound network access for a tunnel adapter.
  TrueNAS is just one such host (the example deployment's). Its snapshots are an optional, host-provided backup layer (§5c).
- **The render sidecar** is a second service in the same compose file, behind a profile, so it's off unless site config turns it on.
- **Day to day**: `cargo run`/`bacon` against `*.localhost`. `part` CLI binaries ship as release assets + `cargo binstall`.

## 5c. Backups, recovery and self-provisioning

**Backup = the `/data` folder**, which is shartifacts' model too ("back up the data folder"). Versioning and rollback stay inside partifactory: immutable versions in SQLite, content-addressed files under `/data`, history, diff, restore-as-new-version and the 30-day bin (§3e).
No external system is involved in any save, and **no save ever depends on a backup target**.
- *Why:* part content is what matters, and one folder with no other state is something every host already knows how to back up. A git archive was considered and rejected (the admin: no git or Gitea dependency for parts), and so were Litestream and a built-in streaming sink (`docs/plan-history.md`).

```
/data/
  partifactory.db (+ -wal, -shm)   all metadata, grants, tokens, comments, per-part data
  blobs/ab/cd/<sha256>             content-addressed, write-once
  insights.db (+ -wal, -shm)       analytics, traces, janitor runs (§5g); its own writer
  logs/                            rotating zstd JSON segments: app/ and parts/<id>/ (§5h)
  parts/<id>/vol/<svc>/            backend volumes (§3f)
  backup/partifactory-latest.sqlite   hourly consistent VACUUM INTO copy (for non-atomic host tools)
  exports/partifactory-<ts>.tar.zst   nightly export, keep 7 (on by default)
  advisories/                      the OSV advisory DB, latest sync only (§3h); downloaded again, never exported
```

| Layer | What | Whose job | Notes |
| --- | --- | --- | --- |
| **1. Host backups of `/data`** | ZFS snapshot tasks on TrueNAS; appdata backup, rsync, restic, whatever the host has | the host's | Instant local rollback. **Docker itself doesn't back up volumes**, and snapshots are same-machine: they don't move with the app to another host. An atomic snapshot (ZFS) of a live WAL database is crash-consistent. A non-atomic copy (rsync) must take `backup/partifactory-latest.sqlite`, never the live `.db` file. Blobs are write-once, so any copy of `blobs/` is safe |
| **2. `partifactory export`** | one self-contained, host-portable file | partifactory's | The off-site and move-to-another-host path. A scheduled export to a folder lets any off-site tool pick it up |
| **3. Optional upload** | the scheduled export also pushed to an S3-compatible bucket (R2 is one example) | partifactory's, behind config | Not needed for v1 (P5) |

`partifactory doctor` and the Health page report which layers are active: layer 1 as **on / off / unknown** (from site config `backup.host_snapshots`, since partifactory can't see the host's tooling), the age of the newest export, and whether exports land on the same filesystem as `/data`.
**An export on the same disk as `/data` shows amber, never green**: it's a convenience, not a backup. No export in 48 h is amber too.

**`partifactory export`** writes `partifactory-<ts>.tar.zst`:
- `MANIFEST.json` (format version, created, binary version, counts, and a sha256 per entry);
- `db.sqlite`, a consistent `VACUUM INTO` copy checked with `PRAGMA integrity_check` (it reads a snapshot under WAL without blocking the writer);
- `blobs/<sha256>` for every blob referenced by any version, build, draft, data snapshot or asset;
- `parts/<id>/vol/<svc>/` for every backend volume, taken consistently (the service's export hook, or stop, copy, start);
- `insights.db` only with `--include-insights`, and part secrets only with `--include-secrets`, which requires `--encrypt`.
- **Parts only** mode (`--parts-only`) writes part manifests, files, notes, titles, spaces by slug and tags as JSON instead of the DB. It has **no access records, tokens or people**. It's what you'd hand to someone else, or keep somewhere less trusted.
- **Optional age encryption** (`--encrypt`, or always on in site config) to an age **recipient public key**, so the box never holds the decryption key. The identity lives offline in the password manager.
- **Scheduled, on by default**: nightly to `/data/exports`, keep 7 (`backup.export_schedule`, `backup.export_dir`, `backup.export_keep`). Turning it off is an explicit key and shows as a warning on Health. The newest export's time and size are metrics.

**`partifactory import <file>`** restores on any OCI host:
1. verify every entry's hash against `MANIFEST.json`;
2. load `db.sqlite` into `/data` and run forward migrations;
3. write blobs (temp → fsync → rename);
4. **bump the boot epoch**, so caps minted before the export can never come back;
5. start serving. `/readyz` reports `importing=true` until blobs are verified.
- Sessions and OAuth tokens in the export are valid again, which is the point of a full export. Refresh tokens issued after the export point are unknown, so those agents sign in again, and refresh-reuse detection is suspended for 24 h after an import to avoid mass family revocations.
- **Parts-only import** recreates every part with its full version history, author labels and timestamps, in a fresh DB. **Access must then be re-granted and agents reconnected**: that's the honest cost of not carrying the access records.
  A helper proposes grants by matching stored audience names (groups by name, people by username or email), and the admin applies them on one screen.

**Per-part data** (`storage`/`db` documents) lives in the same DB, so it's in every copy of `/data` and every full export automatically.
**Data snapshots** are a separate, per-part, point-in-time history of a part's data, for **undoing a bad write, not for disaster recovery**:
- **on by default**: `daily`, keep 7, starting the first time a part stores data. Nobody is asked. Switch it off per part in Details (or an agent sets `snapshots: "off"` at create); `hourly` (keep 48) is the other option;
- stored as blobs (canonical JSON of the part's documents), and skipped on days when the data didn't change, so an idle part costs nothing new;
- it stops after N days (default 90) without the part being opened, and **after that keeps the single most recent snapshot** until the part is deleted and leaves the 30-day bin;
- a snapshot is the part's **shared** `data_docs`: personal docs are each person's own and never rolled back by someone else;
- **roll back** snapshots the current data, replaces the shared docs with the snapshot's (new `rev`s), and emits a `data.changed` for every affected key, so open pages repaint. It's one action from Details or the share sheet, and agents do it with `part_data {action: "snapshot_restore"}`; `reset` empties the shared data the same way, snapshotted first. Download the data as JSON any time;
- switching a public link to Can contribute takes one first (§3a). These on-demand snapshots (and the ones before a roll back or reset) don't count against `keep`;
- **they don't cover backend volumes**, which folder backups and exports do (§3f).
- *Adopted in round 2b:* the Fable review argued for per-part data snapshots on by default, and the admin now agrees. A bad write by an agent or a viewer is exactly what an undo is for, and nobody remembers to opt in before they need it.
  The 90-day idle stop and keep-the-last-one rule keep the cost bounded.

**Identity and DR.** partifactory's grants point at people: IdP users by `(issuer, sub)` and local accounts by id. The IdP's own database is outside partifactory's scope: it's the IdP operator's backup concern, documented in site config.
Local accounts and their passkeys are in `/data`, so they come back with any copy or full export. If the IdP is restored, the same users, passkeys and `sub`s come back. If it isn't, the import helper rebinds grants by email or username as people re-enrol.

**Drills run from a laptop**, not CI, because the CI system (and its secrets) may be on the host that died. The monthly drill takes the newest export, the image and CLI from the release assets, and site config from the offline copy,
imports into a throwaway container on a different OCI host, and runs the e2e suite. It writes `{ok, rto_s, failures}` to `backup/drills/<ts>.json`, which the server exports as `pf_last_drill_ok` and `pf_last_drill_age_s`. RTO target < 15 min.

**Moving to another host.**
1. Point the deploy target (`deploy.host` in site config, or wherever you run `docker compose`) at the new host.
2. Either copy `/data` (stop the old container first, or copy from a snapshot), or run `partifactory export` on the old host and `partifactory import` on the new one.
3. Deploy, and re-point ingress at the new host: change the proxy's upstream, or, with the Cloudflare adapter, let `provision` re-point the tunnel, then **rotate the tunnel connector token** and delete the old host's connections, so a zombie connector on the old host can't take traffic.
4. Check `/readyz` on the new host directly (or through the tunnel's `cfargotunnel.com` name), then `partifactory doctor` (ingress, backup layers, export folder, disk).
5. Re-enable layer 1 on the new host. Old snapshots don't come along.
The hostnames never change.

**From nothing to running**: an OCI host, a domain with a wildcard record and a wildcard cert, and (optionally) an OIDC client. With provider plugins, one DNS token and one IdP token are enough for `provision` to create the rest.

- **`partifactory provision`** is an idempotent **create-if-missing / update-if-owned** script driven by `site.toml`, with **provider plugins** behind two traits: `DnsProvider` (records, and for the Cloudflare plugin also tunnel, cache rules, rate rule, fallback Worker, health check) and `IdpProvisioner` (OIDC client registration, flows, provisioner account).
  It's honest about being a script rather than Terraform: it only manages resources it owns, so reapplying is safe, and it never "reconciles" shared IdP flows.
  - **Providers**: DNS `manual` (the default: prints the two records and the cert names to create, then checks them with a DNS lookup) and `cloudflare` (the first plugin). IdP `manual` (the default: prints the redirect URI and scopes to register in any OIDC provider) and `authentik`. More plugins are additive.
  - **`site.toml` sections**: `[domain]` (apex, workspace_host, part_suffix), `[ingress]` (kind = proxy | cloudflare-tunnel | acme, client_ip_header, trusted_proxies, request_id_header), `[dns] provider` (+ `[dns.cloudflare]` account_id, zone_id, plan, tunnel_name),
    `[idp]` (issuer, client_id, groups_claim, audience_group; optional), `[idp.provisioner]` (kind = none | authentik, base_url, flows.*, tier), `[admin] subject`, `[embed] frame_ancestors`, `[alerts] webhook_url`,
    `[backup]` (host_snapshots, export_schedule, export_dir, export_keep, age_recipient), `[deploy]` (host), `[names] reserved`, `[webhooks]`, `[mirror] allowlist`, `[render] enabled`.
  - **Ownership**: a resource counts as owned if its id is in `provision-state.json` (committed to the site repo by the run) *or* its name starts with `partifactory-`. DNS records also carry the comment `managed-by:partifactory` where the provider supports comments. It deletes only resources that are both owned and listed as `created_by_provision`.
  - **Order** (each provider skips what it doesn't do): ingress (Cloudflare: tunnel) → IdP (Authentik: OIDC app, cloned flows, invite flow, provisioner account and role) → deploy → wait for `/readyz` → `parts` record → wildcard record →
    Cloudflare only: cache rules (`blobs-part/b/*` and `parts/og/*`, "eligible for cache", because Cloudflare caches by file extension and these paths have none) → one rate-limit rule → fallback Worker route → health check (Pro and up) or an external uptime probe (Free).
    Each step is idempotent, so a failed run is fixed by re-running.
  - `--dry-run` prints one line per resource: `+ create`, `~ update (field: old → new)`, `= unchanged`, `! not owned, skipped`. A real run appends `{ts, resource, before, after}` to `provision-log/<ts>.jsonl` in the site repo. Exit codes: 0 ok, 1 error, 3 drift on a resource it doesn't own.
  - The Authentik plugin uses blueprints if they can be applied over the API, otherwise direct API calls (Authentik spike).
- **The provisioner gate** (Authentik plugin), as `partifactory provision --gate`: using the runtime token, try each forbidden operation (§3a) and expect `403`, then expect `2xx` for creating an invitation, sending a recovery link, and changing membership of a `pf-test` group. The result sets `[idp.provisioner] tier`.
- **Secrets** come from env vars or `_FILE` paths per the env contract (§5b), supplied by whatever runs the container: a CI secret store, compose secrets, or a mode-600 `.env` on the host. The export age recipient is a public key, so it isn't secret. The GitHub mirror is passive: no Actions, no secrets.
- **Rotation**: rotate the value where it's stored and redeploy. Retired cap keys are kept 25 h, then dropped. Public links are DB tokens, so rotation never affects them.
- **Site config itself** (`site.toml`, the admin pin, the export age identity) has an offline copy in the password manager. Without it, nothing else is recoverable.

## 5d. Operations: logs, metrics, analytics, abuse, maintenance, scale

**Logs**: structured JSON to stdout (`tracing`), always on, with one line per request carrying the request id (from `[ingress] request_id_header`, e.g. `CF-Ray` or `X-Request-Id`, or generated), route, status, latency, principal/actor (never tokens, caps, cookies or part content).
Secrets are redacted by type at the logging layer, not by convention, and `/c/<…>/` path segments and `pf_*_` tokens are redacted by pattern. Log levels change at runtime from the admin area.
The request id is on every error page and API error, and is propagated to builds, `/_rt` and backends (`X-PF-Request-Id`). Rotation, retention and export are in §5h; the log viewer and traces are in §5g.

**Metrics** (`:9090/metrics`, behind the admin metrics token `PF_METRICS_TOKEN`, never routed through the ingress): RED metrics (rate, errors, duration) per route class and per op, build and backend gauges, each store's size against its cap, **hits on non-`-part` hostnames** caught by the wildcard (a future service whose DNS record is missing shows up here first),
grant decisions, OAuth events, build times and outcomes, the advisory DB's age and open advisories by layer and severity, render queue depth, blob store size, SQLite WAL size, disk headroom, newest export age, last drill result, and edge cache hit ratio (only with a CDN adapter's analytics token).

**Health and alerting**: `/healthz` (process up), `/readyz` (DB writable, blob store OK, disk headroom > 5 %). **IdP reachability, the directory poll and backup freshness are degraded indicators, not readiness**:
when the IdP is down, existing sessions, caps and agent refresh tokens keep working and only new sign-ins fail. *Why:* tying readiness to the IdP would roll back deploys and show the maintenance page exactly when the product is fine.
An admin **Health** page shows the same, plus backup layers, version, disk use naming the biggest spaces and parts, error-rate spikes, quota hits, the last drill and directory staleness.
**Ops alerts are optional and off by default** (§4): one generic webhook (`[alerts] webhook_url`) for the disk budget at 80 %, any store at 80 % of its cap, no export in 48 h, the build worker or a backend crash-looping, the IdP unreachable for 10 minutes, and a critical advisory on a backend or a contribute-link part (§3h). ntfy, Hermes or anything else can consume it. The Health page shows the same things.
Uptime is watched from outside by any probe against `/healthz` (the Cloudflare adapter can use its health checks on Pro and up).
When the origin is down, the proxy serves the static maintenance page from `deploy/proxy/` instead of its raw error (the Cloudflare adapter serves it from a Worker instead of a 1033). The ops listener is reachable on the local network, so `doctor` and a laptop can tell ingress-down from box-down.

**Analytics** live in **Insights** (§5g): first-party, admin-only, in a separate `insights.db`. Views per part per day and unique viewers (signed-in = principal; public-link viewers and guests = a hash of the truncated IP + UA with a salt that lives in memory and rotates daily),
per-public-link open count and last opened, and last viewed. Per-part counts also show on each part's Details and are readable by agents via `part_get`. No third-party scripts, no cross-part tracking.
Raw events are kept 30 days and daily rollups indefinitely. Collection starts in P1 (in the content listener), so nothing is lost before the Insights pages land in P2.

**Abuse prevention**:
- **Rate limits** per IP and per principal (`governor`) on `/grant`, `/oauth/*`, `/device`, `/l/*`, `/mcp`, `/api`, `/_rt` writes (and per guest, §3a), backend requests per part, invite acceptance and comments. This is the real control, and it keys on the client address from the trusted proxy header (§3).
  An edge rule is an optional backstop. With the Cloudflare adapter, Free allows **one** rate-limit rule (IP only, 10 s window), so the adapter installs one combined expression over those paths.
- **Body and size caps**: 2 MiB per source file, 25 MiB per media blob or asset, 8 MiB per request, 256 KiB per db document (and per `window.storage` value), 512 bytes per `window.storage` key; request bodies are capped before auth on unauthenticated routes.
- **Quotas** per space and per agent connection (parts, versions/day, bytes). An agent that loops hits a quota, gets `quota_exceeded`, and the hit shows on the Health page, rather than being discovered when the disk fills.
  Defaults: a personal space 2,000 parts, 20 GiB and 1,000 versions a day; a connection 500 versions a day and 2 GiB introduced. NULL in `spaces.quota_*` and `agent_connections.quota_versions_day` means the default; unlimited needs `[unsafe] quotas = "off"`.
- **Invites**: counted uses, expiring, a cap on outstanding invites, and enrolment is rate-limited per invite and per IP.
- **Public links**: view-only by default, per-link request caps, a kill switch ("revoke all public links"), and `noindex` + `X-Robots-Tag` everywhere.
- **Outbound fetches** (mirror-on-build, CIMD): one egress guard (§2), so neither can be used to reach internal addresses.
- **`part_check`**: 30 per hour per connection, concurrency 1.

**Maintenance**:
- **Everything that grows** has a cap, a retention rule, rotation, an export path and a metric, run by one **janitor** task (§5h).
- **Blob GC** (a janitor job): mark-and-sweep from every version of live and binned parts, every build, draft, data snapshot, asset and kept image tarball. Unreferenced blobs are deleted after a 7-day grace period. Every version is kept (§3e), so in practice GC collects superseded build output, abandoned uploads and purged parts.
- **Disk pressure**: blobs are written temp → fsync → hash-verify → rename, so a partial blob never appears under its hash. **At 95 % of the disk budget (§5h) the writer refuses new versions, assets, builds, deploys and `/_rt` writes with `storage_full`**, while reads, deletes, grants and revocations continue.
- **Events** are kept 30 days; a `part_watch` cursor older than that gets `cursor_expired`.
- **Scratch parts** (`expires_in`) move to the bin when due.
- **Migrations**: forward-only, embedded, run at start inside a transaction, tested by the drill against a production-shaped export.
- **Dependencies**: Renovate (it runs against any forge), plus `cargo audit` + `cargo deny` (licences, advisories) in CI and a nightly advisory run on main, base images pinned by digest. A part's own libraries and images are §3h.
- **`partifactory doctor`**: checks config and the env contract, DNS records for `parts` and the wildcard, ingress (a request through the public name reaches the right listener with the trusted client-IP header; the tunnel, with that adapter), cert coverage and expiry, IdP discovery and reachability, provisioner permissions (Authentik: expects 403 on a forbidden group op, once the gate has run), backup layers, export freshness, disk, **every store capped**, any safe default switched off, the advisory source and its DB's age (§3h), and (with backends) the runner's firewall rules.

**Scale, honestly**: one node by design. Expected load is a few people and agents, about 10k parts and ~100 GB of blobs over years, which is far inside
SQLite (a single writer handles thousands of small writes/s) and one disk. The shapes that would force a change are many concurrent writers or more than one host, and neither is a goal.

## 5e. Data model

**Principles.**
- SQLite holds metadata. Bytes live in the blob store: manifests, files, build outputs, data snapshots, assets, screenshots.
- Secrets are stored only as sha256.
- **Capabilities aren't stored.** They're stateless HMAC tokens in the path. The DB holds only what revokes them (`epochs`).
- Cookies exist only on the workspace host (`sessions`). Part hosts never set or read a cookie.
- Shared vs part blob class is a property of the reference (`shared_blobs`, `build_files`), not of the blob.
- `versions.id` is the hash of a manifest that includes `part_id`, `parent` and `n`, so two parts never share a version id, even with identical files.
- Agents are `agent_connections`, one per (agent, person) or per service, and every kind of token hangs off one: OAuth auth code or device grant (`pat` rows are retired personal tokens and never resolve). One revoke covers everything an agent holds.
- There are no preview tokens. The only bearer links are `public_links`, which only the admin creates.
- There are no request or approval queues. The only pending state a person sees is `access_asks` (a signed-in person asking an owner for access).
- Analytics and observability live in a second file, `/data/insights.db` (DDL after the main block), so their write load never contends with the main writer.
- Part secrets are write-only: stored encrypted, and no query path returns a value to any op.

**Capability claims** (not a table): `{p, h, s, r, ro?, dw?, g?, e, m, me, b, w, v?, d?}`, as in §2. Verification reads only the in-memory mirror of `epochs`.

Both DDL blocks below load cleanly in SQLite 3.54 (`sqlite3 :memory:`) and are the source for the P0 schema migration, whose test asserts every table, index and `CHECK` here. The backend tables are created in P0 and used from P2a.

```sql
-- partifactory schema v1 (SQLite >= 3.45, WAL, foreign_keys=ON, STRICT tables). Everything lives under /data.
-- Conventions: ids are prefixed ULIDs as TEXT ('u_', 'p_', 'sp_', 'c_', ...); times are unix ms INTEGER;
-- secrets (session ids, tokens, codes) are stored only as sha256 BLOB(32); JSON columns are TEXT checked with json_valid.

------------------------------------------------------------------ identity
CREATE TABLE users (
  id            TEXT PRIMARY KEY,                    -- u_...
  auth_source   TEXT NOT NULL CHECK (auth_source IN ('oidc','local')),
  idp_issuer    TEXT,                                -- OIDC issuer URL; NULL for local accounts
  idp_sub       TEXT,                                -- immutable IdP subject; NULL for local accounts
  username      TEXT NOT NULL UNIQUE,
  display_name  TEXT NOT NULL,
  email         TEXT,
  status        TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled','removed')),
  in_audience   INTEGER NOT NULL DEFAULT 1,          -- member of site audience_group (everyone-signed-in)
  created_at    INTEGER NOT NULL,
  last_seen_at  INTEGER,
  synced_at     INTEGER NOT NULL,
  CHECK ((auth_source = 'oidc') = (idp_issuer IS NOT NULL AND idp_sub IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX users_idp ON users(idp_issuer, idp_sub) WHERE idp_sub IS NOT NULL;
CREATE INDEX users_email ON users(email);

CREATE TABLE groups (
  name       TEXT PRIMARY KEY,                       -- IdP group name, or a local pf-* group
  source     TEXT NOT NULL CHECK (source IN ('claim','idp_api','local')),  -- groups claim, provisioner API, or partifactory's own
  idp_id     TEXT UNIQUE,                            -- set only for source = 'idp_api'
  managed    INTEGER NOT NULL DEFAULT 0 CHECK (managed IN (0,1)),  -- 1 = pf-* group partifactory may edit
  synced_at  INTEGER NOT NULL,
  CHECK (managed = 0 OR name LIKE 'pf-%'),
  CHECK (source <> 'local' OR managed = 1)
) STRICT;

CREATE TABLE group_members (
  group_name TEXT NOT NULL REFERENCES groups(name) ON DELETE CASCADE,
  user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  PRIMARY KEY (group_name, user_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX group_members_user ON group_members(user_id);

CREATE TABLE sessions (                              -- workspace host only: part hosts never set cookies
  id_hash      BLOB PRIMARY KEY,                     -- sha256(__Host-pf_session value)
  user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at   INTEGER NOT NULL,
  last_used_at INTEGER NOT NULL,
  expires_at   INTEGER NOT NULL,                     -- 30 d sliding
  stepup_at    INTEGER,                              -- last admin WebAuthn assertion (valid 10 min)
  idp_refresh_enc BLOB,                              -- OIDC refresh token, encrypted with PF_SESSION_KEY; re-read groups every 15 min
  idp_checked_at  INTEGER,                           -- last successful claims re-read (NULL for local accounts)
  ua_summary   TEXT,
  revoked_at   INTEGER
) STRICT;
CREATE INDEX sessions_user ON sessions(user_id) WHERE revoked_at IS NULL;

CREATE TABLE passkeys (                              -- webauthn-rs passkeys partifactory registered itself
  cred_id     BLOB PRIMARY KEY,
  user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  purpose     TEXT NOT NULL CHECK (purpose IN ('login','stepup')),  -- login: local accounts; stepup: step-up and device approval, for any person who registered one (rp.id = the apex)
  passkey     BLOB NOT NULL,                         -- serialized webauthn-rs Passkey
  label       TEXT NOT NULL,
  created_at  INTEGER NOT NULL,
  last_used_at INTEGER,
  revoked_at  INTEGER                                -- set by a recovery link or by the admin
) STRICT;
CREATE INDEX passkeys_user ON passkeys(user_id) WHERE revoked_at IS NULL;

CREATE TABLE break_glass (
  code_hash  BLOB PRIMARY KEY,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,                       -- +5 min
  used_at    INTEGER
) STRICT;

------------------------------------------------------------------ spaces, parts, names
CREATE TABLE spaces (
  id                 TEXT PRIMARY KEY,               -- sp_...
  slug               TEXT NOT NULL UNIQUE,           -- '~admin', 'family', 'system'
  kind               TEXT NOT NULL CHECK (kind IN ('personal','shared','system')),
  title              TEXT NOT NULL,
  owner_user_id      TEXT REFERENCES users(id),      -- personal: the person; shared: NULL (the admin administers)
  quota_parts        INTEGER,                        -- quota_*: NULL = the §5d default
  quota_bytes        INTEGER,
  quota_versions_day INTEGER,
  created_at         INTEGER NOT NULL,
  CHECK (kind <> 'personal' OR owner_user_id IS NOT NULL)
) STRICT;
CREATE UNIQUE INDEX spaces_one_personal ON spaces(owner_user_id) WHERE kind = 'personal';

CREATE TABLE parts (
  id               TEXT PRIMARY KEY,                 -- p_...
  name             TEXT NOT NULL UNIQUE,             -- current hostname label (without -part)
  space_id         TEXT NOT NULL REFERENCES spaces(id),
  title            TEXT NOT NULL,
  description      TEXT NOT NULL DEFAULT '',
  kind             TEXT NOT NULL CHECK (kind IN ('html','markdown','text','svg','code','tsx','slides','files')),
  head_version_id  TEXT,                             -- newest committed source version (NULL = v0 placeholder)
  live_version_id  TEXT,                             -- newest version with an ok build
  live_build_id    INTEGER,                          -- the live pair is (live_version_id, live_build_id)
  head_n           INTEGER NOT NULL DEFAULT 0,
  owner_user_id    TEXT NOT NULL REFERENCES users(id), -- the person it was made for; shares inward
  steward_conn     TEXT,                             -- agent that receives to_agent comments (P3)
  shared_data_writers TEXT NOT NULL DEFAULT 'viewers' CHECK (shared_data_writers IN ('editors','viewers')),  -- who below edit may write shared data (§3a)
  link_comments    INTEGER NOT NULL DEFAULT 0,       -- opt-in: public-link viewers may comment
  isolation        TEXT NOT NULL DEFAULT 'sandbox' CHECK (isolation IN ('sandbox','unsandboxed')),  -- admin-only flag
  shared_data_cap  INTEGER NOT NULL DEFAULT 5242880,  -- bytes of shared data_docs; guests get quota_exceeded past it
  preview          TEXT CHECK (preview IS NULL OR json_valid(preview)),  -- {video, poster, actions, seconds} (§4c); NULL = automatic poster, no clip
  backend_lifecycle TEXT NOT NULL DEFAULT 'none' CHECK (backend_lifecycle IN ('none','scale_to_zero','always_on')),
  api_prefix       TEXT NOT NULL DEFAULT '/api/' CHECK (api_prefix GLOB '/*/'),  -- backend route under /c/<cap>/
  forked_from      TEXT REFERENCES versions(id),     -- part_create {from}
  expires_at       INTEGER,                          -- part_create {expires_in}: moves to the bin when due
  created_by_user  TEXT NOT NULL REFERENCES users(id),
  created_by_conn  TEXT,                             -- agent_connections.id or NULL
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL,
  deleted_at       INTEGER,                          -- the 30-day bin; purge after 30 d
  CHECK (name GLOB '[a-z0-9]*' AND name NOT GLOB '*[^a-z0-9-]*' AND length(name) BETWEEN 1 AND 58
         AND name NOT LIKE '-%' AND name NOT LIKE '%-' AND instr(name,'--') = 0)
) STRICT;
CREATE INDEX parts_space_updated ON parts(space_id, updated_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX parts_deleted ON parts(deleted_at) WHERE deleted_at IS NOT NULL;
CREATE INDEX parts_expiring ON parts(expires_at) WHERE expires_at IS NOT NULL AND deleted_at IS NULL;

CREATE TABLE names (                                 -- every name ever used; never deleted
  name        TEXT PRIMARY KEY,
  part_id     TEXT NOT NULL REFERENCES parts(id),
  state       TEXT NOT NULL CHECK (state IN ('active','redirect','tombstone')),
  created_at  INTEGER NOT NULL,
  retired_at  INTEGER
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX names_one_active ON names(part_id) WHERE state = 'active';

CREATE TABLE part_tags (
  part_id TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  tag     TEXT NOT NULL,
  PRIMARY KEY (part_id, tag)
) STRICT, WITHOUT ROWID;
CREATE INDEX part_tags_tag ON part_tags(tag);

CREATE TABLE part_notes (                            -- agent notes: versioned separately from content
  part_id    TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  n          INTEGER NOT NULL,
  body       TEXT NOT NULL CHECK (length(body) <= 16384),
  author_user TEXT NOT NULL REFERENCES users(id),
  actor_conn TEXT,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (part_id, n)
) STRICT;

------------------------------------------------------------------ versions, builds, blobs
CREATE TABLE versions (
  id            TEXT PRIMARY KEY,                    -- sha256 hex of canonical manifest
  part_id       TEXT NOT NULL REFERENCES parts(id),
  n             INTEGER NOT NULL,
  parent_id     TEXT REFERENCES versions(id),
  manifest_hash TEXT NOT NULL,                       -- = id; manifest bytes live in blobs
  entry         TEXT NOT NULL,
  author_user   TEXT NOT NULL REFERENCES users(id),
  actor_conn    TEXT,                                -- NULL = the person directly
  via           TEXT NOT NULL CHECK (via IN ('api','mcp','cli','editor','self_publish','restore')),
  message       TEXT,
  rebased_from  TEXT,
  restored_from TEXT,
  live_gate     TEXT NOT NULL DEFAULT 'always' CHECK (live_gate IN ('always','after_check')),  -- part_create/part_edit {live}
  created_at    INTEGER NOT NULL,
  UNIQUE (part_id, n)
) STRICT;
CREATE INDEX versions_actor_day ON versions(actor_conn, created_at);  -- quotas: versions/day per token

CREATE TABLE version_files (                         -- denormalized manifest for diff, GC, FTS, serving
  version_id TEXT NOT NULL REFERENCES versions(id) ON DELETE CASCADE,
  path       TEXT NOT NULL,
  blob_hash  TEXT NOT NULL REFERENCES blobs(hash),
  size       INTEGER NOT NULL,
  mime       TEXT NOT NULL,
  PRIMARY KEY (version_id, path)
) STRICT, WITHOUT ROWID;
CREATE INDEX version_files_blob ON version_files(blob_hash);

CREATE TABLE builds (
  id               INTEGER PRIMARY KEY,
  version_id       TEXT NOT NULL REFERENCES versions(id) ON DELETE CASCADE,
  cache_key        TEXT NOT NULL,                    -- sha256(src manifest || toolchain || libset || mirror resolution)
  toolchain        TEXT NOT NULL,                    -- pinned oxc/rolldown/builder version id
  target           TEXT NOT NULL DEFAULT 'frontend' CHECK (target IN ('frontend','backend')),
  method           TEXT NOT NULL CHECK (method IN ('server_build','prebuilt_files','prebuilt_image','uploaded_image')),  -- for Insights
  status           TEXT NOT NULL CHECK (status IN ('queued','running','ok','failed','timeout','superseded')),
  output_hash      TEXT REFERENCES blobs(hash),      -- build manifest blob (frontend) or image manifest (backend)
  log_hash         TEXT REFERENCES blobs(hash),      -- build log; last 20 builds per part kept (janitor)
  sbom_hash        TEXT REFERENCES blobs(hash),      -- backend: the image SBOM from the runner's pinned scanner (§3h); capped per build
  warnings         TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(warnings)),
  errors           TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(errors)),
  gate_check       TEXT CHECK (gate_check IN ('pending','passed','failed','skipped')),  -- live_gate = after_check only
  queued_at        INTEGER NOT NULL,
  started_at       INTEGER,
  finished_at      INTEGER,
  UNIQUE (version_id, cache_key)
) STRICT;
CREATE INDEX builds_queue ON builds(status, queued_at) WHERE status IN ('queued','running');
CREATE INDEX builds_cache ON builds(cache_key) WHERE status = 'ok';

CREATE TABLE build_files (                           -- what the part host may serve for a build
  build_id  INTEGER NOT NULL REFERENCES builds(id) ON DELETE CASCADE,
  path      TEXT NOT NULL,
  blob_hash TEXT NOT NULL REFERENCES blobs(hash),
  br_hash   TEXT REFERENCES blobs(hash),
  content_type TEXT NOT NULL,
  PRIMARY KEY (build_id, path)
) STRICT, WITHOUT ROWID;
CREATE INDEX build_files_blob ON build_files(blob_hash);

CREATE TABLE blobs (
  hash        TEXT PRIMARY KEY,                      -- sha256 hex of plaintext
  size        INTEGER NOT NULL,
  created_at  INTEGER NOT NULL,
  gc_mark     INTEGER NOT NULL DEFAULT 0,
  unref_since INTEGER                                -- set by sweep; delete after 7 d grace
) STRICT, WITHOUT ROWID;
CREATE INDEX blobs_unref ON blobs(unref_since) WHERE unref_since IS NOT NULL;

CREATE TABLE shared_blobs (                          -- class is per reference: only these are served by blobs-part
  hash       TEXT PRIMARY KEY REFERENCES blobs(hash),
  source     TEXT NOT NULL CHECK (source IN ('seed','mirror','runtime')),
  label      TEXT NOT NULL,                          -- 'react@18.3.1/umd/react.production.min.js'
  added_at   INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE mirror_cache (
  url          TEXT PRIMARY KEY,
  resolved_url TEXT NOT NULL,
  hash         TEXT NOT NULL REFERENCES blobs(hash),
  content_type TEXT NOT NULL,
  integrity    TEXT,
  pinned       INTEGER NOT NULL CHECK (pinned IN (0,1)),  -- exact version: cache forever
  fetched_at   INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE build_deps (                            -- each build's library list, a small SBOM (§3h); deleted with its build
  id         INTEGER PRIMARY KEY,
  build_id   INTEGER NOT NULL REFERENCES builds(id) ON DELETE CASCADE,
  ecosystem  TEXT NOT NULL,                          -- OSV ecosystem ('npm', 'crates.io', 'PyPI', 'Go', 'Debian', 'Alpine', 'Ubuntu', ...) or 'unknown'
  name       TEXT NOT NULL,                          -- package name; for 'unknown', the URL as written
  version    TEXT NOT NULL DEFAULT '',               -- '' when it couldn't be parsed
  source_url TEXT,                                   -- the mirrored CDN URL; NULL for the library set and image SBOM entries
  blob_hash  TEXT REFERENCES blobs(hash)             -- the mirrored file; NULL for image SBOM entries
) STRICT;
CREATE INDEX build_deps_build ON build_deps(build_id);
CREATE INDEX build_deps_pkg ON build_deps(ecosystem, name, version);

CREATE TABLE advisories (                            -- a known vulnerability in a build's libraries or image (§3h); deleted with its build
  id          INTEGER PRIMARY KEY,
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  build_id    INTEGER NOT NULL REFERENCES builds(id) ON DELETE CASCADE,
  ecosystem   TEXT NOT NULL,
  package     TEXT NOT NULL,
  version     TEXT NOT NULL,
  osv_id      TEXT NOT NULL,                         -- 'GHSA-...', 'CVE-...', 'RUSTSEC-...'
  severity    TEXT NOT NULL CHECK (severity IN ('critical','high','medium','low','unknown')),  -- OSV's; the context is applied when shown
  fixed_in    TEXT,                                  -- first fixed version, when OSV knows one (auto-update needs it)
  first_seen  INTEGER NOT NULL,
  state       TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','fixed','ignored')),
  ignored_by  TEXT,                                  -- user:<id> | conn:<id> (parts:write); audited
  reason      TEXT CHECK (reason IS NULL OR length(reason) <= 500),
  CHECK ((state = 'ignored') = (ignored_by IS NOT NULL AND reason IS NOT NULL)),
  UNIQUE (build_id, ecosystem, package, version, osv_id)
) STRICT;
CREATE INDEX advisories_open ON advisories(part_id) WHERE state = 'open';

CREATE TABLE drafts (                                -- workspace editor drafts, per person per part
  user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  part_id    TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  base_version_id TEXT NOT NULL REFERENCES versions(id),
  files      TEXT NOT NULL CHECK (json_valid(files)),  -- {path: text}; previewed via a cap with a d claim
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, part_id)
) STRICT, WITHOUT ROWID;

------------------------------------------------------------------ access
CREATE TABLE grants (
  id          INTEGER PRIMARY KEY,
  target_kind TEXT NOT NULL CHECK (target_kind IN ('part','space')),
  target_id   TEXT NOT NULL,
  subject     TEXT NOT NULL CHECK (subject GLOB 'user:*' OR subject GLOB 'group:*' OR subject = 'everyone-signed-in'),
  role        INTEGER NOT NULL CHECK (role BETWEEN 0 AND 3),  -- view, comment, edit, manage
  expires_at  INTEGER,
  created_by  TEXT NOT NULL REFERENCES users(id),   -- the person
  created_by_conn TEXT,                              -- set when an agent made it (share:household): labelled, undoable
  created_at  INTEGER NOT NULL,
  UNIQUE (target_kind, target_id, subject)
) STRICT;
CREATE INDEX grants_subject ON grants(subject);
CREATE INDEX grants_expiry ON grants(expires_at) WHERE expires_at IS NOT NULL;

-- Public links: admin-only, made once the part has a live version, no expiry by default. There are no agent-minted preview links.
CREATE TABLE public_links (
  id            TEXT PRIMARY KEY,                    -- l_...; also the cap subject link:<id> and minter
  token_hash    BLOB NOT NULL UNIQUE,                -- sha256 of the 128-bit /l/<token>
  part_id       TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  role          INTEGER NOT NULL DEFAULT 0 CHECK (role IN (0,1)),  -- view; comment only with parts.link_comments
  mode          TEXT NOT NULL DEFAULT 'view' CHECK (mode IN ('view','contribute')),  -- contribute: guests write shared data
  frozen_at     INTEGER,                             -- last Freeze (contribute -> view); bumps minter:link:<id>
  preview       TEXT NOT NULL DEFAULT 'video' CHECK (preview IN ('video','image','card')),  -- the unfurl preview (§4c); video falls back to image, then card
  created_by    TEXT NOT NULL REFERENCES users(id),  -- always the admin
  created_at    INTEGER NOT NULL,
  expires_at    INTEGER,                             -- NULL = never (the default)
  revoked_at    INTEGER,
  open_count    INTEGER NOT NULL DEFAULT 0,          -- counted on POST /l/<token>/go
  last_opened_at INTEGER,
  request_cap   INTEGER                              -- optional per-link request limit
) STRICT;
CREATE INDEX public_links_part ON public_links(part_id) WHERE revoked_at IS NULL;


-- Capabilities are stateless (in the path, never in a cookie). Only their revocation state is stored.
CREATE TABLE epochs (
  key   TEXT PRIMARY KEY,                            -- 'part:<id>' | 'minter:sess:<id>' | 'minter:conn:<id>' | 'minter:link:<id>' | 'global' | 'boot'
  epoch INTEGER NOT NULL DEFAULT 0
) STRICT, WITHOUT ROWID;

CREATE TABLE access_asks (                          -- 'Ask for access' from a signed-in person; person-to-person, no agent rows
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  note        TEXT CHECK (note IS NULL OR length(note) <= 280),
  created_at  INTEGER NOT NULL,
  closed_at   INTEGER,                               -- Add (an ordinary grant) or Dismiss
  closed_as   TEXT CHECK (closed_as IN ('added','dismissed')),
  PRIMARY KEY (part_id, user_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX access_asks_open ON access_asks(part_id) WHERE closed_at IS NULL;

CREATE TABLE invites (
  id              TEXT PRIMARY KEY,                  -- i_...; handed out as /i/<id>, which records opens
  idp_ref         TEXT,                              -- provisioner only: IdP invitation (unlimited until expiry) or recovery id; NULL for native invites
  kind            TEXT NOT NULL CHECK (kind IN ('single','multi','email','recovery')),
  max_uses        INTEGER NOT NULL DEFAULT 1,        -- counted here; the IdP has no use count
  uses            INTEGER NOT NULL DEFAULT 0,
  email           TEXT,
  target_user_id  TEXT REFERENCES users(id),         -- recovery
  land_part_id    TEXT REFERENCES parts(id),
  grant_role      INTEGER CHECK (grant_role BETWEEN 0 AND 3),
  groups          TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(groups)),  -- pf-* groups, added by API after enrolment
  status          TEXT NOT NULL CHECK (status IN ('sent','opened','enrolled','expired','revoked')),
  created_at      INTEGER NOT NULL,
  opened_at       INTEGER,
  expires_at      INTEGER NOT NULL,
  revoked_at      INTEGER,
  CHECK (uses <= max_uses)
) STRICT;
CREATE INDEX invites_open ON invites(expires_at) WHERE status IN ('sent','opened');

CREATE TABLE invite_enrolments (
  invite_id TEXT NOT NULL REFERENCES invites(id),
  user_id   TEXT NOT NULL REFERENCES users(id),
  at        INTEGER NOT NULL,
  PRIMARY KEY (invite_id, user_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE audit (
  id          INTEGER PRIMARY KEY,
  at          INTEGER NOT NULL,
  user_id     TEXT,                                  -- the person (author)
  actor       TEXT,                                  -- 'conn:<id>' | 'break-glass' | 'system' | NULL
  action      TEXT NOT NULL,                         -- 'grant.set', 'link.revoke', 'invite.create', ...
  target_kind TEXT NOT NULL,
  target_id   TEXT NOT NULL,
  before      TEXT CHECK (before IS NULL OR json_valid(before)),
  after       TEXT CHECK (after IS NULL OR json_valid(after)),
  request_id  TEXT
) STRICT;
CREATE INDEX audit_target ON audit(target_kind, target_id, at DESC);

------------------------------------------------------------------ agents (OAuth AS + personal tokens)
CREATE TABLE oauth_clients (
  id            TEXT PRIMARY KEY,                    -- client_id (CIMD URL, DCR id, manual id, or 'part-cli')
  kind          TEXT NOT NULL CHECK (kind IN ('cimd','dcr','manual','builtin')),
  client_name   TEXT NOT NULL,
  metadata      TEXT NOT NULL CHECK (json_valid(metadata)),
  redirect_uris TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(redirect_uris)),
  created_at    INTEGER NOT NULL,
  consented_at  INTEGER,                             -- NULL + older than 1 h => purged
  secret_hash   BLOB,                                -- manual confidential clients only
  reg_ip_hash   BLOB
) STRICT;
CREATE INDEX oauth_clients_unconsented ON oauth_clients(created_at) WHERE consented_at IS NULL;

CREATE TABLE agent_connections (                     -- one row per (agent, person); the 'act' and minter for agents
  id          TEXT PRIMARY KEY,                      -- c_...
  kind        TEXT NOT NULL CHECK (kind IN ('oauth_code','device','pat')),
  principal_kind TEXT NOT NULL DEFAULT 'person' CHECK (principal_kind IN ('person','service')),
  client_id   TEXT REFERENCES oauth_clients(id),     -- NULL for pat
  user_id     TEXT REFERENCES users(id),             -- the person it acts for; NULL for a service identity
  agent_name  TEXT NOT NULL,                         -- e.g. 'Claude Code', 'Hermes'; shown as '<agent> for <person>'
  software    TEXT,                                  -- self-reported, e.g. 'hermes/1.4' (unverified)
  scopes      TEXT NOT NULL CHECK (json_valid(scopes)),
  spaces      TEXT NOT NULL CHECK (json_valid(spaces)),
  expires_at  INTEGER NOT NULL,                      -- consented lifetime
  created_at  INTEGER NOT NULL,
  last_used_at INTEGER,
  revoked_at  INTEGER,
  quota_versions_day INTEGER,                         -- NULL = the §5d default
  CHECK ((principal_kind = 'person') = (user_id IS NOT NULL))
) STRICT;
CREATE INDEX agent_connections_user ON agent_connections(user_id) WHERE revoked_at IS NULL;

CREATE TABLE oauth_tokens (
  hash        BLOB PRIMARY KEY,                      -- sha256 of opaque token
  kind        TEXT NOT NULL CHECK (kind IN ('access','refresh','pat')),
  prefix      TEXT NOT NULL,                         -- 'pf_at_' | 'pf_rt_' | 'pf_pat_' (secret scanning, logs)
  connection_id TEXT NOT NULL REFERENCES agent_connections(id) ON DELETE CASCADE,
  family_id   TEXT,                                  -- refresh rotation family
  resource    TEXT NOT NULL,                         -- RFC 8707 audience: always the workspace origin, e.g. https://parts.example.com/
  issued_at   INTEGER NOT NULL,
  expires_at  INTEGER NOT NULL,
  used_at     INTEGER,                               -- refresh: set on rotation; reuse => revoke family
  revoked_at  INTEGER
) STRICT, WITHOUT ROWID;
CREATE INDEX oauth_tokens_family ON oauth_tokens(family_id) WHERE family_id IS NOT NULL;
CREATE INDEX oauth_tokens_conn ON oauth_tokens(connection_id);

CREATE TABLE oauth_codes (
  hash          BLOB PRIMARY KEY,
  client_id     TEXT NOT NULL REFERENCES oauth_clients(id),
  user_id       TEXT NOT NULL REFERENCES users(id),
  redirect_uri  TEXT NOT NULL,
  code_challenge TEXT NOT NULL,
  scopes        TEXT NOT NULL,
  spaces        TEXT NOT NULL,
  resource      TEXT NOT NULL,
  lifetime_ms   INTEGER NOT NULL,
  expires_at    INTEGER NOT NULL,                    -- 60 s
  used_at       INTEGER
) STRICT, WITHOUT ROWID;

CREATE TABLE device_codes (
  device_hash   BLOB PRIMARY KEY,
  user_code     TEXT NOT NULL UNIQUE,                -- XXXX-XXXX
  client_id     TEXT NOT NULL REFERENCES oauth_clients(id),
  agent_name    TEXT NOT NULL,
  software      TEXT,
  req_scopes    TEXT NOT NULL,
  req_geo       TEXT,                                -- coarse country/network from proxy headers or GeoIP; NULL if unknown
  status        TEXT NOT NULL CHECK (status IN ('pending','approved','denied','expired','consumed')),
  connection_id TEXT REFERENCES agent_connections(id),
  interval_s    INTEGER NOT NULL DEFAULT 5,
  last_poll_at  INTEGER,
  created_at    INTEGER NOT NULL,
  expires_at    INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE idempotency (                           -- every write's request_id; a row is stored only when the write commits
  principal  TEXT NOT NULL,                          -- 'user:<id>' or 'conn:<id>'
  request_id TEXT NOT NULL,
  operation  TEXT NOT NULL,
  response   TEXT NOT NULL CHECK (json_valid(response)),
  created_at INTEGER NOT NULL,                       -- purge after 24 h
  PRIMARY KEY (principal, request_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE leases (                                -- part_edit {claim}; advisory
  id         TEXT PRIMARY KEY,
  part_id    TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  scope      TEXT NOT NULL DEFAULT 'part' CHECK (scope IN ('part','file','block')),
  scope_ref  TEXT NOT NULL DEFAULT '',               -- path or block id
  holder_user TEXT REFERENCES users(id),
  holder_conn TEXT,
  holder_label TEXT NOT NULL,
  expires_at INTEGER NOT NULL,                       -- <= created_at + 10 min
  created_at INTEGER NOT NULL,
  UNIQUE (part_id, scope, scope_ref)
) STRICT;

------------------------------------------------------------------ collaboration + runtime state
CREATE TABLE comments (
  id          TEXT PRIMARY KEY,
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  thread_id   TEXT NOT NULL,                         -- = id of root
  parent_id   TEXT REFERENCES comments(id),
  version_id  TEXT REFERENCES versions(id),
  anchor      TEXT CHECK (anchor IS NULL OR json_valid(anchor)),  -- {block?|file,lines?, quote}
  body        TEXT NOT NULL CHECK (length(body) <= 16384),
  provenance  TEXT NOT NULL CHECK (provenance IN ('person','agent','link_viewer')),
  author_user TEXT REFERENCES users(id),
  author_conn TEXT,
  author_link TEXT REFERENCES public_links(id),
  to_agent    INTEGER NOT NULL DEFAULT 0,
  agent_state TEXT CHECK (agent_state IN ('open','claimed','answered')),  -- inbox (P3); resolved = resolved_at
  resolved_at INTEGER,
  resolved_by TEXT,
  created_at  INTEGER NOT NULL,
  deleted_at  INTEGER
) STRICT;
CREATE INDEX comments_part ON comments(part_id, created_at);
CREATE INDEX comments_to_agent ON comments(part_id) WHERE to_agent = 1 AND resolved_at IS NULL;

CREATE TABLE data_docs (                             -- window.storage + claude.use("db"); survives versions
  part_id    TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  scope      TEXT NOT NULL CHECK (scope IN ('shared','personal')),
  owner      TEXT NOT NULL DEFAULT '',               -- user id for personal, '' for shared
  collection TEXT NOT NULL,                          -- '__storage' for window.storage
  doc_id     TEXT NOT NULL,
  body       BLOB NOT NULL CHECK (length(body) <= 262144),
  rev        INTEGER NOT NULL,                       -- CAS
  updated_at INTEGER NOT NULL,
  updated_by TEXT NOT NULL,                          -- user:<id> | conn:<id> | link:<id> | guest:<link>:<g>
  CHECK ((scope = 'shared') = (owner = '')),
  PRIMARY KEY (part_id, scope, owner, collection, doc_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE data_leases (
  part_id    TEXT NOT NULL,
  collection TEXT NOT NULL,
  doc_id     TEXT NOT NULL,
  holder     TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  PRIMARY KEY (part_id, collection, doc_id)
) STRICT, WITHOUT ROWID;

-- Per-part data snapshots: ON by default (daily, keep 7), for undoing a bad write (not disaster recovery).
-- A part with no row uses the default; a row exists only once someone changes it.
CREATE TABLE data_snapshot_policies (
  part_id     TEXT PRIMARY KEY REFERENCES parts(id) ON DELETE CASCADE,
  mode        TEXT NOT NULL DEFAULT 'daily' CHECK (mode IN ('off','daily','hourly')),  -- daily keep 7, hourly keep 48
  keep        INTEGER NOT NULL DEFAULT 7,
  expire_days INTEGER NOT NULL DEFAULT 90,           -- stop after N days unopened; keep the last one
  set_by      TEXT NOT NULL,                         -- user:<id> | conn:<id> (agents may request at create)
  set_at      INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE data_snapshots (
  id         INTEGER PRIMARY KEY,
  part_id    TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  taken_at   INTEGER NOT NULL,
  reason     TEXT NOT NULL DEFAULT 'scheduled' CHECK (reason IN ('scheduled','contribute','before_restore','before_reset')),  -- only 'scheduled' counts against keep
  blob_hash  TEXT NOT NULL REFERENCES blobs(hash),   -- canonical JSON of the part's shared data_docs
  docs       INTEGER NOT NULL,
  bytes      INTEGER NOT NULL
) STRICT;
CREATE INDEX data_snapshots_part ON data_snapshots(part_id, taken_at DESC);

CREATE TABLE assets (                                -- viewer uploads (P3)
  id          TEXT PRIMARY KEY,                      -- a_...; served at /_a/<id>
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  blob_hash   TEXT NOT NULL REFERENCES blobs(hash),
  mime        TEXT NOT NULL,
  size        INTEGER NOT NULL CHECK (size <= 26214400),
  name        TEXT NOT NULL,
  uploaded_by TEXT NOT NULL,                         -- user:<id> | conn:<id>
  created_at  INTEGER NOT NULL,
  deleted_at  INTEGER
) STRICT;
CREATE INDEX assets_part ON assets(part_id) WHERE deleted_at IS NULL;

CREATE TABLE runtime_errors (
  id          INTEGER PRIMARY KEY,
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  version_id  TEXT NOT NULL,
  source      TEXT NOT NULL DEFAULT 'viewer' CHECK (source IN ('viewer','render')),
  fingerprint TEXT NOT NULL,                         -- hash(kind, message, top frame)
  kind        TEXT NOT NULL CHECK (kind IN ('error','unhandledrejection','blocked_load','csp')),
  message     TEXT NOT NULL,
  stack       TEXT,
  url         TEXT,
  count       INTEGER NOT NULL DEFAULT 1,
  first_at    INTEGER NOT NULL,
  last_at     INTEGER NOT NULL,
  UNIQUE (part_id, version_id, source, fingerprint)
) STRICT;
CREATE INDEX runtime_errors_part ON runtime_errors(part_id, last_at DESC);
CREATE TRIGGER runtime_errors_keep1000 AFTER INSERT ON runtime_errors BEGIN   -- cap 1,000 per part; the janitor drops > 30 d
  DELETE FROM runtime_errors WHERE part_id = NEW.part_id AND id NOT IN
    (SELECT id FROM runtime_errors WHERE part_id = NEW.part_id ORDER BY last_at DESC LIMIT 1000);
END;

------------------------------------------------------------------ backends (P2a; only the runner touches the Engine API)
CREATE TABLE backend_images (
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  digest      TEXT NOT NULL,                         -- 'sha256:...' as the runtime reports it
  source      TEXT NOT NULL CHECK (source IN ('server_build','prebuilt_image','uploaded_image')),
  ref         TEXT,                                  -- registry ref as written (prebuilt_image)
  tar_hash    TEXT REFERENCES blobs(hash),           -- uploaded_image: the docker-save tarball
  size        INTEGER NOT NULL,
  created_at  INTEGER NOT NULL,
  pruned_at   INTEGER,                               -- removed from the runtime; kept: live + previous 2 versions
  PRIMARY KEY (part_id, digest)
) STRICT, WITHOUT ROWID;

CREATE TABLE base_image_checks (                     -- the runner's nightly re-resolve of each base-image tag a backend build used (§3h)
  build_id     INTEGER NOT NULL REFERENCES builds(id) ON DELETE CASCADE,
  ref          TEXT NOT NULL,                        -- the tag as written in FROM or image:, e.g. 'postgres:16-alpine'
  digest       TEXT NOT NULL,                        -- what the build used
  newer_digest TEXT,                                 -- set when the tag now resolves elsewhere: "a newer digest exists"
  checked_at   INTEGER NOT NULL,
  PRIMARY KEY (build_id, ref)
) STRICT, WITHOUT ROWID;

CREATE TABLE container_sets (                        -- one per (part, version) that has been started
  id              INTEGER PRIMARY KEY,
  part_id         TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  version_id      TEXT NOT NULL REFERENCES versions(id),
  state           TEXT NOT NULL CHECK (state IN ('starting','healthy','unhealthy','draining','stopped','failed')),
  started_at      INTEGER,
  healthy_at      INTEGER,
  last_request_at INTEGER,                           -- scale-to-zero idle timer (15 min)
  stopped_at      INTEGER,
  cold_starts     INTEGER NOT NULL DEFAULT 0,
  UNIQUE (part_id, version_id)
) STRICT;
CREATE INDEX container_sets_running ON container_sets(state) WHERE state IN ('starting','healthy','draining');

CREATE TABLE containers (
  id           TEXT PRIMARY KEY,                     -- the runtime's container id
  set_id       INTEGER NOT NULL REFERENCES container_sets(id) ON DELETE CASCADE,
  service      TEXT NOT NULL,
  role         TEXT NOT NULL CHECK (role IN ('main','worker','cron','dependency')),
  image_digest TEXT NOT NULL,
  state        TEXT NOT NULL CHECK (state IN ('created','running','exited','dead')),
  restarts     INTEGER NOT NULL DEFAULT 0,
  started_at   INTEGER,
  exited_at    INTEGER,
  exit_code    INTEGER
) STRICT;
CREATE INDEX containers_set ON containers(set_id);

CREATE TABLE part_volumes (                          -- /data/parts/<id>/vol/<service>; one named volume per service
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  service     TEXT NOT NULL,
  quota_bytes INTEGER NOT NULL DEFAULT 1073741824,
  used_bytes  INTEGER NOT NULL DEFAULT 0,
  measured_at INTEGER,
  PRIMARY KEY (part_id, service)
) STRICT, WITHOUT ROWID;

CREATE TABLE part_secrets (                          -- write-only: set or replaced by agents (backends:deploy) or people (edit); never read back
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  name        TEXT NOT NULL CHECK (name GLOB '[A-Z_]*' AND name NOT GLOB '*[^A-Z0-9_]*' AND length(name) <= 64),
  value_enc   BLOB NOT NULL,                         -- encrypted with PF_SESSION_KEY; no op returns it
  inject_as   TEXT NOT NULL DEFAULT 'env' CHECK (inject_as IN ('env','file')),
  set_by_user TEXT NOT NULL REFERENCES users(id),    -- the person (author)
  set_by_conn TEXT,                                  -- agent connection, if an agent set it
  set_at      INTEGER NOT NULL,
  PRIMARY KEY (part_id, name)
) STRICT, WITHOUT ROWID;

CREATE TABLE lan_allow (                             -- per-part exceptions to the LAN block; set by a person (admin UI), never an agent
  id          INTEGER PRIMARY KEY,
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  target      TEXT NOT NULL,                         -- 'host:port' or a CIDR
  note        TEXT NOT NULL,                         -- 'may reach the home-automation server'
  added_by    TEXT NOT NULL REFERENCES users(id),
  added_at    INTEGER NOT NULL,
  revoked_at  INTEGER
) STRICT;
CREATE INDEX lan_allow_part ON lan_allow(part_id) WHERE revoked_at IS NULL;

------------------------------------------------------------------ events, webhooks, jobs
CREATE TABLE events (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,      -- cursor for part_watch / SSE Last-Event-ID; kept 30 d
  at         INTEGER NOT NULL,
  part_id    TEXT,
  space_id   TEXT,
  type       TEXT NOT NULL,                          -- part.created, version.committed, build.finished, part.live_changed, comment.added, data.changed, backend.started, part.advisory, ...
  actor      TEXT,                                   -- user:<id> | conn:<id> | guest:<link>:<g> | system
  payload    TEXT NOT NULL CHECK (json_valid(payload))
) STRICT;
CREATE INDEX events_part ON events(part_id, seq);
CREATE INDEX events_space ON events(space_id, seq);

CREATE TABLE webhook_deliveries (                    -- targets live in site config, never in DB
  id         INTEGER PRIMARY KEY,
  target     TEXT NOT NULL,                          -- site-config target name
  event_seq  INTEGER NOT NULL REFERENCES events(seq),
  attempts   INTEGER NOT NULL DEFAULT 0,
  next_at    INTEGER NOT NULL,
  status     TEXT NOT NULL CHECK (status IN ('pending','ok','dead')),
  last_error TEXT,
  UNIQUE (target, event_seq)
) STRICT;
CREATE INDEX webhook_due ON webhook_deliveries(next_at) WHERE status = 'pending';

CREATE TABLE render_jobs (                           -- tier (b) sidecar queue; concurrency 1, coalesced per part
  id          INTEGER PRIMARY KEY,
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  version_id  TEXT NOT NULL,
  viewport    TEXT NOT NULL CHECK (viewport IN ('mobile','desktop')),
  purpose     TEXT NOT NULL CHECK (purpose IN ('check','gate','thumbnail','og','preview')),  -- gate: the live = after_check check; preview: the og:video recording (§4c)
  requested_by TEXT,                                 -- conn:<id> for part_check
  request     TEXT CHECK (request IS NULL OR json_valid(request)),  -- {wait_ms, actions, record, seconds}
  status      TEXT NOT NULL CHECK (status IN ('queued','running','ok','failed','superseded')),
  result      TEXT CHECK (result IS NULL OR json_valid(result)),  -- {screenshot_hash, video_hash?, poster_hash?, blank, errors, console, failed_requests, actions, timing}; GET /api/v1/screenshots/<id>
  queued_at   INTEGER NOT NULL,
  finished_at INTEGER
) STRICT;
CREATE INDEX render_jobs_queue ON render_jobs(status, queued_at) WHERE status IN ('queued','running');

CREATE TABLE part_previews (                         -- §4c: the newest preview per part; replaced rows leave their blobs to GC
  part_id     TEXT PRIMARY KEY REFERENCES parts(id) ON DELETE CASCADE,
  version_id  TEXT NOT NULL REFERENCES versions(id),
  source      TEXT NOT NULL CHECK (source IN ('render','agent','recorded')),  -- render: Slice 1's latest check screenshot, poster only
  poster_hash TEXT,                                  -- part blob; also the gallery thumbnail
  video_hash  TEXT,                                  -- part blob, MP4 (§4c profile)
  width       INTEGER,
  height      INTEGER,
  duration_ms INTEGER,
  bytes       INTEGER,
  state       TEXT NOT NULL CHECK (state IN ('ready','pending','failed')),
  error       TEXT,                                  -- why the last recording or probe failed; the last good clip stays
  updated_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE kv (                                    -- singletons: directory poll cursor + staleness, provisioner tier, last export, last drill, alert state, disk budget state
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) STRICT, WITHOUT ROWID;

------------------------------------------------------------------ search
CREATE VIRTUAL TABLE parts_fts USING fts5(
  part_id UNINDEXED, title, description, tags, body, notes, comments,   -- body = extracted readable text, not code
  tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3'
);
```

```sql
-- /data/insights.db: analytics and observability (§5g). A separate file with its own writer.
-- Never part content: ids, op names, argument keys, sizes, statuses and timings only (a CI fuzz test checks it).
CREATE TABLE events_raw (                            -- kept 30 d, then only in rollups_daily
  id          INTEGER PRIMARY KEY,
  at          INTEGER NOT NULL,
  kind        TEXT NOT NULL,                         -- 'view','op','rt','grant_decision','build','backend','link_open', ...
  surface     TEXT CHECK (surface IN ('mcp','http','cli','ui','part','rt','backend')),
  op          TEXT,                                  -- registry op name or route class
  part_id     TEXT,
  actor       TEXT,                                  -- user:<id> | conn:<id> | guest:<link>:<g> | link:<id> | NULL
  viewer_key  BLOB,                                  -- public viewers and guests: hash(truncated IP, UA, rotating salt)
  status      TEXT,                                  -- 'ok' or an error code
  latency_us  INTEGER,
  bytes_in    INTEGER,
  bytes_out   INTEGER,
  request_id  TEXT,
  attrs       TEXT CHECK (attrs IS NULL OR json_valid(attrs))  -- {arg_keys, build_method, kind, ...}; never values
) STRICT;
CREATE INDEX events_raw_at ON events_raw(at);
CREATE INDEX events_raw_part ON events_raw(part_id, at) WHERE part_id IS NOT NULL;
CREATE INDEX events_raw_actor ON events_raw(actor, at) WHERE actor IS NOT NULL;

CREATE TABLE rollups_daily (                         -- kept indefinitely; small
  day     INTEGER NOT NULL,                          -- days since epoch, UTC
  metric  TEXT NOT NULL,                             -- 'views','uniques','ops','errors','builds_ok','cold_starts','guest_writes', ...
  dim     TEXT NOT NULL DEFAULT '',                  -- 'part:<id>' | 'conn:<id>' | 'op:<name>' | 'method:server_build' | ''
  value   INTEGER NOT NULL,
  PRIMARY KEY (day, metric, dim)
) STRICT, WITHOUT ROWID;

CREATE TABLE traces (                                -- tail-sampled: every error, every slow request, 1 % of the rest; 7 d, 2 GiB
  trace_id     BLOB PRIMARY KEY,
  request_id   TEXT NOT NULL,
  started_at   INTEGER NOT NULL,
  duration_us  INTEGER NOT NULL,
  root         TEXT NOT NULL,                        -- route class or op
  status       TEXT NOT NULL,
  part_id      TEXT,
  actor        TEXT,
  kept_because TEXT NOT NULL CHECK (kept_because IN ('error','slow','sample'))
) STRICT, WITHOUT ROWID;
CREATE INDEX traces_at ON traces(started_at);
CREATE INDEX traces_request ON traces(request_id);

CREATE TABLE spans (
  trace_id    BLOB NOT NULL REFERENCES traces(trace_id) ON DELETE CASCADE,
  span_id     INTEGER NOT NULL,
  parent_id   INTEGER,
  name        TEXT NOT NULL,                         -- 'policy.decide','sqlite.write','build.child','runner.start', ...
  start_us    INTEGER NOT NULL,                      -- offset from the trace start
  duration_us INTEGER NOT NULL,
  fields      TEXT CHECK (fields IS NULL OR json_valid(fields)),
  PRIMARY KEY (trace_id, span_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE log_segments (                          -- index over /data/logs/**/*.jsonl.zst; the lines live in the files
  path      TEXT PRIMARY KEY,                        -- 'app/2026-10-23T14-0001.jsonl.zst' | 'parts/<id>/backend-0042.jsonl.zst'
  stream    TEXT NOT NULL CHECK (stream IN ('app','backend')),
  part_id   TEXT,
  first_at  INTEGER NOT NULL,
  last_at   INTEGER NOT NULL,
  bytes     INTEGER NOT NULL,
  lines     INTEGER NOT NULL,
  max_level TEXT CHECK (max_level IN ('trace','debug','info','warn','error')),
  CHECK ((stream = 'backend') = (part_id IS NOT NULL))
) STRICT, WITHOUT ROWID;
CREATE INDEX log_segments_stream ON log_segments(stream, part_id, first_at);

CREATE TABLE janitor_runs (                          -- what each retention job freed (§5h)
  id          INTEGER PRIMARY KEY,
  at          INTEGER NOT NULL,
  store       TEXT NOT NULL,                         -- a row of the §5h retention table
  action      TEXT NOT NULL CHECK (action IN ('prune','rotate','gc','checkpoint','vacuum','image_prune','cache_prune')),
  items       INTEGER NOT NULL,
  freed_bytes INTEGER NOT NULL,
  size_after  INTEGER NOT NULL,
  cap_bytes   INTEGER,
  error       TEXT
) STRICT;
CREATE INDEX janitor_runs_store ON janitor_runs(store, at);
```

**Hot-path rules that stay outside the DB.**
- `epochs`, the `part → live (version, build)` map and the build manifests of live builds are cached in memory and invalidated from the events stream.
- Directory membership is cached and refreshed by the directory in use (the 15-minute claims re-read, or the Authentik adapter's 60 s poll; local groups change in place). A diff bumps `epochs['part:<id>']` for every part whose grants (or whose space's grants) mention an affected user or group. A failed poll or re-read changes nothing.
- Grant and public-link expiry is enforced by a timer that bumps the part epoch when the expiry passes. Without it, a cap minted before expiry would outlive the grant until its window ends.
- The boot epoch is bumped on every start and every import.

**Modelling decisions.**

| Behaviour | How the schema holds it |
| --- | --- |
| Space grants are inherited live | `grants.target_kind = 'space'`; effective role = max over the part's and its space's grants |
| Quotas "per agent" | keyed on `agent_connections.id` (`versions_actor_day`), because tokens rotate hourly. Storage per connection is approximated by the blobs its versions introduced |
| Block ids | `.pf/blocks.json` inside the manifest. `comments.anchor` and `leases.scope_ref` hold ids as text, with no FK. Re-anchoring is application logic |
| Invite "opened" | set by the `/i/<id>` wrapper (`invites.opened_at`), because the IdP has no open event |
| Invite "up to N people" | `invites.max_uses`/`uses`, counted here for every invite kind, because native invites have no IdP and Authentik has no use count |
| People from an IdP and local accounts | `users.auth_source`; IdP people keyed by `(idp_issuer, idp_sub)`, so changing IdP never collides; local accounts sign in with `passkeys.purpose = 'login'` |
| Groups from a claim, an IdP API, or partifactory | `groups.source`; local `pf-*` groups need no IdP at all |
| Data snapshots on by default | no `data_snapshot_policies` row = `daily`, keep 7; a row records only an explicit change |
| `provision` before-states | in `provision-log/*.jsonl` in the site repo, not in this schema, because `provision` runs in CI |
| `runtime_errors`: 1,000 per part, 30 days; `part_get` shows the latest 20 | a trigger for the count (inserts are rate-limited per part, ≤ 10/min), the janitor for the age |
| Public-link and guest uniques | in `insights.db`: a hash with an in-memory salt that rotates daily and is lost on restart, so uniques over-count on restart days. Accepted |
| Guests | no table: a guest is a `public_links` row in `contribute` mode plus the signed `g` claim; writes are labelled `guest:<link>:<g>` in `data_docs.updated_by` and `events.actor` |
| No approval queues | there is no `share_requests` table. People-only actions are done in the UI from a deep link; `access_asks` holds only person-to-person asks |
| Build method per version | `builds.method`, one build per target (`frontend`, `backend`) per version |
| Write-only secrets | `part_secrets.value_enc` is read only by the runner's deploy path; no op's output type has a field for it |
| Backend data vs part data | `data_docs` (snapshotted) vs `part_volumes` (folder backups and exports only) |
| Agent-made grants | `grants.created_by_conn` labels them and drives "Undo all" on a revoked connection |
| Dependency advisories | `build_deps` per build (from the mirror, the library set or the image SBOM) and `advisories` per build with a `state`; both cascade with their build, so a rollback keeps the old build's list exact. The advisory DB itself is a file under `/data/advisories/`, not a table |
| Every version kept | GC marks from `version_files`, `build_files`, `builds.log_hash`, `builds.sbom_hash`, `build_deps.blob_hash`, `drafts`, `data_snapshots`, `assets` and unpruned `backend_images.tar_hash` |

## 5f. Portability: what's core and what's an adapter

The admin doesn't want partifactory to depend on the particular stack it runs on today. So every coupling to it is an **adapter behind config**, and each concern has a **generic default with no vendor in it**.
The core never imports an adapter's types, adapters are chosen in `site.toml`, and a deployment that configures none of them is a complete product.

| Concern | Core contract | Default (no vendor) | Adapters | What breaks if absent |
| --- | --- | --- | --- | --- |
| **Ingress + TLS** | Two plain HTTP listeners; `Host`-based routing; client IP from one trusted header; optional request-id header (§3) | Any reverse proxy with a wildcard cert: Caddy, nginx or Traefik, configs in `deploy/proxy/` | Cloudflare Tunnel (`cloudflared`); built-in ACME DNS-01 (P5, for hosts with no proxy) | Nothing. Without a tunnel the host needs an inbound port |
| **Edge cache + edge rate limit** | Correct `Cache-Control` on every response; in-process `governor` limits | None: browsers cache shared blobs and cards `immutable` | Cloudflare cache rules + one rate-limit rule; any CDN that honours `Cache-Control` | Nothing for correctness. More origin traffic for shared blobs on new devices |
| **DNS + provisioning** | `DnsProvider` and `IdpProvisioner` plugin traits in `provision`; needs `parts.<domain>` + `*.<domain>` | `manual`: prints the records, cert names and OIDC redirect URI, then verifies them | Cloudflare (first DNS plugin, also tunnel, cache rules, Worker); Authentik (IdP plugin) | You create two DNS records and one OIDC client by hand, once |
| **People sign-in** | OIDC client from a discovery document; local passkey accounts (`webauthn-rs`) | Any OIDC provider; or none at all (local accounts only) | Authentik, configured by its plugin (cloned flows, implicit consent) | With no IdP: everyone is a local passkey account, and there's no SSO with other services |
| **Directory + groups** | `Directory` trait; grants on provider-neutral subjects | `groups` claim + 15-minute refresh-token re-read; local `pf-*` groups | Authentik API with a 60 s poll | Removals in the IdP take up to 15 minutes (or 12 h without refresh tokens) instead of about 1 |
| **Invites + accounts** | Invite tokens, use counts and grants in partifactory's tables (`/i/<id>`) | Native invite links; partifactory registers the passkey and creates a local account | Authentik provisioner: IdP invitations, recovery links, add-by-email, IdP-side `pf-*` groups | No add-by-email (partifactory sends no email); groups are partifactory-local |
| **Build + deploy** | OCI image by digest + `deploy/compose.yaml` + the env contract (`docs/env.md`) | `docker compose pull && docker compose up -d`, by hand or from cron | Example workflows: Gitea Actions (the example deployment), GitHub Actions | Nothing |
| **Secrets** | `PF_*` env vars, each also as `PF_*_FILE` | A mode-600 `.env` or compose secrets | Any CI or secret store that sets env or mounts files | Nothing |
| **Host** | An OCI runtime with compose; one `/data` volume | Any Docker or Podman host | TrueNAS (the example deployment) | Nothing. partifactory calls no host API |
| **Backups** | `/data` is everything; `export`/`import` to a folder | Scheduled exports to `backup.export_dir` | Host snapshots (ZFS on TrueNAS, restic, rsync); S3-compatible upload (P5) | Nothing |
| **Events for agents** | The event log: long-poll, SSE, HMAC-signed webhooks (§4) | Webhooks to configured URLs | Hermes is a webhook consumer and an MCP client | Nothing |
| **Ops alerts** | One optional webhook (`[alerts] webhook_url`), off by default; the Health page always shows the same conditions | None: look at Health | ntfy, Hermes or anything else that accepts a JSON POST | You find out from the Health page instead |
| **Maintenance page + uptime** | Static page in `deploy/proxy/`; `/healthz` | The proxy serves the page; any external probe | Cloudflare Worker fallback and health checks | A raw proxy error during an outage |
| **Embedding** | `frame-ancestors` from `[embed] frame_ancestors`; `pf-embed.js` | Empty: only the part's own host and the workspace may frame it | The family hub is one entry in the list | Nothing; other sites just can't frame parts |
| **Hostnames** | `[domain] apex, workspace_host, part_suffix`, used everywhere | Required config, no default | The example deployment's domain lives in site config | — |
| **Device-connect origin** | Optional country + network from proxy headers or a local GeoIP file | Omitted, shown as "origin unknown" | Cloudflare country header | The "different network" phishing banner can't fire; the request-age banner still does |
| **Backends** | The runner's fixed API; containers from one template; the Docker Engine API (§3f) | Docker or Podman (its Docker-compatible API) on the same host | gVisor (`runsc`) as an optional runtime | With no runner, backend parts fail with `backends_unavailable`; frontends are unaffected |
| **Advisory data** | `AdvisorySource` trait: OSV-format records for the ecosystems in use, matched against `build_deps` and stored SBOMs (§3h) | The OSV bulk dumps, downloaded daily, synced incrementally and matched locally | The OSV API (`querybatch`; sends package names and versions out) | With `source = "none"`: no advisories, and a warning on Health and in `doctor`. A stale DB turns Health amber; builds never block |
| **Telemetry export** | `insights.db`, `/metrics`, JSON logs on stdout (§5g) | None needed: Insights in the UI, a Prometheus scrape, `docker logs` or any host collector | OTLP (traces, logs, metrics) to Tempo, Jaeger, Grafana or any collector; syslog; HTTP JSON log push | Nothing |

**Proving it: the portability lane.** A CI job runs the **full e2e suite** (the §4a.7 click budgets, the §7 checks that don't name an adapter, cross-surface conformance and an export/import round trip) with **no Cloudflare, no Authentik and no Gitea**:
- **Dex** as the OIDC provider (static users and groups, refresh tokens on), with a second pass using no IdP at all (local passkey accounts only, admin via `partifactory admin bootstrap`);
- **Caddy** in front with a locally issued wildcard cert for a test domain, using `X-Forwarded-For` as the client-IP header;
- **local exports** to a folder, then `import` into a fresh container;
- **Podman** instead of Docker for the backends pass (the runner speaks the Docker Engine API, which Podman implements), including both pixel-editor tests;
- run from the published compose file with secrets as `_FILE` mounts, on a stock CI runner (the workflow is written for GitHub Actions, the same steps a shell script runs locally).
It gates every release, so a hidden dependency on the example deployment's stack fails CI the day it's introduced.

**What stays coupled, on purpose** (and why):
- **One registrable domain** for the workspace and all part hosts. The `/grant` flow and the wrapper's chrome API rely on the session cookie being same-site with every part host. That's a property of DNS, not of any vendor.
- **WebAuthn passkeys** for step-up and local accounts. They're a W3C standard implemented in the binary, and passkey-only sign-in is what makes phishing via parts inert (§3).
- **OCI images + compose** as the deployment unit. Both are open standards with several implementations, and the alternative (OS packages per distro) is more coupling, not less.
- **The mirror allowlist's default CDNs** (cdnjs, jsDelivr `/npm/`, unpkg, esm.sh, Google Fonts). They're what Claude-written pages reference, so they're a compatibility target, not infrastructure; the list is config.
- **The Telegram in-app browser in the human checks.** It's where the household opens links, so a real phone checks it (non-blocking, decision 52); nothing in partifactory talks to Telegram.

## 5g. Insights: analytics and observability, in the workspace (admin-only)

Insights is where the admin sees what's used, what agents do, and why something broke, without leaving partifactory.
**Rules, all on by default**: self-hosted, no third parties; **no part content in logs or analytics**, only ids, op names, argument keys, sizes, statuses and timings; IPs truncated and hashed with a rotating salt; raw events kept 30 days and daily rollups indefinitely; admin-only; an optional OTLP export adapter (to Tempo, Jaeger, Grafana or any collector), never required.
**Storage**: a separate SQLite file, `/data/insights.db`, with its own writer, so analytics load never contends with the main writer (§5e). It's in exports only with `--include-insights`.
- *Why a second file:* a busy day of views and traces is many small writes. Keeping them off `partifactory.db` keeps the save path's latency where §5a says it is, and losing `insights.db` loses nothing anyone needs to restore.

**Pages** (Stockroom style; charts follow the dataviz rules, each with a table view, light and dark; every chart exports as CSV or JSON):

| Page | Shows |
| --- | --- |
| **Overview** | parts created and edited per day; active parts; views; active people; guests; agents active; errors; build success rate; disk use against the budget; backup layer status |
| **Agents** | per connection: calls over time by op and by surface (MCP, HTTP, CLI); the error codes it hits most; `part_check` pass rate; median time from create to first good build; kinds and **build methods** it uses over time (server build vs prebuilt files vs images; frontend-only vs backend); deep links handed out (`part_link_for`) and how many were acted on |
| **Parts** | kinds over time; build methods over time; share of parts with a backend; top parts by views and edits; stale parts; largest parts; data-snapshot use; public links (by mode) and their opens; guest writes; open advisories by layer and severity in context, and how long they stay open (§3h) |
| **People** | who's active, views per person, invites sent and accepted, sign-in failures. Guests only in aggregate |
| **Builds** | queue and duration percentiles; failures by error class; toolchain versions; BuildKit cache hit rate |
| **Backends** | running containers; cold starts; restarts; memory and CPU per part; egress bytes; denied LAN attempts |

**Debugging.**
- **Every request gets a request id**, shown on error pages and in API errors, and propagated to builds, backends (`X-PF-Request-Id`) and `/_rt`.
- **Traces**: `tracing` spans go to an in-memory ring buffer, and are persisted with **tail sampling**: every error, every request over the slow threshold (500 ms by default), and 1 % of the rest, kept 7 days. The UI draws a **trace waterfall** for any kept request.
- **The per-part Debug tab** (the admin): recent requests (status, latency, surface, actor), builds with their logs, browser runtime errors (with version), backend logs (tail and search), events, and **the cap and grant decisions that denied access**, so "why did they get a 403?" has an answer.
- **The per-agent connection page**: the last 100 calls with their request and response shapes (op, argument keys and sizes, never values), errors and latency.
- **The log viewer**: structured JSON logs, filtered by level, part, agent or request id, with live tail and "download range".

**Metrics**: Prometheus `/metrics` on the ops listener behind the admin metrics token, alongside the UI: RED metrics per route and per op, build and backend gauges, and each store's size against its cap.
**Tested**: a fuzz test in CI fills parts, data, comments and titles with canary values across every surface, and fails if any canary shows up in logs, traces, `insights.db` or `/metrics`.

## 5h. Disk safety: every growing store is capped, by default

**The principle: every growing store has a default cap, a retention rule, a rotation mechanism, an export path and a metric.** They're on by default with no config. Config can only tighten or loosen them within sane bounds, and `doctor` fails if any store is uncapped.
- *Why:* the way a self-hosted service dies is a full disk, months after anyone last looked. A cap on everything turns that into a Health row and an alert at 80 %.

**The global disk budget**: partifactory may use at most 80 % of its volume by default (`[disk] budget_percent`), counting `/data` plus the runner's images and build cache.
It warns at 80 % of the budget, and at 95 % **refuses new writes** (versions, assets, builds, deploys, `/_rt` writes) with `storage_full`. It **never refuses reads or deletes**, so a person can always free space.

| Store | Default cap / retention | Rotation | Export |
| --- | --- | --- | --- |
| App logs (structured JSON) | 7 days or 1 GiB in total, whichever comes first | size + daily rotation, zstd-compressed segments | the log viewer's "download range"; `partifactory logs export --since`; stdout always (so `docker logs` and host collectors work); optional OTLP, syslog or HTTP JSON push |
| Container stdout from the Docker daemon | `json-file` with `max-size=10m`, `max-file=3`, set by the runner on every container and in partifactory's own compose file | daemon-side | the part's Debug tab; `logs export --part` |
| Backend logs (captured per part) | 50 MiB per part, 7 days | ring segments | Debug tab download; `logs export --part` |
| Build logs | the last 20 builds per part, 30 days | prune job | attached to the build record; export |
| Browser runtime errors | 1,000 per part, 30 days | prune job | export |
| Traces and spans | 7 days, 2 GiB, tail-sampled | prune job | OTLP adapter; trace download (JSON) |
| Insights raw events | 30 days | prune job | CSV/JSON export from Insights |
| Insights daily rollups | indefinitely (tiny) | — | CSV/JSON export |
| Audit log | 1 year (security-relevant) | yearly segments | export (JSONL) |
| Events log (agent feed) | 30 days, then `cursor_expired` | prune job | — |
| Versions and blobs | every version kept (deduplicated); the bin for 30 days; unreferenced blobs collected | GC job | `export` |
| Data snapshots | daily keep 7 / hourly keep 48; the last one kept after 90 days idle | prune job | export |
| Previews (posters and `og:video` clips, §4c) | the newest per part; 2 GiB in total, new previews stop at the cap | replaced on each new live version; GC | export (part blobs) |
| BuildKit cache | 10 GiB, LRU | `buildkit prune` on a schedule | — |
| Advisory DB (§3h) | the latest sync only, under a cap sized from the advisories spike | replaced by each sync | — (downloaded again) |
| Stored image SBOMs (§3h) | kept while their build exists, under a per-build cap (5 MiB by default) | pruned with the build | the build record |
| `advisories` rows (§3h) | deleted with their build | with the build | `part_get`; export |
| Images (and uploaded tarballs) | the images of live + the previous 2 versions per part; the rest pruned weekly | runner prune | — |
| Backend volumes | per-part quota (default 1 GiB) | — | export |
| Scheduled exports | keep 7 | rotation | they *are* the export |
| SQLite WAL (both files) | a scheduled checkpoint + `wal_autocheckpoint`; `VACUUM` monthly when free pages > 20 % | — | — |

**The janitor**: one task runs every prune, GC, rotation and checkpoint job on a schedule. It records what it freed (`janitor_runs`, Insights and metrics), and **never deletes anything a live or binned item references**.
**Logs out**: JSON on stdout is always on, so any host log collector works; optional push adapters (OTLP logs, syslog, HTTP JSON); `partifactory logs export|tail` on the CLI; downloads in the UI. Content is never logged (the no-content fuzz test, §5g).
**Health**: each store's size against its cap is on the Health page and in Insights, and the ops webhook (§4) alerts at 80 % of any cap.

**Safe defaults, enforced** (the principle at the top of the plan): secure headers, rate limits, quotas, the container hardening template, the nightly export (to `/data/exports`, keep 7, with "same disk = amber"), the ingress contract's TLS assumptions, log redaction and these retention caps are all on in the default config.
Turning any of them off needs an explicit key (`[unsafe] <name> = "off"`), and shows as a warning on Health and in `doctor`.
- *Why a separate `[unsafe]` table:* a protection should be impossible to switch off by a typo or a copied snippet. One table that reads as what it is makes every exception visible in review.

## 6. Phases

Each phase ends with something usable and has its own "Done when". Slice 1 (top of the plan) is the first cut through P0–P2 and ships before any phase is complete. The order puts the riskiest technology first (sandbox, compile), then the daily loop (agents), then backends, and invites last among the core, because the household already exists in the example deployment's IdP. Adapters for the example deployment (Cloudflare, Authentik) land in the same phase as the generic default they sit beside, never before it.
- *Why agents before invites:* the first moment of daily value is "Hermes makes a thing from Telegram and I open it on my phone". Invites serve people who aren't users yet, and they carry the most identity work (local accounts, and the Authentik provisioner for the example deployment).
- *Why backends (P2a) right after agents:* a backend is only useful once an agent can make and change one in a single call, and the guest and shared-data work of P2 is exactly what its acceptance test builds on.
- *Why the sandbox and TSX in P0:* the serving model and the compiler are the two things most likely to force a redesign. Finding a rolldown or Safari problem in P3 would be finding it last.
  Slice 1 keeps the compiler spike and runs the `tsx` kind itself right after Slice 1 (decision 39, provisional), so P0's "Done when" is met just after the slice ships.

**P0 Core (local).** Repo, schema + migrations, blob store (temp → fsync → rename), source manifests + build records, the build child process, HTML/Markdown/text/SVG/code **and TSX** kinds with the golden corpus,
mirror-on-build, the content listener with **path caps and the canonical sandbox CSP**, the prelude with the `localStorage` shim and `runtime_errors`, the operation registry, the HTTPS API with `/openapi.json`, the `part` CLI (dev token),
logs + metrics, and a same-machine regression bench. Spikes: rolldown/oxc, rmcp, Leptos vs templates, **the sandbox on every browser engine, emulated and simulated** (§5), including the wrapper flow, and the Authentik adapter spike (conditional UI, blueprint-over-API, `groups` claim, membership by API, recovery-link permission).
*Done when:* `part put x.html --name packing` opens at `packing-part.localhost:8081/c/<cap>/` under the sandbox, a Claude-written page using `localStorage` and a CDN `<script>` runs unchanged there, and the TSX corpus compiles.

**P1 Edge + identity.** The ingress contract with example proxy configs, `provision` with the manual and Cloudflare providers (the example deployment's tunnel and wildcard), generic OIDC login + the claims re-read directory, the Authentik directory adapter, `/grant` → the part host,
the **part-host wrapper** with its chrome API and CORS, policy engine + audience ladder + spaces + the role × runtime matrix, admin pin + native passkey step-up,
the Stockroom workspace (gallery, search, history, diff, preview old versions, restore, bin, share sheet, Move to Family), admin-only public links on the part host with open counts, tier (a) cards, previews (posters and `og:video` for public links and the gallery, decision 53),
rate limits, quotas, health page with optional ops alerts, backups (consistent DB copy, `export`/`import`, the nightly export on by default, per-part data snapshots on by default), the first drill on a second OCI host, the image + compose + env contract with an example CI workflow, `doctor`,
the Insights store with request ids, traces and the log pipeline (§5g), the janitor, the disk budget and the retention test (§5h), the no-content-in-logs fuzz test,
the **portability lane** (full e2e with Dex, Caddy and local exports, §5f), and the **nginx/HAProxy comparison on the real content path**.
The simulated device matrix (decision 52). The real-device pass, including the 5-minute Telegram in-app-browser passkey test (§2), is a human check that blocks nothing.
*Done when:* The admin opens a part on their phone at its own host link via SSO; the portability lane is green; a public link opens from mobile data with no login and unfurls as a card; the security checklist (§7) passes from outside; and the drill restores everything on a second OCI host in < 15 min.

**P2 Agents + the household loop.** OAuth AS (device grant first, then auth code + PKCE + CIMD/DCR + manual clients), one connection per (agent, person) and service identities,
MCP + discovery + the error contract + cross-surface conformance, `part_edit` with rebase, block ids, leases, notes, `part_get` full content and `render: "text"`, comments with provenance,
the event log (long-poll, SSE, webhooks), live reload, inward agent sharing (`share:household`) and `part_link_for` deep links for people-only actions, **`part_check` + the render sidecar (the P2 headline)**, Jig, the source editor with drafts and the formatted view for Markdown and `text`, renames,
shared data over `/_rt` with live `data.changed`, "Can contribute" public links for guests, the Insights pages and debugging UI, and the Hermes skill with per-person pairing.
*Done when:* a household member pairs with Hermes, says "make a packing list and share it with the family" in Telegram, Hermes checks its own render and posts the part's link, another family member opens it on their phone, comments, and watches the fix land;
and Claude Code connects with one passkey tap + Allow and edits a week-old part in one call;
and the named e2e test **`communal_pixel_editor`** passes in CI:
1. an agent is asked to make a communal pixel editor;
2. it creates it in one call, `part_check`s it, and posts `pixel-part.<domain>`;
3. The admin: Share → Anyone with the link → Can contribute → Copy;
4. a signed-out phone paints, and the admin's screen updates live;
5. The admin rolls back one snapshot and freezes it.

**P2a Backends.** The runner sidecar (Docker Engine API; Docker or Podman) with the hardened template and a network per part (egress on; LAN, host and other parts blocked), the compose subset, server builds with rootless BuildKit, registry and uploaded images, a container set per live version gated by its health check,
scale to zero or always on, routing under the cap with the signed identity header (WebSocket and SSE), per-part volumes in exports, write-only secrets, the LAN allowlist (people only), `backends:deploy`, quotas, backend observability in Insights, runner retention, backend SBOMs and base-image digest checks (§3h), and the §7 Backends suite.
*Done when:* the named e2e test **`communal_pixel_editor_backend`** passes on Docker and on Podman: the agent adds a leaderboard service on Postgres to the pixel board in one `part_edit`; the server builds it; each guest's pixel count shows on the leaderboard live; restoring the previous version rolls the backend back too; and the backend reaches the internet but not the LAN.

**P2b Invites.** First the **default**: partifactory invite tokens (`/i/<id>`, counted uses), passkey invite links (single and "up to N") that create **local passkey accounts**, passkey sign-in for local accounts, "new passkey link" recovery, local `pf-*` groups, linking a local account to an IdP identity, pending-invite cleanup.
Then the **Authentik provisioner**, starting with its permission gate (§3a), which decides how much of it ships: the `partifactory-invite` flow, add-by-email, IdP recovery links and IdP-side `pf-*` groups.
*Done when:* The admin mints an invite link from the share sheet with no provisioner configured, a new person creates a passkey and lands on the part, and removing them locks them out at once;
and, with the Authentik provisioner on, an invited email address enrols the same way and removing them in the IdP locks them out within a minute.

**P3 Claude compatibility.** `claude.use` (`db`, `user`, `comments`, `artifact` incl. self-publish; shared `window.storage` arrived in P2), Tailwind + shadcn (mechanism chosen on evidence: prebuilt utility CSS, the Tailwind v4 browser build mirrored as a shared blob, or build-time class extraction),
viewer uploads (assets), and the agent inbox with stewards. The spec is shartifacts' compatibility matrix (credited).
*Done when:* a page copied from a claude.ai artifact runs unchanged, including state persistence and self-editing, and a viewer's photo upload reaches the agent's inbox.

**P4 Family + hub + phone.** Family onboarding, embedding via `[embed] frame_ancestors` (the family hub, with its service identity), the Hermes `hub-family` profile, slides + Mermaid, the optional OTLP export adapter (screenshot thumbnails and preview cards moved right after Slice 1, decision 53),
and tap-to-edit text and point-and-comment on phones. (Notifications for people were cut in round 2b, and approval queues in round 2c, §4.)
*Done when:* a family member opens a hub-embedded part, fixes a typo by tapping it on the phone, long-presses to ask Hermes for a change, and Hermes, watching the event log, tells them in its own chat when the fix lands.

**P5 Later, only if needed.** WASM component backends as an optional alternative to containers, through the same grant model and routing; optional S3-compatible upload of scheduled exports; semantic search alongside FTS5; built-in ACME DNS-01 for hosts with no proxy.
*Done when* (per item): WASM backends need an ADR naming a part that containers serve badly, and ship with the §7 Backends lines green; the S3 upload restores via `import --from s3://…` in the drill; semantic search beats FTS5 alone on a labelled query set; built-in ACME serves the portability lane with no proxy in front.

## 6a. Build order

The tasks live in beads, not in this plan: `br ready` lists what can start, and each bead carries its own acceptance test. Beads are created only at conversion, never during a planning or polish pass.
This section is the order the pieces depend on each other, and why. Read each arrow as "comes before".

**The spine: Slice 1.**
- **The Cargo workspace and CI → the SQLite layer → schema v1 → blobs, manifests and versions.** Everything else stores into these. *Why first:* the schema is the contract every later piece writes against.
- **Spikes in the first week.** The sandbox spike runs before the content listener starts, because items 10, 12 and 13 decide the wrapper flow (§5). The compiler, rmcp and Leptos spikes run beside it.
- **Kind renderers → the build worker → mirror-on-build.** The content listener can only serve what a build produced, and mirroring runs in the parent just before the child (§3d).
- **Capabilities → the content listener → the shared blob host → the prelude and `/_rt`.** The prelude is a shared blob, so the blob host comes first. *Why start these first:* the pixel test, the retention chain and conformance all run through them, so they're the bottleneck.
- **Data snapshots → shared data over `/_rt` → Can-contribute links → `communal_pixel_editor` → Slice 1.** The longest chain. Snapshots come before shared data because the snapshot ops live in `part_data`, and before contribute links because switching a link takes one. The share sheet's public rung feeds the contribute links too.
- **The janitor → the disk budget → the retention test.** As long as the pixel test's chain, and in the slice on purpose: it sets Slice 1's date (decision 41). Start the janitor as soon as snapshots land.
- **The registry → `part_edit`, the read path and `part_check` → MCP → discovery and `/skill.md` → cross-surface conformance.** The third chain of the same length.
- **Identity, in parallel once the schema lands:** generic OIDC with the e2e harness → the claims re-read directory → the policy engine → `/grant` → the part-host wrapper and its chrome API → the share sheet's public rung. The admin pin and step-up sit beside `/grant`.
- **Agent connections:** the token service (after OIDC, the policy engine and the admin pin) → the device grant (personal tokens shipped here and were retired by decision 55).
- **Shipping:** logs and metrics → Health; backups → import and the drill; the ingress contract → the deliverable → the §7 suite and the simulated device matrix. The human checks on real phones hang off nothing and block nothing.
- **`build_deps`** *(provisional, §3h)* rides on mirror-on-build and needs nothing else.

**Right after Slice 1.**
- **`tsx`** (oxc + rolldown, the library set). It needs the build worker, mirroring and the blob host, and it closes P0, so P0's "Done when" is met just after the slice ships (decision 39).
- **The rest of P1:** the titled card pipeline → previews (posters and `og:video` for public links and the gallery; they need the render sidecar and the titled card); search → the workspace extras (versions rail, bin, diff, inward rungs, Move to Family, Ask for access); `provision` → the Cloudflare and Authentik adapters; `doctor` after the disk budget, because it checks that every store is capped; the Insights store → traces and the log pipeline → the no-content fuzz test; the ops-alert webhook after Health and the disk budget; the deliverable extras → the portability lane; the bench → the nginx/HAProxy comparison.
- **The rest of P2:** OAuth authorization code; comments, leases, block ids, notes and `part_find`; `part_watch` and webhooks → the source editor → the formatted view; agent sharing → the Hermes skill; Jig; live reload; renames; the Insights pages → the debugging UI; the remaining deep links.
- **Advisories** (§3h): the spike → matching and its surfaces → `update` and the Update libraries button → auto-update.
- Some pieces plug in later without anything waiting on them: `doctor` adds the provisioner probe once the Authentik gate has run, and the Insights store and the log pipeline register their janitor jobs and retention-test rows when they land.
- Adapters land in the same phase as the generic default they sit beside, never before it.

**Later phases.**
- **P2a Backends:** the runner (after the deliverable) → backend builds → lifecycle → routing (after Can-contribute links and request ids) and data, secrets and permissions → the backend security suite and `communal_pixel_editor_backend`. Backend SBOMs and base-image checks come with the runner. The runner lane is shorter than the routing chain, so it can start as soon as the deliverable lands, beside the backend Insights pages.
- **P2b Invites:** native invites and local accounts → the Authentik provisioner gate → Authentik invites → Authentik extras. Cleanup lists follow native invites.
- **P3:** `claude.use` (after shared data, comments and `/grant`), Tailwind and shadcn (after `tsx`), viewer uploads, the agent inbox (after comments and the event log).
- **P4:** family onboarding (after native invites), family-hub embedding and service identities, slides and Mermaid (after block ids), phone editing, OTLP. **P5:** each item on its own, after what it extends.

## 7. Security checklist (run from outside the network, in CI against staging)

Every line runs against the example deployment's staging (Cloudflare + Authentik). Lines not marked *(adapter)* also run in the portability lane (Dex, Caddy, no vendor, §5f), so no protection depends on an adapter.

**Part hosts and the sandbox**
- Every part document response carries the canonical CSP (§2), including `sandbox` without `allow-same-origin`. Only `unsandboxed` parts lack it, and the admin page lists them.
- Part hosts never emit `Set-Cookie`. A request carrying an old `__Host-pf_cap` cookie is treated exactly like one without. In the e2e suite on Chromium, WebKit and Gecko, `document.cookie` and `cookieStore.set` writes from a part have no effect, and `localStorage` is the shim.
- A part's fetch/POST toward another `<domain>` service, or toward the workspace, carries no `SameSite=Lax` cookies (tested against a canary service).
- `/_rt/v1` rejects a missing cap, a cap bound to another host, an expired window, a stale epoch, and any non-null `Origin` (other than the part's own for `unsandboxed`). A sibling part's page can't read or write another part's data. No `OPTIONS` preflight is ever needed.
- Part responses to `Origin: null` carry `Access-Control-Allow-Origin: null` and never `Access-Control-Allow-Credentials` or `Cross-Origin-Resource-Policy`. The workspace host never emits `Access-Control-Allow-Origin: null`.
- Caps are host-bound (a cap for A is rejected on B), deterministic within a window, and dead after an epoch bump, a boot-epoch bump or an import. An unknown `kid` or bad MAC → 404.
- `/grant` can't redirect off the part's own host (fuzzed), and in `wrap` mode the cap appears only in the fragment. Without a session, `/grant` → sign-in, never content.
- **The wrapper**: the part host root is served without `sandbox` but with its own strict CSP (scripts only from the shared blob host, `connect-src` only the workspace, `frame-src 'self'`, `frame-ancestors` from config). It never sets a cookie, removes `#pf=` before any other script runs, and holds only its own part's cap.
  Content framed inside it can't read it (opaque origin: a test script in the part tries `parent.document` and `parent.sessionStorage` and fails).
- **Chrome-API CORS**: `Access-Control-Allow-Origin` + `Allow-Credentials` are sent only for an active, sandboxed part's own host, and only on that part's chrome routes. `Origin: null`, a sibling part's origin, an unsandboxed part's origin, `blobs-part`, an old redirecting name and any other origin get no CORS headers (fuzzed). Writes need `Sec-Fetch-Site: same-site`.
- `Referrer-Policy: no-referrer` on every part response. Logs never contain `/c/<cap>/` segments (canary test).

**Caching and blobs**
- Part blobs always carry `Cache-Control: private` and the shared blob host serves only hashes in `shared_blobs`. *(adapter)* Private part blobs are never in Cloudflare's cache (`cf-cache-status` ≠ `HIT`).
- Shared blobs carry `public, max-age=31536000, immutable` and never `Set-Cookie`. *(adapter)* A shared blob on `blobs-part/b/<hash>` shows `cf-cache-status: HIT` on the second fetch (the cache rule works).

**Links and cards**
- A part host root fetched by a signed-out client returns the content-free card (or the public card while a public link is active) and no part bytes. Bot and browser User-Agents get byte-identical responses for the root and `/l/`. Neither page's HTML contains a cap or a hop-target URL.
- `/p/<name>` on the workspace only redirects to the part host, and reveals nothing else.
- A private card's input struct has no part-derived field (unit test).
- Only the admin's session can create a public link. No agent scope, token or op can (tested on every surface). A revoked public link → 404 within seconds. The kill switch revokes all.

**Identity and admin**
- Only the admin's pinned subject reaches admin. An IdP-admin user still gets 403. An admin action without a fresh (10 min) partifactory passkey assertion → `step_up_required`. Household sharing never asks.
- *(adapter)* The Authentik provisioner gets 403 from the IdP on a non-`pf-*` group, and the core refuses before calling. Gate results are recorded: no non-`pf-*` membership, no `is_superuser` groups, no flow or stage edits, no privileged parent groups.
- With the IdP down, `/readyz` stays 200, existing sessions work, and no epoch is bumped. A refresh that returns `invalid_grant` ends that session.
- Local accounts: passkey-only (no password path exists), a recovery link revokes the old passkeys, and an invite token beyond its use count is refused.

**Agents**
- Agent tokens: audience-bound to the workspace origin (rejected on part hosts and on `/oauth/*`), scope- and space-limited, no admin scopes obtainable, refresh reuse → the whole token family is revoked.
- An agent without `share:household` can't grant. With it, it can grant only to existing people and groups, only within its spaces, and every such grant is labelled and listed in the share sheet and activity feed.
- No agent scope, token or op can make a public link, an invite, a group, an `everyone-signed-in` grant or a LAN allowlist entry, on any surface. `part_link_for` returns a link that grants nothing by itself: opened by someone without the right, it shows the normal 403. There is no request table for an agent to write to (schema test).
- Unconsented OAuth clients can do nothing. Registration is rate-limited. A CIMD URL resolving to a private address is refused. The consent sheet shows the redirect host.
- Device codes: single use, 10-minute expiry, `slow_down` enforced, manual entry limited to 5 per minute; the stale and different-network banners appear when they should.
- Agent access tokens are redacted in logs, scope- and space-limited, and revoked by the connection cascade; retired `pf_pat_` tokens never resolve.
- Authentication failures are HTTP 401 with `WWW-Authenticate` on `/mcp` and `/api`, never a tool result.
- Discovery documents are reachable unauthenticated and contain no site secrets or user data.

**Guests ("Can contribute" links)**
- A guest cap calling any non-data `/_rt` op (`comments.*`, `artifact.publish`, `assets.put`, `user`, `errors.report`, `refresh`) gets `403`, and so does any `/api` or `/mcp` request from a guest's context (they hold no token).
- A Can-view link's cap can't write shared data. A forged or replayed guest id (`g`) fails the cap's MAC.
- Guest writes past the per-guest or per-IP limit get `429`; past the part's shared-data cap, `quota_exceeded`. After **Freeze**, an open guest tab can't write on its next request.
- Guests never see history, source or the share sheet.

**Backends**
- **Container escape attempts** from a hostile test image fail: mounts, `unshare`, raw sockets, kernel modules, writes to the root filesystem, setuid binaries (`no-new-privileges`), host paths, the Engine API socket. A fork bomb hits the pids limit and a memory blow-up kills only that container.
- **LAN reachability from a backend must fail**: every RFC 1918, CGNAT, link-local and ULA range, the household's real subnets (from site config), the host's addresses, `169.254.169.254` and every other part's network. An allowlisted entry is reachable and nothing else is. Denied attempts are counted. The same holds during a BuildKit build.
- **Egress is on by default**: a request to a public canary succeeds from a backend and from a build.
- **Identity-header forgery**: a request carrying its own `X-PF-Identity` or any `X-PF-*` header reaches the backend with them replaced; a JWT signed with another part's key is rejected by the example backend; a request without a valid cap never reaches a backend. Backends never see a `Cookie`, a cap or a `pf_` token (canary).
- The compose parser refuses every unsupported key, an image from a registry outside the allowlist fails with `image_not_allowed`, and the runner refuses any request outside its template.
- **Secrets are write-only**: no op, surface or screen returns a value, including to the agent that set it; values never appear in versions, exports (without `--include-secrets --encrypt`), logs, traces or Insights (canary).
- Every line runs on Docker and on Podman.

**Updates and advisories** (§3h)
- An advisory DB more than 7 days stale turns Health amber, and a build with the DB stale or missing still succeeds (fail open).
- With `source = "osv-api"`, the outbound requests carry only ecosystem, package name and version, never part content, names, titles or URLs (canary test). With the default `osv-bulk`, nothing about parts leaves the box.
- An advisory sync or match never changes a live part's import map or image digest; only `part_edit {update}`, Update libraries or opt-in auto-update makes a new version, and auto-update is recorded as `system`.

**Render sidecar**
- The renderer has no route off the internal Docker network (an egress canary to an external host fails). Renderer caps can't persist `/_rt` writes, including during a preview recording.
- A private part's root never carries `og:video` or a content poster. `/og/<hash>` on a part host answers only while that part has an active public link with that preview on: otherwise, and at once after revoke, it's a 404 byte-identical to an unknown path. A link set to `card` never gets a preview. An agent-supplied MP4 with an audio track or a codec other than H.264 fails its build with `preview_invalid`. The gallery's clip endpoint refuses a viewer who can't open the part.

**Operations**
- The origin is reachable only through the configured ingress (a direct connection to the listeners from outside fails). `:9090` is never routed. A client-IP header from an address outside `trusted_proxies` is ignored.
- No tokens, caps, cookies, secrets or part content in logs, traces, `insights.db` or `/metrics` (fuzzed with canary values across every surface). `/metrics` refuses a request without the admin metrics token.
- Rate limits trip at the configured thresholds, and quotas stop a looping agent. At 95 % of the disk budget, writes get `storage_full` while reads and deletes continue.
- Every growing store has a cap (`doctor` fails otherwise), and each one, filled past its cap in a test, comes back under. A safe default switched off without its `[unsafe]` key fails `config check`.
- A parts-only export contains no grants, tokens, sessions or people (schema test).
- The simulated device matrix in the gate; a non-blocking real-device pass (iPhone Safari, Android Chrome, the Telegram in-app browser) for: standalone open, part-host open (first open and reload), embedding, invite acceptance, sign-in.

## 8. Decisions

**2026-10-22**
1. **Hostnames**: `parts.<domain>` + `<name>-part.<domain>` via a `*.<domain>` wildcard, with semantic names. The domain and the part suffix are config, kept in site config (round 2b).
2. **Stack**: Rust. Bend 2 considered and set aside.
3. **Host**: any OCI host with compose; TrueNAS in the example deployment. The app knows nothing about TrueNAS/ZFS.
4. **Code reuse**: fresh, with shartifacts as the reference spec, credited.
5. **Docker**: for deploying partifactory only. Parts need no builds. *(Superseded in round 2c: parts may bring containers, and the server builds them; decisions 28 and 29.)*
6. **Tunnel**: the example deployment uses its own Cloudflare tunnel `partifactory` (created 2026-10-22, routes configured, no DNS records yet). Since round 2b the tunnel is one ingress adapter; the default is any reverse proxy (decision 22).
7. **Identity**: OIDC, swappable; Authentik in the example deployment. In-app grants and invites. Admin pinned to one subject with native passkey step-up. Round 2b makes generic OIDC the baseline and Authentik an optional extra (decision 23).
8. **Agents**: OAuth 2.1 per the MCP spec, with partifactory as a federating authorization server. Per-agent consented, scoped, audience-bound tokens.
9. **Source of truth**: private Gitea repo + passive public GitHub mirror. Site config in a separate private repo. Round 2b: secrets come from env vars or files, from any CI or none; Gitea Actions is one example workflow (decision 22).

**2026-10-23 (round 2)**

10. **No cookies on part hosts; every part is sandboxed.** CSP `sandbox` without `allow-same-origin` gives each part document an opaque origin, and caps live in the path. No separate registrable domain: the sandbox removes the reason to buy one. Per-part hostnames stay (§2).
11. **Any AI, any surface.** One operation registry in Rust, rendered as MCP, an HTTPS JSON API with OpenAPI 3.1, the `part` CLI and a skill file. Discovery via `/llms.txt`, `/openapi.json`, `.well-known` metadata, Markdown on `GET /` and a served (not published) `server.json`. No A2A card.
12. **One link to post** (round 2b: the part's own host, decision 25). No agent-minted preview links. Public links are admin-only, made once the part has a live version, with no expiry by default, open counts and revocation.
13. **Agent sharing: inward yes, outward no.** `share:household` lets agents share with people who already have accounts. Public links, invites and groups are people-only (round 2c: no requests; the agent hands over a deep link, decision 34).
14. **One connection per (agent, person)**, labelled "Hermes for <person>", plus service identities. Device grant is the default for headless agents. (Personal agent tokens were retired by decision 55.)
15. **Spaces are first-class**: a personal space per person + `family`. Space grants are inherited live. "Move to Family" is the household share.
16. **Previews**: tier (a) branded cards in the binary, always, decided by audience; tier (b) an optional render sidecar whose headline is `part_check`, a P2 feature.
17. **Backups = the `/data` folder** plus `partifactory export`/`import`. No git archive, no Litestream, no streaming sink on the write path. Per-part data snapshots are on by default since round 2b (decision 26).
18. **Search** is FTS5 over readable text, with bm25 + recency. **Every version is kept**, with a 30-day bin for deleted parts.
19. **Stockroom** is the workspace theme (approved). **Jig** is the separate, optional design system for parts.
20. **Builds run in a child process** of the same binary, with a deadline and a memory cap.
21. **Phase order**: sandbox + TSX in P0, agents (P2) before invites (P2b), the nginx/HAProxy comparison after P1.

**2026-10-23 (round 2b)**

22. **No hard dependency on the current stack.** Cloudflare, Authentik, Gitea, TrueNAS, Telegram/Hermes and the family hub are each an adapter behind config with a vendor-free default: any reverse proxy for ingress (edge caching optional), `provision` provider plugins with a manual mode, an OCI image + compose file + env contract for deploys, a generic webhook for events and alerts, `frame-ancestors` from config. §5f has the table.
23. **Generic OIDC is the identity baseline**, with a claims re-read directory. Authentik's directory poll and provisioner are optional extras. **Native invites with local passkey accounts are in scope for v1 (P2b)**, so inviting people never needs a provisioner, and an IdP-less deployment works.
24. **A portability lane gates every release**: the full e2e suite with Dex (and with no IdP), Caddy and local exports, and no Cloudflare, Authentik or Gitea.
25. **The link to post is the part's own host**, `https://<name>-part.<domain>/`. Its root is a trusted, unsandboxed wrapper with the Stockroom chrome, framing `/c/<cap>/` on the same host. The cap arrives in a `#pf=` fragment from `/grant`. Chrome actions use a credentialed, same-site chrome API with CORS for exactly the sandboxed part-host origins. `/p/<name>` only redirects. Public links move to `<name>-part.<domain>/l/<token>`. First open costs one redirect hop.
26. **Per-part data snapshots are on by default** (daily, keep 7), switchable off per part, with the 90-day idle stop and keep-the-last-one rule.
27. **No notifications for people.** The only push is an optional ops-alert webhook for the admin, off by default. (Round 2c also removed the Requests inbox and approval chips, decision 34.)

**2026-10-23 (round 2c)**

28. **Backends in v1, as containers** (P2a): a part is a page plus, optionally, containers from a Dockerfile, a registry image or an uploaded image, described by a small compose subset. One container set per live version, health-gated, scale to zero by default. Routed under the cap at `/c/<cap>/api/*` with a signed identity header. Only a runner sidecar holds the Docker Engine API (Docker or Podman), and it starts containers from one hardened template on a per-part network: egress on, LAN, host and other parts blocked.
29. **The server builds by default**: frontends in the binary, backends with rootless BuildKit in the runner. Prebuilt files or images are optional. The build method is recorded on every build.
30. **`text` is a first-class kind**: shown as is, with line-anchored comments and edits.
31. **Public links can be "Can contribute"**: guests are web consumers only. They write shared data and call the backend as `role: guest`; they never change code, files or versions, and have no tokens. The admin can roll back, reset or freeze. The communal pixel editor is the named acceptance test for P2, and its backend variant for P2a.
32. **Insights**: admin-only analytics and observability in the workspace, in a separate `insights.db`, with no content in logs or analytics, request ids everywhere, tail-sampled traces, a Debug tab per part, a log viewer, Prometheus metrics and an optional OTLP adapter.
33. **Disk safety and safe defaults**: every growing store has a cap, a retention rule, rotation, an export path and a metric, on by default and run by one janitor. A global disk budget refuses new writes at 95 % but never reads or deletes. Every protection is on by default, and turning one off takes an explicit `[unsafe]` key that shows on Health and in `doctor`.
34. **No approval workflow.** Agents act directly within their consented scopes: create, edit, check, share inward, deploy backends, set secrets. Secrets are write-only and per part. People-only actions (public links, invites, groups, LAN allowlist entries, admin) happen in the UI, reached from a `part_link_for` deep link the agent hands over. "Ask for access" stays, as a person-to-person list in the share sheet. Device-code connect stays: it's sign-in.

**2026-10-23 (Slice 1 freeze)**

35. **Slice 1 is frozen**: exactly what `communal_pixel_editor` needs, cut through P0–P2, with a definition of done. Work that straddled it was split, and the remainders come after Slice 1. Changing it takes the admin.
36. **Telegram in-app browser**: a 5-minute real-phone passkey test is in the non-blocking human checks (decision 52). If it fails, Telegram is set to open links in the normal browser and Hermes' messages hint "open in browser". The person-bound-link fallback is rejected.
37. **OAuth authorization code is after Slice 1.** Slice 1 connects agents by device code and personal tokens; every MCP client the slice needs accepts a static bearer header.
38. **`docs/agent-contract.md` is the shared guide** and the body of `/skill.md`, checked against the registry by a test. It replaces `USAGE.md`.

**2026-10-23 (final round).** Decisions 39–42 are the coordinator's scope calls, **provisional, pending the admin**; decision 43 settles the contract's open questions.

39. **`tsx` is right after Slice 1** *(provisional)*. The `tsx` kind moves off the Slice 1 path and runs right after it; the compiler spike stays in P0 and in the slice. The contract marks `tsx` as after Slice 1, and Slice 1 agents send `html` (React through the mirrored UMD build works).
40. **The Slice 1 preview card is two static PNGs** *(provisional)*, embedded in the binary. The SVG → resvg pipeline moves to the titled card, after Slice 1.
41. **The retention chain stays in Slice 1, and it sets Slice 1's date** *(provisional)*: the janitor → disk budget → retention test chain is as long as the pixel test's path.
42. **Deferred from Slice 1** *(provisional)*, because neither the pixel test nor the definition of done needs them: the versions rail and the bin view (after Slice 1; the ops stay), and the nginx and Traefik example configs, arm64, SBOM, `cosign` and the example CI workflows (after Slice 1).
43. **The agent contract's open questions are settled:** a person connects every agent once (device code or personal token), with no unattended bootstrap; `shared_data_writers: "editors" | "viewers"` (default `viewers`) decides who below `edit` writes shared data, and a contribute link is the only way guests write; `part_check` runs the real prelude over a read-only view of the real shared data with a discarded write overlay, and takes up to 20 `actions`; `live: "always" | "after_check"` on `part_create`/`part_edit` (server default `always`, recommended `after_check`); a rejected write doesn't use up its `request_id`; `part_data` gains `snapshot_list`, `snapshot_restore` and `reset` under `parts:write`; `window.storage` gains paging, `getMany`, limits and `PfError` rejections.

**2026-10-23 (round 3).** Decisions 48 and 49 are **provisional, pending the admin**.

44. **Updates and advisories** (§3h). Three layers kept apart: partifactory itself (§5b, §5d), a part's frontend libraries (a per-build SBOM, `build_deps`) and its backend (an image SBOM at build, matched nightly, base-image tags re-resolved). A live part never changes under you; an update is always a new version that rolls back like any other.
45. **Advisory data is an adapter, matched locally by default**: the OSV bulk dumps, downloaded daily and synced incrementally. The OSV API is the alternative, and it sends package names and versions out; `none` is a warning on Health and in `doctor`. A DB more than 7 days stale is amber, and builds never block on it.
46. **Severity in context**: frontend advisories are shown, not alarmed, and raised one level for a part with a Can-contribute link or a backend; backend advisories weigh more. The display is OSV severity plus context, never a bare CVSS number.
47. **A finding is a record, not a queue.** An `advisories` row; a chrome badge, Health and Insights counts, `part_get`, a `dep_advisory` warning, a `part.advisory` event, and the ops webhook only for critical backend or contribute-link cases. Fixes: `part_edit {rebuild: true, update}`, the Update libraries button, or ignore with a reason (audited).
48. **`build_deps` is recorded in Slice 1** *(provisional)*, so advisory history exists from day one. Matching, the UI and `update` come after Slice 1, backend SBOMs with P2a, and a spike first sizes the OSV dumps, times matching against 10k rows and picks Syft or Trivy.
49. **Auto-update** *(provisional)*: `[advisories] auto_update = "never" | "patch"`, per part too. Frontends default to `never`; backends to `patch`, only with a known fix, through the `after_check` gate, rolled back automatically if the check fails, and recorded as `system`.
50. **Tasks live only in beads.** The plan carries no task ids and no task table: §6a is the build order, `br ready` lists the work, and beads are created only at conversion. The change logs moved to `docs/plan-history.md`.
51. **Backend data is the part's business; rollback is best effort.** As in shartifacts, volumes are shared by every version, rollback restores code only, and partifactory runs no migrations or data rollbacks for backends. It guarantees the exact code and the health gate, nothing more. The backend contract says to migrate forward, additive changes, read per request, lock writes, and back up before a non-additive change. Page data keeps its per-part data snapshots (§5c).
52. **Real devices never block.** Every gate uses emulated or simulated devices: Playwright Chromium (mobile emulation, virtual authenticator), WebKit (mobile profile) and Firefox in CI, and Mobile Safari in the iOS Simulator run locally on a Mac with its results as evidence. Real phones (iPhone Safari, Android Chrome, the Telegram in-app browser, the by-hand run on the example deployment) are human checks the admin does whenever. Nothing depends on them, and a failure is filed as a bug for the swarm. *Why:* the swarm shouldn't wait on a person, and simulated WebKit plus Chromium catch nearly everything a phone would; the rest is cheap to fix afterwards.


**2026-10-23 (round 4).**

53. **Previews show the content wherever the viewer could see it anyway** (the admin, 2026-10-23). A public link unfurls with the titled card and an automatic screenshot poster, plus a short muted `og:video` clip when the part has one, **on by default** for every public link and switchable per link to Image or Card only. The gallery shows the same previews to people who can open the part, and a private part's unfurl stays content-free.
    Posters are made automatically by the render sidecar. A clip is optional and the agent's choice: its own MP4 or `preview.video: "record"`, which the sidecar records while running `preview.actions`. The guide asks agents to consider one for animated or interactive parts. Neither is ever on the save path. In Slice 1, gallery thumbnails are the latest passing render screenshot; the rest of §4c's previews ships right after Slice 1, pulled forward from P4.
    *Why:* a link that shows what it is gets opened, and the privacy line is the audience, not the card.

**2026-10-24 (round 4, continued).**

54. **Markdown and `text` parts get a formatted-view editor in P2** (the admin, 2026-10-24). People edit the page as it reads, like Claude Docs, and every change saves as block edits, so untouched blocks stay byte-identical and agents' edits keep merging by block id. shartifacts and the earlier plan had source editing only.
    Live co-typing moves from the non-goals to a deferred option: `yrs` inside one formatted-view session, committing ordinary versions.
    *Why:* the people who read a write-up are the ones who'll fix a sentence in it, and they shouldn't need Markdown to do it.
55. **Agents hold only short-lived OAuth credentials** (the admin, 2026-10-26). Personal agent tokens are retired: every agent connects by OAuth authorization code (MCP clients) or the device grant (the CLI, headless and unattended agents), with one-hour access tokens and single-use rotating refresh tokens. Lifetimes follow common provider practice instead of a picker: a refresh token expires after 30 days unused and a connection ends 90 days after consent. Supersedes the personal-token parts of decisions 14, 37 and 43.
    *Why:* a long-lived static token is the credential that leaks from env files and CI logs, and once authorization code shipped nothing needed one.
56. **The admin chooses how admin actions are confirmed** (the admin, 2026-10-26). Security settings (`/admin/security`) offer: nothing extra (the signed-in session is enough), the partifactory passkey (the default for a new deployment, as before), or a fresh sign-in at the identity provider (OIDC `prompt=login` with `max_age=0`, checking `auth_time`; provider-neutral, and next to build). Changing the mode is itself an admin action, confirmed under the mode in force, and passkey mode can't be turned on without a registered passkey. Refines §4 "One admin, and only one".
    *Why:* the admin already signs in with a phishing-resistant method at the provider and prefers no second prompt for now; the passkey stays one click away.

## 9. Non-goals (v1)

Multi-tenant SaaS. Multi-node. Offline mode. Listed or indexable public parts. Replacing the family hub's knowledge base.
Public sign-up. A dedicated Telegram bot (Hermes is the bot). Notifications for people: web push, email, mention or reply alerts, per-person notification settings. Agents making public links or inviting people. An A2A agent card, or publishing to the public MCP registry.
Device permissions (camera, mic, geolocation) and service workers in sandboxed parts, except through the admin-gated `unsandboxed` flag. Device-local part storage.
A git or Gitea dependency for part content or backups. A hard dependency on any one vendor for ingress, DNS, identity, CI, hosting or notifications. Backing up the IdP (its operator's concern). Sending email. Version pruning. (Live co-typing moved from here to a deferred option, decision 54.)
Approval queues or request inboxes of any kind. Reading a secret back. Unauthenticated routes into a backend. Privileged containers, host mounts or Engine API access for parts. Backend features outside the compose subset. Third-party analytics or telemetry services (the OTLP adapter sends only where the operator points it). WASM backends in v1.

## 10. Change logs

The change logs of every round, from round 1 to round 3, are in [`docs/plan-history.md`](docs/plan-history.md).
