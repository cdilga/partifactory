# partifactory — plan

> A home for the things Chris, his family and their agents make: pages, React tools,
> docs, decks, diagrams. Each one is a **part**. Every part gets its own origin with a name that means something (`photofinder-part.dilger.dev`),
> every view of it is a capability, and every version is just a manifest of hashes. Serving it is built to be fast and measured
> against nginx and HAProxy.

**Credit.** This project takes its idea from **[shartifacts](https://github.com/LaneBirmingham/shartifacts)
by Lane Birmingham (birmo)**, MIT-licensed. It's a reimagining for a different setup, not a fork.
Any code or text adapted from it keeps Lane's MIT notice in `NOTICE` and gets a credit in the README.

---

## 1. What shartifacts gets right, and what doesn't fit here

I read the repo (Go server ~21k lines, plain JS UI, docs). The parts worth keeping:

| Keep (the good ideas) | Why |
| --- | --- |
| Agents connect over **MCP**. `edits` use `{old_str,new_str}` with a `base_version` compare-and-set | Same shape as Claude's artifact tools, so agents already know how to use it |
| **Immutable numbered versions**, restore = append | You never lose work, and you can say "what changed?" |
| **Claude-artifact compatibility**: `window.storage`, `claude.use("db"/"user"/"comments")`, React/TSX + common libs | Anything Claude writes runs here unchanged |
| **Comments addressed to the agent** (`to_claude`) + `watch` | Lets you comment from your phone and have the agent fix it |
| **Separate share surface** that only answers to tokens | A misconfigured edge can't expose the workspace |
| `get_design_system` so agents make on-brand pages | Pages look consistent without extra prompting |

The parts that don't fit Chris's setup:

| shartifacts assumes | Chris's reality | So partifactory… |
| --- | --- | --- |
| Trust comes from the **network** (LAN/tailnet IPs). Sign-in is optional | Everything comes in through a **Cloudflare Tunnel**, so every request comes from cloudflared and IP trust means nothing | Trust comes from **identity**: partifactory is an OIDC client of **Authentik** (swappable), with a per-part policy engine and in-app invites (§3a) |
| One owner password, "Who are you?" is a name picker | Authentik already knows the household. The Nest already uses it | Real principals: `chris`, family members, `hermes`, `nest`, `claude-mac`, … |
| Opaque-origin `sandbox` iframes + postMessage bridge + hub-managed localStorage + HMAC app sessions | Deep wildcard certs behind a tunnel are fragile or paid; `*.dilger.dev` is free | **Each part gets its own real origin, one level deep: `<name>-part.dilger.dev`**. Native storage, service workers (install to home screen), remembered permissions and same-origin `_rt` calls, with no bridge code |
| JSON-file-per-artifact holding every revision, a hand-written search index, hard-linked app versions | — | **SQLite** (metadata, FTS5 search, comments, db docs, events) + a **content-addressed blob store**. One mechanism covers dedup, rollback, diffs, media and zips |
| Full-stack apps with a Go runner sidecar + Landlock | That's the heaviest third of the codebase | **v1 has no backends.** The per-part `db` capability (a realtime doc store) covers most micro-apps. Backends are a later phase, and only if a real need shows up |
| Offline, every library vendored | Internet-facing anyway | CSP allowlist for cdnjs/jsDelivr/unpkg in v1. Vendoring is optional later |

## 2. The core idea (the "elegant" part)

Three concepts. Everything else is built from them.

1. **Blob**: bytes, named by `sha256`. Stored once, under `blobs/ab/cd/<hash>`.
2. **Part version**: an immutable manifest: `{kind, entry, files: {path → hash}, meta, parent}`.
   Its own hash is its version id, and a monotonically numbered `n` makes it readable.
   A one-line edit to a 40-file part stores one new blob plus one tiny manifest. Diffs are a manifest compare.
3. **Capability**: a signed, scoped, expiring grant `{part, version|live, principal, mode: read|interact|write, exp}`.
   - Opening a part in the workspace → the workspace mints a short-lived capability and the iframe loads `https://<name>-part.dilger.dev/?cap=…`.
     The part host swaps it for a host-only `__Host-cap` cookie (`Secure; HttpOnly; SameSite=None; Partitioned`, so it works framed and standalone) and redirects to a clean URL.
   - A **share link** is a durable, revocable capability (its row in SQLite can be revoked).
   - A **Telegram preview** is a capability with a 24h TTL. It needs no login, so it opens inside Telegram's in-app browser.

   Private view, share link and Telegram preview all use **one mechanism**.

Each part's host answers only for that part, and its cookie is host-only, so a capability for part A can never read part B.
The Claude runtime shim (`window.storage`, `claude.use`) calls **same-origin** `/_rt/*` on its own host, with no postMessage relay.
Native `localStorage`/IndexedDB work too, as per-device caches. Anything that should sync goes through `window.storage`/`db`.
A strict CSP (`connect-src 'self'` + the shared blob host, `form-action 'self'`, `frame-ancestors` = workspace + Nest) keeps parts from
sending requests to other hosts. That limits what being same-site with the homelab can expose.

## 3. Topology

```
                Cloudflare edge
   ┌──────────────────────────────────────────────────────────┐
   │ parts.dilger.dev        → workspace UI, /api, /mcp,      │
   │   the ONE OIDC client of Authentik (auth.dilger.dev);    │
   │   the only host that logs people in and mints grants     │
   │ *.dilger.dev (wildcard) → partifactory answers           │
   │   only names ending -part, 404s everything else          │
   │   <name>-part.dilger.dev  one origin per part            │
   │   blobs-part.dilger.dev   shared immutable /b/<sha256>,  │
   │                           CACHED AT THE EDGE             │
   └───────────────┬──────────────────────────────────────────┘
                   │ Cloudflare Tunnel (existing homelab-tunnel or its own cloudflared)
                   ▼
   TrueNAS custom app: partifactory  (one process, two listeners)
     :8080 workspace  — OIDC session (people) or agent token (machines); policy engine decides
     :8081 content    — requires a valid capability (or the part's policy is `public`); answers nothing else
     /data: partifactory.db (SQLite, WAL) + blobs/   → snapshotted by the host
```

**Hostnames: `parts.dilger.dev` (workspace) + `<name>-part.dilger.dev` (one per part)** (decided). Everything is one level under `dilger.dev`,
so the existing free universal cert (`dilger.dev, *.dilger.dev`) covers it, with no Worker custom-domain tricks and no ACM.
DNS wildcards must be a whole label (`*-part` isn't valid), so routing is **one proxied `*.dilger.dev` record + one tunnel ingress rule → partifactory**.
Explicit records (`ha`, `git`, `nest`…) always take precedence over the wildcard, so existing services are unaffected. Names that exist nowhere land on partifactory and get a 404
unless they end in `-part`. This means no DNS token at runtime and no per-part API calls, and a new part is live the instant it's saved.
(Zone and certificate facts that informed this are recorded in the private site config.)
Parts are same-site with the rest of `dilger.dev`. That's accepted, because parts get `connect-src 'self'` / `form-action 'none'` and so can't send requests to other hosts,
and the remaining exposure is cookie tossing in a viewer's own browser.

## 3a. Identity and access: Authentik by default, swappable, managed in-app

### Reusing the existing Authentik setup

partifactory follows the existing pattern of per-audience groups, one enrollment flow per audience, working email stages and passwordless sign-in. It doesn't invent its own. The site-specific inventory (users, groups, flow and stage names) lives in the **private site config**, never in this repo.

### Three layers, so the provider can be swapped

```
 ┌────────────── partifactory core (provider-neutral) ──────────────┐
 │ Policy engine: grants on parts/spaces → allow/deny + role        │
 │ Subjects: user:<stable-id>, group:<name>, everyone-signed-in,     │
 │           link:<token>, public                                    │
 └──────────────┬──────────────────┬──────────────────┬─────────────┘
          trait Authn        trait Directory     trait Provisioner (optional)
     (who is this person?) (users, groups,      (invite, add by email,
                            membership)          manage own groups)
                ▼                  ▼                  ▼
          Authentik OIDC     Authentik API      Authentik API          ← default
          any OIDC IdP       groups claim only  (not supported → UI    ← swap-in
          dev: local user    static file         hides invite flows)   ← dev/tests
```

Grants refer to provider-neutral subjects, so swapping Authentik for Pocket ID, Keycloak, Zitadel or plain OIDC
changes three adapters and no policy data. A provider without a `Provisioner` still works, and invites just disappear from the UI.

### How people sign in, on per-part origins

- **`parts.dilger.dev` is the only OIDC client** (Authentik OAuth2/OIDC provider, confidential client + PKCE, one redirect URI).
  It's never registered per part, and there are no wildcard redirect URIs.
- Opening `photofinder-part.dilger.dev` without a grant → redirect to `parts.dilger.dev/grant?part=photofinder&return=…`.
  If needed, the workspace runs Authentik login, checks the **policy**, mints a short-lived capability and bounces back. The part host only ever sees capabilities.
- The OIDC `groups` claim gives membership at login. An **Authentik notification webhook** (user/group changes) plus a 60 s
  directory sync means removing someone from a group in Authentik takes effect within a minute. Capabilities are short-lived (10 min, refreshed silently), so revocation takes effect as quickly.
- Cloudflare Access becomes **optional** defence in depth for `parts.dilger.dev/admin` only. It's no longer the auth model, because guests, groups and
  public links have to be decided per part, which Access can't do.

### Who can see a part: the audience ladder

Each part (and each space, as a default) has a list of **grants** `{subject, role}`. Roles: `view` < `comment` < `edit` < `manage`.
The share dialog presents subjects as a ladder from safest to most exposed:

| Audience | Subject | Notes |
| --- | --- | --- |
| **Only me** (default) | `user:chris` | Every new part starts here unless its space says otherwise |
| Specific people | `user:<id>`… | Picked from the directory, or invited (below) |
| A group | `group:<household>`, `group:pf-photofinder`… | Any existing Authentik group can be *granted*; partifactory can only *create and edit* its own `pf-*` groups |
| Everyone signed in | `everyone-signed-in` | Any active Authentik user. Shown with a count ("15 people") so it's never vague |
| **Public, link only** | `link:<token>` | Anyone holding the URL, **no sign-in at all**. Unlisted, `noindex`, unguessable 128-bit token in the hostname path, revocable, optional expiry, `view` (or `comment`) only. Shown in amber as "the worst option". This is the *only* public mode in v1 |
| ~~Listed public~~ | `public` | Not in v1. Nothing in partifactory is ever discoverable without a link |

Effective access is the highest role across all grants that match. Every change is written to an `audit` table (who, what, before → after).

### Invitations: every flow ends in "it just works"

All of these live in the **share dialog** and an **admin → People** page, and they all reuse the existing Authentik mechanics:

1. **Mint an invite link: passkey only, nothing else** (Telegram, SMS, anywhere). It's minted in partifactory's share dialog, never in Authentik's UI.
   partifactory creates an Authentik **invitation** bound to a dedicated `partifactory-invite` flow:
   invitation stage → one field ("What should we call you?"; the username is generated) → **WebAuthn setup (passkey), required** → user write → login → redirect.
   There's no email, no password and no email verification: holding the invite link plus enrolling a passkey on this device *is* the account.
   A recovery path is added later from the person's own settings (add email / second passkey). Lost passkey + no email → Chris mints a new link.
   (Email is optional here. The email path is #2.)
   The invitation's `fixed_data` carries the groups to join (e.g. `pf-photofinder`), and it's single-use with an expiry by default.
   After enrolment the person lands **directly on the part** they were invited to, already granted.
2. **Add by email**: partifactory creates the Authentik user (`type: internal`, email, name, `pf-*` groups), then triggers Authentik's
   **recovery/"set up your account" email** via the site's existing email-link stage (named in site config). The person clicks it, sets a passkey, and arrives at the part.
3. **Invite an existing user**: pick from the directory and grant. A notification (email, or Telegram via Hermes) links to the part.
4. **New group**: "Make a group for this" creates `pf-<name>` in Authentik, so the group can be managed afterwards from either partifactory or Authentik.

Pending invites are listed with status (sent, opened, enrolled, expired) and one-click revoke. Revoking deletes the Authentik invitation.

### UX: click budgets, tested end to end

The UX rule: **every flow has a click budget, and e2e tests (real browser + virtual authenticator) fail if a flow exceeds it.**

| Flow | Who | Budget |
| --- | --- | --- |
| Open a part I'm allowed to see, already signed in | anyone | **0 clicks**: the URL just loads (grant hand-off is silent redirects) |
| Sign in when the session has expired | returning user | **1 tap**: passkey autofill on the identification field (WebAuthn conditional UI), no username typing, no "stay signed in?", no consent screen |
| Make a part public-by-link | Chris | Share → "Anyone with the link" → **Copy** (3), plus a passkey tap if the admin re-auth window has lapsed |
| Mint a passkey invite link | Chris | Share → "Invite someone" → **Copy link** (3). The groups and part grant are pre-filled from the part you're on |
| Accept a passkey invite | new person | open link → type a name → **passkey tap** → they're on the part (1 field, 1 biometric) |
| Add someone by email | Chris | Share → "Invite someone" → type email → **Send** |
| Accept an email invite | new person | click email link → passkey tap → on the part |
| Revoke anything (link, invite, person) | Chris | one click in the share dialog, plus an undo toast for 10 s rather than a confirmation dialog |

**Authentik click reduction** (the `deploy/authentik/` script applies these, idempotently, and records the before-state so each one can be undone):
- **No "Stay signed in?" prompt**: the user-login stage gets one fixed long session (`remember_me_offset=0`), the outcome "yes" used to give (everyone gets the "yes" outcome, with no question asked). (Applied 2026-10-22, with the undo recorded in site config.)
- **No consent screen**: partifactory's OIDC provider uses `default-provider-authorization-implicit-consent`.
- **Passkey autofill**: the identification stage gets the WebAuthn stage attached, so browsers offer the passkey straight away.
- **No logout confirmation**: partifactory's provider invalidation flow is a single-stage, no-prompt flow.
- **Branding**: the Authentik pages partifactory sends people to use a partifactory-styled flow background and title, so the hop doesn't feel like a different site.

**Share dialog** (the heart of the UX): one sheet, with the audience ladder as a segmented control (safest at the top), a live sentence that says
exactly who can see the part now ("Only you" / "You + one person" / "household (9 people)" / "**Anyone with the link**"), an expiry picker,
pending invites with status, and copy buttons. The same sheet works on a phone. Agents' share requests appear at the top as approve/deny chips.

### Admin is Chris, and only Chris (the iykyk part)

- **Admin is pinned to Chris's immutable Authentik user id (`sub`)** in partifactory's config file, not to a group.
  Anyone added to any Authentik group, the IdP's own admin group included, still doesn't become a partifactory admin. Changing the admin means editing config on the host.
- **Admin actions need fresh, strong auth**: changing permissions, inviting, adding users, creating groups, `public`/`link` grants and minting agent tokens
  require an OIDC re-auth with `max_age` ≤ 10 min through the site's **passkey authentication flow** (a passkey, not just a session cookie).
- **partifactory's own Authentik credential is least-privilege.** It's a dedicated service account `partifactory-provisioner` with Authentik RBAC
  permissions to: read users/groups, create users, create/delete invitations, send recovery emails, and add/remove members of **`pf-*` groups only**.
  It's **not** a superuser, so it can't touch the IdP admin group, other services' admin groups, or flows. Even a compromised partifactory
  can't escalate anyone into the rest of the homelab. The core also rejects any provisioning call that names a non-`pf-*` group, so it's enforced twice.
- **Agents never administer.** Agent tokens can create and edit parts in their scope and *request* a share ("Hermes wants to share
  *meal-plan* with the household group"). That lands as a pending approval for Chris, in-app or via a Telegram button. Only Chris's re-authed session applies it.
- **Guests can't escalate either**: `manage` on a part lets someone edit *that part's* grants among subjects they already have, never invite new people or
  create link/public grants. Only the admin can do that.

### Machines (agents)

| Principal | Path in | Auth |
| --- | --- | --- |
| Claude Code (Mac, other envs) | `parts.dilger.dev/mcp` | partifactory **agent token** (`pft_…`, shown once, stored hashed), minted by the admin in-app |
| Hermes (VM) | same `/mcp` + `part` CLI | its own agent token |
| Nest (`nest-api.dilger.dev`) | server-to-server `/api` | its own agent token, scoped to the `family` space |
| Telegram users | never directly | Hermes is the Telegram front door and hands out capability links |

Each token has **scopes** (spaces, `put`/`share-request`/`read`), an optional expiry and a last-used time, and is revocable in-app.
Tokens are a separate `Authn` path from people, so they don't depend on the IdP.

### Spaces

Spaces are **default audiences**: `chris` (only me, the default for Chris's agents) and `family` (the household group or a `pf-family` group,
plus the Nest and Hermes's `nest-family` profile). A part inherits its space's grants and can add or override them. There's no nested hierarchy.
Deploying for "just me" or "the whole household" is a one-line change to a space's default grant.

## 4. Surfaces

### MCP (the agents' API): a small set of verbs

| Tool | Does |
| --- | --- |
| `part_put` | create or update. `name` (semantic, becomes `<name>-part.dilger.dev`), `content` or `edits[]`, `files{path: str\|null}`, `base_version` CAS, `kind`, `title`, `space`, `tags`, `note`. Returns id, version, URL, **warnings** (compile errors, blocked URLs) |
| `part_get` | source of a version (+ `history`, `diff_from`), plus recent `runtime_errors` reported by viewers' browsers |
| `part_find` | FTS5 search + filters (`kind:`, `tag:`, `space:`, `has:comments`, `updated:`) |
| `part_share` | mint/revoke a share or preview capability (`ttl`, `version` or live, `mode`) |
| `part_comments` | list/add/resolve. Threads addressed `to_agent` are surfaced to the agent |
| `part_data` | the part's `db`/`storage` documents (Claude ArtifactData-shaped) |
| `part_watch` | long-poll the event log since a cursor (new versions, comments, data changes) |
| `design_system` | rules + starter, so new parts look like partifactory unless asked otherwise |

Plus **MCP server instructions** sent at init, so "make me a packing checklist in partifactory" is enough.

### Names: semantic, chosen by the agent, made easy

A part's name *is* its hostname (`<name>-part.dilger.dev`), so names are first-class and permanent-ish.

- **`part_put` takes an optional `name`** (`photofinder`). If it's omitted, the server derives one from the title
  ("Max's sleep chart" → `max-sleep-chart`): lowercased, ASCII-folded, stopwords dropped, max 4 words.
- **`part_name` tool** (and `part name` CLI): `{want?: "photofinder", title?, description?}` → is `want` available? If not,
  **ranked semantic alternatives** (`photo-finder`, `photofinder-family`, `family-photos`, `photofinder-2026`), all valid and free right now.
  An agent can check before it builds, and never ends up with a random slug.
- **Taken names fail helpfully**: a clash in `part_put` returns `name_taken` *with* the suggestions, so the agent picks one and retries in one step.
- **Rules (checked once, in one place)**: `[a-z0-9-]`, 1–58 chars (so `<name>-part` fits the 63-char DNS label), no leading/trailing/double hyphen.
  Reserved: `blobs`, `www`, `api`, `admin`, `auth`, `login`, `mcp`, and any name that mirrors an existing `dilger.dev` service (`ha`, `git`, `nest`…),
  so no `ha-part` lookalikes.
- **Renames redirect**: the old hostname 301s to the new one for good, so shared links keep working. Native browser storage stays with the old origin, so
  a rename warns about that, and `window.storage`/`db` data, which lives on the server, is unaffected.
- **Same names for every surface**: Telegram previews, share links, the Nest embed and the workspace URL (`parts.dilger.dev/photofinder`) all use the name.

### `part` CLI (one binary): for shells, scripts, Hermes, CI

`part put page.html --title "…"` · `part put ./dir` (multi-file) · `part share <id> --ttl 1d` · `part get <id> > x.html` · `part watch` · `part open <id>`.
It speaks the same HTTP API as MCP, with service-token creds in `~/.config/partifactory/`.

### Events out

An append-only `events` table drives SSE for the UI, `part_watch` for agents, and **outbound webhooks**
(e.g. to Hermes: "A family member commented on *sleep chart*: '…'" → Hermes posts to Telegram, or picks the job up itself).

### Workspace UI (`parts.dilger.dev`)

Library (search-first, `/` to focus), part view (Preview / Split / Source, version scrubber, diff), comments rail,
Share dialog, phone-first layout. Small and fast. It's the thing the family actually opens, so it gets real design attention.

### Telegram, through Hermes (no separate bot)

Hermes already owns Telegram. Give Hermes the MCP connection + a `partifactory` skill:
- "make me a chart of …" → Hermes `part_put` → replies with a **preview capability link** that opens inside Telegram, no login.
- A comment webhook → Hermes → Telegram message, with a "fix it" path.

### The Nest

- The Nest embeds family parts via a capability iframe (minted server-to-server by the `nest` principal).
- Hermes's `nest-family` profile can create parts in the `family` space while answering in Nest ("make this a chart").
- "Keep this in Nest" stays the Nest's job. partifactory only provides the embed and a stable link.
  Git stays the Nest's source of truth, and partifactory doesn't try to be a knowledge base.

## 5. Stack: Rust, all the way down

One Cargo workspace builds **one static binary** (`partifactory`, with a `part` CLI subcommand/alias) that has every asset embedded. No Node at runtime.

| Concern | Choice |
| --- | --- |
| HTTP / async | `axum` + `tokio`, `tower` middleware for the two listeners |
| Storage | SQLite via `rusqlite` (bundled, FTS5, WAL), behind a `Store` trait. **Stretch:** a `frankensqlite` backend behind the same trait once the conformance tests pass against both |
| Blobs | a `BlobStore` trait. v1 is a local directory (`blobs/ab/cd/<sha256>`). An R2/S3 impl later costs nothing to add |
| TSX/JSX → JS | **`oxc`** (transformer + minifier) to compile each file, **`rolldown`** to bundle multi-file parts. Both are pure Rust and in-process, with output cached by manifest hash |
| Markdown / code | `comrak` (GFM) + `syntect` (highlighting) server-side. Mermaid and KaTeX render client-side |
| MCP | `rmcp` (the official Rust SDK), Streamable HTTP, stateless |
| Edge auth | `jsonwebtoken` + a cached JWKS from `https://<team>.cloudflareaccess.com/cdn-cgi/access/certs`. Capabilities are compact HMAC-signed tokens (`ring`) |
| Workspace UI | **Leptos** (SSR + hydration, Rust → WASM), so the UI shares types with the server |
| In-part runtime shim | the only hand-written JS/TS: a few KB (`window.storage`, `claude.use`, error reporting), built at compile time and embedded |
| Tests | `cargo nextest`, golden tests for manifests and diffs, a property test for capability scoping (cap A never reads B), and an end-to-end test that drives a real browser against `*.localhost` origins |

**Bend 2?** I think Bend isn't the right fit for the server, though I'd confirm against the bend2 skill before ruling it out completely.
Bend's strength is automatically parallelising *pure computation* across cores and GPUs, and this service is almost entirely I/O:
HTTP, SQLite, filesystem, crypto. Bend also lacks the ecosystem a server needs (TLS, HTTP, SQLite, JWT). If a part ever needs heavy
parallel compute, that's where Bend could fit. Rust is the ambitious choice that will actually ship.

## 5a. Performance: a design goal with numbers, tested in CI

Honest framing: a hand-built proxy won't beat HAProxy or nginx at *general* proxying, and at this load any of them would be idle.
But partifactory only has to handle a **reduced subset** of HTTP: a fixed route table compiled into the binary, a handful of headers that matter,
and responses whose bytes are fixed (immutable, precompressed, ETag = hash). It does no config interpretation, regex routing, module chain or generic
header processing. It also needs no upstream hop: nginx would have to proxy to an app server, while here the app *is* the server.
Doing less work per request is how it can beat them on *this* workload. On raw static blobs, nginx's `sendfile` path is already near the syscall floor,
so expect parity there and wins on the dynamic paths.
The ambitious part is the design, which means **most requests never reach the NAS, and the ones that do stay off the slow paths.**

1. **Let the edge do the work.** Every blob is named by its sha256, so its bytes can never change. The content host serves blobs at `/b/<sha256>` with
   `Cache-Control: public, max-age=31536000, immutable` and a Cloudflare cache rule for that path. Pages, scripts, libraries and media
   are then served from Cloudflare's edge after the first view. Only the small per-view HTML entry (which carries the capability) and `_rt` data calls travel the tunnel.
   Blobs live on one shared host, `blobs-part.dilger.dev/b/<sha256>` (CORS `*`, they're public-by-hash and unguessable), so React, libraries and
   media are cached **once** across every part, not once per hostname.
   The current zone-wide "bypass cache for self-hosted" rule needs a single exception for `blobs-part.dilger.dev/b/*`.
2. **Do the work at save time, not view time.** On `part_put`, compile TSX (oxc/rolldown), render Markdown, minify, and precompress each output
   blob with **brotli-11 and zstd-19**, stored next to the raw bytes. Serving is then content negotiation plus a file send. No compression happens on the request path.
3. **A lean hot path.** Blob responses are `pread`/`sendfile` of a precompressed file, so the bytes never pass through userspace buffers.
   Small hot blobs live in a memory-mapped LRU, ETag = the hash, and `304` is answered without touching disk.
   Capability check = one HMAC verify (no DB read). Session check = one keyed lookup in an in-memory session cache.
4. **SQLite tuned for reads.** WAL, `mmap_size`, a pool of read connections + one writer task, prepared statements cached,
   FTS5 with `bm25` ranking. Events go to SSE subscribers from an in-memory broadcast, with no polling.
5. **A runtime that can be swapped.** Start on tokio/hyper (HTTP/1.1 + h2c from cloudflared). The serving core sits behind a small trait, so a
   thread-per-core **io_uring** runtime (`monoio`/`compio`) can be benchmarked as an alternative on Linux. It only replaces tokio if it wins in the benchmark.
   Pingora stays an option if we ever proxy to app backends.
6. **Cut the tunnel's cost too.** cloudflared runs with `--protocol quic`, has multiple connections, and sits on the same host as the binary.

**Targets (failing them blocks a release):**

| Metric | Target |
| --- | --- |
| Blob served from origin, p99 server time (warm) | < 200 µs |
| Per-view HTML entry incl. capability check, p99 | < 1 ms |
| `part_find` over 10k parts, p99 | < 5 ms |
| `part_put` small edit (no TSX), p99 | < 10 ms |
| Repeat open of a part from a phone, end to end | < 150 ms, mostly edge-cached |
| Static-blob throughput on the NAS vs **nginx** and **HAProxy** serving the same precompressed files | ≥ parity, tracked over time |

`partifactory bench` drives these with `oha`/`wrk2` and keeps the results in `bench/history.jsonl`. The nginx/HAProxy comparison runs as a docker
compose fixture serving the identical blob directory, which keeps the comparison honest.

## 5b. Builds and deploys: where Docker belongs

- **Building parts: no Docker, no build chain.** shartifacts runs esbuild in-process for pages, and Go builds in a runner container for app backends.
  partifactory compiles TSX/JSX, CSS and Markdown **in-process** with oxc + rolldown at save time. That takes milliseconds, versus seconds to start a container,
  and the result is cached by manifest hash. Agents just send source, and there's nothing to install.
  For the rare part that needs real npm packages, the agent builds locally (`vite build`) and puts the `dist/` folder as a multi-file part.
- **Deploying partifactory: yes, Docker, because TrueNAS custom apps *are* Docker.** The image is `FROM scratch` (or distroless/static)
  with one static musl binary (~15–25 MB), one volume (`/data`) and env vars. It starts in milliseconds. cloudflared runs alongside it in the same compose file.
- **Day to day, without Docker:** `cargo run` (or `bacon` for watch mode) locally against `*.localhost`. `part` CLI binaries for macOS/Linux
  ship via GitHub Releases + `cargo binstall`, so the Mac and the Hermes VM get a single file, no container.
- **If backends ever happen (P5): WASM before Docker.** Per-part backends as **WebAssembly components on `wasmtime`**, in-process, with
  capability-scoped imports (its own KV/SQLite and an HTTP allowlist). Cold start is microseconds and memory per backend is tiny,
  and the sandbox comes from the runtime, not Landlock + a sidecar. Docker-per-app stays the fallback for anything that can't target WASM.

## 5c. Disaster recovery and self-provisioning (design goal)

**From nothing to running with one credential set:** a Cloudflare API token, an IdP bootstrap token, and a Docker host that partifactory can reach.

- **`partifactory provision` is a declarative, idempotent reconciler.** It isn't a list of steps, and running it twice changes nothing. From `site.toml` (private site config) it ensures:
  - **Cloudflare**: its own tunnel + ingress, the `parts` record and the `*` wildcard record (only after the service answers health checks), the blob cache rule.
  - **Authentik** via **blueprints** (Authentik's own declarative YAML, applied through the API): the OIDC provider/app, the `partifactory-invite` flow,
    the login-stage settings, and the `partifactory-provisioner` service account + least-privilege RBAC role. Its token is minted and stored as a secret.
  - It prints a plan/diff first (`--dry-run`), records every before-state, and refuses to delete anything it didn't create (resources are tagged/named `partifactory-*`).
- **Data is continuously backed up off-host, so recovery is restore + reconcile:**
  - SQLite is streamed with **Litestream** to an R2 bucket (point-in-time, seconds of RPO).
  - Blobs are content-addressed, so the backup is an append-only sync of new hashes to R2. It never rewrites, and verification is just comparing hashes.
  - `partifactory restore --from r2://…` on a fresh host rebuilds `/data`, then `provision` re-points the tunnel. Target RTO: under 15 minutes.
  - A **monthly restore drill** in CI restores into a throwaway container and runs the e2e suite against it. A backup only counts once it has been restored.
- **Any Docker host, not just TrueNAS.** Deploy targets are a small adapter: `docker` (any host via a Docker context over SSH: a VPS, the Hermes VM,
  a Mac), with `truenas` as one adapter (middleware custom app). Moving hosts = change `deploy.target` in site config, run `restore` + `provision`, done.
  The cloudflared connector runs next to the app wherever it lives, so the hostnames never change.
- **CI/CD on Gitea Actions, with secrets only in Gitea:** push → build/test/bench → signed image to the Gitea registry → protected deploy job
  (`partifactory provision && deploy`) to the configured target. Secret names: `PF_CF_API_TOKEN`, `PF_TUNNEL_TOKEN`, `PF_IDP_BOOTSTRAP_TOKEN`, `PF_IDP_PROVISIONER_TOKEN`,
  `PF_SESSION_KEY`, `PF_CAP_HMAC_KEY`, `PF_R2_*`, `PF_DEPLOY_SSH_KEY`. The GitHub mirror is passive: no Actions, no secrets.
- **Key rotation is a reconcile too:** rotate a secret in Gitea and re-run the pipeline. Capability keys are versioned (`kid`), so rotating doesn't break links that were minted under the old key.

**Infrastructure stays outside the app.** partifactory doesn't know about TrueNAS or ZFS. It's an OCI image that needs one
writable data directory and a few env vars. Running it as a TrueNAS custom app, snapshotting the dataset and wiring cloudflared
are deployment concerns, and they live in `deploy/`, not the code. The Cloudflare side (tunnel ingress, wildcard record, cache rule) and the Authentik side (OIDC provider, `partifactory-enrollment` flow, `partifactory-provisioner` RBAC role)
is a script against the Cloudflare API, so it can be reproduced without clicking through the dashboard.

## 6. Phases

Each phase ends with something usable.

**P0 Core (local only).** Repo, SQLite schema, blob store, version manifests, `part` CLI against localhost, HTML + Markdown + SVG + code kinds,
content server with capabilities, precompressed blobs, `partifactory bench` with the nginx/HAProxy fixture.
*Done when:* `part put x.html --name packing` → opens at `packing-part.localhost:8081` (browsers resolve `*.localhost` to loopback), and the blob targets in §5a are measured.

**P1 Edge + identity.** Tunnel ingress + `*.dilger.dev` wildcard, Authentik OIDC login on `parts.dilger.dev`, grant hand-off to part hosts,
policy engine + audience ladder, admin pinned to Chris + passkey re-auth, agent tokens, share/preview links. Minimal workspace UI (library, view, versions, share dialog).
**P1b Invites.** `partifactory-provisioner` service account + RBAC, `partifactory-enrollment` flow, invite links, add-by-email, `pf-*` groups,
Authentik webhook sync, pending-invite list. *Done when:* Chris invites a new email address from the share dialog, they enrol with a passkey and land on the part, and removing them from the group in Authentik locks them out within a minute.
*Done when:* Chris opens a part on his phone via SSO, and a share link works from mobile data with no login. The security checklist (§7) passes from outside.

**P2 Agents.** MCP server + instructions + design system, Claude Code connected (Mac), Hermes connected (MCP + CLI + skill),
comments with `to_agent`, events/SSE/`part_watch`, webhook → Hermes → Telegram.
*Done when:* "Hermes, make me a meal-plan page" in Telegram → link in Telegram → comment on it → Hermes fixes it.

**P3 Claude compatibility.** React/TSX kind with the common library set + shadcn/Tailwind, `window.storage`, `claude.use` for `db`/`user`/`comments`/`artifact`,
runtime error reporting back to `part_get`. Borrow shartifacts' compat matrix as the spec (with credit).
*Done when:* a page copied from a claude.ai artifact runs unchanged.

**P4 Family + Nest.** `family` space, family onboarding, Nest embed, Hermes `nest-family` creating family parts, slides + mermaid kinds.

**P5 (only if needed) backends.** If a real part needs a server: run it as an isolated container per app on TrueNAS (or a Worker),
reached through the same capability model. Not designed further until there's a concrete use.

## 7. Security checklist (run from outside the network)

- `parts.dilger.dev` without a session → Authentik login. A part host without a capability → redirect to `/grant`, never content.
- Only Chris's `sub` reaches admin; a user added to the IdP admin group still gets 403 on admin routes. Admin routes without a fresh passkey auth → re-auth.
- The provisioner credential cannot add anyone to a non-`pf-*` group (test it against Authentik, expecting 403), and the core refuses before even trying.
- Agent tokens can't change grants or invite; share requests queue for approval.
- The origin is unreachable except through the tunnel (no published ports on TrueNAS beyond the docker network).
- The content listener answers **only** capability-bearing requests: `/`, `/api`, `/mcp` → 404 there.
- A capability for part A reads nothing of part B. A revoked share → 404 within seconds. A preview cap expires on time.
- Parts get a strict CSP: `connect-src 'self'`, script/style from self + the CDN allowlist, `frame-ancestors` = workspace + Nest only.
- Service tokens are one per agent, scoped, revocable. No tokens end up in part source, logs or webhook bodies.

## 8. Decisions (2026-10-22)

1. **Hostnames**: `parts.dilger.dev` (workspace) + `<name>-part.dilger.dev` per part via a `*.dilger.dev` wildcard → tunnel, with agent-chosen semantic names.
   No separate proxy process: the edge layer is built into the binary and benchmarked against nginx/HAProxy.
5. **Docker for deploying partifactory, not for building parts** (§5b).
2. **Stack**: Rust (section 5). Bend 2 considered and set aside for the server.
3. **Host**: TrueNAS, but the app stays infrastructure-agnostic (plain OCI image + data dir).
4. **Code reuse**: written fresh, with shartifacts as the reference spec, credited.

**Tunnel**: partifactory gets its own Cloudflare tunnel named `partifactory` (decided 2026-10-22), so it can be revoked independently. Its ingress: `parts.dilger.dev` → `http://partifactory:8080`, `*.dilger.dev` → `http://partifactory:8081`, everything else → 404. DNS records are created only once the service is up, so the wildcard never routes to a dead tunnel.

## 9. Non-goals (v1)

Multi-tenant SaaS. Per-part backends. Offline mode. Replacing the Nest's knowledge base.
Public sign-up. A dedicated Telegram bot (Hermes is the bot).
