import { get, post } from './client';
import { buildPublicUrl } from '../runtime';

export interface CertInfo {
  available: boolean;
  status: "not_installed" | "installed_not_trusted" | "installed_and_trusted" | "unknown";
  status_label: string;
  installed: boolean;
  trusted: boolean;
  status_message: string;
  sha256_fingerprint?: string | null;
  local_ips: string[];
  download_urls: string[];
  qrcode_urls: string[];
}

export type MobilePlatform = "android" | "ios";
export type DeviceTrustCapability =
  | "guide_only"
  | "push_and_open_installer"
  | "managed_auto_trust"
  | "rooted_test_device";
export type DeviceStatus = "connected" | "unauthorized" | "offline" | "unsupported";
export type InstallMode =
  | "normal_guide"
  | "managed_auto_trust"
  | "proxy_config"
  | "lab_root_mode";
export type DeviceCertificateState =
  | "unknown"
  | "not_installed"
  | "pushed_to_device"
  | "installed";

export interface DeviceCertificateStatus {
  state: DeviceCertificateState;
  trusted?: boolean | null;
  fingerprint_match?: boolean | null;
  message: string;
}

export interface MobileDevice {
  id: string;
  name?: string | null;
  managed_install_target?: string | null;
  platform: MobilePlatform;
  status: DeviceStatus;
  capability: DeviceTrustCapability;
  certificate_status?: DeviceCertificateStatus | null;
  status_message: string;
}

export interface AdbDiscovery {
  adb_available: boolean;
  adb_path?: string | null;
  devices: MobileDevice[];
  message: string;
}

export interface IosDiscovery {
  supported: boolean;
  devices: MobileDevice[];
  configurator: ConfiguratorDiscovery;
  message: string;
}

export interface ConfiguratorDiscovery {
  supported: boolean;
  cfgutil_available: boolean;
  cfgutil_path?: string | null;
  message: string;
}

export interface MobileDevicesResponse {
  android: AdbDiscovery;
  ios: IosDiscovery;
  ios_profile_url: string;
  ios_profile_qrcode_url: string;
  ios_wifi_proxy_profile_url: string;
  ios_wifi_proxy_profile_qrcode_url: string;
  suggested_wifi_ssid?: string | null;
  ordinary_device_notice: string;
  managed_device_notice: string;
}

export interface InstallStep {
  name: string;
  success: boolean;
  message: string;
}

export interface InstallSession {
  session_id: string;
  device_id: string;
  platform: MobilePlatform;
  mode: InstallMode;
  capability: DeviceTrustCapability;
  completed: boolean;
  requires_user_confirmation: boolean;
  summary: string;
  steps: InstallStep[];
}

export type TrustProbeStatus =
  | "created"
  | "page_opened"
  | "network_reachable"
  | "tls_trusted"
  | "tls_failed"
  | "network_failed"
  | "expired";

export type TrustProbeProxyAccessStatus =
  | "allowed"
  | "pending"
  | "denied"
  | "unavailable";

export interface TrustProbeEvent {
  type: string;
  at: string;
  message?: string | null;
}

export interface TrustProbeSession {
  sessionId: string;
  status: TrustProbeStatus;
  opened: boolean;
  proxyAccessStatus?: TrustProbeProxyAccessStatus | null;
  proxyAccessAllowed?: boolean | null;
  proxyAccessMessage?: string | null;
  networkReachable: boolean;
  tlsTrusted: boolean;
  clientIp?: string | null;
  userAgent?: string | null;
  platformHint?: string | null;
  lastError?: string | null;
  events: TrustProbeEvent[];
  expiresAt: string;
  host: string;
  adminPort: number;
  probePort: number;
  landingUrl: string;
  qrCodeUrl: string;
  caDownloadUrl: string;
  proxyQrCodeUrl: string;
  caFingerprintSha256?: string | null;
}

