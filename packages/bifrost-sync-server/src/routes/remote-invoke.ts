import type { IStorage } from '../dao/types';
import type { RequestContext } from '../http';
import {
  sendJson,
  sendError,
  parseJsonBody,
  extractPathParam,
  openSse,
  writeSseEvent,
} from '../http';
import type { RemoteInvokeConfig, RemoteInvokeGrant, RemoteInvokeCall } from '../types';
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
  max_grants_per_client: 20,
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
      return handleClientStream(ctx, service);
    }

    if (pathname === '/v4/remote-invoke/client/heartbeat' && method === 'POST') {
      return await handleClientHeartbeat(ctx, service);
    }

    if (pathname === '/v4/remote-invoke/client/pair-code' && method === 'POST') {
      return await handlePublishPairCode(ctx, service);
    }

    if (pathname.startsWith('/v4/remote-invoke/client/discovery-session/') && method === 'DELETE') {
      return await handleCloseDiscovery(ctx, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/grants\/[^/]+\/decision$/) && method === 'POST') {
      return await handleGrantDecision(ctx, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/calls\/[^/]+\/frame$/) && method === 'POST') {
      return await handleClientCallFrame(ctx, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/calls\/[^/]+\/exit$/) && method === 'POST') {
      return await handleClientCallExit(ctx, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/client\/grants\/[^/]+\/revoke-ack$/) && method === 'POST') {
      return handleRevokeAck(ctx);
    }

    if (pathname === '/v4/remote-invoke/pairings/start' && method === 'POST') {
      return await handleStartPairing(ctx, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/pairings\/[^/]+\/watch$/) && method === 'GET') {
      return handlePairingWatch(ctx);
    }

    if (pathname === '/v4/remote-invoke/grants/reusable' && method === 'GET') {
      return await handleFindReusableGrant(ctx, service);
    }

    if (pathname === '/v4/remote-invoke/grants' && method === 'GET') {
      return await handleListGrants(ctx, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/grants\/[^/]+$/) && method === 'PATCH') {
      return await handleUpdateGrant(ctx, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/grants\/[^/]+$/) && method === 'DELETE') {
      return await handleDeleteGrant(ctx, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/calls\/[^/]+\/input$/) && method === 'POST') {
      return await handleCallInput(ctx, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/calls\/[^/]+\/events$/) && method === 'GET') {
      return handleCallEvents(ctx);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/calls\/[^/]+\/cancel$/) && method === 'POST') {
      return await handleCancelCall(ctx, service);
    }

    if (pathname === '/v4/remote-invoke/calls' && method === 'GET') {
      return await handleListCalls(ctx, service);
    }

    if (pathname.match(/^\/v4\/remote-invoke\/calls\/[^/]+$/) && method === 'GET') {
      return await handleGetCall(ctx, service);
    }

    if (pathname === '/v4/remote-invoke/clients' && method === 'GET') {
      return await handleListClients(ctx, service);
    }

    if (pathname === '/v4/remote-invoke/calls/open' && method === 'POST') {
      return await handleOpenCall(ctx, service);
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

function handleClientStream(ctx: RequestContext, service: RemoteInvokeService): boolean {
  const clientId = ctx.url.searchParams.get('client_instance_id') ?? '';
  const streamId = ctx.url.searchParams.get('stream_id') ?? nanoid();
  const userId = ctx.url.searchParams.get('user_id') ?? '';

  if (!clientId) {
    sendError(ctx.res, 400, 'client_instance_id is required');
    return true;
  }

  openSse(ctx.res);

  const state: ClientStreamState = {
    clientInstanceId: clientId,
    userId,
    streamId,
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

async function handleClientHeartbeat(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  const clientId = body?.client_instance_id ?? '';
  if (!clientId) {
    sendError(ctx.res, 400, 'client_instance_id is required');
    return true;
  }

  await service.clientHeartbeat({
    client_instance_id: clientId,
    stream_id: body?.stream_id ?? '',
    active_call_ids: body?.active_call_ids,
  });

  sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  return true;
}

async function handlePublishPairCode(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  const clientId = body?.client_instance_id ?? '';
  if (!clientId || !body?.pair_code) {
    sendError(ctx.res, 400, 'client_instance_id and pair_code are required');
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

async function handleCloseDiscovery(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  const clientId = body?.client_instance_id ?? ctx.url.searchParams.get('client_instance_id') ?? '';
  if (!clientId) {
    sendError(ctx.res, 400, 'client_instance_id is required');
    return true;
  }

  const sessionId = extractPathParam(ctx.url.pathname, '/v4/remote-invoke/client/discovery-session/');
  await service.closeDiscoverySession(clientId, sessionId);

  sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  return true;
}

async function handleGrantDecision(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  const clientId = body?.client_instance_id ?? '';

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/grants\/([^/]+)\/decision/);
  const pairingId = parts?.[1] ?? '';

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

async function handleClientCallFrame(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  const clientId = body?.client_instance_id ?? '';

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/calls\/([^/]+)\/frame/);
  const callId = parts?.[1] ?? '';

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

async function handleClientCallExit(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  const clientId = body?.client_instance_id ?? '';

  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/client\/calls\/([^/]+)\/exit/);
  const callId = parts?.[1] ?? '';

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

function handleRevokeAck(ctx: RequestContext): boolean {
  sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  return true;
}

async function handleStartPairing(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  if (!body?.pair_code || !body?.caller_info) {
    sendError(ctx.res, 400, 'pair_code and caller_info are required');
    return true;
  }

  try {
    const result = await service.startPairing('', body, ctx.clientIp);
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
    if (msg === 'invalid_pair_code' || msg === 'pair_code_already_consumed' || msg === 'pair_code_expired' || msg === 'unsupported_command' || msg === 'pair_slot_occupied') {
      sendError(ctx.res, 400, msg);
    } else {
      sendError(ctx.res, 500, msg);
    }
  }
  return true;
}

function handlePairingWatch(ctx: RequestContext): boolean {
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
    grant_mode: g.grant_mode,
    grant_scope: g.grant_scope,
    status: g.status,
    created_at: firstAuthorizedAt,
    first_authorized_at: firstAuthorizedAt,
    expires_at: isoToUnixMs(g.expires_at),
    last_used_at: isoToUnixMs(g.last_used_at),
    max_calls: g.max_calls,
    remaining_calls: g.remaining_calls,
    use_count: g.max_calls - g.remaining_calls,
  };
}

async function handleFindReusableGrant(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const clientInstanceId = ctx.url.searchParams.get('client_instance_id') ?? '';
  const callerFingerprint = ctx.url.searchParams.get('caller_fingerprint') ?? '';

  if (!clientInstanceId || !callerFingerprint) {
    sendError(ctx.res, 400, 'client_instance_id and caller_fingerprint are required');
    return true;
  }

  const grant = await service.findReusableGrant('', clientInstanceId, callerFingerprint);
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: grant ? toGrantApi(grant) : null });
  return true;
}

async function handleListGrants(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const query = {
    client_instance_id: ctx.url.searchParams.get('client_instance_id') ?? undefined,
    status: ctx.url.searchParams.get('status') ?? undefined,
    offset: parseInt(ctx.url.searchParams.get('offset') ?? '0', 10),
    limit: parseInt(ctx.url.searchParams.get('limit') ?? '100', 10),
  };

  const result = await service.listGrants('', query);
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: { list: result.list.map(toGrantApi), total: result.total } });
  return true;
}

