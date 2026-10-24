#!/usr/bin/env bash
# Proves the guards bite: the boundary check, gitleaks (CI and the pre-push hook) and the
# disk-hygiene scripts each fail on a planted fixture.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'chmod -R u+w "$tmp" 2>/dev/null; rm -rf "$tmp"' EXIT
fail() { echo "selftest: FAIL: $*" >&2; exit 1; }

# 1. Boundaries: pf-model gains an adapter dependency.
cp -R "$here/crates" "$tmp/crates"
printf '\n[dependencies]\npf-adapter-cloudflare = { path = "../pf-adapter-cloudflare" }\n' >> "$tmp/crates/pf-model/Cargo.toml"
if PF_CRATES_DIR="$tmp/crates" "$here/ci/check-boundaries.sh" 2>/dev/null; then
  fail "check-boundaries passed with pf-model -> pf-adapter-cloudflare"
fi
"$here/ci/check-boundaries.sh" >/dev/null || fail "check-boundaries fails on the real tree"
echo "selftest: boundaries ok"

# 2. gitleaks: a planted pf_pat_ token in a scratch commit fails the scan and the hook.
repo="$tmp/leak"
git init -q "$repo"
git -C "$repo" config user.email selftest@pf.test
git -C "$repo" config user.name selftest
git -C "$repo" config commit.gpgsign false
echo "token = \"pf_pat_$(printf 'Q7mZ2kL9xV4bN8cR1tY6wP3s')\"" > "$repo/agent.env"
git -C "$repo" add agent.env
git -C "$repo" commit -qm "planted secret"
if "$here/ci/gitleaks.sh" "$repo" >/dev/null 2>&1; then
  fail "gitleaks passed a planted pf_pat_ token"
fi
# The documented fixture token is allowed.
clean="$tmp/clean"
git init -q "$clean"
git -C "$clean" config user.email selftest@pf.test
git -C "$clean" config user.name selftest
git -C "$clean" config commit.gpgsign false
echo 'token = "pf_pat_TESTFIXTURE0000000000000000"' > "$clean/fixture.env"
git -C "$clean" add fixture.env
git -C "$clean" commit -qm "fixture"
"$here/ci/gitleaks.sh" "$clean" >/dev/null 2>&1 || fail "gitleaks flagged the documented fixture"
# The pre-push hook, installed into the leaky repo, refuses the push.
PF_HOOKS_REPO="$repo" "$here/scripts/install-hooks.sh" >/dev/null
bare="$tmp/remote.git"
git init -q --bare "$bare"
if git -C "$repo" push -q "$bare" HEAD:refs/heads/main 2>/dev/null; then
  fail "the pre-push hook let a planted secret through"
fi
echo "selftest: gitleaks ok (CI scan and pre-push hook both refuse the planted token)"

# 3. Disk hygiene on a fixture target/.
t="$tmp/target"
mkdir -p "$t/debug/deps" "$t/debug/incremental/old-abc" "$t/debug/incremental/new-def" "$t/e2e-logs"
dd if=/dev/zero of="$t/debug/deps/libold.rlib" bs=1024 count=3072 2>/dev/null
dd if=/dev/zero of="$t/debug/incremental/old-abc/x" bs=1024 count=1024 2>/dev/null
echo new > "$t/debug/deps/libnew.rlib"
echo new > "$t/debug/incremental/new-def/x"
touch -t 202001010000 "$t/debug/deps/libold.rlib" "$t/debug/incremental/old-abc"
for i in 1 2 3 4 5 6 7; do mkdir -p "$t/e2e-logs/run-$i"; touch -t "20200101000$i" "$t/e2e-logs/run-$i"; done
if PF_TARGET_DIR="$t" PF_TARGET_MAX_GB=0.002 PF_CLEAN_NO_DOCKER=1 "$here/scripts/disk-guard.sh" >/dev/null 2>&1; then
  fail "disk-guard passed an oversized target/"
fi
if PF_TARGET_DIR="$t" PF_DISK_MIN_FREE_GB=999999 PF_CLEAN_NO_DOCKER=1 "$here/scripts/disk-guard.sh" >/dev/null 2>&1; then
  fail "disk-guard passed below the free-space floor"
fi
dry="$(PF_TARGET_DIR="$t" PF_CLEAN_NO_DOCKER=1 "$here/scripts/clean-build.sh")"
echo "$dry" | grep -q "would remove: .*libold.rlib" || fail "clean-build dry run doesn't list the old artefact"
[ -f "$t/debug/deps/libold.rlib" ] || fail "clean-build dry run removed something"
PF_TARGET_DIR="$t" PF_CLEAN_NO_DOCKER=1 "$here/scripts/clean-build.sh" --apply >/dev/null
[ ! -e "$t/debug/deps/libold.rlib" ] || fail "clean-build --apply kept the old artefact"
[ ! -e "$t/debug/incremental/old-abc" ] || fail "clean-build --apply kept the old incremental dir"
[ -f "$t/debug/deps/libnew.rlib" ] || fail "clean-build --apply removed a file newer than its window"
[ -f "$t/debug/incremental/new-def/x" ] || fail "clean-build --apply removed a new incremental dir"
[ "$(ls "$t/e2e-logs" | wc -l | tr -d ' ')" = 5 ] || fail "clean-build kept more than 5 log runs"
PF_TARGET_DIR="$t" PF_TARGET_MAX_GB=0.002 "$here/scripts/disk-guard.sh" >/dev/null || fail "target/ still over the cap after clean-build"
echo "selftest: disk hygiene ok"
