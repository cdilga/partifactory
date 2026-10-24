#!/usr/bin/env bash
# Install what one CI job needs into the job container (scripts/bootstrap-dev.sh does the same on
# a laptop). `ci/install-tools.sh <job>`.
set -euo pipefail
job="${1:?usage: ci/install-tools.sh <job>}"
bin="${CARGO_HOME:-$HOME/.cargo}/bin"
GITLEAKS_VERSION=8.30.1

binstall() {
  if ! command -v cargo-binstall >/dev/null 2>&1; then
    curl -L --proto '=https' --tlsv1.2 -sSf \
      https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash
  fi
  cargo binstall -y --locked "$@"
}

gitleaks_install() {
  command -v gitleaks >/dev/null 2>&1 && return
  curl -sSfL "https://github.com/gitleaks/gitleaks/releases/download/v${GITLEAKS_VERSION}/gitleaks_${GITLEAKS_VERSION}_linux_x64.tar.gz" \
    | tar xz -C "$bin" gitleaks
}

case "$job" in
  test) binstall cargo-nextest ;;
  deny) binstall cargo-deny ;;
  audit) binstall cargo-audit ;;
  advisories) binstall cargo-audit cargo-deny ;;
  gitleaks) gitleaks_install ;;
  selftest) gitleaks_install ;;
  fmt | clippy | check-boundaries | env-docs) ;;
  *) echo "install-tools: unknown job $job" >&2; exit 2 ;;
esac
# The toolchain (with rustfmt and clippy) comes from rust-toolchain.toml.
rustup show active-toolchain >/dev/null
