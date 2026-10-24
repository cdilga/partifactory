#!/usr/bin/env bash
# docs/env.md is generated from the config structs; fail if the committed copy differs.
source "$(dirname "$0")/lib.sh"
cargo run -q --profile ci -p partifactory -- config docs > docs/env.md
git diff --exit-code -- docs/env.md || {
  echo "env-docs: docs/env.md is stale; run: cargo run -p partifactory -- config docs > docs/env.md" >&2
  exit 1
}
echo "env-docs: ok"