async function handleUpdateGrant(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/grants\/([^/]+)$/);
  const grantId = parts?.[1] ?? '';

  const body = parseJsonBody<any>(ctx.body);
  try {
    await service.updateGrant('', grantId, body ?? {});
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    sendError(ctx.res, 400, e instanceof Error ? e.message : 'update failed');
  }
  return true;
}

async function handleDeleteGrant(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/grants\/([^/]+)$/);
  const grantId = parts?.[1] ?? '';

  try {
    await service.removeGrant('', grantId);
    sendJson(ctx.res, 200, { code: 0, message: 'ok' });
  } catch (e: unknown) {
    sendError(ctx.res, 400, e instanceof Error ? e.message : 'delete failed');
  }
  return true;
}

async function handleCallInput(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
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

function handleCallEvents(ctx: RequestContext): boolean {
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

async function handleCancelCall(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
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

function toCallApi(c: RemoteInvokeCall) {
  let commandSummary = { command_preview: '' };
  let commandObj: Record<string, unknown> = { command: '' };
  try { commandSummary = JSON.parse(c.command_summary_json); } catch { /* ignore */ }
  try { commandObj = JSON.parse(c.command_json); } catch { /* ignore */ }
  const startedAt = isoToUnixMs(c.started_at) ?? 0;
  const endedAt = isoToUnixMs(c.ended_at);
  return {
    call_id: c.id,
    grant_id: c.grant_id,
    pairing_id: c.pairing_id || null,
    client_instance_id: c.client_instance_id,
    caller_fingerprint: c.caller_fingerprint,
    status: c.status,
    command_summary: commandSummary,
    command: (commandObj.command as string) || '',
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

async function handleListCalls(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const query = {
    client_instance_id: ctx.url.searchParams.get('client_instance_id') ?? undefined,
    caller_fingerprint: ctx.url.searchParams.get('caller_fingerprint') ?? undefined,
    status: ctx.url.searchParams.get('status') ?? undefined,
    offset: parseInt(ctx.url.searchParams.get('offset') ?? '0', 10),
    limit: parseInt(ctx.url.searchParams.get('limit') ?? '100', 10),
  };

  const result = await service.listCalls('', query);
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: { list: result.list.map(toCallApi), total: result.total } });
  return true;
}

async function handleGetCall(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const parts = ctx.url.pathname.match(/\/v4\/remote-invoke\/calls\/([^/]+)$/);
  const callId = parts?.[1] ?? '';

  const call = await service.getCall('', callId);
  if (!call) {
    sendError(ctx.res, 404, 'call not found');
    return true;
  }

  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: toCallApi(call) });
  return true;
}

async function handleListClients(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const result = await service.getOnlineClients('');
  sendJson(ctx.res, 200, { code: 0, message: 'ok', data: result });
  return true;
}

async function handleOpenCall(ctx: RequestContext, service: RemoteInvokeService): Promise<boolean> {
  const body = parseJsonBody<any>(ctx.body);
  if (!body?.grant_id || !body?.client_instance_id || !body?.command) {
    sendError(ctx.res, 400, 'grant_id, client_instance_id, and command are required');
    return true;
  }

  try {
    const result = await service.openCall('', {
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
