import type { IStorage } from '../dao/types';
import type { RequestContext } from '../http';
import crypto from 'crypto';
import {
  sendJson,
  sendError,
  sendRateLimited,
  parseJsonBody,
  extractPathParam,
  requireAuth,
  openSse,
  writeSseEvent,
} from '../http';
import type { RemoteInvokeConfig, RemoteInvokeGrant, RemoteInvokeCall } from '../types';
import { RemoteInvokeService } from '../remote-invoke/service';
import { RateLimiter } from '../security';
import {
  registerClientStream,
  unregisterClientStream,
  registerPairingWatcher,
  unregisterPairingWatcher,
  registerCallerEventStream,
  unregisterCallerEventStream,
  flushCallerEventStream,
  startKeepalive,
  getClientStream,
  getAllClientStreams,
} from '../remote-invoke/sse';
import { startCleanupScheduler } from '../remote-invoke/cleanup';
import type { ClientStreamState } from '../remote-invoke/types';
import { normalizeGrantScope, resolveCommandKind } from '../remote-invoke/types';
import { verifyPoP, type PoPRequestEnvelope } from '../remote-invoke/pop';
import { customAlphabet } from 'nanoid';

const nanoid = customAlphabet('0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_', 21);

const registerLimiter = new RateLimiter(60, 60_000);
const clientQueryLimiter = new RateLimiter(240, 60_000);
const clientDataLimiter = new RateLimiter(1_500, 10_000);
const callerLookupLimiter = new RateLimiter(240, 60_000);
const callerOpenLimiter = new RateLimiter(120, 60_000);
const callerControlLimiter = new RateLimiter(240, 60_000);
// PR#6a-followup: high-throughput stream_frame endpoint gets its own dedicated
// limiter so large-output streams do not share quota with legacy /frame.
const clientStreamFrameLimiter = new RateLimiter(60_000, 10_000);
// Hard cap on a single stream_frame payload: 2 MiB. Protects relay memory.
const MAX_STREAM_FRAME_BYTES = 2 * 1024 * 1024;

let serviceInstance: RemoteInvokeService | null = null;
let serviceStorage: IStorage | null = null;

function getService(storage: IStorage, config: RemoteInvokeConfig): RemoteInvokeService {
  if (!serviceInstance || serviceStorage !== storage) {
    serviceInstance = new RemoteInvokeService(storage, config);
    serviceStorage = storage;
    activeConfig = config;
    startKeepalive(config.sse_keepalive_ms);
    startCleanupScheduler(storage, config);
  }
  return serviceInstance;
}

const DEFAULT_REMOTE_INVOKE_CONFIG: RemoteInvokeConfig = {
  enabled: false,
  sse_keepalive_ms: 30000,
  pair_code_ttl_secs: 120,
  max_active_calls_per_client: 5,
  max_grants_per_client: 20,
  retention_days: 90,
  max_records: 10000,
  max_sse_connections_per_client: 2,
  max_sse_connections_per_ip: 10,
  pair_rate_limit_per_ip: 5,
  pair_rate_limit_global_per_client: 10,
};

function extractBearerToken(ctx: RequestContext): string | null {
  const auth = ctx.req.headers['authorization'] || '';
  if (auth.startsWith('Bearer ')) return auth.slice(7);
  return null;
}

function sha256Hex(value: string): string {
  return crypto.createHash('sha256').update(value, 'utf8').digest('hex');
}

let activeConfig: RemoteInvokeConfig = DEFAULT_REMOTE_INVOKE_CONFIG;

async function requireClientAuth(
  ctx: RequestContext,
  service: RemoteInvokeService,
): Promise<{ client_instance_id: string; user_id: string } | null> {
  const token = extractBearerToken(ctx) || ctx.url.searchParams.get('client_auth_token') || '';
  const clientId = ctx.url.searchParams.get('client_instance_id') || (() => {
    try {
      const body = parseJsonBody<any>(ctx.body);
      return body?.client_instance_id || '';
    } catch {
      return '';
    }
  })();
  if (!token || !clientId) {
    sendError(ctx.res, 401, 'client_auth_required');
    return null;
  }
  const record = await service.verifyClientAuth(clientId, token);
  if (!record) {
    sendError(ctx.res, 401, 'invalid_client_auth_token');
    return null;
  }
  return { client_instance_id: record.client_instance_id, user_id: record.user_id };
}

const pairRateLimiters = new Map<string, { count: number; resetAt: number }>();

function applyRateLimit(ctx: RequestContext, limiter: RateLimiter, key: string | null | undefined): boolean {
  const normalizedKey = key?.trim();
  if (!normalizedKey) return true;
  const check = limiter.check(normalizedKey);
  if (check.allowed) return true;
  sendRateLimited(ctx.res, check.retryAfterMs);
  return false;
}

function syncTokenKey(ctx: RequestContext): string {
  const token = (ctx.req.headers['x-bifrost-token'] as string | undefined)?.trim();
  return token ? `sync:${token}` : `ip:${ctx.clientIp || 'unknown'}`;
}

function callerAccessKey(ctx: RequestContext): string {
  const bearer = extractBearerToken(ctx)?.trim();
  return bearer ? `bearer:${bearer}` : `ip:${ctx.clientIp || 'unknown'}`;
}

function protocolVersionNotSupported(ctx: RequestContext): boolean {
  sendJson(ctx.res, 410, { error: 'protocol_version_not_supported' });
  return true;
}

function validateCallerEphemeralPub(ctx: RequestContext, value: unknown): string | null {
  if (typeof value !== 'string' || value.length === 0) {
    sendError(ctx.res, 400, 'caller_ephemeral_pub_required');
    return null;
  }
  if (value.length % 4 !== 0 || !/^[A-Za-z0-9+/]+={0,2}$/.test(value)) {
    sendError(ctx.res, 400, 'caller_ephemeral_pub_invalid');
    return null;
  }
  const decoded = Buffer.from(value, 'base64');
  if (decoded.length !== 32 || decoded.toString('base64') !== value) {
    sendError(ctx.res, 400, 'caller_ephemeral_pub_invalid');
    return null;
  }
  return value;
}