export const MOBILE_INSTALL_CONFIRMATION =
  "push_and_open_mobile_certificate_installer";
export const IOS_PROXY_CONFIG_CONFIRMATION = "install_ios_wifi_proxy_profile";
export const LOCAL_CA_INSTALL_CONFIRMATION = "install_local_ca_certificate";

export async function getCertInfo(): Promise<CertInfo> {
  return get<CertInfo>('/cert/info');
}

export async function installLocalCa(): Promise<CertInfo> {
  return post<CertInfo>('/cert/install', {
    confirmation: LOCAL_CA_INSTALL_CONFIRMATION,
  });
}

export async function getMobileDevices(): Promise<MobileDevicesResponse> {
  return get<MobileDevicesResponse>('/mobile-devices');
}

export async function refreshMobileDevices(): Promise<MobileDevicesResponse> {
  return post<MobileDevicesResponse>('/mobile-devices/refresh');
}

export async function installMobileCa(
  deviceId: string,
  mode: InstallMode = "normal_guide",
): Promise<InstallSession> {
  return post<InstallSession>(
    `/mobile-devices/${encodeURIComponent(deviceId)}/install-ca`,
    {
      mode,
      confirmation: MOBILE_INSTALL_CONFIRMATION,
    },
  );
}

export async function installIosWifiProxyProfile(
  deviceId: string,
  ssid: string,
  proxyHost?: string,
  proxyPort?: number,
): Promise<InstallSession> {
  return post<InstallSession>(
    `/mobile-devices/${encodeURIComponent(deviceId)}/install-ios-proxy-profile`,
    {
      ssid,
      proxy_host: proxyHost,
      proxy_port: proxyPort,
      confirmation: IOS_PROXY_CONFIG_CONFIRMATION,
    },
  );
}

export async function createTrustProbeSession(
  host: string,
  ttlSeconds = 600,
): Promise<TrustProbeSession> {
  return post<TrustProbeSession>('/trust-probe/sessions', {
    host,
    ttlSeconds,
  });
}

export async function getTrustProbeSession(sessionId: string): Promise<TrustProbeSession> {
  return get<TrustProbeSession>(`/trust-probe/sessions/${encodeURIComponent(sessionId)}`);
}

export function getCertDownloadUrl(): string {
  return buildPublicUrl('/cert');
}

export function getCertQRCodeUrl(ip?: string): string {
  const baseUrl = buildPublicUrl('/cert/qrcode');
  if (ip) {
    return `${baseUrl}?ip=${encodeURIComponent(ip)}`;
  }
  return baseUrl;
}

export function getIosMobileConfigUrl(): string {
  return buildPublicUrl('/mobile/ios-profile.mobileconfig');
}

export function getIosMobileConfigQRCodeUrl(ip?: string): string {
  const baseUrl = buildPublicUrl('/mobile/ios-profile.mobileconfig/qrcode');
  if (ip) {
    return `${baseUrl}?ip=${encodeURIComponent(ip)}`;
  }
  return baseUrl;
}

export function getIosWifiProxyConfigUrl(
  ssid: string,
  ip?: string,
  port?: number,
): string {
  const baseUrl = buildPublicUrl('/mobile/ios-wifi-proxy.mobileconfig');
  const params = new URLSearchParams();
  params.set('ssid', ssid);
  if (ip) {
    params.set('ip', ip);
  }
  if (port) {
    params.set('port', String(port));
  }
  return `${baseUrl}?${params.toString()}`;
}

export function getIosWifiProxyConfigQRCodeUrl(
  ssid: string,
  ip?: string,
  port?: number,
): string {
  const baseUrl = buildPublicUrl('/mobile/ios-wifi-proxy.mobileconfig/qrcode');
  const params = new URLSearchParams();
  params.set('ssid', ssid);
  if (ip) {
    params.set('ip', ip);
  }
  if (port) {
    params.set('port', String(port));
  }
  return `${baseUrl}?${params.toString()}`;
}
