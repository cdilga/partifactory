-- A browser session owns each short-lived consent request. Codes already exist in v1.
CREATE TABLE oauth_requests (
  hash BLOB PRIMARY KEY,
  session_id TEXT NOT NULL,
  request TEXT NOT NULL CHECK (json_valid(request)),
  expires_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
CREATE INDEX oauth_requests_expiry ON oauth_requests(expires_at);
ALTER TABLE oauth_codes ADD COLUMN connection_id TEXT REFERENCES agent_connections(id);