async function requirePoP(
  ctx: RequestContext,
  storage: IStorage,
  expectedFp?: string,
): Promise<{ callerPubkey: string; callerPubkeyFp: string; body: any } | null> {
  let body: PoPRequestEnvelope;
  try {
    body = parseJsonBody<any>(ctx.body);
  } catch {
    sendError(ctx.res, 400, 'invalid_json');
    return null;
  }
  try {
    const nonceGcBefore = new Date(Date.now() - 60_000).toISOString();
    await storage.remoteInvoke.gcNonces(nonceGcBefore);
    const result = await verifyPoP(
      body,
      { expectedCallerPubkeyFp: expectedFp },
      (fp, nonce, seenAt) => storage.remoteInvoke.markNonceUsed(fp, nonce, seenAt),
    );
    return { ...result, body };
  } catch (e: unknown) {
    const message = e instanceof Error ? e.message : 'signature_invalid';
    const status = message === 'caller_pubkey_mismatch' ? 403 : 401;
    sendError(ctx.res, status, message);
    return null;
  }
}

async function extractBearerGrantSession(
  ctx: RequestContext,
  service: RemoteInvokeService,
): Promise<RemoteInvokeGrant | null> {
  const bearer = extractBearerToken(ctx);
  if (!bearer) {
    sendError(ctx.res, 401, 'grant_session_token_invalid');
    return null;
  }
  const grant = await service.resolveGrantBySessionToken(bearer);
  if (!grant) {
    sendError(ctx.res, 401, 'grant_session_token_invalid');
    return null;
  }
  return grant;
}

function checkPairRateLimit(ip: string): boolean {
  const limit = activeConfig.pair_rate_limit_per_ip;
  const now = Date.now();
  const windowMs = 60_000;
  const entry = pairRateLimiters.get(ip);
  if (!entry || now > entry.resetAt) {
    pairRateLimiters.set(ip, { count: 1, resetAt: now + windowMs });
    return true;
  }
  entry.count++;
  return entry.count <= limit;
}

export async function handleRemoteInvoke(
  ctx: RequestContext,
  storage: IStorage,
  remoteInvokeConfig?: RemoteInvokeConfig,
): Promise<boolean> {
  const { url, req } = ctx;
  const method = req.method ?? 'GET';
  const pathname = url.pathname.replace(/\/$/, '') || '/';

  if (!pathname.startsWith('/v4/remote-invoke') && !pathname.startsWith('/v5/remote-invoke')) return false;

  const config = remoteInvokeConfig ?? DEFAULT_REMOTE_INVOKE_CONFIG;

  if (!config.enabled) {
    sendError(ctx.res, 403, 'remote invoke is not enabled');
    return true;
  }

  const service = getService(storage, config);

  try {
    if (pathname === '/v5/remote-invoke/pairings/start' && method === 'POST') {
      return await handleStartPairing(ctx, service, 'v5');
    }

    if (pathname.match(/^\/v5\/remote-invoke\/pairings\/[^/]+\/watch$/) && method === 'GET') {
      return await handlePairingWatchV5(ctx, storage);
    }

    if (pathname === '/v5/remote-invoke/grants/lookup' && method === 'POST') {
      return await handleGrantsLookupV5(ctx, storage, service);
    }

    if (pathname === '/v5/remote-invoke/grants/claim' && method === 'POST') {
      return await handleGrantsClaimV5(ctx, storage, service);
    }

    if (pathname === '/v5/remote-invoke/grants/revoke' && method === 'POST') {
      return await handleGrantsRevokeV5(ctx, storage, service);
    }

    if (pathname === '/v5/remote-invoke/calls/open' && method === 'POST') {
      return await handleOpenCallV5(ctx, storage, service);
    }

    if (pathname === '/v4/remote-invoke/client/register/challenge' && method === 'POST') {
      return await handleClientRegisterChallenge(ctx, storage, service);
    }

    if (pathname === '/v4/remote-invoke/client/register' && method === 'POST') {
      if (!(await requireAuth(ctx, storage))) return true;
      return await handleClientRegister(ctx, service);
    }

    if (pathname === '/v4/remote-invoke/ssh/challenge' && method === 'POST') {
      return await handleSshChallenge(ctx, service);
    }

    if (pathname === '/v4/remote-invoke/ssh/connect' && method === 'POST') {
      return await handleSshConnect(ctx, service);
    }

    if (pathname === '/v4/remote-invoke/client/stream' && method === 'GET') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return handleClientStream(ctx, service, auth.client_instance_id, auth.user_id);
    }

    if (pathname === '/v4/remote-invoke/client/heartbeat' && method === 'POST') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleClientHeartbeat(ctx, service, auth.client_instance_id);
    }

    if (pathname === '/v4/remote-invoke/ssh/connect-result' && method === 'POST') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleSshConnectResult(ctx, service, auth.client_instance_id);
    }

    if (pathname === '/v4/remote-invoke/client/pair-code' && method === 'POST') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handlePublishPairCode(ctx, service, auth.client_instance_id);
    }

    if (pathname.startsWith('/v4/remote-invoke/client/discovery-session/') && method === 'DELETE') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleCloseDiscovery(ctx, service, auth.client_instance_id);
    }

    if (pathname === '/v4/remote-invoke/client/pending-pairings' && method === 'GET') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleGetPendingPairings(ctx, service, auth.client_instance_id);
    }

    if (pathname === '/v4/remote-invoke/client/pending-pairings' && method === 'DELETE') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleCancelPendingPairings(ctx, service, auth.client_instance_id);
    }

    if (pathname === '/v4/remote-invoke/client/active-grants' && method === 'GET') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleListActiveGrantsForClient(ctx, service, auth.client_instance_id);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/grants\/[^/]+\/decision$/) && method === 'POST') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleGrantDecision(ctx, service, auth.client_instance_id);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/calls\/[^/]+\/frame$/) && method === 'POST') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleClientCallFrame(ctx, service, auth.client_instance_id);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/calls\/[^/]+\/stream-frame$/) && method === 'POST') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleClientCallStreamFrame(ctx, service, auth.client_instance_id);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/calls\/[^/]+\/exit$/) && method === 'POST') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleClientCallExit(ctx, service, auth.client_instance_id);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/grants\/[^/]+\/revoke-ack$/) && method === 'POST') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return handleRevokeAck(ctx);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/grants\/[^/]+$/) && method === 'DELETE') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleDeleteGrantByClient(ctx, service, auth.client_instance_id);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/grants\/[^/]+$/) && method === 'PATCH') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleUpdateGrantByClient(ctx, service, auth.client_instance_id);
    }

    if (pathname === '/v4/remote-invoke/client/calls' && method === 'GET') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleListCalls(ctx, service, auth.client_instance_id);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/calls\/[^/]+$/) && method === 'GET') {
      const auth = await requireClientAuth(ctx, service);
      if (!auth) return true;
      return await handleGetCall(ctx, service, auth.client_instance_id);
    }

    if (pathname === '/v4/remote-invoke/pairings/start' && method === 'POST') {
      return protocolVersionNotSupported(ctx);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/pairings\/[^/]+\/watch$/) && method === 'GET') {
      return protocolVersionNotSupported(ctx);
    }

    if (pathname === '/v4/remote-invoke/grants/reusable' && method === 'GET') {
      return protocolVersionNotSupported(ctx);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/grants\/[^/]+$/) && method === 'DELETE') {
      return protocolVersionNotSupported(ctx);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/calls\/[^/]+\/input$/) && method === 'POST') {
      return await handleCallInput(ctx, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/calls\/[^/]+\/status$/) && method === 'GET') {
      return await handleCallStatus(ctx, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/calls\/[^/]+\/events$/) && method === 'GET') {
      return handleCallEvents(ctx, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/calls\/[^/]+\/cancel$/) && method === 'POST') {
      return await handleCancelCall(ctx, service);
    }

    if (pathname === '/v4/remote-invoke/calls/open' && method === 'POST') {
      return protocolVersionNotSupported(ctx);
    }

    sendError(ctx.res, 404, 'remote invoke endpoint not found');
    return true;
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'internal error';
    console.error('[remote-invoke] route error:', e);
    sendError(ctx.res, 500, msg);
    return true;
  }
}

