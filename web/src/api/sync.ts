import { get, post, put } from "./client";

export interface SyncUser {
  user_id: string;
  nickname: string;
  avatar: string;
  email: string;
}

export type SyncReason =
  | "disabled"
  | "reachable"
  | "unreachable"
  | "unauthorized"
  | "ready"
  | "syncing"
  | "error";

export type SyncAction =
  | "local_pushed"
  | "remote_pulled"
  | "bidirectional"
  | "no_change";

export interface SyncStatus {
  enabled: boolean;
  auto_sync: boolean;
  remote_base_url: string;
  has_session: boolean;
  reachable: boolean;
  authorized: boolean;
  syncing: boolean;
  reason: SyncReason;
  last_sync_at?: string | null;
  last_sync_action?: SyncAction | null;
  last_error?: string | null;
  user?: SyncUser | null;
  providers?: SyncProviderStatus[];
  first_run_prompt_required?: boolean;
}

export interface SyncProviderCapabilities {
  rules_sync: boolean;
  config_sync: boolean;
  remote_invoke: boolean;
}

export interface SyncProviderStatus {
  id: string;
  name: string;
  description: string;
  remote_base_url?: string | null;
  connected: boolean;
  enabled: boolean;
  reachable: boolean;
  authorized: boolean;
  reason: SyncReason;
  last_error?: string | null;
  last_sync_at?: string | null;
  last_sync_action?: SyncAction | null;
  user?: SyncUser | null;
  capabilities: SyncProviderCapabilities;
  remote_invoke_registered: boolean;
}

export interface UpdateSyncConfigRequest {
  enabled?: boolean;
  auto_sync?: boolean;
  remote_base_url?: string;
  probe_interval_secs?: number;
  connect_timeout_ms?: number;
}

export interface SyncLoginRequest {
  token?: string;
  provider_id?: string;
  remote_base_url?: string;
}

export async function getSyncStatus(): Promise<SyncStatus> {
  return get<SyncStatus>("/sync/status");
}

export async function updateSyncConfig(
  request: UpdateSyncConfigRequest,
): Promise<SyncStatus> {
  return put<SyncStatus>("/sync/config", request);
}

export async function getSyncLoginUrl(callbackUrl: string): Promise<string> {
  const response = await get<{ login_url: string }>(
    `/sync/login-url?callback_url=${encodeURIComponent(callbackUrl)}`,
  );
  return response.login_url;
}

export async function openSyncLogin(request?: SyncLoginRequest): Promise<SyncStatus> {
  return post<SyncStatus>("/sync/login", request);
}

export async function saveSyncSession(token: string): Promise<SyncStatus> {
  return post<SyncStatus>("/sync/session", { token });
}

export async function logoutSyncSession(): Promise<SyncStatus> {
  return post<SyncStatus>("/sync/logout");
}

export async function runSyncNow(): Promise<SyncStatus> {
  return post<SyncStatus>("/sync/run");
}
