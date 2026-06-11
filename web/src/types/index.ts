export interface RuleFile {
  name: string;
  enabled: boolean;
  sort_order: number;
  rule_count: number;
  created_at: string;
  updated_at: string;
}

export interface RuleSyncInfo {
  status: 'local_only' | 'synced' | 'modified';
  last_synced_at?: string | null;
  remote_id?: string | null;
  remote_updated_at?: string | null;
}

export interface RuleFileDetail {
  name: string;
  content: string;
  enabled: boolean;
  sort_order: number;
  created_at: string;
  updated_at: string;
  sync: RuleSyncInfo;
}

export interface MatchedRule {
  pattern: string;
  protocol: string;
  value: string;
  rule_name?: string;
  raw?: string;
  line?: number;
}

export interface RequestTiming {
  dns_ms?: number;
  connect_ms?: number;
  tls_ms?: number;
  send_ms?: number;
  wait_ms?: number;
  first_byte_ms?: number;
  receive_ms?: number;
  total_ms: number;
}

export interface SocketStatus {
  is_open: boolean;
  send_count: number;
  receive_count: number;
  send_bytes: number;
  receive_bytes: number;
  frame_count: number;
  close_code?: number;
  close_reason?: string;
}

export interface TrafficSummary {
  id: string;
  sequence: number;
  timestamp: number;
  method: string;
  url: string;
  status: number;
  content_type: string | null;
  request_content_type?: string | null;
  request_size: number;
  response_size: number;
  duration_ms: number;
  listener_port?: number;
  host: string;
  path: string;
  protocol: string;
  client_ip: string;
  client_app?: string;
  client_pid?: number;
  has_rule_hit: boolean;
  matched_rule_count: number;
  matched_protocols: string[];
  is_websocket?: boolean;
  is_sse?: boolean;
  is_h3?: boolean;
  is_tunnel?: boolean;
  frame_count?: number;
  socket_status?: SocketStatus | null;
  start_time: string;
  end_time?: string | null;

  _displayProtocol?: string;
  _methodColor?: string;
  _statusColor?: string;
  _statusDotColor?: string;
  _displaySize?: string;
  _contentTypeShort?: string;
  _clientDisplay?: string;
  _clientTooltip?: string;
}

export interface ScriptLogEntry {
  timestamp: number;
  level: 'debug' | 'info' | 'warn' | 'error';
  message: string;
  args?: unknown[];
}

export interface ScriptExecutionResult {
  script_name: string;
  script_type: 'request' | 'response' | 'decode' | 'parser';
  success: boolean;
  error?: string;
  duration_ms: number;
  logs: ScriptLogEntry[];
  decode_output?: {
    data: string;
    code: string;
    msg: string;
  };
}

export interface TrafficRecord extends TrafficSummary {
  request_headers: [string, string][] | null;
  response_headers: [string, string][] | null;
  request_body: string | null;
  response_body: string | null;
  request_body_ref?: BodyRef | null;
  response_body_ref?: BodyRef | null;
  raw_request_body_ref?: BodyRef | null;
  raw_response_body_ref?: BodyRef | null;
  matched_rules: MatchedRule[] | null;
  request_content_type: string | null;
  timing?: RequestTiming | null;
  last_frame_id?: number;
  actual_url?: string | null;
  actual_host?: string | null;
  original_request_headers?: [string, string][] | null;
  original_response_headers?: [string, string][] | null;
  req_script_results?: ScriptExecutionResult[] | null;
  res_script_results?: ScriptExecutionResult[] | null;
  decode_req_script_results?: ScriptExecutionResult[] | null;
  decode_res_script_results?: ScriptExecutionResult[] | null;
}

export type BodyRef =
  | { Inline: { data: string } }
  | { File: { path: string; size: number } }
  | { FileRange: { path: string; offset: number; size: number } };

export type FrameDirection = 'send' | 'receive';

export type FrameType = 'text' | 'binary' | 'ping' | 'pong' | 'close' | 'continuation' | 'sse';

export interface WebSocketFrame {
  frame_id: number;
  timestamp: number;
  direction: FrameDirection;
  frame_type: FrameType;
  payload_size: number;
  payload_preview?: string;
  is_masked: boolean;
  is_fin: boolean;
}

