import { get, post } from "./client";

export type WorkerState =
  | "disconnected"
  | "registering"
  | "connecting"
  | "connected"
  | "reconnecting";

export type GrantMode = "once" | "30m" | "1h" | "1d" | "permanent";

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
