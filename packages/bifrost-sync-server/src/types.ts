export interface Env {
  id: string;
  user_id: string;
  name: string;
  rule: string;
  sort_order: number;
  create_time: string;
  update_time: string;
}

export interface User {
  id: string;
  user_id: string;
  nickname: string;
  avatar: string;
  email: string;
  password_hash: string;
  token: string;
  create_time: string;
  update_time: string;
}

export interface CreateEnvReq {
  user_id: string;
  name: string;
  rule?: string;
  sort_order?: number;
}

export interface UpdateEnvReq {
  id?: string;
  user_id?: string;
  name?: string;
  rule?: string;
  sort_order?: number;
}

export interface SearchEnvQuery {
  user_id?: string | string[];
  keyword?: string;
  offset?: number;
  limit?: number;
}

export interface ApiResponse<T = unknown> {
  code: number;
  message: string;
  data?: T;
}

export interface MysqlConfig {
  host: string;
  port: number;
  user: string;
  password: string;
  database: string;
}

export interface OAuth2Config {
  client_id: string;
  client_secret: string;
  authorize_url: string;
  token_url: string;
  userinfo_url: string;
  scopes: string[];
  redirect_uri?: string;
  user_id_field?: string;
  nickname_field?: string;
  email_field?: string;
  avatar_field?: string;
}

export interface AuthConfig {
  mode: 'password' | 'oauth2';
  oauth2?: OAuth2Config;
}

export interface StorageConfig {
  type: 'sqlite' | 'mysql';
  sqlite?: { data_dir: string };
  mysql?: MysqlConfig;
}

export interface ServerConfig {
  port: number;
  host: string;
  trust_forwarded_for?: boolean;
  rate_limit_per_ip?: number;
  auth_rate_limit_per_ip?: number;
}

export interface SyncServerConfig {
  server: ServerConfig;
  storage: StorageConfig;
  auth: AuthConfig;
  remote_invoke?: RemoteInvokeConfig;
}

export interface Group {
  id: string;
  name: string;
  avatar: string;
  description: string;
  visibility: string;
  created_by: string;
  create_time: string;
  update_time: string;
}

export interface GroupMember {
  id: string;
  group_id: string;
  user_id: string;
  level: number;
  nickname: string;
  avatar: string;
  email: string;
  create_time: string;
  update_time: string;
}

export interface GroupSetting {
  group_id: string;
  rules_enabled: number;
  visibility: string;
}

export interface CreateGroupReq {
  name: string;
  avatar?: string;
  description?: string;
  visibility?: string;
}

export interface UpdateGroupReq {
  name?: string;
  avatar?: string;
  description?: string;
  visibility?: string;
}

export interface SearchGroupQuery {
  keyword?: string;
  user_id?: string;
  offset?: number;
  limit?: number;
}

export interface InviteGroupReq {
  group_id: string;
  user_id: string[];
  level?: number;
}

export interface UpdateGroupSettingReq {
  rules_enabled?: boolean;
  visibility?: string;
}

export type GrantMode = 'once' | '30m' | '1h' | '1d' | 'permanent';
export type GrantStatus = 'active' | 'expired' | 'revoked' | 'consumed' | 'removed';
export type CallStatus = 'pending' | 'authorized' | 'key_exchanged' | 'streaming' | 'completed' | 'failed' | 'cancelled' | 'timeout';
export type PairingStatus = 'created' | 'code_verified' | 'pending_approval' | 'approved' | 'rejected' | 'expired' | 'cancelled';
export type RemoteInvokeGrantScope = 'remote_query' | 'remote_shell_exec' | 'remote_shell_interactive' | 'remote_power_mgmt';
export type FileAccessScope = 'none' | 'read' | 'read_write';
export type RemoteCommandKind = 'query.readonly' | 'shell.exec' | 'file' | 'power.mgmt';

