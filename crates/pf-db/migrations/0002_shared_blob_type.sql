-- Rewritten library bytes have a different hash from their mirror_cache raw input.
-- Retain MIME metadata on the served reference rather than guessing from a URL suffix.
ALTER TABLE shared_blobs ADD COLUMN content_type TEXT NOT NULL DEFAULT 'application/octet-stream';
UPDATE shared_blobs SET content_type = coalesce(
  (SELECT content_type FROM mirror_cache WHERE hash = shared_blobs.hash LIMIT 1),
  CASE WHEN label LIKE '%.js' THEN 'text/javascript; charset=utf-8'
       WHEN label LIKE '%.css' THEN 'text/css; charset=utf-8'
       ELSE 'application/octet-stream' END
);
