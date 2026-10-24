#!/usr/bin/env bash
# Tear the harness down, always: save the logs into the run dir, stop partifactory, then
# `docker compose down -v --remove-orphans`, then check nothing labelled pf.harness=1 is left.
#   down.sh [run dir]
set -uo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
h="$repo/tests/e2e/harness"
run="${1:-${PF_RUN_DIR:-}}"
export PF_RUN_DIR="${run:-/tmp/pf-harness-none}"

if [ -n "$run" ] && [ -d "$run" ]; then
  docker compose -f "$h/compose.yaml" --profile container logs --no-color caddy > "$run/caddy.log" 2>&1 || true
  docker compose -f "$h/compose.yaml" --profile container logs --no-color dex dex2 > "$run/dex.log" 2>&1 || true
  if [ -f "$run/logs.pid" ]; then
    kill "$(cat "$run/logs.pid")" 2>/dev/null || true
    rm -f "$run/logs.pid"
  fi
  if [ -f "$run/pf.pid" ]; then
    pid="$(cat "$run/pf.pid")"
    kill "$pid" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$pid" 2>/dev/null || break; sleep 0.5; done
    kill -9 "$pid" 2>/dev/null || true
    rm -f "$run/pf.pid"
  fi
fi

docker compose -f "$h/compose.yaml" --profile container down -v --remove-orphans >/dev/null 2>&1 || true

# Only this compose project's objects: other harnesses (the sandbox spike) share the label.
f=(--filter label=pf.harness=1 --filter label=com.docker.compose.project=pf-harness)
left_c="$(docker ps -aq "${f[@]}")"
left_v="$(docker volume ls -q "${f[@]}")"
left_n="$(docker network ls -q "${f[@]}")"
if [ -n "$left_c$left_v$left_n" ]; then
  echo "down.sh: pf.harness=1 objects left behind; removing" >&2
  [ -n "$left_c" ] && docker rm -f $left_c >/dev/null 2>&1
  [ -n "$left_v" ] && docker volume rm $left_v >/dev/null 2>&1
  [ -n "$left_n" ] && docker network rm $left_n >/dev/null 2>&1
fi
echo "down.sh: harness down"
