# Temporary maintained macro dependency patch

Leptos remains pinned to 0.8.22, the version used for the SSR/hydration shell.
That release, `either_of` 0.1.9, `reactive_graph` 0.2.16,
`reactive_stores` 0.4.4 and `tachys` 0.2.19 still name the unmaintained `paste` crate
(RUSTSEC-2024-0436). These copies are the exact crates.io release contents,
with only the `[dependencies.paste]` entries in their normalized `Cargo.toml`
changed to alias the maintained `pastey` 0.2.3 successor. The original manifests,
source and tests are retained. The upstream MIT license (copyright Greg
Johnston), omitted from the published crate archives, is included at
`vendor/LICENSE` from https://github.com/leptos-rs/leptos/blob/main/LICENSE. No `paste` implementation is vendored,
and no advisory is suppressed.

The libraries use `paste!` to generate identifiers (for enum variants,
reactive field accessors and view tuples); the
`pastey::paste!` macro supports the same syntax. Leptos does not use its direct
`paste` dependency in 0.8.22. Keeping the dependency alias preserves its declared
API and does not require source changes.

Validation: compile the native SSR shell and the wasm hydration shell with their
normal feature sets, then run `cargo audit --deny warnings` and
`cargo deny check advisories`. The existing SSR, browser hydration and enum
compilation tests exercise the affected expansion.

Remove these patches once a compatible upstream release uses a maintained macro
dependency. Do not modify the vendored Rust sources for application changes.
