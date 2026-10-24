-- /data/insights.db: analytics and observability (§5g). A separate file with its own writer.
-- Never part content: ids, op names, argument keys, sizes, statuses and timings only (a CI fuzz test checks it).
CREATE TABLE events_raw (                            -- kept 30 d, then only in rollups_daily
  id          INTEGER PRIMARY KEY,
  at          INTEGER NOT NULL,
  kind        TEXT NOT NULL,                         -- 'view','op','rt','grant_decision','build','backend','link_open', ...
  surface     TEXT CHECK (surface IN ('mcp','http','cli','ui','part','rt','backend')),
  op          TEXT,                                  -- registry op name or route class
  part_id     TEXT,
  actor       TEXT,                                  -- user:<id> | conn:<id> | guest:<link>:<g> | link:<id> | NULL
  viewer_key  BLOB,                                  -- public viewers and guests: hash(truncated IP, UA, rotating salt)
  status      TEXT,                                  -- 'ok' or an error code
  latency_us  INTEGER,
  bytes_in    INTEGER,
  bytes_out   INTEGER,
  request_id  TEXT,
  attrs       TEXT CHECK (attrs IS NULL OR json_valid(attrs))  -- {arg_keys, build_method, kind, ...}; never values
) STRICT;
CREATE INDEX events_raw_at ON events_raw(at);
CREATE INDEX events_raw_part ON events_raw(part_id, at) WHERE part_id IS NOT NULL;
CREATE INDEX events_raw_actor ON events_raw(actor, at) WHERE actor IS NOT NULL;

CREATE TABLE rollups_daily (                         -- kept indefinitely; small
  day     INTEGER NOT NULL,                          -- days since epoch, UTC
  metric  TEXT NOT NULL,                             -- 'views','uniques','ops','errors','builds_ok','cold_starts','guest_writes', ...
  dim     TEXT NOT NULL DEFAULT '',                  -- 'part:<id>' | 'conn:<id>' | 'op:<name>' | 'method:server_build' | ''
  value   INTEGER NOT NULL,
  PRIMARY KEY (day, metric, dim)
) STRICT, WITHOUT ROWID;

CREATE TABLE traces (                                -- tail-sampled: every error, every slow request, 1 % of the rest; 7 d, 2 GiB
  trace_id     BLOB PRIMARY KEY,
  request_id   TEXT NOT NULL,
  started_at   INTEGER NOT NULL,
  duration_us  INTEGER NOT NULL,
  root         TEXT NOT NULL,                        -- route class or op
  status       TEXT NOT NULL,
  part_id      TEXT,
  actor        TEXT,
  kept_because TEXT NOT NULL CHECK (kept_because IN ('error','slow','sample'))
) STRICT, WITHOUT ROWID;
CREATE INDEX traces_at ON traces(started_at);
CREATE INDEX traces_request ON traces(request_id);

CREATE TABLE spans (
  trace_id    BLOB NOT NULL REFERENCES traces(trace_id) ON DELETE CASCADE,
  span_id     INTEGER NOT NULL,
  parent_id   INTEGER,
  name        TEXT NOT NULL,                         -- 'policy.decide','sqlite.write','build.child','runner.start', ...
  start_us    INTEGER NOT NULL,                      -- offset from the trace start
  duration_us INTEGER NOT NULL,
  fields      TEXT CHECK (fields IS NULL OR json_valid(fields)),
  PRIMARY KEY (trace_id, span_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE log_segments (                          -- index over /data/logs/**/*.jsonl.zst; the lines live in the files
  path      TEXT PRIMARY KEY,                        -- 'app/2026-10-23T14-0001.jsonl.zst' | 'parts/<id>/backend-0042.jsonl.zst'
  stream    TEXT NOT NULL CHECK (stream IN ('app','backend')),
  part_id   TEXT,
  first_at  INTEGER NOT NULL,
  last_at   INTEGER NOT NULL,
  bytes     INTEGER NOT NULL,
  lines     INTEGER NOT NULL,
  max_level TEXT CHECK (max_level IN ('trace','debug','info','warn','error')),
  CHECK ((stream = 'backend') = (part_id IS NOT NULL))
) STRICT, WITHOUT ROWID;
CREATE INDEX log_segments_stream ON log_segments(stream, part_id, first_at);

CREATE TABLE janitor_runs (                          -- what each retention job freed (§5h)
  id          INTEGER PRIMARY KEY,
  at          INTEGER NOT NULL,
  store       TEXT NOT NULL,                         -- a row of the §5h retention table
  action      TEXT NOT NULL CHECK (action IN ('prune','rotate','gc','checkpoint','vacuum','image_prune','cache_prune')),
  items       INTEGER NOT NULL,
  freed_bytes INTEGER NOT NULL,
  size_after  INTEGER NOT NULL,
  cap_bytes   INTEGER,
  error       TEXT
) STRICT;
CREATE INDEX janitor_runs_store ON janitor_runs(store, at);
