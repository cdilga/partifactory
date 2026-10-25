# Maintained TOML parser

Source: the crates.io release `i18n-config` 0.4.8, from
https://github.com/kellpossible/cargo-i18n/tree/master/i18n-config.
The upstream MIT license is retained in LICENSE.txt.

This build dependency is reached through age's compile-time localization macros.
The release depends on unmaintained basic-toml (RUSTSEC-2026-0374).
Two changes replace that dependency with the maintained toml 0.9 parser:

- The normalized Cargo.toml aliases `basic-toml` to package `toml` 0.9.
- The parse-error field uses `basic_toml::de::Error`, the maintained parser's type.

No parser implementation is copied and no advisory is ignored. Original
Cargo.toml.orig is preserved. Validation compiles age's localization macros and
runs the application's encrypted export/import tests plus the dependency audit.
Remove this patch after an upstream compatible release adopts a maintained parser.
