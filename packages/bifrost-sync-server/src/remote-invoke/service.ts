import crypto from 'crypto';
import { customAlphabet } from 'nanoid';
import type { IStorage } from '../dao/types';
import type { RemoteInvokeConfig, RemoteInvokeGrant, RemoteInvokeCall, RemoteInvokeEvent, RemoteInvokePairing, RemoteInvokeClientRecord, GrantMode } from '../types';
import type {
  StartPairingRequest,
  GrantDecisionRequest,
  UpdateGrantRequest,
  PublishPairCodeRequest,
  ClientHeartbeatRequest,
  ClientCallFrameRequest,
  ClientCallStreamFrameRequest,
  ClientCallExitRequest,
  OpenCallRequestV5,
  GrantClaimRequest,
  GrantLookupRequest,
  GrantSessionResponse,
  GrantSummary,
  CallsQueryParams,
  ClientRegistrationChallengeResponse,
  ClientRegistrationRequest,
  SshConnectRequest,
  SshConnectResponse,
  SshConnectResultRequest,
  RemoteCommandKind,
} from './types';
import {
  buildRegistrationSignaturePayload,
  grantModeTtlMs,
  normalizeGrantScope,
  normalizeFileAccess,
  resolveCommandKind,
  grantScopeAllowsCommand,
} from './types';
import { SshAuthService } from './ssh-auth';
import { ed25519FingerprintFromBase64 } from './pop';
import {
  pushToClient,
  pushReplaySafeToClient,
  getClientStream,
  updateClientDiscovery,
  clearClientDiscovery,
  consumeClientDiscovery,
  findClientByPairCode,
  pushToPairingWatcher,
  pushToCallerStream,
  clearCallerEventBuffer,
  clearReplaySafeClientEvents,
} from './sse';

const nanoid = customAlphabet('0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_', 21);
const REGISTER_CHALLENGE_TTL_MS = 60_000;
const REGISTER_TIMESTAMP_SKEW_MS = 5 * 60_000;
const CALL_TOKEN_GRACE_MS = 60_000;
const SHELL_CALL_TOKEN_GRACE_MS = 24 * 60 * 60 * 1000;
const SHELL_VIEWER_RESUME_TTL_MS = 10 * 60 * 1000;
const SHELL_RETENTION_TTL_MS = 24 * 60 * 60 * 1000;
const SSH_CONNECT_STREAM_WAIT_MS = 10_000;
const SSH_CONNECT_STREAM_POLL_MS = 100;
const CLAIM_TOKEN_TTL_MS = 180_000;
const GRANT_SESSION_TOKEN_TTL_MS = 30 * 60_000;

interface CallRuntimeMeta {
  commandKind: RemoteCommandKind;
  viewerResumeTtlMs: number;
  retentionTtlMs: number;
  relayTokenTtlMs: number;
  timeoutHintMs: number;
  ptyEnabled: boolean;
}

function generateRelayToken(): string {
  return crypto.randomBytes(32).toString('hex');
}

function randomToken(): string {
  return crypto.randomBytes(32).toString('hex');
}

function sha256Hex(value: string): string {
  return crypto.createHash('sha256').update(value, 'utf8').digest('hex');
}

function computeStoredPubkeyHash(pubkeyDer: Buffer): string {
  return crypto.createHash('sha256').update(pubkeyDer).digest('hex');
}

function constantTimeCompare(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  const bufA = Buffer.from(a, 'utf8');
  const bufB = Buffer.from(b, 'utf8');
  return crypto.timingSafeEqual(bufA, bufB);
}

function clampSshGrantMode(mode: string | undefined): GrantMode {
  // SSH-key grants are target-approved. Preserve the target's explicit mode
  // so default Full Trust remains reusable; fall back to once for malformed
  // or missing relay payloads.
  const allowed: GrantMode[] = ['once', '30m', '1h', '1d', 'permanent'];
  if (mode && (allowed as string[]).includes(mode)) return mode as GrantMode;
  return 'once';
}
function isoTimestampToMillis(value: string | undefined | null): number | null {
  if (!value) return null;
  const millis = new Date(value).getTime();
  return Number.isNaN(millis) ? null : millis;
}

export class RemoteInvokeService {
  private callTokens = new Map<string, { token: string; expiresAt?: number }>();
  private callRuntimeMeta = new Map<string, CallRuntimeMeta>();
  private sshAuth = new SshAuthService();
  private registrationChallenges = new Map<
    string,
    {
      clientInstanceId: string;
      userId: string;
      challenge: string;
      expiresAt: number;
    }
  >();

  constructor(
    private storage: IStorage,
    private config: RemoteInvokeConfig,
  ) { }

  issueRegistrationChallenge(
    userId: string,
    clientInstanceId: string,
  ): ClientRegistrationChallengeResponse {
    this.cleanupExpiredRegistrationChallenges();
    const challengeId = nanoid();
    const challenge = generateRelayToken();
    const expiresAt = Date.now() + REGISTER_CHALLENGE_TTL_MS;
    this.registrationChallenges.set(challengeId, {
      clientInstanceId,
      userId,
      challenge,
      expiresAt,
    });
    return {
      challenge_id: challengeId,
      challenge,
      expires_at: expiresAt,
      algorithm: 'ed25519',
    };
  }

