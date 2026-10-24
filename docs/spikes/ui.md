# Spike: Leptos 0.8 SSR + hydration vs askama + vanilla-JS islands (pf-twz8)

**Decision: GO for Leptos 0.8.22 in the workspace, per the §5 rule.** The share sheet cost about 0.2 h of framework friction, far under a day. The hydrated part-view bundle is **90.5 KB brotli-11**, under 300 KB. CodeMirror 6 interop works both ways.

**The wrapper is a hand-written vanilla ES module + CSS (no framework, no hydration)**, built the way the templates variant's island is built. Its share sheet is about 0.9 KB brotli. The costs Leptos brings, a second compile target and a pinned `wasm-bindgen` CLI, are listed under Risks so the admin can override cheaply. The templates variant is complete and works the same.

## Question

Does Leptos 0.8 (SSR + hydration) earn its place over askama/maud + small vanilla-JS islands for the workspace UI, judged by the §5 rule? The rule: more than a day of friction on the share sheet, or more than 300 KB brotli for the hydrated part view, means templates.

## Versions

| What | Version |
| --- | --- |
| rustc / cargo | 1.97.1, target `wasm32-unknown-unknown` |
| leptos | `=0.8.22` (0.9.0-rc0 exists; not used) |
| leptos_axum | `=0.8.10` |
| any_spawner | 0.3.0 (direct dependency, see friction) |
| wasm-bindgen (crate and CLI) | `=0.2.129`. The CLI must match the crate exactly |
| askama | `=0.16.1` |
| axum | 0.8.9 |
| brotli CLI | Homebrew `brotli`, `-q 11` |
| wasm-opt | not installed. The sizes are **without** wasm-opt, so the real number is lower |
| Browser for the run | the admin's Chrome via browser-harness (CDP) |

## What was built

`spikes/spike-ui-app` (Leptos components, `ssr`/`hydrate`/`editor` features) and `spikes/spike-ui` (an axum server that serves both variants):
- **The 44 px part-view bar**: back, tag, title, name, `REV 12 ▾`, an agent-activity pulse, the audience chip, Share, Edit, Comments 3, and a ⋯ menu.
- **The share sheet**: the five-rung ladder (`Only me` · `People` · `A group` · `Everyone signed in` · `Anyone with the link`, the last one amber), the live sentence (updates on every click, nothing saved), the public rung's warning, a `Can view` / `Can contribute` switch with its own sentence, the URL with **Copy**, and a fake in-sheet step-up ("Confirm it's you…" → "Public link is on.", no navigation). Phones get a bottom sheet (CSS media query).
- `/leptos`: `leptos_axum::render_app_to_stream_with_context` + `<HydrationScripts>`, and the wasm calls `hydrate_body(App)`.
- `/templates`: the same markup from an askama template, plus `static/share-island.js` (about 60 lines, no framework) that wires the same behaviour.
- `/leptos/editor`: the CodeMirror 6 interop probe (a separate wasm build with `--features hydrate,editor`).
- `measure.sh` builds the wasm (`--profile wasm-release`: `opt-level="z"`, `lto`, `codegen-units=1`, `panic="abort"`), runs `wasm-bindgen --target web`, and prints brotli-11 sizes.

Run: `cd spikes && ./spike-ui/measure.sh && cargo run -p spike-ui`, then open `http://127.0.0.1:8081/leptos`, `/templates` and `/leptos/editor`.

## Measurements

`./spike-ui/measure.sh`:
```
leptos 0.8.22 hydrated part view (no wasm-opt):
  spike_ui_app_bg.wasm  raw 799.2 KB  brotli-11 84.8 KB
  spike_ui_app.js       raw 33.8 KB  brotli-11 5.7 KB
  total brotli-11: 90.5 KB (rule: <= 300 KB)
templates + island:
  share-island.js       raw 2.6 KB  brotli-11 0.9 KB
shared:
  app.css               raw 2.3 KB  brotli-11 0.7 KB
editor probe: wasm brotli-11 90.1 KB (CodeMirror itself loads separately)
```
The part-view bundle includes the bar *and* the share sheet. The wasm build took about 55 s cold on the Mac, and the wasm targets used about 0.5 GB of `target/`.

**Browser check** (browser-harness on Chrome, the same script on both pages). It clicked Share, the public rung, Can contribute, the step-up and Copy:
```
leptos    -> open=true | chip=Public link | s1=Anyone with the link can open this, without signing in. | s2=Anyone with the link can open and use this, without signing in. Only you and people you add can change it. | step=Public link is on. | copy=Copied
templates -> open=true | chip=Public link | s1=... same ... | s2=... same ... | step=Public link is on. | copy=Copy*
```
\*The island awaited the clipboard promise before relabelling. That's fixed now: the label changes first. Both pages render the same sheet (screenshots taken during the run).

