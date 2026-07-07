-- Bifrost Sync Server: MySQL schema
-- Usage: mysql -u root -p bifrost_sync < init-mysql.sql
--
-- CREATE DATABASE IF NOT EXISTS bifrost_sync DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
-- USE bifrost_sync;

CREATE TABLE IF NOT EXISTS bifrost_users (
  id            VARCHAR(32)  NOT NULL PRIMARY KEY,
  user_id       VARCHAR(128) NOT NULL,
  nickname      VARCHAR(255) NOT NULL DEFAULT '',
  avatar        VARCHAR(512) NOT NULL DEFAULT '',
  email         VARCHAR(255) NOT NULL DEFAULT '',
  password_hash VARCHAR(255) NOT NULL DEFAULT '',
  token         VARCHAR(128) DEFAULT NULL,
  create_time   VARCHAR(32)  NOT NULL,
  update_time   VARCHAR(32)  NOT NULL,
  UNIQUE KEY uk_bifrost_user_id (user_id),
  KEY idx_bifrost_users_token (token)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS bifrost_envs (
  id          VARCHAR(32)  NOT NULL PRIMARY KEY,
  user_id     VARCHAR(128) NOT NULL,
  name        VARCHAR(255) NOT NULL,
  rule        LONGTEXT     NOT NULL,
  sort_order  INT          NOT NULL DEFAULT 0,
  create_time VARCHAR(32)  NOT NULL,
  update_time VARCHAR(32)  NOT NULL,
  UNIQUE KEY uk_bifrost_user_env (user_id, name),
  KEY idx_bifrost_envs_user_id (user_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS bifrost_basic_configs (
  id          VARCHAR(192) NOT NULL PRIMARY KEY,
  user_id     VARCHAR(128) NOT NULL,
  config_key  VARCHAR(64)  NOT NULL,
  value_json  LONGTEXT     NOT NULL,
  hash        VARCHAR(128) NOT NULL DEFAULT '',
  create_time VARCHAR(32)  NOT NULL,
  update_time VARCHAR(32)  NOT NULL,
  UNIQUE KEY uk_bifrost_basic_config (user_id, config_key),
  KEY idx_bifrost_basic_configs_user_id (user_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS bifrost_groups (
  id          VARCHAR(32)  NOT NULL PRIMARY KEY,
  name        VARCHAR(255) NOT NULL,
  avatar      VARCHAR(512) DEFAULT '',
  description TEXT         DEFAULT NULL,
  visibility  VARCHAR(32)  DEFAULT 'private',
  created_by  VARCHAR(128) NOT NULL,
  create_time VARCHAR(32)  NOT NULL,
  update_time VARCHAR(32)  NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS bifrost_group_members (
  id          VARCHAR(32)  NOT NULL PRIMARY KEY,
  group_id    VARCHAR(32)  NOT NULL,
  user_id     VARCHAR(128) NOT NULL,
  level       INT          DEFAULT 0,
  create_time VARCHAR(32)  NOT NULL,
  update_time VARCHAR(32)  NOT NULL,
  UNIQUE KEY uk_bifrost_group_member (group_id, user_id),
  KEY idx_bifrost_group_members_group_id (group_id),
  KEY idx_bifrost_group_members_user_id (user_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS bifrost_group_settings (
  group_id       VARCHAR(32) NOT NULL PRIMARY KEY,
  rules_enabled  INT         DEFAULT 1,
  visibility     VARCHAR(32) DEFAULT 'private'
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_pairings (
  id                    VARCHAR(32)  NOT NULL PRIMARY KEY,
  user_id               VARCHAR(128) NOT NULL,
  client_instance_id    VARCHAR(128) NOT NULL,
  caller_fingerprint    VARCHAR(128) NOT NULL DEFAULT '',
  pair_code             VARCHAR(32)  NOT NULL DEFAULT '',
  status                VARCHAR(32)  NOT NULL DEFAULT 'created',
  caller_pubkey         TEXT         NOT NULL,
  caller_ephemeral_pub  TEXT         NOT NULL,
  client_ephemeral_pub  TEXT         NOT NULL,
  caller_info_json      LONGTEXT     NOT NULL,
  command_summary_json  LONGTEXT     NOT NULL,
  command_json          LONGTEXT     NOT NULL,
  relay_token           VARCHAR(128) NOT NULL DEFAULT '',
  call_id               VARCHAR(32)  NOT NULL DEFAULT '',
  grant_id              VARCHAR(32)  NOT NULL DEFAULT '',
  watch_token_hash      VARCHAR(128) NOT NULL DEFAULT '',
  claim_token_hash      VARCHAR(128) NOT NULL DEFAULT '',
  claim_expires_at      VARCHAR(32)  NOT NULL DEFAULT '',
  claimed_at            VARCHAR(32)  NOT NULL DEFAULT '',
  expires_at            VARCHAR(32)  NOT NULL DEFAULT '',
  create_time           VARCHAR(32)  NOT NULL,
  update_time           VARCHAR(32)  NOT NULL,
  KEY idx_ri_pairings_user_code (user_id, pair_code, status),
  KEY idx_ri_pairings_client (client_instance_id, status),
  KEY idx_ri_pairings_claim (claim_token_hash),
  KEY idx_ri_pairings_watch (watch_token_hash)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_grants (
  id                    VARCHAR(32)  NOT NULL PRIMARY KEY,
  user_id               VARCHAR(128) NOT NULL,
  client_instance_id    VARCHAR(128) NOT NULL,
  caller_fingerprint    VARCHAR(128) NOT NULL DEFAULT '',
  caller_display_name   VARCHAR(255) NOT NULL DEFAULT '',
  caller_pubkey         TEXT         NOT NULL,
  caller_pubkey_fp      VARCHAR(128) NOT NULL DEFAULT '',
  caller_ephemeral_pub  TEXT         NOT NULL,
  client_ephemeral_pub  TEXT         NOT NULL,
  grant_mode            VARCHAR(32)  NOT NULL DEFAULT 'once',
  grant_scope           VARCHAR(64)  NOT NULL DEFAULT 'remote_query',
  file_access           VARCHAR(32)  NOT NULL DEFAULT 'none',
  ssh_key_id            VARCHAR(128) NOT NULL DEFAULT '',
  ssh_key_fingerprint   VARCHAR(128) NOT NULL DEFAULT '',
  status                VARCHAR(32)  NOT NULL DEFAULT 'active',
  first_authorized_at   VARCHAR(32)  NOT NULL DEFAULT '',
  expires_at            VARCHAR(32)  NOT NULL DEFAULT '',
  session_token_hash    VARCHAR(128) NOT NULL DEFAULT '',
  session_token_expires_at VARCHAR(32) NOT NULL DEFAULT '',
  last_nonce_seen       VARCHAR(128) NOT NULL DEFAULT '',
  revoked_at            VARCHAR(32)  NOT NULL DEFAULT '',
  last_used_at          VARCHAR(32)  NOT NULL DEFAULT '',
  max_calls             INT          NOT NULL DEFAULT 1,
  remaining_calls       INT          NOT NULL DEFAULT 1,
  created_by            VARCHAR(128) NOT NULL DEFAULT '',
  update_time           VARCHAR(32)  NOT NULL,
  KEY idx_ri_grants_reusable (user_id, client_instance_id, caller_fingerprint, status),
  KEY idx_ri_grants_user (user_id, status, expires_at),
  KEY idx_ri_grants_caller_fp (caller_pubkey_fp),
  KEY idx_ri_grants_session (session_token_hash)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_calls (
  id                    VARCHAR(32)  NOT NULL PRIMARY KEY,
  user_id               VARCHAR(128) NOT NULL,
  grant_id              VARCHAR(32)  NOT NULL DEFAULT '',
  pairing_id            VARCHAR(32)  NOT NULL DEFAULT '',
  client_instance_id    VARCHAR(128) NOT NULL DEFAULT '',
  caller_fingerprint    VARCHAR(128) NOT NULL DEFAULT '',
  source_ip             VARCHAR(64)  NOT NULL DEFAULT '',
  caller_display_name   VARCHAR(255) NOT NULL DEFAULT '',
  status                VARCHAR(32)  NOT NULL DEFAULT 'pending',
  command_summary_json  LONGTEXT     NOT NULL,
  command_json          LONGTEXT     NOT NULL,
  payload_digest        VARCHAR(128) NOT NULL DEFAULT '',
  stdout_digest         VARCHAR(128) NOT NULL DEFAULT '',
  stderr_digest         VARCHAR(128) NOT NULL DEFAULT '',
  exit_code             INT          NOT NULL DEFAULT -1,
  started_at            VARCHAR(32)  NOT NULL DEFAULT '',
  ended_at              VARCHAR(32)  NOT NULL DEFAULT '',
  duration_ms           INT          NOT NULL DEFAULT 0,
  bytes_in              INT          NOT NULL DEFAULT 0,
  bytes_out             INT          NOT NULL DEFAULT 0,
  KEY idx_ri_calls_user (user_id, started_at),
  KEY idx_ri_calls_grant (grant_id),
  KEY idx_ri_calls_status (status, started_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_events (
  id                    VARCHAR(32) NOT NULL PRIMARY KEY,
  call_id               VARCHAR(32) NOT NULL DEFAULT '',
  event_type            VARCHAR(64) NOT NULL DEFAULT '',
  seq                   INT         NOT NULL DEFAULT 0,
  direction             VARCHAR(32) NOT NULL DEFAULT '',
  event_summary_json    LONGTEXT    NOT NULL,
  create_time           VARCHAR(32) NOT NULL,
  KEY idx_ri_events_call (call_id, create_time)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_clients (
  client_instance_id    VARCHAR(128) NOT NULL PRIMARY KEY,
  user_id               VARCHAR(128) NOT NULL DEFAULT '',
  client_name           VARCHAR(255) NOT NULL DEFAULT '',
  platform              VARCHAR(64)  NOT NULL DEFAULT '',
  bifrost_version       VARCHAR(64)  NOT NULL DEFAULT '',
  client_auth_token     VARCHAR(128) NOT NULL DEFAULT '',
  client_pubkey_hash    VARCHAR(128) NOT NULL DEFAULT '',
  token_expires_at      VARCHAR(32)  NOT NULL DEFAULT '',
  last_heartbeat_at     VARCHAR(32)  NOT NULL DEFAULT '',
  create_time           VARCHAR(32)  NOT NULL,
  update_time           VARCHAR(32)  NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_nonces (
  caller_pubkey_fp      VARCHAR(128) NOT NULL,
  nonce                 VARCHAR(128) NOT NULL,
  seen_at               VARCHAR(32)  NOT NULL,
  PRIMARY KEY (caller_pubkey_fp, nonce),
  KEY idx_ri_nonces_seen (seen_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_ssh_claims (
  claim_token_hash      VARCHAR(128) NOT NULL PRIMARY KEY,
  grant_id              VARCHAR(64)  NOT NULL,
  client_instance_id    VARCHAR(128) NOT NULL DEFAULT '',
  caller_pubkey_fp      VARCHAR(128) NOT NULL DEFAULT '',
  expires_at            VARCHAR(32)  NOT NULL,
  create_time           VARCHAR(32)  NOT NULL,
  claimed_at            VARCHAR(32)  NOT NULL DEFAULT '',
  KEY idx_ri_ssh_claims_grant (grant_id),
  KEY idx_ri_ssh_claims_expires (expires_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
