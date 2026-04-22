import { get, post, del, patch } from "./client";

export type WorkerState =
  | "disconnected"
  | "registering"
  | "connecting"
  | "connected"
  | "reconnecting";

export type GrantMode = "once" | "30m" | "1h" | "1d" | "permanent";
export type RemoteInvokeAuthMethod = "pair_code" | "ssh_publickey";

export interface DiscoverySession {
  session_id: string;
  pair_code: string;
  expires_at: number;
  created_at: number;
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
  kind?: string;
  command: string;
  args_json?: string;
}

export interface PairingRequest {
  pairing_id: string;
  caller_info: CallerInfo;
  command_summary: CommandSummary;
  command: RemoteCommand;
  caller_pubkey: string;
}

export interface RemoteInvokeStatus {
  state: string;
  discovery_session: DiscoverySession | null;
  pending_pairings_count: number;
  active_call_ids: string[];
}

export interface ClientIdentity {
  instance_id: string;
  device_name: string;
  platform: string;
}

export interface DiscoveryEnterResponse {
  success: boolean;
  session: DiscoverySession;
}

export interface DiscoveryRefreshResponse {
  success: boolean;
  session: DiscoverySession;
}

export interface PendingPairingsResponse {
  pairings: PairingRequest[];
}

export interface ApproveResponse {
  success: boolean;
  data: unknown;
}

export async function getRemoteInvokeStatus(): Promise<RemoteInvokeStatus> {
  return get<RemoteInvokeStatus>("/remote-invoke/status");
}

export async function getClientIdentity(): Promise<ClientIdentity> {
  return get<ClientIdentity>("/remote-invoke/identity");
}

export async function enterDiscoveryMode(): Promise<DiscoveryEnterResponse> {
  return post<DiscoveryEnterResponse>("/remote-invoke/discovery/enter");
}

export async function exitDiscoveryMode(): Promise<{ success: boolean }> {
  return post<{ success: boolean }>("/remote-invoke/discovery/exit");
}

export async function refreshPairCode(): Promise<DiscoveryRefreshResponse> {
  return post<DiscoveryRefreshResponse>("/remote-invoke/discovery/refresh");
}

export async function getPendingPairings(): Promise<PendingPairingsResponse> {
  return get<PendingPairingsResponse>("/remote-invoke/pairings/pending");
}

export async function approvePairing(
  pairingId: string,
  grantMode: GrantMode,
): Promise<ApproveResponse> {
  return post<ApproveResponse>(`/remote-invoke/pairings/${pairingId}/approve`, {
    grant_mode: grantMode,
  });
}

export async function rejectPairing(
  pairingId: string,
): Promise<ApproveResponse> {
  return post<ApproveResponse>(`/remote-invoke/pairings/${pairingId}/reject`);
}

export interface Grant {
  grant_id: string;
  client_instance_id: string;
  caller_fingerprint: string;
  caller_display_name?: string;
  auth_method?: RemoteInvokeAuthMethod | string;
  ssh_key_id?: string | null;
  ssh_key_fingerprint?: string | null;
  grant_mode: GrantMode;
  grant_scope: string;
  status: string;
  created_at: number;
  first_authorized_at: number;
  expires_at: number | null;
  last_used_at: number | null;
  max_calls: number;
  remaining_calls: number;
  use_count: number;
}

export interface GrantsListResponse {
  grants: Grant[];
}

export interface Call {
  call_id: string;
  grant_id: string;
  client_instance_id: string;
  caller_fingerprint: string;
  caller_display_name?: string;
  auth_method?: RemoteInvokeAuthMethod | string;
  ssh_key_id?: string | null;
  ssh_key_fingerprint?: string | null;
  caller_info_json?: Record<string, unknown> | string | null;
  command: RemoteCommand;
  command_summary: { command_preview: string; masked_args_json?: string | null };
  command_kind?: string;
  command_detail?: Record<string, unknown>;
  status: string;
  source_ip?: string;
  created_at: number;
  started_at: number;
  finished_at: number | null;
  ended_at: number | null;
  exit_code: number | null;
  duration_ms: number | null;
  bytes_in?: number | null;
  bytes_out?: number | null;
}

export interface CallsListResponse {
  calls: Call[];
}

export interface CallDetailResponse {
  call: Call;
}

export async function listGrants(): Promise<GrantsListResponse> {
  return get<GrantsListResponse>("/remote-invoke/grants");
}

export async function updateGrant(
  grantId: string,
  updates: Record<string, unknown>,
): Promise<{ success: boolean; data: unknown }> {
  return patch<{ success: boolean; data: unknown }>(`/remote-invoke/grants/${grantId}`, updates);
}

export async function revokeGrant(
  grantId: string,
): Promise<{ success: boolean }> {
  return del<{ success: boolean }>(`/remote-invoke/grants/${grantId}`);
}

export async function listCalls(): Promise<CallsListResponse> {
  return get<CallsListResponse>("/remote-invoke/calls");
}

