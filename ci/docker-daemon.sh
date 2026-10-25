#!/usr/bin/env bash
# The CI job owns this daemon, so bind mounts see the job's checkout and no shared containers.
set -euo pipefail
mkdir -p target/e2e-logs
if ! command -v dockerd >/dev/null; then
  apt-get update -qq
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq docker.io >/dev/null
fi
export DOCKER_HOST=unix:///var/run/pf-ci-docker.sock
if docker info >/dev/null 2>&1; then exit 0; fi
nohup dockerd --host="$DOCKER_HOST" --data-root=/tmp/pf-ci-docker \
  --exec-root=/tmp/pf-ci-docker-exec --pidfile=/tmp/pf-ci-docker.pid \
  --storage-driver=vfs > target/e2e-logs/docker-daemon.log 2>&1 &
for _ in $(seq 1 60); do
  if docker info >/dev/null 2>&1; then
    echo 'ci docker: isolated daemon ready; checkout bind mounts stay inside the job'
    exit 0
  fi
  sleep 1
done
tail -60 target/e2e-logs/docker-daemon.log >&2
exit 1
