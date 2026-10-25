#!/usr/bin/env bash
# Rebuild the embedded Leptos shell; the trusted part wrapper has its own 15 KiB budget.
set -euo pipefail
cd "$(dirname "$0")/.."
scripts/disk-guard.sh
if [[ "$(wasm-bindgen --version)" != "wasm-bindgen 0.2.129" ]]; then
  echo 'build-ui: requires wasm-bindgen-cli =0.2.129' >&2; exit 1
fi
cargo build -p pf-ui-shell --lib --target wasm32-unknown-unknown --profile wasm-release --no-default-features --features hydrate
wasm-bindgen --target web --no-typescript --out-dir crates/pf-ui-shell/pkg --out-name pf_ui_shell target/wasm32-unknown-unknown/wasm-release/pf_ui_shell.wasm
bytes=0
for asset in crates/pf-ui-shell/pkg/pf_ui_shell.js crates/pf-ui-shell/pkg/pf_ui_shell_bg.wasm; do
  brotli -q 11 -f "$asset"
  bytes=$((bytes + $(wc -c < "$asset.br")))
done
if (( bytes > 250 * 1024 )); then echo "build-ui: $bytes bytes brotli exceeds 250 KiB" >&2; exit 1; fi
echo "build-ui: hydrated shell $bytes bytes brotli (budget 256000)"
