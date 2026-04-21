import crypto from 'crypto';
import { customAlphabet } from 'nanoid';
import type {
  SshChallengeResponse,
  SshConnectRequest,
  SshConnectResponse,
  SshConnectResultRequest,
  SshDeviceRoute,
} from './types';

const nanoid = customAlphabet('0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_', 21);
const SSH_ROUTE_TTL_MS = 10 * 60_000;
const SSH_CHALLENGE_TTL_MS = 120_000;
const SSH_CONNECT_TTL_MS = 30_000;
const SSH_TIMESTAMP_SKEW_MS = 30_000;
const SSH_CHALLENGE_RATE_LIMIT = 10;
const SSH_CHALLENGE_RATE_WINDOW_MS = 60_000;

type RouteEntry = SshDeviceRoute & {
  clientInstanceId: string;
  expiresAt: number;
};

type ChallengeEntry = {
  challengeId: string;
  deviceCode: string;
  challenge: string;
  expiresAt: number;
};

type PendingConnectEntry = {
  connectId: string;
  clientInstanceId: string;
  deviceCode: string;
  relayToken: string;
  expiresAt: number;
  callerInfo?: SshConnectRequest['caller_info'];
};

function exportPublicKeyDer(publicKeyPem: string): Buffer {
  const publicKey = crypto.createPublicKey(publicKeyPem);
  return publicKey.export({ format: 'der', type: 'spki' }) as Buffer;
}

function computeSha256Hex(input: Buffer | string): string {
  return crypto.createHash('sha256').update(input).digest('hex');
}

export function deriveSshDeviceCode(publicKeyPem: string): string {
  const der = exportPublicKeyDer(publicKeyPem);
  return `BF-${computeSha256Hex(der).slice(0, 16).toUpperCase()}`;
}

export function computeSshKeyFingerprint(publicKeyPem: string): string {
  return computeSha256Hex(exportPublicKeyDer(publicKeyPem));
}

export function buildSshConnectPayload(
  challengeId: string,
  challenge: string,
  deviceCode: string,
  timestamp: number,
): string {
  return JSON.stringify({
    challenge,
    challenge_id: challengeId,
    device_code: deviceCode,
    timestamp,
  });
}

export class SshAuthService {
  private routeByDeviceCode = new Map<string, RouteEntry>();
  private deviceCodeByClientInstanceId = new Map<string, string>();
  private challenges = new Map<string, ChallengeEntry>();
  private pendingConnects = new Map<string, PendingConnectEntry>();
  private challengeRateLimit = new Map<string, { count: number; resetAt: number }>();

  syncSshRoute(clientInstanceId: string, route: SshDeviceRoute | null | undefined): {
    routeCleared: boolean;
    routeChanged: boolean;
  } {
    this.cleanupExpiredState();
    const previousDeviceCode = this.deviceCodeByClientInstanceId.get(clientInstanceId);
    const previousRoute = previousDeviceCode
      ? this.routeByDeviceCode.get(previousDeviceCode)
      : undefined;

    if (!route) {
      if (previousDeviceCode) {
        this.routeByDeviceCode.delete(previousDeviceCode);
        this.deviceCodeByClientInstanceId.delete(clientInstanceId);
      }
      return {
        routeCleared: !!previousDeviceCode,
        routeChanged: !!previousDeviceCode,
      };
    }

    this.assertRouteMatchesPublicKey(route);
    if (previousDeviceCode && previousDeviceCode !== route.device_code) {
      this.routeByDeviceCode.delete(previousDeviceCode);
    }

    const expiresAt = Date.now() + SSH_ROUTE_TTL_MS;
    this.routeByDeviceCode.set(route.device_code, {
      ...route,
      clientInstanceId,
      expiresAt,
    });
    this.deviceCodeByClientInstanceId.set(clientInstanceId, route.device_code);
    return {
      routeCleared: false,
      routeChanged:
        !previousRoute ||
        previousRoute.device_code !== route.device_code ||
        previousRoute.public_key_pem !== route.public_key_pem,
    };
  }

  issueChallenge(deviceCode: string): SshChallengeResponse {
    this.cleanupExpiredState();
    const route = this.routeByDeviceCode.get(deviceCode);
    if (!route || route.expiresAt < Date.now()) {
      this.routeByDeviceCode.delete(deviceCode);
      throw new Error('device_code_not_found');
    }
    this.checkChallengeRateLimit(deviceCode);
    const challengeId = nanoid();
    const challenge = crypto.randomBytes(64).toString('hex');
    const expiresAt = Date.now() + SSH_CHALLENGE_TTL_MS;
    this.challenges.set(challengeId, {
      challengeId,
      deviceCode,
      challenge,
      expiresAt,
    });
    return {
      challenge_id: challengeId,
      challenge,
      expires_at: expiresAt,
    };
  }