  async registerClient(
    userId: string,
    req: ClientRegistrationRequest,
  ): Promise<{ client_auth_token: string; expires_at: string }> {
    const challengeEntry = this.registrationChallenges.get(req.challenge_id);
    if (!challengeEntry) {
      throw new Error('registration_challenge_not_found');
    }
    this.registrationChallenges.delete(req.challenge_id);
    if (challengeEntry.expiresAt < Date.now()) {
      throw new Error('registration_challenge_expired');
    }
    if (challengeEntry.clientInstanceId !== req.client_instance_id) {
      throw new Error('registration_client_instance_id_mismatch');
    }
    if (challengeEntry.userId !== userId) {
      throw new Error('registration_user_mismatch');
    }
    if (!Number.isFinite(req.timestamp)) {
      throw new Error('invalid_registration_timestamp');
    }
    const timestampMs = req.timestamp * 1000;
    if (Math.abs(Date.now() - timestampMs) > REGISTER_TIMESTAMP_SKEW_MS) {
      throw new Error('registration_timestamp_out_of_window');
    }

    const pubkeyDer = decodeBase64(req.client_long_term_pubkey, 'client_long_term_pubkey');
    const signature = decodeBase64(req.signature, 'signature');
    const payload = buildRegistrationSignaturePayload(
      req.challenge_id,
      challengeEntry.challenge,
      req.client_instance_id,
      req.device_name,
      req.platform,
      req.bifrost_version,
      req.client_long_term_pubkey,
      req.timestamp,
    );
    const publicKey = crypto.createPublicKey({
      key: pubkeyDer,
      format: 'der',
      type: 'spki',
    });
    const verified = crypto.verify(
      null,
      Buffer.from(payload, 'utf8'),
      publicKey,
      signature,
    );
    if (!verified) {
      throw new Error('invalid_registration_signature');
    }

    const existing = await this.storage.remoteInvoke.getClientRecord(req.client_instance_id);
    if (existing) {
      if (existing.user_id && existing.user_id !== userId) {
        throw new Error('client_instance_id_owned_by_another_user');
      }
      const pubkeyHash = computeStoredPubkeyHash(pubkeyDer);
      if (existing.client_pubkey_hash && existing.client_pubkey_hash !== pubkeyHash) {
        throw new Error('pubkey mismatch for existing client_instance_id');
      }
    }

    const now = new Date().toISOString();
    const pubkeyHash = computeStoredPubkeyHash(pubkeyDer);
    const existingTokenStillValid = !!existing?.client_auth_token
      && !!existing.token_expires_at
      && new Date(existing.token_expires_at) > new Date();
    const token = existingTokenStillValid ? existing!.client_auth_token : generateRelayToken();
    const expiresAt = existingTokenStillValid
      ? existing!.token_expires_at
      : new Date(Date.now() + 30 * 24 * 60 * 60 * 1000).toISOString();

    const record: RemoteInvokeClientRecord = {
      client_instance_id: req.client_instance_id,
      user_id: userId,
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
    // Keep SSH route sync three-state:
    // omitted = client could not read local key store, do not touch relay route;
    // null = client has no active SSH key, clear relay route;
    // object = publish/update active route.
    if (Object.prototype.hasOwnProperty.call(req, 'ssh_device_route')) {
      const routeState = this.sshAuth.syncSshRoute(req.client_instance_id, userId, req.ssh_device_route ?? null);
      if (routeState.routeChanged) {
        await this.storage.remoteInvoke.revokeSshGrantsForClient(req.client_instance_id);
      }
    }
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

  async closeDiscoverySession(clientInstanceId: string, discoverySessionId: string): Promise<void> {
    if (clearClientDiscovery(clientInstanceId, discoverySessionId)) {
      await this.cancelPendingPairings(clientInstanceId);
    }
  }

  private isPendingPairingExpired(pairing: RemoteInvokePairing, nowMs = Date.now()): boolean {
    if (pairing.status !== 'pending_approval') {
      return false;
    }
    const expiresAtMs = isoTimestampToMillis(pairing.expires_at);
    return expiresAtMs !== null && expiresAtMs <= nowMs;
  }

  private async expirePendingPairing(pairing: RemoteInvokePairing, reason = 'pairing_expired'): Promise<void> {
    const now = new Date().toISOString();
    await this.storage.remoteInvoke.updatePairing(pairing.id, {
      status: 'expired',
      update_time: now,
    });

    pushToPairingWatcher(pairing.id, 'expired', {
      pairing_id: pairing.id,
      reason,
    });

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: '',
      event_type: 'pairing_expired',
      seq: 0,
      direction: '',
      event_summary_json: JSON.stringify({ pairing_id: pairing.id, reason }),
      create_time: now,
    });
  }

  private async loadActivePendingPairings(clientInstanceId: string): Promise<RemoteInvokePairing[]> {
    const nowMs = Date.now();
    const pairings = await this.storage.remoteInvoke.listPendingPairings(clientInstanceId);
    const active: RemoteInvokePairing[] = [];

    for (const pairing of pairings) {
      if (this.isPendingPairingExpired(pairing, nowMs)) {
        await this.expirePendingPairing(pairing);
        continue;
      }
      active.push(pairing);
    }

    return active;
  }

  async getPendingPairingsForClient(clientInstanceId: string): Promise<Array<{
    pairing_id: string;
    caller_fingerprint: string;
    caller_display_name: string;
    caller_info: any;
    source_ip: string;
    user_agent: string;
    status: string;
    expires_at: string;
  }>> {
    const pairings = await this.loadActivePendingPairings(clientInstanceId);
    return pairings.map((p) => {
      let callerInfo: any = {};
      try { callerInfo = JSON.parse(p.caller_info_json || '{}'); } catch { /* ignore */ }
      return {
        pairing_id: p.id,
        caller_fingerprint: p.caller_fingerprint || callerInfo.fingerprint || '',
        caller_display_name: callerInfo.display_name || '',
        caller_info: callerInfo,
        caller_ephemeral_pub: p.caller_ephemeral_pub ?? '',
        caller_ephemeral_sig: p.caller_ephemeral_sig ?? '',
        source_ip: callerInfo.source_ip || '',
        user_agent: callerInfo.user_agent || '',
        status: p.status,
        expires_at: p.expires_at,
      };
    });
  }

  async cancelPendingPairings(clientInstanceId: string): Promise<number> {
    const pairings = await this.loadActivePendingPairings(clientInstanceId);
    for (const p of pairings) {
      pushToPairingWatcher(p.id, 'rejected', {
        pairing_id: p.id,
        reason: 'cancelled_by_client',
      });
    }
    return this.storage.remoteInvoke.cancelPendingPairings(clientInstanceId);
  }

