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
}

export interface RemoteCommand {
  command: string;
  args_json?: string;
}

export interface RemoteInvokePairing {
  id: string;
  user_id: string;
  client_instance_id: string;
  caller_fingerprint: string;
  pair_code: string;
  status: PairingStatus;
  caller_pubkey: string;
  client_ephemeral_pub: string;
  caller_info_json: string;
  command_summary_json: string;
  command_json: string;
  relay_token: string;
  call_id: string;
  grant_id: string;
  expires_at: string;
  create_time: string;
  update_time: string;
}

export interface RemoteInvokeGrant {
  id: string;
  user_id: string;
  client_instance_id: string;
  caller_fingerprint: string;
  grant_mode: GrantMode;
  grant_scope: string;
  status: GrantStatus;
  first_authorized_at: string;
  expires_at: string;
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
