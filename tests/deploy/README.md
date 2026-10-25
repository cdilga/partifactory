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
and without the render/proxy profiles.

```sh
tests/deploy/compose_smoke.sh --compose /private/normalized-compose.json \
  --base-url https://parts.pf.test --part-url "$PUBLIC_PART_URL" \
  --part-marker 'known restored content' --ca-cert /private/ca.pem \
  --connect-to 127.0.0.1:19443 --evidence-dir target/deploy-gate/smoke
```

Smoke requires expected bytes from a real public part URL through Caddy. It
also checks readiness on the local ops listener, refuses published application
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

All drivers accept `--host` as a Docker endpoint or context and `--env-file`
when required. When using the e2e overlay, first materialize the merged config
with `docker compose … config --format json`, then pass that one normalized
file: the deploy CLI intentionally accepts one compose file. Its service must
still be called `partifactory`.

Evidence contains private deployment details. Directories are mode 700 and raw
logs mode 600. Keep private logs out of public CI artifacts. A passed script
prints `event=deploy.check`; no script reports an unrun gate as passing.
