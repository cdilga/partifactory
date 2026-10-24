# Spike: opaque-origin sandbox and wrapper flow on every engine (pf-6qma)

**Decision: GO for the serving model and the wrapper flow, with four required changes.** The model holds on Chromium, WebKit, Firefox and Mobile Safari (iOS 27 Simulator): the opaque origin, the `#pf=` hand-off through `/grant`, `Origin: null` CORS, the prelude's storage shims, blob workers, SSE and WebSocket through Caddy, nginx and Traefik, and a passkey with `rp.id` = the apex used from the part host. The four required changes:

1. **Spell out the part host in the part CSP.** On WebKit (Safari, iOS), `'self'` in `connect-src` does not match the part's own host for `fetch` from a sandboxed document. `'self'` still works for scripts, images, forms, frames and `wss:`. Write `'self' https://<name>-part.<domain>` in every source list, and add `wss://<name>-part.<domain>` to `connect-src`, because an `https://` host-source does not cover `wss:` on any engine.
2. **The workspace must treat `Origin: null` as hostile on every endpoint, not only for CORS.** WebKit and Mobile Safari send the workspace's `SameSite=Lax` cookies, `__Host-` session included, on credentialed requests from a sandboxed part to the workspace or a sibling service. Chromium and Firefox don't. The canonical CSP blocks those requests (`connect-src` lists neither host), so this is the second wall, and it must be real:
   - reject any request whose `Origin` is `null`, or whose `Sec-Fetch-Site` is `cross-site`, before it acts (both are present on these requests);
   - keep GETs free of side effects;
   - never list the workspace in a part's CSP.
3. **The wrapper's `<iframe allow>` must use `*`.** The default allowlist (`src`) never matches an opaque origin. Write `allow="clipboard-write *; fullscreen *; web-share *"`. Otherwise clipboard, fullscreen and `navigator.share` are blocked by Permissions Policy on every engine.
4. **The workspace and the deployment must never set a `SameSite=None` or `Domain=<apex>` cookie.** A `SameSite=None` apex cookie reaches the part host and every sibling, from the part's own requests, on every engine. Session cookies stay host-only `__Host-…; SameSite=Lax`, as planned.

Finished 2026-10-25, before the part host work (pf-e3wx) starts. Rig: [`tests/sandbox-spike/`](../../tests/sandbox-spike/) (`run.sh`, `ios-sim.sh`, `render.mjs`; CI job `spike` → `ci/spike.sh`).

## Lanes and versions

| Lane | Engine | Version | OS / runtime |
| --- | --- | --- | --- |
| chromium-desktop | Chromium (Playwright 1.62.1, headless) | 151.0.7922.34 | macOS 27.0 arm64 |
| chromium-mobile | Chromium, Pixel 7 emulation (touch, mobile viewport), CDP virtual authenticator | 151.0.7922.34 | macOS 27.0 arm64 |
| webkit-desktop | Playwright WebKit | 26.5 | macOS 27.0 arm64 |
| webkit-mobile | Playwright WebKit, iPhone 15 profile | 26.5 | macOS 27.0 arm64 |
| firefox-desktop | Playwright Firefox | 153.0 | macOS 27.0 arm64 |
| ios-sim | Mobile Safari, `Version/27.0 Mobile/15E148` | iOS 27.0 (24A434) | Simulator, iPhone 18 Pro (`pf-spike-iphone`), macOS 27.0.1 |

The proxies for item 12 were `caddy:2.10.0-alpine`, `nginx:1.27.5-alpine` and `traefik:v3.4.1`, all in Docker.

## How the rig works

- One Node server (`server.mjs`) serves every host of the test domain:
  - `parts.` is the workspace: it sets the cookies and serves `/grant`, `/api/chrome/v1/<part>/echo` and `/webauthn`;
  - `a-part.` and `b-part.` are part hosts: `/` is the stub wrapper, and `/c/<cap>/…` is part content with the canonical CSP;
  - `blobs-part.` serves the prelude and wrapper JS with `ACAO: *`;
  - `canary.` records the cookies it receives;
  - `spike.` serves the runner.
- Part responses answer `Origin: null` with `ACAO: null` and `Vary: Origin`, never ACAC or CORP.
- The workspace sets three cookies when you sign in:
  - `__Host-pf_sess` (Lax, host-only);
  - `pf_dom_lax` (`Domain=<apex>`, Lax);
  - `pf_dom_none` (`Domain=<apex>`, `SameSite=None`). This one is a probe for change 4.
- Domains:
  - Playwright uses `*.pf.test` through a CONNECT proxy in the rig, which maps to 127.0.0.1. It's hermetic, with no `/etc/hosts` edits.
  - The simulator uses `*.localtest.me`, a public wildcard to 127.0.0.1, because the Mac's `/etc/hosts` needs root.
  - The rig CA is trusted properly: in the simulator with `simctl keychain add-root-cert`, and on Chromium with `--ignore-certificate-errors-spki-list`, so WebAuthn sees a valid cert. WebKit and Firefox in Playwright use `ignoreHTTPSErrors`.
- Each item is a page that reports itself. The runner on `spike.` frames it (the runner is listed in `frame-ancestors` as an embedder), or navigates to it top-level for login, 2top, 10 and 13. Results are POSTed as `event="spike.result"` lines to `target/sandbox-spike/results.jsonl`, and `ios-sim/results.jsonl` for the simulator. `render.mjs` builds the tables below and exits 1 if any lane × item is missing.
- Playwright clicks the activation-gated buttons (item 6) and watches downloads. The simulator lane has no input, so those checks are `not_assertable` there.
- Chromium lanes grant `clipboard-write` so that check measures Permissions Policy delegation, not headless focus.

## Results per item

Summary (worst result per item; full per-check table at the end). "Controls" are checks that show what *not* to do; they're recorded as `info`, or as `fail` for the nginx-naive rows from the Playwright run, explained below.

| Item | chromium-desktop | chromium-mobile | webkit-desktop | webkit-mobile | firefox-desktop | ios-sim |
| --- | --- | --- | --- | --- | --- | --- |
| 1 `'self'` matches the part host | pass | pass | **FAIL** (`connect-src` fetch) | **FAIL** (same) | pass | **FAIL** (same) |
| 2 `Origin` / `Sec-Fetch-*` values | pass | pass | pass | pass | pass | pass |
| 3 fetch, module scripts, `import()` with `ACAO: null` | pass | pass | pass | pass | pass | pass |
| 4 `localStorage` redefined before page scripts | pass | pass | pass | pass | pass | pass |
| 5 blob workers, relative-URL wrapper | pass | pass | pass | pass | pass | pass |
| 6 popups escape, clipboard, share, fullscreen | pass | pass | pass | pass | pass | n/a (needs a tap) |
| 7 `document.cookie` / `cookieStore.set` fail | pass | pass | pass | pass | pass | pass |
| 8 no Lax cookies to a sibling or the workspace | pass | pass | **FAIL** | **FAIL** | pass | **FAIL** |
| 9 `sandbox` honoured from the header only | pass | pass | pass | pass | pass | pass |
| 10 wrapper flow (7 checks) | pass 7/7 | pass 7/7 | pass 7/7 | pass 7/7 | pass 7/7 | pass 7/7 |
| 11 Cloudflare wildcard CNAME | deferred | deferred | deferred | deferred | deferred | deferred |
| 12 WS + SSE held 75 s (direct, Caddy, nginx, Traefik) | pass | pass | pass | pass | pass | pass |
| 13 passkey, `rp.id` = apex, asserted from the wrapper origin | pass | pass | n/a | n/a | n/a | n/a (the sheet appears; needs a tap) |
| Known losses (§2) | pass | pass | pass | pass | pass | pass (downloads n/a) |

