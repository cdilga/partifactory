#!/usr/bin/env bash
# Store durability gate (pf-lpvh). Drives `partifactory dev …` against a running
# `partifactory serve` built with the `dev` feature:
#   1. put / fork / restore / delete / undelete / history / diff, checking heads, history, diff;
#   2. a loop of acknowledged edits, `kill -9` of the server mid-loop, a restart, then: every
#      acknowledged version is present with its content, `n` is gap-free and every blob verifies.
# Logs checked: `db.migrate outcome=ok` on first start, one `version.committed` per committed
# version (so at least one per acknowledged edit), no `blob.corrupt`.
#
#   tests/e2e/store/durability.sh [path/to/partifactory]
#   PF_BIN=…        the binary (default: the first argument, else target/ci/partifactory)
#   PF_KEEP=1       keep the work dir (data + logs) for inspection
#   PF_LOOP_ACKS=N  acknowledged edits to wait for before the kill (default 25)
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
BIN="${PF_BIN:-${1:-$repo/target/ci/partifactory}}"
ACKS="${PF_LOOP_ACKS:-25}"
[ -x "$BIN" ] || { echo "durability: no binary at $BIN (build with --features partifactory/dev)" >&2; exit 2; }
command -v jq >/dev/null || { echo "durability: jq is required" >&2; exit 2; }

work="$(mktemp -d "${TMPDIR:-/tmp}/pf-dur.XXXXXX")"
log="$work/serve.log"
srv=""
loop_pid=""

cleanup() {
  local code=$?
  [ -n "$loop_pid" ] && kill "$loop_pid" 2>/dev/null || true
  if [ -n "$srv" ] && kill -0 "$srv" 2>/dev/null; then
    kill -TERM "$srv" 2>/dev/null || true
    wait "$srv" 2>/dev/null || true
  fi
  if [ "$code" -ne 0 ]; then
    echo "durability: FAILED (exit $code); last server log lines:" >&2
    tail -n 30 "$log" >&2 2>/dev/null || true
  fi
  if [ "${PF_KEEP:-}" = "1" ] || [ "$code" -ne 0 ]; then
    echo "durability: work dir kept at $work" >&2
  else
    rm -rf "$work"
  fi
}
trap cleanup EXIT

export PF_DATA_DIR="$work/data"
export PF_SITE_TOML="$work/site.toml" # absent: an empty config
export PF_SESSION_KEY="dur-session-key-0123456789abcdef0123"
export PF_CAP_HMAC_KEY="dur-cap-hmac-key-0123456789abcdef012"
export PF_METRICS_TOKEN="dur-metrics-token-0123"
export PF_DOMAIN_APEX="pf.test"
export PF_LISTEN_WORKSPACE="127.0.0.1:0" PF_LISTEN_CONTENT="127.0.0.1:0" PF_LISTEN_OPS="127.0.0.1:0"
export PF_DEV_SOCKET="$PF_DATA_DIR/run/dev.sock"
export PF_DEV_AS="admin"
unset PF_DEV_IDENTITY

fail() { echo "durability: FAIL: $*" >&2; exit 1; }
step() { echo; echo "== $*"; }
dev() { "$BIN" dev "$@"; }

start_server() {
  "$BIN" serve >>"$log" 2>&1 &
  srv=$!
  for _ in $(seq 1 300); do
    if [ -S "$PF_DEV_SOCKET" ] && dev fsck >/dev/null 2>&1; then return 0; fi
    kill -0 "$srv" 2>/dev/null || fail "serve exited during start"
    sleep 0.1
  done
  fail "serve did not come up within 30 s"
}

expect() { # expect <jq filter> <expected> <json>
  local got
  got="$(jq -c "$1" <<<"$3")"
  [ "$got" = "$2" ] || fail "expected $1 = $2, got $got"
}

# ------------------------------------------------------------------ 1. the operations
step "start serve (first start)"
start_server
grep -q '"event":"db.migrate"' "$log" || fail "no db.migrate line"
[ "$(grep '"event":"db.migrate"' "$log" | grep -c '"outcome":"ok"')" -ge 2 ] || fail "db.migrate outcome=ok missing for a database"
echo "db.migrate: $(grep -c '"event":"db.migrate".*"outcome":"ok"' "$log") x outcome=ok (partifactory.db + insights.db)"

step "sign in"
dev sign-in admin | jq -c '{username, principal}'

site="$work/site"
mkdir -p "$site"
printf '<h1>Packing</h1>\n<ul>\n<li>passports</li>\n</ul>\n' >"$site/index.html"
v1="$(cat "$site/index.html")"

step "put v1, v2, v3"
out="$(dev put packing "$site" --title "Packing list")"; echo "$out" | jq -c '{name, head, live}'
expect .head 1 "$out"
printf '<h1>Packing</h1>\n<ul>\n<li>passports</li>\n<li>chargers</li>\n</ul>\n' >"$site/index.html"
out="$(dev put packing "$site")"; echo "$out" | jq -c '{name, head}'; expect .head 2 "$out"
printf '<h1>Packing</h1>\n<ul>\n<li>chargers</li>\n<li>tent</li>\n<li>stove</li>\n</ul>\n' >"$site/index.html"
printf 'body { font: 16px system-ui }\n' >"$site/style.css"
out="$(dev put packing "$site")"; echo "$out" | jq -c '{name, head}'; expect .head 3 "$out"

