#!/usr/bin/env bash
# Build the hydrated Leptos bundle for the part view and print its brotli size,
# next to the templates + island variant. pf-twz8 UI spike.
# Needs: rustup target wasm32-unknown-unknown, wasm-bindgen-cli =0.2.129, brotli.
# Optional: wasm-opt (binaryen) on PATH, used when present.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
ws="$here/.."
cd "$ws"

cargo build -p spike-ui-app --lib --target wasm32-unknown-unknown \
  --profile wasm-release --no-default-features --features hydrate
wasm="$ws/target/wasm32-unknown-unknown/wasm-release/spike_ui_app.wasm"
wasm-bindgen --target web --no-typescript --out-dir "$here/pkg" --out-name spike_ui_app "$wasm"
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int \
    -o "$here/pkg/spike_ui_app_bg.wasm" "$here/pkg/spike_ui_app_bg.wasm"
  opt="wasm-opt -Oz"
else
  opt="no wasm-opt"
fi

size() { wc -c <"$1" | tr -d ' '; }
br() { brotli -q 11 -c "$1" | wc -c | tr -d ' '; }
kb() { awk -v b="$1" 'BEGIN { printf "%.1f", b / 1024 }'; }

w="$here/pkg/spike_ui_app_bg.wasm"; j="$here/pkg/spike_ui_app.js"
i="$here/static/share-island.js"; c="$here/static/app.css"
total=$(( $(br "$w") + $(br "$j") ))
echo "leptos 0.8.22 hydrated part view ($opt):"
echo "  spike_ui_app_bg.wasm  raw $(kb "$(size "$w")") KB  brotli-11 $(kb "$(br "$w")") KB"
echo "  spike_ui_app.js       raw $(kb "$(size "$j")") KB  brotli-11 $(kb "$(br "$j")") KB"
echo "  total brotli-11: $(kb "$total") KB (rule: <= 300 KB)"
echo "templates + island:"
echo "  share-island.js       raw $(kb "$(size "$i")") KB  brotli-11 $(kb "$(br "$i")") KB"
echo "shared:"
echo "  app.css               raw $(kb "$(size "$c")") KB  brotli-11 $(kb "$(br "$c")") KB"

# CodeMirror interop probe bundle (served at /leptos/editor), not part of the rule.
if [ "${PROBE_EDITOR:-1}" = 1 ]; then
  cargo build -p spike-ui-app --lib --target wasm32-unknown-unknown \
    --profile wasm-release --no-default-features --features hydrate,editor
  wasm-bindgen --target web --no-typescript --out-dir "$here/pkg-editor" --out-name spike_ui_app "$wasm"
  echo "editor probe: wasm brotli-11 $(kb "$(br "$here/pkg-editor/spike_ui_app_bg.wasm")") KB (CodeMirror itself loads separately)"
fi
