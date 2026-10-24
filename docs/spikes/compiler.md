# Spike: the build child, oxc + rolldown as libraries, esbuild fallback (pf-twz8)

**Decision: GO-WITH-FALLBACK, with oxc + rolldown deferred for disk.**
- **The build-child protocol is a GO** and was observed: re-exec as `build-worker`, length-prefixed CBOR over stdin/stdout, 10 s deadline + SIGKILL, a 1 GiB memory cap, and a hang, panic or memory breach reported as `timeout`/`builder_crashed` with the parent still up. A pre-forked pool of 2 is cheap.
- **esbuild-as-subprocess behind the same protocol is a GO** and meets the TSX targets with a wide margin: p50 12-39 ms warm, p99 under 70 ms.
- **oxc `=0.155.0` + rolldown `=1.2.15` were not compiled.** The shared volume was at 18-21 GiB free during this session, under the admin's 21 GiB build floor, while other agents were building.

Since `tsx` comes after Slice 1 (decision 39), the recommendation is to **ship `tsx` on esbuild first**, behind the protocol exactly as built here, and run the oxc + rolldown spike as the first task of the tsx epic (pf-8ssu) on a machine with room.

## Question

Can TSX builds run in a child process that a hang, panic or memory blow-up can't take down with the server, fast enough for the TSX targets (p50 < 150 ms, p99 < 1 s)? And are oxc + rolldown usable as Rust libraries for it, or do we use the esbuild fallback?

## Versions

| What | Version |
| --- | --- |
| rustc / cargo | 1.97.1, macOS arm64 (8 cores) |
| ciborium | `=0.2.2` |
| libc | `=0.2.191` |
| esbuild | **0.28.2**, `@esbuild/darwin-arm64` from the npm registry, sha256 `10b6243df618d374bb2d5c9cfbe7052e1405f6aa4e53a6164f11a91b9f2e1384`. Linux needs `@esbuild/linux-x64` / `linux-arm64` 0.28.2, pinned by digest the same way |
| oxc (not built) | `=0.155.0` (2026-10-19), features `transformer, minifier, codegen, semantic`; MSRV 1.97.0 |
| rolldown (not built) | `=1.2.15` (2026-10-21), on crates.io; depends on `oxc ^0.155.0`, so the two pins agree |

## What was built

`spikes/spike-compiler`:
- **Protocol** (`src/lib.rs`): `Request::{Ping, Build(BuildRequest)}` → `Response::{Pong, Built(BuildOutput), Failed{code, message}}`. Frames are a `u32` big-endian length + CBOR (ciborium), with a 64 MiB frame cap. One worker serves many requests until EOF.
  - `BuildRequest` = entry, in-memory `files` map, backend, minify, plus a test-only `fault`.
  - `BuildOutput` = js, import map, build ms, and the peak RSS of the worker and of the tool.
- **The parent**: `Worker::spawn` re-execs `current_exe() build-worker` with piped stdin/stdout, and a reader thread feeds a channel. `call()` waits with `recv_timeout(deadline)`.
  - Timeout → `child.kill()` (SIGKILL) + reap → `Outcome::Timeout`.
  - EOF or a bad frame → reap → `Outcome::BuilderCrashed("killed by signal N" | "exited with …")`.
  - `Pool` keeps `max(1, min(2, cores/2))` warm workers and respawns any that die.
  - Every build logs `event="build.child" outcome="ok|failed|timeout|builder_crashed"`.
- **Memory cap**: `setrlimit(RLIMIT_AS, 1 GiB)` in the child (Linux enforces it), **plus a counting `#[global_allocator]` armed only in the worker**. macOS accepts RLIMIT_AS but doesn't enforce it, so the allocator carries the cap there. Over the cap, allocation fails, Rust aborts, and the parent sees SIGABRT.
- **The esbuild backend** (a grandchild of the server): the files go to a private tempdir, then esbuild runs with `--bundle --format=esm --jsx=automatic --target=es2022 --minify --metafile --external:<each virtual module>` and `env_clear()`. There's no `node_modules` and no network. A bare import outside the virtual set fails as `compile_error` ("Could not resolve \"left-pad\""). The metafile lists the externals actually used, and the **import map** maps each one, once, to its `/_mirror/react@18.3.1/...` URL.
- The CLI is `spike-compiler <entry.tsx>`, plus `bench` and `build-worker`.

## Observed runs

```
$ cd spikes && PF_ESBUILD=<pinned esbuild> cargo run -q -p spike-compiler -- examples/app.tsx
// ---- bundle.js (1017 bytes, 14 ms in the child)
import{useEffect as c,useMemo as f,useState as d}from"react";import{createRoot as g}from"react-dom/client";import{jsx as n,jsxs as l}from"react/jsx-runtime";function p(r,e){...
// ---- importmap.json
{
  "imports": {
    "react": "/_mirror/react@18.3.1/index.mjs",
    "react-dom/client": "/_mirror/react-dom@18.3.1/client.mjs",
    "react/jsx-runtime": "/_mirror/react@18.3.1/jsx-runtime.mjs"
  }
}
stderr: {"level":"INFO","event":"build.child","outcome":"ok","build_ms":14,...}
```

