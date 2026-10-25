#!/usr/bin/env bash
# Running-binary CLI gate; localtest.me needs no hosts-file changes.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo"
scripts/disk-guard.sh
cargo build -p partifactory --features dev
export PF_BIN="$repo/target/debug/partifactory"
export PF_APEX=localtest.me PF_HTTPS_PORT=18443
export PF_RUN_DIR="$repo/target/e2e-runs/ops-$(date +%s)"
proxy_pid=''
cleanup() {
  if [ -n "$proxy_pid" ]; then kill "$proxy_pid" 2>/dev/null || true; fi
  tests/e2e/harness/down.sh "$PF_RUN_DIR"
}
trap cleanup EXIT
tests/e2e/harness/up.sh >/dev/null
export PARTIFACTORY_URL="https://parts.$PF_APEX:$PF_HTTPS_PORT"
export PARTIFACTORY_UPSTREAM="$PARTIFACTORY_URL"
export PARTIFACTORY_TOKEN="$(cat "$PF_RUN_DIR/secrets/api_token")"
export PARTIFACTORY_CA_CERT="$PF_RUN_DIR/certs/ca.pem"
ln -s "$PF_BIN" "$PF_RUN_DIR/part"
part="$PF_RUN_DIR/part"
mkdir -p "$PF_RUN_DIR/dist"
echo '<h1>alpha beta</h1>' > "$PF_RUN_DIR/x.html"
"$part" put "$PF_RUN_DIR/x.html" --name packing --kind html > "$PF_RUN_DIR/create.json"
"$part" get packing > "$PF_RUN_DIR/get.json"
python3 - "$PF_RUN_DIR" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1])
out=json.load(open(p/'get.json'))
assert out['content']['x.html']==(p/'x.html').read_text()
for rid,old,new in [('a','alpha','one'),('b','beta','two')]:
    (p/(rid+'.json')).write_text(json.dumps({'name':'packing','base_version':1,'wait':False,'request_id':rid,'edits':[{'op':'replace','file':'x.html','old_str':old,'new_str':new}]}))
(p/'timeout.json').write_text(json.dumps({'name':'packing','wait':False,'request_id':'timeout-once','edits':[{'op':'replace','file':'x.html','old_str':'one','new_str':'three'}]}))
PY
"$part" part_edit --json "@$PF_RUN_DIR/a.json" > "$PF_RUN_DIR/a-result.json"
"$part" part_edit --json "@$PF_RUN_DIR/b.json" > "$PF_RUN_DIR/b-result.json"
cp "$PF_RUN_DIR/x.html" "$PF_RUN_DIR/dist/index.html"
"$part" put "$PF_RUN_DIR/dist" --name bundle --kind files > "$PF_RUN_DIR/dist-first.json"
"$part" put "$PF_RUN_DIR/dist" --name bundle --kind files > "$PF_RUN_DIR/dist-second.json"
export PF_TIMEOUT_PROXY_PORT="$PF_RUN_DIR/proxy-port"
python3 tests/api/timeout-proxy.py > "$PF_RUN_DIR/timeout-proxy.log" 2>&1 &
proxy_pid=$!
for _ in $(seq 1 50); do [ -s "$PF_TIMEOUT_PROXY_PORT" ] && break; sleep 0.1; done
PARTIFACTORY_URL="http://127.0.0.1:$(cat "$PF_TIMEOUT_PROXY_PORT")" \
  "$part" part_edit --json "@$PF_RUN_DIR/timeout.json" > "$PF_RUN_DIR/timeout-result.json"
curl -fsS --cacert "$PARTIFACTORY_CA_CERT" "$PARTIFACTORY_UPSTREAM/openapi.json" > "$PF_RUN_DIR/openapi.json"
python3 - "$PF_RUN_DIR" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1])
assert json.load(open(p/'b-result.json'))['rebased_from']==1
assert json.load(open(p/'dist-second.json'))['unchanged'] is True
assert json.load(open(p/'timeout-result.json'))['replayed'] is True
assert json.load(open(p/'openapi.json'))['openapi']=='3.1.0'
lines=[json.loads(s) for s in (p/'server.jsonl').read_text().splitlines() if s.startswith('{')]
assert any(s.get('event')=='op' and s.get('surface')=='cli' for s in lines)
assert any(s.get('event')=='http.request' and s.get('route_class')=='api' for s in lines)
assert len([s for s in lines if s.get('event')=='blob.put'])==1
print('ops gate: source roundtrip, zero duplicate upload, disjoint rebase, timeout replay, OAS3.1 and CLI/API logs PASS')
PY
mkdir -p target/e2e-logs/ops
cp "$PF_RUN_DIR/timeout-proxy.log" "$PF_RUN_DIR/openapi.json" target/e2e-logs/ops/
