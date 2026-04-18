import type { IStorage } from '../dao/types';
import type { RequestContext } from '../http';
import {
  sendJson,
  sendError,
  requireAuth,
  parseJsonBody,
  extractPathParam,
  openSse,
  writeSseEvent,
  closeSse,
  extractBearerToken,
} from '../http';
import type { RemoteInvokeConfig } from '../types';
import { RemoteInvokeService } from '../remote-invoke/service';
import {
  registerClientStream,
  unregisterClientStream,
  registerPairingWatcher,
  unregisterPairingWatcher,
  registerCallerEventStream,
  unregisterCallerEventStream,
  startKeepalive,
} from '../remote-invoke/sse';
import { startCleanupScheduler } from '../remote-invoke/cleanup';
import type { ClientStreamState } from '../remote-invoke/types';
import { customAlphabet } from 'nanoid';

const nanoid = customAlphabet('0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_', 21);

let serviceInstance: RemoteInvokeService | null = null;

function getService(storage: IStorage, config: RemoteInvokeConfig): RemoteInvokeService {
  if (!serviceInstance) {
    serviceInstance = new RemoteInvokeService(storage, config);
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
  retention_days: 90,
  max_records: 10000,
  max_sse_connections_per_client: 2,
  max_sse_connections_per_ip: 10,
  pair_rate_limit_per_ip: 5,
  pair_rate_limit_global_per_client: 10,
};

export async function handleRemoteInvoke(
  ctx: RequestContext,
  storage: IStorage,
  remoteInvokeConfig?: RemoteInvokeConfig,
): Promise<boolean> {
  const { url, req } = ctx;
  const method = req.method ?? 'GET';
  const pathname = url.pathname.replace(/\/$/, '') || '/';

  if (!pathname.startsWith('/v4/remote-invoke')) return false;

  const config = remoteInvokeConfig ?? DEFAULT_REMOTE_INVOKE_CONFIG;

  if (!config.enabled) {
    sendError(ctx.res, 403, 'remote invoke is not enabled');
    return true;
  }

  const service = getService(storage, config);

  try {
    if (pathname === '/v4/remote-invoke/client/register' && method === 'POST') {
      return await handleClientRegister(ctx, service);
    }

    if (pathname === '/v4/remote-invoke/client/stream' && method === 'GET') {
      return await handleClientStream(ctx, storage, service);
    }

    if (pathname === '/v4/remote-invoke/client/heartbeat' && method === 'POST') {
      return await handleClientHeartbeat(ctx, storage, service);
    }

    if (pathname === '/v4/remote-invoke/client/pair-code' && method === 'POST') {
      return await handlePublishPairCode(ctx, storage, service);
    }

    if (pathname.startsWith('/v4/remote-invoke/client/discovery-session/') && method === 'DELETE') {
      return await handleCloseDiscovery(ctx, storage, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/grants\/[^/]+\/decision$/) && method === 'POST') {
      return await handleGrantDecision(ctx, storage, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/calls\/[^/]+\/frame$/) && method === 'POST') {
      return await handleClientCallFrame(ctx, storage, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/calls\/[^/]+\/exit$/) && method === 'POST') {
      return await handleClientCallExit(ctx, storage, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/grants\/[^/]+\/revoke-ack$/) && method === 'POST') {
      return await handleRevokeAck(ctx, storage, service);
    }

    if (pathname === '/v4/remote-invoke/pairings/start' && method === 'POST') {
      return await handleStartPairing(ctx, storage, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/pairings\/[^/]+\/watch$/) && method === 'GET') {
      return await handlePairingWatch(ctx, storage, service);
    }

    if (pathname === '/v4/remote-invoke/grants/reusable' && method === 'GET') {
      return await handleFindReusableGrant(ctx, storage, service);
    }

    if (pathname === '/v4/remote-invoke/grants' && method === 'GET') {
      return await handleListGrants(ctx, storage, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/grants\/[^/]+$/) && method === 'PATCH') {
      return await handleUpdateGrant(ctx, storage, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/grants\/[^/]+$/) && method === 'DELETE') {
      return await handleDeleteGrant(ctx, storage, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/calls\/[^/]+\/input$/) && method === 'POST') {
      return await handleCallInput(ctx, storage, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/calls\/[^/]+\/events$/) && method === 'GET') {
      return await handleCallEvents(ctx, storage, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/calls\/[^/]+\/cancel$/) && method === 'POST') {
      return await handleCancelCall(ctx, storage, service);
    }

    if (pathname === '/v4/remote-invoke/calls' && method === 'GET') {
      return await handleListCalls(ctx, storage, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/calls\/[^/]+$/) && method === 'GET') {
      return await handleGetCall(ctx, storage, service);
    }

    if (pathname === '/v4/remote-invoke/clients' && method === 'GET') {
      return await handleListClients(ctx, storage, service);
    }

    if (pathname === '/v4/remote-invoke/calls/open' && method === 'POST') {
      return await handleOpenCall(ctx, storage, service);
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

async function requireClientAuth(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<string | null> {
  const token = extractBearerToken(ctx.req) ?? ctx.url.searchParams.get('client_auth_token') ?? '';
  const clientId = parseJsonBody<{ client_instance_id?: string }>(ctx.body)?.client_instance_id
    ?? ctx.url.searchParams.get('client_instance_id') ?? '';
  if (!token || !clientId) {
    sendError(ctx.res, 401, 'missing client_auth_token or client_instance_id');
    return null;
  }
  const record = await service.verifyClientAuth(clientId, token);
  if (!record) {
    sendError(ctx.res, 401, 'invalid client_auth_token');
    return null;
  }
  return clientId;
}

async function handleClientRegister(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  if (!body?.client_instance_id || !body?.client_long_term_pubkey) {
    sendError(ctx.res, 400, 'client_instance_id and client_long_term_pubkey are required');
    return true;
  }
  try {
    const result = await service.registerClient(body);
    sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  } catch (e: unknown) {
    sendError(ctx.res, 400, e instanceof Error ? e.message : 'register failed');
  }
  return true;
}

async function handleClientStream(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  const clientId = ctx.url.searchParams.get('client_instance_id') ?? '';
  const token = ctx.url.searchParams.get('client_auth_token') ?? '';
  const streamId = ctx.url.searchParams.get('stream_id') ?? nanoid();
  const userId = ctx.url.searchParams.get('user_id') ?? '';

  if (!clientId || !token) {
    sendError(ctx.res, 401, 'missing client_instance_id or client_auth_token');
    return true;
  }

  const record = await service.verifyClientAuth(clientId, token);
  if (!record) {
    sendError(ctx.res, 401, 'invalid client_auth_token');
    return true;
  }

  openSse(ctx.res);

  const state: ClientStreamState = {
    clientInstanceId: clientId,
    userId: userId,
    streamId: streamId,
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

async function handleClientHeartbeat(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  const clientId = await requireClientAuth(ctx, storage, service);
  if (!clientId) return true;

  const body = parseJsonBody<any>(ctx.body);
  await service.clientHeartbeat({
    client_instance_id: clientId,
    stream_id: body?.stream_id ?? '',
    active_call_ids: body?.active_call_ids,
  });

  sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  return true;
}

async function handlePublishPairCode(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  const clientId = await requireClientAuth(ctx, storage, service);
  if (!clientId) return true;

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

async function handleCloseDiscovery(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  const clientId = await requireClientAuth(ctx, storage, service);
  if (!clientId) return true;

  const sessionId = extractPathParam(ctx.url.pathname, '/v4/remote-invoke/client/discovery-session/');
  await service.closeDiscoverySession(clientId, sessionId);

  sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  return true;
}

async function handleGrantDecision(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  const clientId = await requireClientAuth(ctx, storage, service);
  if (!clientId) return true;

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/grants\/([^/]+)\/decision/);
  const pairingId = parts?.[1] ?? '';

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
      client_ephemeral_pub: body.client_ephemeral_pub,
    });
    sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  } catch (e: unknown) {
    sendError(ctx.res, 400, e instanceof Error ? e.message : 'decision failed');
  }
  return true;
}

async function handleClientCallFrame(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  const clientId = await requireClientAuth(ctx, storage, service);
  if (!clientId) return true;

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/calls\/([^/]+)\/frame/);
  const callId = parts?.[1] ?? '';

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
    sendError(ctx.res, 400, e instanceof Error ? e.message : 'frame failed');
  }
  return true;
}

async function handleClientCallExit(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  const clientId = await requireClientAuth(ctx, storage, service);
  if (!clientId) return true;

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/calls\/([^/]+)\/exit/);
  const callId = parts?.[1] ?? '';

  const body = parseJsonBody<any>(ctx.body);
  try {
    await service.postClientExit({
      call_id: callId,
      client_instance_id: clientId,
      exit_code: body?.exit_code ?? 0,
      duration_ms: body?.duration_ms,
      stdout_digest: body?.stdout_digest,
      stderr_digest: body?.stderr_digest,
      bytes_in: body?.bytes_in,
      bytes_out: body?.bytes_out,
    });
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    sendError(ctx.res, 400, e instanceof Error ? e.message : 'exit failed');
  }
  return true;
}

async function handleRevokeAck(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  const clientId = await requireClientAuth(ctx, storage, service);
  if (!clientId) return true;

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/grants\/([^/]+)\/revoke-ack/);
  const grantId = parts?.[1] ?? '';

  await service.revokeAck(grantId);
  sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  return true;
}