export async function getCall(
  callId: string,
): Promise<CallDetailResponse> {
  return get<CallDetailResponse>(`/remote-invoke/calls/${callId}`);
}

export interface RemoteInvokeSshCallerInfo {
  hostname?: string;
  username?: string;
  platform?: string;
  user_agent?: string;
  source_ip?: string;
  ip?: string;
}

export interface RemoteInvokeSshKeyRecord {
  id: string;
  label: string;
  device_code: string;
  ssh_key_fingerprint: string;
  status: string;
  grant_mode: GrantMode;
  created_at?: string | number | null;
  last_used_at?: string | number | null;
  last_caller_info: RemoteInvokeSshCallerInfo | null;
}

export interface RemoteInvokeSshKeySecretPayload {
  id?: string;
  label?: string;
  device_code: string;
  ssh_key_fingerprint: string;
  bifrost_key_file: string;
  public_key_pem?: string | null;
  grant_mode?: GrantMode;
}

interface RemoteInvokeSshKeyRecordResponse {
  id?: string;
  label?: string;
  device_code?: string;
  ssh_key_fingerprint?: string;
  fingerprint?: string;
  status?: string;
  grant_mode?: GrantMode;
  created_at?: string | number | null;
  last_used_at?: string | number | null;
  last_caller_info_json?: unknown;
  last_caller_info?: unknown;
}

function isObjectRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readString(
  source: Record<string, unknown>,
  key: string,
): string | undefined {
  const value = source[key];
  return typeof value === "string" && value.trim() ? value : undefined;
}

export function normalizeRemoteInvokeSshCallerInfo(
  value: unknown,
): RemoteInvokeSshCallerInfo | null {
  let parsed = value;
  if (typeof parsed === "string") {
    try {
      parsed = JSON.parse(parsed);
    } catch {
      return null;
    }
  }

  if (!isObjectRecord(parsed)) {
    return null;
  }

  const info: RemoteInvokeSshCallerInfo = {
    hostname: readString(parsed, "hostname"),
    username: readString(parsed, "username"),
    platform: readString(parsed, "platform"),
    user_agent: readString(parsed, "user_agent") ?? readString(parsed, "userAgent"),
    source_ip: readString(parsed, "source_ip") ?? readString(parsed, "sourceIp"),
    ip: readString(parsed, "ip"),
  };

  return Object.values(info).some(Boolean) ? info : null;
}

export function normalizeRemoteInvokeSshKeyRecord(
  value: RemoteInvokeSshKeyRecordResponse | null | undefined,
): RemoteInvokeSshKeyRecord | null {
  if (!value || !value.device_code) {
    return null;
  }

  return {
    id: value.id ?? value.device_code,
    label: value.label ?? "SSH key",
    device_code: value.device_code,
    ssh_key_fingerprint:
      value.ssh_key_fingerprint ?? value.fingerprint ?? "",
    status: value.status ?? "active",
    grant_mode: value.grant_mode ?? "once",
    created_at: value.created_at,
    last_used_at: value.last_used_at,
    last_caller_info: normalizeRemoteInvokeSshCallerInfo(
      value.last_caller_info_json ?? value.last_caller_info,
    ),
  };
}

export interface CreateRemoteInvokeSshKeyInput {
  label: string;
  grant_mode: GrantMode;
}

export interface UpdateRemoteInvokeSshKeyInput {
  label?: string;
  grant_mode?: GrantMode;
}

export async function getRemoteInvokeSshKey(): Promise<RemoteInvokeSshKeyRecord | null> {
  const response = await get<RemoteInvokeSshKeyRecordResponse | null>("/remote-invoke/ssh-key");
  return normalizeRemoteInvokeSshKeyRecord(response);
}

export async function createRemoteInvokeSshKey(
  input: CreateRemoteInvokeSshKeyInput,
): Promise<RemoteInvokeSshKeySecretPayload> {
  return post<RemoteInvokeSshKeySecretPayload>("/remote-invoke/ssh-key", input);
}

export async function updateRemoteInvokeSshKey(
  input: UpdateRemoteInvokeSshKeyInput,
): Promise<RemoteInvokeSshKeyRecord> {
  const response = await patch<RemoteInvokeSshKeyRecordResponse>("/remote-invoke/ssh-key", input);
  return normalizeRemoteInvokeSshKeyRecord(response) as RemoteInvokeSshKeyRecord;
}

export async function resetRemoteInvokeSshKey(): Promise<RemoteInvokeSshKeySecretPayload> {
  return post<RemoteInvokeSshKeySecretPayload>("/remote-invoke/ssh-key/reset");
}

export async function revokeRemoteInvokeSshKey(): Promise<{ success: boolean }> {
  return del<{ success: boolean }>("/remote-invoke/ssh-key");
}

export async function getRemoteInvokeSshPrivateKey(): Promise<RemoteInvokeSshKeySecretPayload> {
  return get<RemoteInvokeSshKeySecretPayload>("/remote-invoke/ssh-key/private-key");
}
