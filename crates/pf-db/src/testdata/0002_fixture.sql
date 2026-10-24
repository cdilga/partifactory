-- Test-only fixture: proves the N-1 -> N path (a schema v1 database, then a 0002 migration).
CREATE TABLE fixture_0002 (id INTEGER PRIMARY KEY, note TEXT NOT NULL) STRICT;
ALTER TABLE parts ADD COLUMN fixture_0002_flag INTEGER NOT NULL DEFAULT 0;
