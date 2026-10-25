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
cgroup_args=()
if [ -f /sys/fs/cgroup/cgroup.controllers ]; then
  # Delegate only this container's private cgroup namespace. Never rearrange a
  # host hierarchy or a shared ancestor when the runner uses host cgroup mode.
  current_cgroup="$(awk -F: '$1 == "0" { print $3 }' /proc/self/cgroup)"
  if [ ! -f /.dockerenv ] || { [ "$current_cgroup" != / ] && [ "$current_cgroup" != /pf-ci-init ]; }; then
    echo 'ci docker: a private container cgroup namespace is required' >&2
    exit 1
  fi
fi
# The job uses an explicit capability set instead of runner-wide privileged mode.
# Give nested containers their own writable cgroup subtree in this job's mount namespace.
if [ -d /sys/fs/cgroup ] && [ ! -w /sys/fs/cgroup ]; then
  mount -o remount,rw /sys/fs/cgroup
fi
if [ -f /sys/fs/cgroup/cgroup.controllers ]; then
  mkdir -p /sys/fs/cgroup/pf-ci-init
  # cgroup v2 forbids domain controllers on a populated internal node. Keep the
  # job and daemon in one leaf and bounded nested workloads in a separate tree.
  # Snapshot before moving: cgroup.procs changes as each PID leaves, so streaming
  # it while moving can skip the next process and leave the internal node busy.
  mapfile -t job_pids < /sys/fs/cgroup/cgroup.procs
  for pid in "${job_pids[@]}"; do
    if ! printf '%s\n' "$pid" > /sys/fs/cgroup/pf-ci-init/cgroup.procs; then
      [ ! -d "/proc/$pid" ] || exit 1
    fi
  done
  read -r -a controllers < /sys/fs/cgroup/cgroup.controllers
  enable=''
  for controller in "${controllers[@]}"; do enable+="+$controller "; done
  printf '%s\n' "$enable" > /sys/fs/cgroup/cgroup.subtree_control
  mkdir -p /sys/fs/cgroup/pf-ci-workloads
  printf '%s\n' "$enable" > /sys/fs/cgroup/pf-ci-workloads/cgroup.subtree_control
  cgroup_args=(--cgroup-parent=/pf-ci-workloads)
fi
# Docker's bridge setup writes interface sysctls after entering a child's network
# namespace. Make only network sysctls writable in this job's mount namespace;
# retain the other masked/read-only proc paths and the runner's non-privileged mode.
mount --bind /proc/sys/net /proc/sys/net
mount -o remount,bind,rw /proc/sys/net
# Keep overlay upper directories on the private checkout volume: /tmp may be
# overlay-backed itself, while vfs copies every layer and stalls cold workloads.
nohup dockerd --host="$DOCKER_HOST" --data-root="$PWD/target/ci-docker" \
  --exec-root=/tmp/pf-ci-docker-exec --pidfile=/tmp/pf-ci-docker.pid \
  --storage-driver=overlay2 "${cgroup_args[@]}" > target/e2e-logs/docker-daemon.log 2>&1 &
for _ in $(seq 1 60); do
  if docker info >/dev/null 2>&1; then
    echo 'ci docker: isolated daemon ready; checkout bind mounts stay inside the job'
    exit 0
  fi
  sleep 1
done
tail -60 target/e2e-logs/docker-daemon.log >&2
exit 1