**CodeMirror 6 interop** (`/leptos/editor`, about 20 minutes): `cm=true | len0=54 | len1=72 | len2=75`. The editor mounted in a Leptos `NodeRef` from an `Effect`. A Leptos button inserted text through a `#[wasm_bindgen(module = "/static/editor-glue.js")]` extern (54 → 72). Typing in CodeMirror updated a Leptos signal through a `Closure<dyn FnMut(String)>` (72 → 75). **Pass.**

## Friction log (honest hours)

Total framework friction for the share sheet and bar: **about 0.2 h**. Writing the components themselves was quick.
1. leptos_axum panicked on every request ("tried to spawn a Future with Executor::spawn() before a global executor was initialized"). The fix is `any_spawner::Executor::init_tokio()` at startup. `leptos::task::Executor` is private, so `any_spawner` has to be a direct dependency. `LeptosRoutes` usually hides this, and we don't use the router. (About 5 min.)
2. Browser-only APIs (clipboard) need `#[cfg(feature = "hydrate")]` and `web-sys` features. `JsValue` is `!Send`, so the CodeMirror handle needs `StoredValue::new_local`. (About 5 min.)
3. A second hydrate entry (the editor page) can't use `<HydrationScripts>`, which always calls `hydrate()`, so it needs a hand-written boot `<script type=module>`. (About 3 min.)
4. Minor SSR quirks: the trailing space in `"Share "` before `<em>` was dropped; `<link rel=modulepreload ... crossorigin="<nonce>">`; resource scripts emitted after `</html>`. None of them broke anything.

## Findings and risks

- **Build pipeline cost (the main argument for templates).** Leptos adds a second compile target (`wasm32-unknown-unknown`), a `wasm-bindgen` CLI pinned to the crate's exact version, optionally `wasm-opt`, and about 0.5 GB of extra `target/` per profile. CI and the image both need the wasm step to run before the server build, which embeds `pkg/`. That weighs against the binding disk-hygiene rules.
- **The wrapper's share sheet has to be vanilla anyway** (§2: "no framework hydration"). With Leptos in the workspace, the share sheet exists twice: Leptos in the workspace, the island in the wrapper. The live-sentence logic would live in Rust and in JS. Mitigation: the sentence comes from the server (an op or the chrome API returns it) and both sides render it.
- Hydration needs the SSR and client trees to match exactly. Keep conditional markup in `<Show>`, never `cfg`.
- 0.9 is in RC. Pin `=0.8.22`, and plan one migration later.
- 90.5 KB without wasm-opt leaves a lot of room under 300 KB. Keep `measure.sh` as a CI budget check, failing above 250 KB.

## How the wrapper's tiny standalone bundle is built (under this decision)

- **Hand-written ES modules + one CSS file, no framework, no bundler, no hydration.** The model is `static/share-island.js` + `static/app.css`: the server renders the bar and sheet markup, and the module wires them up and talks to the workspace only through the chrome API.
- The files sit in the repo (e.g. `crates/pf-serve/wrapper/`), are embedded with `include_bytes!`, precompressed with brotli-11 in `build.rs` (the `brotli` crate), and served as a content-addressed shared blob with immutable caching.
- Budget: under 15 KB brotli for the wrapper JS + CSS. The spike's sheet + bar is 1.6 KB.
- If minification is wanted later, run the esbuild binary already pinned for the builds fallback over it at build time. Not needed now.

## What the dependent beads must do

- **pf-wmcw (wrapper and Stockroom shell)**:
  - The wrapper: vanilla ES modules + CSS as above, embedded and precompressed, with a budget check. Port `share-island.js` (ladder, live sentence, mode switch, copy, step-up hook calling `navigator.credentials.get()`) and the bar markup. Take the live sentence from the server and don't duplicate the wording in JS.
  - The Stockroom workspace shell: Leptos `=0.8.22` SSR + hydration with `leptos_axum =0.8.10`. Call `any_spawner::Executor::init_tokio()` at startup (or use `LeptosRoutes`). Build the wasm with the `wasm-release` profile (`opt-level=z`, LTO, cgu=1, `panic=abort`) and `wasm-bindgen-cli =0.2.129`, wasm before server, `pkg/` embedded. Add a CI budget check at 250 KB brotli.
  - CodeMirror: the `#[wasm_bindgen(module=...)]` glue pattern from `spike-ui-app/static/editor-glue.js`, with CodeMirror vendored as a shared blob, not esm.sh.
  - If the admin prefers to avoid the wasm toolchain, switch to askama `=0.16.1` + islands; `/templates` shows it's equivalent.
- **pf-1q1u (builds), pf-7qza (MCP), pf-8ssu (tsx)**: nothing from this spike.