async function handleClientRegisterChallenge(
  ctx: RequestContext,
  storage: IStorage,
  service: RemoteInvokeService,
): Promise<boolean> {
  if (!(await requireAuth(ctx, storage))) {
    return true;
  }
  const body = parseJsonBody<any>(ctx.body);
  if (!body?.client_instance_id) {
    sendError(ctx.res, 400, 'client_instance_id is required');
    return true;
  }
  if (!applyRateLimit(ctx, registerLimiter, `${syncTokenKey(ctx)}:register_challenge:${body.client_instance_id}`)) {
    return true;
  }
  const result = service.issueRegistrationChallenge(ctx.user!.user_id, body.client_instance_id);
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  return true;
}

async function handleClientRegister(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  if (!ctx.user?.user_id) {
    sendError(ctx.res, 401, 'unauthorized');
    return true;
  }
  if (!body?.challenge_id || !body?.client_instance_id || !body?.client_long_term_pubkey) {
    sendError(ctx.res, 400, 'challenge_id, client_instance_id and client_long_term_pubkey are required');
    return true;
  }
  if (!applyRateLimit(ctx, registerLimiter, `${syncTokenKey(ctx)}:register:${body.client_instance_id}`)) {
    return true;
  }
  try {
    const result = await service.registerClient(ctx.user.user_id, body);
    sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  } catch (e: unknown) {
    const message = e instanceof Error ? e.message : 'register failed';
    const status = (() => {
      switch (message) {
        case 'invalid_registration_signature':
          return 401;
        case 'client_instance_id_owned_by_another_user':
        case 'pubkey mismatch for existing client_instance_id':
        case 'registration_user_mismatch':
          return 403;
        default:
          return 400;
      }
    })();
    sendError(ctx.res, status, message);
  }
  return true;
}

async function handleSshChallenge(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  if (!body?.device_code) {
    sendError(ctx.res, 400, 'device_code is required');
    return true;
  }
  try {
    const result = service.issueSshChallenge(body.device_code);
    sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  } catch (e: unknown) {
    const message = e instanceof Error ? e.message : 'ssh challenge failed';
    const status = message === 'challenge_rate_limited' ? 429 : 400;
    sendError(ctx.res, status, message);
  }
  return true;
}

async function handleSshConnect(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  if (!body?.device_code || !body?.challenge_id || !body?.signature || !Number.isFinite(body?.timestamp) || !body?.caller_ephemeral_pub) {
    sendError(ctx.res, 400, 'device_code, challenge_id, signature, timestamp and caller_ephemeral_pub are required');
    return true;
  }
  try {
    const result = await service.verifySshConnect(body);
    sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  } catch (e: unknown) {
    const message = e instanceof Error ? e.message : 'ssh connect failed';
    const status = message === 'client_offline' ? 404 : 400;
    sendError(ctx.res, status, message);
  }
  return true;
}

