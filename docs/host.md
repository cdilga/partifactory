# Host contract

Run the amd64 release on an OCI runtime with Docker Compose or Podman Compose. The
application has one durable volume at `/data`. It contains SQLite, blobs and exports.
Keep an offline copy of the site config, the immutable admin pin and the export age
identity in a password manager. Provide secrets as files or environment variables
using [the environment contract](env.md).

Copy `deploy/` to the host, copy `.env.example` to a private `.env`, and fill its image
reference from the release's `digest.txt`. Supply the site config and secret files;
the non-root user `65532:65532` must be able to read them. The data volume must be
writable by that user. Container logs rotate at 10 MB with three files per service.
The image embeds all browser assets and runs without a shell or package manager.

Only a trusted proxy or tunnel on the ingress network may reach ports 8080 and 8081.
The workspace host routes to 8080 and part hosts to 8081. Publish neither listener
directly. Port 9090 is bound to loopback for local readiness and authenticated metrics;
never put it behind the public proxy. Configure trusted proxy addresses explicitly in
`site.toml`; do not trust arbitrary forwarded headers.

For a host with its own proxy, attach it to the compose ingress network. Otherwise set
`COMPOSE_PROFILES=proxy`, provide a certificate covering the workspace and every part
host, then run `docker compose --env-file .env -f compose.yaml pull` and
`docker compose --env-file .env -f compose.yaml up -d`. The proxy binds to loopback by
default; choose the host bind address in the private `.env` when exposing it directly.
Health is checked by the binary's `healthcheck` command against local `/readyz`.

From the host console, deploy a new digest with:

```sh
partifactory deploy --compose deploy/compose.yaml --env-file deploy/.env \
  --image registry.example.com/household/partifactory@sha256:<release-digest>
```

The running image must already be pinned by digest. The command records its previous
digest, pulls the candidate, applies forward migrations, and checks a temporary
container beside the running service. The temporary container publishes no ports.
After it passes readiness, Compose replaces the application service. A two-minute
readiness watch restores the previous digest if the new service fails. The proxy
stays running and serves its maintenance page during the service switch. Inspect
`deploy/last-deploy.json` for the result. After an interrupted command, review that
journal and the running service before removing its `.lock` file and retrying.
Use `--engine podman` or `--host ssh://operator@example.com` for another OCI host;
the compose bind-mount paths must exist on that host. On a managed appliance, use
its application update API with the same digest and readiness checks.

Browser checks require `PF_RENDER_ENABLED=true` and the `render` profile. The renderer
has only an internal network. It can reach the application's TLS content listener and
has no Internet path. Only the public certificate and SPKI are shared through its
read-only volume. Do not attach it to ingress, publish CDP or mount `/data` into it.

The application needs no ZFS or appliance API. Use the host's management API to install
the compose stack on an appliance and take a host snapshot before changing it. Export
application data regularly to a separate host; a local snapshot alone is not a backup.

Release builds use canonical OCI timestamps (`SOURCE_DATE_EPOCH=0`) so source-only
changes can reuse the dependency layers. The image revision label and `RELEASE.json`
record the actual source commit. When reproducing an existing build, supply its
`SOURCE_DATE_EPOCH` explicitly to `scripts/release.sh`; keep that value fixed to
reuse the same BuildKit metadata and dependency cache.