export interface FramesResponse {
  frames: WebSocketFrame[];
  socket_status: SocketStatus | null;
  last_frame_id: number;
  has_more: boolean;
  is_monitored: boolean;
}

export interface TrafficListResponse {
  total: number;
  offset: number;
  limit: number;
  records: TrafficSummary[];
}

export interface TrafficUpdatesResponse {
  new_records: TrafficSummary[];
  updated_records: TrafficSummary[];
  has_more: boolean;
  server_total: number;
}

export interface TrafficUpdatesResponseCompact {
  new_records: TrafficSummaryCompact[];
  updated_records: TrafficSummaryCompact[];
  has_more: boolean;
  server_total: number;
  server_sequence: number;
}

export interface TrafficSummaryCompact {
  id: string;
  seq: number;
  ts: number;
  m: string;
  h: string;
  p: string;
  s: number;
  ct?: string | null;
  req_ct?: string | null;
  req_sz: number;
  res_sz: number;
  dur: number;
  lp?: number;
  proto: string;
  cip: string;
  capp?: string | null;
  cpid?: number | null;
  flags: number;
  fc: number;
  ss?: SocketStatus | null;
  st: string;
  et?: string | null;
  rc: number;
  rp: string[];
}

export const TrafficFlags = {
  IS_TUNNEL: 1 << 0,
  IS_WEBSOCKET: 1 << 1,
  IS_SSE: 1 << 2,
  IS_H3: 1 << 3,
  HAS_RULE_HIT: 1 << 4,
  IS_REPLAY: 1 << 5,
} as const;

export type RuleMode = 'enabled' | 'selected' | 'none';

export interface RuleConfig {
  mode: RuleMode;
  selected_rules?: string[];
}

export type BodyType = 'none' | 'form-data' | 'x-www-form-urlencoded' | 'raw' | 'binary';
export type RawType = 'json' | 'xml' | 'text' | 'javascript' | 'html';

export type ConnectionType = 'http' | 'sse' | 'websocket';

export interface SSEEvent {
  id?: string;
  event?: string;
  data: string;
  timestamp: number;
}

export interface WebSocketMessage {
  id: string;
  direction: 'send' | 'receive';
  type: 'text' | 'binary' | 'ping' | 'pong' | 'close';
  data: string;
  timestamp: number;
}

export interface StreamingConnection {
  id: string;
  type: ConnectionType;
  status: 'connecting' | 'connected' | 'disconnected' | 'error';
  url: string;
  startedAt: number;
  endedAt?: number;
  error?: string;
  trafficId?: string;
  appliedUrl?: string;
  appliedRules?: MatchedRule[];
}

export interface ReplayKeyValueItem {
  id: string;
  key: string;
  value: string;
  enabled: boolean;
  description?: string;
}

export interface ReplayBody {
  type: BodyType;
  raw_type?: RawType;
  content?: string;
  form_data?: ReplayKeyValueItem[];
  binary_file?: string;
}

export interface ReplayGroup {
  id: string;
  name: string;
  parent_id?: string;
  sort_order: number;
  created_at: number;
  updated_at: number;
}

export type RequestType = 'http' | 'sse' | 'websocket';

export interface ReplayRequest {
  id: string;
  group_id?: string;
  name?: string;
  request_type: RequestType;
  method: string;
  url: string;
  headers: ReplayKeyValueItem[];
  body?: ReplayBody;
  is_saved: boolean;
  sort_order: number;
  created_at: number;
  updated_at: number;
}

export interface ReplayRequestSummary {
  id: string;
  group_id?: string;
  name?: string;
  request_type: RequestType;
  method: string;
  url: string;
  is_saved: boolean;
  created_at: number;
  updated_at: number;
}

export interface ReplayHistory {
  id: string;
  request_id?: string;
  traffic_id: string;
  method: string;
  url: string;
  status: number;
  duration_ms: number;
  executed_at: number;
  rule_config?: RuleConfig;
}

export interface ReplayExecuteRequest {
  request: {
    method: string;
    url: string;
    headers: [string, string][];
    body?: string;
  };
  rule_config: RuleConfig;
  request_id?: string;
  timeout_ms?: number;
}

export const DEFAULT_TIMEOUT_MS = 10_000;