function handleClientStream(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientId: string,
  userId: string,
): boolean {
  const streamId = ctx.url.searchParams.get('stream_id') ?? nanoid();
  const ip = ctx.clientIp || '';
  const existing = getClientStream(clientId);
  const isReplacingExisting = !!existing && existing.streamId !== streamId;

  if (!isReplacingExisting && existing && activeConfig.max_sse_connections_per_client <= 1) {
    sendError(ctx.res, 429, 'max SSE connections per client exceeded');
    return true;
  }
  const sharedStreamKey = userId ? `user:${userId}` : (ip ? `ip:${ip}` : '');
  if (sharedStreamKey) {
    let currentIpCount = 0;
    for (const state of getAllClientStreams().values()) {
      const stateKey = state.userId ? `user:${state.userId}` : (state.clientIp ? `ip:${state.clientIp}` : '');
      if (stateKey === sharedStreamKey) {
        currentIpCount += 1;
      }
    }
    const existingKey = existing?.userId
      ? `user:${existing.userId}`
      : (existing?.clientIp ? `ip:${existing.clientIp}` : '');
    if (isReplacingExisting && existingKey === sharedStreamKey) {
      currentIpCount = Math.max(0, currentIpCount - 1);
    }
    if (currentIpCount >= activeConfig.max_sse_connections_per_ip) {
      sendError(ctx.res, 429, userId ? 'max SSE connections per user exceeded' : 'max SSE connections per IP exceeded');
      return true;
    }
  }

  openSse(ctx.res);

  const state: ClientStreamState = {
    clientInstanceId: clientId,
    userId,
    streamId,
    clientIp: ip || undefined,
    res: ctx.res,
    lastHeartbeat: Date.now(),
    discoverable: false,
    connectedAt: Date.now(),
  };

  registerClientStream(state);

  writeSseEvent(ctx.res, 'client_hello_ack', {
    stream_id: streamId,
    server_time: new Date().toISOString(),
  });

  ctx.req.on('close', () => {
    unregisterClientStream(clientId, streamId);
  });

  return true;
}

async function handleClientHeartbeat(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientId: string,
): Promise<boolean> {
  if (!applyRateLimit(ctx, clientQueryLimiter, `client:${clientId}:heartbeat`)) {
    return true;
  }
  const body = parseJsonBody<any>(ctx.body);

  await service.clientHeartbeat({
    client_instance_id: clientId,
    stream_id: body?.stream_id ?? '',
    active_call_ids: body?.active_call_ids,
    ssh_device_route: body?.ssh_device_route,
  });

  sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  return true;
}

async function handleSshConnectResult(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientId: string,
): Promise<boolean> {
  if (!applyRateLimit(ctx, clientQueryLimiter, `client:${clientId}:ssh_connect_result`)) {
    return true;
  }
  const body = parseJsonBody<any>(ctx.body);
  if (!body?.connect_id || !body?.status) {
    sendError(ctx.res, 400, 'connect_id and status are required');
    return true;
  }
  try {
    await service.submitSshConnectResult(clientId, body);
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    const message = e instanceof Error ? e.message : 'ssh connect result failed';
    const status = message === 'client_timeout' ? 408 : message === 'client_instance_id_mismatch' ? 403 : 400;
    sendError(ctx.res, status, message);
  }
  return true;
}

async function handlePublishPairCode(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientId: string,
): Promise<boolean> {
  if (!applyRateLimit(ctx, clientQueryLimiter, `client:${clientId}:publish_pair_code`)) {
    return true;
  }
  const body = parseJsonBody<any>(ctx.body);
  if (!body?.pair_code) {
    sendError(ctx.res, 400, 'pair_code is required');
    return true;
  }

  const userId = body.user_id ?? '';
  await service.publishPairCode(userId, {
    client_instance_id: clientId,
    pair_code: body.pair_code,
    expires_at: body.expires_at ?? Date.now() + 120000,
    discovery_session_id: body.discovery_session_id,
  });

  sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  return true;
}

async function handleCloseDiscovery(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientId: string,
): Promise<boolean> {
  if (!applyRateLimit(ctx, clientQueryLimiter, `client:${clientId}:close_discovery`)) {
    return true;
  }
  const sessionId = extractPathParam(ctx.url.pathname, '/v4/remote-invoke/client/discovery-session/');
  await service.closeDiscoverySession(clientId, sessionId);

  sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  return true;
}

async function handleGrantDecision(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientId: string,
): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/grants\/([^/]+)\/decision/);
  const pairingId = parts?.[1] ?? '';
  if (!applyRateLimit(ctx, clientQueryLimiter, `client:${clientId}:grant_decision:${pairingId || 'unknown'}`)) {
    return true;
  }
  const body = parseJsonBody<any>(ctx.body);

  if (!body?.decision) {
    sendError(ctx.res, 400, 'decision is required');
    return true;
  }

  try {
    const userId = body.user_id ?? '';
    const result = await service.submitGrantDecision(userId, {
      pairing_id: pairingId,
      client_instance_id: clientId,
      decision: body.decision,
      grant_mode: body.grant_mode,
      grant_scope: body.grant_scope,
      file_access: body.file_access,
      client_ephemeral_pub: body.client_ephemeral_pub,
    });
    sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'decision failed';
    if (msg === 'client_mismatch') {
      sendError(ctx.res, 403, msg);
    } else if (msg === 'pairing_expired') {
      sendError(ctx.res, 410, msg);
    } else if (msg === 'pairing_not_found') {
      sendError(ctx.res, 404, msg);
    } else {
      sendError(ctx.res, 400, msg);
    }
  }
  return true;
}

async function handleClientCallFrame(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientId: string,
): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/calls\/([^/]+)\/frame/);
  const callId = parts?.[1] ?? '';
  if (!applyRateLimit(ctx, clientDataLimiter, `call:${callId}:frame`)) {
    return true;
  }
  const body = parseJsonBody<any>(ctx.body);

  if (!body?.envelope_json) {
    sendError(ctx.res, 400, 'envelope_json is required');
    return true;
  }

  try {
    await service.postClientFrame({
      call_id: callId,
      client_instance_id: clientId,
      envelope_json: body.envelope_json,
    });
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'frame failed';
    if (msg === 'client_mismatch') {
      sendError(ctx.res, 403, msg);
    } else if (msg === 'call_not_found') {
      sendError(ctx.res, 404, msg);
    } else {
      sendError(ctx.res, 400, msg);
    }
  }
  return true;
}

