#!/usr/bin/env bash
# Explicit manual CI probes. Success means the selected real checker fails the job.
# Only disposable fixtures change; no credential-shaped value enters repo history.
source "$(dirname "$0")/lib.sh"
kind="${1:?usage: ci/negative-check.sh fmt|clippy|gitleaks}"
case "$kind" in fmt|clippy|gitleaks) ;; *) echo 'unknown negative checker' >&2; exit 2 ;; esac
fixture="$repo/target/ci-negative/$kind-$$"
mkdir -p "$fixture/src"
trap 'rm -rf -- "$fixture"' EXIT
cat > "$fixture/Cargo.toml" <<'TOML'
[package]
name = "pf-ci-negative-probe"
version = "0.0.0"
edition = "2024"
[workspace]
[features]
dev = []
[profile.dev]
debug = false
incremental = false
TOML
echo "negative-check: $kind must reject the disposable fixture"
case "$kind" in
  fmt)
    rustfmt --version
    printf 'pub fn probe( )->i32{1}\n' > "$fixture/src/lib.rs"
    cargo fmt --manifest-path "$fixture/Cargo.toml" --check
    ;;
  clippy)
    cargo clippy --version
    printf 'pub fn probe(values: &[i32]) -> usize { values.iter().count() }\n' > "$fixture/src/lib.rs"
    cargo clippy --manifest-path "$fixture/Cargo.toml" --features dev \
      --target-dir "$repo/target" -- -D warnings
    ;;
  gitleaks)
    gitleaks version
    python3 - "$fixture" <<'PY'
import pathlib,sys,uuid
pathlib.Path(sys.argv[1], 'planted.txt').write_text('pf' + '_pat_' + uuid.uuid4().hex + '\n')
PY
    git -C "$fixture" init --quiet
    git -C "$fixture" add planted.txt
    git -C "$fixture" -c user.name='CI probe' -c user.email='ci@example.com' commit --quiet -m 'Disposable checker probe'
    "$repo/ci/gitleaks.sh" "$fixture"
    ;;
esac
echo 'negative-check: checker unexpectedly accepted invalid input' >&2
exit 2