export interface ReplayExecuteResponse {
  traffic_id: string;
  status: number;
  headers: [string, string][];
  body?: string;
  duration_ms: number;
  applied_rules: MatchedRule[];
  error?: string;
}

export interface ReplayDbStats {
  request_count: number;
  history_count: number;
  group_count: number;
  db_size: number;
  db_path: string;
}

export const REPLAY_LIMITS = {
  MAX_REQUESTS: 1000,
  MAX_HISTORY: 2000,
  MAX_CONCURRENT: 100,
} as const;

export interface TrafficDeltaData {
  inserts: TrafficSummaryCompact[];
  updates: TrafficSummaryCompact[];
  has_more: boolean;
  server_total: number;
  server_sequence: number;
}

export interface TrafficQueryRequest {
  cursor?: number;
  limit?: number;
  direction?: 'forward' | 'backward';
  method?: string;
  status?: number;
  status_min?: number;
  status_max?: number;
  protocol?: string;
  has_rule_hit?: boolean;
  is_websocket?: boolean;
  is_sse?: boolean;
  is_h3?: boolean;
  is_tunnel?: boolean;
  host_contains?: string;
  url_contains?: string;
  path_contains?: string;
  client_app?: string;
  client_ip?: string;
  listener_port?: number;
  content_type?: string;
}

export interface TrafficQueryResponse {
  records: TrafficSummaryCompact[];
  next_cursor: number | null;
  prev_cursor: number | null;
  has_more: boolean;
  total: number;
  server_sequence: number;
}

export interface TrafficUpdatesFilter extends TrafficFilter {
  after_id?: string;
  after_seq?: number;
  pending_ids?: string;
}

export interface TrafficFilter {
  method?: string;
  status?: number;
  status_min?: number;
  status_max?: number;
  url_contains?: string;
  host?: string;
  content_type?: string;
  limit?: number;
  offset?: number;
  has_rule_hit?: boolean;
  protocol?: string;
  request_content_type?: string;
  domain?: string;
  path_contains?: string;
  header_contains?: string;
  client_ip?: string;
  client_app?: string;
  listener_port?: number;
  is_h3?: boolean;
  is_websocket?: boolean;
  is_sse?: boolean;
}

export interface ToolbarFilters {
  rule: string[];
  protocol: string[];
  type: string[];
  status: string[];
  imported: string[];
}

export interface FilterCondition {
  id: string;
  field: string;
  operator: string;
  value: string;
  enabled?: boolean;
}

export interface TrafficTypeMetrics {
  requests: number;
  bytes_sent: number;
  bytes_received: number;
  active_connections: number;
}

export interface MetricsSnapshot {
  timestamp: number;
  memory_used: number;
  memory_total: number;
  cpu_usage: number;
  total_requests: number;
  active_connections: number;
  bytes_sent: number;
  bytes_received: number;
  bytes_sent_rate: number;
  bytes_received_rate: number;
  qps: number;
  max_qps: number;
  max_bytes_sent_rate: number;
  max_bytes_received_rate: number;
  http: TrafficTypeMetrics;
  https: TrafficTypeMetrics;
  tunnel: TrafficTypeMetrics;
  ws: TrafficTypeMetrics;
  wss: TrafficTypeMetrics;
  h3: TrafficTypeMetrics;
  h3s: TrafficTypeMetrics;
  socks5: TrafficTypeMetrics;
}

export interface AppMetrics {
  app_name: string;
  requests: number;
  active_connections: number;
  bytes_sent: number;
  bytes_received: number;
  http_requests: number;
  https_requests: number;
  tunnel_requests: number;
  ws_requests: number;
  wss_requests: number;
  h3_requests: number;
  h3s_requests: number;
  socks5_requests: number;
}

export interface HostMetrics {
  host: string;
  requests: number;
  active_connections: number;
  bytes_sent: number;
  bytes_received: number;
  http_requests: number;
  https_requests: number;
  tunnel_requests: number;
  ws_requests: number;
  wss_requests: number;
  h3_requests: number;
  h3s_requests: number;
  socks5_requests: number;
}

export interface SystemInfo {
  version: string;
  device_name: string;
  os: string;
  arch: string;
  cpu_logical_cores: number;
  cpu_physical_cores?: number;
  memory_total_bytes: number;
  memory_available_bytes: number;
  storage_total_bytes?: number;
  storage_available_bytes?: number;
  storage_mount_point?: string;
  uptime_secs: number;
  pid: number;
}