async function handleClientCallStreamFrame(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientId: string,
): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/calls\/([^/]+)\/stream-frame/);
  const callId = parts?.[1] ?? '';
  if (!applyRateLimit(ctx, clientStreamFrameLimiter, `call:${callId}:stream_frame`)) {
    return true;
  }
  const body = parseJsonBody<any>(ctx.body);

  if (!body?.frame_json || typeof body.frame_json !== 'string') {
    sendError(ctx.res, 400, 'frame_json is required');
    return true;
  }
  if (body.frame_json.length > MAX_STREAM_FRAME_BYTES) {
    sendError(ctx.res, 413, 'frame_json exceeds MAX_STREAM_FRAME_BYTES');
    return true;
  }

  try {
    await service.postClientStreamFrame({
      call_id: callId,
      client_instance_id: clientId,
      frame_json: body.frame_json,
    });
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'stream_frame failed';
    if (msg === 'client_mismatch') {
      sendError(ctx.res, 403, msg);
    } else if (msg === 'call_not_found') {
      sendError(ctx.res, 404, msg);
    } else {
      sendError(ctx.res, 400, msg);
    }
  }
  return true;
}

async function handleClientCallExit(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientId: string,
): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/calls\/([^/]+)\/exit/);
  const callId = parts?.[1] ?? '';
  if (!applyRateLimit(ctx, callerControlLimiter, `call:${callId}:exit`)) {
    return true;
  }
  const body = parseJsonBody<any>(ctx.body);

  try {
    await service.postClientExit({
      call_id: callId,
      client_instance_id: clientId,
      exit_code: body?.exit_code ?? 0,
      duration_ms: body?.duration_ms,
      stderr: body?.stderr,
      stdout_digest: body?.stdout_digest,
      stderr_digest: body?.stderr_digest,
      bytes_in: body?.bytes_in,
      bytes_out: body?.bytes_out,
      exit_encrypted: body?.exit_encrypted,
    });
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'exit failed';
    if (msg === 'client_mismatch') {
      sendError(ctx.res, 403, msg);
    } else if (msg === 'call_not_found') {
      sendError(ctx.res, 404, msg);
    } else {
      sendError(ctx.res, 400, msg);
    }
  }
  return true;
}

function handleRevokeAck(ctx: RequestContext): boolean {
  sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  return true;
}

async function handleDeleteGrantByClient(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientId: string,
): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/grants\/([^/]+)$/);
  const grantId = parts?.[1] ?? '';
  if (!applyRateLimit(ctx, clientQueryLimiter, `client:${clientId}:delete_grant:${grantId || 'unknown'}`)) {
    return true;
  }

  try {
    await service.removeGrantByClient(clientId, grantId);
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'delete failed';
    if (msg === 'client_instance_id_mismatch') {
      sendError(ctx.res, 403, msg);
    } else if (msg === 'grant_not_found') {
      sendError(ctx.res, 404, msg);
    } else {
      sendError(ctx.res, 400, msg);
    }
  }
  return true;
}

async function handleUpdateGrantByClient(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientId: string,
): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/grants\/([^/]+)$/);
  const grantId = parts?.[1] ?? '';
  if (!applyRateLimit(ctx, clientQueryLimiter, `client:${clientId}:update_grant:${grantId || 'unknown'}`)) {
    return true;
  }

  const body = parseJsonBody<any>(ctx.body);
  try {
    const grant = await service.updateGrantByClient(clientId, grantId, {
      grant_scope: body?.grant_scope,
      file_access: body?.file_access,
    });
    sendJson(ctx.res, 200, { code: 0, message: 'ok', data: toGrantApi(grant) });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'update failed';
    if (msg === 'client_instance_id_mismatch') {
      sendError(ctx.res, 403, msg);
    } else if (msg === 'grant_not_found') {
      sendError(ctx.res, 404, msg);
    } else {
      sendError(ctx.res, 400, msg);
    }
  }
  return true;
}

async function handleListActiveGrantsForClient(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientId: string,
): Promise<boolean> {
  if (!applyRateLimit(ctx, clientQueryLimiter, `client:${clientId}:active_grants`)) {
    return true;
  }
  const grants = await service.listActiveGrantsForClient(clientId);
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: grants.map(toGrantApi) });
  return true;
}

async function handleStartPairing(ctx: RequestContext, service: RemoteInvokeService, protocol: 'v4' | 'v5' = 'v4'): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  if (!body?.pair_code || !body?.caller_info || !body?.caller_ephemeral_pub) {
    sendError(ctx.res, 400, 'pair_code, caller_info and caller_ephemeral_pub are required');
    return true;
  }

  const ip = ctx.clientIp || '';
  if (ip && !checkPairRateLimit(ip)) {
    sendError(ctx.res, 429, 'pair_code rate limit exceeded');
    return true;
  }

  try {
    const result = await service.startPairing('', body, ctx.clientIp);
    sendJson(ctx.res, 200, {
      code: 0,
      message: 'ok',
      data: {
        ...result,
        approval_sse_url: `/${protocol}/remote-invoke/pairings/${result.pairing_id}/watch`,
      },
    });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'pairing failed';
    if (msg === 'invalid_pair_code' || msg === 'pair_code_already_consumed' || msg === 'pair_code_expired' || msg === 'pair_slot_occupied') {
      sendError(ctx.res, 400, msg);
    } else {
      sendError(ctx.res, 500, msg);
    }
  }
  return true;
}