async function handleStartPairing(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  if (!(await requireAuth(ctx, storage))) return true;

  const body = parseJsonBody<any>(ctx.body);
  if (!body?.client_instance_id || !body?.pair_code || !body?.caller_pubkey || !body?.caller_info) {
    sendError(ctx.res, 400, 'client_instance_id, pair_code, caller_pubkey, and caller_info are required');
    return true;
  }

  try {
    const result = await service.startPairing(ctx.user!.user_id, body, ctx.clientIp);
    sendJson(ctx.res, 200, {
      code: 0,
      message: 'ok',
      data: {
        ...result,
        approval_sse_url: `/v4/remote-invoke/pairings/${result.pairing_id}/watch`,
      },
    });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : 'pairing failed';
    if (msg === 'invalid_pair_code') {
      sendError(ctx.res, 400, msg);
    } else if (msg === 'unsupported_command') {
      sendError(ctx.res, 400, msg);
    } else {
      sendError(ctx.res, 500, msg);
    }
  }
  return true;
}

async function handlePairingWatch(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  if (!(await requireAuth(ctx, storage))) return true;

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/pairings\/([^/]+)\/watch/);
  const pairingId = parts?.[1] ?? '';

  openSse(ctx.res);
  registerPairingWatcher(pairingId, ctx.res);

  writeSseEvent(ctx.res, 'connected', { pairing_id: pairingId });

  ctx.req.on('close', () => {
    unregisterPairingWatcher(pairingId);
  });

  return true;
}

