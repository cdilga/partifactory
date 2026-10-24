-- partifactory schema v1 (SQLite >= 3.45, WAL, foreign_keys=ON, STRICT tables). Everything lives under /data.
-- Conventions: ids are prefixed ULIDs as TEXT ('u_', 'p_', 'sp_', 'c_', ...); times are unix ms INTEGER;
-- secrets (session ids, tokens, codes) are stored only as sha256 BLOB(32); JSON columns are TEXT checked with json_valid.

------------------------------------------------------------------ identity
CREATE TABLE users (
  id            TEXT PRIMARY KEY,                    -- u_...
  auth_source   TEXT NOT NULL CHECK (auth_source IN ('oidc','local')),
  idp_issuer    TEXT,                                -- OIDC issuer URL; NULL for local accounts
  idp_sub       TEXT,                                -- immutable IdP subject; NULL for local accounts
  username      TEXT NOT NULL UNIQUE,
  display_name  TEXT NOT NULL,
  email         TEXT,
  status        TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled','removed')),
  in_audience   INTEGER NOT NULL DEFAULT 1,          -- member of site audience_group (everyone-signed-in)
  created_at    INTEGER NOT NULL,
  last_seen_at  INTEGER,
  synced_at     INTEGER NOT NULL,
  CHECK ((auth_source = 'oidc') = (idp_issuer IS NOT NULL AND idp_sub IS NOT NULL))
) STRICT;
CREATE UNIQUE INDEX users_idp ON users(idp_issuer, idp_sub) WHERE idp_sub IS NOT NULL;
CREATE INDEX users_email ON users(email);

CREATE TABLE groups (
  name       TEXT PRIMARY KEY,                       -- IdP group name, or a local pf-* group
  source     TEXT NOT NULL CHECK (source IN ('claim','idp_api','local')),  -- groups claim, provisioner API, or partifactory's own
  idp_id     TEXT UNIQUE,                            -- set only for source = 'idp_api'
  managed    INTEGER NOT NULL DEFAULT 0 CHECK (managed IN (0,1)),  -- 1 = pf-* group partifactory may edit
  synced_at  INTEGER NOT NULL,
  CHECK (managed = 0 OR name LIKE 'pf-%'),
  CHECK (source <> 'local' OR managed = 1)
) STRICT;

