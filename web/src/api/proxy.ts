import { get, put } from "./client";
import { buildPublicUrl } from "../runtime";

export interface SystemProxyStatus {
  supported: boolean;
  /** Current OS system proxy state. */
  enabled: boolean;
  host: string;
  port: number;
  bypass: string;
  managed_by_bifrost?: boolean;
  /** Persisted Bifrost preference, independent from crash cleanup recovery. */
  configured_enabled?: boolean;
  configured_bypass?: string;
}

export interface SystemProxySupportStatus {
  supported: boolean;
  platform: string;
}

export interface SystemProxyLaunchdStatus {
  supported: boolean;
  installed: boolean;
  loaded: boolean;
  label: string;
  plist_path: string;
  program?: string;
  data_dir?: string;
  installed_version?: string;
  installed_mode?: "one_shot" | "keep_alive" | "unknown";
  current_version: string;
  needs_upgrade: boolean;
  needs_upgrade_reason?: string;
  message?: string;
}

export interface CliProxyStatus {
  enabled: boolean;
  shell: string;
  config_files: string[];
  proxy_url: string;
}

export interface EnhancedProxyPolicy {
  include_apps: string[];
  exclude_apps: string[];
  include_hosts: string[];
  exclude_hosts: string[];
  tcp_ports: number[];
  udp_ports: number[];
  capture_tcp: boolean;
  capture_udp: boolean;
}

export type EnhancedProxyState =
  | "unsupported"
  | "disabled"
  | "helper_missing"
  | "extension_missing"
  | "approval_required"
  | "running"
  | "error";

export interface EnhancedProxyStatus {
  supported: boolean;
  state: EnhancedProxyState;
  enabled: boolean;
  configured_enabled: boolean;
  platform: string;
  helper_bundle_id: string;
  extension_bundle_id: string;
  helper_app_path?: string;
  extension_path?: string;
  control_socket_path: string;
  controller_connected: boolean;
  proxy_host: string;
  proxy_port: number;
  policy: EnhancedProxyPolicy;
  message?: string;
  remediation?: string;
}

export interface SetSystemProxyRequest {
  enabled: boolean;
  bypass?: string;
}

export interface SetSystemProxyLaunchdRequest {
  enabled: boolean;
}

export interface SetEnhancedProxyRequest {
  enabled: boolean;
}

export async function getSystemProxyStatus(): Promise<SystemProxyStatus> {
  return get<SystemProxyStatus>("/proxy/system");
}

export async function setSystemProxy(
  request: SetSystemProxyRequest,
): Promise<SystemProxyStatus> {
  return put<SystemProxyStatus>("/proxy/system", request);
}

export async function getSystemProxySupport(): Promise<SystemProxySupportStatus> {
  return get<SystemProxySupportStatus>("/proxy/system/support");
}

export async function getSystemProxyLaunchdStatus(): Promise<SystemProxyLaunchdStatus> {
  return get<SystemProxyLaunchdStatus>("/proxy/system/launchd");
}

export async function setSystemProxyLaunchd(
  request: SetSystemProxyLaunchdRequest,
): Promise<SystemProxyLaunchdStatus> {
  return put<SystemProxyLaunchdStatus>("/proxy/system/launchd", request);
}

export async function getCliProxyStatus(): Promise<CliProxyStatus> {
  return get<CliProxyStatus>("/proxy/cli");
}

export async function getEnhancedProxyStatus(): Promise<EnhancedProxyStatus> {
  return get<EnhancedProxyStatus>("/proxy/enhanced");
}

export async function setEnhancedProxy(
  request: SetEnhancedProxyRequest,
): Promise<EnhancedProxyStatus> {
  return put<EnhancedProxyStatus>("/proxy/enhanced", request);
}

export interface ProxyAddress {
  ip: string;
  address: string;
  qrcode_url: string;
  is_preferred: boolean;
}

export interface ProxyAddressInfo {
  port: number;
  local_ips: string[];
  addresses: ProxyAddress[];
}

export async function getProxyAddressInfo(): Promise<ProxyAddressInfo> {
  return get<ProxyAddressInfo>("/proxy/address");
}

export function getProxyQRCodeUrl(ip?: string): string {
  const baseUrl = buildPublicUrl('/proxy/qrcode');
  if (ip) {
    return `${baseUrl}?ip=${encodeURIComponent(ip)}`;
  }
  return baseUrl;
}