async function handleFindReusableGrant(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  if (!(await requireAuth(ctx, storage))) return true;

  const clientInstanceId = ctx.url.searchParams.get('client_instance_id') ?? '';
  const callerFingerprint = ctx.url.searchParams.get('caller_fingerprint') ?? '';

  if (!clientInstanceId || !callerFingerprint) {
    sendError(ctx.res, 400, 'client_instance_id and caller_fingerprint are required');
    return true;
  }

  const grant = await service.findReusableGrant(ctx.user!.user_id, clientInstanceId, callerFingerprint);
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: grant });
  return true;
}

async function handleListGrants(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  if (!(await requireAuth(ctx, storage))) return true;

  const query = {
    client_instance_id: ctx.url.searchParams.get('client_instance_id') ?? undefined,
    status: ctx.url.searchParams.get('status') ?? undefined,
    offset: parseInt(ctx.url.searchParams.get('offset') ?? '0', 10),
    limit: parseInt(ctx.url.searchParams.get('limit') ?? '100', 10),
  };

  const result = await service.listGrants(ctx.user!.user_id, query);
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  return true;
}

async function handleUpdateGrant(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  if (!(await requireAuth(ctx, storage))) return true;

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/grants\/([^/]+)$/);
  const grantId = parts?.[1] ?? '';

  const body = parseJsonBody<any>(ctx.body);
  try {
    await service.updateGrant(ctx.user!.user_id, grantId, body ?? {});
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    sendError(ctx.res, 400, e instanceof Error ? e.message : 'update failed');
  }
  return true;
}

