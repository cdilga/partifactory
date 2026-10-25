#!/usr/bin/env bash
# The tag lane is thin plumbing; scripts/release.sh owns the portable deliverable.
set -euo pipefail
cd "$(dirname "$0")/.."
tag="${GITHUB_REF_NAME:-${GITEA_REF_NAME:-}}"
[[ "${GITHUB_REF:-${GITEA_REF:-}}" == refs/tags/* ]] || { echo 'release: a tag event is required' >&2; exit 2; }
[[ "$tag" == v* && "$tag" != *[!a-zA-Z0-9._-]* ]] || { echo 'release: invalid version tag' >&2; exit 2; }
for name in PF_RELEASE_IMAGE PF_RELEASE_REGISTRY_USER PF_RELEASE_REGISTRY_PASSWORD PF_RELEASE_TOKEN PF_RELEASE_API; do
  [ -n "${!name:-}" ] || { echo "release: required credential/configuration $name is missing" >&2; exit 2; }
done
git fetch origin main
git merge-base --is-ancestor HEAD origin/main || { echo 'release: tag must refer to a commit on main' >&2; exit 2; }
registry="${PF_RELEASE_IMAGE%%/*}"
[[ "$registry" != "$PF_RELEASE_IMAGE" && "$registry" != *[!a-zA-Z0-9.:-]* ]] || { echo 'release: image must include its registry hostname' >&2; exit 2; }
scripts/disk-guard.sh
mkdir -p target
source_config="${DOCKER_CONFIG:-$HOME/.docker}"
auth_dir="$(mktemp -d "$PWD/target/release-auth.XXXXXX")"
chmod 700 "$auth_dir"
# Preserve installed CLI plugins and the existing single BuildKit builder; only
# registry authentication belongs in this short-lived configuration directory.
if [ -d "$source_config/cli-plugins" ]; then ln -s "$source_config/cli-plugins" "$auth_dir/cli-plugins"; fi
export BUILDX_CONFIG="${BUILDX_CONFIG:-$source_config/buildx}"
export DOCKER_CONFIG="$auth_dir"
cleanup(){
  docker logout "$registry" >/dev/null 2>&1 || true
  rm -rf "$auth_dir"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
printf '%s' "$PF_RELEASE_REGISTRY_PASSWORD" | docker login "$registry" --username "$PF_RELEASE_REGISTRY_USER" --password-stdin >/dev/null
scripts/release.sh --tag "$tag"