`PF_ESBUILD=<pinned esbuild> cargo test -p spike-compiler`, 5 passed:
- `panic_in_child_is_builder_crashed`: panic → `BuilderCrashed`; logs `outcome="builder_crashed"`; the replacement worker serves the next build.
- `hang_is_killed_after_deadline`: an infinite sleep → `Timeout` within the deadline (1.5 s in the test; `DEADLINE` = 10 s in product code); logs `outcome="timeout"`; the pool recovers.
- `rlimit_breach_is_failed_build`: allocating 64 MiB chunks past 1 GiB → `BuilderCrashed("killed by signal 6")`; logs `outcome="builder_crashed"`.
- `bundle_resolves_virtual_react_once`: five files, four importing `react`, give an import map of exactly `react`, `react-dom/client`, `react/jsx-runtime`. Every bare specifier in the bundle is a map key, React's source is never inlined, TS is stripped and the JSX goes through `react/jsx-runtime`.
- `compile_error_is_failed_not_crash`: an unresolvable import → `Failed{code:"compile_error"}`.

## Measurements (esbuild backend, `spike-compiler bench`, n = 30, debug build of the parent/worker)

```
cores=8 pool_size=2
spawn+ping  p50 2.97 ms  p99 4.14 ms      <- fork/exec of the worker + one CBOR round trip
warm ping   p50 0.022 ms p99 0.122 ms     <- the same round trip on a pre-forked worker
input        cold p50     cold p99     warm p50     warm p99  worker MB    tool MB    out KB
small         15.5 ms      21.1 ms      12.2 ms      15.5 ms        3.0       17.6       1.0
big           41.7 ms      45.3 ms      38.5 ms      66.9 ms        6.9       75.4       1.2
five          16.4 ms      18.3 ms      13.0 ms      18.8 ms        6.9       75.4       0.9
```
- *small* = `examples/app.tsx` (50 lines).
- *big* = app + a recharts-sized in-memory library (40 modules, about 600 KB of TS; tree-shaken to the one function used).
- *five* = five files.
- "cold" = spawn a fresh worker per build. "warm" = the pool.
- "tool MB" is the peak RSS of esbuild (from `RUSAGE_CHILDREN`, so it's the worker's running maximum: *five* inherits *big*'s peak).
- **Against the targets** (p50 < 150 ms, p99 < 1 s): met by about 10x on every input, cold or warm. The pool saves about 3 ms per build. Keep it for isolation and steady latency, but it isn't critical for speed; esbuild's own process start dominates. Memory peak is 75 MB for the big input, far under the 1 GiB cap.

## oxc + rolldown: what was measured instead of compiled

- `cargo generate-lockfile` for `oxc =0.155.0 (transformer, minifier, codegen, semantic) + rolldown =1.2.15`, done in a scratch project, resolves **273 registry crates** (27 `oxc_*`, 24 `rolldown*`). Their **compressed `.crate` sources total 43.2 MB**, the largest being `windows` 9.4 MB, `icu_segmenter` 4.5 MB, `rolldown` 1.4 MB.
- Build size and time: **not measured**. The disk floor stopped it. By comparison, the Leptos + axum spike used about 1.6 GB of debug `target/` for about 250 crates. oxc's AST/visitor crates are large codegen units, so budget **3-5 GB per profile** until measured.
- Promising for later: rolldown pulls in `rolldown_fs` (a filesystem abstraction, for the in-memory module set) and `rolldown_plugin_chunk_import_map` (import-map emission). Both point at the shape this spike needs.

## Findings and risks

- **esbuild doesn't merge repeated external imports.** Each source module keeps its own `import … from "react"` (4 in the five-file test). The import map resolves all of them to one URL, so the browser gets one React instance. It's cosmetic, about 30 bytes per import. rolldown would merge them.
- macOS doesn't enforce `RLIMIT_AS`, so the counting allocator is what made the breach test pass on the Mac. Production is Linux, where both apply. Keep the allocator: it also gives a clean, testable failure. For the esbuild grandchild, set `RLIMIT_AS` in a `pre_exec` (or use a cgroup in the container), because the allocator only guards the Rust worker.
- esbuild is a Go binary: one more pinned artefact in the image (about 10 MB), fetched by digest, with no Node needed.
- The `tsx` output depends on the toolchain, so the cache key `hash(manifest, toolchain)` must include the esbuild version/digest, and later the oxc/rolldown pins.

## What the dependent beads must do

- **pf-1q1u (builds)**: take the protocol from `spikes/spike-compiler/src/lib.rs` as the starting point:
  - frames: u32 BE length + CBOR (ciborium `=0.2.2`), 64 MiB cap;
  - the `Worker` with a reader thread + `recv_timeout`, SIGKILL on deadline, reaping with the signal recorded;
  - a `Pool` of `max(1, min(2, cores/2))` that respawns dead workers;
  - outcomes `ok | failed | timeout | builder_crashed`, logged as `event="build.child"`;
  - in the worker, `RLIMIT_AS` 1 GiB + the counting allocator; for any tool it runs, `env_clear()`, a private tempdir and rlimits via `pre_exec`.
  Port the five tests in `spikes/spike-compiler/tests/child.rs`. Slice 1 kinds (html etc.) can use the same child unchanged.
- **pf-8ssu (tsx epic)**:
  - Ship `tsx` on **esbuild 0.28.2 pinned by digest** behind this protocol. It meets the targets today.
  - Keep `--external` for the virtual React/react-dom 18.3.1 set and emit the import map from the metafile.
  - Make the first task the deferred oxc `=0.155.0` + rolldown `=1.2.15` compile on a machine with at least 30 GiB free: measure the `target/` size, build time and binary-size delta, and run the golden corpus on both backends. Switch only if it wins.
  - Vendor (`cargo vendor` of the pinned set) only once it's adopted.
- **pf-7qza (MCP), pf-wmcw (wrapper)**: nothing from this spike.
