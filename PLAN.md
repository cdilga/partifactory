# partifactory — plan

> A home for the things Chris, his family and their agents make: pages, React tools,
> docs, decks, diagrams. Each one is a **part**, with its own host and a name that means something (`photofinder-part.dilger.dev`), and that host is the link you post.
> Every part runs sandboxed in an opaque origin with no cookies, every view is a grant, and every version is a manifest of hashes.
> Any AI can find and use it, through MCP, an HTTPS JSON API, a CLI or a skill file, each with its own scoped, revocable identity.
> Serving is built to be fast and is measured against nginx and HAProxy.

**Credit.** This project takes its idea from **[shartifacts](https://github.com/LaneBirmingham/shartifacts)
by Lane Birmingham (birmo)**, MIT-licensed. It's a reimagining for a different setup, not a fork.
Any code or text adapted from it keeps Lane's MIT notice in `NOTICE` and gets a credit in the README.

**Site config is private.** Everything specific to one deployment (IdP inventory, flow and stage names, group names, account facts, deploy target)
lives in a separate private repo (`partifactory-site`). Secrets come from environment variables or files, whichever the host or CI supplies (§5b). This plan uses `dilger.dev` as the **example deployment**: every hostname is config (`[domain]`), and dilger.dev is public DNS anyway. Nothing else site-specific is here.
People in the household are "a family member" here. Chris is named because he's the owner and the only admin.

**Portable by design.** partifactory takes no hard dependency on the stack its first deployment happens to run (Cloudflare, Authentik, Gitea, TrueNAS, Telegram/Hermes, the Nest). Each of those is an **adapter behind config** with a generic, dependency-free default: a plain reverse proxy, generic OIDC, any CI or none, a folder for backups, a generic webhook.
Where this plan describes Cloudflare or Authentik behaviour, it's describing the adapter. §5f has the table, and CI proves it by running the full e2e suite with none of them (T93).

**How to read this plan.** §1–§3e say what partifactory is and how its core works. §4–§4c are the surfaces people and agents touch. §5–§5f are the stack, operations, the data model and portability.
§6 and §6a are the phases and the task graph. §7 is the security checklist, §8 the decisions, §9 the non-goals, and §10 the change logs. Every architectural choice has a **Why** next to it.

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

What doesn't fit Chris's setup:

| shartifacts assumes | Chris's reality | So partifactory… |
| --- | --- | --- |
| Trust comes from the **network** (LAN/tailnet IPs). Sign-in is optional | Everything comes in through a reverse proxy or tunnel (a **Cloudflare Tunnel** today), so every request comes from the proxy and IP trust means nothing | Trust comes from **identity**: an OIDC client of **any** provider (Authentik in the example deployment), a per-part policy engine and in-app invites (§3a) |
| One owner password, "Who are you?" is a name picker | The IdP already knows the household | Real principals: people from the IdP, agents acting *on behalf of* one person, each with its own connection (§3b) |
| Static all-powerful agent tokens, which can share publicly | Agents read untrusted web content and can be prompt-injected | **Scoped, consented, space-limited agent identities**. Agents can share *inward* to people who already have accounts. They can never make public links or invite outsiders (§3b) |
| Anyone on the LAN sees everything | Reachable from the internet, with per-person audiences | **Sharing is a security boundary.** Audiences per part and per space, with a live sentence saying who can see it (§3a) |
| Opaque-origin `sandbox` iframes + postMessage bridge + hub-managed localStorage + HMAC app sessions, all on one host | `*.dilger.dev` is covered by the free cert, and parts share a site with `auth`, `ha`, `git`, `nest` | **Each part gets its own hostname (`<name>-part.dilger.dev`) *and* an opaque origin**. Every part document is served with CSP `sandbox` (no `allow-same-origin`). Part hosts never set or read cookies. The hostname scopes the CSP, the capability and the edge rules (§2) |
| MCP is the only agent door | Chris wants any AI, on any surface, to be able to use it | **One operation registry** rendered as MCP, an HTTPS JSON API with OpenAPI, the `part` CLI and a downloadable skill file (§4b) |
| JSON-file-per-artifact holding every revision, a hand-written search index | — | **SQLite** (metadata, FTS5, comments, db docs, events, audit) + a **content-addressed blob store**, both under `/data` |
| Full-stack apps with a Go runner sidecar + Landlock | That's the heaviest third of the codebase | **v1 has no backends.** The per-part `db` covers most micro-apps. WASM backends later, only if a real need shows up |
| Every library vendored, offline | We want both: no third-party runtime loads, and no hand-maintained vendor list | **Mirror-on-build**: CDN URLs in a part are fetched once at build time, pinned by hash and served from our own blob host (§2) |

## 2. The core idea

Five concepts. Everything else is built from them.

1. **Blob**: bytes, named by `sha256`, stored once (`/data/blobs/ab/cd/<hash>`). Whether a blob is shared or private is a property of the **reference**, not of the blob:
   - **Shared blobs**: third-party libraries, fonts, the runtime prelude, the part-host wrapper and Jig. They're listed in `shared_blobs`, aren't private to anyone, are served `public, immutable` from `blobs-part.<domain>/b/<hash>` (so any CDN in front may cache them forever), and are cached once across all parts.
   - **Part blobs**: a part's own files, build output and media. They're served **only from that part's host**, under the part's capability, and only if the part's live (or requested) build references them.
     Browsers cache them `private, immutable`, and **no shared cache may hold them**, so revoking access really revokes it and nothing private sits in a CDN.
   - *Why per reference:* one hash can be both (an agent uploads its own copy of `react.production.min.js`). Serving rules then follow what refers to it, not an accident of upload order.
2. **Part version**: an immutable **source manifest** `{kind, entry, files: {path → {hash, size, mime}}, meta, parent, author, actor, created_at}`.
   Its hash (which includes the part id, `parent` and `n`) is the version id, and `n` is the readable counter agents use. A one-line edit to a 40-file part stores one new blob plus one tiny manifest.
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
- **Import maps** for HTML parts with `<script type="module">` and for TSX bundles are **frozen into the build record**, so a later release never changes which library an old part loads. `part_edit {rebuild: true}` opts a part into the current set.
- **Failure**: the source version still commits, the build ends `failed` with `{code: "mirror_failed", url, reason: "not_allowlisted" | "private_address" | "too_large" | "timeout" | "integrity_mismatch" | "http_<status>"}`, and `live` doesn't move.

### Kinds

Detected from the entry file. `kind` overrides. **Every kind is built**, so `live` only ever moves through a successful build.

| kind | entry | build work | served as |
| --- | --- | --- | --- |
| `html` | `index.html` | mirror CDN refs, sandbox rewrites (root-relative → relative, `crossorigin` on `<img>`), prelude tag, import map, brotli | the HTML + prelude |
| `tsx` | `App.tsx`/`.jsx` (default export) | oxc transform, rolldown bundle against the shared library set, a generated HTML shell (`#root`, Tailwind per §6 P3, prelude) | generated shell |
| `markdown` | `*.md` | comrak render, block ids, Mermaid/KaTeX hooks, Jig styles | rendered HTML shell |
| `slides` | `*.md` with `---` separators | as markdown + the slide runtime (block = slide) | slide shell |
| `svg` | `*.svg` | none beyond brotli (it's sandboxed like everything else) | the SVG as a sandboxed document |
| `code` | any other text file | syntect highlight | read-only viewer shell |
| `files` | a built bundle (e.g. `vite build`) | sandbox rewrites only, no bundling | as uploaded |

A part that needs real npm packages is built by the agent locally (`vite build`, with `base: './'`) and uploaded as `files`.

### Serving a part: sandboxed, cookie-free, capability in the path

**No part host ever sets or reads a cookie.** Every part document (HTML entry, any navigable HTML file, an SVG served as a document) carries the canonical CSP below, including `sandbox` without `allow-same-origin`.
The document runs in an **opaque origin**: `document.cookie`, `localStorage`, `indexedDB`, `caches` and service workers are unavailable to it, and cookie tossing onto `.<domain>` is impossible at the platform level.
Cookies exist only on the workspace host (`parts.<domain>`; `parts.dilger.dev` in the example deployment), for sessions.
- *Why:* agent-written JS is untrusted by default. The sandbox removes the cookie attacks without buying a second domain (§3, decision 10), and it's the same isolation Claude and shartifacts use for artifacts.

**The canonical part CSP.** One header, written once here. §3 and §7 refer to it. `<blob>` is `https://blobs-part.<domain>`, `<self-host>` is this part's own `https://<name>-part.<domain>` (where its wrapper lives), `<workspace>` is `https://parts.<domain>`,
and `<embedders>` is the site-config list `[embed] frame_ancestors` (empty by default; the example deployment lists the Nest).

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
  `payload` is canonical CBOR: `{p: part_id, h: host_label, s: subject ("user:u_…" | "link:l_…" | "conn:c_…" | "render"), r: role (0 view … 3 manage), ro?: read_only,
  e: part_epoch, m: minter ("sess:<id>" | "link:<id>" | "conn:<id>" | "render"), me: minter_epoch, b: boot_epoch, w: window, v?: pinned version_id, d?: draft id}`.
- **Deterministic and windowed**: there's no random nonce. The same subject, part, epochs and 12 h window give the **same cap**, so `private, immutable` part blobs stay in the browser cache across repeat opens.
  A cap is valid until its window ends. A cap minted in the last hour of a window uses the next window, so a fresh cap always has at least an hour left.
- **Verify** (no DB): known `kid` → MAC → `h` equals the request's host label → `e ≥ epoch[part]` → `me ≥ epoch[minter]` → `b ≥ boot_floor` → window not over.
- **Revocation stays instant** through in-memory epochs, persisted in SQLite: per part (any revoke or unshare), per minter (revoking a session, a link or an agent connection), and global.
  A **boot-epoch floor** is bumped on every start and every import, so a restored or rolled-back database can never resurrect a revoked cap. The cost is one silent re-grant per open tab after a deploy.
- **Hand-off**: `/grant?part=<name>&path=<path>&mode=wrap|direct` on the workspace host (path ≤ 2 KiB, must start with `/`; the host is derived from the name, so it **can't act as an open redirect**)
  authenticates with the workspace session (or sends the person to sign in), asks the policy engine and mints the cap. In `wrap` mode (the default, used by the wrapper) it 302s to `https://<name>-part.<domain><path>#pf=<cap>`.
  In `direct` mode (a standalone `/c/` URL that lost its cap) it 302s to `https://<name>-part.<domain>/c/<cap><path>`. There's no one-time code: it existed only to set a cookie.
- **Errors on the part host**: a top-level navigation to `/c/…` with an expired window or a stale epoch → 302 to `/grant?mode=direct` (silent if still allowed). A subresource → `401` with `X-PF-Reauth: 1`.
  An unknown `kid` or a bad MAC → `404`, which looks the same as a part that doesn't exist.
- **Framed expiry**: a framed part never navigates itself to `/grant`. A subframe navigation from an opaque origin is cross-site for `SameSite=Lax`, so the session cookie wouldn't be sent and a login page would render inside the frame.
  The prelude posts `{type: "pf:regrant"}` to the parent. The part's wrapper answers it with a credentialed `POST https://parts.<domain>/api/chrome/v1/grant` (same-site, so the Lax session cookie goes along) and sets a freshly minted `src`, with no navigation.
  The workspace's own frames (editor preview) and an embedder's `pf-embed.js` do the same. Standalone documents reload through `/grant`, since top-level GET navigations do carry Lax cookies.
- **Leaked URLs**: the cap sits in the address bar and history only when someone opens `/c/…` standalone. A URL copied out deliberately gives its holder the copier's role on that one part until the window ends (≤ 13 h), unless an epoch is bumped first.
  The **default human view is the part host's root, `https://<name>-part.<domain>/`**, whose wrapper frames the content, so the address bar there never shows a cap. Logs redact `/c/<…>/` by route and never see the `#pf=` fragment.
- **Signing keys** rotate by `kid`. A retired key is kept 25 h (longer than any cap's life), then dropped. Public links are random DB tokens, not HMAC-signed, so rotation never affects them.

**Runtime calls: `/_rt`, one simple request.** The prelude calls `POST /c/<cap>/_rt/v1` with `Content-Type: text/plain` and a JSON body `{op, args}`.
That's a CORS-simple request, so there's **no preflight round trip through the proxy**. Live events use `EventSource('/c/<cap>/_rt/events')`.

- **Auth** is the cap in the path, bound to this host. The server accepts `Origin: null` (or the part's own origin for `unsandboxed` parts) and rejects any other non-null `Origin` before reading the body.
  If `Sec-Fetch-Mode` is present it must be `cors`. **There is no `Sec-Fetch-Site` check**: requests from an opaque origin are `cross-site` even to their own host, so `same-origin` would reject every legitimate call.
  CSRF is moot because there's no ambient credential to ride. A sibling part can't reach this host (`connect-src 'self'`), and its cap verifies only for its own host.
- `/_rt` ops: `storage.get/set/delete/list` (personal and shared), `db.*`, `comments.*`, `artifact.publish`, `user`, `errors.report`, `refresh`. What each role may do is the role × runtime matrix in §3a.

**The prelude** is a shared blob, first in `<head>` of every built HTML document. It:
- installs `window.storage`, `claude.use(...)` and `window.partifactory` (`ready()`, `basePath`);
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
5. `photofinder-part.dilger.dev` is a name a person remembers and an agent can say in chat, and it's the link itself (below).

The cost that stays: the `*.<domain>` wildcard catches typos for future services, which is metered (§5d).

### The link to post: the part's own host

Agents and people post **one link per part: `https://<name>-part.<domain>/`** (`https://photofinder-part.dilger.dev/` in the example deployment). It follows the live version.

**The part host's root is a small trusted wrapper page.** It's partifactory's own code, served by the content listener, and it is **not sandboxed**. It holds the thin Stockroom chrome (title, audience chip, Share, versions, comments, agent activity, §4a.6)
and frames the sandboxed part content at `/c/<cap>/…` on the same host. The wrapper's script and styles are shared blobs; its HTML is generated per part and carries no part content unless the part has a public link (below).
The wrapper is served at `/` and at any other path outside the reserved prefixes (`/c/`, `/l/`, `/_pf/`), and passes that path on to the frame, so deep links work.

**The flow on first open:**
1. The browser loads `https://<name>-part.<domain>/`. The wrapper looks for a cap in the `#pf=` fragment, then in its own `sessionStorage`. With none, it does `location.replace("https://parts.<domain>/grant?part=<name>&path=<path>&mode=wrap")`.
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
  It's the wrapper's UI backend, like the workspace's own pages, not an agent surface: each chrome call is a thin view over a registry op (`part_share`, `part_comments`, `part_get`), so nothing exists only there.
- **CSRF**: chrome-API writes require `Sec-Fetch-Site: same-site` (or `same-origin`) and an allowed `Origin`; a `Content-Type: application/json` body forces a preflight, so a plain cross-site form can't reach them.
- *Why this is safe:* only the wrapper's own code can send a request whose `Origin` is a sandboxed part's host. The part's content runs in an opaque origin, so its requests carry `Origin: null`, which the workspace refuses (§2), and it can't script the wrapper.

**The part host still never sets or reads a cookie.** The wrapper's state is a `sessionStorage` entry on the part host, and the session cookie stays on the workspace host.
The wrapper has its own strict CSP (no `sandbox`): scripts and styles only from the shared blob host, `connect-src` only the workspace, `frame-src 'self'`, `frame-ancestors` = `<workspace> <embedders>`.

| Who opens it | What they get |
| --- | --- |
| Signed in, with access | the wrapper with their own role, the part framed inside it (one hop on first open) |
| Signed in, no access | `/grant` shows `403 · NOT SHARED WITH YOU` with **Ask for access** (creates a share request, §3a). The title isn't revealed |
| Not signed in | `/grant` shows a sign-in page carrying the **content-free card** (§4c), then returns to the part |
| An unfurler or any client without JS | the wrapper HTML, whose og tags are the **content-free card**, or the **public card** if the part has an active public link, plus a `<noscript>` "Sign in to open" link |

- *Why one link:* every other shape (per-version previews, minted links) goes stale after the first fix, and agents would have to know which one to post.
  With the part host, the access decision happens when the link is opened, by whoever opens it, and the link is the name an agent already knows.
- **`/p/<name>` on the workspace is only a redirect** (302 to `https://<name>-part.<domain>/`), kept so old links and typed paths work. The workspace gallery links straight to part hosts.
- **Embedding**: an embedder in `[embed] frame_ancestors` frames the part host root (the wrapper, with its chrome) or `/c/` directly with a server-minted cap and `pf-embed.js`, which handles `pf:regrant` (§4).
- **Public links are separate and Chris-only** (§3a): `https://<name>-part.<domain>/l/<token>`, on the part host. They're made after publishing, have **no expiry by default** (optional expiry), show open count and last opened, and are revocable.
  The `/l/` page is the wrapper in public mode (title, no share or comments unless opted in) carrying the public card, and a JS hop: `POST /l/<token>/go` on the same host returns a cap with subject `link:<id>`, and that call is what counts an open.
  There's no `<meta refresh>` and no target URL in the HTML, so a crawler finds nothing to follow. The root `/` and `/l/` are the two part-host routes that read the DB (through a cache); `/c/` stays DB-free.
- **The response never depends on the User-Agent.** Bots and people get byte-identical HTML. What it reveals is decided by the audience: the root carries the content-free card unless the part has an active public link, in which case its title is already public and the root carries the public card; `/l/` always carries the public card.
  *Why:* unfurl-bot detection is unreliable. iMessage fetches from the sender's device, and several apps use browser-like UAs.
- **Telegram in-app browser and passkeys: open item.** Tested on a real device in P1 (§6). If passkey sign-in fails there, the fallback is a link bound to one specific person who already has access
  (view-only, 7 days, revocable, first use binds it to that device). It is never an anonymous link.

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
     /data            — partifactory.db (SQLite, WAL) + blobs/ + backup/ + exports/
   Optional sidecar: partifactory-render (chrome-headless-shell), internal network only (§4c)
```

**The ingress contract** (the core's only requirement of whatever sits in front):
- route `parts.<domain>` to `:8080` and `*.<domain>` to `:8081`, preserving `Host`; terminate TLS with a cert covering both (one wildcard cert does);
- pass the client address in one header named in config (`[ingress] client_ip_header`, e.g. `X-Forwarded-For` or `CF-Connecting-IP`), trusted only from `[ingress] trusted_proxies`. Rate limits and analytics key on it;
- optionally pass a request id (`[ingress] request_id_header`); without one, partifactory makes its own. Logs and error pages show it;
- allow long-lived responses (SSE, long-poll ≤ 50 s) and request bodies up to 8 MiB.
Example Caddy, nginx and Traefik configs ship in `deploy/proxy/`, each exercised by the portability e2e lane (§5f). **Edge caching is an optimisation, never required for correctness**: every response already carries the right `Cache-Control`, and nothing breaks with no cache at all.

**Hostnames** (decided): the workspace and every part host live one label under one apex (`[domain] apex`, `workspace_host`, `part_suffix`), so one wildcard cert covers them (in the example deployment, Cloudflare's free universal cert for `dilger.dev`).
DNS wildcards must be a whole label (`*-part` isn't valid), so routing is **one `*.<domain>` record + one `parts.<domain>` record**, created by `provision` through a DNS provider plugin or printed for you to create by hand (§5c).
Explicit records for other services always take precedence over the wildcard. Names that exist nowhere land on partifactory and get a 404 unless they end in `-part`.
There's no DNS token at runtime, and a new part is live the instant it's saved. *Cloudflare adapter only:* a wildcard CNAME to `<tunnel>.cfargotunnel.com` works in practice but isn't documented, so the adapter's spike verifies it.

**Isolation of agent-written JS, stated honestly.** Parts share a site with the rest of `<domain>` (in the example deployment, with the IdP, home automation, the forge and the Nest). The opaque origin (§2) closes the cookie attacks: a sandboxed document can't read or write cookies,
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
| Personal (`~chris`, one per person) | first sign-in | only its owner | the owner, and agents the owner connected |
| `family` | `provision` | `group:<household> → edit` | the household, and agents consented into it |
| `system` | `provision` | everyone signed in → view | Chris (holds Jig, the design-system part) |
| more | Chris (admin) | as set | as granted |

- A part is in exactly one space. `part_create` without `space` uses the creator's personal space. An agent acting for a person defaults to that person's personal space.
- **Effective access = max(part grants, space grants)**, inherited live. There's no per-part "restrict below the space" in v1.
- **Moving a part** needs write access to both spaces. "Move to Family" is the one-tap household share.
- Agent consent is limited to spaces. A connection for `~chris` + `family` can't see another person's personal space.

### Who can see a part: the audience ladder

Each part and each space has **grants** `{subject, role}`. Roles: `view` < `comment` < `edit` < `manage`.

| Audience | Subject | Who can set it | Notes |
| --- | --- | --- | --- |
| **Only me** (default in a personal space) | `user:<me>` | — | A part inherits its space's grants: private in `~me`, the household in `family` |
| Specific people | `user:<id>`… | the part's owner, Chris, agents with `share:household` | Picked from the directory. Inviting someone *new* is Chris-only |
| A group | `group:<household>`, `group:pf-photofinder`… | the part's owner, Chris, agents with `share:household` | Any IdP group can be *granted*. partifactory can only *create/edit* its own `pf-*` groups (local, or IdP-side with a provisioner), and only Chris creates them |
| Everyone signed in | `everyone-signed-in` | Chris | Shown with a live count ("15 people") so it's never vague |
| **Public link** | `link:<id>` | **Chris only**, after publishing | No sign-in, unlisted, `noindex`, revocable, **no expiry by default**, open count and last opened. `view` only by default. `comment` needs an explicit opt-in (spam and prompt-injection risk, §3c). Shown in amber as "the worst option" |

A part's **owner** is the person it was made for (the person a creating agent acted for). Chris can change it. Owners and `manage` grantees share inward; nobody but Chris shares outward.
There's no listed or indexable public mode. Effective access is the highest matching role. Every change goes to the `audit` table (who, actor, what, before → after).

- *Why owners can share inward without Chris:* sharing a part with someone who already has an account can't leak beyond the household, and gating it made the common case slow.
  Everything outward (public links, everyone-signed-in, inviting new people, creating groups) stays with Chris, because that's where data leaves.

**What each role can do at runtime** (enforced in `/_rt` and in every op):

| | personal state (`localStorage` shim, personal `storage`) | shared `storage` / `db` | comments | self-publish (`artifact.publish`) | source edit | assets upload |
| --- | --- | --- | --- | --- | --- | --- |
| public-link viewer | memory only | read | only if opted in | no | no | no |
| `view` | read/write | read, **write if "viewers can update data" is on** (default **on**, as in Claude) | no | no | no | no |
| `comment` | read/write | as `view` | yes | no | no | yes |
| `edit` / `manage` | read/write | read/write | yes | yes | yes | yes |

"Viewers can update data" is a per-part toggle in the share sheet, so a poll or checklist works for everyone who can see it, while a dashboard can be read-only.
`manage` in v1 means "edit, plus see the audit log and change inward grants". It never includes outward sharing.

### Ask for access

A signed-in person who opens a part they can't see gets `403 · NOT SHARED WITH YOU` with **Ask for access**. That creates a `share_requests` row (`requester_kind = user`), which appears as an approve/deny chip in the owner's and Chris's **Requests** inbox and on the part's own chrome bar, and is approved with one tap. Nobody is messaged.
The page never reveals the title. Public-link holders never see it: a revoked link is a plain `404`.

### Invitations, all minted in partifactory (Chris only)

The link handed out is always **partifactory's own** `parts.<domain>/i/<id>`. Opening it records `opened_at` and counts the use. What happens next depends on whether an IdP provisioner is configured. Grants and use counts always live in partifactory's own tables.
- *Why grants in partifactory:* per-part access never depends on IdP group plumbing (Authentik invitations, for one, can't set groups). IdP groups are only needed for groups people manage elsewhere.

**Default: native invites with local passkey accounts (no provisioner). In scope for v1, P2b.**
1. **Passkey-only invite link** (share anywhere): `/i/<id>` → one field ("What should we call you?", username generated) → **passkey creation, required**, which partifactory registers itself (`webauthn-rs`) → a local account is created → land on the part with the invite's grants applied.
   - There's no email and no password. 7-day default expiry. The dialog offers **"for one person"** (single use) or **"for a group chat: up to N people"**, counted by partifactory.
     *Why multi-use:* single-use links pasted into family chats lose a race and confuse everyone else.
   - **Lost passkey** → Chris clicks "Send a new passkey link" on that *existing* person: a single-use `/i/<id>` of kind `recovery` bound to that account, which registers a new passkey and revokes the old ones. History and grants stay put, and no zombie account is created.
   - A local account can later be **linked to an IdP identity** (sign in with OIDC once while signed in locally), so a household that adopts an IdP keeps everyone's history.
   - *Why in scope:* without it, a deployment with plain OIDC and no provisioner couldn't invite anyone, and invites would quietly depend on Authentik. `webauthn-rs` is already in the binary for step-up, so the added surface is registration, passkey sign-in and recovery for local accounts.
2. **Add by email**: needs a provisioner (the IdP sends the email). Without one, the share sheet offers the invite link instead. partifactory doesn't send email.
3. **Existing user**: pick and grant (an inward share, no Chris needed if it's the owner's own part). It shows up in their "Shared with me" and activity feed.
4. **New group**: "Make a group for this" creates a local `pf-<name>` group in partifactory's tables.

**Adapter: the Authentik provisioner (the example deployment).** Authentik facts it's built on (checked 2026-10-23): invitations **can't set groups** (custom attributes become user attributes, groups come from the User Write stage), and there's **no use count**, only single-use or unlimited-until-expiry.
- `/i/<id>` 302s to an Authentik invitation bound to a dedicated **`partifactory-invite`** flow: invitation → one field → **passkey enrolment, required** → user write (into one holding group) → login → land on the part.
  After the first login, partifactory matches the new `sub` to the invite (carried in the login `state`), applies the invite's grants, and adds any `pf-*` memberships by API. The Authentik invitation is unlimited-until-expiry, and partifactory deletes it at the Nth enrolment.
- **Lost passkey** uses an IdP recovery link (`POST /api/v3/core/users/{id}/recovery/`) bound to the same user. **Add by email** creates the IdP user + `pf-*` groups, then triggers the IdP's "set up your account" email. **New groups** are IdP groups named `pf-<name>`, manageable from either side.
- Unverified Authentik items (object-level permission names, `is_superuser` protection, blueprint-over-API, recovery-link permission) are gates in the Authentik spike and the provisioner gate, not assumptions.

Either way: pending invites show their status (sent, opened, enrolled, expired) and have one-click revoke, and invitees who never enrolled, and people inactive for 180 days, are listed for one-click cleanup.

### UX: click budgets, tested end to end

Every flow has a budget in **real human interactions, including OS/browser prompts and the biometric**. The e2e suite (Chromium + virtual WebAuthn) enforces the page-level part,
and a manual pass on a **real iPhone (Safari), an Android phone and the Telegram in-app browser** before each phase's sign-off covers what a virtual authenticator can't see. §4a.7 adds the UI-level budgets.

| Flow | Who | Budget |
| --- | --- | --- |
| Open a part I may see, already signed in | anyone | **0**: silent redirects only |
| Sign in after session expiry | returning user | **2 interactions**: pick the passkey from autofill + biometric. No "stay signed in?", no consent |
| Share with someone in the household | owner | Share → pick the person → **Done** (no step-up) |
| Share with the family | owner | Share → **Move to Family** (no step-up) |
| Make a public link | Chris | Share → "Anyone with the link" → **Copy** (+ 1 biometric if the 10-minute step-up window has lapsed. Step-up is in-page WebAuthn, not a redirect) |
| Mint a passkey invite link | Chris | Share → "Invite someone" → **Copy link** (+ step-up) |
| Accept a passkey invite | new person | open → type a name → **passkey tap** → on the part |
| Add someone by email (provisioner only) | Chris | Share → "Invite someone" → email → **Send** (+ step-up) |
| Connect a new agent (OAuth) | anyone | agent opens browser → passkey tap (if signed out) → **Allow** + passkey on the consent sheet |
| Pair yourself with a chat agent (Hermes in the example deployment) | household member | "connect me" in chat → open link → **Allow** + passkey |
| Approve an agent's share request | Chris | Requests → **Approve** (+ step-up) |
| Revoke anything | owner or Chris | one click, with a 10 s undo toast instead of a confirmation dialog. Revocation applies at once (epoch bump), and undo re-grants |

**Authentik adapter: click reduction**. With a generic IdP, the operator configures implicit consent and passkey-first login in the IdP however it offers them, and the budgets above are what the e2e suite holds partifactory's own pages to.
With the Authentik adapter, shared flows belong to every service on the IdP, so partifactory **gets its own cloned authentication and authorization flows**,
which it owns, provisions and tunes. Shared flows are touched only for changes Chris explicitly wants globally. Global so far: no "Stay signed in?" prompt (applied 2026-10-22 to the
shared login stage at Chris's request, with the before-state in site config). partifactory-owned: implicit consent, passkey-first identification with conditional UI
(supported since Authentik 2025.12 when a WebAuthn validation stage is set on the identification stage; still verified in the Authentik spike), a no-prompt invalidation flow, and partifactory branding.

**Share sheet**: one sheet, with the audience ladder as a segmented control, a live sentence saying exactly who can see the part now, "Move to Family", the public link (Chris), pending invites,
agents' share requests as approve/deny chips, agent-made grants labelled ("Hermes shared this with the Family group · Undo"), and copy buttons. It works on a phone (§4a.6).

### Admin is Chris, and only Chris

- **Pinned to Chris's immutable subject** in config (`[admin] subject = "oidc:<issuer>#<sub>"` or `"local:<id>"`), not a group. Being added to any IdP group (including the IdP's own admins) grants nothing here.
  A fresh deployment with no IdP bootstraps the admin with `partifactory admin bootstrap` on the host console, which prints a one-time link to register the admin's passkey as a local account.
- **Step-up is native WebAuthn in partifactory**, not trust in IdP claims. Chris registers an admin passkey in partifactory itself (`webauthn-rs`).
  **Admin actions need a passkey assertion within 10 minutes**: public links, `everyone-signed-in`, invites, add-by-email, groups, users, approving share requests, agent consent into spaces the consenting person doesn't own, `unsandboxed`, renames.
  Household sharing never needs step-up. *Why one 10-minute window:* short enough that a stolen session can't administer, long enough for a batch of approvals. (Round 1 said 30 in one table and 10 in another. It's 10.)
- **Authentik adapter only: the IdP credential is least-privilege, and that's verified, not assumed** (the provisioner gate, P2b). With no provisioner, partifactory holds no IdP write credential at all. The intent: a service account `partifactory-provisioner` that can read users/groups,
  create invitations, send recovery links, and change membership of `pf-*` groups **only** (Authentik object-level permissions on each `pf-*` group).
  The gate tests, against the real IdP, that this account **cannot**: add anyone to a non-`pf-*` group, create a group with `is_superuser`, edit flows or stages,
  create users with elevated attributes, or read another application's provider secret. It also checks that `pf-*` groups have no privileged parent, because Authentik roles inherit down the group hierarchy.
  The core's own `pf-*` check is defence in depth only, because a compromised process would skip it.
  **If the IdP can't scope it that finely**: `pf-*` groups are created by `provision` (bootstrap token, run by Chris), never at runtime. The runtime account drops to
  invitations + recovery links + membership of pre-created groups. If even membership can't be object-scoped, add-by-email and new groups become "Chris approves, `provision` applies".
  The result sets `[idp.provisioner] tier = "A" | "B" | "C"` in site config, which the runtime reads at start to hide or queue the matching UI.
- **Break-glass**: `partifactory admin break-glass`, run on the host console, prints a one-time admin login valid for 5 minutes. It needs local shell access, works when the IdP is down,
  and is logged and shown on the Health page and in the audit log. The admin subject pin lives in site config, which has an offline copy (§5c).

## 3b. Agents: their own identities, scoped to what they need

**Connecting is how an agent gets its own identity**, not just MCP convenience. Each connection is scoped, revocable, and labelled "Hermes for <person>".
The same token works on every surface: MCP, the HTTPS API and the CLI (§4b).

### One connection per (agent, person)

- **Delegation is explicit**: every write records `author = <person>, actor = <connection>`, which the UI shows as "Hermes for <person>". An agent can't see or do more than the person it acts for.
- **Shared agents hold one connection per person.** For example, Hermes (the example deployment's chat agent) keeps a map `telegram_user_id → token`. Each household member pairs once from Telegram ("Hermes, connect me to partifactory"):
  Hermes starts a device grant, and the person approves at `parts.dilger.dev/device` in their real browser (passkey + Allow). Unpaired chat users get nothing, and Hermes tells them to ask Chris for an invite.
  - *Why:* with one shared token, every family request was "Hermes for Chris", it landed in Chris's space, and Hermes could read Chris's private parts while serving someone else. That's the confused-deputy problem delegation exists to prevent.
- **Who may consent**: any person may connect an agent into their own personal space and the shared spaces they belong to. Only Chris (with step-up) may consent an agent into anyone else's space.
- **Service identities** for agents that serve the whole household with no single person, such as the Nest's server: `principal_kind = service`, spaces = `family` only, no human `sub`.
  Writes are labelled as the service. A caller-supplied `requested_by` is recorded and shown as unverified.

### How agents authenticate: OAuth 2.1, per the MCP authorization spec

Target revision **2026-07-28**, compatible with **2025-11-25**. partifactory is both the **resource server** (`/mcp`, `/api`) and a small **authorization server** that *federates* login to the IdP.

```
claude mcp add --transport http partifactory https://parts.dilger.dev/mcp
  → POST /mcp  → 401  WWW-Authenticate: Bearer resource_metadata="…/.well-known/oauth-protected-resource", scope="parts:read parts:write"
  → client reads Protected Resource Metadata (RFC 9728) → authorization server = parts.dilger.dev
  → client identifies itself: Client ID Metadata Document (CIMD, preferred) or Dynamic Client Registration (RFC 7591, deprecated, kept)
  → browser: /oauth/authorize (PKCE S256, resource=https://parts.dilger.dev/ per RFC 8707: one resource for /mcp, /api and the CLI)
       → IdP or local-passkey login if needed (1 passkey tap)
       → partifactory consent sheet: "Claude Code (redirect: localhost) wants to: read & edit parts in [~chris ▾], ask Chris to share.
          [ ] Share with people already in the household.  Expires: [90 days ▾]"   → passkey → Allow
  → authorization response carries iss (RFC 9207)
  → token: opaque, audience https://parts.dilger.dev/, sub=<person>, act=conn:<id>, scopes + spaces
```

- **AS metadata** (RFC 8414 at `/.well-known/oauth-authorization-server`, also at `/.well-known/openid-configuration` for clients that only look there; it's not an OIDC provider, so no `id_token`):
  `client_id_metadata_document_supported: true`, `code_challenge_methods_supported: ["S256"]`, `authorization_response_iss_parameter_supported: true`, `registration_endpoint` (DCR), `device_authorization_endpoint`, `revocation_endpoint`, `scopes_supported`.
- **The consent sheet shows the redirect URI's host** (the spec says it MUST), with an extra warning for a localhost redirect from a CIMD client.
- **CIMD fetches** use the mirror egress guard (§2): https only, private/loopback/link-local addresses refused after DNS resolution, ≤ 5 KiB, 3 s, no redirects.
- **DCR** stays for Cursor and Gemini CLI, which don't do CIMD. It requires `application_type`, is rate-limited, and a registered client is inert until someone consents. Unconsented registrations expire after 1 hour.
- **Manual clients**: Chris can register a client by hand in the admin area (client id + secret + redirect URI) for clients that need a pre-registered one, such as GPT Actions.
- **Tokens**: opaque random strings with prefixes (`pf_at_`, `pf_rt_`, `pf_pat_`), stored hashed, revocable instantly. Access tokens last 1 h, and refresh tokens rotate (detecting reuse) for up to the consented lifetime.
  - *Why not JWTs:* no key distribution or clock skew, and revocation is a single row. The cost is a DB read per API call, which is fine because `/api` and `/mcp` aren't the hot path.
- **Auth failures are HTTP 401 with `WWW-Authenticate`**, on every surface, never an MCP tool result: that 401 is what triggers a client's OAuth flow. The 401 body explains the ways to get a token (§4b).
- **Swappable**: an `AuthorizationServer` trait. If the IdP gains CIMD + resource indicators, partifactory can switch to validating IdP-issued tokens as a pure resource server.
  - *Why a facade now:* Authentik's DCR (2026.x) needs an authenticated user and creates a provider + app per registration, and CIMD and RFC 8707 aren't documented. It can't serve MCP clients directly.

### Device grant (RFC 8628): the default for headless agents

*Why default:* there's no secret to copy, and approval happens on the person's phone with a passkey.

- **Client**: one pre-registered public client `part-cli`. `POST /oauth/device_authorization` carries `client_id=part-cli`, `scope`, `agent_name` (required, ≤ 40 chars, e.g. "Hermes") and `software` (e.g. "hermes/1.4").
  Each approval creates its own `agent_connections` row, so each (agent, person) is revoked separately.
- **Codes**: `device_code` is 32 random bytes. `user_code` is 8 characters from `BCDFGHJKLMNPQRSTVWXZ`, shown as `XXXX-XXXX`. `expires_in` 600 s, `interval` 5 s. Polling faster gets `slow_down` (+5 s).
  `verification_uri = https://parts.dilger.dev/device`, and `verification_uri_complete` adds `?code=XXXX-XXXX`. The CLI also prints a terminal QR.
- **Approval page** (any signed-in person, for their own connection): it shows "**This device says** it is *Hermes* (hermes/1.4)", labelled unverified; the request's coarse origin (country + network, from trusted proxy headers or a local GeoIP database; shown as "origin unknown" when neither is configured) and age ("requested 14 s ago");
  scopes as toggles; a space picker limited to spaces the person may consent; a lifetime picker (default 90 days); and **Allow / Deny**. Allow needs a passkey assertion.
- **Phishing defences**: when the request is over 2 minutes old, or (where origin is known) comes from a different network than the approving browser, a banner says "Only approve if you just asked an agent to connect".
  Typing a code by hand is limited to 5 attempts per minute per session. Codes are single use.
- **Token responses**: `authorization_pending`, `slow_down`, `access_denied`, `expired_token`. Success returns the same token shape as the auth-code flow.

### Personal agent tokens, for clients without OAuth

Minted on the consent sheet with a name, scopes, spaces and expiry (default 30 days, max 1 year), shown once, formatted `pf_pat_<base32, 160-bit>` so secret scanners and log redaction match it.
Used as `Authorization: Bearer` on `/api` and `/mcp`, by convention via `PARTIFACTORY_URL` + `PARTIFACTORY_TOKEN` env vars (the CLI and the skill file both read them).
They're equally supported for servers Chris controls, are marked "static" in the UI, and resolve to the same `(person, connection, scopes, spaces)` principal as OAuth tokens.

### Scopes

| Scope | Allows |
| --- | --- |
| `parts:read` | find, get, history, diff, `part_check` |
| `parts:write` | create, edit, restore a version, fork |
| `parts:delete` | soft delete to the 30-day bin, undelete |
| `data:read`, `data:write` | the part's `storage`/`db` documents and assets |
| `comments:write` | add and resolve comments |
| `share:household` | **inward** sharing: grant view/comment/edit to people and groups that already have accounts, and move a part into a space the connection may write. Consent-time toggle, **off by default** for clients registered by CIMD, DCR or by hand |
| `share:request` | file an **outward** request (public link, invite, new group, everyone-signed-in) that Chris approves with one tap |

Agents can never get admin scopes: public links, invites, users, groups, `unsandboxed`.

### Agent sharing: inward yes, outward no

- **Inward** (`share:household`): an agent may share with existing people and groups, or move a part into Family, within the spaces its connection allows.
  Every agent grant is labelled ("Hermes shared this with the Family group"), undoable in one tap, listed in the share sheet, and shows up in Chris's Requests inbox and the activity feed (no message is sent).
  This is what makes "Hermes, make a packing list and share it with the family" one turn.
- **Outward, never agent-controlled**: public links, inviting new people and creating groups. An agent can only file a `share:request`, which Chris approves with one tap.
- *Why:* an agent reading untrusted web content can be prompt-injected, and a public link or an outsider invite is the exfiltration channel. Inward grants can't leak beyond people who already have accounts.
- **Divergence from shartifacts**: shartifacts trusts the LAN (anyone on the network sees everything), and an agent with the token can make public links itself. partifactory is reachable from the internet and has per-person audiences, so sharing is a security boundary.

### Revocation cascades

Revoking a connection kills its refresh family, its access tokens and its PATs, and bumps its minter epoch, so every cap minted through it dies on the next request.
Grants it made stay (they're the person's choices too), but the connection page lists them with "Undo all".

| Agent | How it connects | Typical scopes |
| --- | --- | --- |
| Claude Code (Mac, other dev envs) | auth code + PKCE (CIMD) | `parts:*`, `data:*`, `comments:write`, `share:request` in `~chris` (+ `share:household` if Chris ticks it) |
| Hermes (VM, Telegram front door) | device code, **once per household member** | the same, in that person's space + `family`, with `share:household` |
| Nest (`nest-api`) | device code once at setup, **service identity** | `parts:read`, `parts:write` in `family` |
| CI (provision/smoke tests), any CI system | device code → token in that CI's secret store | `parts:read` + a test space |
| A script or a function-calling app | personal agent token | as minted |

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
- **Errors close the loop**: viewers' browsers report uncaught errors and blocked loads, which `part_get` returns as `runtime_errors` (latest 20 per part, `source: viewer | render`).
  `part_check` (§4c) lets the agent see its own render, screenshot included, before anyone opens it.
- **Live reload** (P2): when a new live version lands, open viewers get `pf:version` from the workspace page (or from the prelude's `EventSource` when standalone).
  A page with no unsaved personal state reloads in place; otherwise a toast says "Updated by Hermes · Reload". You watch the page change after asking.
- **Agent inbox** (P3): each part has a **steward** (the creating connection and its person, changeable in the share sheet). A `to_agent` comment goes to the steward.
  Push agents (Hermes) get a signed webhook. Pull agents get it via `part_inbox`, and the shared guide tells them to check it at session start. States: `open → claimed ("Hermes is on it") → answered → resolved`.
  A claimed request with no new version after 15 minutes shows "stalled".
- **In place, like Claude's artifacts**:
  - The workspace has a source editor (CodeMirror 6) with live preview from the draft, where saving creates a version. Drafts live server-side per (person, part), so they survive reloads and device switches.
  - **Persistent state**: `window.storage` (personal + shared) and `claude.use("db")` documents live in SQLite. They're the part's state across versions and devices, agents read and write them (`part_data`), and they're in every copy of `/data` and every full export.
  - **Self-editing parts**: `claude.use("artifact").publish(...)` lets a part save a new version of itself, using compare-and-set against the version it's running, which must also be head. Viewers with `edit` can do it. Public-link viewers never can.
  - **Assets** (P3): viewers with `comment` or higher can add files from the page (`POST /_rt/v1 {op: "assets.put"}`, 25 MiB each, counted against the space quota). They're part blobs served at `/_a/<id>` behind the cap, listed and deletable by agents via `part_data {collection: "_assets"}`.
  - **Phone editing** (P4): "Edit text" mode lets you tap any text in the preview and change it. It's saved as an anchor edit (`old_str` = the original text), so it works whenever the text occurs exactly once in the source (Markdown maps to block ids).
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
- `part_create {name?, title, kind?, space?, files?, from?, expires_in?, request_id}` returns `{id, name, url, version: 0}` **in < 10 ms**, before anything is built. The name is reserved by a unique constraint
  in the same transaction, and the part's host works immediately, its wrapper showing a "being made…" placeholder that live-updates when the first version lands. Optional `files` become v1, built after the link returns.
  The agent can hand you the link *first* and build second.
- **Fork**: `from: <name>[@n]` starts a new part as a copy of that version: a new manifest, zero new blobs, with cross-part lineage in `forked_from`. "Copy last trip's packing list" is one call.
- **Scratch parts**: `expires_in: "7d"` soft-deletes the part to the bin when it expires, so chat-made scratch parts don't silt up the library. Shown with a badge, extendable.
- **Idempotent, every write**: every mutating op takes a client-chosen `request_id`, scoped to the connection (or person) and kept 24 h. A retry with the same arguments returns the stored result.
  A retry with different arguments fails with `request_id_reused`. A timed-out `part_edit` retried after the network recovers says "already applied: version 12", never `anchor_not_found`.
- **Durable when acknowledged**: a call returns only after the blobs are written temp → fsync → hash-verify → rename and the manifest commit is fsynced (SQLite WAL, `synchronous=FULL` on the single writer connection).
  "Acknowledged" means it survives a power cut. **No save ever waits on a backup target** (§5c).
- **Latest vs live**: every result carries `{head, live, build: {status, id, warnings, errors}}`. Edits apply to head. The live URL advances only to versions whose build is `ok`, so a broken save never replaces a working page. The agent sees the errors instead.
- **Big files**: `POST /api/v1/blobs` (raw body → sha256) and a have/need check (`POST /api/v1/blobs/missing` with `{path → hash}` returns the missing hashes). Writes accept `{path: {blob: sha256}}`.
  `part put ./dist` uploads only missing hashes. MCP agents get a 10-minute upload URL from `part_create`/`part_edit {upload: [...]}`, so they never base64 media through the model.

**The build worker.**
- **States**: `queued → running → ok | failed | timeout | superseded`. A queued build is `superseded` (and skipped) once a newer version of the same part is queued. A running build is never cancelled.
- **Isolation**: each build runs in a **child process of the same binary** (`partifactory build-worker`, re-exec'd), with length-prefixed CBOR over stdin/stdout, a **10 s deadline**, a memory cap (`RLIMIT_AS` 1 GiB) and `SIGKILL` on breach.
  A crash marks the build `failed` with `code: "builder_crashed"`. Pool size `min(2, cores/2)`, pre-forked.
  - *Why a child process:* oxc and rolldown are young. A hang can't be killed in-process (a `spawn_blocking` timeout only abandons the thread), and a panic under `panic = "abort"` would take the content listener down with it.
    Spawning a static binary costs 1–2 ms, so the TSX target survives, and the esbuild fallback becomes a drop-in for the same protocol.
- **Cache key**: `sha256(source_manifest_hash ‖ toolchain_version ‖ library_set_version ‖ mirror_resolution_hash)`. An identical key reuses the output.
- **Output**: a build manifest `{files: {path → {hash, br_hash?, type}}, import_map?, warnings[], errors[], mirrored: [{url, resolved, hash}]}`, stored as a blob and referenced from `builds.output_hash`.
- **Brotli**: quality 11 for text ≤ 1 MiB, 5 above that, skipped for already-compressed types.
- **Live switch**: on `ok`, inside the writer: `if version.n > live.n then live = version`. Emit `build.finished` and, on a switch, `part.live_changed`.
- **`wait: true`** (the default for agents) blocks ≤ 2 s on `build.finished`. If it's still running, the result says `build: {status: "running", id}`, and `part_get {build: id}` or the next `part_watch` event finishes the story. `wait: false` returns at once.

**Concurrency contract: several agents (and people) on one part.**
- **Linearisable writes**: one writer task serialises all commits, so every part has a strict version sequence `n, n+1, …` with no lost updates. Rebase runs inside that same critical section (read head → apply → commit).
- **`part_edit` request**: `{name, base_version?, edits: [Edit], message?, wait?, check?, request_id?}`, where `Edit` is one of:
  `{op: "replace", file?, old_str, new_str}` (`file` defaults to the entry) · `{op: "write", file, content | blob, if_match: sha256 | null}` (`null` = must not exist) ·
  `{op: "delete", file, if_match}` · `{op: "rename", from, to, if_match}` · block ops (below). Mode fields instead of edits: `restore: n`, `delete: true`, `undelete: true`, `rebuild: true`, `meta: {title, tags, notes, description}`, `claim: {scope, ttl}`.
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

**Do we need CRDTs?** Not for v1. CRDTs (Yjs/`yrs`, Automerge) are production-ready, but they solve *simultaneous keystroke-level typing by humans*.
Agents submit whole edits, which versions, anchors, blocks and leases handle with far less machinery. If live human co-typing is ever wanted, `yrs` can run *inside one editing session*, with each session's result committed as an ordinary version.

## 3e. Search, history and the bin

**Search** is SQLite FTS5, the same index for the workspace search box and for `part_find`.
- **Indexed**: title, description, tags, agent notes, comments, and the **extracted readable text** of HTML and Markdown parts (text nodes after rendering, minus `<script>`/`<style>`). Code is not indexed as text, so a search for "map" doesn't match every `.map(` call.
  The index updates when a version goes live and when notes or comments change.
- **Filters**: `space:`, `kind:`, `by:` (an agent or a person, e.g. `by:hermes`, `by:me`), `updated:` (today/week/month or a date range), `has:data`, `has:comments`, `tag:`.
- **Ranking**: `bm25` with column weights (title 10, tags 5, description 4, notes 3, text 1, comments 2) multiplied by a recency factor `1 / (1 + age_days / 30)` on the last live change. No query means "recent, by me or this agent".
- **Workspace**: an instant search box (`/` to focus, results as you type, debounced 80 ms) with matches marked and "where it matched" (title, text, comment, agent notes).
- **Later, additive**: embeddings for semantic search alongside FTS5 (P5). Nothing in v1 depends on them.

**Versions and rollback, per part.**
- Every version is immutable and labelled with who and when ("Hermes for Chris · 14:02", "Chris · yesterday"), plus the agent's edit message or an auto summary ("app.tsx +14 −3").
- **History**: the full ledger. **Diff** between any two versions: a file list with `+/−` counts and unified (or side-by-side) diffs; binary files show size changes and before/after thumbnails for images.
- **Preview any old version**: the frame reloads with a cap pinned to that version (`v` claim). People with `view` can preview versions that were once live; drafts and never-live versions need `edit`.
- **Restore** = a new version whose manifest equals the old one. It's non-destructive, shows "Restored as REV 13 · Undo", and goes through the build like any other version.
- **Keep all versions.** Blobs are deduplicated by hash, so history is cheap. There's no version-pruning policy in v1.
- **Deleted parts go to a 30-day bin**: hidden from search and the gallery, restorable by the owner or Chris (`part_edit {undelete: true}`), purged after 30 days. A purged part's name stays reserved forever.

## 4. Operations: what agents and people can do

Every operation is defined once in the operation registry (§4b) and appears identically on MCP, the HTTPS API and the CLI. There are ten tools around one write op, plus `part_inbox` in P3.

| Op | Does |
| --- | --- |
| `part_create` | reserve a name and get the part's link in < 10 ms, before building (§3d). `check_only: true` tests a name and returns ranked alternatives. Optional `files`/`content`, `kind`, `title`, `space`, `tags`, `notes`, `description`, `from` (fork), `expires_in`, `request_id` |
| `part_edit` | **the one write op**: ordered anchor edits, per-file `write`/`delete`/`rename` with `if_match`, block ops, `meta`, `restore: n`, `delete`/`undelete`, `rebuild`, `claim`; `base_version` with auto-rebase; `wait` for the build; `check: true` runs `part_check` after the build (§3d) |
| `part_get` | full content when the part is ≤ 32 KiB, else outline + notes; `file`/`range`, `version`, `history`, `diff_from`/`diff_to`, `runtime_errors`, `render: "text"`, `build: <id>` |
| `part_find` | search + filters (`kind:`, `tag:`, `space:`, `by:`, `has:data`, `has:comments`, `updated:`); no query = recent by me / this agent (§3e) |
| `part_share` | `list` who can see it (the live sentence + grants); `grant`/`revoke` inward (people and groups with accounts, `share:household`); `move` to a space; `request` an outward audience from Chris (`share:request`). Always returns the link to post (the part's host) |
| `part_check` | render a version (`head`, `live` or `n`) in the sandboxed renderer and return a screenshot (an image block in MCP) + `blank` + errors, console and failed loads (§4c) |
| `part_comments` | list/add/resolve; `to_agent` threads; anchors; `provenance` per comment |
| `part_data` | the part's `db`/`storage` documents and `_assets` (Claude ArtifactData-shaped), with `rev` compare-and-set and leases |
| `part_watch` | long-poll the event log since a cursor, `wait` default 25 s, max 50 s (§4, Events) |
| `design_system` | Jig's rules and starter files, read from the Jig part (§4a.8) |
| `part_inbox` (P3) | requests addressed to this connection across parts: `to_agent` comments, runtime-error spikes, share decisions; claim/answer |

*Why so few:* agents choose tools by name and description, and near-duplicates (`part_put` vs `part_create`, `part_recent` vs `part_find`, preview vs share request) cause wrong picks and wasted turns. Every tool also costs context.
Every op carries hints (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint: false`), so clients can auto-approve reads.
Every mutation returns `{name, url, head, live, build: {status, warnings, errors}}`, so the agent never needs a second call to learn where things stand.

### Names: semantic, chosen by the agent

A part's name *is* its hostname, and its hostname is its link (`https://<name>-part.dilger.dev/` in the example deployment).

- `part_create` takes an optional `name`. Without one, it's derived from the title ("Sleep chart" → `sleep-chart`).
- `part_create {check_only: true, name?, title?, description?}` → available? If not, ranked alternatives that are valid and free right now.
  The generator is deterministic, with no LLM: drop stop words, try 2- and 3-token subsets of the title and description, then the space as a prefix, then numeric suffixes last.
- A clash returns `name_taken` **with** suggestions, so the agent retries in one step.
- Rules: `[a-z0-9-]`, 1–58 chars, no leading/trailing/double hyphens. Reserved: infrastructure words and names that mirror existing services (the list is in site config).
- **Names are not secret.** A private part's name is in its URL, and unfurlers show URLs. The shared guide tells agents to pick a neutral name for sensitive parts.
- **Renames are rare and admin-confirmed**: the old hostname 301s (query string stripped) for good, and **a name that was ever used stays reserved**, so a future part can't collide with an old redirect.
  Home-screen installs live on the part host, whose old name 301s to the new one, and there's no device-local state to strand (personal state is server-side).

### `part` CLI

Generated from the registry: `part <op> [--flags | --json @file | -]` for any op, plus ergonomic aliases for the common ones:
`part login` (device code) · `part put ./dir --name …` · `part edit <name> --replace 'old' 'new'` · `part get <name> [--file f]` · `part find …` · `part check <name>` · `part watch` · `part open <name>` · `part skill install`.
Output is human text, or `--json` for agents. `part help <op>` prints the description, schema and examples.
Credentials come from `PARTIFACTORY_TOKEN` if set, else the OS keychain, else `~/.config/partifactory/` (mode 600). The server comes from `PARTIFACTORY_URL`.
Binaries ship as release assets on any forge (the passive GitHub mirror's Releases in the example deployment) and through `cargo binstall`.

### Events: one log, three deliveries

An append-only `events` table, `{id, ts, part, type, actor: {kind, id, on_behalf_of}, data}`, retained **30 days**. A cursor older than that gets `cursor_expired {oldest}`. This is the agents' feed, and it stays as it is.
- **Long-poll** (every surface, MCP included): `part_watch {since, wait: 25, part?, kinds?}` or `GET /api/v1/events?since=…&wait=25` returns early on the first matching event.
- **SSE** (HTTP): the same URL with `Accept: text/event-stream`. `id:` is the event id, `Last-Event-ID` resumes, and a 25 s heartbeat stays under any proxy's idle timeout (Cloudflare's is 100 s). The workspace UI and agents' own servers use it.
- **Webhooks**: to admin-configured URLs only, the same event JSON, signed with HMAC (`X-PF-Signature`, documented so any consumer can verify it), retried with backoff, with the event id as the idempotency key. Targets are site config, never set by agents, which rules out SSRF.
  A chat agent (Hermes in the example deployment) is just one webhook consumer.
- *Why one log:* nothing depends on MCP notifications, which stateless Streamable HTTP doesn't have anyway. An MCP client that wants a push feel calls `part_watch` in a loop.

### No notifications for people: approvals happen in-band

partifactory sends people **no messages**: no web push, no mention or reply notifications, no per-person notification settings. Everything that needs a person's attention is where they already are:
- **Share requests and access requests** appear as approve/deny chips in the workspace **Requests** inbox (top bar, with a count) and on the part's own chrome bar.
- **The agent that asked tells the person in its own channel.** Hermes says "I've asked Chris to approve a public link" in Telegram. partifactory never messages Telegram itself.
- **Device-code approval** is started by the person, so no notice is needed.
- **Agent-made inward shares** are an entry in the Requests inbox and the activity feed ("Hermes shared this with the Family group · Undo"), not a message.
- "Shared with you" shows up in the gallery's **Shared with me** filter and the activity feed.
- *Why:* every channel partifactory owns (web push, email, a chat bot) is another integration to run and another place where a title can leak. The household already talks to its agents, and the agents already have the event log.

**The only push: optional ops alerts for the admin.** Off by default. One generic webhook URL in config (`[alerts] webhook_url`, HMAC-signed JSON `{alert, severity, detail, at}`), which ntfy, Hermes or anything else can consume.
Conditions: disk ≥ 90 %, no export in 48 h, the build worker crash-looping, the IdP unreachable for 10 minutes. The admin Health page shows the same things, so nothing depends on the webhook.

### Example consumers: a chat agent and an embedder

These are clients of the contracts above, not dependencies. The example deployment has two.
- **Hermes** (a chat agent on Telegram) uses MCP + the `partifactory` skill, with one connection per household member (§3b). It posts the part's **own host link**, with `link_preview_options {prefer_small_media: true}`,
  so the unfurl is the small content-free card, plus inline buttons **Open** and **Ask for a change** (Hermes asks for the text and files a `to_agent` comment with that person's own connection).
  Before posting a new part, Hermes runs `part_check` and fixes what it finds. Whether screenshots go into chat is Hermes's own setting (owner's direct chat only, never a group), not partifactory's.
- **The Nest** (a household dashboard) is listed in `[embed] frame_ancestors` and embeds family parts: the part host root (the wrapper, with its chrome), or `/c/` directly with a server-minted `src` and `pf-embed.js`, which handles `pf:regrant`. It has its own service identity. The Nest's knowledge base stays separate from partifactory.

## 4a. Workspace UI and visual identity

The workspace is the workspace host (`parts.dilger.dev` in the example deployment): gallery, search, share sheet, history, editor, comments, agent approvals and admin. The **part view** lives on each part's own host, as the trusted wrapper (§2), and carries the same chrome. Its theme is **Stockroom** (approved by Chris).
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
- **Kind bin codes** replace file-type icons: a mono caps code on a tag chip (`HTML`, `TSX`, `MD`, `SVG`, `DECK`, `MMD`, `CODE`, `FILES`).

### 4a.6 Screens

Shared chrome: a **48 px top bar**: mark + wordmark (home), search (`/`), a **Requests** tray with a count (share requests, ask-for-access, device approvals, pending invites), theme toggle, avatar menu (connected agents, sign out, sign out everywhere, admin for Chris).
Keyboard: `/` search, `g p` parts. There's no `n`: agents make parts. "Paste HTML…" lives in the menu.

**Gallery (`/`, "Parts")**
- Header: "Parts" + a mono count, a segmented filter `Recent · Mine · Shared with me · Family`, a kind filter, grid/list toggle (grid on desktop, list on phones).
- **Card = drawer front** (`raised`, `r-3`, hairline, tag notch): a 16:10 thumbnail; a label strip (tag chip with part number and bin code, title, name in mono); a foot (audience chip, `REV 12`, "Hermes · 3 min ago", open comment count);
  live state on the thumbnail (a blue "Hermes editing" pill while a lease is held, "being made…" on version 0, an expiry badge on scratch parts).
- Sections: "Pinned", then "Recent" grouped by day (`TODAY`, `YESTERDAY`, `THIS WEEK`, `EARLIER`) with caps column heads.
- List view: 56 px rows with a 72×45 thumbnail, title + name, bin code, audience chip, rev and time. The whole row is the hit target.
- **Thumbnails**, in order of preference: a screenshot from the render sidecar (if it's on, §4c); the agent's own (`meta.thumbnail`, PNG/WebP ≤ 256 KB); a server-drawn SVG spec-sheet thumb (title, first heading, kind, first lines, a colour strip from the part's CSS).
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
- **Requests**: when the part has pending share or access requests and the viewer can decide them, an approve/deny chip sits on the bar (the same chips as the workspace Requests inbox).
- **Agent activity**: a blue pulse dot + "Hermes is editing *app.tsx*" (from leases and in-flight edits), "Claude Code saved REV 12 · just now" for 30 s, or "Didn't build in REV 13: still showing REV 12" in danger colour, linking to the error.
- **Audience chip**: icon + words (`Only me`, `3 people`, `Family`, `Everyone signed in · 15`, `Public link`). Click opens Share.
- **Share**, **Edit** (hidden without `edit`), **Comments** with a count, and **⋯**: Open standalone ↗, Copy link (always `https://<name>-part.<domain>/`), Details (agent notes, runtime errors, analytics, files, data documents, data snapshots), Download as zip, Pin, Delete (to the bin).
- **Frame**: an `<iframe>` with `sandbox="allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-modals allow-downloads allow-top-navigation-by-user-activation"` (belt and braces with the header) and a server-minted `/c/<cap>/` `src`.
  The wrapper (and the workspace's own preview frames) identify the frame's messages by `event.source === frame.contentWindow`, never by `event.origin` (which is `"null"`). Registration corner ticks sit just outside the frame: the boundary between our chrome and someone else's page.
- **Runtime errors**: a danger tick on ⋯ and "2 runtime errors" in Details, with "Send to agent" (a `to_agent` comment with the stack).
- Rails (comments, versions, details) are 360 px on desktop and push the frame narrower rather than covering it.
- **Install**: the wrapper serves a per-part web app manifest (`id` and `scope` = `/` on the part host, name, icon, theme colour), so each part is its own home-screen app on its own origin.

**Share sheet (the audience ladder).** 480 px on desktop, a bottom sheet on phones. Top to bottom:
1. "Share *Photo finder*" with the tag chip.
2. **The ladder**, a five-step segmented control, narrow → wide: `Only me` · `People` · `A group` · `Everyone signed in` · `Anyone with the link`. The last two show only for Chris. The last is amber, with "The worst option: no sign-in, anyone who gets the link can open it."
3. **The live sentence**, in `t-heading`: "**You and 3 people** can see this. **A family member** can also edit." It updates before anything is saved.
4. **Move to Family**: one button when the part is in a personal space.
5. Rung detail: *People* (directory type-ahead with per-person role); *A group* (picker with live member counts; "Make a group for this" for Chris); *Everyone signed in* (count and role);
   *Anyone with the link* (Chris): role fixed to `View` (comment needs the opt-in), **expiry picker `Never` (default) · 1 day · 7 days · 30 days**, the URL with **Copy**, **opened 14 times · last 2 h ago**, "Use a screenshot as the preview" (if the sidecar is on), and **Revoke link**.
6. **Viewers can update data** toggle, and the `unsandboxed` flag (Chris, danger red).
7. **Agent-made grants**: "Hermes shared this with the Family group · Undo".
8. **Requests**: "Hermes asks to make a public link" or "A family member asked for access" as approve/deny chips. Approve uses the step-up if needed.
9. **Pending invites** for this part, with status and revoke.
10. Footer: "Changes save as you go." Revocations show a 10 s undo toast instead of a confirm.

Step-up is in-sheet: "Confirm it's you" with a passkey button inline, then the action completes after the biometric, with no navigation.

**Invites** (Chris). Share → "Invite someone" (or Admin → Invites): two choices, `Invite link` (passkey-only: "For one person" / "For a group chat: up to [5] people", expiry 7 days, what they'll get, **Copy link** once, with a QR code) and `By email`.
The invitee's side (`/i/<id>`, then the partifactory-invite flow with partifactory branding): one centred card, "*You're invited to* Photo finder", "What should we call you?", **Create passkey**, then straight to the part.
A used or expired link says "This invite has been used. Ask whoever sent it for a new one." The invite list shows status chips, sent/opened times, "Send a new passkey link" and "Clean up".

**Version history, diff and restore.** The history rail opens from the `REV` chip: a vertical **revision ledger**, each row `REV 12` · "Hermes for Chris" · relative time · one-line summary, with a green `LIVE` tag on the live version and a danger `DIDN'T BUILD` tag on failed ones.
Clicking a row previews it in the frame. **Compare** picks any two rows (default: selected vs live): a file list with `+/−` counts and a unified diff (side-by-side ≥ 1100 px), hunks collapsed beyond 3 lines of context.
**Restore REV 9** appends REV 13 and shows "Restored as REV 13 · Undo". `[` and `]` step through versions.

**In-place editor (CodeMirror 6).** `Edit` adds **Preview · Split · Source**. Split = file tree + CodeMirror + live preview rendered from the **draft** (a cap with a `d` claim, never the live version).
Syntax in four hues (keyword: signal; string: agent blue; number/const: public-text amber; comment: `ink-3` italic), Plex Mono 13.5/20. Drafts autosave to the server.
When an agent lands a version mid-edit: "Hermes saved REV 13 while you were editing. Your changes were merged." Overlaps open **Resolve** (Keep mine / Keep theirs / Edit per hunk).
A lease shows a blue dot and "Hermes is editing this file until 14:32"; saving asks once. Markdown and slides get block-aware editing with pending placeholders. Phone: Source only, plus tap-to-edit text in P4 (§3c).

**Comments.** A right rail, threads ordered by position then time, each with its anchor quote. **"For the agent"** toggle (the `to_agent` flag), shown as a blue chip. Then "Hermes is on it", then "Fixed in REV 14 · View diff".
Comments from public-link viewers or other people carry a small "untrusted for agents" marker for the owner. Resolve, reopen, "show resolved". Phone: a bottom sheet with the composer above the keyboard.

**Agent approvals (`/device`).** Two ways in: a chat agent (Hermes) posts "Connect to partifactory: `parts.dilger.dev/device?code=KMQT-4XRA`", or `part login` prints the URL and code. On a phone, single column, large type:
the code in big mono with "Make sure this matches what the agent shows"; **who** ("This device says it is Hermes · hermes/1.4", unverified, plus coarse origin and age); **what it wants** in plain words, each removable
("Read and edit parts", "Read and write part data", "Comment", "Ask Chris to share parts", "Share with people already in the household" — off unless requested); **where** (spaces as toggles); **for how long** (`30 days · 90 days · 1 year`);
**Allow** (filled, full width, triggers the passkey) and **Deny**. Success: "Hermes is connected for you." The same sheet serves the browser OAuth flow, plus the redirect host.
**Connected agents** (avatar menu) lists each connection: "Hermes for you", scopes, spaces, last used, token type (`OAuth` / `device` / `personal token`), grants it made, with **Revoke** (undo toast; cascades per §3b).

**Admin (`/admin`, Chris only).** **People · Groups · Invites · Agents · Public links · Audit · Health**. A persistent step-up chip: `Admin unlocked · 8 min` or `Locked` with a passkey button.
*Public links*: every live link, its part, created, expiry (usually "never"), **open count and last opened**, revoke, and a red **Revoke all public links** kill switch (hold to confirm, 1 s).
*Audit*: a filterable ledger, "who · as whom · what · before → after · when", in mono. *Health*: the `/readyz` checks, degraded indicators (IdP, directory poll), backup layers (§5c), disk headroom, version, last drill, and "what's actually used".

**Empty, loading and "being made…" states.**
- Empty gallery: the drawer mark at 64 px, "No parts yet. Parts are made by your agents. Connect one, then ask it for something." with **Connect an agent** and a copyable `claude mcp add --transport http partifactory https://parts.dilger.dev/mcp`.
- Loading: skeleton drawer fronts (no shimmer). The frame shows the tag, title and corner ticks at once, and no spinner for the first 400 ms.
- **"Being made…"** (version 0): a **drafting sheet** with a faint 8 px grid and a title block bottom-right (`P-0215 · grocery-planner · REV 0 · Hermes for Chris`), and "Hermes is making this. It'll appear here as soon as the first version lands."
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
| Make a public link + copy (Chris) | Share → Anyone with the link → Copy | **3** (+1 biometric) |
| Revoke a public link | Share → Revoke link | **2**, undo toast |
| Approve an agent's share request | Requests → Approve | **2** (+1 biometric) |
| Approve a request while looking at the part | chip on the part's bar → Approve | **1** (+1 biometric for outward requests) |
| Copy the link to post | ⋯ → Copy link, or copy the address bar (it's already clean) | **2**, or **0** |
| See what changed in the last version | `REV` chip → Compare | **2** |
| Restore an older version | `REV` chip → row → Restore | **3**, undo toast |
| Undelete from the bin | Bin → Restore | **2** |
| Small text fix by hand | Edit → (type) → ⌘S | **2 + type** |
| Comment for the agent | Comments → type → "For the agent" → Post | **3 + type** |
| Approve a headless agent on the phone | tap link in chat → Allow → biometric | **3** |
| Connect Claude Code (signed in) | (browser opens) → Allow → biometric | **2** |
| Accept an invite | open → name → Create passkey → biometric | **3 + type** |
| Revoke an agent | avatar → Connected agents → Revoke | **3**, undo toast |

### 4a.8 Part content, and the optional Jig design system

**Part content is not themed by us.** A part is served from its own sandboxed host and gets exactly the HTML, CSS and JS its author wrote. No `--pf-*` token, font or stylesheet reaches it. Claude-written artifacts render unchanged.

**Jig** (a jig is the guide you use to make parts consistently) is an **optional** design system that agents opt into, returned by `design_system` with rules and a starter.
It's deliberately **distinct from the workspace chrome**: neutral white paper, system fonts by default, a blueprint-blue accent, softer radii. Parts should look like *their* content, not like partifactory.
We also use Jig to render the kinds we render ourselves (Markdown, slides, Mermaid, code). **Jig is itself a part** in the `system` space, so Chris edits it like anything else and parts record `meta.jig_version`.

**Delivery.** `jig.css` (≈ 8 KB brotli) is a shared blob at `https://blobs-part.dilger.dev/b/<hash>/jig.css`, allowed by the part CSP. Opt in with one `<link>` and `data-jig` on `<html>`. The import map exposes `@partifactory/jig` for TSX parts (CSS import + a tiny `useJigTheme()` hook).

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
pub struct PartEdit { pub name: String, pub edits: Vec<Edit>, pub base_version: Option<u64>, pub request_id: Option<String>, pub check: Option<bool> }

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

**The shared guide** is one hand-written Markdown file (`USAGE.md`): names, notes, quick edits, Jig, the link to post (the part's own host), `part_check`, sensitive names, provenance, the inbox.
It's the MCP `instructions`, the body of `/skill.md`, the top of `/llms-full.txt`, and the OpenAPI `info.description`, so "make me a packing checklist in partifactory" is enough on any surface.
GPT Actions constraints are checked by a CI lint: every op has an `operationId`, descriptions are ≤ 300 chars (`x-pf-description` carries the rest), and there are ≤ 30 operations.

### Discovery: an agent told only "parts.dilger.dev" can find everything

| Path | What |
| --- | --- |
| `GET /` with an `Accept` that isn't HTML | `text/markdown`: what partifactory is, the four ways to connect, and links to everything below (`Vary: Accept`; browsers get the UI) |
| `/llms.txt` | the llms.txt index: one paragraph, then links to `/skill.md`, `/openapi.json`, `/mcp`, `/llms-full.txt`. Cheap and harmless; no client relies on it |
| `/llms-full.txt` | the guide + every op's description, schema summary and examples, as one file |
| `/openapi.json` | OAS 3.1, ETag = build hash |
| `/.well-known/oauth-protected-resource` | RFC 9728 for the one resource `https://parts.dilger.dev/`: `authorization_servers`, `scopes_supported`, `bearer_methods_supported: ["header"]`, `resource_documentation` → `/llms.txt` |
| `/.well-known/oauth-authorization-server` | RFC 8414 (§3b), also at `/.well-known/openid-configuration` |
| `/.well-known/mcp/server.json` | the MCP registry `server.json` shape (name, description, version, `remotes: [{type: streamable-http, url}]`). **Served, not published** to the public registry: this is a private instance, and the registry doesn't support private servers. The public repo ships a template |

- **No A2A agent card.** A2A describes agents that accept tasks. partifactory is a tool and resource server, not an agent. If Hermes ever speaks A2A, *Hermes* publishes the card and lists partifactory as a tool.
- Every `401` from `/api` or `/mcp` carries `WWW-Authenticate: Bearer resource_metadata="…"` **and** a JSON body explaining the three ways to get a token (OAuth, device code, personal token), with links. An agent that hits the endpoint blind learns how to authenticate from the error alone.
- Discovery documents are unauthenticated and contain no site secrets or user data: they never list spaces, parts or people.

### Auth for each kind of client (one token model)

| Client | Flow | Token |
| --- | --- | --- |
| MCP clients (Claude Code, Codex: CIMD; Cursor, Gemini CLI: DCR) | auth code + PKCE | opaque access token + rotating refresh token |
| Headless (Hermes, the `part` CLI, CI, cron) | device grant (RFC 8628) | the same |
| GPT Actions and other pre-registered-only clients | auth code with a manual client | the same |
| Scripts, function-calling apps, a pasted env var | personal agent token, `PARTIFACTORY_URL` + `PARTIFACTORY_TOKEN` | `pf_pat_…` as Bearer |

All of them resolve to the same `(person, connection, scopes, spaces)` principal, so the policy engine, audit and revocation are identical everywhere. PATs are accepted on `/mcp` too, since some MCP clients only support a static header.

### Errors written for an LLM to self-correct

One shape on every surface:

```json
{"error": {
  "code": "anchor_not_found",
  "message": "edits[1].old_str was not found in app.tsx at v14 (you based on v12; v13 changed this region).",
  "hint": "Re-read the region with part_get {name, file: \"app.tsx\", range: [40, 60]} and retry just edits[1]; edits[0] applied cleanly.",
  "retryable": true,
  "details": {"edit_index": 1, "file": "app.tsx", "context": "…the closest matching region…", "lines": [44, 52], "similarity": 0.83},
  "docs": "https://parts.dilger.dev/llms-full.txt#anchor_not_found"
}}
```

Rules, enforced by a test that every error variant has a hint and an example:
- Every message says **what was expected, what was found, and the next call to make**.
- Unknown fields get "did you mean": "unknown field `content`; did you mean `files`?" (`deny_unknown_fields` + edit distance).
- Wrong-surface mistakes are recognised: a JSON-RPC body POSTed to `/api/v1/…` gets "this is the HTTPS API; MCP lives at /mcp".
- A missing scope names the scope and how to get it. Quota and rate errors carry `retry_after`.

| Code | Details | HTTP | CLI exit |
| --- | --- | --- | --- |
| `invalid_input`, `invalid_edit`, `unknown_field` | `path`, `suggestion` | 400 | 2 |
| `unauthenticated` | how to get a token | 401 + `WWW-Authenticate` | 3 |
| `forbidden` | `scope_needed` or `space` | 403 | 3 |
| `step_up_required` | — | 403 | 3 |
| `not_found`, `file_not_found`, `block_not_found` | — | 404 | 4 |
| `name_taken` | `suggestions` | 409 | 5 |
| `if_match_failed` | `file`, `current_sha256` | 409 | 5 |
| `file_exists`, `request_id_reused`, `version_conflict` | `head` | 409 | 5 |
| `claimed` | `by`, `until`, `scope` | 409 | 5 |
| `anchor_not_found`, `anchor_ambiguous`, `block_changed` | `edit_index`, `file`, `context`, `lines`, `count` | 422 | 5 |
| `base_unknown`, `invalid_against_base` | `base`, `failing edit` | 422 | 5 |
| `cursor_expired` | `oldest` | 410 | 4 |
| `quota_exceeded`, `rate_limited` | `kind`, `limit`, `retry_after` | 429 + `Retry-After` | 6 |
| `too_large` | `limit` | 413 | 2 |
| `storage_full` | — | 507 | 7 |
| `renderer_unavailable` | `hint` | 503 | 7 |
| `internal` | `request_id` | 500 | 7 |

`build_failed` isn't an HTTP error: the version *was* saved, so the call returns 200 with `build: {status: "failed", errors: [{file, line, col, message}], live}`. Warnings (`cdn_unpinned`, `root_relative_url`, `duplicate_library`) ride in `build.warnings`.
In MCP, errors are tool results with `isError: true` and the JSON as `structuredContent`, plus a one-line text rendering, **except authentication failures, which are HTTP 401**. The CLI prints `message` and the first actionable field, or the JSON with `--json`.
OpenAPI has one `Error` schema whose `code` is an enum generated from the catalogue.

### Conformance: one scenario, every surface

`tests/agent_surfaces/scenario.yaml` is one list of `(op, input, expect)` steps: create a part → get the link → edit with a stale base (auto-rebase) → a conflicting edit (expect `anchor_not_found` with a hint) → find it →
share inward with a group → request a public link → comment `to_agent` → write `db` → `part_check` → restore v1. Four drivers run it against an ephemeral server:
1. MCP via the `rmcp` client.
2. HTTP via a client **generated from `/openapi.json`**, which also proves the spec is truthful.
3. The CLI via subprocess with `--json`.
4. **The skill file itself**: the example commands in `/skill.md` and `/llms-full.txt` are extracted and executed.

Normalised results (ids, timestamps) and audit rows must be identical across drivers and across the three auth kinds. The suite also validates `/openapi.json`, runs the GPT Actions lint and fetches every discovery URL.
An optional nightly, not gating: a small model gets only `PARTIFACTORY_URL` + a PAT + "make a packing list page and give me the link", and must succeed via discovery.

## 4c. Previews and render checks

Two tiers. Tier (a) always runs. Tier (b) is an optional sidecar whose headline job is **`part_check`: agents screenshot and validate their own work**.

### Tier (a): branded cards, in the binary, always

- Rendered from an SVG template → `resvg`/`usvg` → PNG 1200 × 630 in a few ms, no browser. Fonts are bundled (`fontdb`; a `FROM scratch` image has no system fonts). Titles are wrapped to ≤ 2 lines and ellipsised in Rust (`rustybuzz` measures glyph advances), because resvg doesn't wrap.
  Output ≤ 150 KB, cached as a blob keyed by `sha256(template_version, fields)`, served at **content-addressed `/og/<sha256>.png`**, so a new title or version means a new URL and unfurl caches never go stale.
- **The card's inputs are fixed fields, never part content**: `{template_version, audience, title?, description?, kind_label?, rev?, thumbnail?}`. The designer owns the template; the mechanics own the inputs. That makes the content-free guarantee checkable: a test asserts the private variant's input has no part-derived field.
- All variants share the frame: `paper`, the 8 px drafting grid at 4 % ink, corner ticks 40 px in, the mark + wordmark top-left, and a catalogue-style title block bottom-right.

| Variant | Served on | Shows | Never shows |
| --- | --- | --- | --- |
| **Private** | the part host root (the wrapper's og tags) for every part without an active public link, sign-in pages, `/grant` | the mark, "A private part", "Sign in to open it", the workspace host name, a lock glyph | title, name, part number, thumbnail, description, author, audience, counts |
| **Public link** | `https://<name>-part.<domain>/l/<token>`, and the part host root while the part has an active public link | title, kind bin code, `REV n`, a 2–3 line description, the thumbnail in a framed window, "Anyone with the link · view only" | comments, data, who else has access, agent notes |
| **Invite** | `/i/<id>` | "You're invited to a part on partifactory" (+ the inviter's display name only if they opt in) | the part title, until the invitee has enrolled |

Every card page carries `noindex`, `X-Robots-Tag: noindex, nofollow`, `og:title`/`og:description` that match the card exactly (so text tags never leak what the image hides), `twitter:card=summary_large_image`, and an absolute https `og:image`.
Private and invite cards are generic, so `/og/*` is cacheable by any shared cache (`public, immutable`). Cards are served from the workspace host, and part-host pages reference them by absolute URL. **The audience decides the card, never the User-Agent** (§2).

### Tier (b): the render sidecar (optional)

- **Image**: `chrome-headless-shell` pinned by digest, plus fonts (Noto subset with CJK and emoji), in its own container `partifactory-render`. Non-root, read-only root fs, tmpfs `/tmp`, `mem_limit: 768m`, `pids_limit: 256`, `cpus: 1`, default seccomp.
  Flags: `--disable-gpu --no-first-run --disable-extensions --disable-background-networking --disable-sync --mute-audio --deny-permission-prompts`.
- **Network**: attached only to a Docker `internal: true` network shared with partifactory, so it has no route to the internet or the LAN. Chrome resolves `*-part.<domain>` and `blobs-part.<domain>` to the gateway via `--host-resolver-rules`,
  and partifactory serves the same handlers on an internal-only TLS listener (`:443` on that network, self-signed `*.<domain>` cert generated at start, trusted via `--ignore-certificate-errors-spki-list`). URLs, CSP and the secure context match production byte for byte.
  Parts load nothing third-party (mirror-on-build), so nothing else is needed.
- **Renderer caps**: minted for subject `render`, a 60 s window, **read-only**: `/_rt` writes succeed in memory (so the page behaves normally) and are never persisted. It can reach only the part host.
- **Driving it**: CDP over the internal network (`chromiumoxide`). Each job gets a **fresh browser context**, a viewport (`mobile` 390×844 @2x or `desktop` 1280×800), and waits for `partifactory.ready()`, network idle for 500 ms, or 8 s, whichever is first. Hard timeout 15 s, then the context is destroyed.
- **Collects**: uncaught exceptions, `console.error`/`warn` (first 50, truncated), failed and CSP-blocked requests, document height, timing, and a **blank flag** (body text empty *and* the screenshot ≥ 98 % one colour).
- **Queue**: `render_jobs`, **concurrency 1**, coalesced per part (a 5 s debounce after a live change, newest only), one retry, a circuit breaker. Chrome starts on demand and stops after 5 minutes idle, so it costs no RAM most of the day. **The renderer is never on the save path.**
- **Outputs**: `part_check` results; runtime errors tagged `source: render`, so a broken page is known before any person opens it; private gallery thumbnails (600×375 WebP part blobs, P4); and screenshot og:images **only for public links that opt in** (per link, off by default, because unfurlers store images on their servers).
- **If it's disabled or down**: `part_check` returns `renderer_unavailable` with a hint, thumbnails fall back to the agent's own or the SVG thumb, and nothing else degrades.

### `part_check`

`part_check {name, version?: head | live | n, viewport?: mobile | desktop, wait_ms? ≤ 8000}` returns `{screenshot, blank, errors[], console[], failed_requests[], timing}`.
In MCP the screenshot is an **image content block** (PNG, ≤ 1 MB, downscaled), so multimodal agents can look at it. Over HTTP it's a short-lived URL plus optional base64.
It can check `head` before it goes live, so an agent can verify a change before it ever replaces a working page. `part_edit {check: true}` runs it after the build and folds the result in (about 1–3 s).
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
| Cards | `resvg` + `usvg` + `fontdb` (bundled fonts) + `rustybuzz` for wrapping |
| Render checks (optional sidecar) | `chrome-headless-shell` by digest, driven over CDP with `chromiumoxide` |
| Export | `tar` + `zstd`, `age` (encrypt to a recipient public key) |
| Workspace UI | Leptos 0.8 (SSR + hydration), or `askama`/`maud` templates + small vanilla-JS islands if the spike says so (below). CodeMirror 6 for the source editor (the one JS dependency, vendored as a shared blob) |
| Observability | `tracing` (JSON logs), `metrics` + Prometheus exporter, optional OTLP |
| Tests | `cargo nextest`, golden tests (manifests, diffs, builds), property tests (cap for A never reads B), OAuth conformance, cross-surface conformance, browser e2e with virtual WebAuthn against `*.localhost`, a sandbox compat corpus (30 real Claude-written artifacts and 10 Vite builds rendered under the sandbox in CI with zero uncaught errors) |

**Why Rust, not Go** (the reference is 21k lines of Go):
one static `FROM scratch` binary with every asset embedded; `rmcp`, `oxc`, `rolldown` and `resvg` in the same binary instead of sidecars; `webauthn-rs` for native step-up; memory safety on an internet-facing authorization server; and the owner's preference.
The cost, stated: porting shartifacts' Go compatibility layer would reach P3 sooner. **Bend 2** was considered and set aside: this service is I/O-bound, and Bend's strength is parallel pure computation.

**Why Leptos, and when it loses.** One language end to end, typed request/response structs shared with the server, and SSR pages that work before any JS loads.
The risks are hydration bundle size on phones, API churn (0.9 is in release candidates; pin 0.8.x) and CodeMirror interop.
**Decision rule for the P0 spike**: if building the share sheet costs more than a day of framework friction, or the hydrated bundle for the part view exceeds 300 KB brotli, use `askama`/`maud` server templates plus small vanilla-JS islands (share sheet, editor, SSE). It's still Rust.

**Why a child process for builds**: see §3d. In short, a hang or panic in young compiler code costs one build, not the server.

**Maturity check before P0 freezes**: one-day spikes on rolldown/oxc as libraries, rmcp stateless Streamable HTTP, Leptos SSR vs templates, and the **opaque-origin sandbox on real browsers**. The sandbox spike checks, on Chrome, Safari, iOS Safari and Firefox:
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
11. *Cloudflare adapter only:* the wildcard CNAME to `<tunnel>.cfargotunnel.com` serves a new `*-part` name.

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
| Card render (tier a), p99 | < 20 ms |
| Repeat open of a part from a phone, end to end (the wrapper and entry HTML always reach the origin; shared libs from cache) | < 150 ms on a good uplink, measured from real devices and reported, not gated |
| Static-file throughput vs nginx and HAProxy on the same box and files | ≥ parity |

## 5b. Builds and deploys

- **Parts need no build infrastructure.** TSX/CSS/Markdown compile in the build child at save time, CDN references are mirrored at build time, and a part that needs real npm packages
  is built by the agent locally (`vite build`) and uploaded as `files`. Nothing to install, and no containers per part.
- **The deliverable is three things, none tied to a CI vendor**: an **OCI image**, a **compose file**, and a **documented env contract**.
  - **Image**: `cargo-chef` layer caching → static musl binary → `FROM scratch` image (~20 MB), amd64 + arm64, with an SBOM, signed with `cosign`, pushed to any OCI registry and **deployed by digest**, never `:latest`.
    Every tag also publishes the image and the `part` CLI as release assets (the passive GitHub mirror's Releases in the example deployment), so a laptop drill doesn't depend on the CI host. A registry holds only images, which CI can rebuild from git, so backing it up is optional.
  - **Compose file** (`deploy/compose.yaml`): partifactory, the `/data` volume, the optional render sidecar behind a profile, and an optional proxy service (Caddy by default, behind a profile) for hosts that have none. Example proxy configs for Caddy, nginx and Traefik live in `deploy/proxy/`.
  - **Env contract** (`docs/env.md`, generated from the config loader so it can't drift): every setting is `PF_*`, and every secret also accepts `PF_*_FILE` (a path, which is how Docker/compose secrets and most secret stores mount them). Required: `PF_SESSION_KEY`, `PF_CAP_HMAC_KEY`, `PF_DOMAIN_APEX`, and the OIDC client secret if `[idp]` is set.
    Optional per adapter: `PF_DNS_API_TOKEN`, `PF_TUNNEL_TOKEN`, `PF_IDP_PROVISIONER_TOKEN`, `PF_ALERTS_WEBHOOK_SECRET`, `PF_CDN_ANALYTICS_TOKEN`. `partifactory config check` validates a set and names any missing variable.
- **Deploying is `docker compose pull && docker compose up -d`.** That works by hand, from cron, or from any CI. `partifactory deploy` wraps it with a health gate: start the new container next to the old one, wait for `/readyz`, switch, and **roll back automatically** if health fails within 2 minutes.
  Schema migrations run before the switch and must be forward-compatible with the previous binary.
- **CI is an example, not a dependency.** The repo ships `examples/ci/gitea-actions.yml` (what the example deployment runs: build, sign, push, then `partifactory deploy` over SSH from a protected runner) and `examples/ci/github-actions.yml`. Secrets come from whatever the CI or host provides, as env vars or files. Nothing reads a CI vendor's API.
- **The host contract.** partifactory never calls ZFS, TrueNAS or any host API. A host must offer:
  1. an OCI runtime with compose (Docker or Podman);
  2. one data volume mounted at `/data` (SQLite, blobs, backups, exports). There's no other state anywhere;
  3. env vars or secret files per the env contract;
  4. inbound HTTP from the proxy on the same host or network, or outbound network access for a tunnel adapter.
  TrueNAS is just one such host (the example deployment's). Its snapshots are an optional, host-provided backup layer (§5c).
- **The render sidecar** is a second service in the same compose file, behind a profile, so it's off unless site config turns it on.
- **Day to day**: `cargo run`/`bacon` against `*.localhost`. `part` CLI binaries ship as release assets + `cargo binstall`.
- **If backends ever happen**: WASM components on `wasmtime` first, a container per app only as a fallback.

## 5c. Backups, recovery and self-provisioning

**Backup = the `/data` folder**, which is shartifacts' model too ("back up the data folder"). Versioning and rollback stay inside partifactory: immutable versions in SQLite, content-addressed files under `/data`, history, diff, restore-as-new-version and the 30-day bin (§3e).
No external system is involved in any save, and **no save ever depends on a backup target**.
- *Why:* part content is what matters, and one folder with no other state is something every host already knows how to back up. A git archive was considered and rejected (Chris: no git or Gitea dependency for parts), and so were Litestream and a built-in streaming sink (§10).

```
/data/
  partifactory.db (+ -wal, -shm)   all metadata, grants, tokens, comments, per-part data
  blobs/ab/cd/<sha256>             content-addressed, write-once
  backup/partifactory-latest.sqlite   hourly consistent VACUUM INTO copy (for non-atomic host tools)
  exports/partifactory-<ts>.tar.zst   scheduled exports (optional, keep N)
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
- `blobs/<sha256>` for every blob referenced by any version, build, draft, data snapshot or asset.
- **Parts only** mode (`--parts-only`) writes part manifests, files, notes, titles, spaces by slug and tags as JSON instead of the DB. It has **no access records, tokens or people**. It's what you'd hand to someone else, or keep somewhere less trusted.
- **Optional age encryption** (`--encrypt`, or always on in site config) to an age **recipient public key**, so the box never holds the decryption key. The identity lives offline in the password manager.
- **Scheduled**: `backup.export_schedule = "nightly"`, `backup.export_dir`, `backup.export_keep = 7`. The newest export's time and size are metrics.

**`partifactory import <file>`** restores on any OCI host:
1. verify every entry's hash against `MANIFEST.json`;
2. load `db.sqlite` into `/data` and run forward migrations;
3. write blobs (temp → fsync → rename);
4. **bump the boot epoch**, so caps minted before the export can never come back;
5. start serving. `/readyz` reports `importing=true` until blobs are verified.
- Sessions and OAuth tokens in the export are valid again, which is the point of a full export. Refresh tokens issued after the export point are unknown, so those agents sign in again, and refresh-reuse detection is suspended for 24 h after an import to avoid mass family revocations.
- **Parts-only import** recreates every part with its full version history, author labels and timestamps, in a fresh DB. **Access must then be re-granted and agents reconnected**: that's the honest cost of not carrying the access records.
  A helper proposes grants by matching stored audience names (groups by name, people by username or email) for Chris to approve on one screen.

**Per-part data** (`storage`/`db` documents) lives in the same DB, so it's in every copy of `/data` and every full export automatically.
**Data snapshots** are a separate, per-part, point-in-time history of a part's data, for **undoing a bad write, not for disaster recovery**:
- **on by default**: `daily`, keep 7, starting the first time a part stores data. Nobody is asked. Switch it off per part in Details (or an agent sets `snapshots: "off"` at create); `hourly` (keep 48) is the other option;
- stored as blobs (canonical JSON of the part's documents), and skipped on days when the data didn't change, so an idle part costs nothing new;
- it stops after N days (default 90) without the part being opened, and **after that keeps the single most recent snapshot** until the part is deleted and leaves the 30-day bin;
- restore a snapshot from Details in one action (the current data is snapshotted first); download the data as JSON any time.
- *Adopted in round 2b:* the Fable review argued for per-part data snapshots on by default, and Chris now agrees. A bad write by an agent or a viewer is exactly what an undo is for, and nobody remembers to opt in before they need it.
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

**Logs**: structured JSON to stdout (`tracing`), with one line per request carrying the request id (from `[ingress] request_id_header`, e.g. `CF-Ray` or `X-Request-Id`, or generated), route, status, latency, principal/actor (never tokens, caps, cookies or part content).
Secrets are redacted by type at the logging layer, not by convention, and `/c/<…>/` path segments and `pf_*_` tokens are redacted by pattern. Log levels change at runtime from the admin area.

**Metrics** (`:9090/metrics`, never routed through the ingress): request rate, errors and latency per route class, **hits on non-`-part` hostnames** caught by the wildcard (a future service whose DNS record is missing shows up here first),
grant decisions, OAuth events, build times and outcomes, render queue depth, blob store size, SQLite WAL size, disk headroom, newest export age, last drill result, and edge cache hit ratio (only with a CDN adapter's analytics token).

**Health and alerting**: `/healthz` (process up), `/readyz` (DB writable, blob store OK, disk headroom > 5 %). **IdP reachability, the directory poll and backup freshness are degraded indicators, not readiness**:
when the IdP is down, existing sessions, caps and agent refresh tokens keep working and only new sign-ins fail. *Why:* tying readiness to the IdP would roll back deploys and show the maintenance page exactly when the product is fine.
An admin **Health** page shows the same, plus backup layers, version, disk use naming the biggest spaces and parts, error-rate spikes, quota hits, the last drill and directory staleness.
**Ops alerts are optional and off by default** (§4): one generic webhook (`[alerts] webhook_url`) for disk ≥ 90 %, no export in 48 h, the build worker crash-looping, and the IdP unreachable for 10 minutes. ntfy, Hermes or anything else can consume it. The Health page shows the same things.
Uptime is watched from outside by any probe against `/healthz` (the Cloudflare adapter can use its health checks on Pro and up).
When the origin is down, the proxy serves the static maintenance page from `deploy/proxy/` instead of its raw error (the Cloudflare adapter serves it from a Worker instead of a 1033). The ops listener is reachable on the local network, so `doctor` and a laptop can tell ingress-down from box-down.

**Analytics, first-party and privacy-respecting**: views per part per day and unique viewers (signed-in = principal; public-link viewers = a daily-salted hash of IP + UA, never stored raw; the salt lives in memory and rotates at UTC midnight),
per-public-link open count and last opened, and last viewed. Shown on each part's Details and in an admin overview ("what's actually used"), and readable by agents via `part_get`. No third-party scripts, no cross-part tracking.
Raw view events are kept for 90 days, then rolled up. Collection starts in P1 (in the content listener), so nothing is lost before the P4 overview.

**Abuse prevention**:
- **Rate limits** per IP and per principal (`governor`) on `/grant`, `/oauth/*`, `/device`, `/l/*`, `/mcp`, `/api`, `/_rt` writes, invite acceptance and comments. This is the real control, and it keys on the client address from the trusted proxy header (§3).
  An edge rule is an optional backstop. With the Cloudflare adapter, Free allows **one** rate-limit rule (IP only, 10 s window), so the adapter installs one combined expression over those paths.
- **Body and size caps**: 2 MiB per source file, 25 MiB per media blob or asset, 8 MiB per request, 256 KiB per db document; request bodies are capped before auth on unauthenticated routes.
- **Quotas** per space and per agent connection (parts, versions/day, bytes). An agent that loops hits a quota, gets `quota_exceeded`, and the hit shows on the Health page, rather than being discovered when the disk fills.
- **Invites**: counted uses, expiring, a cap on outstanding invites, and enrolment is rate-limited per invite and per IP.
- **Public links**: view-only by default, per-link request caps, a kill switch ("revoke all public links"), and `noindex` + `X-Robots-Tag` everywhere.
- **Outbound fetches** (mirror-on-build, CIMD): one egress guard (§2), so neither can be used to reach internal addresses.
- **`part_check`**: 30 per hour per connection, concurrency 1.

**Maintenance**:
- **Blob GC**: mark-and-sweep from every version of live and binned parts, every build, draft, data snapshot and asset. Unreferenced blobs are deleted after a 7-day grace period. Every version is kept (§3e), so in practice GC collects superseded build output, abandoned uploads and purged parts.
- **Disk pressure**: blobs are written temp → fsync → hash-verify → rename, so a partial blob never appears under its hash. **At 95 % used the writer refuses new versions, assets and `/_rt` writes with `storage_full`**, while reads, grants and revocations continue. Raw analytics and old events are trimmed first.
- **Events** are kept 30 days; a `part_watch` cursor older than that gets `cursor_expired`.
- **Scratch parts** (`expires_in`) move to the bin when due.
- **Migrations**: forward-only, embedded, run at start inside a transaction, tested by the drill against a production-shaped export.
- **Dependencies**: Renovate (it runs against any forge), plus `cargo audit` + `cargo deny` (licences, advisories) in CI, base images pinned by digest.
- **`partifactory doctor`**: checks config and the env contract, DNS records for `parts` and the wildcard, ingress (a request through the public name reaches the right listener with the trusted client-IP header; the tunnel, with that adapter), cert coverage and expiry, IdP discovery and reachability, provisioner permissions (Authentik: expects 403 on a forbidden group op, once the gate has run), backup layers, export freshness and disk.

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
- Agents are `agent_connections`, one per (agent, person) or per service, and every kind of token hangs off one: OAuth auth code, device grant, or personal agent token (`pat`). One revoke covers everything an agent holds.
- There are no preview tokens. The only bearer links are `public_links`, which only Chris creates.

**Capability claims** (not a table): `{p, h, s, r, ro?, e, m, me, b, w, v?, d?}`, as in §2. Verification reads only the in-memory mirror of `epochs`.

The DDL below loads cleanly in SQLite 3.54 and is the source for the P0 schema migration (T06), whose test asserts every table, index and `CHECK` here.

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
  purpose     TEXT NOT NULL CHECK (purpose IN ('login','stepup')),  -- login: local accounts; stepup: the admin
  passkey     BLOB NOT NULL,                         -- serialized webauthn-rs Passkey
  label       TEXT NOT NULL,
  created_at  INTEGER NOT NULL,
  last_used_at INTEGER,
  revoked_at  INTEGER                                -- set by a recovery link or by Chris
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
  slug               TEXT NOT NULL UNIQUE,           -- '~chris', 'family', 'system'
  kind               TEXT NOT NULL CHECK (kind IN ('personal','shared','system')),
  title              TEXT NOT NULL,
  owner_user_id      TEXT REFERENCES users(id),      -- personal: the person; shared: NULL (Chris administers)
  quota_parts        INTEGER,
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
  kind             TEXT NOT NULL CHECK (kind IN ('html','markdown','svg','code','tsx','slides','files')),
  head_version_id  TEXT,                             -- newest committed source version (NULL = v0 placeholder)
  live_version_id  TEXT,                             -- newest version with an ok build
  live_build_id    INTEGER,                          -- the live pair is (live_version_id, live_build_id)
  head_n           INTEGER NOT NULL DEFAULT 0,
  owner_user_id    TEXT NOT NULL REFERENCES users(id), -- the person it was made for; shares inward
  steward_conn     TEXT,                             -- agent that receives to_agent comments (P3)
  viewers_write_data INTEGER NOT NULL DEFAULT 1,     -- 'viewers can update data' (role x _rt matrix)
  link_comments    INTEGER NOT NULL DEFAULT 0,       -- opt-in: public-link viewers may comment
  isolation        TEXT NOT NULL DEFAULT 'sandbox' CHECK (isolation IN ('sandbox','unsandboxed')),  -- admin-only flag
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
  status           TEXT NOT NULL CHECK (status IN ('queued','running','ok','failed','timeout','superseded')),
  output_hash      TEXT REFERENCES blobs(hash),      -- build manifest blob
  warnings         TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(warnings)),
  errors           TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(errors)),
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

-- Public links: Chris-only, made after publishing, no expiry by default. There are no agent-minted preview links.
CREATE TABLE public_links (
  id            TEXT PRIMARY KEY,                    -- l_...; also the cap subject link:<id> and minter
  token_hash    BLOB NOT NULL UNIQUE,                -- sha256 of the 128-bit /l/<token>
  part_id       TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  role          INTEGER NOT NULL DEFAULT 0 CHECK (role IN (0,1)),  -- view; comment only with parts.link_comments
  screenshot_og INTEGER NOT NULL DEFAULT 0,          -- opt-in: screenshot as the link preview
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

CREATE TABLE share_requests (
  id             TEXT PRIMARY KEY,
  part_id        TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  requester_kind TEXT NOT NULL CHECK (requester_kind IN ('agent','user')),  -- people ask for access too
  requester_user TEXT REFERENCES users(id),         -- NULL only for a service identity
  requester_conn TEXT,
  kind           TEXT NOT NULL CHECK (kind IN ('access','grant','public_link','invite','group','everyone')),
  subject        TEXT,                               -- for access/grant/everyone
  role           INTEGER CHECK (role BETWEEN 0 AND 3),
  reason         TEXT,
  status         TEXT NOT NULL CHECK (status IN ('pending','approved','denied','expired')),
  created_at     INTEGER NOT NULL,
  decided_at     INTEGER
) STRICT;
CREATE INDEX share_requests_pending ON share_requests(created_at) WHERE status = 'pending';

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
  status          TEXT NOT NULL CHECK (status IN ('pending_approval','sent','opened','enrolled','expired','revoked')),
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
  quota_versions_day INTEGER,
  CHECK ((principal_kind = 'person') = (user_id IS NOT NULL))
) STRICT;
CREATE INDEX agent_connections_user ON agent_connections(user_id) WHERE revoked_at IS NULL;

CREATE TABLE oauth_tokens (
  hash        BLOB PRIMARY KEY,                      -- sha256 of opaque token
  kind        TEXT NOT NULL CHECK (kind IN ('access','refresh','pat')),
  prefix      TEXT NOT NULL,                         -- 'pf_at_' | 'pf_rt_' | 'pf_pat_' (secret scanning, logs)
  connection_id TEXT NOT NULL REFERENCES agent_connections(id) ON DELETE CASCADE,
  family_id   TEXT,                                  -- refresh rotation family
  resource    TEXT NOT NULL,                         -- RFC 8707 audience: always the workspace origin, e.g. https://parts.dilger.dev/
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

CREATE TABLE idempotency (                           -- part_create request_id, and any write with request_id
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
  updated_by TEXT NOT NULL,                          -- user:<id> | conn:<id> | link:<id>
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
  blob_hash  TEXT NOT NULL REFERENCES blobs(hash),   -- canonical JSON of the part's data_docs
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
CREATE TRIGGER runtime_errors_keep20 AFTER INSERT ON runtime_errors BEGIN
  DELETE FROM runtime_errors WHERE part_id = NEW.part_id AND id NOT IN
    (SELECT id FROM runtime_errors WHERE part_id = NEW.part_id ORDER BY last_at DESC LIMIT 20);
END;

------------------------------------------------------------------ events, webhooks, analytics
CREATE TABLE events (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,      -- cursor for part_watch / SSE Last-Event-ID; kept 30 d
  at         INTEGER NOT NULL,
  part_id    TEXT,
  space_id   TEXT,
  type       TEXT NOT NULL,                          -- part.created, version.committed, build.finished, part.live_changed, comment.added, ...
  actor      TEXT,
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
  purpose     TEXT NOT NULL CHECK (purpose IN ('check','thumbnail','og')),
  requested_by TEXT,                                 -- conn:<id> for part_check
  status      TEXT NOT NULL CHECK (status IN ('queued','running','ok','failed','superseded')),
  result      TEXT CHECK (result IS NULL OR json_valid(result)),  -- {screenshot_hash, blank, errors, console, failed_requests, timing}
  queued_at   INTEGER NOT NULL,
  finished_at INTEGER
) STRICT;
CREATE INDEX render_jobs_queue ON render_jobs(status, queued_at) WHERE status IN ('queued','running');

CREATE TABLE view_events (                           -- raw, 90 d
  at          INTEGER NOT NULL,
  part_id     TEXT NOT NULL,
  viewer_key  BLOB NOT NULL,                         -- user id hash, or daily-salted hash(IP, UA) for links
  via         TEXT NOT NULL CHECK (via IN ('session','public_link','frame')),
  referrer_host TEXT
) STRICT;
CREATE INDEX view_events_part_at ON view_events(part_id, at);

CREATE TABLE view_daily (
  part_id TEXT NOT NULL,
  day     INTEGER NOT NULL,                          -- days since epoch, UTC
  views   INTEGER NOT NULL,
  uniques INTEGER NOT NULL,
  PRIMARY KEY (part_id, day)
) STRICT, WITHOUT ROWID;

CREATE TABLE kv (                                    -- singletons: directory poll cursor + staleness, provisioner tier, last export, last drill, alert state
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) STRICT, WITHOUT ROWID;

------------------------------------------------------------------ search
CREATE VIRTUAL TABLE parts_fts USING fts5(
  part_id UNINDEXED, title, description, tags, body, notes, comments,   -- body = extracted readable text, not code
  tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3'
);
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
| `runtime_errors` "latest 20 per part" | a trigger; inserts are rate-limited per part (≤ 10/min), so the cost is negligible |
| Public-link uniques | a daily-salted hash; the salt is in memory and lost on restart, so uniques over-count on restart days. Accepted |
| Agent-made grants | `grants.created_by_conn` labels them and drives "Undo all" on a revoked connection |
| Every version kept | GC marks from `version_files`, `build_files`, `drafts`, `data_snapshots` and `assets` |

## 5f. Portability: what's core and what's an adapter

Chris doesn't want partifactory to depend on the particular stack he runs today. So every coupling to it is an **adapter behind config**, and each concern has a **generic default with no vendor in it**.
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
| **Embedding** | `frame-ancestors` from `[embed] frame_ancestors`; `pf-embed.js` | Empty: only the part's own host and the workspace may frame it | The Nest is one entry in the list | Nothing; other sites just can't frame parts |
| **Hostnames** | `[domain] apex, workspace_host, part_suffix`, used everywhere | Required config, no default | `dilger.dev` is the example deployment | — |
| **Device-approval origin** | Optional country + network from proxy headers or a local GeoIP file | Omitted, shown as "origin unknown" | Cloudflare country header | The "different network" phishing banner can't fire; the request-age banner still does |

**Proving it: the portability lane.** A CI job runs the **full e2e suite** (the §4a.7 click budgets, the §7 checks that don't name an adapter, cross-surface conformance and an export/import round trip) with **no Cloudflare, no Authentik and no Gitea**:
- **Dex** as the OIDC provider (static users and groups, refresh tokens on), with a second pass using no IdP at all (local passkey accounts only, admin via `partifactory admin bootstrap`);
- **Caddy** in front with a locally issued wildcard cert for a test domain, using `X-Forwarded-For` as the client-IP header;
- **local exports** to a folder, then `import` into a fresh container;
- run from the published compose file with secrets as `_FILE` mounts, on a stock CI runner (the workflow is written for GitHub Actions, the same steps a shell script runs locally).
It gates every release, so a hidden dependency on the example deployment's stack fails CI the day it's introduced (T93).

**What stays coupled, on purpose** (and why):
- **One registrable domain** for the workspace and all part hosts. The `/grant` flow and the wrapper's chrome API rely on the session cookie being same-site with every part host. That's a property of DNS, not of any vendor.
- **WebAuthn passkeys** for step-up and local accounts. They're a W3C standard implemented in the binary, and passkey-only sign-in is what makes phishing via parts inert (§3).
- **OCI images + compose** as the deployment unit. Both are open standards with several implementations, and the alternative (OS packages per distro) is more coupling, not less.
- **The mirror allowlist's default CDNs** (cdnjs, jsDelivr `/npm/`, unpkg, esm.sh, Google Fonts). They're what Claude-written pages reference, so they're a compatibility target, not infrastructure; the list is config.
- **The Telegram in-app browser in the real-device pass.** It's where the household opens links, so it stays in the test matrix; nothing in partifactory talks to Telegram.

## 6. Phases

Each phase ends with something usable and has its own "Done when". The order puts the riskiest technology first (sandbox, compile), then the daily loop (agents), and invites last among the core, because the household already exists in the example deployment's IdP. Adapters for the example deployment (Cloudflare, Authentik) land in the same phase as the generic default they sit beside, never before it.
- *Why agents before invites:* the first moment of daily value is "Hermes makes a thing from Telegram and I open it on my phone". Invites serve people who aren't users yet, and they carry the most identity work (local accounts, and the Authentik provisioner for the example deployment).
- *Why the sandbox and TSX in P0:* the serving model and the compiler are the two things most likely to force a redesign. Finding a rolldown or Safari problem in P3 would be finding it last.

**P0 Core (local).** Repo, schema + migrations, blob store (temp → fsync → rename), source manifests + build records, the build child process, HTML/Markdown/SVG/code **and TSX** kinds with the golden corpus,
mirror-on-build, the content listener with **path caps and the canonical sandbox CSP**, the prelude with the `localStorage` shim and `runtime_errors`, the operation registry, the HTTPS API with `/openapi.json`, the `part` CLI (dev token),
logs + metrics, and a same-machine regression bench. Spikes: rolldown/oxc, rmcp, Leptos vs templates, **the sandbox on real browsers** (§5), including the wrapper flow, and the Authentik adapter spike (conditional UI, blueprint-over-API, `groups` claim, membership by API, recovery-link permission).
*Done when:* `part put x.html --name packing` opens at `packing-part.localhost:8081/c/<cap>/` under the sandbox, a Claude-written page using `localStorage` and a CDN `<script>` runs unchanged there, and the TSX corpus compiles.

**P1 Edge + identity.** The ingress contract with example proxy configs, `provision` with the manual and Cloudflare providers (the example deployment's tunnel and wildcard), generic OIDC login + the claims re-read directory, the Authentik directory adapter, `/grant` → the part host,
the **part-host wrapper** with its chrome API and CORS, policy engine + audience ladder + spaces + the role × runtime matrix, admin pin + native passkey step-up,
the Stockroom workspace (gallery, search, history, diff, preview old versions, restore, bin, share sheet, Move to Family), Chris-only public links on the part host with open counts, tier (a) cards,
rate limits, quotas, health page with optional ops alerts, backups (consistent DB copy, `export`/`import`, scheduled export, per-part data snapshots on by default), the first drill on a second OCI host, the image + compose + env contract with an example CI workflow, `doctor`,
the **portability lane** (full e2e with Dex, Caddy and local exports, §5f), and the **nginx/HAProxy comparison on the real content path**.
Real-device pass, including the Telegram in-app browser with passkeys (the open item in §2).
*Done when:* Chris opens a part on his phone at its own host link via SSO; the portability lane is green; a public link opens from mobile data with no login and unfurls as a card; the security checklist (§7) passes from outside; and the drill restores everything on a second OCI host in < 15 min.

**P2 Agents + the household loop.** OAuth AS (device grant first, then auth code + PKCE + CIMD/DCR + manual clients), personal agent tokens, one connection per (agent, person) and service identities,
MCP + discovery + the error contract + cross-surface conformance, `part_edit` with rebase, block ids, leases, notes, `part_get` full content and `render: "text"`, comments with provenance,
the event log (long-poll, SSE, webhooks), live reload, inward agent sharing (`share:household`) and outward share requests, **`part_check` + the render sidecar (the P2 headline)**, Jig, the source editor with drafts, renames,
and the Hermes skill with per-person pairing.
*Done when:* a household member pairs with Hermes, says "make a packing list and share it with the family" in Telegram, Hermes checks its own render and posts the part's link, another family member opens it on their phone, comments, and watches the fix land;
and Claude Code connects with one passkey tap + Allow and edits a week-old part in one call.

**P2b Invites.** First the **default**: partifactory invite tokens (`/i/<id>`, counted uses), passkey invite links (single and "up to N") that create **local passkey accounts**, passkey sign-in for local accounts, "new passkey link" recovery, local `pf-*` groups, linking a local account to an IdP identity, pending-invite cleanup.
Then the **Authentik provisioner**, starting with its permission gate (§3a), which decides how much of it ships: the `partifactory-invite` flow, add-by-email, IdP recovery links and IdP-side `pf-*` groups.
*Done when:* Chris mints an invite link from the share sheet with no provisioner configured, a new person creates a passkey and lands on the part, and removing them locks them out at once;
and, with the Authentik provisioner on, an invited email address enrols the same way and removing them in the IdP locks them out within a minute.

**P3 Claude compatibility.** `claude.use` (`db`, `user`, `comments`, `artifact` incl. self-publish), shared `window.storage`, Tailwind + shadcn (mechanism chosen on evidence: prebuilt utility CSS, the Tailwind v4 browser build mirrored as a shared blob, or build-time class extraction),
viewer uploads (assets), and the agent inbox with stewards. The spec is shartifacts' compatibility matrix (credited).
*Done when:* a page copied from a claude.ai artifact runs unchanged, including state persistence and self-editing, and a viewer's photo upload reaches the agent's inbox.

**P4 Family + Nest + phone.** Family onboarding, embedding via `[embed] frame_ancestors` (the Nest, with its service identity), the Hermes `nest-family` profile, slides + Mermaid, the analytics overview, screenshot thumbnails for the private gallery and opt-in screenshot cards for public links,
and tap-to-edit text and point-and-comment on phones. (Notifications for people were cut in round 2b: approvals are in-band, §4.)
*Done when:* a family member opens a Nest-embedded part, fixes a typo by tapping it on the phone, long-presses to ask Hermes for a change, and Hermes, watching the event log, tells them in its own chat when the fix lands.

**P5 Later, only if needed.** WASM component backends through the same grant model; optional S3-compatible upload of scheduled exports; semantic search alongside FTS5; built-in ACME DNS-01 for hosts with no proxy.
*Done when* (per item): backends need an ADR naming a real part that needs one, and ship with the §7 suite green; the S3 upload restores via `import --from s3://…` in the drill; semantic search beats FTS5 alone on a labelled query set; built-in ACME serves the portability lane with no proxy in front.

## 6a. Task graph

Generated from `dag2.py` (kept with the planning notes, not in this repo), which also checks for cycles and orphans. **90 tasks and 6 milestones.**
- `M*` rows are phase exits. Each carries its phase's "Done when" as its acceptance test.
- A task is ready when everything under "blocked by" is closed. Ids from round 1 are kept where the task survived; round-2 tasks are T72–T88, and round 2b added T89–T94 (ingress contract, ACME, Cloudflare adapter, Authentik adapter, portability lane, native invites) and removed T86 (notifications for people).
- Phase order is the build order. One later task plugs into an earlier one without an edge: `doctor` (T68, P1) adds the provisioner probe once the gate (T39, P2b) has run.

| id | phase | task | blocked by | blocks | acceptance test |
| --- | --- | --- | --- | --- | --- |
| **T01** | P0 | Cargo workspace, crate layout, CI (fmt, clippy, nextest, cargo deny/audit), Renovate | — | T02, T72, T04, T05, T07, T20, T37 | CI green on a hello-world binary; `cargo deny check` and `cargo audit` run in CI; Renovate opens a PR on a pinned dep |
| **T02** | P0 | Spikes: rolldown/oxc as libraries, rmcp 3.x stateless Streamable HTTP, Leptos SSR vs askama + islands (1 day each, go/no-go notes) | T01 | T60, T31, T53 | Three go/no-go notes in docs/spikes/, each with a runnable example; the Leptos note applies the §5 decision rule |
| **T72** | P0 | Spike: opaque-origin sandbox on real browsers (Chrome, Safari, iOS Safari, Firefox): `'self'` under sandbox, `Origin`/`Sec-Fetch-*` values, ACAO null, module scripts, storage shim, blob workers, Cookie Store API, popups, clipboard; the wrapper flow (`#pf=` fragment through a 302, `replaceState`, explicit part-host `frame-ancestors`, credentialed same-site fetch carrying the Lax cookie); Cloudflare adapter: wildcard CNAME to the tunnel | T01 | T16 | A results table in docs/spikes/sandbox.md, one row per §2 spike item per browser, each pass/fail with the browser version |
| **T03** | P0 | Authentik adapter spike against the real Authentik: conditional-UI passkeys, blueprint-over-API, `groups` claim, group membership by API after enrolment, recovery-link permission | — | T92, T39 | Result table recorded in site config; each item is yes/no with the Authentik version tested; nothing outside the Authentik adapter depends on the result |
| **T04** | P0 | Config + secrets loader (`site.toml` + `PF_*` env, every secret also as `PF_*_FILE`), typed, redacted Debug/Display, generated env contract (`docs/env.md`), `partifactory config check` | T01 | T15, T18, T19, T32, T37 | Missing required secret -> exit 2 with the env var name; a `_FILE` secret loads identically to the env var; a canary secret never appears in Debug, logs or panics; `docs/env.md` regenerates with no diff in CI |
| **T05** | P0 | SQLite layer: read pool, single writer task, pragmas, embedded forward-only migrations | T01 | T06, T34 | Migrations apply from empty and from N-1 in a test; 1,000 concurrent commits through the writer produce a gap-free sequence |
| **T06** | P0 | Schema v1 migration (§5e Data model), incl. FTS5 table and triggers | T05 | T08, T09, T10, T15, T62, T24, T76, T77, T51 | `schema_test` asserts every table, index and CHECK in §5e exists; FK violations fail |
| **T07** | P0 | `BlobStore` trait + local impl: sha256, fan-out dirs, temp -> fsync -> verify -> rename, verify-on-read, `storage_full` at 95 % | T01 | T08, T14, T17, T75, T34 | put/has/get round trip; concurrent put of the same hash is idempotent; a flipped byte on disk is detected; a write at 95 % returns `storage_full` |
| **T08** | P0 | Source manifests + versions: canonical JSON with per-file size/mime, version id = sha256, `n`, parent, head/live pointers | T06, T07 | T11, T12, T13, T34, T59 | Golden manifest hashes stable across runs; a one-line edit to a 40-file part writes exactly 1 blob + 1 manifest |
| **T09** | P0 | Name rules, reservation (ever-used names stay reserved), title->name derivation, deterministic ranked alternatives | T06 | T11, T70 | Rule table test (length, hyphens, reserved list from site config); a retired name is rejected; `name_taken` returns >= 3 valid free suggestions |
| **T10** | P0 | Spaces: personal space per person, `family` shared space, space grants inherited live, per-space quotas config (enforcement in T33) | T06 | T11, T26, T33, T63 | A part created without `space` lands in the creator's personal space; a grant added to a space applies to its existing parts at once |
| **T11** | P0 | Part service: create (request_id, placeholder v0, `from` fork, `expires_in`), get, history, diff of any two versions, soft delete to the 30-day bin, undelete, restore-as-new-version | T08, T09, T10 | T21, T76, T46, T49 | Retried create with the same request_id returns the same part; restore of v3 creates v(n+1) equal to v3; a fork writes zero new blobs; undelete at 29 days works, at 31 fails |
| **T12** | P0 | Kind renderers: HTML passthrough, Markdown (comrak), SVG, code (syntect) | T08 | T13, T48, T66 | Golden render corpus passes for each kind |
| **T13** | P0 | Build worker: child process (same binary re-exec), 10 s deadline, memory cap, state machine, build records keyed by hash(manifest, toolchain), brotli, live-pointer switch | T08, T12 | T14, T16, T60 | A build that panics, hangs or exceeds memory marks the build failed/timeout, the server keeps serving, and `live` is unchanged; `wait:true` returns within 2 s or `build.status=running` |
| **T14** | P0 | Mirror-on-build: CDN allowlist (jsDelivr `/npm/` only), transitive rewrite of build output (source untouched), SRI, unpinned -> pinned, font CSS one level deep, limits, SSRF guard, url->hash cache | T07, T13 | T60, M0 | Fixture page with a cdnjs `<script>`, Google Fonts CSS and a jsDelivr `+esm` module renders with zero third-party requests; the source's anchors still match; an off-allowlist redirect and a private-IP resolution fail with `mirror_failed` |
| **T15** | P0 | Capability tokens: path-safe format, HMAC (`ring`) with `kid` rotation, deterministic per 12 h window, per-part/per-minter/global epochs + boot-epoch floor (memory + SQLite) | T04, T06 | T16, T25, T35, T80 | Property test: a cap for part A never verifies on host B; same subject+window gives the same cap; bumping an epoch or the boot epoch rejects older caps; a rotated key verifies for 25 h then fails |
| **T16** | P0 | Content listener: `-part` host routing + non-part hit metric, `/c/<cap>/` paths, canonical CSP with `sandbox` and explicit `frame-ancestors` (own host, workspace, `[embed]` list), ACAO null + Vary, no CORP, no Set-Cookie, live-version cache, `/_b/<hash>`, ETag/304, mmap LRU, `unsandboxed` flag | T13, T15, T72 | T17, T62, T23, T27, T89, T29, T51, T80, T67 | `curl` on `/c/` without cap -> 302 to /grant?mode=direct; with cap -> 200 + the canonical CSP and no `Set-Cookie`; `Origin: https://evil.example` is rejected; 304 on matching ETag with no DB query (query counter) |
| **T17** | P0 | Shared blob host (`blobs-part`): serves only hashes in `shared_blobs`, immutable public caching, ACAO `*` | T07, T16 | T60, T23, T54 | A part-class hash returns 404 on the blob host; a shared hash returns `Cache-Control: public, max-age=31536000, immutable` and never `Set-Cookie` |
| **T62** | P0 | Prelude + `/_rt/v1`: one text/plain POST endpoint, `localStorage`/`sessionStorage` shim synced to personal state, inert `document.cookie`, no `indexedDB`, fetch/XHR/EventSource prefixing, blob-URL workers, `crossorigin` images, `pf:regrant`, runtime error reporting | T16, T06 | M0, T77, T38, T80, T79, T73, T83, T85 | A Claude-written page that uses `localStorage` keeps its state across reload and across two browsers for the same person; no `OPTIONS` request is ever sent; uncaught errors appear in `runtime_errors` |
| **T60** | P0 | TSX/JSX: oxc transform + rolldown bundle in the build child, pre-seeded library set as shared blobs, import map frozen into the build record, CDN-URL aliasing to the set, golden compile corpus | T02, T13, T14, T17 | M0, T61 | Golden corpus (compat matrix + Claude samples) compiles; a page importing React from a CDN URL and bare `react` loads one React; a library upgrade leaves old builds' import maps unchanged |
| **T18** | P0 | Logs + metrics: JSON `tracing` with type-based redaction (incl. `/c/<cap>/` and `pf_pat_`), Prometheus on :9090, /healthz, /readyz skeleton | T04 | M0, T89, T36 | Canary token/cap/content values never appear in logs under the test suite; /metrics exposes route-class latency histograms |
| **T19** | P0 | Dev/test identity fake: `Authn` + `Directory` traits with a static-file implementation (shared by unit tests; the e2e suites use Dex) | T04 | T21, T24 | Integration tests can sign in as fixture users and groups without a network |
| **T20** | P0 | Operation registry crate: typed inputs/outputs (`serde` + `schemars`, `deny_unknown_fields`), scopes, hints, executable examples, one error catalogue with HTTP-status and CLI-exit maps | T01 | T21, T53, T80, T56 | Snapshot test of generated JSON Schemas; adding an op or error without a description, hint and example fails the build |
| **T21** | P0 | HTTPS JSON API (`POST /api/v1/{op}` + GET aliases for reads) generated from T20, `/openapi.json` (OAS 3.1), blob upload with have/need | T11, T19, T20 | T22, T30, T31, T33, T50, T51, T56 | OpenAPI validates; every route is in the document and vice versa; `part put ./dist` twice uploads nothing the second time |
| **T22** | P0 | `part` CLI generated from the registry (`part <op> --json`) + ergonomic aliases, `PARTIFACTORY_URL`/`PARTIFACTORY_TOKEN`, dev-token auth | T21 | M0, T57 | `part put x.html --name packing` then `part get packing` returns the same bytes; exit codes follow the §4b map |
| **T23** | P0 | Regression bench (same machine, previous release as baseline) | T16, T17 | M0, T74 | `partifactory bench` writes a results JSON; a >10 % regression on the fixture fails the bench job |
| **M0** | P0 | Milestone: P0 done-when | T22, T23, T14, T18, T60, T62 | — | `part put x.html --name packing` opens at `packing-part.localhost:8081/c/<cap>/` under the sandbox; a Claude-written page using `localStorage` and a CDN `<script>` runs unchanged; the TSX corpus compiles |
| **T24** | P1 | Generic OIDC client (discovery document, confidential + PKCE, any provider; claim names from config), workspace session cookie, sign out everywhere | T06, T19 | T25, T27, T28, T43 | Login round trip in e2e against Dex; the same binary signs in against a second provider config with no code change; sign out everywhere revokes all sessions and bumps the user's minter epoch |
| **T25** | P1 | Generic directory: `groups` claim at login + 15-minute refresh-token re-read per session (encrypted refresh token), `invalid_grant` ends the session, local `pf-*` groups; membership diff -> epoch bumps; unreachable IdP keeps last-known groups (stale flag) | T24, T15 | T26, T92, T93 | Removing a user from a Dex group fails their next part request after the next re-read (test clock); deleting the user ends the session on `invalid_grant`; with the IdP down, existing sessions keep working and no epoch is bumped |
| **T26** | P1 | Policy engine: subjects, roles, effective = max(part, space grants), `everyone-signed-in` = audience group + local accounts, role x `/_rt` matrix, expiry timer, audit rows | T10, T25 | T27, T31, T43, T50, T52, T94, T83 | Property test: no subject gets a role above its best matching grant; a `view` grantee can write shared data only when the part allows it; every grant change writes one audit row |
| **T27** | P1 | `/grant` hand-off: path-only input, session auth, policy decision; `wrap` mode 302s to the part host with `#pf=<cap>`, `direct` mode to `/c/<cap>/<path>`; credentialed regrant endpoint for wrappers and embedders; 403 page with Ask for access | T16, T24, T26 | T29, T30, T74, T38, T70, T73, T64 | Open-redirect fuzz: no input yields a Location outside the part's host; `wrap` puts the cap only in the fragment; an expired cap on a navigation re-enters /grant silently; Ask for access creates a `share_requests` row |
| **T28** | P1 | Admin pin (subject in config: `oidc:<issuer>#<sub>` or `local:<id>`) + native WebAuthn step-up (10 min window, admin actions only) + `admin bootstrap` for IdP-less deployments + console break-glass | T24 | T29, T36, T38, T43, T70, T94 | IdP-admin user gets 403; an admin action without a fresh assertion returns `step_up_required`; household sharing never asks; `admin bootstrap` registers a local admin passkey once; break-glass works once within 5 min and is audited |
| **T75** | P1 | Tier (a) cards: SVG template -> resvg PNG 1200x630 in-binary, bundled fonts, Rust-side wrapping, content-addressed `/og/<sha256>.png`, private vs public variants | T07 | T29, T81 | A private card's input struct has no part-derived field (test); a title change yields a new image URL; render < 20 ms p99 |
| **T89** | P1 | Ingress contract: trusted-proxy client IP header + `trusted_proxies`, request-id header, example Caddy / nginx / Traefik configs and the static maintenance page in `deploy/proxy/` | T16, T18 | T30, T91, T68, T93, T90 | Each example proxy config passes a smoke test (routing by Host, SSE survives 60 s, 8 MiB body); a spoofed client-IP header from an untrusted address is ignored; the request id appears in logs and error pages |
| **T29** | P1 | Part-host root: the trusted wrapper page (cap from `#pf=` then sessionStorage, `replaceState`, deep-link paths, og tags content-free unless a public link is active, `<noscript>` sign-in link), chrome API + CORS for exactly sandboxed part-host origins (origin bound to part), `/p/<name>` redirect, Chris-only public links at `<name>-part/l/<token>` (no default expiry, open count, last opened, revoke, kill switch) | T16, T27, T28, T75 | T31, T38, T93, T52 | A signed-out fetch of a part root returns the private card and no part bytes; the address bar ends clean after first open and a reload makes no /grant hop; the chrome API refuses `Origin: null`, a sibling part's origin, an unsandboxed part's origin and any non-part origin; a public link opens from a fresh browser with no login and its open count increments; revoked -> 404 on the next request; bot and browser get byte-identical responses |
| **T30** | P1 | Rate limits (`governor`, per path and principal, keyed on the trusted client-IP header), pre-auth body caps | T21, T27, T89 | T38 | Each limited route returns 429 at its threshold in a test |
| **T76** | P1 | Search: FTS5 over title, description, tags, notes, extracted readable text (not code) and comments; filters space/kind/maker/date/has-data; bm25 + recency | T06, T11 | T31, T49, T88 | Over a 10k-part fixture p99 < 5 ms; code inside a TSX file does not match; a newer part outranks an equally relevant older one |
| **T31** | P1 | Workspace UI (Stockroom): gallery linking to part hosts, search sheet, the wrapper's chrome (title, audience chip, share sheet, versions rail, comments rail, agent activity, request chips), history ledger, diff any two, preview old version, restore, bin, install as home-screen app per part host | T02, T21, T26, T29, T76 | T77, T93, M1, T55, T79, T94, T67, T81, T85 | e2e: search, open a part host, compare v1 vs v3, restore v1 as v4, move to Family from the wrapper's share sheet, delete and undelete; §4a.7 click budgets asserted; contrast test passes |
| **T32** | P1 | `partifactory provision` framework: `DnsProvider` + `IdpProvisioner` plugin traits, `manual` providers (print + verify records, cert names and the OIDC redirect URI), ownership rules, `--dry-run`, run log | T04 | T91, T92, T68, T90 | Manual mode prints exactly the two records and the redirect URI and verifies them by lookup; a second run is a no-op (`--dry-run` shows no changes); only owned resources are modified; before-states go to the site repo's run log |
| **T91** | P1 | Cloudflare adapter: tunnel ingress, DNS provider plugin (`parts` + wildcard records gated on readyz), cache rules for `/b/*` and `/og/*`, one combined rate-limit rule, fallback Worker, health check or external probe, `CF-Connecting-IP` as client-IP header | T32, T89 | T38 | Against a test zone: a second run is a no-op; only owned resources change; a shared blob shows `cf-cache-status: HIT` on the second fetch and a part blob never does |
| **T92** | P1 | Authentik adapter: `Directory` over the API with a 60 s poll, and the `IdpProvisioner` plugin (OIDC app, cloned flows, invite flow, provisioner account) | T03, T25, T32 | M1, T39 | Removing a user from a granted group fails their next part request within 70 s; with Authentik down, sessions keep working and no epoch is bumped; provisioning twice is a no-op |
| **T33** | P1 | Quotas: per space and per agent connection (parts, versions/day, bytes), hits shown on the Health page | T10, T21 | T38 | A loop of 10k creates stops at the quota with `quota_exceeded` and one Health-page entry |
| **T34** | P1 | Backups: hourly consistent DB copy in `/data/backup/`, `partifactory export` (tar.zst, optional age, parts-only mode), scheduled export to a folder (keep N) | T05, T07, T08 | T35, T36, T68, T59, T87 | An export taken during a write burst imports to an identical part set; a parts-only export contains no grants or tokens (schema test); the scheduled job keeps exactly N files |
| **T35** | P1 | `partifactory import` (full or parts-only, boot-epoch bump, regrant-by-name helper for parts-only) + laptop drill script + drill-result report | T34, T15 | T93, M1 | Drill imports the newest export into a throwaway container on another OCI host in < 15 min and the e2e suite passes; the drill result appears in /metrics |
| **T77** | P1 | Per-part data snapshots: on by default (daily, keep 7) from the first data write, off or hourly (keep 48) per part, skip unchanged days, expiry after N days unopened, keep the last one, restore a snapshot, JSON download | T06, T62, T31 | M1 | A part that never touched its settings has daily snapshots; a bad shared-data write is undone from the snapshot in one action; switching a part off stops new snapshots; after expiry exactly one snapshot remains until the part leaves the bin |
| **T36** | P1 | Health page, /readyz (DB, blobs, disk; not the IdP), degraded indicators, backup-layer status; optional ops-alert webhook (off by default: disk >= 90 %, no export in 48 h, build worker crash-looping, IdP unreachable 10 min) | T18, T34, T28 | T37, T68 | With the webhook set, each alert condition produces exactly one signed POST; with it unset, nothing is sent and Health shows the same rows; IdP down -> /readyz still 200 and a degraded row; no export in 48 h -> amber |
| **T37** | P1 | Deliverable: OCI image (cargo-chef, musl, FROM scratch, SBOM, cosign, by digest) + `deploy/compose.yaml` + env contract; `partifactory deploy` (compose pull/up, health-gated switch + auto rollback); example CI workflows (Gitea Actions, GitHub Actions); image + CLI as release assets | T01, T04, T36 | T38, T93 | `docker compose pull && up -d` from the published compose file serves a part with only env/`_FILE` secrets; a deliberately failing /readyz build rolls back within 2 min and the old digest keeps serving; a tag produces release assets; no step reads a CI vendor API |
| **T68** | P1 | `partifactory doctor` (config + env contract, DNS records, ingress and trusted client-IP header, cert coverage and expiry, tunnel with that adapter, IdP discovery, provisioner 403 probe once T39 exists, backup layers active, export freshness, disk) | T32, T34, T36, T89 | M1 | Each check has a failing fixture with a specific non-zero exit and message; the layer report shows host snapshots as on/off/unknown |
| **T74** | P1 | nginx + HAProxy comparison on the real content path (cap check, `/c/` prefix, brotli), same box, same files | T27, T23 | M1 | Results JSON and a comparison table are committed; the §5a static-file row is filled in |
| **T38** | P1 | Security checklist suite (§7) from outside against staging (the example deployment: Cloudflare + Authentik), plus the real-device pass (iPhone Safari, Android Chrome, Telegram in-app browser + passkey) | T27, T28, T29, T30, T33, T37, T62, T91 | M1 | Every §7 line is an automated test or a recorded manual result; the Telegram passkey result is recorded with the chosen fallback |
| **T93** | P1 | Portability lane: the full e2e suite with no Cloudflare, no Authentik and no Gitea (Dex for OIDC plus an IdP-less local-accounts pass, Caddy with a local wildcard cert, local exports + import), from the published compose file with `_FILE` secrets, gating every release | T25, T29, T31, T35, T37, T89 | M1 | The lane passes on a stock CI runner with no network access to any vendor API; grepping the lane's config and env finds no Cloudflare, Authentik or Gitea setting; removing any adapter crate from the build leaves it green |
| **M1** | P1 | Milestone: P1 done-when | T31, T35, T38, T68, T74, T77, T92, T93 | — | Chris opens a part on his phone at its own host link via SSO; the portability lane is green; a public link opens from mobile data with no login and unfurls as a card; §7 passes; the drill restores everything on a second OCI host |
| **T43** | P2 | OAuth AS (MCP auth 2026-07-28, 2025-11-25 compatible): RFC 9728/8414 metadata, `iss` (RFC 9207), CIMD (SSRF-guarded) + DCR (deprecated, kept), manual clients, authorize + PKCE + RFC 8707 (`resource` = the workspace origin), refresh rotation with reuse detection, revocation, consent sheet | T24, T26, T28 | T44, T45, T52, T53, T56 | OAuth conformance suite passes; refresh reuse revokes the family; a CIMD URL resolving to a private IP is refused; the consent sheet shows the redirect host |
| **T44** | P2 | Device grant (RFC 8628) + `/device` approval page with phishing defences; one connection per (agent, person); service identities | T43 | T58, M2, T64 | `part login` -> code -> approve on phone with a passkey -> token; `slow_down`, `expired_token`, `access_denied` tested; two people pairing Hermes give two connections labelled 'Hermes for <person>' |
| **T45** | P2 | Personal agent tokens (`pf_pat_...`): named, scoped, space-limited, expiring, shown once, listed with connections | T43 | T57 | A PAT works on /api and /mcp with the same scopes as an OAuth token; revoke takes effect on the next call |
| **T46** | P2 | `part_edit` engine: ordered anchor edits, write/delete/rename with `if_match`, rebase inside the writer, request_id replay, error shapes, restore/delete/undelete modes | T11 | T47, T48, T53, T55, T73, T85 | Table-driven test of every error code; two agents editing disjoint anchors from one base both succeed; a retried timed-out edit returns the original result, never `anchor_not_found` |
| **T47** | P2 | Leases: part scope by default, optional file/block, TTL <= 10 min, person override, release on commit | T46 | M2 | Agent B's edit to a claimed file returns `claimed` with holder + expiry; a person's edit succeeds and notifies the holder |
| **T48** | P2 | Block ids for markdown: parse, id carry-over across arbitrary edits, block ops, `pending` blocks, sidecar in manifest | T46, T12 | M2, T66 | Ids survive an anchor edit, a whole-file replace and an editor save in golden tests; parallel `replace_block` on different blocks both commit |
| **T49** | P2 | Agent read path: notes (versioned separately), outline/symbols, `part_get` full content <= 32 KiB, `render:"text"`, `part_find` over T76 | T11, T76 | T53 | Notes edits do not create part versions; `part_get` on a 20 KiB single-file part returns the file; `render:"text"` matches the indexed text |
| **T50** | P2 | Comments: threads, anchors (version + block/file range + quote), `to_agent`, resolve, `provenance` field + MCP untrusted wrapper | T26, T21 | T53, T73, T84, T85 | A public-link viewer's comment arrives with `provenance: link_viewer` and wrapped in MCP text; resolve is audited |
| **T51** | P2 | Event log: long-poll (`part_watch`, `GET /api/v1/events`), SSE with `Last-Event-ID` (25 s heartbeat), signed webhooks with retry, 30-day retention + `cursor_expired`, v0 placeholder live update | T06, T21, T16 | T53, T55, T79, T58, T84 | Kill the webhook target for 5 min: deliveries retry and succeed after recovery; an SSE client resumes with no gap; a 31-day-old cursor gets `cursor_expired` |
| **T52** | P2 | Sharing by agents: `share:household` inward grants and moves (labelled, undoable, listed in Chris's Requests inbox and activity feed), outward share requests (public link, invite, group, everyone-signed-in) approved by Chris in one tap | T29, T43, T26 | T53, T58 | An agent without `share:household` gets `forbidden {scope_needed}`; with it, a grant to an existing group shows 'Hermes shared this' and one-tap undo; an agent can never create a public link |
| **T53** | P2 | MCP server (rmcp, stateless Streamable HTTP): tools from the registry, annotations, resources as projections, instructions from the shared guide | T02, T20, T43, T46, T49, T50, T51, T52, T80 | T54, T56, T57, T58, T84 | Claude Code connects with one passkey tap + Allow; `tools/list` schemas byte-equal the T20 snapshots; auth failure is HTTP 401, never a tool result |
| **T80** | P2 | Render sidecar + `part_check`: chrome-headless-shell container on an internal network, read-only renderer caps, coalesced queue (concurrency 1), screenshot + console + failed loads + blank flag | T15, T16, T20, T62 | T53, M2, T81 | `part_check` on a page that throws returns the error and a screenshot; a blank page returns `blank: true`; an egress canary from the sidecar fails; `/_rt` writes from the renderer are not persisted; sidecar absent -> `renderer_unavailable` |
| **T54** | P2 | Jig design system: `jig.css` shared blob + `@partifactory/jig`, presets, `design_system` op; Jig source is itself a part in the system space | T53, T17 | M2 | The op returns rules + starter; editing the Jig part changes the op output; Jig contrast test passes |
| **T55** | P2 | Workspace source editor: CodeMirror 6 (shared blob), server drafts, draft preview cap, 3-way merge + side-by-side resolve | T31, T46, T51 | M2 | e2e: an agent edit lands mid-draft; non-overlapping changes merge silently, overlapping ones show resolve |
| **T56** | P2 | Discovery: Markdown `GET /`, `/llms.txt`, `/llms-full.txt`, `/skill.md` + AGENTS.md snippet, `part skill install`, `/.well-known/mcp/server.json` (served, not published), 401 bodies | T20, T21, T43, T53 | T57 | A scripted agent given only the workspace URL creates and edits a part; docs regenerate in CI with no diff |
| **T57** | P2 | Cross-surface conformance: one scenario through MCP, an OpenAPI-generated client, the CLI and the commands in skill.md | T22, T45, T53, T56 | M2 | Normalised results, errors and audit rows are identical across the four drivers and three auth kinds (OAuth, device, PAT) |
| **T79** | P2 | Live reload for open viewers (`pf:version` from the wrapper or the prelude's EventSource; silent reload or toast) | T51, T31, T62 | M2 | With a part open on a phone, an agent edit appears within 2 s; a page with unsaved personal state shows the toast instead |
| **T58** | P2 | Hermes `partifactory` skill (the example deployment's chat agent): per-person pairing (device code), posts the part's own host link, runs `part_check` before posting, relays comments with a fix-it path, tells people about pending approvals in its own chat | T53, T51, T52, T44 | M2, T65 | Scripted Telegram flow: pair -> 'make a packing list and share it with the family' -> link -> comment -> fix -> live reload |
| **T59** | P2 | Blob GC: mark from every version, build, draft, data snapshot and asset; 7-day grace | T08, T34 | M2 | An unreferenced blob is deleted after the grace period; a blob referenced by any version is never collected |
| **T70** | P2 | Rename flow: admin-confirmed, 301 forever (query stripped), old name reserved | T09, T27, T28 | M2 | Old host 301s to the new host with no query; the old name cannot be reused |
| **M2** | P2 | Milestone: P2 done-when | T44, T47, T48, T54, T55, T57, T58, T59, T70, T79, T80 | — | A household member pairs with Hermes, asks for a packing list shared with the family, opens the link on a phone, comments, and watches the fix land; Claude Code connects with one passkey tap and edits a week-old part in one call; Hermes attaches a `part_check` screenshot before posting |
| **T39** | P2b | Authentik provisioner permission gate against the real IdP; record outcome tier (A full / B pre-created groups / C admin-approved) | T92, T03 | T40 | Gate report in site config lists each forbidden operation and its observed HTTP status |
| **T94** | P2b | Native invites + local passkey accounts (default, no provisioner): `/i/<id>` tokens (records opened, counts uses for 'up to N'), passkey registration and sign-in via `webauthn-rs`, grants keyed on the invite, recovery links that replace passkeys, local `pf-*` groups, linking a local account to an IdP identity, revoke, outstanding cap | T26, T28, T31 | T40, T42, T63 | With no provisioner configured, an invitee creates a passkey (virtual authenticator) and lands on the part already granted; status moves sent->opened->enrolled; the N+1th use is refused; a recovery link keeps the same account and grants and revokes the old passkey |
| **T40** | P2b | Authentik invites: `partifactory-invite` flow behind the same `/i/<id>` tokens, group membership by API after enrolment (per gate tier) | T39, T94 | T41 | With the Authentik provisioner on, an invitee enrols with a virtual authenticator through the IdP and lands on the part already granted; the Authentik invitation is deleted at the Nth enrolment |
| **T41** | P2b | Authentik extras: add by email, IdP recovery ('new passkey link') for IdP people, IdP-side `pf-*` group create/manage (per gate tier) | T40 | M2b | A recovery link keeps the same `sub` and grants; tier-B/C paths queue an approval instead of calling the IdP |
| **T42** | P2b | Cleanup lists: never-enrolled invitees, 180-day inactive users, one-click remove | T94 | M2b | Fixture users appear in the right list and removal revokes their grants with audit rows |
| **M2b** | P2b | Milestone: P2b done-when | T41, T42 | — | With no provisioner, Chris mints an invite link, a new person creates a passkey and lands on the part, and removing them locks them out at once; with the Authentik provisioner, an invited email enrols and removing them in the IdP locks them out within a minute |
| **T61** | P3 | Tailwind + shadcn: pick on evidence between prebuilt utility CSS, Tailwind v4 browser build (mirrored) and build-time class extraction; shadcn in the library set | T60 | M3 | A decision note with corpus results for each option; corpus pages using Tailwind utilities and shadcn `Button`/`Card` match the claude.ai reference screenshots |
| **T73** | P3 | `claude.use` (`db`, `user`, `comments`, `artifact` incl. self-publish CAS against live == head), `window.storage` shared scope, data leases | T62, T46, T50, T27 | M3 | Self-publish with a stale version is refused with the newer version; a poll part works for every viewer when 'viewers can update data' is on |
| **T83** | P3 | Assets: viewer uploads (`comment` role and up, 25 MiB), `/_a/<id>`, quota, listed and deletable by agents | T62, T26 | M3 | A viewer uploads a photo from a part and an agent lists it via `part_data {collection: "_assets"}` |
| **T84** | P3 | Agent inbox + steward: `part_inbox` op, routing of `to_agent` comments, states open/claimed/answered/resolved, stalled after 15 min | T50, T51, T53 | M3 | A comment for the steward appears in its inbox; claiming shows 'Hermes is on it'; no version after 15 min shows 'stalled' |
| **M3** | P3 | Milestone: P3 done-when | T61, T73, T83, T84 | T71 | A page copied from a claude.ai artifact runs unchanged, including state persistence and self-editing; a viewer upload reaches the agent |
| **T63** | P4 | Family onboarding: invite into `family`, first-run tour, personal space | T10, T94 | T65, M4 | A new family member invited to the space sees all family parts and none of Chris's private parts |
| **T64** | P4 | Embedding: `[embed] frame_ancestors` from config, embedding the part-host wrapper or `/c/` with a server-minted src, `pf-embed.js` regrant handler; the Nest as the first embedder with its service identity | T27, T44 | M4 | A Nest page embeds a family part on iPhone Safari and Android Chrome (manual pass recorded); an origin not in the list cannot frame it; the Nest's writes are labelled as the service |
| **T65** | P4 | Hermes `nest-family` profile | T58, T63 | M4 | The profile can create parts in `family` only; a write elsewhere is refused with `forbidden {space}` |
| **T66** | P4 | Slides kind + Mermaid/KaTeX (block = slide) | T12, T48 | M4 | Golden slide deck renders; block ops address slides |
| **T67** | P4 | Analytics overview: views per part per day, per-public-link open counts, 90-day rollups, admin 'what's used' page | T16, T31 | M4 | No raw IP/UA stored (schema test); the overview matches fixture counts |
| **T81** | P4 | Screenshot thumbnails for the private gallery + opt-in screenshot og:image for public links (sidecar), fallback to agent thumbnail or SVG thumb | T80, T75, T31 | M4 | Gallery tiles show screenshots when the sidecar is on and SVG thumbs when it's off; a public link without the opt-in never gets a screenshot card |
| **T85** | P4 | Phone editing: tap-to-edit text (saved as anchor edits) and long-press point-and-comment | T31, T62, T46, T50 | M4 | On a phone, a typo fixed by tapping it becomes one `part_edit`; a long-press comment carries `{version, selector|block, quote}` |
| **M4** | P4 | Milestone: P4 done-when | T63, T64, T65, T66, T67, T81, T85 | — | A family member opens a Nest-embedded part, fixes a typo by tapping it, long-presses to ask Hermes, and Hermes tells them in its own chat when the fix lands |
| **T71** | P5 | WASM component backends under the same grant model | M3 | — | An ADR names a real part that needs a backend; a WASM component serves it with the §7 suite green |
| **T87** | P5 | Optional S3-compatible upload of scheduled exports | T34 | — | Behind config; an export lands in a test bucket and `import --from s3://...` restores it |
| **T88** | P5 | Semantic search (embeddings) added alongside FTS5 | T76 | — | Hybrid ranking beats FTS5 alone on a labelled query set; FTS5-only behaviour is unchanged when disabled |
| **T90** | P5 | Built-in ACME DNS-01 TLS (wildcard cert through a DNS provider plugin) for hosts with no proxy | T32, T89 | — | The portability lane passes with no proxy in front; the cert renews before expiry in a test with a short-lived ACME staging cert |

**Graph checks** (output of the checker):
- **Cycles: none. Orphans** (tasks nothing consumes that aren't milestones or P5 leaves): **none.**
- **Critical path to M2** (14 steps): T01 → T05 → T06 → T08 → T12 → T13 → T16 → T27 → T29 → T52 → T53 → T56 → T57 → M2. The build worker (T13), the content listener (T16), the hand-off (T27) and the wrapper (T29) are the bottleneck, so start them first.
- **Critical path to M1** (12 steps): … T16 → T27 → T29 → T31 → T77 → M1.
- **Parallel lanes once the schema lands:** identity (T24–T28), ops (T32, T34–T37, T89), adapters (T91, T92, after their generic defaults), the API surfaces (T20 → T21 → T22), the compiler (T13 → T14 → T60) and the sandbox spike (T72) mostly don't touch each other.
- **Every feature has a task.** Small items folded into a larger task: break-glass and `admin bootstrap` (T28), the Cloudflare fallback Worker and health probe (T91), the maintenance page for other proxies (T89), the optional ops-alert webhook (T36), CORS for the chrome API (T29), view collection (T16, overview in T67), the "being made…" placeholder (T51), arm64 images, release assets and example CI workflows (T37), connector-token rotation (the moving checklist, T35 drill), runtime log levels (T36), optional OTLP (T18).

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
- Only Chris's session can create a public link. No agent scope, token or op can (tested on every surface). A revoked public link → 404 within seconds. The kill switch revokes all.

**Identity and admin**
- Only Chris's pinned subject reaches admin. An IdP-admin user still gets 403. An admin action without a fresh (10 min) partifactory passkey assertion → `step_up_required`. Household sharing never asks.
- *(adapter)* The Authentik provisioner gets 403 from the IdP on a non-`pf-*` group, and the core refuses before calling. Gate results are recorded: no non-`pf-*` membership, no `is_superuser` groups, no flow or stage edits, no privileged parent groups.
- With the IdP down, `/readyz` stays 200, existing sessions work, and no epoch is bumped. A refresh that returns `invalid_grant` ends that session.
- Local accounts: passkey-only (no password path exists), a recovery link revokes the old passkeys, and an invite token beyond its use count is refused.

**Agents**
- Agent tokens: audience-bound to the workspace origin (rejected on part hosts and on `/oauth/*`), scope- and space-limited, no admin scopes obtainable, refresh reuse → the whole token family is revoked.
- An agent without `share:household` can't grant. With it, it can grant only to existing people and groups, only within its spaces, and every such grant is labelled and listed in the Requests inbox and activity feed.
- Unconsented OAuth clients can do nothing. Registration is rate-limited. A CIMD URL resolving to a private address is refused. The consent sheet shows the redirect host.
- Device codes: single use, 10-minute expiry, `slow_down` enforced, manual entry limited to 5 per minute; the stale and different-network banners appear when they should.
- A `pf_pat_` token is redacted in logs, scope- and space-limited like OAuth tokens, and revoked by the same cascade.
- Authentication failures are HTTP 401 with `WWW-Authenticate` on `/mcp` and `/api`, never a tool result.
- Discovery documents are reachable unauthenticated and contain no site secrets or user data.

**Render sidecar**
- The renderer has no route off the internal Docker network (an egress canary to an external host fails). Renderer caps can't persist `/_rt` writes. A public link without the screenshot opt-in never gets a screenshot card.

**Operations**
- The origin is reachable only through the configured ingress (a direct connection to the listeners from outside fails). `:9090` is never routed. A client-IP header from an address outside `trusted_proxies` is ignored.
- No tokens, caps, cookies or part content in logs (tested with canary values).
- Rate limits trip at the configured thresholds, and quotas stop a looping agent. At 95 % disk, writes get `storage_full` and reads continue.
- A parts-only export contains no grants, tokens, sessions or people (schema test).
- Real-device pass (iPhone Safari, Android Chrome, the Telegram in-app browser) for: standalone open, part-host open (first open and reload), embedding, invite acceptance, sign-in.

## 8. Decisions

**2026-10-22**
1. **Hostnames**: `parts.<domain>` + `<name>-part.<domain>` via a `*.<domain>` wildcard, with semantic names. The domain and the part suffix are config; `dilger.dev` is the example deployment (round 2b).
2. **Stack**: Rust. Bend 2 considered and set aside.
3. **Host**: any OCI host with compose; TrueNAS in the example deployment. The app knows nothing about TrueNAS/ZFS.
4. **Code reuse**: fresh, with shartifacts as the reference spec, credited.
5. **Docker**: for deploying partifactory only. Parts need no builds.
6. **Tunnel**: the example deployment uses its own Cloudflare tunnel `partifactory` (created 2026-10-22, routes configured, no DNS records yet). Since round 2b the tunnel is one ingress adapter; the default is any reverse proxy (decision 22).
7. **Identity**: OIDC, swappable; Authentik in the example deployment. In-app grants and invites. Admin pinned to Chris with native passkey step-up. Round 2b makes generic OIDC the baseline and Authentik an optional extra (decision 23).
8. **Agents**: OAuth 2.1 per the MCP spec, with partifactory as a federating authorization server. Per-agent consented, scoped, audience-bound tokens.
9. **Source of truth**: private Gitea repo + passive public GitHub mirror. Site config in a separate private repo. Round 2b: secrets come from env vars or files, from any CI or none; Gitea Actions is one example workflow (decision 22).

**2026-10-23 (round 2)**

10. **No cookies on part hosts; every part is sandboxed.** CSP `sandbox` without `allow-same-origin` gives each part document an opaque origin, and caps live in the path. No separate registrable domain: the sandbox removes the reason to buy one. Per-part hostnames stay (§2).
11. **Any AI, any surface.** One operation registry in Rust, rendered as MCP, an HTTPS JSON API with OpenAPI 3.1, the `part` CLI and a skill file. Discovery via `/llms.txt`, `/openapi.json`, `.well-known` metadata, Markdown on `GET /` and a served (not published) `server.json`. No A2A card.
12. **One link to post** (round 2b: the part's own host, decision 25). No agent-minted preview links. Public links are Chris-only, made after publishing, with no expiry by default, open counts and revocation.
13. **Agent sharing: inward yes, outward no.** `share:household` lets agents share with people who already have accounts; public links, invites and groups are requests Chris approves.
14. **One connection per (agent, person)**, labelled "Hermes for <person>", plus service identities. Device grant is the default for headless agents; personal agent tokens for clients without OAuth.
15. **Spaces are first-class**: a personal space per person + `family`. Space grants are inherited live. "Move to Family" is the household share.
16. **Previews**: tier (a) branded cards in the binary, always, decided by audience; tier (b) an optional render sidecar whose headline is `part_check`, a P2 feature.
17. **Backups = the `/data` folder** plus `partifactory export`/`import`. No git archive, no Litestream, no streaming sink on the write path. Per-part data snapshots are on by default since round 2b (decision 26).
18. **Search** is FTS5 over readable text, with bm25 + recency. **Every version is kept**, with a 30-day bin for deleted parts.
19. **Stockroom** is the workspace theme (approved). **Jig** is the separate, optional design system for parts.
20. **Builds run in a child process** of the same binary, with a deadline and a memory cap.
21. **Phase order**: sandbox + TSX in P0, agents (P2) before invites (P2b), the nginx/HAProxy comparison after P1.

**2026-10-23 (round 2b)**

22. **No hard dependency on the current stack.** Cloudflare, Authentik, Gitea, TrueNAS, Telegram/Hermes and the Nest are each an adapter behind config with a vendor-free default: any reverse proxy for ingress (edge caching optional), `provision` provider plugins with a manual mode, an OCI image + compose file + env contract for deploys, a generic webhook for events and alerts, `frame-ancestors` from config. §5f has the table.
23. **Generic OIDC is the identity baseline**, with a claims re-read directory. Authentik's directory poll and provisioner are optional extras. **Native invites with local passkey accounts are in scope for v1 (P2b)**, so inviting people never needs a provisioner, and an IdP-less deployment works.
24. **A portability lane gates every release**: the full e2e suite with Dex (and with no IdP), Caddy and local exports, and no Cloudflare, Authentik or Gitea.
25. **The link to post is the part's own host**, `https://<name>-part.<domain>/`. Its root is a trusted, unsandboxed wrapper with the Stockroom chrome, framing `/c/<cap>/` on the same host. The cap arrives in a `#pf=` fragment from `/grant`. Chrome actions use a credentialed, same-site chrome API with CORS for exactly the sandboxed part-host origins. `/p/<name>` only redirects. Public links move to `<name>-part.<domain>/l/<token>`. First open costs one redirect hop.
26. **Per-part data snapshots are on by default** (daily, keep 7), switchable off per part, with the 90-day idle stop and keep-the-last-one rule.
27. **No notifications for people.** Approvals are in-band (Requests inbox, the part's chrome bar, the asking agent's own channel). The only push is an optional ops-alert webhook for the admin, off by default.

## 9. Non-goals (v1)

Multi-tenant SaaS. Multi-node. Per-part backends. Offline mode. Listed or indexable public parts. Replacing the Nest's knowledge base.
Public sign-up. A dedicated Telegram bot (Hermes is the bot). Notifications for people: web push, email, mention or reply alerts, per-person notification settings. Agents making public links or inviting people. An A2A agent card, or publishing to the public MCP registry.
Device permissions (camera, mic, geolocation) and service workers in sandboxed parts, except through the admin-gated `unsandboxed` flag. Device-local part storage.
A git or Gitea dependency for part content or backups. A hard dependency on any one vendor for ingress, DNS, identity, CI, hosting or notifications. Backing up the IdP (its operator's concern). Sending email. Version pruning. Live human co-typing (CRDTs).
## 10. Change logs

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

### Round 2 (2026-10-23)

Inputs: Chris's decisions in chat (binding, and later messages overriding earlier ones on backups), an Opus review (O1–O17), a Fable review (parts A–H), a grounding check against primary sources (G), a self-containment, data-model and task-graph validation (V), and a design proposal (D).

**What changed.**
- **Sandbox, no part-host cookies**: every part document gets CSP `sandbox` without `allow-same-origin`. The `__Host-pf_cap` cookie, the one-time `?g=` codes, the cookie-tossing prelude and the separate-domain open decision are gone. Caps are in the path, deterministic per 12 h window, with a boot-epoch floor.
  `/_rt` is one text/plain POST (no preflights). The `Sec-Fetch-Site: same-origin` requirement is dropped: it would have rejected every legitimate call from an opaque origin. Three inconsistent CSPs became one canonical block.
- **Surfaces**: one operation registry renders MCP, the HTTPS API (OpenAPI 3.1), the CLI and `/skill.md`. Discovery, one error shape with status and exit maps, one event log with three deliveries, a cross-surface conformance suite. Tools consolidated to ten around one write op.
- **Links**: agent preview links (`preview:mint`, 24 h, version-pinned) are removed. One link to post, `/p/<name>`. Public links are Chris-only, no default expiry, with open counts.
- **Agents**: one connection per (agent, person); service identities; inward sharing via `share:household`; outward only by request. Token audience fixed to `https://parts.dilger.dev/`. MCP spec 2026-07-28 named, with its AS metadata, `iss`, CIMD SSRF guards and redirect-host display.
- **Spaces** defined: personal + `family`, live inheritance. A role × runtime matrix; viewers may write shared data by default. Step-up is one 10-minute window for admin actions only.
- **Previews**: in-binary branded cards decided by audience; an optional render sidecar; `part_check` as a P2 headline.
- **Core model**: build records keyed by `hash(manifest, toolchain)`, head vs live, import maps frozen per build, builds in a child process, mirror-on-build (source untouched), `request_id` on every write, ordered batch edits, a structured error catalogue, fork, scratch parts, assets, `render: "text"`, full content for small parts.
- **Backups**: the `/data` folder plus `export`/`import`, a three-layer table, a host contract and a moving checklist. Litestream (whose v0.5 dropped age encryption), R2 as a built-in, the blob-before-manifest ordering and the async sink outbox are all gone.
- **Search, history, bin** specified. **Stockroom** UI and **Jig** added as §4a. **Data model** (§5e) and **task graph** (§6a) added. Phases re-ordered, with "Done when" for every phase.
- **Grounding fixes**: Authentik invitations can't set groups and have no use count; Cloudflare Free has one rate-limit rule; Cloudflare caches by extension; oxc/rolldown Rust APIs aren't semver-stable; the wildcard CNAME to the tunnel needs verifying.

**Considered and rejected during round 2.**
- **A git parts archive** (each version a commit in a private Gitea repo, pushed from an outbox). Chris: no git or Gitea dependency for part content or backups. Versioning stays inside partifactory, and backup is the `/data` folder.
- **An async backup-sink outbox with storage-agnostic sinks** (local dir, S3 via `object_store`) on the core path. Replaced by `export` + host backups. An S3 upload of exports is optional and P5.
- **15-minute `VACUUM INTO` snapshots shipped off-box.** Replaced by an hourly local consistent copy for host tools and full exports.

**Merge table.**

| Source | Item | Verdict | Where it landed |
| --- | --- | --- | --- |
| Chris 1 | No cookies on part hosts; sandbox; no second domain | **Adopted** | §2 Serving, §3, §8.10, §7 |
| Chris 2 | Any AI, any surface; registry; discovery; PATs; one error shape; one event log; no MCP-only features; conformance | **Adopted** | §4, §4b, §3b, §8.11 |
| Chris 3 | Tier (a) cards always, by audience; tier (b) sidecar; `part_check` headline | **Adopted** | §4c, §6 P2, §8.16 |
| Chris 4 | No agent-minted preview links; Chris-only public links, no default expiry, open counts | **Adopted** (round 2b: the link to post became the part's own host, and public links moved to `<name>-part/l/<token>`) | §2 link, §3a ladder, §5e `public_links` |
| Chris 4 | Telegram in-app browser + passkeys | **Open item**, tested in P1; person-bound fallback, never anonymous | §2, §6 P1, T38 |
| Chris 4b | Inward yes (`share:household`), outward no; divergence from shartifacts; connecting = identity | **Adopted** | §3b, §1, §8.13 |
| Chris 5 → later messages | Storage-agnostic sinks + outbox, then a git archive, then **`/data` + export/import** | **Final: `/data` + export/import**; git archive and core-path sinks rejected | §5c, §10 rejected list |
| Chris 5 | Per-part data opt-in, expiry, keep the last one | **Adopted** as "keep data snapshots", for undo not DR (round 2b: on by default) | §5c, §5e |
| Chris 5 | Authentik's Postgres out of scope | **Adopted** | §5c, §9 |
| Chris 5 / F E1.5 | Image + CLI on the GitHub mirror for laptop drills | **Adopted** | §5b, T37 |
| Chris (later) | Host contract, backup layers, `doctor` reports layers, moving checklist | **Adopted** | §5b, §5c, T68 |
| Chris 6 | FTS5 search, filters, bm25 + recency, embeddings later | **Adopted** | §3e, T76, T88 |
| Chris 7 | Versions labelled, diff any two, preview old, restore = new, keep all, 30-day bin | **Adopted** | §3e, §4a.6, T11, T31 |
| Chris 8 / D | Stockroom theme, Jig design system, mockup | **Adopted**, adjusted for `/p/<name>` (round 2b: the part view moved into the part host's wrapper), no previews, public links without expiry | §4a |
| Sandbox mechanics | Cap in path, windowed, boot floor, no `?g=`, `pf:regrant`, `/_rt` simple request, ACAO null, no CORP, canonical CSP, shim, escape hatch, per-part hosts kept, P0 spike | **Adopted** | §2, §5 spike list, T15, T16, T62, T72 |
| O1 | Opaque-origin sandbox | **Adopted**, with ACAO `null` + `Vary` (not `*`) and no UA-mismatch re-grant | §2 |
| O2 / F B6 | Spaces first-class | **Adopted**, live inheritance (V) over copy-at-create (F) | §2, §3a |
| O3 | One token per (agent, person); Nest service principal | **Adopted** | §3b |
| O4 | Canonical smart link `/s/<token>` with fallbacks and agent previews | **Modified**: canonical `/p/<name>` + separate Chris-only public link; `/s/` dropped. **Round 2b:** canonical is now the part's own host with a trusted wrapper; `/p/<name>` only redirects | §2 link |
| O5 | Agent inbox + live reload | **Partly**: live reload P2, inbox and steward P3 | §3c, T79, T84 |
| O6 | Tap-to-edit text, point-and-comment | **Adopted for P4** | §3c, T85 |
| O7 / F E1 | Saves don't wait on backups; restore to newest complete | **Adopted in spirit**: no backup target on the write path at all | §3d, §5c |
| O8 | Role × `/_rt` matrix, viewer-writable data, no step-up for household | **Adopted**; step-up window fixed at **10 min** (O suggested 15) | §3a |
| O9 / F part F | Phase re-order | **Adopted**: sandbox + TSX in P0, agents before invites (P2 then P2b), bench comparison after P1, Done-when for every phase | §6 |
| O10 | Blob upload with have/need, idempotent edits, latest vs live | **Adopted** | §3d |
| O11 / F C1 / F H | Consolidate tools; part-level leases | **Adopted**: ten ops, one write op; part scope by default with optional file/block | §4, §3d |
| O12 | Notifications for people | **Adopted, minimal, P4**. **Cut in round 2b**: approvals are in-band; only an optional ops-alert webhook remains | §4 |
| O13 | Kinds table; fixed Tailwind build | **Kinds adopted**; Tailwind is a P3 spike choosing among three options on evidence | §2 Kinds, §6 P3, T61 |
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
| F B5 | Token audience = `https://parts.dilger.dev/` | **Adopted** | §3b |
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

Inputs: four decisions from Chris, the owner, made after round 2. They're binding, and where they conflict with round 2 they win.

**What changed.**
- **Portability: no strong dependency on the current stack.** An audit found couplings to Cloudflare (tunnel, DNS, cache rules, the rate-limit rule, the fallback Worker, health checks, `cf-ray` ids, country/ASN on device approval), Authentik (the only `Authn`/`Directory` adapter, the admin `sub` pin, invites, recovery, groups, cloned flows),
  Gitea (secrets only in Gitea Actions, the deploy pipeline, the registry, Renovate), TrueNAS (the default host), Telegram/Hermes (the default alert notifier, a notify channel, a `[hermes]` config section) and the Nest (hardcoded in `frame-ancestors`), plus `dilger.dev` written into normative text.
  Each one is now an adapter behind config with a generic default: the ingress contract with Caddy/nginx/Traefik examples (§3), `provision` provider plugins with a manual mode (§5c), generic OIDC with a claims re-read directory and native invites on local passkey accounts (§3a), an OCI image + compose file + env contract with `_FILE` secrets (§5b),
  generic webhooks (§4), `[embed] frame_ancestors` (§2), and `<domain>` everywhere with dilger.dev as the example deployment. New §5f has the table, and a **portability lane** (T93) runs the full e2e suite with Dex, Caddy and local exports, and no Cloudflare, Authentik or Gitea.
  Schema: `users` gains `auth_source` and `(idp_issuer, idp_sub)`, `groups` gains `source`, `admin_credentials` becomes `passkeys` with a `purpose`, sessions keep an encrypted IdP refresh token, `audit.cf_ray` becomes `request_id`.
  Tasks: T89 (ingress contract), T90 (built-in ACME, P5), T91 (Cloudflare adapter), T92 (Authentik adapter), T93 (portability lane) and T94 (native invites + local accounts) added; T24, T25, T32, T36, T37, T39–T42 re-scoped. Decisions 22–24.
- **The link to post is the part's own host.** `https://<name>-part.<domain>/` serves a trusted, unsandboxed wrapper with the Stockroom chrome, framing the sandboxed content at `/c/<cap>/` on the same host. With no cap, the wrapper sends the browser to `/grant`, which returns with the cap in a `#pf=` fragment.
  The wrapper clears the fragment, keeps the cap in `sessionStorage` and sets the frame. Chrome actions use a credentialed same-site chrome API on the workspace, with CORS for exactly the sandboxed part-host origins. The part host still never sets or reads a cookie.
  `/p/<name>` only redirects, public links moved to `<name>-part.<domain>/l/<token>`, and the first open in a tab costs one redirect hop. Updated: §2 link section and capability hand-off, §3 topology, §4a.6 part view, §4a.7 budgets, §4c cards, §5 spike list, §7, T16, T27, T29, T31, decision 25.
  *One correction to the brief:* the "no cap → `/grant`" step has to be a client-side `location.replace`, not a server 302, because the server never sees the fragment and can't tell a cap-carrying load from a bare one.
  *Added beyond the brief:* the chrome API's CORS never admits an `unsandboxed` part's origin, because that part's code is same-origin with its wrapper and would otherwise ride the viewer's session. Its wrapper falls back to opening Share and Comments on the workspace. The wrapper's exposure to that part (its own cap only) is accepted.
- **Per-part data snapshots are on by default** (daily, keep 7), switchable off per part, with the 90-day idle stop and keep-the-last-one rule kept. Fable's round-2 suggestion is adopted. §5c, `data_snapshot_policies.mode DEFAULT 'daily'` (the prompt column is gone), T77, decision 26.
- **Notifications for people are cut.** Share and access requests are approve/deny chips in the Requests inbox and on the part's chrome bar. The asking agent tells the person in its own channel. Device approvals are started by the person. Agent inward shares are inbox and activity-feed entries.
  The only push left is an **optional ops-alert webhook** for the admin (disk ≥ 90 %, no export in 48 h, build worker crash-looping, IdP unreachable), off by default and mirrored on the Health page. Removed: web push, comment and mention notifications, per-person settings, the `notify_prefs` table and T86. The event log and its webhooks for agents stay. Decision 27.

**Kept coupled, on purpose** (§5f): one registrable domain for workspace and part hosts (the cookie model needs same-site), WebAuthn passkeys, OCI + compose as the deployment unit, the default mirror CDNs (a Claude-compatibility target, and config), and the Telegram in-app browser in the real-device pass.
