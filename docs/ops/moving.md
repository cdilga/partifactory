# Moving to another host

Keep the public hostnames unchanged. Prepare the new OCI host according to
[the host contract](../host.md), using the digest-pinned release image and the
offline site config and secret files. Set the deploy target (`deploy.host` in
site config or the Docker context used for compose) to the new host.

1. Either stop the old server and copy all of `/data` (a consistent host
   snapshot is also suitable), or make a portable export and import it on the
   stopped new host. Follow [backup and recovery](backup.md) for encrypted
   archives, offline identities, sufficient separate verification scratch space
   and regrant. Do not copy a live SQLite database
   without its coordinated snapshot/WAL state.
2. Deploy using `deploy/compose.yaml`, mount the offline site config and supply
   `_FILE` secrets. Wait for `/readyz` on the new host; this checks local
   operation and does not require the IdP to be reachable. Open a restored
   part through the real TLS proxy and verify a fresh grant succeeds.
3. Repoint ingress: update the proxy upstream, or run the Cloudflare adapter's
   provision operation to repoint the tunnel. Rotate the tunnel connector
   token and delete the old host's connections, so an old connector cannot
   receive new traffic. Verify through the ingress as well as directly.
4. Run `partifactory doctor` for ingress, backup layers, export directory and
   disk headroom. Check Health and the logs. Re-enable host snapshots on the
   new host: snapshots on the previous host do not move with an export.
5. Take an off-host export from the new host and run the second-daemon drill.
   Keep the previous host stopped and recoverable until this validation is
   complete, then retire only the old deployment's resources.

Deployments bump the boot epoch, so an open browser tab may silently re-grant
once. Full exports restore retained local identities, sessions and OAuth token
records; `--include-secrets` adds part/backend secret values and requires
encryption. Parts-only exports require access to be granted again. The external
IdP must be restored independently to preserve its original issuer/subject
pairs, or the admin must review identity regrant proposals.