async function handlePairingWatchV5(ctx: RequestContext, storage: IStorage): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v5\/remote-invoke\/pairings\/([^/]+)\/watch/);
  const pairingId = parts?.[1] ?? '';
  const watchToken = ctx.url.searchParams.get('watch_token') ?? '';
  if (!watchToken) {
    sendError(ctx.res, 401, 'watch_token_invalid');
    return true;
  }
  const pairing = await storage.remoteInvoke.getPairingByWatchTokenHash(sha256Hex(watchToken));
  if (!pairing || pairing.id !== pairingId) {
    sendError(ctx.res, 401, 'watch_token_invalid');
    return true;
  }
  if (pairing.expires_at && new Date(pairing.expires_at) < new Date()) {
    sendError(ctx.res, 401, 'watch_token_invalid');
    return true;
  }

  openSse(ctx.res);
  registerPairingWatcher(pairingId, ctx.res, sha256Hex(watchToken));

  writeSseEvent(ctx.res, 'connected', { pairing_id: pairingId });

  ctx.req.on('close', () => {
    unregisterPairingWatcher(pairingId, ctx.res);
  });

  return true;
}

function handlePairingWatch(ctx: RequestContext): boolean {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/pairings\/([^/]+)\/watch/);
  const pairingId = parts?.[1] ?? '';

  openSse(ctx.res);
  registerPairingWatcher(pairingId, ctx.res);

  writeSseEvent(ctx.res, 'connected', { pairing_id: pairingId });

  ctx.req.on('close', () => {
    unregisterPairingWatcher(pairingId, ctx.res);
  });

  return true;
}

async function handleGrantsLookupV5(
  ctx: RequestContext,
  storage: IStorage,
  service: RemoteInvokeService,
): Promise<boolean> {
  const pop = await requirePoP(ctx, storage);
  if (!pop) return true;
  const clientInstanceId = String(pop.body.client_instance_id ?? '');
  if (!clientInstanceId) {
    sendError(ctx.res, 400, 'client_instance_id is required');
    return true;
  }
  const callerEphemeralPub = validateCallerEphemeralPub(ctx, pop.body.caller_ephemeral_pub);
  if (!callerEphemeralPub) return true;
  if (!applyRateLimit(ctx, callerLookupLimiter, `${pop.callerPubkeyFp}:${clientInstanceId}:lookup`)) {
    return true;
  }
  try {
    const session = await service.lookupGrantSession({
      client_instance_id: clientInstanceId,
      caller_ephemeral_pub: callerEphemeralPub,
    }, pop.callerPubkeyFp);
    sendJson(ctx.res, 200, { code: 0, message: 'ok', data: session });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'grant_not_found';
    sendError(ctx.res, 404, msg);
  }
  return true;
}

async function handleGrantsClaimV5(
  ctx: RequestContext,
  storage: IStorage,
  service: RemoteInvokeService,
): Promise<boolean> {
  const pop = await requirePoP(ctx, storage);
  if (!pop) return true;
  const body = pop.body;
  if (!body.client_instance_id || !body.pair_code || !body.claim_token) {
    sendError(ctx.res, 400, 'client_instance_id, pair_code and claim_token are required');
    return true;
  }
  const callerEphemeralPub = validateCallerEphemeralPub(ctx, body.caller_ephemeral_pub);
  if (!callerEphemeralPub) return true;
  try {
    const session = await service.redeemClaim({
      client_instance_id: String(body.client_instance_id),
      pair_code: String(body.pair_code),
      claim_token: String(body.claim_token),
      caller_pubkey: pop.callerPubkey,
      caller_ephemeral_pub: callerEphemeralPub,
    }, pop.callerPubkeyFp);
    sendJson(ctx.res, 200, { code: 0, message: 'ok', data: session });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'claim_token_invalid';
    const status = msg === 'client_instance_id_mismatch' ? 403 : 401;
    sendError(ctx.res, status, msg);
  }
  return true;
}

async function handleGrantsRevokeV5(
  ctx: RequestContext,
  storage: IStorage,
  service: RemoteInvokeService,
): Promise<boolean> {
  const grant = await extractBearerGrantSession(ctx, service);
  if (!grant) return true;
  const pop = await requirePoP(ctx, storage, grant.caller_pubkey_fp);
  if (!pop) return true;
  try {
    await service.revokeGrantByBearer(extractBearerToken(ctx) ?? '', pop.callerPubkeyFp);
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    sendError(ctx.res, 401, e instanceof Error ? e.message : 'grant_session_token_invalid');
  }
  return true;
}

async function handleOpenCallV5(
  ctx: RequestContext,
  storage: IStorage,
  service: RemoteInvokeService,
): Promise<boolean> {
  const grant = await extractBearerGrantSession(ctx, service);
  if (!grant) return true;
  const pop = await requirePoP(ctx, storage, grant.caller_pubkey_fp);
  if (!pop) return true;
  const body = pop.body;
  const commandKind = resolveCommandKind(body?.command_kind);
  if (!body?.client_instance_id || !body?.command_encrypted) {
    sendError(ctx.res, 400, 'client_instance_id and command_encrypted are required');
    return true;
  }
  if (Object.prototype.hasOwnProperty.call(body, 'grant_id')) {
    sendError(ctx.res, 400, 'grant_id is not accepted in v5');
    return true;
  }
  if (!applyRateLimit(ctx, callerOpenLimiter, `${pop.callerPubkeyFp}:${body.client_instance_id}:open`)) {
    return true;
  }

  try {
    const result = await service.openCall(grant, {
      client_instance_id: String(body.client_instance_id),
      caller_pubkey: pop.callerPubkey,
      command_summary: body.command_summary ?? { command_preview: commandKind },
      command_kind: commandKind,
      command_encrypted: body.command_encrypted,
      pty_enabled: body.pty_enabled,
      timeout_hint_ms: body.timeout_hint_ms,
    });
    sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'open call failed';
    if (msg === 'client_instance_id_mismatch' || msg === 'grant_expired' || msg === 'grant_not_active' || msg === 'grant_consumed' || msg === 'grant_scope_mismatch') {
      sendError(ctx.res, 403, msg);
    } else if (msg === 'grant_not_found') {
      sendError(ctx.res, 404, msg);
    } else {
      sendError(ctx.res, 400, msg);
    }
  }
  return true;
}

