PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_version (
  version INTEGER PRIMARY KEY,
  applied_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS memories (
  id TEXT PRIMARY KEY,
  scope_kind TEXT NOT NULL,
  scope_type TEXT NOT NULL,
  scope_value TEXT,
  kind TEXT NOT NULL,
  content TEXT NOT NULL,
  source_json TEXT NOT NULL,
  tags_json TEXT NOT NULL,
  pinned INTEGER NOT NULL DEFAULT 0,
  confidence REAL NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  last_used_at INTEGER,
  use_count INTEGER NOT NULL DEFAULT 0,
  expires_at INTEGER,
  dedupe_hash TEXT NOT NULL,
  deleted_at INTEGER
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_memories_scope_dedupe
  ON memories(scope_kind, dedupe_hash)
  WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_memories_scope_last_used
  ON memories(scope_kind, last_used_at DESC, updated_at DESC)
  WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_memories_kind
  ON memories(kind)
  WHERE deleted_at IS NULL;

CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
  id UNINDEXED,
  content,
  tags,
  tokenize = 'unicode61'
);

CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
  INSERT INTO memories_fts(rowid, id, content, tags)
  VALUES (new.rowid, new.id, new.content, new.tags_json);
END;

CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
  DELETE FROM memories_fts WHERE rowid = old.rowid;
END;

CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
  DELETE FROM memories_fts WHERE rowid = old.rowid;
  INSERT INTO memories_fts(rowid, id, content, tags)
  VALUES (new.rowid, new.id, new.content, new.tags_json);
END;

INSERT OR IGNORE INTO schema_version(version, applied_at)
VALUES (1, CAST(strftime('%s','now') AS INTEGER));