  verifyAndPrepareConnect(body: SshConnectRequest): SshConnectResponse & {
    client_instance_id: string;
    ssh_key_fingerprint: string;
  } {
    this.cleanupExpiredState();
    const challenge = this.challenges.get(body.challenge_id);
    if (!challenge) {
      throw new Error('challenge_expired');
    }
    this.challenges.delete(body.challenge_id);
    if (challenge.expiresAt < Date.now()) {
      throw new Error('challenge_expired');
    }
    if (challenge.deviceCode !== body.device_code) {
      throw new Error('device_code_mismatch');
    }
    if (!Number.isFinite(body.timestamp) || Math.abs(Date.now() - body.timestamp) > SSH_TIMESTAMP_SKEW_MS) {
      throw new Error('timestamp_out_of_window');
    }

    const route = this.routeByDeviceCode.get(body.device_code);
    if (!route || route.expiresAt < Date.now()) {
      this.routeByDeviceCode.delete(body.device_code);
      throw new Error('device_code_not_found');
    }

    const payload = buildSshConnectPayload(
      challenge.challengeId,
      challenge.challenge,
      challenge.deviceCode,
      body.timestamp,
    );
    const publicKey = crypto.createPublicKey(route.public_key_pem);
    const signature = decodeBase64(body.signature, 'signature');
    const verified = crypto.verify(null, Buffer.from(payload, 'utf8'), publicKey, signature);
    if (!verified) {
      throw new Error('ssh_signature_invalid');
    }

    const fingerprint = computeSshKeyFingerprint(route.public_key_pem);
    const connectId = crypto.randomBytes(16).toString('hex');
    const relayToken = crypto.randomBytes(32).toString('hex');
    const expiresAt = Date.now() + SSH_CONNECT_TTL_MS;
    this.pendingConnects.set(connectId, {
      connectId,
      clientInstanceId: route.clientInstanceId,
      deviceCode: route.device_code,
      relayToken,
      expiresAt,
      callerInfo: body.caller_info,
    });

    return {
      connect_id: connectId,
      relay_token: relayToken,
      expires_at: expiresAt,
      client_instance_id: route.clientInstanceId,
      ssh_key_fingerprint: fingerprint,
    };
  }

  verifyPendingConnectToken(connectId: string, relayToken: string): boolean {
    this.cleanupExpiredState();
    const pending = this.pendingConnects.get(connectId);
    if (!pending || pending.expiresAt < Date.now()) {
      this.pendingConnects.delete(connectId);
      return false;
    }
    return timingSafeEqual(pending.relayToken, relayToken);
  }

  completeConnect(
    clientInstanceId: string,
    body: SshConnectResultRequest,
  ): {
    connect_id: string;
    status: 'approved' | 'rejected';
    grant_id?: string;
    expires_at?: number | string | null;
    reason?: string;
    caller_fingerprint?: string;
    grant_mode?: SshConnectResultRequest['grant_mode'];
    caller_info?: SshConnectRequest['caller_info'];
  } {
    this.cleanupExpiredState();
    const pending = this.pendingConnects.get(body.connect_id);
    if (!pending) {
      throw new Error('connect_not_found');
    }
    if (pending.expiresAt < Date.now()) {
      this.pendingConnects.delete(body.connect_id);
      throw new Error('client_timeout');
    }
    if (pending.clientInstanceId !== clientInstanceId) {
      throw new Error('client_instance_id_mismatch');
    }
    this.pendingConnects.delete(body.connect_id);
    return {
      connect_id: body.connect_id,
      status: body.status,
      grant_id: body.grant_id,
      expires_at: body.expires_at ?? null,
      reason: body.reason,
      caller_fingerprint: body.caller_fingerprint,
      grant_mode: body.grant_mode,
      caller_info: pending.callerInfo,
    };
  }

  private assertRouteMatchesPublicKey(route: SshDeviceRoute): void {
    const derived = deriveSshDeviceCode(route.public_key_pem);
    if (derived !== route.device_code) {
      throw new Error('device_code_derivation_mismatch');
    }
  }

  private checkChallengeRateLimit(deviceCode: string): void {
    const now = Date.now();
    const entry = this.challengeRateLimit.get(deviceCode);
    if (!entry || now > entry.resetAt) {
      this.challengeRateLimit.set(deviceCode, {
        count: 1,
        resetAt: now + SSH_CHALLENGE_RATE_WINDOW_MS,
      });
      return;
    }
    entry.count += 1;
    if (entry.count > SSH_CHALLENGE_RATE_LIMIT) {
      throw new Error('challenge_rate_limited');
    }
  }

  private cleanupExpiredState(): void {
    const now = Date.now();
    for (const [deviceCode, route] of this.routeByDeviceCode.entries()) {
      if (route.expiresAt < now) {
        this.routeByDeviceCode.delete(deviceCode);
        if (this.deviceCodeByClientInstanceId.get(route.clientInstanceId) === deviceCode) {
          this.deviceCodeByClientInstanceId.delete(route.clientInstanceId);
        }
      }
    }
    for (const [challengeId, challenge] of this.challenges.entries()) {
      if (challenge.expiresAt < now) {
        this.challenges.delete(challengeId);
      }
    }
    for (const [connectId, pending] of this.pendingConnects.entries()) {
      if (pending.expiresAt < now) {
        this.pendingConnects.delete(connectId);
      }
    }
    for (const [deviceCode, entry] of this.challengeRateLimit.entries()) {
      if (entry.resetAt < now) {
        this.challengeRateLimit.delete(deviceCode);
      }
    }
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

function timingSafeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  return crypto.timingSafeEqual(Buffer.from(a, 'utf8'), Buffer.from(b, 'utf8'));
}