### Item 1: `'self'` (FAIL on WebKit, fixed by spelling out the host)

| Policy used | Chromium | WebKit / iOS | Firefox |
| --- | --- | --- | --- |
| `'self'`: `script-src`, `img-src`, `form-action` + `frame-src` | match | match | match |
| `'self'`: `connect-src` for `fetch` | match | **no match** ("Refused to connect … connect-src") | match |
| `'self'`: `connect-src` for `wss://` own host | match | match | match |
| `https://<host>` explicit: `connect-src` for `fetch` | match | match | match |
| `https://<host>` explicit: `connect-src` for `wss://` | **no match** | **no match** | **no match** |
| `https://<host> wss://<host>` explicit | match | match | match |
| `'self'` excludes a sibling part host | yes | yes | yes |

*Consequence (part host):* the canonical CSP becomes `<src> = 'self' https://<name>-part.<domain>` in `script-src`, `style-src`, `img-src`, `media-src`, `font-src`, `worker-src`, `frame-src` and `form-action`, plus `connect-src 'self' https://<name>-part.<domain> wss://<name>-part.<domain> <blob>`. That's the fallback PLAN §5 already names. The rig's canonical policy uses it, and every later item passed under it, including `importScripts` from a blob worker on WebKit. Keeping `'self'` costs nothing and documents the intent.

### Item 2: header values (pass everywhere, identical on every engine)

| Request | `Origin` | `Sec-Fetch-Site` | `Sec-Fetch-Mode` | `Sec-Fetch-Dest` | Preflight |
| --- | --- | --- | --- | --- | --- |
| `fetch("../x")` to own host | `null` | `cross-site` | `cors` | `empty` | – |
| `POST /_rt` shape (`text/plain`) | `null` | `cross-site` | `cors` | `empty` | **none** (CORS-simple confirmed) |
| `<img>` own host | none (Firefox: `null`) | `cross-site` | `no-cors` | `image` | – |
| classic `<script>` own host | none (Firefox: `null`) | `cross-site` | `no-cors` | `script` | – |
| module `import()` own host | `null` | `cross-site` | `cors` | `script` | – |
| part framed by its same-host wrapper | – | `same-origin` | `navigate` | `iframe` | – |
| part framed by an embedder on a sibling host | – | `same-site` | `navigate` | `iframe` | – |
| top-level navigation to `/c/…` (from a same-site page) | – | `same-site` | `navigate` | `document` | – |

*Consequence (part host, `/_rt`):* the re-grant rule "navigation = `Sec-Fetch-Dest` ∈ {`document`, `iframe`}" works on every engine; all six send Fetch Metadata. `/_rt` must not check `Sec-Fetch-Site`: it's `cross-site` even to the part's own host. Check `Sec-Fetch-Mode: cors` and `Origin: null` (or the own origin for `unsandboxed`). A classic `<script>`/`<img>` carries no `Origin` on Chromium/WebKit, so never require `Origin` on blob paths.

### Item 3: pass everywhere

`fetch("./x.json")`, `<script type="module">`, `import()` on the own host (with `ACAO: null`) and `import()` from the blob host (with `ACAO: *`) all work. A `credentials: "include"` read fails, as it should, since there's no ACAC.

### Item 4: pass everywhere

The prelude stand-in (`blobs/prelude.js`, first in `<head>`, from the blob host) redefines `localStorage`/`sessionStorage` with `Object.defineProperty(window, …)`, redefines `document.cookie` on the `document` instance, and sets `indexedDB`/`caches` to `undefined`. Later inline scripts see the redefinitions on every engine.
- A `Proxy`-backed storage makes `localStorage.foo = "x"` work too; the real prelude should use one.
- Natively, `localStorage`, `sessionStorage`, `caches` and `document.cookie` throw `SecurityError` in the opaque origin. Firefox leaves `caches` reachable but unusable.
- `window.indexedDB` is *reachable* on every engine, but `open()` throws. So the prelude must redefine it, not rely on it being absent.

### Item 5: pass everywhere

The prelude's wrapper works on all six lanes: fetch the relative URL, then start the worker from a `blob:` URL. Inside the blob worker, `fetch` and `importScripts` to the absolute own-host URL work (on WebKit, only with the explicit host from item 1).
- A direct `new Worker("./w.js")` fails on every engine (`SecurityError` on Chromium, `onerror` elsewhere), which confirms the wrapper is needed.
- A module worker from a `blob:` URL fails on Chromium (info) and works elsewhere. The prelude should start blob workers as classic workers.

### Item 6: pass with `allow="… *"`