step "fork packing@2"
out="$(dev fork packing@2 packing-copy)"; echo "$out" | jq -c '{name, head}'; expect .head 1 "$out"
out="$(dev get packing-copy)"; expect '.content["index.html"] | contains("chargers")' true "$out"

step "restore v1 as v4"
out="$(dev restore packing 1)"; echo "$out" | jq -c '{name, head}'; expect .head 4 "$out"
out="$(dev get packing)"
[ "$(jq -r '.content["index.html"]' <<<"$out")" = "$v1" ] || fail "restored content differs from v1"
expect '.files | keys' '["index.html"]' "$out"

step "delete, then undelete"
out="$(dev delete packing)"; echo "$out" | jq -c '{name, head, deleted_at}'
if dev get packing >"$work/err.json"; then fail "a deleted part was readable"; fi
expect .error.code '"not_found"' "$(cat "$work/err.json")"
out="$(dev undelete packing)"; echo "$out" | jq -c '{name, head}'; expect .head 4 "$out"

step "history"
out="$(dev history packing)"
echo "$out" | jq -c '.history[] | {n, via, message, author}'
expect '[.history[].n]' '[1,2,3,4]' "$out"
expect '.history[3].via' '"restore"' "$out"

step "diff v1..v3"
out="$(dev diff packing 1 3)"
echo "$out" | jq -c '.files[] | {path, status, added, removed}'
expect '[.files[] | {path, status, added, removed}]' \
  '[{"path":"index.html","status":"modified","added":3,"removed":1},{"path":"style.css","status":"added","added":1,"removed":0}]' "$out"

# ------------------------------------------------------------------ 2. kill -9 mid-loop
step "loop of acknowledged edits, then kill -9"
acked="$work/acked.txt"
: >"$acked"
loopdir="$work/loop"
mkdir -p "$loopdir"
(
  for i in $(seq 1 100000); do
    printf 'edit %d\n' "$i" >"$loopdir/index.html"
    if out="$(dev put loop "$loopdir" --request-id "loop-$i" 2>/dev/null)"; then
      echo "$(jq -r .head <<<"$out") $i" >>"$acked"
    fi
  done
) &
loop_pid=$!
for _ in $(seq 1 600); do
  [ "$(wc -l <"$acked")" -ge "$ACKS" ] && break
  sleep 0.05
done
kill -9 "$srv"
wait "$srv" 2>/dev/null || true
srv=""
sleep 0.3
kill "$loop_pid" 2>/dev/null || true
wait "$loop_pid" 2>/dev/null || true
loop_pid=""
n_acked="$(wc -l <"$acked" | tr -d ' ')"
last="$(tail -n 1 "$acked")"
echo "before kill: $n_acked acknowledged edits; last acknowledged: n=${last%% *} (edit ${last##* })"
[ "$n_acked" -ge "$ACKS" ] || fail "only $n_acked acknowledged edits before the kill"

step "restart serve"
start_server
grep -q '"event":"store.ready"' "$log" || fail "no store.ready line"

out="$(dev history loop)"
head="$(jq .head <<<"$out")"
ns="$(jq -c '[.history[].n]' <<<"$out")"
want="$(jq -c --argjson h "$head" -n '[range(1; $h + 1)]')"
[ "$ns" = "$want" ] || fail "n is not gap-free 1..$head: $ns"
echo "after restart: head=$head, versions 1..$head present, gap-free"
max_acked="$(sort -n "$acked" | tail -n 1 | cut -d' ' -f1)"
[ "$head" -ge "$max_acked" ] || fail "head $head is below the last acknowledged n=$max_acked"
while read -r n i; do
  c="$(dev get loop --version "$n" | jq -r '.content["index.html"]')"
  [ "$c" = "edit $i" ] || fail "acknowledged v$n should hold \"edit $i\", holds \"$c\""
done <"$acked"
echo "every acknowledged version ($n_acked) is present with its content"

out="$(dev fsck)"
echo "$out" | jq -c '{ok, blobs_ok, blobs_bad: (.blobs_bad | length), parts: [.parts[] | {name, head, gap_free}]}'
expect .ok true "$out"

# ------------------------------------------------------------------ logs
step "logs"
committed="$(grep -c '"event":"version.committed"' "$log" || true)"
total="$(jq '[.parts[].versions] | add' <<<"$out")"
echo "version.committed lines: $committed; versions in the database: $total; acknowledged loop edits: $n_acked"
# A version committed in the instant before the kill may lack its line (the line follows the
# commit), so allow one; never more lines than versions.
[ "$committed" -le "$total" ] || fail "more version.committed lines ($committed) than versions ($total)"
[ "$committed" -ge $((total - 1)) ] || fail "version.committed lines ($committed) missing for versions ($total)"
if grep -q '"event":"blob.corrupt"' "$log"; then fail "blob.corrupt logged"; fi
echo "no blob.corrupt"

echo
echo "durability: PASS"