function isoToUnixMs(iso: string | undefined | null): number | null {
  if (!iso) return null;
  const ms = new Date(iso).getTime();
  return Number.isNaN(ms) ? null : ms;
}

function toGrantApi(g: RemoteInvokeGrant) {
  const firstAuthorizedAt = isoToUnixMs(g.first_authorized_at) ?? 0;
  return {
    grant_id: g.id,
    client_instance_id: g.client_instance_id,
    caller_fingerprint: g.caller_fingerprint,
    caller_display_name: g.caller_display_name || null,
    caller_ephemeral_pub: g.caller_ephemeral_pub || null,
    client_ephemeral_pub: g.client_ephemeral_pub || null,
    grant_mode: g.grant_mode,
    grant_scope: normalizeGrantScope(g.grant_scope),
    status: g.status,
    created_at: firstAuthorizedAt,
    first_authorized_at: firstAuthorizedAt,
    expires_at: isoToUnixMs(g.expires_at),
    last_used_at: isoToUnixMs(g.last_used_at),
    max_calls: g.max_calls,
    remaining_calls: g.remaining_calls,
    use_count: g.max_calls - g.remaining_calls,
    ssh_key_id: g.ssh_key_id || null,
    ssh_key_fingerprint: g.ssh_key_fingerprint || null,
    file_access: g.file_access || 'none',
  };
}

async function handleFindReusableGrant(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const clientInstanceId = ctx.url.searchParams.get('client_instance_id') ?? '';
  const callerFingerprint = ctx.url.searchParams.get('caller_fingerprint') ?? '';

  if (!clientInstanceId || !callerFingerprint) {
    sendError(ctx.res, 400, 'client_instance_id and caller_fingerprint are required');
    return true;
  }
  if (!applyRateLimit(ctx, callerLookupLimiter, `${callerAccessKey(ctx)}:reusable_grant:${clientInstanceId}:${callerFingerprint}`)) {
    return true;
  }

  const grant = await service.findReusableGrant('', clientInstanceId, callerFingerprint);
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: grant ? toGrantApi(grant) : null });
  return true;
}

async function handleDeleteGrant(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/grants\/([^/]+)$/);
  const grantId = parts?.[1] ?? '';
  const callerFingerprint = ctx.url.searchParams.get('caller_fingerprint') ?? '';

  if (!callerFingerprint) {
    sendError(ctx.res, 400, 'caller_fingerprint query parameter is required');
    return true;
  }
  if (!applyRateLimit(ctx, callerControlLimiter, `${callerAccessKey(ctx)}:delete_grant:${grantId}:${callerFingerprint}`)) {
    return true;
  }

  try {
    await service.removeGrant('', grantId, callerFingerprint);
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'delete failed';
    if (msg === 'caller_fingerprint_mismatch') {
      sendError(ctx.res, 403, msg);
    } else if (msg === 'grant_not_found') {
      sendError(ctx.res, 404, msg);
    } else {
      sendError(ctx.res, 400, msg);
    }
  }
  return true;
}

async function handleCallInput(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/calls\/([^/]+)\/input/);
  const callId = parts?.[1] ?? '';
  if (!applyRateLimit(ctx, callerControlLimiter, `${callerAccessKey(ctx)}:call_input:${callId}`)) {
    return true;
  }

  const token = extractBearerToken(ctx);
  if (!token || !service.verifyCallToken(callId, token)) {
    sendError(ctx.res, 401, 'invalid or missing relay_token');
    return true;
  }

  const body = parseJsonBody<any>(ctx.body);
  const envelopeJson = body?.envelope_json ?? ctx.body;

  try {
    await service.postCallerInput(callId, envelopeJson);
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    sendError(ctx.res, 400, e instanceof Error ? e.message : 'input failed');
  }
  return true;
}

function handleCallEvents(ctx: RequestContext, service: RemoteInvokeService): boolean {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/calls\/([^/]+)\/events/);
  const callId = parts?.[1] ?? '';
  if (!applyRateLimit(ctx, callerControlLimiter, `${callerAccessKey(ctx)}:call_events:${callId}`)) {
    return true;
  }

  const token = extractBearerToken(ctx);
  if (!token || !service.verifyCallToken(callId, token)) {
    sendError(ctx.res, 401, 'invalid or missing relay_token');
    return true;
  }

  openSse(ctx.res);
  registerCallerEventStream(callId, ctx.res);
  writeSseEvent(ctx.res, 'connected', { call_id: callId });
  flushCallerEventStream(callId);

  ctx.req.on('close', () => {
    unregisterCallerEventStream(callId);
  });

  return true;
}

async function handleCallStatus(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/calls\/([^/]+)\/status/);
  const callId = parts?.[1] ?? '';
  if (!applyRateLimit(ctx, callerControlLimiter, `${callerAccessKey(ctx)}:call_status:${callId}`)) {
    return true;
  }

  const token = extractBearerToken(ctx);
  if (!token || !service.verifyCallToken(callId, token)) {
    sendError(ctx.res, 401, 'invalid or missing relay_token');
    return true;
  }

  const call = await service.getCall('', callId);
  if (!call) {
    sendError(ctx.res, 404, 'call not found');
    return true;
  }

  sendJson(ctx.res, 200, {
    code: 0,
    message: 'ok',
    data: {
      call_id: call.id,
      status: call.status,
      exit_code: call.exit_code,
      duration_ms: call.duration_ms || null,
      ended_at: call.ended_at || null,
    },
  });
  return true;
}