export interface SystemOverview {
  system: SystemInfo;
  metrics: MetricsSnapshot;
  rules: {
    total: number;
    enabled: number;
  };
  traffic: {
    recorded: number;
  };
  server: {
    port: number;
    admin_url: string;
  };
  pending_authorizations: number;
}

export interface ApiResponse<T = unknown> {
  success?: boolean;
  message?: string;
  error?: string;
  status?: number;
  data?: T;
}

export type AccessMode = 'allow_all' | 'local_only' | 'whitelist' | 'interactive';

export interface UserPassAccount {
  username: string;
  enabled: boolean;
  has_password: boolean;
  last_connected_at: string | null;
}

export interface UserPassStatus {
  enabled: boolean;
  accounts: UserPassAccount[];
  loopback_requires_auth: boolean;
}

export interface UserPassAccountUpdate {
  username: string;
  password?: string | null;
  enabled: boolean;
}

export interface WhitelistStatus {
  mode: AccessMode;
  allow_lan: boolean;
  whitelist: string[];
  temporary_whitelist: string[];
  session_denied: string[];
  userpass: UserPassStatus;
}

export interface PendingAuth {
  ip: string;
  first_seen: number;
  attempt_count: number;
}

export interface TlsConfig {
  enable_tls_interception: boolean;
  intercept_exclude: string[];
  intercept_include: string[];
  app_intercept_exclude: string[];
  app_intercept_include: string[];
  ip_intercept_exclude: string[];
  ip_intercept_include: string[];
  unsafe_ssl: boolean;
  disconnect_on_config_change: boolean;
}

export interface ProxySettings {
  tls: TlsConfig;
  tray: TrayConfig;
  port: number;
  host: string;
}

export interface TrayConfig {
  enabled: boolean;
  supported: boolean;
}

export interface CertInfo {
  available: boolean;
  status: 'not_installed' | 'installed_not_trusted' | 'installed_and_trusted' | 'unknown';
  status_label: string;
  installed: boolean;
  trusted: boolean;
  status_message: string;
  local_ips: string[];
  download_urls: string[];
  qrcode_urls: string[];
}

export interface SessionTargetSearchState {
  value?: string;
  show?: boolean;
  total?: number;
  next?: number;
  current?: number;
  tab?: string;
}

export const DisplayFormat = {
  HighLight: 'HighLight',
  Hex: 'Hex',
  Tree: 'Tree',
  Media: 'Media',
} as const;

export type DisplayFormat = typeof DisplayFormat[keyof typeof DisplayFormat];

export type RecordContentType =
  | 'JSON'
  | 'HTML'
  | 'XML'
  | 'JavaScript'
  | 'CSS'
  | 'Media'
  | 'Other';

export interface KeyValueItem {
  key: string;
  value?: string | number;
  id?: string;
  children?: KeyValueItem[];
}

export interface SearchScope {
  request_body: boolean;
  response_body: boolean;
  request_headers: boolean;
  response_headers: boolean;
  url: boolean;
  websocket_messages: boolean;
  sse_events: boolean;
  all: boolean;
}

export interface SearchFilterCondition {
  field: string;
  operator: string;
  value: string;
}

export interface SearchFilters {
  protocols: string[];
  status_ranges: string[];
  content_types: string[];
  has_rule_hit?: boolean;
  conditions: SearchFilterCondition[];
  client_ips: string[];
  client_apps: string[];
  domains: string[];
}

export interface SearchRequest {
  keyword: string;
  scope: SearchScope;
  filters: SearchFilters;
  cursor?: number;
  limit?: number;
  max_scan?: number;
  max_results?: number;
}

export interface MatchLocation {
  field: string;
  preview: string;
  offset: number;
}

export interface SearchResultItem {
  record: TrafficSummaryCompact;
  matches: MatchLocation[];
}

export interface SearchResponse {
  results: SearchResultItem[];
  total_searched: number;
  total_matched: number;
  next_cursor: number | null;
  has_more: boolean;
  search_id: string;
}

export interface VersionCheckResponse {
  has_update: boolean;
  current_version: string;
  latest_version: string | null;
  release_highlights: string[];
  release_url: string | null;
  checked_at: string | null;
}
