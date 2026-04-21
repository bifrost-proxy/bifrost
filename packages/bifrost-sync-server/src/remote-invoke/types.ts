import type { ServerResponse } from 'http';
import type {
  GrantMode,
  GrantStatus,
  CallStatus,
  PairingStatus,
  CallerInfo,
  CommandSummary,
  RemoteCommand,
} from '../types';

export type {
  GrantMode,
  GrantStatus,
  CallStatus,
  PairingStatus,
  CallerInfo,
  CommandSummary,
  RemoteCommand,
};

export interface ClientStreamState {
  clientInstanceId: string;
  userId: string;
  streamId: string;
  clientIp?: string;
  res: ServerResponse;
  lastHeartbeat: number;
  discoverable: boolean;
  discoverySessionId?: string;
  pairCode?: string;
  pairCodeExpiresAt?: number;
  connectedAt: number;
}

export interface PairingSessionRecord {
  id: string;
  userId: string;
  clientInstanceId: string;
  callerFingerprint: string;
  pairCode: string;
  status: PairingStatus;
  callerPubkey: string;
  clientEphemeralPub: string;
  callerInfo: CallerInfo;
  commandSummary: CommandSummary;
  command: RemoteCommand;
  relayToken: string;
  callId: string;
  grantId: string;
  expiresAt: number;
  createdAt: number;
}

export interface GrantRecord {
  id: string;
  userId: string;
  clientInstanceId: string;
  callerFingerprint: string;
  grantMode: GrantMode;
  grantScope: string;
  status: GrantStatus;
  firstAuthorizedAt: number;
  expiresAt: number;
  lastUsedAt: number;
  maxCalls: number;
  remainingCalls: number;
  createdBy: string;
}

export interface CallRecord {
  id: string;
  userId: string;
  grantId: string;
  pairingId: string;
  clientInstanceId: string;
  callerFingerprint: string;
  sourceIp: string;
  callerDisplayName: string;
  status: CallStatus;
  commandSummary: CommandSummary;
  command: RemoteCommand;
  payloadDigest: string;
  stdoutDigest: string;
  stderrDigest: string;
  exitCode: number;
  startedAt: number;
  endedAt: number;
  durationMs: number;
  bytesIn: number;
  bytesOut: number;
}

export interface EventCursor {
  callId: string;
  lastSeq: number;
}

export interface EncryptedEnvelope {
  version: number;
  call_id: string;
  seq: number;
  direction: 'caller_to_client' | 'client_to_caller';
  nonce: string;
  ciphertext: string;
  tag: string;
  aad?: {
    token_hash?: string;
    frame_type?: string;
  };
}

export interface StartPairingRequest {
  pair_code: string;
  caller_info: CallerInfo;
}

export interface GrantDecisionRequest {
  pairing_id: string;
  client_instance_id: string;
  decision: 'approve' | 'reject';
  grant_mode?: GrantMode;
  client_ephemeral_pub?: string;
}

export interface PublishPairCodeRequest {
  client_instance_id: string;
  pair_code: string;
  expires_at: number;
  discovery_session_id?: string;
}

export interface ClientHeartbeatRequest {
  client_instance_id: string;
  stream_id: string;
  active_call_ids?: string[];
  ssh_device_route?: SshDeviceRoute | null;
}

export interface ClientCallFrameRequest {
  call_id: string;
  client_instance_id: string;
  envelope_json: string;
}

export interface ClientCallExitRequest {
  call_id: string;
  client_instance_id: string;
  exit_code: number;
  duration_ms?: number;
  stderr?: string;
  stdout_digest?: string;
  stderr_digest?: string;
  bytes_in?: number;
  bytes_out?: number;
}

export interface OpenCallRequest {
  grant_id: string;
  client_instance_id: string;
  caller_fingerprint: string;
  caller_pubkey: string;
  command_summary: CommandSummary;
  command: RemoteCommand;
}

export interface CallsQueryParams {
  client_instance_id?: string;
  caller_fingerprint?: string;
  status?: string;
  offset?: number;
  limit?: number;
}

export interface GrantsQueryParams {
  client_instance_id?: string;
  status?: string;
  offset?: number;
  limit?: number;
}

export interface ClientRegistrationRequest {
  challenge_id: string;
  client_instance_id: string;
  client_long_term_pubkey: string;
  device_name: string;
  platform: string;
  bifrost_version: string;
  signature: string;
  timestamp: number;
  ssh_device_route?: SshDeviceRoute | null;
}

export interface SshDeviceRoute {
  device_code: string;
  public_key_pem: string;
}

export interface SshChallengeRequest {
  device_code: string;
}

export interface SshChallengeResponse {
  challenge_id: string;
  challenge: string;
  expires_at: number;
}

export interface SshCallerInfo {
  hostname?: string;
  username?: string;
  platform?: string;
  user_agent?: string;
}

export interface SshConnectRequest {
  device_code: string;
  challenge_id: string;
  signature: string;
  timestamp: number;
  caller_info?: SshCallerInfo;
}

export interface SshConnectResponse {
  connect_id: string;
  relay_token: string;
  expires_at: number;
}

export interface SshConnectResultRequest {
  connect_id: string;
  status: 'approved' | 'rejected';
  grant_id?: string;
  expires_at?: number | string | null;
  reason?: string;
  caller_fingerprint?: string;
  grant_mode?: GrantMode;
}

export interface ClientRegistrationChallengeRequest {
  client_instance_id: string;
}

export interface ClientRegistrationChallengeResponse {
  challenge_id: string;
  challenge: string;
  expires_at: number;
  algorithm: string;
}

export function buildRegistrationSignaturePayload(
  challengeId: string,
  challenge: string,
  clientInstanceId: string,
  deviceName: string,
  platform: string,
  bifrostVersion: string,
  clientLongTermPubkey: string,
  timestamp: number,
): string {
  return JSON.stringify([
    'bifrost-remote-register-v1',
    challengeId,
    challenge,
    clientInstanceId,
    deviceName,
    platform,
    bifrostVersion,
    clientLongTermPubkey,
    timestamp,
  ]);
}

export const ALLOWED_COMMANDS = new Set([
  'connect',
  'status',
  'traffic.list',
  'traffic.get',
  'traffic.search',
  'search.get',
]);

export function isAllowedCommand(command: string): boolean {
  return ALLOWED_COMMANDS.has(command);
}

export function grantModeTtlMs(mode: GrantMode): number | null {
  switch (mode) {
    case 'once': return null;
    case '30m': return 30 * 60 * 1000;
    case '1h': return 60 * 60 * 1000;
    case '1d': return 24 * 60 * 60 * 1000;
    case 'permanent': return null;
  }
}
