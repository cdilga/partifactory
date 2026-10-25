#!/busybox sh
set -eu
if [ "${1:-}" = serve ]; then
  if [ "$PF_FAULT_MODE" = immediate ]; then
    # No listener: the original binary's real HTTP healthcheck must fail.
    exec /busybox sleep 2147483647
  elif [ "$PF_FAULT_MODE" = delayed ]; then
    /partifactory-real "$@" &
    child=$!
    trap '/busybox kill -TERM "$child" 2>/dev/null || true; exit 0' TERM INT
    /busybox sleep 60
    /busybox kill -TERM "$child"
    wait "$child" || true
    # Keep the faulty container alive so restart policy cannot hide the outage.
    exec /busybox sleep 2147483647
  fi
fi
exec /partifactory-real "$@"