async function handleCancelCall(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/calls\/([^/]+)\/cancel/);
  const callId = parts?.[1] ?? '';
  if (!applyRateLimit(ctx, callerControlLimiter, `${callerAccessKey(ctx)}:cancel:${callId}`)) {
    return true;
  }

  const token = extractBearerToken(ctx);
  if (!token || !service.verifyCallToken(callId, token)) {
    sendError(ctx.res, 401, 'invalid or missing relay_token');
    return true;
  }

  try {
    await service.cancelCall(callId);
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    sendError(ctx.res, 400, e instanceof Error ? e.message : 'cancel failed');
  }
  return true;
}

function toCallApi(c: RemoteInvokeCall) {
  let commandSummary = { command_preview: '' };
  let commandObj: Record<string, unknown> = { kind: 'query.readonly' };
  try { commandSummary = JSON.parse(c.command_summary_json); } catch { /* ignore */ }
  try { commandObj = JSON.parse(c.command_json); } catch { /* ignore */ }
  const commandKind = resolveCommandKind(
    typeof commandSummary === 'object' && commandSummary && 'command_kind' in commandSummary
      ? String((commandSummary as Record<string, unknown>).command_kind ?? '')
      : typeof commandObj.kind === 'string'
        ? String(commandObj.kind)
        : 'query.readonly',
  );
  const startedAt = isoToUnixMs(c.started_at) ?? 0;
  const endedAt = isoToUnixMs(c.ended_at);
  return {
    call_id: c.id,
    grant_id: c.grant_id,
    pairing_id: c.pairing_id || null,
    client_instance_id: c.client_instance_id,
    caller_fingerprint: c.caller_fingerprint,
    status: c.status,
    command_kind: commandKind,
    command_summary: commandSummary,
    command_detail: commandObj,
    source_ip: c.source_ip || null,
    caller_display_name: c.caller_display_name || null,
    payload_digest: c.payload_digest || null,
    stdout_digest: c.stdout_digest || null,
    stderr_digest: c.stderr_digest || null,
    exit_code: c.exit_code === -1 ? null : c.exit_code,
    created_at: startedAt,
    started_at: startedAt,
    finished_at: endedAt,
    ended_at: endedAt,
    duration_ms: c.duration_ms || null,
    bytes_in: c.bytes_in || null,
    bytes_out: c.bytes_out || null,
  };
}

async function handleListCalls(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientInstanceId: string,
): Promise<boolean> {
  if (!applyRateLimit(ctx, clientQueryLimiter, `client:${clientInstanceId}:list_calls`)) {
    return true;
  }
  const query = {
    caller_fingerprint: ctx.url.searchParams.get('caller_fingerprint') ?? undefined,
    status: ctx.url.searchParams.get('status') ?? undefined,
    offset: parseInt(ctx.url.searchParams.get('offset') ?? '0', 10),
    limit: parseInt(ctx.url.searchParams.get('limit') ?? '100', 10),
  };

  const result = await service.listCallsForClient(clientInstanceId, query);
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: { list: result.list.map(toCallApi), total: result.total } });
  return true;
}

async function handleGetCall(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientInstanceId: string,
): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/calls\/([^/]+)$/);
  const callId = parts?.[1] ?? '';
  if (!applyRateLimit(ctx, clientQueryLimiter, `client:${clientInstanceId}:get_call:${callId}`)) {
    return true;
  }

  const call = await service.getCallForClient(clientInstanceId, callId);
  if (!call) {
    sendError(ctx.res, 404, 'call not found');
    return true;
  }

  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: toCallApi(call) });
  return true;
}

async function handleGetPendingPairings(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientInstanceId: string,
): Promise<boolean> {
  if (!applyRateLimit(ctx, clientQueryLimiter, `client:${clientInstanceId}:pending_pairings`)) {
    return true;
  }
  const pairings = await service.getPendingPairingsForClient(clientInstanceId);
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: pairings });
  return true;
}

async function handleCancelPendingPairings(
  ctx: RequestContext,
  service: RemoteInvokeService,
  clientInstanceId: string,
): Promise<boolean> {
  if (!applyRateLimit(ctx, clientQueryLimiter, `client:${clientInstanceId}:cancel_pending_pairings`)) {
    return true;
  }
  const cancelled = await service.cancelPendingPairings(clientInstanceId);
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: { cancelled } });
  return true;
}

async function handleOpenCall(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  const commandKind = resolveCommandKind(body?.command_kind);
  if (!body?.grant_id || !body?.client_instance_id || !body?.caller_fingerprint) {
    sendError(ctx.res, 400, 'grant_id, client_instance_id, and caller_fingerprint are required');
    return true;
  }
  if (!body?.command_encrypted) {
    sendError(ctx.res, 400, 'command_encrypted is required');
    return true;
  }
  if (!applyRateLimit(ctx, callerOpenLimiter, `${callerAccessKey(ctx)}:open:${body.grant_id}:${body.client_instance_id}:${body.caller_fingerprint}`)) {
    return true;
  }

  try {
    const grant = await storage.remoteInvoke.getGrant(String(body.grant_id));
    if (!grant) {
      sendError(ctx.res, 404, 'grant_not_found');
      return true;
    }
    const result = await service.openCall(grant, {
      client_instance_id: body.client_instance_id,
      caller_pubkey: body.caller_pubkey ?? '',
      command_summary: body.command_summary ?? { command_preview: commandKind },
      command_kind: commandKind,
      command_encrypted: body.command_encrypted,
      pty_enabled: body.pty_enabled,
      timeout_hint_ms: body.timeout_hint_ms,
    });
    sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'open call failed';
    if (msg === 'caller_fingerprint_mismatch' || msg === 'client_instance_id_mismatch' || msg === 'grant_expired' || msg === 'grant_not_active' || msg === 'grant_consumed' || msg === 'grant_scope_mismatch') {
      sendError(ctx.res, 403, msg);
    } else if (msg === 'grant_not_found') {
      sendError(ctx.res, 404, msg);
    } else {
      sendError(ctx.res, 400, msg);
    }
  }
  return true;
}
