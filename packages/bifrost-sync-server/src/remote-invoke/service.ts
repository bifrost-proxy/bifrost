import crypto from 'crypto';
import { customAlphabet } from 'nanoid';
import type { IStorage } from '../dao/types';
import type { RemoteInvokeConfig, RemoteInvokeGrant, RemoteInvokeCall, RemoteInvokeEvent, RemoteInvokePairing, RemoteInvokeClientRecord } from '../types';
import type {
  StartPairingRequest,
  GrantDecisionRequest,
  PublishPairCodeRequest,
  ClientHeartbeatRequest,
  ClientCallFrameRequest,
  ClientCallExitRequest,
  OpenCallRequest,
  UpdateGrantRequest,
  CallsQueryParams,
  GrantsQueryParams,
  ClientRegistrationRequest,
} from './types';
import { isAllowedCommand, grantModeTtlMs } from './types';
import {
  pushToClient,
  getClientStream,
  updateClientDiscovery,
  clearClientDiscovery,
  consumeClientDiscovery,
  findClientByPairCode,
  pushToPairingWatcher,
  pushToCallerStream,
  registerCallerEventStream,
  getAllClientStreams,
} from './sse';

const nanoid = customAlphabet('0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_', 21);

function generateRelayToken(): string {
  return crypto.randomBytes(32).toString('hex');
}

function constantTimeCompare(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  const bufA = Buffer.from(a, 'utf8');
  const bufB = Buffer.from(b, 'utf8');
  return crypto.timingSafeEqual(bufA, bufB);
}

export class RemoteInvokeService {
  constructor(
    private storage: IStorage,
    private config: RemoteInvokeConfig,
  ) {}

  async registerClient(req: ClientRegistrationRequest): Promise<{ client_auth_token: string; expires_at: string }> {
    const existing = await this.storage.remoteInvoke.getClientRecord(req.client_instance_id);
    if (existing) {
      const pubkeyHash = crypto.createHash('sha256').update(req.client_long_term_pubkey).digest('hex');
      if (existing.client_pubkey_hash && existing.client_pubkey_hash !== pubkeyHash) {
        throw new Error('pubkey mismatch for existing client_instance_id');
      }
    }

    const token = generateRelayToken();
    const now = new Date().toISOString();
    const expiresAt = new Date(Date.now() + 30 * 24 * 60 * 60 * 1000).toISOString();
    const pubkeyHash = crypto.createHash('sha256').update(req.client_long_term_pubkey).digest('hex');

    const record: RemoteInvokeClientRecord = {
      client_instance_id: req.client_instance_id,
      user_id: '',
      client_name: req.device_name,
      platform: req.platform,
      bifrost_version: req.bifrost_version,
      client_auth_token: token,
      client_pubkey_hash: pubkeyHash,
      token_expires_at: expiresAt,
      last_heartbeat_at: now,
      create_time: existing?.create_time ?? now,
      update_time: now,
    };

    await this.storage.remoteInvoke.registerClient(record);
    return { client_auth_token: token, expires_at: expiresAt };
  }

  async verifyClientAuth(clientInstanceId: string, token: string): Promise<RemoteInvokeClientRecord | null> {
    const record = await this.storage.remoteInvoke.getClientRecord(clientInstanceId);
    if (!record) return null;
    if (!constantTimeCompare(record.client_auth_token, token)) return null;
    if (new Date(record.token_expires_at) < new Date()) return null;
    return record;
  }

  async publishPairCode(userId: string, req: PublishPairCodeRequest): Promise<void> {
    updateClientDiscovery(
      req.client_instance_id,
      req.pair_code,
      req.expires_at,
      req.discovery_session_id,
    );
  }

  async closeDiscoverySession(clientInstanceId: string, _discoverySessionId: string): Promise<void> {
    clearClientDiscovery(clientInstanceId);
  }

