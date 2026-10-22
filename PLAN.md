# partifactory — plan

> A home for the things Chris, his family and their agents make: pages, React tools,
> docs, decks, diagrams. Each one is a **part**, with its own origin and a name that means something (`photofinder-part.dilger.dev`).
> Every view of a part is a grant, every version is a manifest of hashes, and agents connect with their own OAuth tokens, separate from any person's.
> Serving is built to be fast and measured against nginx and HAProxy.

**Credit.** This project takes its idea from **[shartifacts](https://github.com/LaneBirmingham/shartifacts)
by Lane Birmingham (birmo)**, MIT-licensed. It's a reimagining for a different setup, not a fork.
Any code or text adapted from it keeps Lane's MIT notice in `NOTICE` and gets a credit in the README.

**Site config is private.** Everything specific to one deployment (IdP inventory, flow and stage names, group names, account facts, deploy target)
lives in a separate private repo (`partifactory-site`). Secrets live only in Gitea Actions secrets. This plan uses `dilger.dev` hostnames, which are public DNS anyway, and nothing else site-specific.

---

## 1. What shartifacts gets right, and what doesn't fit here

I read the repo (Go server ~21k lines, plain JS UI, docs). The parts worth keeping:

| Keep (the good ideas) | Why |
| --- | --- |
| Agents connect over **MCP**. `edits` use `{old_str,new_str}` with a `base_version` compare-and-set | Same shape as Claude's artifact tools, so agents already know how to use it |
| **Immutable numbered versions**, restore = append | You never lose work, and you can say "what changed?" |
| **Claude-artifact compatibility**: `window.storage`, `claude.use("db"/"user"/"comments"/"artifact")`, React/TSX + common libs | Anything Claude writes runs here unchanged, including state that persists and pages that save themselves |
| **Comments addressed to the agent** (`to_claude`) + `watch` | Lets you comment from your phone and have the agent fix it |
| **Separate share surface** that only answers to tokens | A misconfigured edge can't expose the workspace |
| **Per-agent revocable tokens**, removed before anything reaches app code | Each agent can be cut off on its own |
| **Runtime errors reported back to the agent** (`runtime_errors`) | The agent finds out its page is broken without needing a browser |
| `get_design_system` so agents make on-brand pages | Pages look consistent without extra prompting |

How shartifacts does agent auth, for reference: there's an optional owner password. With it on, each agent gets a static bearer token (`shx_…`)
made in the UI or with `shartifacts auth token NAME`, sent as `Authorization: Bearer` on a stateless MCP endpoint. The token can do
everything except manage the password and tokens. With the password off, network position is the only check. Tokens never expire, so you revoke them by hand.
partifactory keeps "one token per agent, revocable" and replaces how the token is obtained and what it can do (§3b).

What doesn't fit Chris's setup:

| shartifacts assumes | Chris's reality | So partifactory… |
| --- | --- | --- |
| Trust comes from the **network** (LAN/tailnet IPs). Sign-in is optional | Everything comes in through a **Cloudflare Tunnel**, so every request comes from cloudflared and IP trust means nothing | Trust comes from **identity**: an OIDC client of **Authentik** (swappable), a per-part policy engine and in-app invites (§3a) |
| One owner password, "Who are you?" is a name picker | The IdP already knows the household | Real principals: people from the IdP, agents as OAuth clients acting *on behalf of* a person (§3b) |
| Static all-powerful agent tokens | Agents should get exactly what they need | **OAuth 2.1 per the MCP authorization spec**: consented, scoped, space-limited, expiring, audited as "Hermes on behalf of Chris" |
| Opaque-origin `sandbox` iframes + postMessage bridge + hub-managed localStorage + HMAC app sessions | `*.dilger.dev` is covered by the free cert | **Each part gets its own real origin: `<name>-part.dilger.dev`**. Native storage, service workers, remembered permissions, same-origin `_rt` calls |
| JSON-file-per-artifact holding every revision, a hand-written search index | — | **SQLite** (metadata, FTS5, comments, db docs, events, audit) + a **content-addressed blob store** |
| Full-stack apps with a Go runner sidecar + Landlock | That's the heaviest third of the codebase | **v1 has no backends.** The per-part `db` covers most micro-apps. WASM backends later, only if a real need shows up |
| Every library vendored, offline | We want both: no third-party runtime loads, and no hand-maintained vendor list | **Mirror-on-save**: CDN URLs in a part are fetched once at save time, pinned by hash and served from our own blob host (§2) |

## 2. The core idea

Four concepts. Everything else is built from them.

1. **Blob**: bytes, named by `sha256`, stored once (`blobs/ab/cd/<hash>`). There are two classes:
   - **Shared blobs**: third-party libraries, fonts and the runtime shim. They aren't private to anyone, are edge-cached forever on `blobs-part.dilger.dev/b/<hash>`, and are cached once across all parts.
   - **Part blobs**: a part's own files and media. They're served **only from that part's host** under `/_b/<hash>`, behind the part's grant.
     Browsers cache them with `private, immutable`, and **they are never edge-cached**, so revoking access really revokes it and nothing private sits in Cloudflare's cache.
2. **Part version**: an immutable manifest `{kind, entry, files: {path → hash}, meta, parent, author, actor}`. Its hash is the version id, and `n` is a readable counter.
   A one-line edit to a 40-file part stores one new blob plus one tiny manifest. Diffs compare manifests, then the changed files.
3. **Grant**: who may do what to which part, from the policy engine (§3a). It covers people, groups, link holders and agents.
4. **Capability**: a short-lived, signed, host-bound token that carries a decided grant to a part's host, so the content listener never needs the database to authorise a request.

**Mirror-on-save, instead of a CDN allowlist.** When a part references `https://cdnjs.cloudflare.com/…` (or jsDelivr or unpkg), the save step fetches it once
(an allowlist of CDN hosts, size-capped, SRI-verified when the source gives an `integrity`), stores it as a shared blob and rewrites the URL.
At runtime parts load **nothing from third parties**, so the CSP can be `default-src 'self' https://blobs-part.dilger.dev` with no exceptions,
and Claude-written HTML with CDN `<script>` tags still works unchanged. Bundled common libraries (React, recharts, d3, three, …) are pre-seeded as shared blobs. **Bare imports** (`import { LineChart } from "recharts"`) in TSX are resolved
by the bundler against that set at save time. HTML parts with `<script type="module">` get a server-emitted import map pointing at the same shared blobs, so there's one library set and one version per release.

**Grant hand-off, without tokens in URLs that stick around.**
- A part host with no valid cookie redirects to `parts.dilger.dev/grant?part=<name>&path=<path>`. Only the *path* is carried, and the host is derived from the part name, so this **can't act as an open redirect**.
- The workspace authenticates the person if needed, asks the policy engine, and redirects back with a **one-time code** (`?g=…`: 30 s TTL, single use, bound to the part host).
- The part host exchanges the code server-side, sets a **host-only** cookie `__Host-pf_cap` (`Secure; HttpOnly; SameSite=Lax`) holding a 10-minute capability that refreshes silently,
  and 302s to the clean URL. `Referrer-Policy: no-referrer` on every response.
- `SameSite=Lax` works framed *and* standalone because the workspace, the Nest and every part are **same-site** (`dilger.dev`).
  A same-site iframe is a first-party context, so third-party-cookie blocking and Safari's ITP don't apply, and `Partitioned`/`SameSite=None` aren't needed.
- **The design doesn't *depend* on that cookie**, so it survives Safari quirks and a future move of parts to their own domain (§3):
  - **Framed views** get their `src` minted server-side (workspace, Nest) with a one-time code. If the part host can't set its cookie (a cross-site frame),
    it falls back to **path mode** (`/c/<cap>/…`): the capability is a path prefix, so relative URLs inherit it, with no cookies needed.
  - **Runtime calls (`/_rt/*`) never use ambient credentials.** The shim gets its capability from a bootstrap response, keeps it in JS memory and sends
    `X-PF-Cap` on every call. The server also requires `Sec-Fetch-Site: same-origin`. That rules out CSRF from other sites and from sibling parts, cookie or not.
- **Revocation without DB reads**: an in-memory `grant_epoch` (persisted in SQLite) is bumped on any revoke, unshare or agent-token change. Capabilities embed the epoch they were
  minted under, and verifying one is an HMAC check plus `epoch ≥ current-for-that-part`. Revocation applies on the next request, and the hot path stays DB-free.
  Every capability also records `minted_by` (person, agent or link), so revoking an agent or a link cascades to everything it minted.
- **Telegram and other unfurlers never see content**: preview and share links posted anywhere point at `parts.dilger.dev/s/<token>`, which serves a static,
  content-free card (title only if the part opts in) and a JS hop to the real URL. Link-preview fetchers get the card, and people get the part.
- **Link shares** are the exception, by design: the URL carries a long-lived 128-bit token (`/l/<token>/`), because that's what a link share *is*.
  The token is exchanged for the same cookie on first load and stripped from the address bar.

**Each part's host answers only for that part.** Its cookie is host-only, so a grant for part A can never read part B.
The runtime shim (`window.storage`, `claude.use`) calls same-origin `/_rt/*`, with no postMessage relay.
Native `localStorage`/IndexedDB work as per-device caches. Anything that should sync or be shared uses `window.storage`/`db`.

## 3. Topology

```
                Cloudflare edge  (own tunnel: `partifactory`)
   ┌──────────────────────────────────────────────────────────────┐
   │ parts.dilger.dev        workspace UI, /api, /mcp, /oauth/*,   │
   │                         /grant — the only host that logs     │
   │                         people in and issues tokens           │
   │ *.dilger.dev (wildcard) → content listener; answers only     │
   │                         names ending -part, else 404          │
   │   <name>-part.dilger.dev   one origin per part (not cached)   │
   │   blobs-part.dilger.dev    shared blobs only, edge-cached     │
   │ WAF rate-limit rules on /grant, /oauth/*, /l/*, /mcp          │
   └───────────────┬──────────────────────────────────────────────┘
                   │ cloudflared (runs next to the app, QUIC)
                   ▼
   Any Docker host (TrueNAS by default): partifactory, one process
     :8080 workspace  — person session or agent OAuth token; policy engine decides
     :8081 content    — capability cookie or link token; nothing else
     :9090 ops        — /metrics, /healthz, /readyz; never routed through the tunnel
     /data: partifactory.db (SQLite, WAL) + blobs/ → Litestream + blob sync to R2
```

**Hostnames** (decided): everything is one level under `dilger.dev`, so the existing free universal cert covers it.
DNS wildcards must be a whole label (`*-part` isn't valid), so routing is **one proxied `*.dilger.dev` record + one tunnel ingress rule**.
Explicit records (`ha`, `git`, `nest`…) always take precedence over the wildcard. Names that exist nowhere land on partifactory and get a 404 unless they end in `-part`.
There's no DNS token at runtime, and a new part is live the instant it's saved.

**Same-site risk, stated honestly.** Parts share a site with the rest of `dilger.dev`, which allows two things:
1. **Same-site requests.** CSP blocks fetches, submits and embeds elsewhere: `default-src 'none'; script-src 'self' <blob-host>; style-src 'self' 'unsafe-inline' <blob-host>;
   img-src 'self' data: blob: <blob-host>; media-src/font-src likewise; connect-src 'self'; frame-src 'self'; worker-src 'self' blob:; form-action 'self';
   base-uri 'none'; object-src 'none'; frame-ancestors <workspace> <nest>`. **CSP can't stop top-level navigation**, so a malicious part can still navigate its viewer
   to a GET on another `dilger.dev` service, or phish. That's inherent to running arbitrary JS, and it's why the domain decision below matters.
2. **Cookie tossing.** A part's script can set `document.cookie` with `Domain=dilger.dev`, which other services' cookies don't prevent unless they use `__Host-` names.
   CSP can't stop it. Defences: the part's content is written by your own agents and people. Every part HTML response gets a tiny server-injected prelude that
   makes `document.cookie` writes with a `Domain` attribute throw (defence in depth only, since it can be bypassed). And **the part suffix is site config**, so
   moving parts to a separate registrable domain later (e.g. `*.dilger.parts`) is a config change plus 301s, not a redesign.
   Revisit this if parts ever show untrusted third-party content, for example an agent that renders scraped web pages.

**Open decision: give parts their own registrable domain?** The GLM review's #1 recommendation. A separate domain (e.g. `dilger.parts`, ~$10–30/yr,
with its own free universal cert, so names become `photofinder.dilger.parts`) removes cookie tossing and same-site exposure of `auth`, `ha`, `git` and `nest` to agent-written JS completely.
Every other mitigation above is a patch. The cost: framed views become cross-site, so they use path mode (above) instead of the cookie. The design already supports that, so it's a config change.
**My recommendation: buy the domain before P1**, since the cookie tossing risk to the IdP's own session cookie is the one that matters. It's Chris's call.

## 3a. Identity and access: Authentik by default, swappable, managed in-app

### Three layers, so the provider can be swapped

```
 ┌────────────── partifactory core (provider-neutral) ──────────────┐
 │ Policy engine: grants on parts/spaces → allow/deny + role        │
 │ Subjects: user:<stable-id>, group:<name>, everyone-signed-in,     │
 │           link:<token>, agent:<client-id>@user:<id>               │
 └──────────────┬──────────────────┬──────────────────┬─────────────┘
          trait Authn        trait Directory     trait Provisioner (optional)
     (who is this person?) (users, groups,      (invite, add by email,
                            membership)          manage own groups)
                ▼                  ▼                  ▼
          Authentik OIDC     Authentik API      Authentik API          ← default
          any OIDC IdP       groups claim only  (absent → UI hides     ← swap-in
          dev: local user    static file         invite flows)         ← dev/tests
```

Grants refer to provider-neutral subjects, so swapping Authentik for Pocket ID, Keycloak, Zitadel or plain OIDC changes adapters, not policy data.
Kept deliberately thin, because swappability was asked for, but there's no second production adapter until one is needed: v1 ships the Authentik adapter plus a test-only fake.
partifactory reuses the site's existing Authentik pattern (per-audience groups, an enrollment flow per audience, passwordless sign-in, working email stages).
The inventory is in site config.

### How people sign in

- **`parts.dilger.dev` is the only OIDC client** (confidential + PKCE, one redirect URI), with no per-part or wildcard redirect URIs. It uses implicit consent, so there's no consent screen for people.
- Group membership comes from the OIDC `groups` claim at login plus a **60 s directory poll**, which is the single mechanism. A push webhook is only added if poll latency ever hurts.
  When the poll sees a change, it bumps the grant epoch for affected parts, so removing someone takes effect within about a minute.
- **Admin step-up and IdP login are separate**, so the IdP's `max_age`/`prompt=login` behaviour isn't load-bearing for security (§ Admin).
- Workspace sessions: `__Host-pf_session`, `HttpOnly; Secure; SameSite=Lax`, stored hashed server-side, 30 days sliding, with "sign out everywhere" available.

### Who can see a part: the audience ladder

Each part (and each space, as a default) has **grants** `{subject, role}`. Roles: `view` < `comment` < `edit` < `manage`.

| Audience | Subject | Notes |
| --- | --- | --- |
| **Only me** (default) | `user:chris` | Every new part starts here unless its space says otherwise |
| Specific people | `user:<id>`… | Picked from the directory, or invited |
| A group | `group:<household>`, `group:pf-photofinder`… | Any IdP group can be *granted*; partifactory can only *create/edit* its own `pf-*` groups |
| Everyone signed in | `everyone-signed-in` | Shown with a live count ("15 people") so it's never vague |
| **Public, link only** | `link:<token>` | No sign-in, unlisted, `noindex`, revocable, optional expiry. **`view` only by default**. `comment` needs an explicit admin opt-in (spam and prompt-injection risk, §3c). Shown in amber as "the worst option" |

There's no listed or indexable public mode. Effective access is the highest matching role. Every change goes to the `audit` table (who, actor, what, before → after).

### Invitations, all minted in partifactory

1. **Passkey-only invite link** (share anywhere): an Authentik invitation bound to a dedicated **`partifactory-invite`** flow:
   invitation → one field ("What should we call you?", username generated) → **passkey enrolment, required** → user write → login → land on the part, already granted.
   There's no email and no password. 7-day default expiry, and `fixed_data` carries the `pf-*` groups. The dialog offers **"for one person"** (single use) or **"for a group chat: up to N people"**
   (multi-use with an explicit count), because single-use links pasted into family chats lose a race and confuse everyone else.
   **Lost passkey** → Chris clicks "Send a new passkey link" on that *existing* person. That's an IdP recovery link bound to the same user, so their history and grants stay put and no zombie account is created.
   Invitees who never enrolled, and enrolled people inactive for 180 days, are listed for one-click cleanup.
   (The invitation → `fixed_data` → user-write group-join plumbing is verified in the P1b spike, not assumed.)
2. **Add by email**: creates the IdP user + `pf-*` groups, then triggers the IdP's "set up your account" email. Click → passkey → on the part.
3. **Existing user**: pick and grant. They're notified by email or Telegram via Hermes.
4. **New group**: "Make a group for this" creates `pf-<name>`, which can be managed from either side afterwards.

Pending invites show their status (sent, opened, enrolled, expired) and have one-click revoke.

### UX: click budgets, tested end to end

Every flow has a budget in **real human interactions, including OS/browser prompts and the biometric**. The e2e suite (Chromium + virtual WebAuthn) enforces the page-level part,
and a manual pass on a **real iPhone (Safari) and an Android phone** before each phase's sign-off covers what a virtual authenticator can't see.

| Flow | Who | Budget |
| --- | --- | --- |
| Open a part I may see, already signed in | anyone | **0**: silent redirects only |
| Sign in after session expiry | returning user | **2 interactions**: pick the passkey from autofill + biometric. No "stay signed in?", no consent |
| Make a part public-by-link | Chris | Share → "Anyone with the link" → **Copy** (+ 1 biometric if the 30-minute step-up window has lapsed. Step-up is in-page WebAuthn, not a redirect) |
| Mint a passkey invite link | Chris | Share → "Invite someone" → **Copy link** |
| Accept a passkey invite | new person | open → type a name → **passkey tap** → on the part |
| Add someone by email | Chris | Share → "Invite someone" → email → **Send** |
| Connect a new agent (OAuth) | Chris | agent opens browser → passkey tap (if signed out) → **Allow** on the consent sheet |
| Revoke anything | Chris | one click, with a 10 s undo toast instead of a confirmation dialog |

**Authentik click reduction**. Shared flows belong to every service on the IdP, so partifactory **gets its own cloned authentication and authorization flows**,
which it owns, provisions and tunes. Shared flows are touched only for changes Chris explicitly wants globally. Global so far: no "Stay signed in?" prompt (applied 2026-10-22 to the
shared login stage at Chris's request, with the before-state in site config). partifactory-owned: implicit consent, passkey-first identification (conditional UI, if the
deployed Authentik's identification stage supports `mediation: "conditional"`, verified in the P1 spike), a no-prompt invalidation flow, and partifactory branding.

**Share dialog**: one sheet, with the audience ladder as a segmented control, a live sentence saying exactly who can see the part now,
an expiry picker, pending invites, agents' share requests as approve/deny chips, and copy buttons. It works on a phone.

### Admin is Chris, and only Chris

- **Pinned to Chris's immutable IdP `sub`** in config, not a group. Being added to any IdP group (including the IdP's own admins) grants nothing here.
- **Step-up is native WebAuthn in partifactory**, not trust in IdP claims. Chris registers an admin passkey in partifactory itself (`webauthn-rs`).
  Admin actions (grants, invites, users, groups, link shares, connecting agents, approving share requests) need a passkey assertion within 10 minutes.
  This works the same whatever the IdP, and a stolen session cookie alone can't administer.
- **The IdP credential is least-privilege, and that's verified, not assumed** (a P1b design gate). The intent: a service account `partifactory-provisioner` that can read users/groups,
  create invitations, send recovery links, and change membership of `pf-*` groups **only** (Authentik object-level permissions on each `pf-*` group).
  The gate tests, against the real IdP, that this account **cannot**: add anyone to a non-`pf-*` group, create a group with `is_superuser`, edit flows or stages,
  or create users with elevated attributes. The core's own `pf-*` check is defence in depth only, because a compromised process would skip it.
  **If the IdP can't scope it that finely**: `pf-*` groups are created by `provision` (bootstrap token, run by Chris), never at runtime. The runtime account drops to
  invitations + recovery links + membership of pre-created groups. If even membership can't be object-scoped, add-by-email and new groups become "Chris approves, `provision` applies".
  The plan's containment claim is only as strong as what this gate shows, and the gate's result is written into site config.
- **Guests can't escalate**: in v1 `manage` means "edit content, comments and data, and see the audit log". **All grant changes go through Chris** (or his approval queue).
  Delegated grant editing is deferred until someone other than Chris actually manages a part.
- **Break-glass**: `partifactory admin break-glass`, run on the host console, prints a one-time admin login valid for 5 minutes. It needs local shell access, works when the IdP is down,
  and is logged and alerted. The admin `sub` pin lives in site config, which is backed up off-host (§5c).

## 3b. Agents: their own OAuth tokens, scoped to what they need

**Do we need MCP?** Yes. It's how Claude Code, Hermes (and Codex, Cursor and others) discover and call tools with no glue code. Underneath it is the **same HTTP API**
the `part` CLI and the Nest use, with the same tokens and the same policy engine. MCP is one front end, not a separate system.

**How agents authenticate: OAuth 2.1, per the MCP authorization spec.** partifactory is both the **resource server** (`/mcp`, `/api`) and a small
**authorization server** that *federates* login to the IdP. This gives every agent a token distinct from your browser session:

```
claude mcp add --transport http partifactory https://parts.dilger.dev/mcp
  → POST /mcp  → 401  WWW-Authenticate: Bearer resource_metadata="…/.well-known/oauth-protected-resource"
  → client reads Protected Resource Metadata (RFC 9728) → authorization server = parts.dilger.dev
  → client identifies itself: Client ID Metadata Document (preferred) or Dynamic Client Registration (RFC 7591)
  → browser: /oauth/authorize (PKCE, resource=https://parts.dilger.dev/mcp per RFC 8707)
       → Authentik login if needed (1 passkey tap)
       → partifactory consent sheet: "Claude Code on chris-mac wants to: read & edit parts in [chris ▾],
          request shares, mint 24 h previews. Expires: [90 days ▾]"   → passkey step-up → Allow
  → token: opaque, audience-bound to /mcp, sub=chris, act=client:<id>, scopes + spaces
```

- **Tokens**: opaque random strings, stored hashed, revocable instantly. Access tokens last 1 h, and refresh tokens rotate (detecting reuse) for up to the consented lifetime.
  They're never JWTs, so there's no key distribution or clock skew to worry about, and revocation is a single row.
- **Scopes**: `parts:read`, `parts:write`, `parts:delete` (soft delete only, restorable for 30 days), `data:read`, `data:write`, `comments:write`,
  `share:request`, `preview:mint`. Plus **space limits** picked on the consent sheet. Agents can never get admin scopes: grants, invites, users, link shares.
- **Delegation is explicit**: every write records `author=chris, actor=hermes`, which the UI shows as "Hermes for Chris". An agent can't see or do more than the person it acts for.
- **Headless agents** (Hermes on its VM, the Nest server, CI): the **device authorization grant** (RFC 8628).
  `part login` prints a code, Chris approves it on his phone (passkey tap + consent sheet), and the agent gets the same kind of scoped token.
- **Fallback for clients with no OAuth support**: Chris can mint a static scoped token on the consent sheet (shown once). It's the same scopes, but marked "static" in the UI and short-lived by default.
- **Abuse limits on the auth server**: client registration is rate-limited, and a registered client is inert until Chris consents. Unconsented registrations expire after 1 hour.
- **Swappable**: an `AuthorizationServer` trait. If the IdP gains client-ID metadata/DCR + resource indicators, partifactory can switch to validating IdP-issued tokens as a pure resource server.
  The facade exists because MCP clients expect DCR/CIMD, and Authentik support for them is unverified.
- **Revocation cascades**: revoking an agent kills its refresh token family, its access tokens and every preview it minted (via `minted_by`).
- **Agents and link previews**: `preview:mint` lets an agent create **view-only, single-part, single-version links lasting ≤ 24 h**. That's a deliberate, narrow exception to "agents never share",
  so Hermes can drop a working link in Telegram. Each preview is logged, listed in the share dialog and revocable, and the scope can be withheld at consent.
  Anything broader is a `share:request` that Chris approves.

| Agent | Grant type | Typical scopes |
| --- | --- | --- |
| Claude Code (Mac, other dev envs) | authorization code + PKCE | `parts:*`, `data:*`, `comments:write`, `share:request`, `preview:mint` in `chris` |
| Hermes (VM, Telegram front door) | device code | the same, in `chris` + `family` |
| Nest (`nest-api`) | device code (once, at setup) | `parts:read`, `parts:write`, `preview:mint` in `family` |
| CI (provision/smoke tests) | device code → token in Gitea secrets | `parts:read` + a test space |

## 3c. Agents editing parts: discoverable, quick, in place

The common case is "change that thing from last week", so finding a part and making a small edit must be cheap for an agent.

- **Discoverable**:
  - `part_find` (FTS5 over titles, descriptions, rendered text, file paths, tags, comments, agent notes) + `part_recent` (recently touched by *me*/*this agent*).
  - **MCP resources**: `part://recent`, `part://<name>` (manifest, summary, outline), resource templates for files.
  - Each part has **agent notes**: a short, versioned "what this is / how it's built / gotchas" field the agent keeps up to date, returned first by `part_get`.
    The next agent starts from the notes, not from re-reading every file.
- **Read only what's needed**: `part_get` returns the outline (files, sizes, symbols) by default. `file` + `range` fetch a slice.
- **Quick edits**: `part_edit` takes `edits[]` of `{file?, old_str, new_str}` across files, atomically.
  If `base_version` is stale but the edits still apply cleanly to the latest version, the edit is **rebased automatically**. Otherwise it fails with the conflicting hunks.
  It returns the new version, compile warnings and a fresh preview link. Target p99 < 10 ms.
- **Errors close the loop**: viewers' browsers report uncaught errors and blocked loads, which `part_get` returns as `runtime_errors` (latest 20 per part).
- **In place, like Claude's artifacts**:
  - The workspace has a source editor (CodeMirror 6) with live preview, where saving creates a version, and drafts survive reloads.
  - **Persistent state**: `window.storage` (personal + shared) and `claude.use("db")` documents live on the server. They're the part's state across versions and devices, agents can read and write them (`part_data`), and they're backed up like everything else.
  - **Self-editing parts**: `claude.use("artifact").publish(...)` lets a part save a new version of itself (a doc edited in place, a checklist that rewrites its own source),
    using compare-and-set against the version it's running. Viewers with `edit` can do it, while link viewers never can.
- **Prompt-injection hygiene**: tool results label content by provenance. Comments and data written by link viewers or other people are wrapped as
  `untrusted_content` in MCP results, and the server instructions tell agents to treat them as data, not instructions.

## 3d. Creation and concurrency: the contract

**What the others do, for reference.**
- *shartifacts*: optimistic concurrency. Every update sends `base_version`, and a stale one fails with "reload the latest version before saving". It holds an in-process lock per artifact,
  has cooperative **leases** (`acquire` with holder + TTL) on its `db` documents, a "conflict" notice when a browser draft is overtaken by an agent save, and two-step deletes with a confirmation token.
- *Claude's artifacts*: also optimistic, not locks. Edits are `{old_str, new_str}` pairs, where each `old_str` must occur exactly once and a batch applies all-or-none.
  A publish that loses a race is refused and handed the newer version to merge onto. Per-file compare-and-set uses `ifMatch` sha256s. Coordination between viewers uses `db` `acquire` leases.
  Claude Docs uses atomic operation batches with idempotency ids (`opId`) and `pending` placeholder blocks that are filled in later.
- Nobody uses pessimistic locks on source text, and partifactory doesn't either. **Optimistic by default, leases when coordination is worth it.**

**Creation contract: a link right away, robustly.**
- `part_create {name?, title, kind?, space?, request_id}` returns `{id, name, url, version: 0}` **in < 10 ms**, before anything is built. The name is reserved by a unique constraint
  in the same transaction, and the URL works immediately, showing a "being made…" placeholder that live-updates (SSE) when the first version lands.
  The agent can hand you the link *first* and build second.
- **Idempotent**: `request_id` (client-chosen) makes retries safe. A timed-out call that's retried returns the same part, never a duplicate.
- **Durable when acknowledged**: a call returns only after the manifest commit is fsynced (SQLite WAL, `synchronous=FULL` for manifest commits) and its blobs are stored.
  Off-host durability follows within seconds (§5c). "Acknowledged" means it survives a power cut.
- **Saves never block on compiling**: the source version is committed first, then TSX/bundle/precompress runs and the compiled output is attached to *that* version.
  `wait: true` (the default for agents) waits for compile warnings, up to 2 s. `wait: false` returns at once, and the warnings arrive as an event.
  The live URL only switches to a version whose compile succeeded, so a broken save never replaces a working page. The agent sees the errors instead.

**Concurrency contract: several agents (and people) on one part.**
- **Linearisable writes**: one writer task serialises all commits, so every part has a strict version sequence `n, n+1, …` with no lost updates.
- **Anchor-based edits compose**: `part_edit` edits find their target by content (`old_str`), not by line number. Two agents editing *different* parts of a file both succeed:
  if `base_version` is stale, the server re-applies the edits to the latest version, and they apply as long as each anchor is still present exactly once.
  An edit fails only when its anchor itself changed, and the error returns the *current* text around it so the agent can redo just that edit.
- **Whole-file replacement** (`files: {path: content}`) is compare-and-set per file (`if_match: <sha256>`, as in Claude's `publish`). It's never a blind overwrite.
- **Leases for longer jobs**: `part_claim {scope: part | file | block, ttl ≤ 10 min, holder}` is advisory, like Claude's `db.acquire`. Others' edits to a claimed scope get
  `claimed_by: Hermes until 14:32`, and a person can always override. Leases auto-release on commit or expiry, and the UI shows them ("Hermes is editing *app.tsx*").
  Use them for multi-step refactors and "you take section 2, I'll take section 3" splits.
- **People and agents together**: the workspace editor keeps a local draft. When a new version lands mid-edit, the draft is rebased onto it the same anchor-based way
  (non-overlapping changes merge silently). Overlaps show a side-by-side resolve. Nobody's work is silently discarded, and "force" exists only for the person, never for agents.
- **Data** (`window.storage`/`db`) has per-document compare-and-set plus leases, exactly like Claude's `db`, and is separate from source versioning.

**Long documents** (a research write-up, a spec, a report).
- `markdown` and `slides` parts get **stable block ids**, kept in a sidecar map, not in the visible text. Agents edit by block: `replace_block`, `insert_after`, `append_to_section`.
  Block edits are independent, so parallel agents on different sections never conflict, and each block edit is a tiny commit.
- **Outline-first, fill-in-parallel**, like Claude Docs' `pending` blocks: an agent creates the outline with `pending` sections ("Methods: what we measured and how"),
  shares the link at once, and one or more agents each claim a section and fill it in. The page renders pending sections as placeholders that fill in live.
- `part_get` on a long doc returns the outline + block ids, so agents read the section they need rather than the whole document.

**Do we need CRDTs?** Not for v1. CRDTs (Yjs/`yrs`, Automerge) are production-ready, not just research, but they solve *simultaneous keystroke-level typing by humans*.
Agents don't type keystrokes. They submit whole edits, which versions, anchors, blocks and leases handle with far less machinery. If live human co-typing in the workspace editor
is ever wanted, `yrs` (Rust Yjs) can run *inside one editing session*, with each session's result committed as an ordinary version. The storage model doesn't change.

## 4. Surfaces

### MCP tools

| Tool | Does |
| --- | --- |
| `part_create` | reserve a name and get a live link in < 10 ms, before building (§3d). Idempotent via `request_id` |
| `part_claim` | advisory lease on a part, file or block (≤ 10 min) |
| `part_put` | create a part, or replace it wholesale: `name`, `content`/`files`, `kind`, `title`, `space`, `tags`, `notes`, `description` |
| `part_edit` | the fast path: anchor edits across files, per-file `if_match` replacements, block edits for long docs; `base_version` with auto-rebase (§3c, §3d) |
| `part_get` | outline + notes by default; `file`/`range`, `version`, `history`, `diff_from`, `runtime_errors` |
| `part_find` / `part_recent` | search + filters (`kind:`, `tag:`, `space:`, `has:comments`, `updated:`), recent by me/this agent |
| `part_name` | check a semantic name and get ranked alternatives |
| `part_preview` | mint a ≤ 24 h view-only link (`preview:mint`) |
| `part_share_request` | ask Chris to grant an audience; returns a pending request id |
| `part_comments` | list/add/resolve; `to_agent` threads; provenance-labelled |
| `part_data` | the part's `db`/`storage` documents (Claude ArtifactData-shaped) |
| `part_watch` | long-poll the event log since a cursor |
| `part_restore` | restore an older version, or undelete within 30 days |
| `design_system` | rules + starter, so new parts look consistent |

MCP server instructions sent at init cover names, notes, quick edits and the design system, so "make me a packing checklist in partifactory" is enough.

### Names: semantic, chosen by the agent

A part's name *is* its hostname (`<name>-part.dilger.dev`).

- `part_put` takes an optional `name`. Without one, it's derived from the title ("Max's sleep chart" → `max-sleep-chart`).
- `part_name {want?, title?, description?}` → available? If not, ranked semantic alternatives that are valid and free right now.
- A clash returns `name_taken` **with** suggestions, so the agent retries in one step.
- Rules: `[a-z0-9-]`, 1–58 chars, no leading/trailing/double hyphens. Reserved: infrastructure words and names that mirror existing services (the list is in site config).
- **Renames are rare and admin-confirmed**: the old hostname 301s (query string stripped) for good, and **a name that was ever used stays reserved**, so a future part can't collide with an old redirect.
  The confirmation says plainly that installed home-screen apps and device-local storage on the old name stop working, while server-side state is unaffected.

### `part` CLI

`part login` (device code) · `part put ./dir --name …` · `part edit <name> --replace 'old' 'new'` · `part get <name> [--file f]` · `part find …` · `part preview <name>` · `part watch` · `part open <name>`.
It's the same HTTP API and tokens as MCP. Credentials live in the OS keychain where available, otherwise in `~/.config/partifactory/` (mode 600).

### Events out

An append-only `events` table drives SSE (UI, with a 25 s heartbeat so Cloudflare never idles the stream out), `part_watch` (agents, ≤ 50 s long-poll) and **outbound webhooks** to admin-configured URLs only (Hermes → Telegram),
signed with HMAC and retried with backoff. Webhook targets are site config, never set by agents (that rules out SSRF).

### Workspace UI (`parts.dilger.dev`)

Library (search-first, `/` to focus), part view (Preview / Split / Source with in-place editing, version scrubber, diff), comments rail, share dialog,
connected agents (scopes, last used, revoke), and an admin area (people, invites, audit, health). Phone-first.

### Telegram (through Hermes) and the Nest

Hermes owns Telegram. It uses MCP + a `partifactory` skill, sends preview links back to chat, and relays comment webhooks with a "fix it" path.
The Nest embeds family parts (same-site iframe, no cookie tricks needed) and its Hermes profile can create parts in `family`. The Nest's knowledge base stays separate from partifactory.

## 5. Stack: Rust, all the way down

One Cargo workspace builds **one static binary** (`partifactory`, with `part` as a CLI alias) that has every asset embedded.

| Concern | Choice |
| --- | --- |
| HTTP | `axum` + `tokio` + `tower` (three listeners: workspace, content, ops) |
| Storage | SQLite via `rusqlite` (bundled, FTS5, WAL). No storage-backend abstraction until there's a second backend |
| Blobs | `BlobStore` trait: local dir in v1, R2/S3 as an implementation |
| TSX/JSX → JS | `oxc` (transform + minify) + `rolldown` (bundle), in-process, cached by manifest hash. Versions pinned, with a **golden compile corpus** (shartifacts' compat-matrix artifacts + Claude-written samples) run on every upgrade |
| Markdown / code | `comrak` + `syntect` server-side; Mermaid and KaTeX client-side |
| MCP | `rmcp` (official Rust SDK), Streamable HTTP, stateless |
| People auth | `openidconnect` (OIDC client), `webauthn-rs` (admin step-up) |
| Agent auth | OAuth 2.1 AS/RS: authorization code + PKCE, device code, CIMD/DCR, RFC 9728 metadata, RFC 8707 resource binding. Small and hand-written over well-tested primitives, with a conformance test suite |
| Capabilities | compact HMAC-signed tokens (`ring`), versioned keys (`kid`) |
| Workspace UI | Leptos (SSR + hydration), CodeMirror 6 for the source editor (the one JS dependency, vendored as a shared blob) |
| Observability | `tracing` (JSON logs), `metrics` + Prometheus exporter, optional OTLP |
| Tests | `cargo nextest`, golden tests (manifests, diffs), property tests (grant A never reads B), OAuth conformance, browser e2e with virtual WebAuthn against `*.localhost` |

Maturity check before P0: `rolldown` as a library, `rmcp` Streamable HTTP and Leptos SSR each get a one-day spike, and esbuild-as-subprocess is the fallback for the bundler.
**Bend 2** was considered and set aside: this service is I/O-bound, and Bend's strength is parallel pure computation.

## 5a. Performance: numbers, measured honestly

partifactory handles a **reduced subset** of HTTP: a fixed route table, fixed responses (immutable, precompressed, ETag = hash) and no upstream hop.
Doing less work per request is how it can beat general-purpose proxies on *this* workload. On raw static files nginx's `sendfile` is near the syscall floor, so the target there is parity.
**Most requests never reach the NAS**: shared libraries come from Cloudflare's edge.

- **Work at save time**: compile, render, minify and precompress (**brotli only**: every browser supports it, and zstd isn't worth doubling the blob store) when saving, so serving is negotiation + a file read.
- **Lean hot path**: capability check = one HMAC verify (no DB), 304s answered from the ETag alone, hot small blobs in an mmap'd LRU.
- **SQLite for reads**: WAL, mmap, a read pool + a single writer task, cached statements, in-memory SSE broadcast.
- **One runtime: tokio.** A thread-per-core io_uring runtime is parked as an idea, to be revisited only if a profile shows the runtime is the bottleneck, which at this load it won't be.
- **Honest about zero-copy**: hyper's default body path copies through userspace. v1 uses mmap'd LRU + `pread`, which is plenty. Real `sendfile(2)` plumbing is optional work, and it only helps the loopback hop.
- **Serving a part is two steps**: an in-memory `part → live version` cache (invalidated from the events stream) resolves the entry without a DB read.

**How it's gated, so the numbers mean something**: absolute targets are *tracked*. Release gates compare against the previous release **on the same machine**
(the deploy host's bench job runs off-hours), failing on a >10 % regression. Shared CI runners are too noisy to gate on microseconds.

| Metric | Target (tracked) |
| --- | --- |
| Part blob from origin, p99 server time (warm) | < 200 µs |
| Grant hand-off round trip (code exchange), p99 server time | < 1 ms |
| `part_find` over 10k parts, p99 | < 5 ms |
| `part_edit` small edit, no compile, p99 | < 10 ms |
| `part_edit` TSX part incl. compile + bundle + brotli, p50 / p99 | < 150 ms / < 1 s |
| Repeat open of a part from a phone, end to end (entry HTML always crosses the tunnel; shared libs from the edge) | < 150 ms on a good uplink, measured from real devices and reported, not gated |
| Static-file throughput vs nginx and HAProxy on the same box and files | ≥ parity |

## 5b. Builds and deploys

- **Parts need no build infrastructure.** TSX/CSS/Markdown compile in-process at save time, CDN references are mirrored on save, and a part that needs real npm packages
  is built by the agent locally (`vite build`) and uploaded as files. Nothing to install, and no containers per part.
- **partifactory itself is a Docker image**, built by Gitea Actions: `cargo-chef` layer caching → static musl binary → `FROM scratch` image (~20 MB),
  amd64 (+ arm64 for other hosts), with an SBOM, signed with `cosign`, pushed to the Gitea registry and **deployed by digest**, never `:latest`.
- **Deploy = pull + start + health-gate**: the deploy job (protected runner, Gitea secrets) runs `partifactory provision` (reconcile Cloudflare/Authentik),
  then starts the new container next to the old one, waits for `/readyz`, switches, and **rolls back automatically** if health fails within 2 minutes. Schema migrations run before the switch and must be forward-compatible with the previous binary.
- **Day to day**: `cargo run`/`bacon` against `*.localhost`. `part` CLI binaries ship through GitHub Releases (the mirror) + `cargo binstall`.
- **If backends ever happen**: WASM components on `wasmtime` first, a container per app only as a fallback.

## 5c. Disaster recovery and self-provisioning

**From nothing to running with one credential set**: a Cloudflare API token, an IdP bootstrap token, and a Docker host it can reach.

- **`partifactory provision`** is an idempotent **create-if-missing / update-if-owned** script driven by `site.toml` (private site config). It's honest about being a script rather than Terraform:
  it only manages resources it owns (named `partifactory-*`), so reapplying is safe, and it never "reconciles" shared IdP flows. A diff/plan mode comes later, if it's ever needed.
  - Cloudflare: its own tunnel + ingress, the `parts` record and the `*` wildcard (created only after `/readyz` passes), cache rules (shared blobs only), WAF rate-limit rules.
  - IdP: the OIDC provider/app, the `partifactory-invite` flow, login-stage tweaks, the `partifactory-provisioner` account + RBAC role.
    It uses Authentik blueprints if they can be applied over the API, otherwise direct API calls. Which one is to be confirmed in P1.
  - `--dry-run` shows a diff, every before-state is recorded, and it only ever deletes resources named `partifactory-*` that it created.
- **Backups, continuous and off-host**: Litestream streams SQLite to R2 with **client-side encryption** (age), and blobs sync to R2 append-only (encrypted, content-addressed).
  **Ordering**: a blob is written locally *and* acknowledged by R2 before any manifest referencing it commits, so a point-in-time restore never points at missing blobs. Restore verifies hashes.
  RPO is seconds.
- **Restore is lazy, so RTO is real**: `partifactory restore --from r2://…` restores the DB first and starts serving, pre-fetches blobs for the newest parts,
  and fetches the rest from R2 on first miss. RTO < 15 min regardless of how much history exists.
- **Identity is part of DR**: partifactory's grants point at IdP users. **The IdP's database (Authentik's Postgres) is in the site's off-host backup set** (site ops, documented in site config),
  so a full-loss restore brings back the same users, passkeys and `sub`s. Degraded mode, if the IdP can't be restored: `partifactory export-grants` (keyed by email/username, kept with backups)
  plus a re-invite runbook that rebinds grants as people re-enrol.
- **Drills run from a laptop**, not CI, because Gitea (and its secrets) may be on the host that died. The monthly drill uses a released binary + the three credentials + site config from the offline copy,
  restores into a throwaway container and runs the e2e suite. A CI drill also runs, but the laptop drill is the one that counts.
- **Host cutover rotates the tunnel connector token**, so a zombie connector on the old host can't take traffic. The wildcard DNS record is created only after `/readyz` passes
  through the tunnel (probed via the tunnel's `cfargotunnel.com` name).
- **Any Docker host**: deploy targets are adapters (`docker` over an SSH Docker context, `truenas` via its middleware). Moving = change `deploy.target`, then `restore` + `provision`.
  The hostnames never change.
- **Secrets, only in Gitea Actions**: `PF_CF_API_TOKEN`, `PF_TUNNEL_TOKEN`, `PF_IDP_BOOTSTRAP_TOKEN`, `PF_IDP_PROVISIONER_TOKEN`, `PF_SESSION_KEY`,
  `PF_CAP_HMAC_KEY`, `PF_BACKUP_AGE_KEY`, `PF_R2_*`, `PF_DEPLOY_SSH_KEY`. **The backup age key also has an offline copy** (password manager), or encrypted backups are unrecoverable after losing Gitea.
  The GitHub mirror is passive: no Actions, no secrets.
- **Rotation**: rotate in Gitea and re-run the pipeline. Capabilities are short-lived (≤ 10 min, previews ≤ 24 h), so a rotated HMAC key is kept for 25 h, then dropped.
  Link shares are random tokens looked up in the DB, not HMAC-signed, so rotation never affects them.
- **Site config itself** (`site.toml`, the admin pin, the backup age key) has an offline copy in the password manager. Without it, nothing else is recoverable.

## 5d. Operations: logs, metrics, analytics, abuse, maintenance, scale

**Logs**: structured JSON to stdout (`tracing`), with one line per request carrying the `cf-ray` id, route, status, latency, principal/actor (never tokens, cookies or part content).
Secrets are redacted by type at the logging layer, not by convention. Log levels change at runtime from the admin area.

**Metrics** (`:9090/metrics`, not routed through the tunnel). These include **hits on non-`-part` hostnames** caught by the wildcard, because a future service whose DNS record is missing shows up here first: request rate, errors and latency per route class, grant decisions, OAuth events, compile times,
blob store size, cache hit ratio (via Cloudflare's analytics API, polled), SQLite WAL size, backup lag (Litestream), last restore-drill result.

**Health and alerting**: `/healthz` (process up), `/readyz` (DB writable, blob store OK, backup lag < 5 min, IdP reachable).
An admin **Health** page shows the same, plus disk headroom and version. Alerts (backup lag, disk > 80 %, error-rate spike, failed restore drill, IdP unreachable)
go through a `Notifier` trait: a webhook to Hermes → Telegram by default, `ntfy` as an option. A Cloudflare health check on `/healthz` catches "the whole box is down".
When the origin is down, a tiny static Cloudflare Worker fallback page says "partifactory is down for maintenance" instead of a raw 1033.

**Analytics, first-party and privacy-respecting**: views per part per day, unique viewers (signed-in = principal; link viewers = a daily-salted hash of IP + UA, never stored raw),
referrer host for link shares, and last viewed. They're shown on each part's details and in an admin overview ("what's actually used"), and agents can read them via `part_get`.
There are no third-party scripts and no cross-part tracking. Raw view events are kept for 90 days, then rolled up.

**Abuse prevention**:
- **Rate limits** per IP and per principal (`governor`) on `/grant`, `/oauth/*`, `/l/*`, `/mcp`, `_rt` writes, invite acceptance and comments, mirrored by Cloudflare WAF rules at the edge.
- **Body and size caps**: 2 MiB per source file, 25 MiB per media blob, 8 MiB per version, 256 KiB per db document, and request bodies are capped before auth on unauthenticated routes.
- **Quotas** per space and per agent token: parts, versions/day, storage. An agent that loops hits a quota, and the event is surfaced as an alert, not discovered when the disk fills.
- **Invites**: single use, expiring, a cap on outstanding invites, and enrolment is rate-limited per invite and per IP.
- **Link shares**: view-only by default, per-link request caps, a kill switch ("revoke all link shares") in the admin area, and `noindex` + `X-Robots-Tag` everywhere.
- **Mirror-on-save fetches**: CDN allowlist only, size and time caps, no redirects off-allowlist, so saving can't be used to make requests to internal addresses.

**Maintenance**:
- **Blob GC**: mark-and-sweep from all manifests (every version of live and soft-deleted parts) plus db/storage references. Unreferenced blobs are deleted after a 7-day grace period,
  and R2 copies follow 30 days later.
- **Retention**: every version manifest is kept forever (they're tiny). An optional per-space policy can drop old versions' unique blobs beyond N versions or X days, and a version is never dropped while a link pins it.
- **Migrations**: forward-only, embedded, run at start inside a transaction, tested by the restore drill against a production-shaped backup.
- **Dependencies**: Renovate on Gitea, plus `cargo audit` + `cargo deny` (licences, advisories) in CI, base image pinned by digest.
- **`partifactory doctor`**: checks config, DNS, tunnel, IdP reachability, provisioner permissions (expects 403 on a forbidden group op), backup freshness and disk.

**Scale, honestly**: one node by design. Expected load is a few people and agents, about 10k parts and ~100 GB of blobs over years, which is far inside
SQLite (a single writer handles thousands of small writes/s) and one disk. The shapes that would force a change are many concurrent writers or more than one host,
and neither is a goal. Blobs moving to R2 as primary storage is the first lever if the disk is ever the limit.

## 6. Phases

Each phase ends with something usable, and has its own "Done when".

**P0 Core (local).** Repo, schema + migrations, blob store, manifests, `part` CLI, HTML/Markdown/SVG/code kinds, content listener with capabilities,
mirror-on-save, precompression, logs + metrics, `bench` with the nginx/HAProxy fixture. One-day spikes on rolldown, rmcp and Leptos. P1 spike on Authentik: conditional UI, blueprint-over-API, OIDC `groups` claim.
*Done when:* `part put x.html --name packing` opens at `packing-part.localhost:8081`, and the bench numbers are recorded.

**P1 Edge + identity.** Own tunnel + wildcard via `provision`, OIDC login, grant hand-off (one-time codes), policy engine + audience ladder,
admin pin + native passkey step-up, link shares, minimal workspace UI (library, view, versions, share dialog), rate limits, health page, backups + first restore drill, CI deploy.
*Done when:* Chris opens a part on his phone via SSO, a link share works from mobile data with no login, and the security checklist (§7) passes from outside.

**P1b Invites.** **First, the provisioner permission gate** (§3a), which decides how much of the following ships. Then the `partifactory-invite` flow, passkey invite links (single and multi-use), add-by-email,
"new passkey link" for existing people, `pf-*` groups, pending-invite cleanup.
*Done when:* Chris invites a new email address from the share dialog, they enrol with a passkey and land on the part, and removing them in the IdP locks them out within a minute.

**P2 Agents.** OAuth AS (auth code + PKCE, device code, CIMD/DCR, consent sheet), MCP tools + resources + instructions, agent notes, `part_edit` with rebase,
comments with provenance, events/SSE/webhooks, Hermes skill.
*Done when:* Claude Code connects with one passkey tap + Allow and edits a week-old part in one call, and "Hermes, make me a meal-plan page" in Telegram → preview link → comment → Hermes fixes it.

**P3 Claude compatibility.** React/TSX with the common libraries + shadcn/Tailwind, `window.storage`, `claude.use` (`db`, `user`, `comments`, `artifact` incl. self-publish), runtime error reporting.
The spec is shartifacts' compatibility matrix (credited).
*Done when:* a page copied from a claude.ai artifact runs unchanged, including state persistence and self-editing.

**P4 Family + Nest.** The `family` space, family onboarding, Nest embed, Hermes `nest-family` profile, slides + mermaid, analytics overview.

**P5 (only if needed) backends.** WASM components, through the same grant model.

## 7. Security checklist (run from outside the network, in CI against staging)

- No session → IdP login. A part host without a grant → `/grant`, never content. `/grant` can't redirect off the part's own host.
- Grant codes are single-use and expire in 30 s. Capabilities are host-bound (a cookie for A is ignored on B). Revoked link → 404 within seconds.
- Private part blobs are never in Cloudflare's cache (`cf-cache-status` ≠ HIT), and the shared blob host serves only shared-class blobs.
- Only Chris's `sub` reaches admin. An IdP-admin user still gets 403. Admin without a fresh partifactory passkey assertion → step-up.
- The provisioner gets 403 from the IdP on a non-`pf-*` group, and the core refuses before calling.
- Agent tokens: audience-bound (a token for `/mcp` is rejected elsewhere), scope- and space-limited, no admin scopes obtainable, refresh reuse → the whole token family is revoked.
- Unconsented OAuth clients can do nothing. Registration is rate-limited.
- CSP on every part response: `default-src 'self' https://blobs-part.dilger.dev; form-action 'self'; frame-ancestors https://parts.dilger.dev https://nest.dilger.dev`. There are no third-party origins.
- The origin is reachable only through the tunnel. `:9090` is never routed.
- No tokens, cookies or part content in logs (tested with canary values).
- Rate limits trip at the configured thresholds, and quotas stop a looping agent.
- `/_rt/*` rejects requests without `X-PF-Cap` or with `Sec-Fetch-Site` ≠ `same-origin`. A sibling part's page can't write another part's data.
- Revocation via grant epoch: a revoked person, link or agent fails on their very next request, with no DB read on the hot path.
- Telegram/Slack unfurlers fetching a share link get the content-free card only.
- Provisioner gate (P1b) results are recorded: no non-`pf-*` membership, no `is_superuser` groups, no flow or stage edits.
- Real-device pass (iPhone Safari, Android Chrome) for: standalone open, Nest embed, invite acceptance, sign-in.

## 8. Decisions (2026-10-22)

1. **Hostnames**: `parts.dilger.dev` + `<name>-part.dilger.dev` via a `*.dilger.dev` wildcard, with semantic names. The part suffix is configurable.
2. **Stack**: Rust. Bend 2 considered and set aside.
3. **Host**: any Docker host, TrueNAS by default. The app knows nothing about TrueNAS/ZFS.
4. **Code reuse**: fresh, with shartifacts as the reference spec, credited.
5. **Docker**: for deploying partifactory only. Parts need no builds.
6. **Tunnel**: its own Cloudflare tunnel `partifactory` (created 2026-10-22, routes configured, no DNS records yet).
7. **Identity**: Authentik via OIDC, swappable. In-app grants and invites. Admin pinned to Chris with native passkey step-up.
8. **Agents**: OAuth 2.1 per the MCP spec, with partifactory as a federating authorization server. Per-agent consented, scoped, audience-bound tokens.
9. **Source of truth**: private Gitea repo + passive public GitHub mirror. Site config in a separate private repo. Secrets in Gitea Actions only.

## 9. Non-goals (v1)

Multi-tenant SaaS. Multi-node. Per-part backends. Offline mode. Listed or indexable public parts. Replacing the Nest's knowledge base.
Public sign-up. A dedicated Telegram bot (Hermes is the bot).

## 10. Changes from the fresh-eyes review (2026-10-22)

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
| O1 IdP traits are YAGNI | **Partly rejected**: Chris asked for swappability. Kept thin, with one real adapter + a test fake |
| O2 io_uring + frankensqlite | **Adopted**: both cut (parked as ideas) |
| O3 webhook + poll | **Adopted**: poll only |
| O4 reconciler/adapter matrix too heavy | **Partly adopted**: an honest idempotent script, owned resources only, no plan mode in v1. Multi-host stays (Chris wants DR and portability) |
| O5 `manage` grant editing | **Adopted**: grants are Chris-only in v1 |
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