export interface RemoteInvokeConfig {
  enabled: boolean;
  sse_keepalive_ms: number;
  pair_code_ttl_secs: number;
  max_active_calls_per_client: number;
  max_grants_per_client: number;
  retention_days: number;
  max_records: number;
  max_sse_connections_per_client: number;
  max_sse_connections_per_ip: number;
  pair_rate_limit_per_ip: number;
  pair_rate_limit_global_per_client: number;
  ssh_grant_max_calls?: number;
}

export interface CallerInfo {
  fingerprint: string;
  display_name?: string;
  user_agent?: string;
  source_ip?: string;
  platform?: string;
}

export interface CommandSummary {
  command_preview: string;
  masked_args_json?: string;
  payload_digest?: string;
  payload_size?: number;
  command_kind?: RemoteCommandKind;
  encrypted_payload_present?: boolean;
  pty_enabled?: boolean;
  timeout_hint_ms?: number;
  viewer_resume_ttl_ms?: number;
  retention_ttl_ms?: number;
  relay_token_ttl_ms?: number;
}

export interface RemoteCommand {
  command: string;
  args_json?: string;
  kind?: RemoteCommandKind;
  policy_id?: string;
  exec_mode?: string;
  argv_json?: string;
  shell?: string | null;
  command_text?: string | null;
  cwd?: string;
  env_json?: string;
  stdin_mode?: string;
  timeout_ms?: number;
  pty_json?: string;
}

export interface RemoteInvokePairing {
  id: string;
  user_id: string;
  client_instance_id: string;
  caller_fingerprint: string;
  pair_code: string;
  status: PairingStatus;
  caller_pubkey: string;
  caller_ephemeral_pub?: string;
  caller_ephemeral_sig?: string;
  client_ephemeral_pub?: string;
  caller_info_json: string;
  command_summary_json: string;
  command_json: string;
  relay_token: string;
  call_id: string;
  grant_id: string;
  watch_token_hash?: string;
  claim_token_hash?: string;
  claim_expires_at?: string;
  claimed_at?: string;
  expires_at: string;
  create_time: string;
  update_time: string;
}

export interface RemoteInvokeGrant {
  id: string;
  user_id: string;
  client_instance_id: string;
  caller_fingerprint: string;
  caller_display_name: string;
  caller_pubkey?: string;
  caller_pubkey_fp?: string;
  caller_ephemeral_pub?: string;
  client_ephemeral_pub?: string;
  grant_mode: GrantMode;
  grant_scope: RemoteInvokeGrantScope;
  file_access?: FileAccessScope;
  ssh_key_id?: string;
  ssh_key_fingerprint?: string;
  status: GrantStatus;
  first_authorized_at: string;
  expires_at: string;
  session_token_hash?: string;
  session_token_expires_at?: string;
  last_nonce_seen?: string;
  revoked_at?: string;
  last_used_at: string;
  max_calls: number;
  remaining_calls: number;
  created_by: string;
  update_time: string;
}

export interface RemoteInvokeCall {
  id: string;
  user_id: string;
  grant_id: string;
  pairing_id: string;
  client_instance_id: string;
  caller_fingerprint: string;
  source_ip: string;
  caller_display_name: string;
  status: CallStatus;
  command_summary_json: string;
  command_json: string;
  payload_digest: string;
  stdout_digest: string;
  stderr_digest: string;
  exit_code: number;
  started_at: string;
  ended_at: string;
  duration_ms: number;
  bytes_in: number;
  bytes_out: number;
}

export interface RemoteInvokeEvent {
  id: string;
  call_id: string;
  event_type: string;
  seq: number;
  direction: string;
  event_summary_json: string;
  create_time: string;
}

export interface RemoteInvokeClientRecord {
  client_instance_id: string;
  user_id: string;
  client_name: string;
  platform: string;
  bifrost_version: string;
  client_auth_token: string;
  client_pubkey_hash: string;
  token_expires_at: string;
  last_heartbeat_at: string;
  create_time: string;
  update_time: string;
}

export interface RemoteInvokeSshClaim {
  claim_token_hash: string;
  grant_id: string;
  client_instance_id: string;
  caller_pubkey_fp: string;
  expires_at: string;
  create_time: string;
  claimed_at: string;
}
