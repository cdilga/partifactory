# Deployment gates

These drivers inspect an actual caller-owned disposable deployment. The caller
starts it from `deploy/compose.yaml`, publishes its proxy, then always runs
`docker compose down -v --remove-orphans`. Gate scripts do not start a second
harness, prune the daemon or replace the test deployment's secrets. Use the
release image for these tests; the dev-feature harness image is not evidence
for the release image checks.

`image_checks.sh IMAGE@sha256:DIGEST` inspects the exported root filesystem,
non-root user, native healthcheck and release-only command/config behavior.
It removes its own temporary container. `log_limits.sh` checks the delivered
compose definitions with both optional profiles enabled and checks the actual
`json-file`, `10m` / `3` settings on every running container. Run it both with
and without the render/proxy profiles. The default requires the application and
actual Caddy proxy running. Stop both `proxy` and `partifactory-render` and pass
`--without-proxy` for the app-only log gate: it requires the application running,
both optional services absent, and log limits on every remaining running service.
The flag is accepted only by `log_limits.sh`; smoke always requires real Caddy.

```sh
tests/deploy/compose_smoke.sh --compose /private/normalized-compose.json \
  --base-url https://parts.pf.test --part-url "$PUBLIC_PART_URL" \
  --part-marker 'known restored content' --ca-cert /private/ca.pem \
  --connect-to 127.0.0.1:19443 --evidence-dir target/deploy-gate/smoke
```

Smoke requires expected bytes from a real public part URL through Caddy. It
accepts a stable `/l/<token>` URL: each observation performs the actual signed-out
`POST /go` flow, then fetches the content entry (`--part-entry`, default
`index.html`) using that fresh capability. This remains valid across boot-epoch
bumps and checks actual part bytes rather than the wrapper's title. A direct
content URL remains suitable when the caller does not restart the application.
It also checks readiness on the local ops listener, refuses published application
ports or public ops bindings, and checks that ops routes do not pass through
the proxy. TLS verification remains enabled. A client connection override
preserves the original hostname/SNI without editing `/etc/hosts`.

Add `--deploy-image "$GOOD_RELEASE_IMAGE" --binary /path/to/partifactory` to
smoke to exercise the successful deployment path too. It waits for the actual
two-minute readiness watch, checks the completed journal and new container's
digest, confirms the old container was removed and opens the part again.
Without this option smoke records `successful_deploy: null`; it does not
claim that the successful-switch gate was observed.

`rollback.sh` adds `--binary`, `--good-image`, `--bad-image`,
`--delayed-image` to the same connection arguments. All images must be pinned
by digest. The live stack must start on the good digest. The immediate image
must migrate successfully but fail the real native healthcheck. The delayed
image must become unready 60 seconds after `serve` starts. The script calls
the actual deploy CLI, samples the part throughout, observes restoration of
the good digest and readiness in under two minutes per case, and asserts
`deploy.rollback` with `reason=readyz_failed` and `outcome=ok`. It checks the
journal's candidate-failure/rollback phases and records outage samples.

Prepare faults from the real release with the small fixture build (after the
shared harness/build slot is available):

```sh
PF_RELEASE_ASSETS=target/release-assets \
  tests/deploy/fault-image.sh "$GOOD_IMAGE" "$PINNED_STATIC_BUSYBOX_IMAGE" \
  localhost:25000/partifactory-deploy-fixture
```

Use a digest-pinned **static** amd64 BusyBox (for example the musl variant),
not a dynamically linked glibc executable: release is `FROM scratch`. This
build copies the unmodified release binary to `/partifactory-real` and places
a fault-injecting launcher at `/partifactory`, so the deploy CLI's absolute
`docker exec … /partifactory healthcheck` still invokes the original binary's
real HTTP readiness probe. Migration also runs the original binary. The
immediate fixture leaves the listener unavailable; the delayed fixture serves
the real application for 60 seconds and then stops it. Neither returns a
fabricated healthcheck result or uses a fake HTTP server. The container stays
alive so restart policy cannot hide the failure. The fixture is labelled
`pf.harness=1` and must never be used as a
release. Keep the disposable registry, fixture image digests and any image
IDs in the caller's exact-resource cleanup list.
The local-assets form verifies the release checksums and uses its extracted OCI
layout as a named build context. The host uploads the exported fixture bytes to
the disposable registry, so the builder does not need access to host loopback.
`PF_FIXTURE_REGISTRY_SCHEME` defaults to `http` for this local registry.

All drivers accept `--host` as a Docker endpoint or context and `--env-file`
when required. When using the e2e overlay, first materialize the merged config
with `docker compose … config --format json`, then pass that one normalized
file: the deploy CLI intentionally accepts one compose file. Its service must
still be called `partifactory`.
For the harness's normalized config, remove only the application's fixed network
addresses before deploying a candidate beside it; retain the proxy address that
matches `trusted_proxies`. The shipped production compose uses dynamic application
addresses already. `deploy --image` applies an authoritative final image override
to literal normalized inputs as well as `${PF_IMAGE}` inputs.

Evidence contains private deployment details. Directories are mode 700 and raw
logs mode 600. Keep private logs out of public CI artifacts. A passed script
prints `event=deploy.check`; no script reports an unrun gate as passing.

`ci/deploy.sh` runs these gates in order using the checksummed `PF_RELEASE_ASSETS`
from the release-security job, without rebuilding the release or rerunning its
whole browser security suite. It requires an empty caller-owned source daemon by
default. `PF_DEPLOY_REQUIRE_FRESH_SOURCE=0` allows a local observation on an existing
daemon and explicitly records `fresh_source_daemon: false`; that does not satisfy
the fresh-context gate. An already running shared harness is always refused.

The driver tests successful deployment, both faults, and running log limits with
the proxy and renderer started, then both stopped. It then exports with the actual release CLI and
restores twice on a distinct empty daemon: an intentionally failing external test
must produce a failure report and metric 0, then the real restored-host checks must
complete within 900 seconds and produce metric 1. Both outcomes must appear on
Health. The daemon is pinned, limited to two CPUs, 2 GiB with no swap, 512 processes
and a 2 GiB tmpfs-backed labelled volume. Its API and HTTPS mapping bind only to
loopback. Teardown removes its exact resources and always tears down the source
compose deployment.

`drill-e2e.py` rejects the pre-import cap, follows the restored link's real go flow,
checks exact content, signs in through the actual independent Dex, obtains a new
scoped device token through real member consent, and creates a part in that
member's authorized space. It runs every external security probe and requires
each to pass. A caller-supplied argv capture hook obtains fresh logs and consistent
database/Insights exports after the canaries, before scanning them. Private ops
requests are transported through the target's actual proxy container, never
published at ingress. The broader accumulated browser suite remains a separate
required e2e job. Only explicit public report files are uploaded; sessions, tokens,
capabilities, offline config, database exports and raw logs remain private.