CREATE TABLE group_members (
  group_name TEXT NOT NULL REFERENCES groups(name) ON DELETE CASCADE,
  user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  PRIMARY KEY (group_name, user_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX group_members_user ON group_members(user_id);

CREATE TABLE sessions (                              -- workspace host only: part hosts never set cookies
  id_hash      BLOB PRIMARY KEY,                     -- sha256(__Host-pf_session value)
  user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at   INTEGER NOT NULL,
  last_used_at INTEGER NOT NULL,
  expires_at   INTEGER NOT NULL,                     -- 30 d sliding
  stepup_at    INTEGER,                              -- last admin WebAuthn assertion (valid 10 min)
  idp_refresh_enc BLOB,                              -- OIDC refresh token, encrypted with PF_SESSION_KEY; re-read groups every 15 min
  idp_checked_at  INTEGER,                           -- last successful claims re-read (NULL for local accounts)
  ua_summary   TEXT,
  revoked_at   INTEGER
) STRICT;
CREATE INDEX sessions_user ON sessions(user_id) WHERE revoked_at IS NULL;

CREATE TABLE passkeys (                              -- webauthn-rs passkeys partifactory registered itself
  cred_id     BLOB PRIMARY KEY,
  user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  purpose     TEXT NOT NULL CHECK (purpose IN ('login','stepup')),  -- login: local accounts; stepup: step-up and device approval, for any person who registered one (rp.id = the apex)
  passkey     BLOB NOT NULL,                         -- serialized webauthn-rs Passkey
  label       TEXT NOT NULL,
  created_at  INTEGER NOT NULL,
  last_used_at INTEGER,
  revoked_at  INTEGER                                -- set by a recovery link or by the admin
) STRICT;
CREATE INDEX passkeys_user ON passkeys(user_id) WHERE revoked_at IS NULL;

CREATE TABLE break_glass (
  code_hash  BLOB PRIMARY KEY,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,                       -- +5 min
  used_at    INTEGER
) STRICT;

------------------------------------------------------------------ spaces, parts, names
CREATE TABLE spaces (
  id                 TEXT PRIMARY KEY,               -- sp_...
  slug               TEXT NOT NULL UNIQUE,           -- '~admin', 'family', 'system'
  kind               TEXT NOT NULL CHECK (kind IN ('personal','shared','system')),
  title              TEXT NOT NULL,
  owner_user_id      TEXT REFERENCES users(id),      -- personal: the person; shared: NULL (the admin administers)
  quota_parts        INTEGER,                        -- quota_*: NULL = the §5d default
  quota_bytes        INTEGER,
  quota_versions_day INTEGER,
  created_at         INTEGER NOT NULL,
  CHECK (kind <> 'personal' OR owner_user_id IS NOT NULL)
) STRICT;
CREATE UNIQUE INDEX spaces_one_personal ON spaces(owner_user_id) WHERE kind = 'personal';

CREATE TABLE parts (
  id               TEXT PRIMARY KEY,                 -- p_...
  name             TEXT NOT NULL UNIQUE,             -- current hostname label (without -part)
  space_id         TEXT NOT NULL REFERENCES spaces(id),
  title            TEXT NOT NULL,
  description      TEXT NOT NULL DEFAULT '',
  kind             TEXT NOT NULL CHECK (kind IN ('html','markdown','text','svg','code','tsx','slides','files')),
  head_version_id  TEXT,                             -- newest committed source version (NULL = v0 placeholder)
  live_version_id  TEXT,                             -- newest version with an ok build
  live_build_id    INTEGER,                          -- the live pair is (live_version_id, live_build_id)
  head_n           INTEGER NOT NULL DEFAULT 0,
  owner_user_id    TEXT NOT NULL REFERENCES users(id), -- the person it was made for; shares inward
  steward_conn     TEXT,                             -- agent that receives to_agent comments (P3)
  shared_data_writers TEXT NOT NULL DEFAULT 'viewers' CHECK (shared_data_writers IN ('editors','viewers')),  -- who below edit may write shared data (§3a)
  link_comments    INTEGER NOT NULL DEFAULT 0,       -- opt-in: public-link viewers may comment
  isolation        TEXT NOT NULL DEFAULT 'sandbox' CHECK (isolation IN ('sandbox','unsandboxed')),  -- admin-only flag
  shared_data_cap  INTEGER NOT NULL DEFAULT 5242880,  -- bytes of shared data_docs; guests get quota_exceeded past it
  preview          TEXT CHECK (preview IS NULL OR json_valid(preview)),  -- {video, poster, actions, seconds} (§4c); NULL = automatic poster, no clip
  backend_lifecycle TEXT NOT NULL DEFAULT 'none' CHECK (backend_lifecycle IN ('none','scale_to_zero','always_on')),
  api_prefix       TEXT NOT NULL DEFAULT '/api/' CHECK (api_prefix GLOB '/*/'),  -- backend route under /c/<cap>/
  forked_from      TEXT REFERENCES versions(id),     -- part_create {from}
  expires_at       INTEGER,                          -- part_create {expires_in}: moves to the bin when due
  created_by_user  TEXT NOT NULL REFERENCES users(id),
  created_by_conn  TEXT,                             -- agent_connections.id or NULL
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL,
  deleted_at       INTEGER,                          -- the 30-day bin; purge after 30 d
  CHECK (name GLOB '[a-z0-9]*' AND name NOT GLOB '*[^a-z0-9-]*' AND length(name) BETWEEN 1 AND 58
         AND name NOT LIKE '-%' AND name NOT LIKE '%-' AND instr(name,'--') = 0)
) STRICT;
CREATE INDEX parts_space_updated ON parts(space_id, updated_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX parts_deleted ON parts(deleted_at) WHERE deleted_at IS NOT NULL;
CREATE INDEX parts_expiring ON parts(expires_at) WHERE expires_at IS NOT NULL AND deleted_at IS NULL;

CREATE TABLE names (                                 -- every name ever used; never deleted
  name        TEXT PRIMARY KEY,
  part_id     TEXT NOT NULL REFERENCES parts(id),
  state       TEXT NOT NULL CHECK (state IN ('active','redirect','tombstone')),
  created_at  INTEGER NOT NULL,
  retired_at  INTEGER
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX names_one_active ON names(part_id) WHERE state = 'active';

CREATE TABLE part_tags (
  part_id TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  tag     TEXT NOT NULL,
  PRIMARY KEY (part_id, tag)
) STRICT, WITHOUT ROWID;
CREATE INDEX part_tags_tag ON part_tags(tag);

CREATE TABLE part_notes (                            -- agent notes: versioned separately from content
  part_id    TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  n          INTEGER NOT NULL,
  body       TEXT NOT NULL CHECK (length(body) <= 16384),
  author_user TEXT NOT NULL REFERENCES users(id),
  actor_conn TEXT,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (part_id, n)
) STRICT;

------------------------------------------------------------------ versions, builds, blobs
CREATE TABLE versions (
  id            TEXT PRIMARY KEY,                    -- sha256 hex of canonical manifest
  part_id       TEXT NOT NULL REFERENCES parts(id),
  n             INTEGER NOT NULL,
  parent_id     TEXT REFERENCES versions(id),
  manifest_hash TEXT NOT NULL,                       -- = id; manifest bytes live in blobs
  entry         TEXT NOT NULL,
  author_user   TEXT NOT NULL REFERENCES users(id),
  actor_conn    TEXT,                                -- NULL = the person directly
  via           TEXT NOT NULL CHECK (via IN ('api','mcp','cli','editor','self_publish','restore')),
  message       TEXT,
  rebased_from  TEXT,
  restored_from TEXT,
  live_gate     TEXT NOT NULL DEFAULT 'always' CHECK (live_gate IN ('always','after_check')),  -- part_create/part_edit {live}
  created_at    INTEGER NOT NULL,
  UNIQUE (part_id, n)
) STRICT;
CREATE INDEX versions_actor_day ON versions(actor_conn, created_at);  -- quotas: versions/day per token

CREATE TABLE version_files (                         -- denormalized manifest for diff, GC, FTS, serving
  version_id TEXT NOT NULL REFERENCES versions(id) ON DELETE CASCADE,
  path       TEXT NOT NULL,
  blob_hash  TEXT NOT NULL REFERENCES blobs(hash),
  size       INTEGER NOT NULL,
  mime       TEXT NOT NULL,
  PRIMARY KEY (version_id, path)
) STRICT, WITHOUT ROWID;
CREATE INDEX version_files_blob ON version_files(blob_hash);

CREATE TABLE builds (
  id               INTEGER PRIMARY KEY,
  version_id       TEXT NOT NULL REFERENCES versions(id) ON DELETE CASCADE,
  cache_key        TEXT NOT NULL,                    -- sha256(src manifest || toolchain || libset || mirror resolution)
  toolchain        TEXT NOT NULL,                    -- pinned oxc/rolldown/builder version id
  target           TEXT NOT NULL DEFAULT 'frontend' CHECK (target IN ('frontend','backend')),
  method           TEXT NOT NULL CHECK (method IN ('server_build','prebuilt_files','prebuilt_image','uploaded_image')),  -- for Insights
  status           TEXT NOT NULL CHECK (status IN ('queued','running','ok','failed','timeout','superseded')),
  output_hash      TEXT REFERENCES blobs(hash),      -- build manifest blob (frontend) or image manifest (backend)
  log_hash         TEXT REFERENCES blobs(hash),      -- build log; last 20 builds per part kept (janitor)
  sbom_hash        TEXT REFERENCES blobs(hash),      -- backend: the image SBOM from the runner's pinned scanner (§3h); capped per build
  warnings         TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(warnings)),
  errors           TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(errors)),
  gate_check       TEXT CHECK (gate_check IN ('pending','passed','failed','skipped')),  -- live_gate = after_check only
  queued_at        INTEGER NOT NULL,
  started_at       INTEGER,
  finished_at      INTEGER,
  UNIQUE (version_id, cache_key)
) STRICT;
CREATE INDEX builds_queue ON builds(status, queued_at) WHERE status IN ('queued','running');
CREATE INDEX builds_cache ON builds(cache_key) WHERE status = 'ok';

CREATE TABLE build_files (                           -- what the part host may serve for a build
  build_id  INTEGER NOT NULL REFERENCES builds(id) ON DELETE CASCADE,
  path      TEXT NOT NULL,
  blob_hash TEXT NOT NULL REFERENCES blobs(hash),
  br_hash   TEXT REFERENCES blobs(hash),
  content_type TEXT NOT NULL,
  PRIMARY KEY (build_id, path)
) STRICT, WITHOUT ROWID;
CREATE INDEX build_files_blob ON build_files(blob_hash);

CREATE TABLE blobs (
  hash        TEXT PRIMARY KEY,                      -- sha256 hex of plaintext
  size        INTEGER NOT NULL,
  created_at  INTEGER NOT NULL,
  gc_mark     INTEGER NOT NULL DEFAULT 0,
  unref_since INTEGER                                -- set by sweep; delete after 7 d grace
) STRICT, WITHOUT ROWID;
CREATE INDEX blobs_unref ON blobs(unref_since) WHERE unref_since IS NOT NULL;

CREATE TABLE shared_blobs (                          -- class is per reference: only these are served by blobs-part
  hash       TEXT PRIMARY KEY REFERENCES blobs(hash),
  source     TEXT NOT NULL CHECK (source IN ('seed','mirror','runtime')),
  label      TEXT NOT NULL,                          -- 'react@18.3.1/umd/react.production.min.js'
  added_at   INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE mirror_cache (
  url          TEXT PRIMARY KEY,
  resolved_url TEXT NOT NULL,
  hash         TEXT NOT NULL REFERENCES blobs(hash),
  content_type TEXT NOT NULL,
  integrity    TEXT,
  pinned       INTEGER NOT NULL CHECK (pinned IN (0,1)),  -- exact version: cache forever
  fetched_at   INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE build_deps (                            -- each build's library list, a small SBOM (§3h); deleted with its build
  id         INTEGER PRIMARY KEY,
  build_id   INTEGER NOT NULL REFERENCES builds(id) ON DELETE CASCADE,
  ecosystem  TEXT NOT NULL,                          -- OSV ecosystem ('npm', 'crates.io', 'PyPI', 'Go', 'Debian', 'Alpine', 'Ubuntu', ...) or 'unknown'
  name       TEXT NOT NULL,                          -- package name; for 'unknown', the URL as written
  version    TEXT NOT NULL DEFAULT '',               -- '' when it couldn't be parsed
  source_url TEXT,                                   -- the mirrored CDN URL; NULL for the library set and image SBOM entries
  blob_hash  TEXT REFERENCES blobs(hash)             -- the mirrored file; NULL for image SBOM entries
) STRICT;
CREATE INDEX build_deps_build ON build_deps(build_id);
CREATE INDEX build_deps_pkg ON build_deps(ecosystem, name, version);

CREATE TABLE advisories (                            -- a known vulnerability in a build's libraries or image (§3h); deleted with its build
  id          INTEGER PRIMARY KEY,
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  build_id    INTEGER NOT NULL REFERENCES builds(id) ON DELETE CASCADE,
  ecosystem   TEXT NOT NULL,
  package     TEXT NOT NULL,
  version     TEXT NOT NULL,
  osv_id      TEXT NOT NULL,                         -- 'GHSA-...', 'CVE-...', 'RUSTSEC-...'
  severity    TEXT NOT NULL CHECK (severity IN ('critical','high','medium','low','unknown')),  -- OSV's; the context is applied when shown
  fixed_in    TEXT,                                  -- first fixed version, when OSV knows one (auto-update needs it)
  first_seen  INTEGER NOT NULL,
  state       TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','fixed','ignored')),
  ignored_by  TEXT,                                  -- user:<id> | conn:<id> (parts:write); audited
  reason      TEXT CHECK (reason IS NULL OR length(reason) <= 500),
  CHECK ((state = 'ignored') = (ignored_by IS NOT NULL AND reason IS NOT NULL)),
  UNIQUE (build_id, ecosystem, package, version, osv_id)
) STRICT;
CREATE INDEX advisories_open ON advisories(part_id) WHERE state = 'open';

CREATE TABLE drafts (                                -- workspace editor drafts, per person per part
  user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  part_id    TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  base_version_id TEXT NOT NULL REFERENCES versions(id),
  files      TEXT NOT NULL CHECK (json_valid(files)),  -- {path: text}; previewed via a cap with a d claim
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, part_id)
) STRICT, WITHOUT ROWID;

------------------------------------------------------------------ access
CREATE TABLE grants (
  id          INTEGER PRIMARY KEY,
  target_kind TEXT NOT NULL CHECK (target_kind IN ('part','space')),
  target_id   TEXT NOT NULL,
  subject     TEXT NOT NULL CHECK (subject GLOB 'user:*' OR subject GLOB 'group:*' OR subject = 'everyone-signed-in'),
  role        INTEGER NOT NULL CHECK (role BETWEEN 0 AND 3),  -- view, comment, edit, manage
  expires_at  INTEGER,
  created_by  TEXT NOT NULL REFERENCES users(id),   -- the person
  created_by_conn TEXT,                              -- set when an agent made it (share:household): labelled, undoable
  created_at  INTEGER NOT NULL,
  UNIQUE (target_kind, target_id, subject)
) STRICT;
CREATE INDEX grants_subject ON grants(subject);
CREATE INDEX grants_expiry ON grants(expires_at) WHERE expires_at IS NOT NULL;

-- Public links: admin-only, made once the part has a live version, no expiry by default. There are no agent-minted preview links.
CREATE TABLE public_links (
  id            TEXT PRIMARY KEY,                    -- l_...; also the cap subject link:<id> and minter
  token_hash    BLOB NOT NULL UNIQUE,                -- sha256 of the 128-bit /l/<token>
  part_id       TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  role          INTEGER NOT NULL DEFAULT 0 CHECK (role IN (0,1)),  -- view; comment only with parts.link_comments
  mode          TEXT NOT NULL DEFAULT 'view' CHECK (mode IN ('view','contribute')),  -- contribute: guests write shared data
  frozen_at     INTEGER,                             -- last Freeze (contribute -> view); bumps minter:link:<id>
  preview       TEXT NOT NULL DEFAULT 'video' CHECK (preview IN ('video','image','card')),  -- the unfurl preview (§4c); video falls back to image, then card
  created_by    TEXT NOT NULL REFERENCES users(id),  -- always the admin
  created_at    INTEGER NOT NULL,
  expires_at    INTEGER,                             -- NULL = never (the default)
  revoked_at    INTEGER,
  open_count    INTEGER NOT NULL DEFAULT 0,          -- counted on POST /l/<token>/go
  last_opened_at INTEGER,
  request_cap   INTEGER                              -- optional per-link request limit
) STRICT;
CREATE INDEX public_links_part ON public_links(part_id) WHERE revoked_at IS NULL;


-- Capabilities are stateless (in the path, never in a cookie). Only their revocation state is stored.
CREATE TABLE epochs (
  key   TEXT PRIMARY KEY,                            -- 'part:<id>' | 'minter:sess:<id>' | 'minter:conn:<id>' | 'minter:link:<id>' | 'global' | 'boot'
  epoch INTEGER NOT NULL DEFAULT 0
) STRICT, WITHOUT ROWID;

CREATE TABLE access_asks (                          -- 'Ask for access' from a signed-in person; person-to-person, no agent rows
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  note        TEXT CHECK (note IS NULL OR length(note) <= 280),
  created_at  INTEGER NOT NULL,
  closed_at   INTEGER,                               -- Add (an ordinary grant) or Dismiss
  closed_as   TEXT CHECK (closed_as IN ('added','dismissed')),
  PRIMARY KEY (part_id, user_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX access_asks_open ON access_asks(part_id) WHERE closed_at IS NULL;

CREATE TABLE invites (
  id              TEXT PRIMARY KEY,                  -- i_...; handed out as /i/<id>, which records opens
  idp_ref         TEXT,                              -- provisioner only: IdP invitation (unlimited until expiry) or recovery id; NULL for native invites
  kind            TEXT NOT NULL CHECK (kind IN ('single','multi','email','recovery')),
  max_uses        INTEGER NOT NULL DEFAULT 1,        -- counted here; the IdP has no use count
  uses            INTEGER NOT NULL DEFAULT 0,
  email           TEXT,
  target_user_id  TEXT REFERENCES users(id),         -- recovery
  land_part_id    TEXT REFERENCES parts(id),
  grant_role      INTEGER CHECK (grant_role BETWEEN 0 AND 3),
  groups          TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(groups)),  -- pf-* groups, added by API after enrolment
  status          TEXT NOT NULL CHECK (status IN ('sent','opened','enrolled','expired','revoked')),
  created_at      INTEGER NOT NULL,
  opened_at       INTEGER,
  expires_at      INTEGER NOT NULL,
  revoked_at      INTEGER,
  CHECK (uses <= max_uses)
) STRICT;
CREATE INDEX invites_open ON invites(expires_at) WHERE status IN ('sent','opened');

CREATE TABLE invite_enrolments (
  invite_id TEXT NOT NULL REFERENCES invites(id),
  user_id   TEXT NOT NULL REFERENCES users(id),
  at        INTEGER NOT NULL,
  PRIMARY KEY (invite_id, user_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE audit (
  id          INTEGER PRIMARY KEY,
  at          INTEGER NOT NULL,
  user_id     TEXT,                                  -- the person (author)
  actor       TEXT,                                  -- 'conn:<id>' | 'break-glass' | 'system' | NULL
  action      TEXT NOT NULL,                         -- 'grant.set', 'link.revoke', 'invite.create', ...
  target_kind TEXT NOT NULL,
  target_id   TEXT NOT NULL,
  before      TEXT CHECK (before IS NULL OR json_valid(before)),
  after       TEXT CHECK (after IS NULL OR json_valid(after)),
  request_id  TEXT
) STRICT;
CREATE INDEX audit_target ON audit(target_kind, target_id, at DESC);

------------------------------------------------------------------ agents (OAuth AS + personal tokens)
CREATE TABLE oauth_clients (
  id            TEXT PRIMARY KEY,                    -- client_id (CIMD URL, DCR id, manual id, or 'part-cli')
  kind          TEXT NOT NULL CHECK (kind IN ('cimd','dcr','manual','builtin')),
  client_name   TEXT NOT NULL,
  metadata      TEXT NOT NULL CHECK (json_valid(metadata)),
  redirect_uris TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(redirect_uris)),
  created_at    INTEGER NOT NULL,
  consented_at  INTEGER,                             -- NULL + older than 1 h => purged
  secret_hash   BLOB,                                -- manual confidential clients only
  reg_ip_hash   BLOB
) STRICT;
CREATE INDEX oauth_clients_unconsented ON oauth_clients(created_at) WHERE consented_at IS NULL;

CREATE TABLE agent_connections (                     -- one row per (agent, person); the 'act' and minter for agents
  id          TEXT PRIMARY KEY,                      -- c_...
  kind        TEXT NOT NULL CHECK (kind IN ('oauth_code','device','pat')),
  principal_kind TEXT NOT NULL DEFAULT 'person' CHECK (principal_kind IN ('person','service')),
  client_id   TEXT REFERENCES oauth_clients(id),     -- NULL for pat
  user_id     TEXT REFERENCES users(id),             -- the person it acts for; NULL for a service identity
  agent_name  TEXT NOT NULL,                         -- e.g. 'Claude Code', 'Hermes'; shown as '<agent> for <person>'
  software    TEXT,                                  -- self-reported, e.g. 'hermes/1.4' (unverified)
  scopes      TEXT NOT NULL CHECK (json_valid(scopes)),
  spaces      TEXT NOT NULL CHECK (json_valid(spaces)),
  expires_at  INTEGER NOT NULL,                      -- consented lifetime
  created_at  INTEGER NOT NULL,
  last_used_at INTEGER,
  revoked_at  INTEGER,
  quota_versions_day INTEGER,                         -- NULL = the §5d default
  CHECK ((principal_kind = 'person') = (user_id IS NOT NULL))
) STRICT;
CREATE INDEX agent_connections_user ON agent_connections(user_id) WHERE revoked_at IS NULL;

CREATE TABLE oauth_tokens (
  hash        BLOB PRIMARY KEY,                      -- sha256 of opaque token
  kind        TEXT NOT NULL CHECK (kind IN ('access','refresh','pat')),
  prefix      TEXT NOT NULL,                         -- 'pf_at_' | 'pf_rt_' | 'pf_pat_' (secret scanning, logs)
  connection_id TEXT NOT NULL REFERENCES agent_connections(id) ON DELETE CASCADE,
  family_id   TEXT,                                  -- refresh rotation family
  resource    TEXT NOT NULL,                         -- RFC 8707 audience: always the workspace origin, e.g. https://parts.example.com/
  issued_at   INTEGER NOT NULL,
  expires_at  INTEGER NOT NULL,
  used_at     INTEGER,                               -- refresh: set on rotation; reuse => revoke family
  revoked_at  INTEGER
) STRICT, WITHOUT ROWID;
CREATE INDEX oauth_tokens_family ON oauth_tokens(family_id) WHERE family_id IS NOT NULL;
CREATE INDEX oauth_tokens_conn ON oauth_tokens(connection_id);

CREATE TABLE oauth_codes (
  hash          BLOB PRIMARY KEY,
  client_id     TEXT NOT NULL REFERENCES oauth_clients(id),
  user_id       TEXT NOT NULL REFERENCES users(id),
  redirect_uri  TEXT NOT NULL,
  code_challenge TEXT NOT NULL,
  scopes        TEXT NOT NULL,
  spaces        TEXT NOT NULL,
  resource      TEXT NOT NULL,
  lifetime_ms   INTEGER NOT NULL,
  expires_at    INTEGER NOT NULL,                    -- 60 s
  used_at       INTEGER
) STRICT, WITHOUT ROWID;

CREATE TABLE device_codes (
  device_hash   BLOB PRIMARY KEY,
  user_code     TEXT NOT NULL UNIQUE,                -- XXXX-XXXX
  client_id     TEXT NOT NULL REFERENCES oauth_clients(id),
  agent_name    TEXT NOT NULL,
  software      TEXT,
  req_scopes    TEXT NOT NULL,
  req_geo       TEXT,                                -- coarse country/network from proxy headers or GeoIP; NULL if unknown
  status        TEXT NOT NULL CHECK (status IN ('pending','approved','denied','expired','consumed')),
  connection_id TEXT REFERENCES agent_connections(id),
  interval_s    INTEGER NOT NULL DEFAULT 5,
  last_poll_at  INTEGER,
  created_at    INTEGER NOT NULL,
  expires_at    INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE idempotency (                           -- every write's request_id; a row is stored only when the write commits
  principal  TEXT NOT NULL,                          -- 'user:<id>' or 'conn:<id>'
  request_id TEXT NOT NULL,
  operation  TEXT NOT NULL,
  response   TEXT NOT NULL CHECK (json_valid(response)),
  created_at INTEGER NOT NULL,                       -- purge after 24 h
  PRIMARY KEY (principal, request_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE leases (                                -- part_edit {claim}; advisory
  id         TEXT PRIMARY KEY,
  part_id    TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  scope      TEXT NOT NULL DEFAULT 'part' CHECK (scope IN ('part','file','block')),
  scope_ref  TEXT NOT NULL DEFAULT '',               -- path or block id
  holder_user TEXT REFERENCES users(id),
  holder_conn TEXT,
  holder_label TEXT NOT NULL,
  expires_at INTEGER NOT NULL,                       -- <= created_at + 10 min
  created_at INTEGER NOT NULL,
  UNIQUE (part_id, scope, scope_ref)
) STRICT;

------------------------------------------------------------------ collaboration + runtime state
CREATE TABLE comments (
  id          TEXT PRIMARY KEY,
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  thread_id   TEXT NOT NULL,                         -- = id of root
  parent_id   TEXT REFERENCES comments(id),
  version_id  TEXT REFERENCES versions(id),
  anchor      TEXT CHECK (anchor IS NULL OR json_valid(anchor)),  -- {block?|file,lines?, quote}
  body        TEXT NOT NULL CHECK (length(body) <= 16384),
  provenance  TEXT NOT NULL CHECK (provenance IN ('person','agent','link_viewer')),
  author_user TEXT REFERENCES users(id),
  author_conn TEXT,
  author_link TEXT REFERENCES public_links(id),
  to_agent    INTEGER NOT NULL DEFAULT 0,
  agent_state TEXT CHECK (agent_state IN ('open','claimed','answered')),  -- inbox (P3); resolved = resolved_at
  resolved_at INTEGER,
  resolved_by TEXT,
  created_at  INTEGER NOT NULL,
  deleted_at  INTEGER
) STRICT;
CREATE INDEX comments_part ON comments(part_id, created_at);
CREATE INDEX comments_to_agent ON comments(part_id) WHERE to_agent = 1 AND resolved_at IS NULL;

CREATE TABLE data_docs (                             -- window.storage + claude.use("db"); survives versions
  part_id    TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  scope      TEXT NOT NULL CHECK (scope IN ('shared','personal')),
  owner      TEXT NOT NULL DEFAULT '',               -- user id for personal, '' for shared
  collection TEXT NOT NULL,                          -- '__storage' for window.storage
  doc_id     TEXT NOT NULL,
  body       BLOB NOT NULL CHECK (length(body) <= 262144),
  rev        INTEGER NOT NULL,                       -- CAS
  updated_at INTEGER NOT NULL,
  updated_by TEXT NOT NULL,                          -- user:<id> | conn:<id> | link:<id> | guest:<link>:<g>
  CHECK ((scope = 'shared') = (owner = '')),
  PRIMARY KEY (part_id, scope, owner, collection, doc_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE data_leases (
  part_id    TEXT NOT NULL,
  collection TEXT NOT NULL,
  doc_id     TEXT NOT NULL,
  holder     TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  PRIMARY KEY (part_id, collection, doc_id)
) STRICT, WITHOUT ROWID;

-- Per-part data snapshots: ON by default (daily, keep 7), for undoing a bad write (not disaster recovery).
-- A part with no row uses the default; a row exists only once someone changes it.
CREATE TABLE data_snapshot_policies (
  part_id     TEXT PRIMARY KEY REFERENCES parts(id) ON DELETE CASCADE,
  mode        TEXT NOT NULL DEFAULT 'daily' CHECK (mode IN ('off','daily','hourly')),  -- daily keep 7, hourly keep 48
  keep        INTEGER NOT NULL DEFAULT 7,
  expire_days INTEGER NOT NULL DEFAULT 90,           -- stop after N days unopened; keep the last one
  set_by      TEXT NOT NULL,                         -- user:<id> | conn:<id> (agents may request at create)
  set_at      INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE data_snapshots (
  id         INTEGER PRIMARY KEY,
  part_id    TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  taken_at   INTEGER NOT NULL,
  reason     TEXT NOT NULL DEFAULT 'scheduled' CHECK (reason IN ('scheduled','contribute','before_restore','before_reset')),  -- only 'scheduled' counts against keep
  blob_hash  TEXT NOT NULL REFERENCES blobs(hash),   -- canonical JSON of the part's shared data_docs
  docs       INTEGER NOT NULL,
  bytes      INTEGER NOT NULL
) STRICT;
CREATE INDEX data_snapshots_part ON data_snapshots(part_id, taken_at DESC);

CREATE TABLE assets (                                -- viewer uploads (P3)
  id          TEXT PRIMARY KEY,                      -- a_...; served at /_a/<id>
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  blob_hash   TEXT NOT NULL REFERENCES blobs(hash),
  mime        TEXT NOT NULL,
  size        INTEGER NOT NULL CHECK (size <= 26214400),
  name        TEXT NOT NULL,
  uploaded_by TEXT NOT NULL,                         -- user:<id> | conn:<id>
  created_at  INTEGER NOT NULL,
  deleted_at  INTEGER
) STRICT;
CREATE INDEX assets_part ON assets(part_id) WHERE deleted_at IS NULL;

CREATE TABLE runtime_errors (
  id          INTEGER PRIMARY KEY,
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  version_id  TEXT NOT NULL,
  source      TEXT NOT NULL DEFAULT 'viewer' CHECK (source IN ('viewer','render')),
  fingerprint TEXT NOT NULL,                         -- hash(kind, message, top frame)
  kind        TEXT NOT NULL CHECK (kind IN ('error','unhandledrejection','blocked_load','csp')),
  message     TEXT NOT NULL,
  stack       TEXT,
  url         TEXT,
  count       INTEGER NOT NULL DEFAULT 1,
  first_at    INTEGER NOT NULL,
  last_at     INTEGER NOT NULL,
  UNIQUE (part_id, version_id, source, fingerprint)
) STRICT;
CREATE INDEX runtime_errors_part ON runtime_errors(part_id, last_at DESC);
CREATE TRIGGER runtime_errors_keep1000 AFTER INSERT ON runtime_errors BEGIN   -- cap 1,000 per part; the janitor drops > 30 d
  DELETE FROM runtime_errors WHERE part_id = NEW.part_id AND id NOT IN
    (SELECT id FROM runtime_errors WHERE part_id = NEW.part_id ORDER BY last_at DESC LIMIT 1000);
END;

------------------------------------------------------------------ backends (P2a; only the runner touches the Engine API)
CREATE TABLE backend_images (
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  digest      TEXT NOT NULL,                         -- 'sha256:...' as the runtime reports it
  source      TEXT NOT NULL CHECK (source IN ('server_build','prebuilt_image','uploaded_image')),
  ref         TEXT,                                  -- registry ref as written (prebuilt_image)
  tar_hash    TEXT REFERENCES blobs(hash),           -- uploaded_image: the docker-save tarball
  size        INTEGER NOT NULL,
  created_at  INTEGER NOT NULL,
  pruned_at   INTEGER,                               -- removed from the runtime; kept: live + previous 2 versions
  PRIMARY KEY (part_id, digest)
) STRICT, WITHOUT ROWID;

CREATE TABLE base_image_checks (                     -- the runner's nightly re-resolve of each base-image tag a backend build used (§3h)
  build_id     INTEGER NOT NULL REFERENCES builds(id) ON DELETE CASCADE,
  ref          TEXT NOT NULL,                        -- the tag as written in FROM or image:, e.g. 'postgres:16-alpine'
  digest       TEXT NOT NULL,                        -- what the build used
  newer_digest TEXT,                                 -- set when the tag now resolves elsewhere: "a newer digest exists"
  checked_at   INTEGER NOT NULL,
  PRIMARY KEY (build_id, ref)
) STRICT, WITHOUT ROWID;

CREATE TABLE container_sets (                        -- one per (part, version) that has been started
  id              INTEGER PRIMARY KEY,
  part_id         TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  version_id      TEXT NOT NULL REFERENCES versions(id),
  state           TEXT NOT NULL CHECK (state IN ('starting','healthy','unhealthy','draining','stopped','failed')),
  started_at      INTEGER,
  healthy_at      INTEGER,
  last_request_at INTEGER,                           -- scale-to-zero idle timer (15 min)
  stopped_at      INTEGER,
  cold_starts     INTEGER NOT NULL DEFAULT 0,
  UNIQUE (part_id, version_id)
) STRICT;
CREATE INDEX container_sets_running ON container_sets(state) WHERE state IN ('starting','healthy','draining');

CREATE TABLE containers (
  id           TEXT PRIMARY KEY,                     -- the runtime's container id
  set_id       INTEGER NOT NULL REFERENCES container_sets(id) ON DELETE CASCADE,
  service      TEXT NOT NULL,
  role         TEXT NOT NULL CHECK (role IN ('main','worker','cron','dependency')),
  image_digest TEXT NOT NULL,
  state        TEXT NOT NULL CHECK (state IN ('created','running','exited','dead')),
  restarts     INTEGER NOT NULL DEFAULT 0,
  started_at   INTEGER,
  exited_at    INTEGER,
  exit_code    INTEGER
) STRICT;
CREATE INDEX containers_set ON containers(set_id);

CREATE TABLE part_volumes (                          -- /data/parts/<id>/vol/<service>; one named volume per service
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  service     TEXT NOT NULL,
  quota_bytes INTEGER NOT NULL DEFAULT 1073741824,
  used_bytes  INTEGER NOT NULL DEFAULT 0,
  measured_at INTEGER,
  PRIMARY KEY (part_id, service)
) STRICT, WITHOUT ROWID;

CREATE TABLE part_secrets (                          -- write-only: set or replaced by agents (backends:deploy) or people (edit); never read back
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  name        TEXT NOT NULL CHECK (name GLOB '[A-Z_]*' AND name NOT GLOB '*[^A-Z0-9_]*' AND length(name) <= 64),
  value_enc   BLOB NOT NULL,                         -- encrypted with PF_SESSION_KEY; no op returns it
  inject_as   TEXT NOT NULL DEFAULT 'env' CHECK (inject_as IN ('env','file')),
  set_by_user TEXT NOT NULL REFERENCES users(id),    -- the person (author)
  set_by_conn TEXT,                                  -- agent connection, if an agent set it
  set_at      INTEGER NOT NULL,
  PRIMARY KEY (part_id, name)
) STRICT, WITHOUT ROWID;

CREATE TABLE lan_allow (                             -- per-part exceptions to the LAN block; set by a person (admin UI), never an agent
  id          INTEGER PRIMARY KEY,
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  target      TEXT NOT NULL,                         -- 'host:port' or a CIDR
  note        TEXT NOT NULL,                         -- 'may reach the home-automation server'
  added_by    TEXT NOT NULL REFERENCES users(id),
  added_at    INTEGER NOT NULL,
  revoked_at  INTEGER
) STRICT;
CREATE INDEX lan_allow_part ON lan_allow(part_id) WHERE revoked_at IS NULL;

------------------------------------------------------------------ events, webhooks, jobs
CREATE TABLE events (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,      -- cursor for part_watch / SSE Last-Event-ID; kept 30 d
  at         INTEGER NOT NULL,
  part_id    TEXT,
  space_id   TEXT,
  type       TEXT NOT NULL,                          -- part.created, version.committed, build.finished, part.live_changed, comment.added, data.changed, backend.started, part.advisory, ...
  actor      TEXT,                                   -- user:<id> | conn:<id> | guest:<link>:<g> | system
  payload    TEXT NOT NULL CHECK (json_valid(payload))
) STRICT;
CREATE INDEX events_part ON events(part_id, seq);
CREATE INDEX events_space ON events(space_id, seq);

CREATE TABLE webhook_deliveries (                    -- targets live in site config, never in DB
  id         INTEGER PRIMARY KEY,
  target     TEXT NOT NULL,                          -- site-config target name
  event_seq  INTEGER NOT NULL REFERENCES events(seq),
  attempts   INTEGER NOT NULL DEFAULT 0,
  next_at    INTEGER NOT NULL,
  status     TEXT NOT NULL CHECK (status IN ('pending','ok','dead')),
  last_error TEXT,
  UNIQUE (target, event_seq)
) STRICT;
CREATE INDEX webhook_due ON webhook_deliveries(next_at) WHERE status = 'pending';

CREATE TABLE render_jobs (                           -- tier (b) sidecar queue; concurrency 1, coalesced per part
  id          INTEGER PRIMARY KEY,
  part_id     TEXT NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
  version_id  TEXT NOT NULL,
  viewport    TEXT NOT NULL CHECK (viewport IN ('mobile','desktop')),
  purpose     TEXT NOT NULL CHECK (purpose IN ('check','gate','thumbnail','og','preview')),  -- gate: the live = after_check check; preview: the og:video recording (§4c)
  requested_by TEXT,                                 -- conn:<id> for part_check
  request     TEXT CHECK (request IS NULL OR json_valid(request)),  -- {wait_ms, actions, record, seconds}
  status      TEXT NOT NULL CHECK (status IN ('queued','running','ok','failed','superseded')),
  result      TEXT CHECK (result IS NULL OR json_valid(result)),  -- {screenshot_hash, video_hash?, poster_hash?, blank, errors, console, failed_requests, actions, timing}; GET /api/v1/screenshots/<id>
  queued_at   INTEGER NOT NULL,
  finished_at INTEGER
) STRICT;
CREATE INDEX render_jobs_queue ON render_jobs(status, queued_at) WHERE status IN ('queued','running');

CREATE TABLE part_previews (                         -- §4c: the newest preview per part; replaced rows leave their blobs to GC
  part_id     TEXT PRIMARY KEY REFERENCES parts(id) ON DELETE CASCADE,
  version_id  TEXT NOT NULL REFERENCES versions(id),
  source      TEXT NOT NULL CHECK (source IN ('render','agent','recorded')),  -- render: Slice 1's latest check screenshot, poster only
  poster_hash TEXT,                                  -- part blob; also the gallery thumbnail
  video_hash  TEXT,                                  -- part blob, MP4 (§4c profile)
  width       INTEGER,
  height      INTEGER,
  duration_ms INTEGER,
  bytes       INTEGER,
  state       TEXT NOT NULL CHECK (state IN ('ready','pending','failed')),
  error       TEXT,                                  -- why the last recording or probe failed; the last good clip stays
  updated_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE kv (                                    -- singletons: directory poll cursor + staleness, provisioner tier, last export, last drill, alert state, disk budget state
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) STRICT, WITHOUT ROWID;

------------------------------------------------------------------ search
CREATE VIRTUAL TABLE parts_fts USING fts5(
  part_id UNINDEXED, title, description, tags, body, notes, comments,   -- body = extracted readable text, not code
  tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3'
);