async function handleDeleteGrant(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  if (!(await requireAuth(ctx, storage))) return true;

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/grants\/([^/]+)$/);
  const grantId = parts?.[1] ?? '';

  try {
    await service.removeGrant(ctx.user!.user_id, grantId);
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    sendError(ctx.res, 400, e instanceof Error ? e.message : 'delete failed');
  }
  return true;
}

async function handleCallInput(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  const token = extractBearerToken(ctx.req);
  if (!token) {
    sendError(ctx.res, 401, 'missing relay_token');
    return true;
  }

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/calls\/([^/]+)\/input/);
  const callId = parts?.[1] ?? '';

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

async function handleCallEvents(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  const token = extractBearerToken(ctx.req);
  const isCallerSse = !!token;

  if (isCallerSse) {
    const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/calls\/([^/]+)\/events/);
    const callId = parts?.[1] ?? '';

    openSse(ctx.res);
    registerCallerEventStream(callId, ctx.res);
    writeSseEvent(ctx.res, 'connected', { call_id: callId });

    ctx.req.on('close', () => {
      unregisterCallerEventStream(callId);
    });

    return true;
  }

  if (!(await requireAuth(ctx, storage))) return true;

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/calls\/([^/]+)\/events/);
  const callId = parts?.[1] ?? '';
  const offset = parseInt(ctx.url.searchParams.get('offset') ?? '0', 10);
  const limit = parseInt(ctx.url.searchParams.get('limit') ?? '500', 10);

  const result = await service.listCallEvents(callId, { offset, limit });
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  return true;
}

async function handleCancelCall(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  const token = extractBearerToken(ctx.req);
  if (!token) {
    sendError(ctx.res, 401, 'missing relay_token');
    return true;
  }

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/calls\/([^/]+)\/cancel/);
  const callId = parts?.[1] ?? '';

  try {
    await service.cancelCall(callId);
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    sendError(ctx.res, 400, e instanceof Error ? e.message : 'cancel failed');
  }
  return true;
}

async function handleListCalls(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  if (!(await requireAuth(ctx, storage))) return true;

  const query = {
    client_instance_id: ctx.url.searchParams.get('client_instance_id') ?? undefined,
    caller_fingerprint: ctx.url.searchParams.get('caller_fingerprint') ?? undefined,
    status: ctx.url.searchParams.get('status') ?? undefined,
    offset: parseInt(ctx.url.searchParams.get('offset') ?? '0', 10),
    limit: parseInt(ctx.url.searchParams.get('limit') ?? '100', 10),
  };

  const result = await service.listCalls(ctx.user!.user_id, query);
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  return true;
}

async function handleGetCall(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  if (!(await requireAuth(ctx, storage))) return true;

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/calls\/([^/]+)$/);
  const callId = parts?.[1] ?? '';

  const call = await service.getCall(ctx.user!.user_id, callId);
  if (!call) {
    sendError(ctx.res, 404, 'call not found');
    return true;
  }

  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: call });
  return true;
}

async function handleListClients(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  if (!(await requireAuth(ctx, storage))) return true;

  const result = await service.getOnlineClients(ctx.user!.user_id);
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  return true;
}

async function handleOpenCall(ctx: RequestContext, storage: IStorage, service: RemoteInvokeService): Promise<boolean> {
  if (!(await requireAuth(ctx, storage))) return true;

  const body = parseJsonBody<any>(ctx.body);
  if (!body?.grant_id || !body?.client_instance_id || !body?.command) {
    sendError(ctx.res, 400, 'grant_id, client_instance_id, and command are required');
    return true;
  }

  try {
    const result = await service.openCall(ctx.user!.user_id, {
      grant_id: body.grant_id,
      client_instance_id: body.client_instance_id,
      caller_pubkey: body.caller_pubkey ?? '',
      command_summary: body.command_summary ?? { command_preview: body.command.command },
      command: body.command,
    });
    sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  } catch (e: unknown) {
    sendError(ctx.res, 400, e instanceof Error ? e.message : 'open call failed');
  }
  return true;
}
