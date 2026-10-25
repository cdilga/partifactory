-- Rejected writes have no idempotency row. Their deduplicated diagnostic is recorded in a
-- separate writer transaction after the attempted mutation has rolled back.
CREATE TABLE quota_hits (
  principal TEXT NOT NULL,
  kind TEXT NOT NULL,
  day INTEGER NOT NULL,
  at INTEGER NOT NULL,
  limit_value INTEGER NOT NULL,
  PRIMARY KEY (principal, kind, day)
) STRICT, WITHOUT ROWID;

-- Purged parts retain their part/name tombstone, so names remain reserved forever without
-- weakening the names foreign key. Their content has been removed and stops counting quotas.
ALTER TABLE parts ADD COLUMN purged_at INTEGER;
CREATE INDEX parts_purged ON parts(purged_at) WHERE purged_at IS NOT NULL;

-- All insertion paths, including mirror builds and preview jobs, obey the same caps. These
-- fixed markers are translated to the public storage_full error; they contain no content.
CREATE TRIGGER blobs_store_cap BEFORE INSERT ON blobs
WHEN NOT EXISTS (SELECT 1 FROM kv WHERE key='janitor.inventory' AND value='on') AND
 NOT EXISTS (SELECT 1 FROM blobs WHERE hash=NEW.hash) AND
 (SELECT COALESCE(sum(size),0) FROM blobs)+NEW.size >
 COALESCE((SELECT CAST(value AS INTEGER) FROM kv WHERE key='store_cap:blobs'),21474836480)
BEGIN SELECT RAISE(ABORT,'pf_storage_full:blobs'); END;
CREATE TRIGGER shared_blobs_store_cap BEFORE INSERT ON shared_blobs
WHEN (SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN
 (SELECT hash FROM shared_blobs UNION SELECT NEW.hash)) >
 COALESCE((SELECT CAST(value AS INTEGER) FROM kv WHERE key='store_cap:shared_blobs'),5368709120)
BEGIN SELECT RAISE(ABORT,'pf_storage_full:shared_blobs'); END;
CREATE TRIGGER mirror_cache_store_cap BEFORE INSERT ON mirror_cache
WHEN (SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN
 (SELECT hash FROM mirror_cache WHERE url<>NEW.url UNION SELECT NEW.hash)) >
 COALESCE((SELECT CAST(value AS INTEGER) FROM kv WHERE key='store_cap:mirror_cache'),5368709120)
BEGIN SELECT RAISE(ABORT,'pf_storage_full:mirror_cache'); END;
CREATE TRIGGER snapshots_store_cap BEFORE INSERT ON data_snapshots
WHEN (SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN
 (SELECT blob_hash FROM data_snapshots UNION SELECT NEW.blob_hash)) >
 COALESCE((SELECT CAST(value AS INTEGER) FROM kv WHERE key='store_cap:data_snapshots'),2147483648)
BEGIN SELECT RAISE(ABORT,'pf_storage_full:data_snapshots'); END;
CREATE TRIGGER previews_store_cap BEFORE INSERT ON part_previews
WHEN (SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN
 (SELECT poster_hash FROM part_previews WHERE part_id<>NEW.part_id UNION
 SELECT video_hash FROM part_previews WHERE part_id<>NEW.part_id UNION SELECT NEW.poster_hash UNION SELECT NEW.video_hash)) >
 COALESCE((SELECT CAST(value AS INTEGER) FROM kv WHERE key='store_cap:previews'),2147483648)
BEGIN SELECT RAISE(ABORT,'pf_storage_full:previews'); END;
CREATE TRIGGER docs_store_cap BEFORE INSERT ON data_docs
WHEN (SELECT COALESCE(sum(length(body)),0) FROM data_docs WHERE NOT
 (part_id=NEW.part_id AND scope=NEW.scope AND owner=NEW.owner AND collection=NEW.collection AND doc_id=NEW.doc_id))+length(NEW.body) >
 COALESCE((SELECT CAST(value AS INTEGER) FROM kv WHERE key='store_cap:data_docs'),21474836480)
