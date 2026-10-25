# Maintained macro diagnostics

Source: the crates.io release `i18n-embed-fl` 0.9.4, from
https://github.com/kellpossible/cargo-i18n/tree/master/i18n-embed-fl.
The upstream MIT license is retained in LICENSE.txt.

This compile-time localization macro is used by age 0.11.5. Its normalized
Cargo.toml replaces `proc-macro-error2` with maintained `proc-macro-error3` 3.1.1,
and src/lib.rs updates that one import to its canonical name (the diagnostic
attribute expands paths to its canonical crate). This removes RUSTSEC-2026-0173
without suppressing advisories.
Original Cargo.toml.orig remains unchanged. Both expose the abort, emit_error and
proc_macro_error macros this crate uses. The current upstream 0.10.1 release uses
the same maintained successor but requires a different localization stack.

Validation compiles the real age localization expansions, runs encrypted
export/import tests and audits the entire resolved dependency graph. Remove this
patch when a compatible upstream release adopts the maintained macro dependency.