  async startPairing(userId: string, req: StartPairingRequest, sourceIp: string): Promise<{ pairing_id: string; status: string }> {
    if (!isAllowedCommand(req.command.command)) {
      throw new Error('unsupported_command');
    }

    const result = findClientByPairCode(req.pair_code);
    if (!result.found) {
      if (result.reason === 'consumed') {
        throw new Error('pair_code_already_consumed');
      }
      if (result.reason === 'expired') {
        throw new Error('pair_code_expired');
      }
      throw new Error('invalid_pair_code');
    }

    const clientStream = result.client;

    if (!constantTimeCompare(clientStream.pairCode ?? '', req.pair_code)) {
      throw new Error('invalid_pair_code');
    }

    if (req.client_instance_id !== clientStream.clientInstanceId) {
      throw new Error('client_instance_id_mismatch');
    }

    const pendingCount = await this.storage.remoteInvoke.countPendingPairings(req.client_instance_id);
    if (pendingCount > 0) {
      throw new Error('pair_slot_occupied');
    }

    const pairingId = nanoid();
    const now = new Date().toISOString();
    const expiresAt = new Date(Date.now() + this.config.pair_code_ttl_secs * 1000).toISOString();

    const pairing: RemoteInvokePairing = {
      id: pairingId,
      user_id: userId,
      client_instance_id: req.client_instance_id,
      caller_fingerprint: req.caller_info.fingerprint,
      pair_code: req.pair_code,
      status: 'pending_approval',
      caller_pubkey: req.caller_pubkey,
      client_ephemeral_pub: '',
      caller_info_json: JSON.stringify(req.caller_info),
      command_summary_json: JSON.stringify(req.command_summary),
      command_json: JSON.stringify(req.command),
      relay_token: '',
      call_id: '',
      grant_id: '',
      expires_at: expiresAt,
      create_time: now,
      update_time: now,
    };

    await this.storage.remoteInvoke.createPairing(pairing);

    consumeClientDiscovery(req.client_instance_id);

    pushToClient(req.client_instance_id, 'pairing_request', {
      pairing_id: pairingId,
      caller_fingerprint: req.caller_info.fingerprint,
      caller_display_name: req.caller_info.display_name ?? '',
      caller_pubkey: req.caller_pubkey,
      source_ip: sourceIp,
      user_agent: req.caller_info.user_agent ?? '',
      command_summary: req.command_summary,
      command: req.command,
      expires_at: expiresAt,
    });

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: '',
      event_type: 'pairing_created',
      seq: 0,
      direction: '',
      event_summary_json: JSON.stringify({ pairing_id: pairingId, caller_fingerprint: req.caller_info.fingerprint }),
      create_time: now,
    });

    return { pairing_id: pairingId, status: 'pending_approval' };
  }

  async submitGrantDecision(userId: string, req: GrantDecisionRequest): Promise<{ grant_id?: string; call_id?: string; relay_token?: string; status: string }> {
    const pairing = await this.storage.remoteInvoke.getPairing(req.pairing_id);
    if (!pairing) throw new Error('pairing_not_found');
    if (pairing.status !== 'pending_approval') throw new Error('pairing_not_pending');
    if (pairing.client_instance_id !== req.client_instance_id) throw new Error('client_mismatch');

    const now = new Date().toISOString();

    if (req.decision === 'reject') {
      await this.storage.remoteInvoke.updatePairing(req.pairing_id, {
        status: 'rejected',
        update_time: now,
      });

      pushToPairingWatcher(req.pairing_id, 'rejected', { pairing_id: req.pairing_id });

      await this.storage.remoteInvoke.appendEvent({
        id: nanoid(),
        call_id: '',
        event_type: 'pairing_rejected',
        seq: 0,
        direction: '',
        event_summary_json: JSON.stringify({ pairing_id: req.pairing_id }),
        create_time: now,
      });

      return { status: 'rejected' };
    }

    const grantMode = req.grant_mode ?? 'once';
    const grantId = nanoid();
    const callId = nanoid();
    const relayToken = generateRelayToken();

    const activeGrantCount = await this.storage.remoteInvoke.countActiveGrantsForClient(pairing.client_instance_id);
    if (activeGrantCount >= this.config.max_grants_per_client) {
      throw new Error('max_grants_exceeded');
    }

    let grantExpiresAt = '';
    const ttl = grantModeTtlMs(grantMode as any);
    if (ttl) {
      grantExpiresAt = new Date(Date.now() + ttl).toISOString();
    }

    const maxCalls = grantMode === 'once' ? 1 : 999999;

    const callerDisplayName = (() => {
      try { return JSON.parse(pairing.caller_info_json).display_name ?? ''; } catch { return ''; }
    })();

    const grant: RemoteInvokeGrant = {
      id: grantId,
      user_id: pairing.user_id,
      client_instance_id: pairing.client_instance_id,
      caller_fingerprint: pairing.caller_fingerprint,
      caller_display_name: callerDisplayName,
      grant_mode: grantMode as any,
      grant_scope: 'query',
      status: 'active',
      first_authorized_at: now,
      expires_at: grantExpiresAt,
      last_used_at: now,
      max_calls: maxCalls,
      remaining_calls: maxCalls,
      created_by: userId,
      update_time: now,
    };
    await this.storage.remoteInvoke.createGrant(grant);

    const call: RemoteInvokeCall = {
      id: callId,
      user_id: pairing.user_id,
      grant_id: grantId,
      pairing_id: pairing.id,
      client_instance_id: pairing.client_instance_id,
      caller_fingerprint: pairing.caller_fingerprint,
      source_ip: JSON.parse(pairing.caller_info_json).source_ip ?? '',
      caller_display_name: JSON.parse(pairing.caller_info_json).display_name ?? '',
      status: 'authorized',
      command_summary_json: pairing.command_summary_json,
      command_json: pairing.command_json,
      payload_digest: '',
      stdout_digest: '',
      stderr_digest: '',
      exit_code: -1,
      started_at: now,
      ended_at: '',
      duration_ms: 0,
      bytes_in: 0,
      bytes_out: 0,
    };
    await this.storage.remoteInvoke.createCall(call);

    await this.storage.remoteInvoke.updatePairing(pairing.id, {
      status: 'approved',
      client_ephemeral_pub: req.client_ephemeral_pub ?? '',
      relay_token: relayToken,
      call_id: callId,
      grant_id: grantId,
      update_time: now,
    });

    pushToPairingWatcher(pairing.id, 'approved', {
      pairing_id: pairing.id,
      call_id: callId,
      relay_token: relayToken,
      grant_id: grantId,
      client_ephemeral_pub: req.client_ephemeral_pub ?? '',
      expires_at: grantExpiresAt,
    });

    pushToClient(pairing.client_instance_id, 'grant_created', {
      grant_id: grantId,
      call_id: callId,
      caller_fingerprint: pairing.caller_fingerprint,
      grant_mode: grantMode,
      command: JSON.parse(pairing.command_json),
      command_summary: JSON.parse(pairing.command_summary_json),
    });

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: callId,
      event_type: 'pairing_approved',
      seq: 0,
      direction: '',
      event_summary_json: JSON.stringify({ pairing_id: pairing.id, grant_id: grantId, grant_mode: grantMode }),
      create_time: now,
    });

    return { grant_id: grantId, call_id: callId, relay_token: relayToken, status: 'approved' };
  }

  async findReusableGrant(userId: string, clientInstanceId: string, callerFingerprint: string): Promise<RemoteInvokeGrant | null> {
    const grant = await this.storage.remoteInvoke.findReusableGrant(userId, clientInstanceId, callerFingerprint);
    if (!grant) return null;

    if (grant.expires_at && new Date(grant.expires_at) < new Date()) {
      await this.storage.remoteInvoke.updateGrant(grant.id, { status: 'expired' });
      return null;
    }

    if (grant.grant_mode === 'once' && grant.remaining_calls <= 0) {
      await this.storage.remoteInvoke.updateGrant(grant.id, { status: 'consumed' });
      return null;
    }

    return grant;
  }

  async openCall(userId: string, req: OpenCallRequest): Promise<{ call_id: string; relay_token: string }> {
    const grant = await this.storage.remoteInvoke.getGrant(req.grant_id);
    if (!grant) throw new Error('grant_not_found');
    if (grant.status !== 'active') throw new Error('grant_not_active');

    if (grant.expires_at && new Date(grant.expires_at) < new Date()) {
      await this.storage.remoteInvoke.updateGrant(grant.id, { status: 'expired' });
      throw new Error('grant_expired');
    }

    if (grant.remaining_calls <= 0) {
      await this.storage.remoteInvoke.updateGrant(grant.id, { status: 'consumed' });
      throw new Error('grant_consumed');
    }

    if (!isAllowedCommand(req.command.command)) {
      throw new Error('unsupported_command');
    }

    const callId = nanoid();
    const relayToken = generateRelayToken();
    const now = new Date().toISOString();

    const call: RemoteInvokeCall = {
      id: callId,
      user_id: userId,
      grant_id: grant.id,
      pairing_id: '',
      client_instance_id: req.client_instance_id,
      caller_fingerprint: grant.caller_fingerprint,
      source_ip: '',
      caller_display_name: grant.caller_display_name || '',
      status: 'authorized',
      command_summary_json: JSON.stringify(req.command_summary),
      command_json: JSON.stringify(req.command),
      payload_digest: '',
      stdout_digest: '',
      stderr_digest: '',
      exit_code: -1,
      started_at: now,
      ended_at: '',
      duration_ms: 0,
      bytes_in: 0,
      bytes_out: 0,
    };
    await this.storage.remoteInvoke.createCall(call);

    await this.storage.remoteInvoke.consumeGrantCall(grant.id);
    await this.storage.remoteInvoke.touchGrantLastUsed(grant.id, now);

    pushToClient(req.client_instance_id, 'call_open', {
      call_id: callId,
      grant_id: grant.id,
      caller_fingerprint: grant.caller_fingerprint,
      caller_pubkey: req.caller_pubkey,
      command: req.command,
      command_summary: req.command_summary,
      relay_token: relayToken,
    });

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: callId,
      event_type: 'call_opened',
      seq: 0,
      direction: '',
      event_summary_json: JSON.stringify({ grant_id: grant.id }),
      create_time: now,
    });

    return { call_id: callId, relay_token: relayToken };
  }

  async postCallerInput(callId: string, envelopeJson: string): Promise<void> {
    const call = await this.storage.remoteInvoke.getCall(callId);
    if (!call) throw new Error('call_not_found');
    if (call.status === 'completed' || call.status === 'failed' || call.status === 'cancelled') {
      throw new Error('call_already_ended');
    }

    if (call.status === 'authorized') {
      await this.storage.remoteInvoke.updateCall(callId, { status: 'streaming' });
    }

    pushToClient(call.client_instance_id, 'call_frame', {
      call_id: callId,
      envelope_json: envelopeJson,
    });

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: callId,
      event_type: 'call_frame_in',
      seq: 0,
      direction: 'caller_to_client',
      event_summary_json: JSON.stringify({ size: envelopeJson.length }),
      create_time: new Date().toISOString(),
    });
  }

  async postClientFrame(req: ClientCallFrameRequest): Promise<void> {
    const call = await this.storage.remoteInvoke.getCall(req.call_id);
    if (!call) throw new Error('call_not_found');

    pushToCallerStream(req.call_id, 'frame', {
      call_id: req.call_id,
      envelope_json: req.envelope_json,
    });

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: req.call_id,
      event_type: 'call_frame_out',
      seq: 0,
      direction: 'client_to_caller',
      event_summary_json: JSON.stringify({ size: req.envelope_json.length }),
      create_time: new Date().toISOString(),
    });
  }

  async postClientExit(req: ClientCallExitRequest): Promise<void> {
    const call = await this.storage.remoteInvoke.getCall(req.call_id);
    if (!call) throw new Error('call_not_found');

    const now = new Date().toISOString();
    await this.storage.remoteInvoke.updateCall(req.call_id, {
      status: 'completed',
      exit_code: req.exit_code,
      ended_at: now,
      duration_ms: req.duration_ms ?? 0,
      stdout_digest: req.stdout_digest ?? '',
      stderr_digest: req.stderr_digest ?? '',
      bytes_in: req.bytes_in ?? 0,
      bytes_out: req.bytes_out ?? 0,
    });

    pushToCallerStream(req.call_id, 'exit', {
      call_id: req.call_id,
      exit_code: req.exit_code,
      duration_ms: req.duration_ms,
      stdout_digest: req.stdout_digest,
      stderr_digest: req.stderr_digest,
    });

    const grant = await this.storage.remoteInvoke.getGrant(call.grant_id);
    if (grant && grant.grant_mode === 'once') {
      await this.storage.remoteInvoke.updateGrant(grant.id, { status: 'consumed' });
    }

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: req.call_id,
      event_type: 'call_completed',
      seq: 0,
      direction: '',
      event_summary_json: JSON.stringify({ exit_code: req.exit_code, duration_ms: req.duration_ms }),
      create_time: now,
    });
  }

  async cancelCall(callId: string): Promise<void> {
    const call = await this.storage.remoteInvoke.getCall(callId);
    if (!call) throw new Error('call_not_found');

    const now = new Date().toISOString();
    await this.storage.remoteInvoke.updateCall(callId, {
      status: 'cancelled',
      ended_at: now,
    });

    pushToClient(call.client_instance_id, 'call_cancel', { call_id: callId });
    pushToCallerStream(callId, 'status', { call_id: callId, status: 'cancelled' });

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: callId,
      event_type: 'call_cancelled',
      seq: 0,
      direction: '',
      event_summary_json: '{}',
      create_time: now,
    });
  }

  async clientHeartbeat(req: ClientHeartbeatRequest): Promise<void> {
    const stream = getClientStream(req.client_instance_id);
    if (stream) {
      stream.lastHeartbeat = Date.now();
    }
    await this.storage.remoteInvoke.updateClientRecord(req.client_instance_id, {
      last_heartbeat_at: new Date().toISOString(),
    });
  }

  async listGrants(userId: string, query: GrantsQueryParams): Promise<{ list: RemoteInvokeGrant[]; total: number }> {
    return this.storage.remoteInvoke.listGrants(userId, query);
  }

  async updateGrant(userId: string, grantId: string, req: UpdateGrantRequest): Promise<void> {
    const grant = await this.storage.remoteInvoke.getGrant(grantId);
    if (!grant) throw new Error('grant_not_found');

    const fields: Partial<RemoteInvokeGrant> = { update_time: new Date().toISOString() };
    if (req.grant_mode) {
      fields.grant_mode = req.grant_mode;
      const ttl = grantModeTtlMs(req.grant_mode);
      if (ttl) {
        fields.expires_at = new Date(Date.now() + ttl).toISOString();
      } else if (req.grant_mode === 'permanent') {
        fields.expires_at = '';
      }
      if (req.grant_mode === 'once') {
        fields.max_calls = 1;
        fields.remaining_calls = 1;
      }
    }
    if (req.expires_at) {
      fields.expires_at = req.expires_at;
    }

    await this.storage.remoteInvoke.updateGrant(grantId, fields);

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: '',
      event_type: 'grant_updated',
      seq: 0,
      direction: '',
      event_summary_json: JSON.stringify({ grant_id: grantId, changes: fields }),
      create_time: new Date().toISOString(),
    });
  }

  async removeGrant(userId: string, grantId: string): Promise<void> {
    const grant = await this.storage.remoteInvoke.getGrant(grantId);
    if (!grant) throw new Error('grant_not_found');

    await this.storage.remoteInvoke.updateGrant(grantId, {
      status: 'removed',
      update_time: new Date().toISOString(),
    });

    pushToClient(grant.client_instance_id, 'grant_revoked', {
      grant_id: grantId,
    });

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: '',
      event_type: 'grant_removed',
      seq: 0,
      direction: '',
      event_summary_json: JSON.stringify({ grant_id: grantId }),
      create_time: new Date().toISOString(),
    });
  }

  async listCalls(userId: string, query: CallsQueryParams): Promise<{ list: RemoteInvokeCall[]; total: number }> {
    return this.storage.remoteInvoke.listCalls(userId, query);
  }

  async getCall(userId: string, callId: string): Promise<RemoteInvokeCall | null> {
    const call = await this.storage.remoteInvoke.getCall(callId);
    if (!call) return null;
    return call;
  }

  async listCallEvents(callId: string, query?: { offset?: number; limit?: number }): Promise<{ list: RemoteInvokeEvent[]; total: number }> {
    return this.storage.remoteInvoke.listCallEvents(callId, query);
  }

  async getOnlineClients(userId: string): Promise<Array<{
    client_instance_id: string;
    client_name: string;
    platform: string;
    online_status: string;
    discoverable: boolean;
    discovery_expires_at: number | undefined;
    last_heartbeat_at: number;
    active_grant_count: number;
  }>> {
    const result: Array<{
      client_instance_id: string;
      client_name: string;
      platform: string;
      online_status: string;
      discoverable: boolean;
      discovery_expires_at: number | undefined;
      last_heartbeat_at: number;
      active_grant_count: number;
    }> = [];

    for (const [cid, state] of getAllClientStreams()) {
      if (state.userId && state.userId !== userId) continue;

      const { list: grants } = await this.storage.remoteInvoke.listGrants(userId, {
        client_instance_id: cid,
        status: 'active',
      });

      result.push({
        client_instance_id: cid,
        client_name: cid,
        platform: '',
        online_status: 'online',
        discoverable: state.discoverable,
        discovery_expires_at: state.pairCodeExpiresAt,
        last_heartbeat_at: state.lastHeartbeat,
        active_grant_count: grants.length,
      });
    }

    return result;
  }

  async revokeAck(_grantId: string): Promise<void> {
  }
}
