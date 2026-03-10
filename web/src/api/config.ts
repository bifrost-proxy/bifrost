import { get, put, del, post } from './client';

export interface TlsConfig {
  enable_tls_interception: boolean;
  intercept_exclude: string[];
  intercept_include: string[];
  app_intercept_exclude: string[];
  app_intercept_include: string[];
  unsafe_ssl: boolean;
  disconnect_on_config_change: boolean;
}

export interface ProxySettings {
  tls: TlsConfig;
  port: number;
  host: string;
}

export interface UpdateTlsConfigRequest {
  enable_tls_interception?: boolean;
  intercept_exclude?: string[];
  intercept_include?: string[];
  app_intercept_exclude?: string[];
  app_intercept_include?: string[];
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

export interface TrafficConfig {
  max_records: number;
  max_db_size_bytes: number;
  max_body_memory_size: number;
  max_body_buffer_size: number;
  max_body_probe_size: number;
  enable_body_index: boolean;
  file_retention_days: number;
}

export interface BodyStoreStats {
  file_count: number;
  total_size: number;
  temp_dir: string;
  max_memory_size: number;
  retention_days: number;
}

export interface TrafficStoreStats {
  record_count: number;
  file_size: number;
  total_records_processed: number;
  last_sequence: number;
  oldest_record_timestamp: number | null;
  newest_record_timestamp: number | null;
  traffic_dir: string;
  max_records: number;
  retention_hours: number;
  pending_writes: number;
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
}

export interface PerformanceConfig {
  traffic: TrafficConfig;
  body_store_stats: BodyStoreStats | null;
  traffic_store_stats: TrafficStoreStats | null;
  frame_store_stats: FrameStoreStats | null;
  ws_payload_store_stats: WsPayloadStoreStats | null;
}

export interface UpdateTrafficConfigRequest {
  max_records?: number;
  max_db_size_bytes?: number;
  max_body_memory_size?: number;
  max_body_buffer_size?: number;
  max_body_probe_size?: number;
  enable_body_index?: boolean;
  file_retention_days?: number;
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