- With `allow="clipboard-write *; fullscreen *; web-share *"` on the frame and a user click:
  - `clipboard.writeText` resolves on every Playwright lane;
  - fullscreen enters on all lanes except iPhone WebKit, which has no element fullscreen (a platform limit, not the sandbox's);
  - `navigator.share` resolves on WebKit and is absent on desktop Chromium/Firefox;
  - a `target=_blank` popup opens on the canary host with a real origin and working `localStorage`, so `allow-popups-to-escape-sandbox` works.
- With the default allowlist (`src`), clipboard and fullscreen are blocked by Permissions Policy on Chromium, and fullscreen and share on WebKit and Firefox. That's change 3.
- Playwright has no popup blocker, so "popup without activation" is info only. In the simulator, nothing here can be tapped: `not_assertable`, and on the real-device list.

### Item 7: pass everywhere

`document.cookie` writes throw `SecurityError`, or are a no-op (Firefox), and `cookieStore.set` rejects. No cookie the part tried to plant reaches the part host. The rig's `pf_dom_none` probe cookie does reach the part host, on every engine (change 4).

### Item 8: FAIL on WebKit and Mobile Safari (consequence: change 2)

With a relaxed test CSP that allows `connect-src` to the canary and the workspace, a credentialed `fetch`/POST from the sandboxed part:
- Chromium, Firefox: no Lax cookies; only the `SameSite=None` probe goes along.
- WebKit, webkit-mobile, **Mobile Safari (iOS 27)**: `pf_dom_lax` goes to the canary, and **`__Host-pf_sess`** + `pf_dom_lax` go to the workspace, with `Origin: null` and `Sec-Fetch-Site: cross-site`. WebKit computes SameSite from the frame's URL, not from its opaque origin.
- Under the **canonical** CSP (check `canonical_csp_blocks_*`), neither request leaves the browser on any engine (`securitypolicyviolation`, 0 hits at the server).

*Consequence:*
- `connect-src` (and `form-action`/`img-src`, which never list the workspace) is the first wall.
- The workspace's second wall: every handler rejects `Origin: null`, and any request with `Sec-Fetch-Site: cross-site`, before it does anything. That goes beyond the plan's "never answers CORS for null". The response is unreadable anyway (no ACAO), so what's at stake is side effects.
- The chrome API's CSRF rule (allowed `Origin` + `Sec-Fetch-Site: same-site|same-origin`) already rejects these requests, so keep it on every method.
- Add an e2e regression for "workspace refuses `Origin: null` with a valid session cookie" to the workspace/auth beads.
- The wrapper flow doesn't change.

### Item 9: pass everywhere

`sandbox` from the `Content-Security-Policy` header gives `self.origin === "null"`. From `Content-Security-Policy-Report-Only` or a `<meta>`, it's ignored and the page has a real origin. *Consequence:* the header is the only carrier. The part host must send it on every HTML and SVG document, including error pages served under `/c/`.

### Item 10: wrapper flow (all seven checks pass on all six lanes, simulator screenshots included)

| # | Check | Result (all lanes) |
| --- | --- | --- |
| 1 | No cap → `location.replace` to `parts./grant?part=a&path=…&mode=wrap` | pass. `/grant` saw the Lax session (top-level, `Sec-Fetch-Site: same-site`). Its `Referer` is `https://a-part.<domain>/`: origin only, no path, no fragment |
| 2 | `#pf=` in the 302 `Location` survives to the part host | pass |
| 3a | `history.replaceState` in the first script clears it; a later script sees `location.hash === ""` | pass |
| 3b | The fragment never appears in a request line or `Referer` (the server scans its whole request log) | pass |
| 4 | Explicit part-host `frame-ancestors` admits the same-host wrapper | pass. `frame-ancestors 'self'` also admitted it on every engine (info); keep it explicit anyway |
| 5 | Credentialed same-site `fetch` from the wrapper to `parts./api/chrome/v1/a/echo` carries the Lax `__Host-` cookie (JSON POST, so preflighted) | pass, WebKit, webkit-mobile and Mobile Safari (default ITP) included |
| 6 | The framed part can't read `parent.document` / `parent.sessionStorage` / `parent.location.href` | pass (`SecurityError` everywhere) |
| 7 | `#share=public` survives the hop (kept in sessionStorage, restored) | pass |

*Consequence:* the hand-off design stands as written in PLAN §2. Two adjustments:
- the wrapper's iframe needs `allow="… *"` (change 3);
- add `Referrer-Policy: no-referrer` (or `same-origin`) on the wrapper if the part name shouldn't reach `/grant`'s logs as a `Referer`. It's only the origin today, which the `part=` parameter already carries, so this is optional.

### Item 11: deferred (not run: no test zone)

A wildcard CNAME to `<tunnel>.cfargotunnel.com` serving a new `*-part` name needs a Cloudflare zone and tunnel. It's handed to the Cloudflare adapter work (rest-of-P1 epic). Nothing in Slice 1 depends on it.

### Item 12: WebSocket + SSE from a sandboxed document, held 75 s with a 25 s heartbeat (pass)

Three streams per lane per proxy:
- `wss://<host>/c/<cap>/api/ws`;
- `EventSource('/c/<cap>/api/sse')`, from a backend with no buffering hint;
- `EventSource('/c/<cap>/_rt/events')`, which sends `X-Accel-Buffering: no`.

The WS handshake carries `Origin: null` on every engine, and SSE requests carry `Origin: null` with `Sec-Fetch-Mode: cors`. Each needs the `ACAO: null` answer. The hold crossed the 60 s marks where proxy idle timeouts usually sit.

| Proxy | WS | SSE `/api/sse` (no hint) | SSE `/_rt/events` (`X-Accel-Buffering: no`) | Settings needed |
| --- | --- | --- | --- | --- |
| direct (Node, h1) | pass | pass | pass | – |
| Caddy 2.10 (defaults) | pass | pass | pass | none: `reverse_proxy` upgrades WS and flushes `text/event-stream` by default |
| nginx 1.27 tuned | pass | pass | pass | `proxy_http_version 1.1`; `Upgrade`/`Connection $connection_upgrade` map; `proxy_buffering off` (or rely on `X-Accel-Buffering: no`); `proxy_read_timeout` above the heartbeat (default 60 s works with 25 s heartbeats; 1 h in the draft) |
| nginx naive (bare `proxy_pass`, the control) | **fails** (1006) | **fails** (buffered: no headers in 75 s) | pass | it shows the two settings are required: the WS upgrade headers, and buffering off for any SSE that doesn't send `X-Accel-Buffering: no` |
| Traefik v3.4 (defaults, file provider) | pass | pass | pass | none at 75 s; the 25 s heartbeat keeps the stream inside Traefik's default timeouts |

*Consequence (`/_rt`, part backends):*
- `/_rt/events` always sends `X-Accel-Buffering: no` and `Cache-Control: no-cache`, with a ≤ 25 s heartbeat as a named event (comments are invisible to `EventSource`, so the client can't watch them).
- The prelude should add `X-Accel-Buffering: no` guidance for part backends' SSE, and the docs should put it in the nginx example.
- The draft configs are `tests/sandbox-spike/proxies/{Caddyfile,nginx.conf,traefik.yml,dynamic.yml}`. The nginx and Traefik examples ship after Slice 1.
- The nginx-naive `fail` lines in the Playwright `results.jsonl` come from the control. They were recorded before the rig relabelled control results as `info`; the simulator run already shows them as `info`.

### Item 13: passkey with `rp.id` = apex, asserted from the wrapper origin

- **Chromium desktop and Pixel 7 emulation (virtual authenticator): pass.** Registered on `https://parts.pf.test` with `rp.id: "pf.test"`. Asserted from the wrapper origin `https://a-part.pf.test` with `rpId: "pf.test"`: `clientDataJSON.origin` = `https://a-part.pf.test`, and `authenticatorData.rpIdHash` = SHA-256(`pf.test`).
- **WebKit, Firefox (Playwright): `not_assertable`.** There's no virtual authenticator there (`SecurityError: The operation is insecure`).
- **Mobile Safari, iOS 27 Simulator: `not_assertable`, with evidence.** `create()` on `parts.localtest.me` with `rp.id: "localtest.me"`, called without a user gesture, was accepted: there was no `SecurityError` and the system sheet appeared.
  - With no biometrics enrolled, the sheet says "Simulator requires enrolled biometrics to use passkeys" (`ios-sim/13-midway.png`).
  - After enrolling (`notifyutil … BiometricKit.enrollmentChanged`), it shows **"Save a passkey? 'localtest.me' supports passkeys … for 'spike@localtest.me'"** with "Add Passkey" (`ios-sim/13-enrolled-midway.png`). That confirms WebKit accepts the apex as rp.id from the workspace subdomain.
  - Completing it needs a tap, and simctl has no input, so it timed out after 20 s. The real-device assertion moves to the human checks.

*Consequence:* step-up with `rp.id` = apex from the wrapper origin is GO on Chromium, and plausible on WebKit up to the user prompt. Decision 36 stands (Telegram: open in the browser; Hermes hints).

### Known losses (§2): confirmed on every lane

| Loss | Result |
| --- | --- |
| Service workers | `navigator.serviceWorker` throws (Chromium) or `register` rejects |
| Persistent IndexedDB | `indexedDB.open` throws `SecurityError` on every engine |
| Cache Storage | throws or rejects |
| Remembered permissions | structural (`self.origin === "null"`, so there's nothing to key a grant on). The geolocation permission reads `denied` or `prompt` |
| Own-host `<a download>` name | ignored on all five Playwright lanes. Chromium/Firefox use the URL's name; WebKit navigates to the file. `?dl=1` with `Content-Disposition: attachment; filename="pf-report.txt"` gives the right name everywhere. Simulator: n/a (no tap or download observer) |

## Go / no-go per design choice

| Design choice (PLAN §2/§3) | Verdict |
| --- | --- |
| Part documents under CSP `sandbox` (no `allow-same-origin`), cookie-free, one host per part | **GO** |
| `'self'` in the part CSP | **NO-GO alone**: spell out `https://<host>` (+ `wss://<host>` in `connect-src`) |
| `Origin: null` → `ACAO: null` + `Vary: Origin`, never ACAC/CORP | **GO** (fetch, modules, EventSource, WS all work) |
| `/_rt` as a CORS-simple `text/plain` POST, no `Sec-Fetch-Site` check | **GO** (no preflight on any engine; site is `cross-site` everywhere) |
| Re-grant rule on `Sec-Fetch-Dest` ∈ {document, iframe} | **GO** (all engines send it) |
| Prelude shims: `defineProperty` storage, inert cookie, `indexedDB`/`caches` undefined, blob-worker wrapper | **GO** (classic blob workers; Proxy-backed storage) |
| Wrapper at the part-host root, `#pf=` hand-off, `replaceState`, sessionStorage cap, `#share` re-apply | **GO** |
| Explicit part-host `frame-ancestors` | **GO** |
| Chrome API: credentialed same-site fetch with Lax session, CORS for exact part origins | **GO** (WebKit + ITP included) |
| "A part's requests carry no Lax cookies" as a security property | **NO-GO on WebKit**: rely on CSP + the workspace rejecting `Origin: null` / `cross-site` (change 2) |
| Wrapper iframe permissions | **GO with `allow="clipboard-write *; fullscreen *; web-share *"`** |
| SSE + WebSocket through Caddy (shipped), nginx, Traefik | **GO** with the settings above |
| WebAuthn `rp.id` = apex from the wrapper origin | **GO** on Chromium; WebKit accepts the rp.id up to the user prompt; real device in the human checks |
| Known losses | **Confirmed** as stated |

## What implementers must do

**Part host (pf-e3wx):**
- Canonical CSP with the host spelled out (item 1).
- `sandbox` only from the header, on every HTML/SVG response under `/c/`, error pages included.
- `ACAO: null` + `Vary: Origin` for `Origin: null` on every `/c/` response, including SSE, and accept `Origin: null` on WS upgrades.
- Never send CORP or ACAC.
- Navigation detection by `Sec-Fetch-Dest`.
- Blob paths must not require `Origin`.
- Downloads via `?dl=1` → `Content-Disposition`.

**Wrapper:**
- The first script reads `#pf=` and calls `replaceState` before anything else.
- Iframe `allow="clipboard-write *; fullscreen *; web-share *"`.
- Credentialed chrome API calls to the workspace.
- Consider `Referrer-Policy: no-referrer`.

**Prelude:**
- `Object.defineProperty` shims with a Proxy-backed `Storage`.
- Redefine `indexedDB`/`caches` (they're reachable natively).
- The Worker wrapper uses classic blob workers, and absolute URLs inside them.
- `EventSource`/`fetch` prefixing as planned.

**`/_rt` and part backends:**
- Accept `Origin: null` and require `Sec-Fetch-Mode: cors` when present. No `Sec-Fetch-Site` check.
- SSE with `X-Accel-Buffering: no`, `Cache-Control: no-cache`, and a named heartbeat event ≤ 25 s.

**Workspace (auth/chrome API):**
- Reject `Origin: null` and `Sec-Fetch-Site: cross-site` on every endpoint before it acts.
- Cookies host-only `__Host-` Lax only, never `SameSite=None` or `Domain=`.

**Proxies / deploy:** Caddy defaults are fine. The nginx and Traefik examples use the settings in item 12.

## Needs real-device confirmation (for the human-checks bead)

- Item 6 on a real iPhone and Android: clipboard write, `navigator.share` sheet, fullscreen (expected absent on iPhone), and a `target=_blank` popup escaping the sandbox, all from a tap inside the wrapper's frame.
- Item 13 on a real iPhone (Safari) and Android (Chrome): register on `parts.<domain>`, assert from `<name>-part.<domain>` with `rp.id` = apex.
- Telegram's in-app browser (iOS and Android): item 13 passkey, plus the item 10 hop and the item 8 cookie behaviour. Approximated here only by the WebKit lanes.
- Item 8 on a real iPhone: confirm Mobile Safari sends Lax cookies on a sandboxed frame's credentialed request, as the simulator does. Change 2 is required either way.
- Own-host `<a download>` behaviour on iOS Safari (simulator: n/a).

## Running it

```bash
tests/sandbox-spike/run.sh                 # five Playwright lanes (+ item 12 proxies if Docker is up), renders results.md
tests/sandbox-spike/ios-sim.sh             # Mobile Safari in the iOS Simulator (macOS, binds :443)
tests/sandbox-spike/ios-sim.sh --delete-device
ci/spike.sh                                # the CI job: npm ci, playwright install, run.sh
```

Outputs:
- `target/sandbox-spike/results.jsonl`, `ios-sim/results.jsonl` (one `event="spike.result"` line per check per lane);
- `results.md` (the tables below);
- `screens/<lane>.png`, and `ios-sim/<item>.png` with mid-flight shots for items 10 and 13.

Rig notes:
- A cold Safari in a freshly created simulator can hang on its first `openurl`. Rebooting the device fixed it, and `ios-sim.sh` now detaches `openurl` and polls the collector.
- `simctl keychain add-root-cert` can hang when the CA is already present; the script adds it once per device.

## Evidence

- Playwright run: 5 lanes, 570 result lines, every item on every lane (render reports no missing lane × item). Screenshots are in `target/sandbox-spike/screens/`.
- Simulator run: 112 result lines plus the item-13 rerun. Screenshots are in `target/sandbox-spike/ios-sim/` (`10-midway.png`, `10.png`, `13-midway.png`, `13-enrolled-midway.png`, one per item).
- CI: the `spike` job runs `ci/spike.sh`. No runner has run it yet, so `ci_green` is open.

## Full per-check results (generated by `render.mjs`)

Generated 2026-10-24T22:33:47.099Z from `target/sandbox-spike/results.jsonl` and `ios-sim/results.jsonl`.

### Lanes

| Lane | Engine | Version | OS / runtime | Device | UA |
| --- | --- | --- | --- | --- | --- |
| chromium-desktop | chromium | 151.0.7922.34 | Darwin 27.0.0 (arm64) | desktop | `Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/151.0.79` |
| chromium-mobile | chromium | 151.0.7922.34 | Darwin 27.0.0 (arm64) | Pixel 7 | `Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.7922.34 Mobile Sa` |
| webkit-desktop | webkit | 26.5 | Darwin 27.0.0 (arm64) | desktop | `Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.5 Safari/6` |
| webkit-mobile | webkit | 26.5 | Darwin 27.0.0 (arm64) | iPhone 15 | `Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.5 M` |
| firefox-desktop | firefox | 153.0 | Darwin 27.0.0 (arm64) | desktop | `Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:153.0) Gecko/20100101 Firefox/153.0` |
| ios-sim | Mobile Safari (WebKit) | from UA | iOS 27.0 (24A434) simulator on macOS 27.0.1 | pf-spike-iphone | `Mozilla/5.0 (iPhone; CPU iPhone OS 18_7 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.0 M` |

### Summary (worst outcome per item; fail > pass > n/a > info)

| Item | chromium-desktop | chromium-mobile | webkit-desktop | webkit-mobile | firefox-desktop | ios-sim |
| --- | --- | --- | --- | --- | --- | --- |
| 1 'self' matches the part host | pass (20/21) | pass (20/21) | **FAIL** (1/21) | **FAIL** (1/21) | pass (20/21) | **FAIL** (1/21) |
| 2 Origin / Sec-Fetch-* values | pass (15/15) | pass (15/15) | pass (15/15) | pass (15/15) | pass (15/15) | pass (15/15) |
| 3 fetch, modules, import() with ACAO null | pass (5/5) | pass (5/5) | pass (5/5) | pass (5/5) | pass (5/5) | pass (5/5) |
| 4 localStorage redefinition | pass (5/6) | pass (5/6) | pass (5/6) | pass (5/6) | pass (5/6) | pass (5/6) |
| 5 blob workers, relative-URL wrapper | pass (4/6) | pass (4/6) | pass (5/6) | pass (5/6) | pass (5/6) | pass (5/6) |
| 6 popups, clipboard, share, fullscreen | pass (6/12, 2 n/a) | pass (6/12, 2 n/a) | pass (8/12) | pass (7/12) | pass (7/12, 2 n/a) | pass (1/12, 10 n/a) |
| 7 cookie writes fail | pass (3/4) | pass (3/4) | pass (3/4) | pass (3/4) | pass (3/4) | pass (3/4) |
| 8 no Lax cookies to siblings/workspace | pass (4/6) | pass (4/6) | **FAIL** (2/6) | **FAIL** (2/6) | pass (4/6) | **FAIL** (2/6) |
| 9 sandbox from header only | pass (3/3) | pass (3/3) | pass (3/3) | pass (3/3) | pass (3/3) | pass (3/3) |
| 10 wrapper flow (7 checks) | pass (8/9) | pass (8/9) | pass (8/9) | pass (8/9) | pass (8/9) | pass (8/9) |
| 11 Cloudflare wildcard CNAME | deferred | deferred | deferred | deferred | deferred | deferred |
| 12 WebSocket + SSE held open, per proxy | **FAIL** (2/15) | **FAIL** (2/15) | **FAIL** (2/15) | **FAIL** (2/15) | **FAIL** (2/15) | pass (13/15) |
| 13 WebAuthn rp.id = apex from wrapper origin | pass (2/2) | pass (2/2) | n/a | n/a | n/a | n/a |
| L known losses (§2) | pass (6/6) | pass (6/6) | pass (6/6) | pass (6/6) | pass (6/6) | pass (4/6, 2 n/a) |

### Every check

| Item | Check | chromium-desktop | chromium-mobile | webkit-desktop | webkit-mobile | firefox-desktop | ios-sim |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | `explicit_connect_src` | pass | pass | pass | pass | pass | pass |
| 1 | `explicit_connect_src_wss` | pass | pass | pass | pass | pass | pass |
| 1 | `explicit_excludes_sibling_host` | pass | pass | pass | pass | pass | pass |
| 1 | `explicit_form_action_and_frame_src` | pass | pass | pass | pass | pass | pass |
| 1 | `explicit_https_only_connect_src` | pass | pass | pass | pass | pass | pass |
| 1 | `explicit_https_only_connect_src_wss` | info | info | info | info | info | info |
| 1 | `explicit_https_only_excludes_sibling_host` | pass | pass | pass | pass | pass | pass |
| 1 | `explicit_https_only_form_action_and_frame_src` | pass | pass | pass | pass | pass | pass |
| 1 | `explicit_https_only_img_src` | pass | pass | pass | pass | pass | pass |
| 1 | `explicit_https_only_origin_is_opaque` | pass | pass | pass | pass | pass | pass |
| 1 | `explicit_https_only_script_src` | pass | pass | pass | pass | pass | pass |
| 1 | `explicit_img_src` | pass | pass | pass | pass | pass | pass |
| 1 | `explicit_origin_is_opaque` | pass | pass | pass | pass | pass | pass |
| 1 | `explicit_script_src` | pass | pass | pass | pass | pass | pass |
| 1 | `self_connect_src` | pass | pass | **FAIL** | **FAIL** | pass | **FAIL** |
| 1 | `self_connect_src_wss` | pass | pass | pass | pass | pass | pass |
| 1 | `self_excludes_sibling_host` | pass | pass | pass | pass | pass | pass |
| 1 | `self_form_action_and_frame_src` | pass | pass | pass | pass | pass | pass |
| 1 | `self_img_src` | pass | pass | pass | pass | pass | pass |
| 1 | `self_origin_is_opaque` | pass | pass | pass | pass | pass | pass |
| 1 | `self_script_src` | pass | pass | pass | pass | pass | pass |
| 2 | `framed_fetch_origin_null` | pass | pass | pass | pass | pass | pass |
| 2 | `framed_fetch_sec_fetch_site` | pass | pass | pass | pass | pass | pass |
| 2 | `framed_navigation_dest` | pass | pass | pass | pass | pass | pass |
| 2 | `framed_rt_post_is_simple` | pass | pass | pass | pass | pass | pass |
| 2 | `framed_subresource_image` | pass | pass | pass | pass | pass | pass |
| 2 | `framed_subresource_module` | pass | pass | pass | pass | pass | pass |
| 2 | `framed_subresource_script` | pass | pass | pass | pass | pass | pass |
| 2 | `top_fetch_origin_null` | pass | pass | pass | pass | pass | pass |
| 2 | `top_fetch_sec_fetch_site` | pass | pass | pass | pass | pass | pass |
| 2 | `top_navigation_dest` | pass | pass | pass | pass | pass | pass |
| 2 | `top_rt_post_is_simple` | pass | pass | pass | pass | pass | pass |
| 2 | `top_subresource_image` | pass | pass | pass | pass | pass | pass |
| 2 | `top_subresource_module` | pass | pass | pass | pass | pass | pass |
| 2 | `top_subresource_script` | pass | pass | pass | pass | pass | pass |
| 2 | `wrapper_framed_part_nav_headers` | pass | pass | pass | pass | pass | pass |
| 3 | `credentialed_read_refused` | pass | pass | pass | pass | pass | pass |
| 3 | `dynamic_import_blob_host` | pass | pass | pass | pass | pass | pass |
| 3 | `dynamic_import_own_host` | pass | pass | pass | pass | pass | pass |
| 3 | `fetch_relative_json` | pass | pass | pass | pass | pass | pass |
| 3 | `module_script_tag` | pass | pass | pass | pass | pass | pass |
| 4 | `indexedDB_caches_undefined` | pass | pass | pass | pass | pass | pass |
| 4 | `localStorage_property_access` | pass | pass | pass | pass | pass | pass |
| 4 | `native_storage_in_opaque_origin` | info | info | info | info | info | info |
| 4 | `redefine_document_cookie` | pass | pass | pass | pass | pass | pass |
| 4 | `redefine_localStorage` | pass | pass | pass | pass | pass | pass |
| 4 | `redefine_sessionStorage` | pass | pass | pass | pass | pass | pass |
| 5 | `blob_module_worker` | info | info | pass | pass | pass | pass |
| 5 | `blob_worker_fetch_own_host_absolute` | pass | pass | pass | pass | pass | pass |
| 5 | `blob_worker_importScripts_own_host` | pass | pass | pass | pass | pass | pass |
| 5 | `blob_worker_inline` | pass | pass | pass | pass | pass | pass |
| 5 | `direct_relative_worker` | info | info | info | info | info | info |
| 5 | `relative_url_wrapper` | pass | pass | pass | pass | pass | pass |
| 6 | `allowsrc_clipboard_write` | info | info | pass | pass | pass | n/a |
| 6 | `allowsrc_fullscreen` | info | info | info | info | info | n/a |
| 6 | `allowsrc_navigator_share` | n/a | n/a | info | info | n/a | n/a |
| 6 | `allowsrc_popup_escapes_sandbox` | pass | pass | pass | pass | pass | n/a |
| 6 | `allowsrc_popup_with_activation` | pass | pass | pass | pass | pass | n/a |
| 6 | `api_presence` | info | info | info | info | info | info |
| 6 | `clipboard_write` | pass | pass | pass | pass | pass | n/a |
| 6 | `fullscreen` | pass | pass | pass | info | pass | n/a |
| 6 | `navigator_share` | n/a | n/a | pass | pass | n/a | n/a |
| 6 | `popup_escapes_sandbox` | pass | pass | pass | pass | pass | n/a |
| 6 | `popup_with_activation` | pass | pass | pass | pass | pass | n/a |
| 6 | `popup_without_activation` | info | info | info | info | info | pass |
| 7 | `cookiestore_set_fails` | pass | pass | pass | pass | pass | pass |
| 7 | `document_cookie_write_fails` | pass | pass | pass | pass | pass | pass |
| 7 | `info_apex_samesite_none_cookie_reaches_part_host` | info | info | info | info | info | info |
| 7 | `no_part_planted_cookie_reaches_part_host` | pass | pass | pass | pass | pass | pass |
| 8 | `canonical_csp_blocks_canary` | pass | pass | pass | pass | pass | pass |
| 8 | `canonical_csp_blocks_workspace` | pass | pass | pass | pass | pass | pass |
| 8 | `no_lax_cookies_to_canary` | pass | pass | **FAIL** | **FAIL** | pass | **FAIL** |
| 8 | `no_lax_cookies_to_workspace` | pass | pass | **FAIL** | **FAIL** | pass | **FAIL** |
| 8 | `samesite_none_cookie_to_canary` | info | info | info | info | info | info |
| 8 | `samesite_none_cookie_to_workspace` | info | info | info | info | info | info |
| 9 | `from_header` | pass | pass | pass | pass | pass | pass |
| 9 | `not_from_meta` | pass | pass | pass | pass | pass | pass |
| 9 | `not_from_report_only` | pass | pass | pass | pass | pass | pass |
| 10 | `1_nocap_location_replace_to_grant` | pass | pass | pass | pass | pass | pass |
| 10 | `2_pf_fragment_survives_302` | pass | pass | pass | pass | pass | pass |
| 10 | `3a_replacestate_clears_before_other_scripts` | pass | pass | pass | pass | pass | pass |
| 10 | `3b_fragment_never_in_referer_or_request_logs` | pass | pass | pass | pass | pass | pass |
| 10 | `4_explicit_frame_ancestors_admits_same_host_wrapper` | pass | pass | pass | pass | pass | pass |
| 10 | `5_credentialed_same_site_fetch_carries_lax_cookie` | pass | pass | pass | pass | pass | pass |
| 10 | `6_framed_part_cannot_read_parent` | pass | pass | pass | pass | pass | pass |
| 10 | `7_share_fragment_survives_hop` | pass | pass | pass | pass | pass | pass |
| 10 | `info_frame_ancestors_self_admits_wrapper` | info | info | info | info | info | info |
| 12 | `sse_api@caddy` | pass | pass | pass | pass | pass | pass |
| 12 | `sse_api@direct` | pass | pass | pass | pass | pass | pass |
| 12 | `sse_api@nginx` | pass | pass | pass | pass | pass | pass |
| 12 | `sse_api@nginxnaive` | **FAIL** | **FAIL** | **FAIL** | **FAIL** | **FAIL** | info |
| 12 | `sse_api@traefik` | pass | pass | pass | pass | pass | pass |
| 12 | `sse_rt@caddy` | pass | pass | pass | pass | pass | pass |
| 12 | `sse_rt@direct` | pass | pass | pass | pass | pass | pass |
| 12 | `sse_rt@nginx` | pass | pass | pass | pass | pass | pass |
| 12 | `sse_rt@nginxnaive` | pass | pass | pass | pass | pass | pass |
| 12 | `sse_rt@traefik` | pass | pass | pass | pass | pass | pass |
| 12 | `ws@caddy` | pass | pass | pass | pass | pass | pass |
| 12 | `ws@direct` | pass | pass | pass | pass | pass | pass |
| 12 | `ws@nginx` | pass | pass | pass | pass | pass | pass |
| 12 | `ws@nginxnaive` | **FAIL** | **FAIL** | **FAIL** | **FAIL** | **FAIL** | info |
| 12 | `ws@traefik` | pass | pass | pass | pass | pass | pass |
| 13 | `assert_from_wrapper_origin_rpid_apex` | pass | pass | n/a | n/a | n/a | n/a |
| 13 | `register_on_workspace_rpid_apex` | pass | pass | n/a | n/a | n/a | n/a |
| L | `cache_storage_lost` | pass | pass | pass | pass | pass | pass |
| L | `dl1_content_disposition_name` | pass | pass | pass | pass | pass | n/a |
| L | `download_attr_name_lost` | pass | pass | pass | pass | pass | n/a |
| L | `indexeddb_lost` | pass | pass | pass | pass | pass | pass |
| L | `remembered_permissions_lost` | pass | pass | pass | pass | pass | pass |
| L | `service_worker_lost` | pass | pass | pass | pass | pass | pass |

### Details for fail, info and n/a results

- **1 `explicit_https_only_connect_src_wss`**
  - chromium-desktop (info): `blocked: Error: ws error`
  - chromium-mobile (info): `blocked: Error: ws error`
  - webkit-desktop (info): `blocked: SecurityError: The operation is insecure.`
  - webkit-mobile (info): `blocked: SecurityError: The operation is insecure.`
  - firefox-desktop (info): `blocked: NS_ERROR_CONTENT_BLOCKED: `
  - ios-sim (info): `blocked: SecurityError: The operation is insecure.`
- **1 `self_connect_src`**
  - webkit-desktop (fail): `TypeError: Load failed`
  - webkit-mobile (fail): `TypeError: Load failed`
  - ios-sim (fail): `TypeError: Load failed`
- **4 `native_storage_in_opaque_origin`**
  - chromium-desktop (info): `{"localStorage":"threw SecurityError","sessionStorage":"threw SecurityError","indexedDB":"available","caches":"threw SecurityError","cookie":"threw SecurityError"}`
  - chromium-mobile (info): `{"localStorage":"threw SecurityError","sessionStorage":"threw SecurityError","indexedDB":"available","caches":"threw SecurityError","cookie":"threw SecurityError"}`
  - webkit-desktop (info): `{"localStorage":"threw SecurityError","sessionStorage":"threw SecurityError","indexedDB":"available","caches":"threw SecurityError","cookie":"threw SecurityError"}`
  - webkit-mobile (info): `{"localStorage":"threw SecurityError","sessionStorage":"threw SecurityError","indexedDB":"available","caches":"threw SecurityError","cookie":"threw SecurityError"}`
  - firefox-desktop (info): `{"localStorage":"threw SecurityError","sessionStorage":"threw SecurityError","indexedDB":"available","caches":"available","cookie":"threw SecurityError"}`
  - ios-sim (info): `{"localStorage":"threw SecurityError","sessionStorage":"threw SecurityError","indexedDB":"available","caches":"threw SecurityError","cookie":"threw SecurityError"}`
- **5 `blob_module_worker`**
  - chromium-desktop (info): `Error: worker error: onerror`
  - chromium-mobile (info): `Error: worker error: onerror`
- **5 `direct_relative_worker`**
  - chromium-desktop (info): `failed (wrapper needed): SecurityError: Failed to construct 'Worker': Script at 'https://a-part.pf.test/c/v1.spike.chromium-desktop-mvmyk3fx/assets/w.js' cannot be accessed from origin 'null'.`
  - chromium-mobile (info): `failed (wrapper needed): SecurityError: Failed to construct 'Worker': Script at 'https://a-part.pf.test/c/v1.spike.chromium-mobile-mvmyk3fy/assets/w.js' cannot be accessed from origin 'null'.`
  - webkit-desktop (info): `failed (wrapper needed): Error: worker error: onerror`
  - webkit-mobile (info): `failed (wrapper needed): Error: worker error: onerror`
  - firefox-desktop (info): `failed (wrapper needed): Error: worker error: onerror`
  - ios-sim (info): `failed (wrapper needed): Error: worker error: onerror`
- **6 `allowsrc_clipboard_write`**
  - chromium-desktop (info): `blocked as expected with the default allowlist: NotAllowedError: Failed to execute 'writeText' on 'Clipboard': The Clipboard API has been blocked because of a permissions policy applied to the current document. See https://crbug.com/414348233 for more details.`
  - chromium-mobile (info): `blocked as expected with the default allowlist: NotAllowedError: Failed to execute 'writeText' on 'Clipboard': The Clipboard API has been blocked because of a permissions policy applied to the current document. See https://crbug.com/414348233 for more details.`
  - ios-sim (not_assertable): `no user activation in this lane (needs a tap)`
- **6 `allowsrc_fullscreen`**
  - chromium-desktop (info): `blocked as expected with the default allowlist: TypeError: Disallowed by permissions policy`
  - chromium-mobile (info): `blocked as expected with the default allowlist: TypeError: Disallowed by permissions policy`
  - webkit-desktop (info): `blocked as expected with the default allowlist: TypeError: Fullscreen API is disabled by permissions policy.`
  - webkit-mobile (info): `blocked as expected with the default allowlist: requestFullscreen absent on this platform (iPhone WebKit has no element fullscreen)`
  - firefox-desktop (info): `blocked as expected with the default allowlist: TypeError: Fullscreen request denied`
  - ios-sim (not_assertable): `no user activation in this lane (needs a tap)`
- **6 `allowsrc_navigator_share`**
  - chromium-desktop (not_assertable): `navigator.share absent in this lane`
  - chromium-mobile (not_assertable): `navigator.share absent in this lane`
  - webkit-desktop (info): `blocked as expected with the default allowlist: NotAllowedError: Third-party iframes are not allowed to call share() unless explicitly allowed via Feature-Policy (web-share)`
  - webkit-mobile (info): `blocked as expected with the default allowlist: NotAllowedError: Third-party iframes are not allowed to call share() unless explicitly allowed via Feature-Policy (web-share)`
  - firefox-desktop (not_assertable): `navigator.share absent in this lane`
  - ios-sim (not_assertable): `no user activation in this lane (needs a tap)`
- **6 `allowsrc_popup_escapes_sandbox`**
  - ios-sim (not_assertable): `no user activation in this lane (needs a tap)`
- **6 `allowsrc_popup_with_activation`**
  - ios-sim (not_assertable): `no user activation in this lane (needs a tap)`
- **6 `api_presence`**
  - chromium-desktop (info): `{"clipboard":true,"share":"undefined","canShare":"undefined","fullscreenEnabled":false,"webkitFullscreenEnabled":false,"requestFullscreen":"function"}`
  - chromium-mobile (info): `{"clipboard":true,"share":"undefined","canShare":"undefined","fullscreenEnabled":false,"webkitFullscreenEnabled":false,"requestFullscreen":"function"}`
  - webkit-desktop (info): `{"clipboard":true,"share":"function","canShare":"function","fullscreenEnabled":false,"webkitFullscreenEnabled":false,"requestFullscreen":"function"}`
  - webkit-mobile (info): `{"clipboard":true,"share":"function","canShare":"function","requestFullscreen":"undefined"}`
  - firefox-desktop (info): `{"clipboard":true,"share":"undefined","canShare":"undefined","fullscreenEnabled":false,"requestFullscreen":"function"}`
  - ios-sim (info): `{"clipboard":true,"share":"function","canShare":"function","requestFullscreen":"undefined"}`
- **6 `clipboard_write`**
  - ios-sim (not_assertable): `no user activation in this lane (needs a tap)`
- **6 `fullscreen`**
  - webkit-mobile (info): `requestFullscreen absent on this platform (iPhone WebKit has no element fullscreen)`
  - ios-sim (not_assertable): `no user activation in this lane (needs a tap)`
- **6 `navigator_share`**
  - chromium-desktop (not_assertable): `navigator.share absent in this lane`
  - chromium-mobile (not_assertable): `navigator.share absent in this lane`
  - firefox-desktop (not_assertable): `navigator.share absent in this lane`
  - ios-sim (not_assertable): `no user activation in this lane (needs a tap)`
- **6 `popup_escapes_sandbox`**
  - ios-sim (not_assertable): `no user activation in this lane (needs a tap)`
- **6 `popup_with_activation`**
  - ios-sim (not_assertable): `no user activation in this lane (needs a tap)`
- **6 `popup_without_activation`**
  - chromium-desktop (info): `opened (no popup blocker in this lane)`
  - chromium-mobile (info): `opened (no popup blocker in this lane)`
  - webkit-desktop (info): `opened (no popup blocker in this lane)`
  - webkit-mobile (info): `opened (no popup blocker in this lane)`
  - firefox-desktop (info): `opened (no popup blocker in this lane)`
- **7 `info_apex_samesite_none_cookie_reaches_part_host`**
  - chromium-desktop (info): `SENT`
  - chromium-mobile (info): `SENT`
  - webkit-desktop (info): `SENT`
  - webkit-mobile (info): `SENT`
  - firefox-desktop (info): `SENT`
  - ios-sim (info): `SENT`
- **8 `no_lax_cookies_to_canary`**
  - webkit-desktop (fail): `[{"k":"canary-get","target":"canary","method":"GET","cookies":["pf_dom_lax","pf_dom_none"],"origin":"null","sfs":"cross-site"},{"k":"canary-post","target":"canary","method":"POST","cookies":["pf_dom_lax","pf_dom_none"],"origin":"null","sfs":"cross-site"}]`
  - webkit-mobile (fail): `[{"k":"canary-get","target":"canary","method":"GET","cookies":["pf_dom_lax","pf_dom_none"],"origin":"null","sfs":"cross-site"},{"k":"canary-post","target":"canary","method":"POST","cookies":["pf_dom_lax","pf_dom_none"],"origin":"null","sfs":"cross-site"}]`
  - ios-sim (fail): `[{"k":"canary-get","target":"canary","method":"GET","cookies":["pf_dom_lax","pf_dom_none"],"origin":"null","sfs":"cross-site"},{"k":"canary-post","target":"canary","method":"POST","cookies":["pf_dom_lax","pf_dom_none"],"origin":"null","sfs":"cross-site"}]`
- **8 `no_lax_cookies_to_workspace`**
  - webkit-desktop (fail): `[{"k":"workspace-get","target":"workspace","method":"GET","cookies":["pf_dom_lax","pf_dom_none","__Host-pf_sess"],"origin":"null","sfs":"cross-site"},{"k":"workspace-post","target":"workspace","method":"POST","cookies":["pf_dom_lax","pf_dom_none","__Host-pf_sess"],"origin":"null","sfs":"cross-site"}]`
  - webkit-mobile (fail): `[{"k":"workspace-get","target":"workspace","method":"GET","cookies":["pf_dom_lax","pf_dom_none","__Host-pf_sess"],"origin":"null","sfs":"cross-site"},{"k":"workspace-post","target":"workspace","method":"POST","cookies":["pf_dom_lax","pf_dom_none","__Host-pf_sess"],"origin":"null","sfs":"cross-site"}]`
  - ios-sim (fail): `[{"k":"workspace-get","target":"workspace","method":"GET","cookies":["pf_dom_lax","pf_dom_none","__Host-pf_sess"],"origin":"null","sfs":"cross-site"},{"k":"workspace-post","target":"workspace","method":"POST","cookies":["pf_dom_lax","pf_dom_none","__Host-pf_sess"],"origin":"null","sfs":"cross-site"}]`
- **8 `samesite_none_cookie_to_canary`**
  - chromium-desktop (info): `SENT (any SameSite=None cookie on the apex is exposed)`
  - chromium-mobile (info): `SENT (any SameSite=None cookie on the apex is exposed)`
  - webkit-desktop (info): `SENT (any SameSite=None cookie on the apex is exposed)`
  - webkit-mobile (info): `SENT (any SameSite=None cookie on the apex is exposed)`
  - firefox-desktop (info): `SENT (any SameSite=None cookie on the apex is exposed)`
  - ios-sim (info): `SENT (any SameSite=None cookie on the apex is exposed)`
- **8 `samesite_none_cookie_to_workspace`**
  - chromium-desktop (info): `SENT (any SameSite=None cookie on the apex is exposed)`
  - chromium-mobile (info): `SENT (any SameSite=None cookie on the apex is exposed)`
  - webkit-desktop (info): `SENT (any SameSite=None cookie on the apex is exposed)`
  - webkit-mobile (info): `SENT (any SameSite=None cookie on the apex is exposed)`
  - firefox-desktop (info): `SENT (any SameSite=None cookie on the apex is exposed)`
  - ios-sim (info): `SENT (any SameSite=None cookie on the apex is exposed)`
- **10 `info_frame_ancestors_self_admits_wrapper`**
  - chromium-desktop (info): `'self' admitted the same-host wrapper`
  - chromium-mobile (info): `'self' admitted the same-host wrapper`
  - webkit-desktop (info): `'self' admitted the same-host wrapper`
  - webkit-mobile (info): `'self' admitted the same-host wrapper`
  - firefox-desktop (info): `'self' admitted the same-host wrapper`
  - ios-sim (info): `'self' admitted the same-host wrapper`
- **12 `sse_api@nginxnaive`**
  - chromium-desktop (fail): `{"hello":0,"hbs":0,"errors":0,"firstMs":null,"origin":null,"readyState":0,"want":2}`
  - chromium-mobile (fail): `{"hello":0,"hbs":0,"errors":0,"firstMs":null,"origin":null,"readyState":0,"want":2}`
  - webkit-desktop (fail): `{"hello":0,"hbs":0,"errors":0,"firstMs":null,"origin":null,"readyState":0,"want":2}`
  - webkit-mobile (fail): `{"hello":0,"hbs":0,"errors":0,"firstMs":null,"origin":null,"readyState":0,"want":2}`
  - firefox-desktop (fail): `{"hello":0,"hbs":0,"errors":0,"firstMs":null,"origin":null,"readyState":0,"want":2}`
  - ios-sim (info): `control, breaks without the tuned settings: {"hello":0,"hbs":0,"errors":0,"firstMs":null,"origin":null,"readyState":0,"want":2}`
- **12 `ws@nginxnaive`**
  - chromium-desktop (fail): `{"hello":0,"hbs":0,"closed":{"code":1006,"reason":"","atMs":61},"firstMs":null,"origin":null,"error":true,"readyState":3,"want":2}`
  - chromium-mobile (fail): `{"hello":0,"hbs":0,"closed":{"code":1006,"reason":"","atMs":63},"firstMs":null,"origin":null,"error":true,"readyState":3,"want":2}`
  - webkit-desktop (fail): `{"hello":0,"hbs":0,"closed":{"code":1006,"reason":"","atMs":296},"firstMs":null,"origin":null,"error":true,"readyState":3,"want":2}`
  - webkit-mobile (fail): `{"hello":0,"hbs":0,"closed":{"code":1006,"reason":"","atMs":236},"firstMs":null,"origin":null,"error":true,"readyState":3,"want":2}`
  - firefox-desktop (fail): `{"hello":0,"hbs":0,"closed":{"code":1006,"reason":"","atMs":78},"firstMs":null,"origin":null,"error":true,"readyState":3,"want":2}`
  - ios-sim (info): `control, breaks without the tuned settings: {"hello":0,"hbs":0,"closed":{"code":1006,"reason":"","atMs":114},"firstMs":null,"origin":null,"error":true,"readyState":3,"want":2}`
- **13 `assert_from_wrapper_origin_rpid_apex`**
  - webkit-desktop (not_assertable): `NotAllowedError: The document is not focused.`
  - webkit-mobile (not_assertable): `NotAllowedError: The document is not focused.`
  - firefox-desktop (not_assertable): `SecurityError: The operation is insecure.`
  - ios-sim (not_assertable): `Error: timeout 20000ms`
- **13 `register_on_workspace_rpid_apex`**
  - webkit-desktop (not_assertable): `NotAllowedError: The document is not focused.`
  - webkit-mobile (not_assertable): `NotAllowedError: The document is not focused.`
  - firefox-desktop (not_assertable): `SecurityError: The operation is insecure.`
  - ios-sim (not_assertable): `Error: timeout 20000ms`
- **L `dl1_content_disposition_name`**
  - ios-sim (not_assertable): `needs a tap and a download observer (Playwright lanes only)`
- **L `download_attr_name_lost`**
  - ios-sim (not_assertable): `needs a tap and a download observer (Playwright lanes only)`
