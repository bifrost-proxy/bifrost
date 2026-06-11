import { get, put, del, post } from './client';

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

export interface UpdateTlsConfigRequest {
  enable_tls_interception?: boolean;
  intercept_exclude?: string[];
  intercept_include?: string[];
  app_intercept_exclude?: string[];
  app_intercept_include?: string[];
  ip_intercept_exclude?: string[];
  ip_intercept_include?: string[];
  unsafe_ssl?: boolean;
  disconnect_on_config_change?: boolean;
}

export async function getProxySettings(): Promise<ProxySettings> {
  return get<ProxySettings>('/config');
}

export async function getTlsConfig(): Promise<TlsConfig> {
  return get<TlsConfig>('/config/tls');
}

export async function updateTlsConfig(config: UpdateTlsConfigRequest): Promise<TlsConfig> {
  return put<TlsConfig>('/config/tls', config);
}

export async function updateTrayConfig(config: { enabled: boolean }): Promise<TrayConfig> {
  return put<TrayConfig>('/config/tray', config);
}

export interface TrafficConfig {
  max_records: number;
  max_db_size_bytes: number;
  max_body_memory_size: number;
  max_body_buffer_size: number;
  max_body_probe_size: number;
  binary_traffic_performance_mode: boolean;
  inject_bifrost_badge: boolean;
  file_retention_days: number;
}

export interface BodyStoreStats {
  file_count: number;
  total_size: number;
  temp_dir: string;
  max_memory_size: number;
  retention_days: number;
  active_stream_writers: number;
  max_open_stream_writers: number;
}

export interface FrameStoreStats {
  connection_count: number;
  total_size: number;
  frames_dir: string;
  retention_hours: number;
}

export interface WsPayloadStoreStats {
  file_count: number;
  total_size: number;
  payload_dir: string;
  retention_days: number;
  active_writers: number;
  max_open_files: number;
}

export type ResourceAlertLevel = 'ok' | 'warn' | 'critical';

export interface ResourceAlertStatus {
  level: ResourceAlertLevel;
  current: number;
  limit: number;
  usage_ratio: number;
  message: string;
}

export interface ResourceAlerts {
  overall_level: ResourceAlertLevel;
  body_stream_writers: ResourceAlertStatus | null;
  ws_payload_writers: ResourceAlertStatus | null;
}

export interface PerformanceConfig {
  traffic: TrafficConfig;
  breakpoint: BreakpointPerformanceConfig;
  body_store_stats: BodyStoreStats | null;
  frame_store_stats: FrameStoreStats | null;
  ws_payload_store_stats: WsPayloadStoreStats | null;
  resource_alerts: ResourceAlerts;
}

export interface BreakpointPerformanceConfig {
  timeout_ms: number;
  timeout_min_ms: number;
  timeout_max_ms: number;
}

export interface SandboxFileConfig {
  sandbox_dir: string;
  allowed_dirs: string[];
  max_bytes: number;
}

export interface SandboxNetConfig {
  enabled: boolean;
  timeout_ms: number;
  max_request_bytes: number;
  max_response_bytes: number;
}

export interface SandboxLimitsConfig {
  timeout_ms: number;
  max_memory_bytes: number;
  max_decode_input_bytes: number;
  max_decompress_output_bytes: number;
}

export interface SandboxConfig {
  file: SandboxFileConfig;
  net: SandboxNetConfig;
  limits: SandboxLimitsConfig;
}

export interface UpdateSandboxConfigRequest {
  file?: Partial<SandboxFileConfig>;
  net?: Partial<SandboxNetConfig>;
  limits?: Partial<SandboxLimitsConfig>;
}

export async function getSandboxConfig(): Promise<SandboxConfig> {
  return get<SandboxConfig>('/config/sandbox');
}

export async function updateSandboxConfig(config: UpdateSandboxConfigRequest): Promise<SandboxConfig> {
  return put<SandboxConfig>('/config/sandbox', config);
}

export interface UpdateTrafficConfigRequest {
  max_records?: number;
  max_db_size_bytes?: number;
  max_body_memory_size?: number;
  max_body_buffer_size?: number;
  max_body_probe_size?: number;
  binary_traffic_performance_mode?: boolean;
  inject_bifrost_badge?: boolean;
  file_retention_days?: number;
  breakpoint_timeout_ms?: number;
}

export async function getPerformanceConfig(): Promise<PerformanceConfig> {
  return get<PerformanceConfig>('/config/performance');
}

export async function updatePerformanceConfig(config: UpdateTrafficConfigRequest): Promise<PerformanceConfig> {
  return put<PerformanceConfig>('/config/performance', config);
}

export interface ClearCacheResponse {
  removed_files: number;
  message: string;
}

export async function clearBodyCache(): Promise<ClearCacheResponse> {
  return del<ClearCacheResponse>('/config/performance/clear-cache');
}

export interface DisconnectResponse {
  success: boolean;
  disconnected_count: number;
  message: string;
}

export async function disconnectByDomain(domain: string): Promise<DisconnectResponse> {
  return post<DisconnectResponse>('/config/connections/disconnect', { domain });
}

export async function disconnectByApp(app: string): Promise<DisconnectResponse> {
  return post<DisconnectResponse>('/config/connections/disconnect-by-app', { app });
}

export interface PendingIpTls {
  ip: string;
  first_seen: number;
  attempt_count: number;
}

export async function getIpTlsPending(): Promise<PendingIpTls[]> {
  return get<PendingIpTls[]>('/config/ip-tls/pending');
}

export async function approveIpTls(ip: string): Promise<{ success: boolean; message: string }> {
  return post('/config/ip-tls/pending/approve', { ip });
}

export async function skipIpTls(ip: string): Promise<{ success: boolean; message: string }> {
  return post('/config/ip-tls/pending/skip', { ip });
}

export async function clearIpTlsPending(): Promise<{ success: boolean; message: string }> {
  return del('/config/ip-tls/pending');
}