  async startPairing(userId: string, req: StartPairingRequest, sourceIp: string): Promise<{ pairing_id: string; status: string; watch_token: string; client_instance_id: string }> {
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

    const resolvedClientId = clientStream.clientInstanceId;

    const pendingPairings = await this.loadActivePendingPairings(resolvedClientId);
    if (pendingPairings.length > 0) {
      throw new Error('pair_slot_occupied');
    }

    // P0-4: caller_pubkey is required, and the server derives the canonical fingerprint.
    const callerPubkey = (req.caller_pubkey ?? '').trim();
    if (!callerPubkey) {
      throw new Error('caller_pubkey_required');
    }
    let derivedCallerFp = '';
    try {
      derivedCallerFp = ed25519FingerprintFromBase64(callerPubkey);
    } catch {
      throw new Error('caller_pubkey_invalid');
    }

    const pairingId = nanoid();
    const watchToken = randomToken();
    const now = new Date().toISOString();
    const expiresAt = new Date(Date.now() + this.config.pair_code_ttl_secs * 1000).toISOString();

    const pairing: RemoteInvokePairing = {
      id: pairingId,
      user_id: userId,
      client_instance_id: resolvedClientId,
      caller_fingerprint: derivedCallerFp,
      pair_code: req.pair_code,
      status: 'pending_approval',
      caller_pubkey: callerPubkey,
      caller_ephemeral_pub: req.caller_ephemeral_pub ?? '',
      caller_ephemeral_sig: req.caller_ephemeral_sig ?? '',
      client_ephemeral_pub: '',
      caller_info_json: JSON.stringify({ ...req.caller_info, fingerprint: derivedCallerFp }),
      command_summary_json: '{}',
      command_json: '{}',
      relay_token: '',
      call_id: '',
      grant_id: '',
      watch_token_hash: sha256Hex(watchToken),
      claim_token_hash: '',
      claim_expires_at: '',
      claimed_at: '',
      expires_at: expiresAt,
      create_time: now,
      update_time: now,
    };

    await this.storage.remoteInvoke.createPairing(pairing);

    consumeClientDiscovery(resolvedClientId);

    pushToClient(resolvedClientId, 'pairing_request', {
      pairing_id: pairingId,
      caller_fingerprint: derivedCallerFp,
      caller_display_name: req.caller_info.display_name ?? '',
      caller_info: { ...req.caller_info, fingerprint: derivedCallerFp },
      caller_ephemeral_pub: req.caller_ephemeral_pub ?? '',
      caller_ephemeral_sig: req.caller_ephemeral_sig ?? '',
      source_ip: sourceIp,
      user_agent: req.caller_info.user_agent ?? '',
      expires_at: expiresAt,
    });

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: '',
      event_type: 'pairing_created',
      seq: 0,
      direction: '',
      event_summary_json: JSON.stringify({ pairing_id: pairingId, caller_fingerprint: derivedCallerFp }),
      create_time: now,
    });

    return { pairing_id: pairingId, status: 'pending_approval', watch_token: watchToken, client_instance_id: resolvedClientId };
  }

  async submitGrantDecision(userId: string, req: GrantDecisionRequest): Promise<{ status: string; grant_id?: string; client_instance_id?: string; device_name?: string; platform?: string; caller_fingerprint?: string; caller_display_name?: string; grant_mode?: string; grant_scope?: string; file_access?: string; claim_token?: string; claim_expires_at?: string; grant_summary?: GrantSummary }> {
    const pairing = await this.storage.remoteInvoke.getPairing(req.pairing_id);
    if (!pairing) throw new Error('pairing_not_found');
    if (this.isPendingPairingExpired(pairing)) {
      await this.expirePendingPairing(pairing);
      throw new Error('pairing_expired');
    }
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
    if (!pairing.caller_ephemeral_pub) {
      throw new Error('caller_ephemeral_pub_required');
    }
    if (!req.client_ephemeral_pub) {
      throw new Error('client_ephemeral_pub_required');
    }
    const grantId = nanoid();

    await this.storage.remoteInvoke.revokeActiveGrantsForCaller(
      pairing.client_instance_id,
      pairing.caller_fingerprint,
    );

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
      caller_ephemeral_pub: pairing.caller_ephemeral_pub,
      client_ephemeral_pub: req.client_ephemeral_pub,
      grant_mode: grantMode as any,
      grant_scope: normalizeGrantScope(req.grant_scope),
      file_access: normalizeFileAccess(req.file_access),
      ssh_key_id: '',
      ssh_key_fingerprint: '',
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

    await this.storage.remoteInvoke.updatePairing(pairing.id, {
      status: 'approved',
      client_ephemeral_pub: req.client_ephemeral_pub ?? '',
      grant_id: grantId,
      update_time: now,
    });

    const clientRecord = await this.storage.remoteInvoke.getClientRecord(pairing.client_instance_id);
    const deviceName = clientRecord?.client_name || pairing.client_instance_id;
    const platform = clientRecord?.platform || '';

    const claim = await this.mintClaimToken(pairing.id);
    const grantSummary = this.toGrantSummary(grant);

    pushToPairingWatcher(pairing.id, 'approved', {
      type: 'approved',
      claim_token: claim.claim_token,
      claim_expires_at: claim.claim_expires_at,
      grant_summary: grantSummary,
    });

    let callerInfoObj: any = {};
    try { callerInfoObj = JSON.parse(pairing.caller_info_json || '{}'); } catch { /* ignore */ }

    pushToClient(pairing.client_instance_id, 'grant_created', {
      grant_id: grantId,
      caller_fingerprint: pairing.caller_fingerprint,
      caller_display_name: callerDisplayName,
      caller_info: callerInfoObj,
      grant_mode: grantMode,
      grant_scope: normalizeGrantScope(req.grant_scope),
      file_access: normalizeFileAccess(req.file_access),
      caller_ephemeral_pub: pairing.caller_ephemeral_pub,
      client_ephemeral_pub: req.client_ephemeral_pub,
    });

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: '',
      event_type: 'pairing_approved',
      seq: 0,
      direction: '',
      event_summary_json: JSON.stringify({ pairing_id: pairing.id, grant_mode: grantMode }),
      create_time: now,
    });

    return {
      status: 'approved',
      grant_id: grantId,
      client_instance_id: pairing.client_instance_id,
      device_name: deviceName,
      platform,
      caller_fingerprint: pairing.caller_fingerprint,
      caller_display_name: callerDisplayName,
      grant_mode: grantMode,
      grant_scope: normalizeGrantScope(req.grant_scope),
      file_access: normalizeFileAccess(req.file_access),
      claim_token: claim.claim_token,
      claim_expires_at: claim.claim_expires_at,
      grant_summary: grantSummary,
    };
  }

  async mintClaimToken(pairingId: string): Promise<{ claim_token: string; claim_expires_at: string }> {
    const pairing = await this.storage.remoteInvoke.getPairing(pairingId);
    if (!pairing) throw new Error('pairing_not_found');
    const claimToken = randomToken();
    const claimExpiresAt = new Date(Date.now() + CLAIM_TOKEN_TTL_MS).toISOString();
    await this.storage.remoteInvoke.setPairingClaimTokens(
      pairingId,
      sha256Hex(claimToken),
      pairing.watch_token_hash ?? '',
      claimExpiresAt,
    );
    return { claim_token: claimToken, claim_expires_at: claimExpiresAt };
  }

  async redeemClaim(req: GrantClaimRequest, callerPubkeyFp: string): Promise<GrantSessionResponse> {
    const pairing = await this.storage.remoteInvoke.getPairingByClaimTokenHash(sha256Hex(req.claim_token));
    if (!pairing) throw new Error('claim_token_invalid');
    if (pairing.claimed_at) throw new Error('claim_token_invalid');
    if (pairing.claim_expires_at && new Date(pairing.claim_expires_at) < new Date()) {
      throw new Error('claim_token_invalid');
    }
    if (!constantTimeCompare(pairing.pair_code, req.pair_code)) {
      throw new Error('claim_token_invalid');
    }
    if (pairing.client_instance_id !== req.client_instance_id) {
      throw new Error('client_instance_id_mismatch');
    }
    if (pairing.status !== 'approved' || !pairing.grant_id) {
      throw new Error('claim_token_invalid');
    }
    // P0-4: redeem must be performed by the same caller_pubkey that started the pairing,
    // and the PoP envelope's caller pubkey must derive to that fingerprint.
    if (!pairing.caller_pubkey) {
      throw new Error('pairing_caller_pubkey_missing');
    }
    if (!constantTimeCompare(pairing.caller_pubkey, req.caller_pubkey)) {
      throw new Error('caller_pubkey_mismatch');
    }
    const expectedFp = ed25519FingerprintFromBase64(pairing.caller_pubkey);
    if (!constantTimeCompare(expectedFp, callerPubkeyFp)) {
      throw new Error('caller_pubkey_mismatch');
    }
    const grant = await this.storage.remoteInvoke.getGrant(pairing.grant_id);
    if (!grant) throw new Error('grant_not_found');
    await this.storage.remoteInvoke.updateGrantCallerPubkey(grant.id, req.caller_pubkey, callerPubkeyFp);
    await this.storage.remoteInvoke.updateGrantCallerEphemeralPub(grant.id, req.caller_ephemeral_pub);
    await this.storage.remoteInvoke.markPairingClaimed(pairing.id, new Date().toISOString());
    const updated = await this.storage.remoteInvoke.getGrant(grant.id);
    if (!updated) throw new Error('grant_not_found');
    return this.mintGrantSessionToken(updated.id);
  }

  async redeemSshClaim(
    req: { claim_token: string; client_instance_id: string; caller_pubkey: string; caller_ephemeral_pub?: string },
    callerPubkeyFp: string,
  ): Promise<GrantSessionResponse> {
    const claim = await this.storage.remoteInvoke.getSshClaimByTokenHash(sha256Hex(req.claim_token));
    if (!claim) throw new Error('claim_token_invalid');
    if (claim.claimed_at) throw new Error('claim_token_invalid');
    if (claim.expires_at && new Date(claim.expires_at) < new Date()) {
      throw new Error('claim_token_invalid');
    }
    if (claim.client_instance_id !== req.client_instance_id) {
      throw new Error('client_instance_id_mismatch');
    }
    // P0-3 + P0-4: PoP pubkey fingerprint must match the fingerprint frozen at SSH approval time.
    const expectedFp = ed25519FingerprintFromBase64(req.caller_pubkey);
    if (!constantTimeCompare(expectedFp, callerPubkeyFp)) {
      throw new Error('caller_pubkey_mismatch');
    }
    if (claim.caller_pubkey_fp && !constantTimeCompare(claim.caller_pubkey_fp, callerPubkeyFp)) {
      throw new Error('caller_pubkey_mismatch');
    }
    const grant = await this.storage.remoteInvoke.getGrant(claim.grant_id);
    if (!grant) throw new Error('grant_not_found');
    if (grant.status !== 'active') throw new Error('grant_not_found');
    if (req.caller_ephemeral_pub) {
      await this.storage.remoteInvoke.updateGrantCallerEphemeralPub(grant.id, req.caller_ephemeral_pub);
    }
    await this.storage.remoteInvoke.markSshClaimRedeemed(claim.claim_token_hash, new Date().toISOString());
    return this.mintGrantSessionToken(grant.id);
  }
  async lookupGrantSession(req: GrantLookupRequest, callerPubkeyFp: string): Promise<GrantSessionResponse> {
    const grant = await this.storage.remoteInvoke.getGrantByCallerFp(callerPubkeyFp, req.client_instance_id);
    if (!grant || grant.revoked_at || grant.status !== 'active') {
      throw new Error('grant_not_found');
    }
    if (grant.expires_at && new Date(grant.expires_at) < new Date()) {
      await this.storage.remoteInvoke.updateGrant(grant.id, { status: 'expired' });
      throw new Error('grant_not_found');
    }
    if (grant.caller_ephemeral_pub) {
      // P0-2: an established ephemeral_pub is frozen for the lifetime of the grant.
      if (grant.caller_ephemeral_pub !== req.caller_ephemeral_pub) {
        throw new Error('ephemeral_pub_rotation_not_allowed');
      }
    } else if (req.caller_ephemeral_pub) {
      // First-time set: only allowed when the grant has no ephemeral_pub yet.
      await this.storage.remoteInvoke.updateGrantCallerEphemeralPub(grant.id, req.caller_ephemeral_pub);
    }
    return this.mintGrantSessionToken(grant.id);
  }

  async mintGrantSessionToken(grantId: string): Promise<GrantSessionResponse> {
    const grant = await this.storage.remoteInvoke.getGrant(grantId);
    if (!grant) throw new Error('grant_not_found');
    const token = randomToken();
    const expiresAt = new Date(Date.now() + GRANT_SESSION_TOKEN_TTL_MS).toISOString();
    await this.storage.remoteInvoke.updateGrantSessionToken(grantId, sha256Hex(token), expiresAt);
    return {
      grant_session_token: token,
      expires_at: expiresAt,
      grant_summary: this.toGrantSummary(grant, true),
    };
  }

  async resolveGrantBySessionToken(bearer: string): Promise<RemoteInvokeGrant | null> {
    const grant = await this.storage.remoteInvoke.getGrantBySessionTokenHash(sha256Hex(bearer));
    if (!grant) return null;
    if (grant.revoked_at) return null;
    if (grant.status !== 'active') return null;
    if (grant.session_token_expires_at && new Date(grant.session_token_expires_at) < new Date()) {
      return null;
    }
    return grant;
  }

  async revokeGrantByBearer(bearer: string, callerFp: string): Promise<void> {
    const grant = await this.resolveGrantBySessionToken(bearer);
    if (!grant) throw new Error('grant_session_token_invalid');
    if (grant.caller_pubkey_fp !== callerFp) throw new Error('caller_pubkey_mismatch');
    const revokedAt = new Date().toISOString();
    await this.storage.remoteInvoke.revokeGrant(grant.id, revokedAt);
    pushToClient(grant.client_instance_id, 'grant_revoked', {
      grant_id: grant.id,
    });
  }

  async listActiveGrantsForClient(clientInstanceId: string): Promise<RemoteInvokeGrant[]> {
    const grants = await this.storage.remoteInvoke.listActiveGrantsForClient(clientInstanceId);
    const now = new Date();
    const active: RemoteInvokeGrant[] = [];
    for (const g of grants) {
      if (g.expires_at && new Date(g.expires_at) < now) {
        await this.storage.remoteInvoke.updateGrant(g.id, { status: 'expired' });
        continue;
      }
      if (g.grant_mode === 'once' && g.remaining_calls <= 0) {
        await this.storage.remoteInvoke.updateGrant(g.id, { status: 'consumed' });
        continue;
      }
      active.push(g);
    }
    return active;
  }

  async openCall(
    grant: RemoteInvokeGrant,
    req: OpenCallRequestV5,
  ): Promise<{
    call_id: string;
    relay_token: string;
    call_meta: {
      command_kind: RemoteCommandKind;
      pty_enabled: boolean;
      timeout_hint_ms: number;
      viewer_resume_ttl_ms: number;
      retention_ttl_ms: number;
      relay_token_ttl_ms: number;
    };
  }> {
    if (grant.client_instance_id !== req.client_instance_id) {
      throw new Error('client_instance_id_mismatch');
    }

    if (grant.status !== 'active') throw new Error('grant_not_active');

    if (grant.expires_at && new Date(grant.expires_at) < new Date()) {
      await this.storage.remoteInvoke.updateGrant(grant.id, { status: 'expired' });
      throw new Error('grant_expired');
    }

    if (grant.remaining_calls <= 0) {
      await this.storage.remoteInvoke.updateGrant(grant.id, { status: 'consumed' });
      throw new Error('grant_consumed');
    }

    const commandKind = resolveCommandKind(req.command_kind);
    if (!grantScopeAllowsCommand(grant.grant_scope, commandKind, grant.file_access)) {
      throw new Error('grant_scope_mismatch');
    }

    if (!req.command_encrypted) {
      throw new Error('command_encrypted_required');
    }

    if (!grant.caller_ephemeral_pub) {
      throw new Error('caller_ephemeral_pub_required');
    }

    if (!grant.client_ephemeral_pub) {
      throw new Error('client_ephemeral_pub_required');
    }

    const ptyEnabled = !!req.pty_enabled;
    const timeoutHintMs = Number.isFinite(req.timeout_hint_ms) && (req.timeout_hint_ms ?? 0) > 0
      ? Math.trunc(req.timeout_hint_ms!)
      : 0;
    const viewerResumeTtlMs = commandKind === 'shell.exec' ? SHELL_VIEWER_RESUME_TTL_MS : CALL_TOKEN_GRACE_MS;
    const retentionTtlMs = commandKind === 'shell.exec' ? SHELL_RETENTION_TTL_MS : CALL_TOKEN_GRACE_MS;
    const relayTokenTtlMs = commandKind === 'shell.exec'
      ? Math.max(timeoutHintMs * 2, SHELL_CALL_TOKEN_GRACE_MS)
      : Math.max(timeoutHintMs * 2, CALL_TOKEN_GRACE_MS);
    const commandSummary = {
      ...(req.command_summary ?? { command_preview: commandKind }),
      command_kind: commandKind,
      encrypted_payload_present: true,
      pty_enabled: ptyEnabled,
      timeout_hint_ms: timeoutHintMs,
      viewer_resume_ttl_ms: viewerResumeTtlMs,
      retention_ttl_ms: retentionTtlMs,
      relay_token_ttl_ms: relayTokenTtlMs,
    };

    const requestedCallId = typeof req.call_id === 'string' ? req.call_id.trim() : '';
    if (requestedCallId && !/^[A-Za-z0-9_-]{8,80}$/.test(requestedCallId)) {
      throw new Error('invalid_call_id');
    }
    const callId = requestedCallId || nanoid();
    const relayToken = generateRelayToken();
    this.callTokens.set(callId, { token: relayToken });
    this.callRuntimeMeta.set(callId, {
      commandKind,
      viewerResumeTtlMs,
      retentionTtlMs,
      relayTokenTtlMs,
      timeoutHintMs,
      ptyEnabled,
    });
    const now = new Date().toISOString();
    const storedCommand = { kind: commandKind };

    const call: RemoteInvokeCall = {
      id: callId,
      user_id: grant.user_id,
      grant_id: grant.id,
      pairing_id: '',
      client_instance_id: req.client_instance_id,
      caller_fingerprint: grant.caller_fingerprint,
      source_ip: '',
      caller_display_name: grant.caller_display_name || '',
      status: 'authorized',
      command_summary_json: JSON.stringify(commandSummary),
      command_json: JSON.stringify(storedCommand),
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
    const consumed = await this.storage.remoteInvoke.consumeGrantCall(grant.id);
    if (!consumed) {
      const latestGrant = await this.storage.remoteInvoke.getGrant(grant.id);
      if (latestGrant?.status === 'active' && latestGrant.remaining_calls <= 0) {
        await this.storage.remoteInvoke.updateGrant(grant.id, { status: 'consumed' });
      }
      throw new Error('grant_consumed');
    }

    await this.storage.remoteInvoke.createCall(call);
    await this.storage.remoteInvoke.touchGrantLastUsed(grant.id, now);

    pushToClient(req.client_instance_id, 'call_open', {
      call_id: callId,
      grant_id: grant.id,
      grant_scope: normalizeGrantScope(grant.grant_scope),
      file_access: normalizeFileAccess(grant.file_access),
      caller_fingerprint: grant.caller_fingerprint,
      ssh_key_fingerprint: grant.ssh_key_fingerprint || '',
      caller_pubkey: req.caller_pubkey ?? grant.caller_pubkey ?? '',
      caller_ephemeral_pub: grant.caller_ephemeral_pub,
      client_ephemeral_pub: grant.client_ephemeral_pub,
      command_kind: commandKind,
      command_encrypted: req.command_encrypted,
      command_summary: commandSummary,
      pty_enabled: ptyEnabled,
      timeout_hint_ms: timeoutHintMs,
      viewer_resume_ttl_ms: viewerResumeTtlMs,
      retention_ttl_ms: retentionTtlMs,
      relay_token_ttl_ms: relayTokenTtlMs,
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

    return {
      call_id: callId,
      relay_token: relayToken,
      call_meta: {
        command_kind: commandKind,
        pty_enabled: ptyEnabled,
        timeout_hint_ms: timeoutHintMs,
        viewer_resume_ttl_ms: viewerResumeTtlMs,
        retention_ttl_ms: retentionTtlMs,
        relay_token_ttl_ms: relayTokenTtlMs,
      },
    };
  }

  private toGrantSummary(grant: RemoteInvokeGrant, includeCryptoContext = false): GrantSummary {
    const summary: GrantSummary = {
      scope: normalizeGrantScope(grant.grant_scope),
      mode: grant.grant_mode,
      file_access: normalizeFileAccess(grant.file_access),
    };
    if (includeCryptoContext && grant.client_ephemeral_pub) {
      summary.client_ephemeral_pub = grant.client_ephemeral_pub;
    }
    return summary;
  }

  verifyCallToken(callId: string, token: string): boolean {
    this.cleanupExpiredCallToken(callId);
    const stored = this.callTokens.get(callId);
    if (!stored) return false;
    return constantTimeCompare(stored.token, token);
  }

  clearCallToken(callId: string): void {
    this.callTokens.delete(callId);
    this.callRuntimeMeta.delete(callId);
    clearCallerEventBuffer(callId);
    clearReplaySafeClientEvents(callId);
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

    const accepted = pushReplaySafeToClient(call.client_instance_id, callId, 'call_frame', {
      call_id: callId,
      envelope_json: envelopeJson,
    });
    if (!accepted) {
      throw new Error('target_stream_unavailable');
    }

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
    if (call.client_instance_id !== req.client_instance_id) {
      throw new Error('client_mismatch');
    }

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

  /**
   * PR#6a: forward a worker-side StreamFrame to the caller SSE as an
   * `stream_frame` event. Payload is the raw StreamFrame JSON string.
   */
  async postClientStreamFrame(req: ClientCallStreamFrameRequest): Promise<void> {
    const call = await this.storage.remoteInvoke.getCall(req.call_id);
    if (!call) throw new Error('call_not_found');
    if (call.client_instance_id !== req.client_instance_id) {
      throw new Error('client_mismatch');
    }

    // PR#6a-followup: forward frame_json verbatim as inner string so the
    // caller SSE consumer parses a single well-formed JSON object. Re-parsing
    // here and letting pushToCallerStream JSON.stringify a second time both
    // risks floating-point precision loss and breaks the {call_id, frame_json}
    // contract asserted by remote-invoke-stream-frame.test.ts.
    pushToCallerStream(req.call_id, 'stream_frame', {
      call_id: req.call_id,
      frame_json: req.frame_json,
    });

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: req.call_id,
      event_type: 'call_stream_frame_out',
      seq: 0,
      direction: 'client_to_caller',
      event_summary_json: JSON.stringify({ size: req.frame_json.length }),
      create_time: new Date().toISOString(),
    });
  }

  async postClientExit(req: ClientCallExitRequest): Promise<void> {
    const call = await this.storage.remoteInvoke.getCall(req.call_id);
    if (!call) throw new Error('call_not_found');
    if (call.client_instance_id !== req.client_instance_id) {
      throw new Error('client_mismatch');
    }

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
      stderr: req.stderr,
      stdout_digest: req.stdout_digest,
      stderr_digest: req.stderr_digest,
      exit_encrypted: req.exit_encrypted,
    });
    clearReplaySafeClientEvents(req.call_id);

    const grant = await this.storage.remoteInvoke.getGrant(call.grant_id);
    if (grant && grant.grant_mode === 'once' && grant.remaining_calls <= 0) {
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

    this.expireCallToken(req.call_id, this.callRuntimeMeta.get(req.call_id)?.relayTokenTtlMs);
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
    clearReplaySafeClientEvents(callId);

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: callId,
      event_type: 'call_cancelled',
      seq: 0,
      direction: '',
      event_summary_json: '{}',
      create_time: now,
    });

    this.expireCallToken(callId, this.callRuntimeMeta.get(callId)?.relayTokenTtlMs);
  }

  async clientHeartbeat(req: ClientHeartbeatRequest): Promise<void> {
    const stream = getClientStream(req.client_instance_id);
    if (stream) {
      stream.lastHeartbeat = Date.now();
    }
    await this.storage.remoteInvoke.updateClientRecord(req.client_instance_id, {
      last_heartbeat_at: new Date().toISOString(),
    });
    // Keep SSH route sync three-state. See registerClient for the protocol
    // meaning of omitted vs null vs object.
    if (Object.prototype.hasOwnProperty.call(req, 'ssh_device_route')) {
      // P0-1: the heartbeat route owner must match the client record's user_id.
      const record = await this.storage.remoteInvoke.getClientRecord(req.client_instance_id);
      const ownerUserId = record?.user_id ?? '';
      const routeState = this.sshAuth.syncSshRoute(req.client_instance_id, ownerUserId, req.ssh_device_route ?? null);
      if (routeState.routeChanged) {
        await this.storage.remoteInvoke.revokeSshGrantsForClient(req.client_instance_id);
      }
    }
  }

  issueSshChallenge(deviceCode: string) {
    return this.sshAuth.issueChallenge(deviceCode);
  }

  async verifySshConnect(req: SshConnectRequest): Promise<SshConnectResponse> {
    const verified = this.sshAuth.verifyAndPrepareConnect(req);
    const eventPayload = {
      connect_id: verified.connect_id,
      device_code: req.device_code,
      ssh_key_fingerprint: verified.ssh_key_fingerprint,
      caller_ephemeral_pub: req.caller_ephemeral_pub ?? '',
      caller_info: req.caller_info ?? {},
      relay_verified: true,
    };
    let delivered = pushToClient(verified.client_instance_id, 'ssh_connect', eventPayload);
    if (!delivered) {
      await this.waitForClientStream(verified.client_instance_id, SSH_CONNECT_STREAM_WAIT_MS);
      delivered = pushToClient(verified.client_instance_id, 'ssh_connect', eventPayload);
    }
    if (!delivered) {
      throw new Error('client_offline');
    }
    this.callTokens.set(verified.connect_id, { token: verified.relay_token });
    return {
      connect_id: verified.connect_id,
      relay_token: verified.relay_token,
      expires_at: verified.expires_at,
    };
  }

  async submitSshConnectResult(
    clientInstanceId: string,
    req: SshConnectResultRequest,
  ): Promise<void> {
    const result = this.sshAuth.completeConnect(clientInstanceId, req);
    let effectiveCallerFingerprint = result.caller_fingerprint;
    let claimToken: string | null = null;
    let claimExpiresAt: string | null = null;
    let grantIdForClaim: string | null = null;
    if (result.status === 'approved' && result.grant_id) {
      const now = new Date().toISOString();
      // Preserve target-approved SSH grant mode; invalid relay payloads
      // are clamped by clampSshGrantMode.
      const grantMode = clampSshGrantMode(req.grant_mode);
      const callerPubkey = result.caller_info?.caller_pubkey || '';
      if (!callerPubkey) {
        throw new Error('ssh_caller_pubkey_required');
      }
      let derivedFp = '';
      try {
        derivedFp = ed25519FingerprintFromBase64(callerPubkey);
      } catch {
        throw new Error('ssh_caller_pubkey_invalid');
      }
      effectiveCallerFingerprint = derivedFp;
      const callerDisplayName =
        result.caller_info?.hostname ||
        result.caller_info?.username ||
        '';

      await this.storage.remoteInvoke.revokeActiveGrantsForCaller(
        clientInstanceId,
        effectiveCallerFingerprint,
      );

      const maxCalls = Math.max(1, Math.floor(this.config.ssh_grant_max_calls ?? 1000));
      const grant: RemoteInvokeGrant = {
        id: result.grant_id,
        user_id: result.user_id ?? '',
        client_instance_id: clientInstanceId,
        caller_fingerprint: effectiveCallerFingerprint,
        caller_display_name: callerDisplayName,
        caller_pubkey: callerPubkey,
        caller_pubkey_fp: effectiveCallerFingerprint,
        caller_ephemeral_pub: req.caller_ephemeral_pub ?? '',
        client_ephemeral_pub: req.client_ephemeral_pub ?? '',
        grant_mode: grantMode,
        grant_scope: normalizeGrantScope(req.grant_scope),
        file_access: normalizeFileAccess(req.file_access),
        ssh_key_id: '',
        ssh_key_fingerprint: result.ssh_key_fingerprint,
        status: 'active',
        first_authorized_at: now,
        expires_at: '',
        last_used_at: now,
        max_calls: maxCalls,
        remaining_calls: maxCalls,
        created_by: 'ssh_publickey',
        update_time: now,
      };
      await this.storage.remoteInvoke.createGrant(grant);

      // P0-3: do NOT issue a grant_session_token here. Instead mint a single-use
      // claim_token; the caller must redeem it via PoP at /v5/remote-invoke/grants/ssh-claim.
      claimToken = randomToken();
      claimExpiresAt = new Date(Date.now() + CLAIM_TOKEN_TTL_MS).toISOString();
      grantIdForClaim = result.grant_id;
      await this.storage.remoteInvoke.createSshClaim({
        claim_token_hash: sha256Hex(claimToken),
        grant_id: result.grant_id,
        client_instance_id: clientInstanceId,
        caller_pubkey_fp: effectiveCallerFingerprint,
        expires_at: claimExpiresAt,
        create_time: now,
        claimed_at: '',
      });

      pushToClient(clientInstanceId, 'grant_created', {
        grant_id: result.grant_id,
        caller_fingerprint: effectiveCallerFingerprint,
        caller_display_name: callerDisplayName,
        caller_info: {
          ...(result.caller_info ?? {}),
          fingerprint: effectiveCallerFingerprint,
          caller_pubkey: callerPubkey,
        },
        grant_mode: grantMode,
        grant_scope: normalizeGrantScope(req.grant_scope),
        file_access: normalizeFileAccess(req.file_access),
        caller_ephemeral_pub: req.caller_ephemeral_pub ?? '',
        client_ephemeral_pub: req.client_ephemeral_pub ?? '',
        auth_method: 'ssh_publickey',
        ssh_key_fingerprint: result.ssh_key_fingerprint,
        max_calls: maxCalls,
        remaining_calls: maxCalls,
      });
    }
    pushToCallerStream(result.connect_id, 'ssh_connect_result', {
      ...result,
      client_instance_id: clientInstanceId,
      caller_fingerprint: effectiveCallerFingerprint,
      caller_ephemeral_pub: req.caller_ephemeral_pub ?? null,
      client_ephemeral_pub: req.client_ephemeral_pub ?? null,
      claim_token: claimToken,
      claim_expires_at: claimExpiresAt,
      grant_id: grantIdForClaim,
    });
    this.expireCallToken(result.connect_id);
  }
  async removeGrant(userId: string, grantId: string, callerFingerprint: string): Promise<void> {
    const grant = await this.storage.remoteInvoke.getGrant(grantId);
    if (!grant) throw new Error('grant_not_found');

    if (grant.caller_fingerprint !== callerFingerprint) {
      throw new Error('caller_fingerprint_mismatch');
    }

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

  async removeGrantByClient(clientInstanceId: string, grantId: string): Promise<void> {
    const grant = await this.storage.remoteInvoke.getGrant(grantId);
    if (!grant) throw new Error('grant_not_found');

    if (grant.client_instance_id !== clientInstanceId) {
      throw new Error('client_instance_id_mismatch');
    }

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

  async getCallForClient(clientInstanceId: string, callId: string): Promise<RemoteInvokeCall | null> {
    const call = await this.storage.remoteInvoke.getCall(callId);
    if (!call || call.client_instance_id !== clientInstanceId) {
      return null;
    }
    return call;
  }

  async listCallsForClient(
    clientInstanceId: string,
    query: CallsQueryParams,
  ): Promise<{ list: RemoteInvokeCall[]; total: number }> {
    return this.storage.remoteInvoke.listCalls('', {
      ...query,
      client_instance_id: clientInstanceId,
    });
  }

  async listCallEvents(callId: string, query?: { offset?: number; limit?: number }): Promise<{ list: RemoteInvokeEvent[]; total: number }> {
    return this.storage.remoteInvoke.listCallEvents(callId, query);
  }

  async revokeAck(_grantId: string): Promise<void> {
  }

  private expireCallToken(callId: string, ttlMs: number = CALL_TOKEN_GRACE_MS): void {
    const existing = this.callTokens.get(callId);
    if (!existing) return;
    existing.expiresAt = Date.now() + ttlMs;
    this.callTokens.set(callId, existing);
  }

  private cleanupExpiredCallToken(callId: string): void {
    const existing = this.callTokens.get(callId);
    if (!existing?.expiresAt) return;
    if (existing.expiresAt > Date.now()) return;
    this.callTokens.delete(callId);
    this.callRuntimeMeta.delete(callId);
    clearCallerEventBuffer(callId);
    clearReplaySafeClientEvents(callId);
  }

  private cleanupExpiredRegistrationChallenges(): void {
    const now = Date.now();
    for (const [challengeId, challenge] of this.registrationChallenges.entries()) {
      if (challenge.expiresAt < now) {
        this.registrationChallenges.delete(challengeId);
      }
    }
  }

  private async waitForClientStream(clientInstanceId: string, timeoutMs: number): Promise<boolean> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      if (getClientStream(clientInstanceId)) {
        return true;
      }
      await new Promise((resolve) => setTimeout(resolve, SSH_CONNECT_STREAM_POLL_MS));
    }
    return !!getClientStream(clientInstanceId);
  }

  async updateGrantByClient(
    clientInstanceId: string,
    grantId: string,
    req: UpdateGrantRequest,
  ): Promise<RemoteInvokeGrant> {
    const grant = await this.storage.remoteInvoke.getGrant(grantId);
    if (!grant) throw new Error('grant_not_found');
    if (grant.client_instance_id !== clientInstanceId) {
      throw new Error('client_instance_id_mismatch');
    }
    if (grant.status !== 'active') {
      throw new Error('grant_not_active');
    }

    const normalizedScope = normalizeGrantScope(req.grant_scope ?? grant.grant_scope);
    const normalizedFileAccess = normalizeFileAccess(req.file_access ?? grant.file_access);
    const now = new Date().toISOString();

    await this.storage.remoteInvoke.updateGrant(grantId, {
      grant_scope: normalizedScope,
      file_access: normalizedFileAccess,
      update_time: now,
    });

    const updated = await this.storage.remoteInvoke.getGrant(grantId);
    if (!updated) {
      throw new Error('grant_not_found_after_update');
    }

    pushToClient(updated.client_instance_id, 'grant_updated', {
      grant_id: grantId,
      grant_scope: normalizedScope,
      file_access: normalizedFileAccess,
    });

    await this.storage.remoteInvoke.appendEvent({
      id: nanoid(),
      call_id: '',
      event_type: 'grant_updated',
      seq: 0,
      direction: '',
      event_summary_json: JSON.stringify({ grant_id: grantId }),
      create_time: now,
    });

    return updated;
  }
}

function decodeBase64(value: string, field: string): Buffer {
  if (!value || typeof value !== 'string' || value.length % 4 !== 0) {
    throw new Error(`invalid_${field}_base64`);
  }
  if (!/^[A-Za-z0-9+/=]+$/.test(value)) {
    throw new Error(`invalid_${field}_base64`);
  }
  const decoded = Buffer.from(value, 'base64');
  if (decoded.length === 0 || decoded.toString('base64') !== value) {
    throw new Error(`invalid_${field}_base64`);
  }
  return decoded;
}
