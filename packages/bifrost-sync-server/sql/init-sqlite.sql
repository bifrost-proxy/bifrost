-- Bifrost Sync Server: SQLite schema
-- Usage: sqlite3 bifrost-sync.db < init-sqlite.sql

CREATE TABLE IF NOT EXISTS bifrost_users (
  id            TEXT PRIMARY KEY,
  user_id       TEXT NOT NULL UNIQUE,
  nickname      TEXT NOT NULL DEFAULT '',
  avatar        TEXT NOT NULL DEFAULT '',
  email         TEXT NOT NULL DEFAULT '',
  password_hash TEXT NOT NULL DEFAULT '',
  token         TEXT,
  create_time   TEXT NOT NULL,
  update_time   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS bifrost_envs (
  id          TEXT PRIMARY KEY,
  user_id     TEXT NOT NULL,
  name        TEXT NOT NULL,
  rule        TEXT NOT NULL DEFAULT '',
  sort_order  INTEGER NOT NULL DEFAULT 0,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL,
  UNIQUE(user_id, name)
);

CREATE TABLE IF NOT EXISTS bifrost_basic_configs (
  id          TEXT PRIMARY KEY,
  user_id     TEXT NOT NULL,
  config_key  TEXT NOT NULL,
  value_json  TEXT NOT NULL DEFAULT '{}',
  hash        TEXT NOT NULL DEFAULT '',
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL,
  UNIQUE(user_id, config_key)
);

CREATE TABLE IF NOT EXISTS bifrost_groups (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL,
  avatar      TEXT DEFAULT '',
  description TEXT DEFAULT '',
  visibility  TEXT DEFAULT 'private',
  created_by  TEXT NOT NULL,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS bifrost_group_members (
  id          TEXT PRIMARY KEY,
  group_id    TEXT NOT NULL,
  user_id     TEXT NOT NULL,
  level       INTEGER DEFAULT 0,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL,
  UNIQUE(group_id, user_id)
);

CREATE TABLE IF NOT EXISTS bifrost_group_settings (
  group_id       TEXT PRIMARY KEY,
  rules_enabled  INTEGER DEFAULT 1,
  visibility     TEXT DEFAULT 'private'
);

CREATE INDEX IF NOT EXISTS idx_bifrost_envs_user_id ON bifrost_envs(user_id);
CREATE INDEX IF NOT EXISTS idx_bifrost_basic_configs_user_id ON bifrost_basic_configs(user_id);
CREATE INDEX IF NOT EXISTS idx_bifrost_users_token  ON bifrost_users(token);
CREATE INDEX IF NOT EXISTS idx_bifrost_group_members_group_id ON bifrost_group_members(group_id);
CREATE INDEX IF NOT EXISTS idx_bifrost_group_members_user_id  ON bifrost_group_members(user_id);

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_pairings (
  id                    TEXT PRIMARY KEY,
  user_id               TEXT NOT NULL,
  client_instance_id    TEXT NOT NULL,
  caller_fingerprint    TEXT NOT NULL DEFAULT '',
  pair_code             TEXT NOT NULL DEFAULT '',
  status                TEXT NOT NULL DEFAULT 'created',
  caller_pubkey         TEXT NOT NULL DEFAULT '',
  caller_ephemeral_pub  TEXT NOT NULL DEFAULT '',
  client_ephemeral_pub  TEXT NOT NULL DEFAULT '',
  caller_info_json      TEXT NOT NULL DEFAULT '{}',
  command_summary_json  TEXT NOT NULL DEFAULT '{}',
  command_json          TEXT NOT NULL DEFAULT '{}',
  relay_token           TEXT NOT NULL DEFAULT '',
  call_id               TEXT NOT NULL DEFAULT '',
  grant_id              TEXT NOT NULL DEFAULT '',
  watch_token_hash      TEXT NOT NULL DEFAULT '',
  claim_token_hash      TEXT NOT NULL DEFAULT '',
  claim_expires_at      TEXT NOT NULL DEFAULT '',
  claimed_at            TEXT NOT NULL DEFAULT '',
  expires_at            TEXT NOT NULL DEFAULT '',
  create_time           TEXT NOT NULL,
  update_time           TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ri_pairings_user_code ON bifrost_remote_invoke_pairings(user_id, pair_code, status);
CREATE INDEX IF NOT EXISTS idx_ri_pairings_client ON bifrost_remote_invoke_pairings(client_instance_id, status);
CREATE INDEX IF NOT EXISTS idx_ri_pairings_claim ON bifrost_remote_invoke_pairings(claim_token_hash);
CREATE INDEX IF NOT EXISTS idx_ri_pairings_watch ON bifrost_remote_invoke_pairings(watch_token_hash);

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_grants (
  id                    TEXT PRIMARY KEY,
  user_id               TEXT NOT NULL,
  client_instance_id    TEXT NOT NULL,
  caller_fingerprint    TEXT NOT NULL DEFAULT '',
  caller_display_name   TEXT NOT NULL DEFAULT '',
  caller_pubkey         TEXT NOT NULL DEFAULT '',
  caller_pubkey_fp      TEXT NOT NULL DEFAULT '',
  caller_ephemeral_pub  TEXT NOT NULL DEFAULT '',
  client_ephemeral_pub  TEXT NOT NULL DEFAULT '',
  grant_mode            TEXT NOT NULL DEFAULT 'once',
  grant_scope           TEXT NOT NULL DEFAULT 'remote_query',
  file_access           TEXT NOT NULL DEFAULT 'none',
  ssh_key_id            TEXT NOT NULL DEFAULT '',
  ssh_key_fingerprint   TEXT NOT NULL DEFAULT '',
  status                TEXT NOT NULL DEFAULT 'active',
  first_authorized_at   TEXT NOT NULL DEFAULT '',
  expires_at            TEXT NOT NULL DEFAULT '',
  session_token_hash    TEXT NOT NULL DEFAULT '',
  session_token_expires_at TEXT NOT NULL DEFAULT '',
  last_nonce_seen       TEXT NOT NULL DEFAULT '',
  revoked_at            TEXT NOT NULL DEFAULT '',
  last_used_at          TEXT NOT NULL DEFAULT '',
  max_calls             INTEGER NOT NULL DEFAULT 1,
  remaining_calls       INTEGER NOT NULL DEFAULT 1,
  created_by            TEXT NOT NULL DEFAULT '',
  update_time           TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ri_grants_reusable ON bifrost_remote_invoke_grants(user_id, client_instance_id, caller_fingerprint, status);
CREATE INDEX IF NOT EXISTS idx_ri_grants_user ON bifrost_remote_invoke_grants(user_id, status, expires_at);
CREATE INDEX IF NOT EXISTS idx_ri_grants_caller_fp ON bifrost_remote_invoke_grants(caller_pubkey_fp);
CREATE INDEX IF NOT EXISTS idx_ri_grants_session ON bifrost_remote_invoke_grants(session_token_hash);

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_calls (
  id                    TEXT PRIMARY KEY,
  user_id               TEXT NOT NULL,
  grant_id              TEXT NOT NULL DEFAULT '',
  pairing_id            TEXT NOT NULL DEFAULT '',
  client_instance_id    TEXT NOT NULL DEFAULT '',
  caller_fingerprint    TEXT NOT NULL DEFAULT '',
  source_ip             TEXT NOT NULL DEFAULT '',
  caller_display_name   TEXT NOT NULL DEFAULT '',
  status                TEXT NOT NULL DEFAULT 'pending',
  command_summary_json  TEXT NOT NULL DEFAULT '{}',
  command_json          TEXT NOT NULL DEFAULT '{}',
  payload_digest        TEXT NOT NULL DEFAULT '',
  stdout_digest         TEXT NOT NULL DEFAULT '',
  stderr_digest         TEXT NOT NULL DEFAULT '',
  exit_code             INTEGER NOT NULL DEFAULT -1,
  started_at            TEXT NOT NULL DEFAULT '',
  ended_at              TEXT NOT NULL DEFAULT '',
  duration_ms           INTEGER NOT NULL DEFAULT 0,
  bytes_in              INTEGER NOT NULL DEFAULT 0,
  bytes_out             INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_ri_calls_user ON bifrost_remote_invoke_calls(user_id, started_at);
CREATE INDEX IF NOT EXISTS idx_ri_calls_grant ON bifrost_remote_invoke_calls(grant_id);
CREATE INDEX IF NOT EXISTS idx_ri_calls_status ON bifrost_remote_invoke_calls(status, started_at);

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_events (
  id                    TEXT PRIMARY KEY,
  call_id               TEXT NOT NULL DEFAULT '',
  event_type            TEXT NOT NULL DEFAULT '',
  seq                   INTEGER NOT NULL DEFAULT 0,
  direction             TEXT NOT NULL DEFAULT '',
  event_summary_json    TEXT NOT NULL DEFAULT '{}',
  create_time           TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ri_events_call ON bifrost_remote_invoke_events(call_id, create_time);

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_clients (
  client_instance_id    TEXT PRIMARY KEY,
  user_id               TEXT NOT NULL DEFAULT '',
  client_name           TEXT NOT NULL DEFAULT '',
  platform              TEXT NOT NULL DEFAULT '',
  bifrost_version       TEXT NOT NULL DEFAULT '',
  client_auth_token     TEXT NOT NULL DEFAULT '',
  client_pubkey_hash    TEXT NOT NULL DEFAULT '',
  token_expires_at      TEXT NOT NULL DEFAULT '',
  last_heartbeat_at     TEXT NOT NULL DEFAULT '',
  create_time           TEXT NOT NULL,
  update_time           TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_nonces (
  caller_pubkey_fp      TEXT NOT NULL,
  nonce                 TEXT NOT NULL,
  seen_at               TEXT NOT NULL,
  PRIMARY KEY (caller_pubkey_fp, nonce)
);
CREATE INDEX IF NOT EXISTS idx_ri_nonces_seen ON bifrost_remote_invoke_nonces(seen_at);

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_ssh_claims (
  claim_token_hash      TEXT PRIMARY KEY,
  grant_id              TEXT NOT NULL,
  client_instance_id    TEXT NOT NULL DEFAULT '',
  caller_pubkey_fp      TEXT NOT NULL DEFAULT '',
  expires_at            TEXT NOT NULL,
  create_time           TEXT NOT NULL,
  claimed_at            TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_ri_ssh_claims_grant ON bifrost_remote_invoke_ssh_claims(grant_id);
CREATE INDEX IF NOT EXISTS idx_ri_ssh_claims_expires ON bifrost_remote_invoke_ssh_claims(expires_at);
