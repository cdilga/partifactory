# Backup and recovery

Keep three independent layers: host snapshots of `/data`, the server's hourly
SQLite copies under `/data/backup`, and portable exports under `/data/exports`.
Hourly copies are useful for a database accident; an export is the portable
recovery unit for losing the host. Per-part data snapshots undo bad writes and
do not replace disaster recovery. Check the Health page for the age and outcome
of each layer, disk pressure and the latest monthly drill.

Nightly exports are enabled by default and retain seven archives. Configure
`[backup] export_schedule`, `export_dir`, `export_keep` and `age_recipient` in the
offline site config. Disabling exports is explicit and appears on Health.
Copy exports off the host. Keep the age identity, site config, admin pin and
secret files in the offline password-manager copy. The server needs only the
age public recipient. The IdP's database has its own backup procedure: a
partifactory export cannot recover an external IdP.

Create a portable export with `partifactory export --output latest.tar.zst`.
Use `--encrypt RECIPIENT` for age encryption. `--include-secrets` requires
encryption; ordinary full exports strip part/backend secret values and retain
browser sessions and OAuth token records. A
`--parts-only` export preserves content/history and audience-name proposals,
but access must be granted again and agents reconnected after importing it.
Keep the most recent complete export when pruning: a failed new export must
never remove the previous recovery point.

To recover, stop the destination server and run
`partifactory import latest.tar.zst` with `PF_DATA_DIR`, `PF_SITE_TOML` and the
usual `_FILE` secret variables set. For an encrypted archive add
`--identity /offline/age-identity.txt`. Import verifies all archive entries
before replacing destination data, migrates forward and bumps the boot epoch.
Old capabilities therefore fail even if their signed bytes were in a browser;
a newly granted capability works. A corrupt archive exits 3. Replacing an
existing data directory requires the explicit `--force` option.

Import first expands and verifies the complete archive in temporary storage.
Set `TMPDIR` to a private scratch directory with at least the uncompressed
archive size available, separate from the destination `/data`. For an offline
container restore, mount a writable scratch volume at `/tmp`; the published
compose file's 64 MiB `/tmp` tmpfs is for the running server and is too small
for larger recovery archives. The drill provisions and removes this scratch
volume automatically. Never stage an unverified archive inside destination
data: corruption must leave the destination untouched.

If identities changed, generate `partifactory import regrant --plan`, review
the proposed username/email and audience-name matches, then run
`partifactory import regrant --apply reviewed-plan.json` while the server is
stopped. The configured immutable admin pin authorizes this offline operation.
Do not apply a proposal merely because the display names look familiar.

## Monthly drill

Run the drill from a laptop using downloaded release assets and the offline
config, rather than relying on the CI host that may be the failed machine.
Use an empty second Docker daemon/context. The script refuses the source
daemon and refuses a target with images, containers or volumes. It transfers
files into named volumes, so an SSH context needs no remote bind-mount paths.
The target must be able to fetch the pinned Caddy image. Publish the selected
HTTPS port to the laptop; a nested daemon also needs an outer port mapping.

```sh
scripts/drill.sh --host recovery-host \
  --assets /offline/release-assets --export /offline/latest.tar.zst \
  --site /offline/site.toml --secrets-dir /offline/secrets \
  --tls-cert /offline/cert.pem --tls-key /offline/key.pem \
  --ca-cert /offline/ca.pem --address 127.0.0.1 --https-port 19443 \
  --report-dir /offline/drill-results \
  --e2e-command /offline/run-recovery-e2e
```

The secret directory contains `session_key`, `cap_hmac_key`, `metrics_token`
and `idp_client_secret` (an empty file when OIDC is disabled). Add
`--identity-file` for each age identity needed to decrypt the export. If the
source daemon is offline, pass its previously recorded `docker info` ID with
`--source-daemon-id`; independence is still checked against the target ID.

The external e2e command receives `PF_BASE_URL`, `PF_CA`, `PF_CONNECT_TO`
(`address:port`), `PF_DRILL_EVIDENCE`, `PF_DRILL_IMAGE`, `PF_DRILL_HOST` and
`PF_DRILL_COMPOSE`. It must perform the deployment's recovery checks through
Caddy with TLS verification, including opening a restored part. Use a client
connection override or CONNECT proxy to preserve the original hostname and
SNI; changing `/etc/hosts` is unnecessary. Run the complete applicable security
suite as part of this command. Missing prerequisites must stay unobserved and
must not count as passing tests. Any nonzero test exit fails the drill.

The script verifies release checksums, OCI blobs and CLI/image binary equality,
imports with the shipped CLI, waits for readiness and runs the external tests.
The whole restore target is under 900 seconds. It removes only its exact
containers, volumes, networks and images, including on failure; it never prunes
the target daemon. Evidence diagnostics can contain private deployment data:
the evidence directory is mode 700 and its log mode 600. Keep it private.

Each run writes `<timestamp>.json` containing `at` (Unix milliseconds), `ok`,
`rto_s` and sanitized `failures`. Copy that JSON into the active server's
`/data/backup/drills/` (or write there directly with `--report-dir` where it is
mounted locally). The background sampler updates the Health drill row,
`pf_last_drill_ok` and `pf_last_drill_age_s`. A failing test or recovery at or
beyond 900 seconds reports failure. A deliberate failing-test run should show
`ok: false` and metric 0 before the next successful drill restores metric 1.