BEGIN SELECT RAISE(ABORT,'pf_storage_full:data_docs'); END;

-- Updates are guarded too: administrative maintenance and future writers cannot bypass the
-- same cap by replacing an existing row. Metadata-only GC updates do not run these triggers.
CREATE TRIGGER blobs_store_cap_update BEFORE UPDATE OF hash,size ON blobs
WHEN NOT EXISTS (SELECT 1 FROM kv WHERE key='janitor.inventory' AND value='on') AND
 (SELECT COALESCE(sum(size),0) FROM blobs WHERE hash<>OLD.hash)+NEW.size >
 COALESCE((SELECT CAST(value AS INTEGER) FROM kv WHERE key='store_cap:blobs'),21474836480)
BEGIN SELECT RAISE(ABORT,'pf_storage_full:blobs'); END;
CREATE TRIGGER shared_blobs_store_cap_update BEFORE UPDATE OF hash ON shared_blobs
WHEN (SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN
 (SELECT hash FROM shared_blobs WHERE hash<>OLD.hash UNION SELECT NEW.hash)) >
 COALESCE((SELECT CAST(value AS INTEGER) FROM kv WHERE key='store_cap:shared_blobs'),5368709120)
BEGIN SELECT RAISE(ABORT,'pf_storage_full:shared_blobs'); END;
CREATE TRIGGER mirror_cache_store_cap_update BEFORE UPDATE OF hash ON mirror_cache
WHEN (SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN
 (SELECT hash FROM mirror_cache WHERE url<>OLD.url UNION SELECT NEW.hash)) >
 COALESCE((SELECT CAST(value AS INTEGER) FROM kv WHERE key='store_cap:mirror_cache'),5368709120)
BEGIN SELECT RAISE(ABORT,'pf_storage_full:mirror_cache'); END;
CREATE TRIGGER snapshots_store_cap_update BEFORE UPDATE OF blob_hash ON data_snapshots
WHEN (SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN
 (SELECT blob_hash FROM data_snapshots WHERE id<>OLD.id UNION SELECT NEW.blob_hash)) >
 COALESCE((SELECT CAST(value AS INTEGER) FROM kv WHERE key='store_cap:data_snapshots'),2147483648)
BEGIN SELECT RAISE(ABORT,'pf_storage_full:data_snapshots'); END;
CREATE TRIGGER previews_store_cap_update BEFORE UPDATE OF poster_hash,video_hash ON part_previews
WHEN (SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN
 (SELECT poster_hash FROM part_previews WHERE part_id<>OLD.part_id UNION
 SELECT video_hash FROM part_previews WHERE part_id<>OLD.part_id UNION SELECT NEW.poster_hash UNION SELECT NEW.video_hash)) >
 COALESCE((SELECT CAST(value AS INTEGER) FROM kv WHERE key='store_cap:previews'),2147483648)
BEGIN SELECT RAISE(ABORT,'pf_storage_full:previews'); END;
CREATE TRIGGER docs_store_cap_update BEFORE UPDATE OF body ON data_docs
WHEN (SELECT COALESCE(sum(length(body)),0) FROM data_docs WHERE NOT
 (part_id=OLD.part_id AND scope=OLD.scope AND owner=OLD.owner AND collection=OLD.collection AND doc_id=OLD.doc_id))+length(NEW.body) >
 COALESCE((SELECT CAST(value AS INTEGER) FROM kv WHERE key='store_cap:data_docs'),21474836480)
BEGIN SELECT RAISE(ABORT,'pf_storage_full:data_docs'); END;

-- Introduced bytes are a connection-lifetime budget, so purging old versions or rotating
-- tokens cannot reset it. Hashes deliberately have no blobs FK: GC may collect the bytes.
CREATE TABLE connection_blob_bytes (
  connection_id TEXT NOT NULL REFERENCES agent_connections(id) ON DELETE CASCADE,
  hash TEXT NOT NULL,
  size INTEGER NOT NULL CHECK(size>=0),
  PRIMARY KEY(connection_id,hash)
) STRICT, WITHOUT ROWID;
