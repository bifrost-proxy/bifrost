import type { ServerResponse } from 'http';
import { writeSseEvent, writeSseComment, closeSse } from '../http';
import type { ClientStreamState } from './types';

const clientStreams = new Map<string, ClientStreamState>();
const callerStreams = new Map<string, { res: ServerResponse; callId: string }>();
const callerEventBuffers = new Map<string, Array<{ event: string; data: unknown; id?: string }>>();
type ReplaySafeClientEvent = {
  scopeKey: string;
  event: string;
  data: unknown;
  id?: string;
  bufferedAt: number;
  encodedBytes: number;
};
const replaySafeClientEventBuffers = new Map<string, ReplaySafeClientEvent[]>();
type WatcherEntry = { res: ServerResponse; pairingId: string; watchTokenHash?: string };
const pairingWatchers = new Map<string, Set<WatcherEntry>>();
const pairingEventBuffers = new Map<string, Array<{ event: string; data: unknown }>>();
const MAX_BUFFERED_CALLER_EVENTS = 256;
const MAX_REPLAY_SAFE_CLIENTS = 128;
const MAX_REPLAY_SAFE_CLIENT_EVENTS = 64;
const MAX_REPLAY_SAFE_CLIENT_BYTES = 512 * 1024;
const REPLAY_SAFE_CLIENT_EVENT_TTL_MS = 30_000;

let keepaliveTimer: ReturnType<typeof setInterval> | null = null;

export function startKeepalive(intervalMs: number) {
  if (keepaliveTimer) return;
  keepaliveTimer = setInterval(() => {
    const now = Date.now();
    for (const [key, state] of clientStreams) {
      try {
        writeSseEvent(state.res, 'ping', { ts: now });
      } catch {
        clientStreams.delete(key);
      }
    }
    for (const [key, watcher] of callerStreams) {
      try {
        writeSseComment(watcher.res, 'keepalive');
      } catch {
        callerStreams.delete(key);
      }
    }
    for (const [key, watchers] of pairingWatchers) {
      for (const watcher of [...watchers]) {
        try {
          writeSseComment(watcher.res, 'keepalive');
        } catch {
          watchers.delete(watcher);
        }
      }
      if (watchers.size === 0) {
        pairingWatchers.delete(key);
      }
    }
  }, intervalMs);
}

export function stopKeepalive() {
  if (keepaliveTimer) {
    clearInterval(keepaliveTimer);
    keepaliveTimer = null;
  }
}

export function registerClientStream(state: ClientStreamState): void {
  const existing = clientStreams.get(state.clientInstanceId);
  if (existing && existing.streamId !== state.streamId) {
    state.discoverable = existing.discoverable;
    state.discoverySessionId = existing.discoverySessionId;
    state.pairCode = existing.pairCode;
    state.pairCodeExpiresAt = existing.pairCodeExpiresAt;
    try {
      writeSseEvent(existing.res, 'replaced', { new_stream_id: state.streamId });
      closeSse(existing.res);
    } catch {}
  }
  clientStreams.set(state.clientInstanceId, state);
}

export function unregisterClientStream(clientInstanceId: string, streamId?: string): void {
  const existing = clientStreams.get(clientInstanceId);
  if (existing && (!streamId || existing.streamId === streamId)) {
    clientStreams.delete(clientInstanceId);
  }
}

export function getClientStream(clientInstanceId: string): ClientStreamState | undefined {
  return clientStreams.get(clientInstanceId);
}

export function getAllClientStreams(): Map<string, ClientStreamState> {
  return clientStreams;
}

export function pushToClient(clientInstanceId: string, event: string, data: unknown, id?: string): boolean {
  const state = clientStreams.get(clientInstanceId);
  if (!state) {
    return false;
  }
  try {
    console.debug('[pushToClient]', { event, clientInstanceId, streamId: state.streamId });
    writeSseEvent(state.res, event, data, id);
    return true;
  } catch (e) {
    clientStreams.delete(clientInstanceId);
    return false;
  }
}

function pruneReplaySafeClientEvents(clientInstanceId: string, now: number): ReplaySafeClientEvent[] {
  const retained = (replaySafeClientEventBuffers.get(clientInstanceId) ?? []).filter(
    (entry) => now - entry.bufferedAt <= REPLAY_SAFE_CLIENT_EVENT_TTL_MS,
  );
  if (retained.length > 0) {
    replaySafeClientEventBuffers.set(clientInstanceId, retained);
  } else {
    replaySafeClientEventBuffers.delete(clientInstanceId);
  }
  return retained;
}

function bufferReplaySafeClientEvent(
  clientInstanceId: string,
  scopeKey: string,
  event: string,
  data: unknown,
  allowEviction: boolean,
  id?: string,
): boolean {
  const now = Date.now();
  let encodedBytes: number;
  try {
    encodedBytes = Buffer.byteLength(JSON.stringify(data), 'utf8')
      + Buffer.byteLength(scopeKey, 'utf8')
      + Buffer.byteLength(event, 'utf8')
      + Buffer.byteLength(id ?? '', 'utf8');
  } catch {
    return false;
  }
  if (encodedBytes > MAX_REPLAY_SAFE_CLIENT_BYTES) {
    return false;
  }

  if (!replaySafeClientEventBuffers.has(clientInstanceId)
    && replaySafeClientEventBuffers.size >= MAX_REPLAY_SAFE_CLIENTS) {
    if (!allowEviction) return false;
    const oldestClient = replaySafeClientEventBuffers.keys().next().value;
    if (oldestClient) replaySafeClientEventBuffers.delete(oldestClient);
  }

  const buffered = pruneReplaySafeClientEvents(clientInstanceId, now);
  let bufferedBytes = buffered.reduce((total, entry) => total + entry.encodedBytes, 0);
  if (!allowEviction
    && (buffered.length >= MAX_REPLAY_SAFE_CLIENT_EVENTS
      || bufferedBytes + encodedBytes > MAX_REPLAY_SAFE_CLIENT_BYTES)) {
    return false;
  }
  while (buffered.length >= MAX_REPLAY_SAFE_CLIENT_EVENTS
    || bufferedBytes + encodedBytes > MAX_REPLAY_SAFE_CLIENT_BYTES) {
    const removed = buffered.shift();
    if (!removed) break;
    bufferedBytes -= removed.encodedBytes;
  }
  buffered.push({ scopeKey, event, data, id, bufferedAt: now, encodedBytes });
  replaySafeClientEventBuffers.set(clientInstanceId, buffered);
  return true;
}

/**
 * Send an event whose receiver can safely reject duplicate delivery, while
 * retaining it briefly for replay after a target SSE reconnect. Caller stdin
 * frames satisfy that contract because their encrypted sequence number is
 * checked by the target replay window.
 */
export function pushReplaySafeToClient(
  clientInstanceId: string,
  scopeKey: string,
  event: string,
  data: unknown,
  id?: string,
): boolean {
  const delivered = pushToClient(clientInstanceId, event, data, id);
  const buffered = bufferReplaySafeClientEvent(
    clientInstanceId,
    scopeKey,
    event,
    data,
    delivered,
    id,
  );
  return delivered || buffered;
}

export function flushReplaySafeClientEvents(clientInstanceId: string): boolean {
  const state = clientStreams.get(clientInstanceId);
  if (!state) return false;
  const buffered = pruneReplaySafeClientEvents(clientInstanceId, Date.now());
  try {
    for (const entry of buffered) {
      writeSseEvent(state.res, entry.event, entry.data, entry.id);
    }
    return true;
  } catch {
    clientStreams.delete(clientInstanceId);
    return false;
  }
}

export function clearReplaySafeClientEvents(scopeKey: string): void {
  for (const [clientInstanceId, buffered] of replaySafeClientEventBuffers) {
    const retained = buffered.filter((entry) => entry.scopeKey !== scopeKey);
    if (retained.length > 0) {
      replaySafeClientEventBuffers.set(clientInstanceId, retained);
    } else {
      replaySafeClientEventBuffers.delete(clientInstanceId);
    }
  }
}

export function registerCallerEventStream(callId: string, res: ServerResponse): void {
  callerStreams.set(callId, { res, callId });
}

export function unregisterCallerEventStream(callId: string): void {
  callerStreams.delete(callId);
}

export function flushCallerEventStream(callId: string): boolean {
  const entry = callerStreams.get(callId);
  if (!entry) return false;
  const buffered = callerEventBuffers.get(callId);
  if (!buffered?.length) return true;

  try {
    for (const event of buffered) {
      writeSseEvent(entry.res, event.event, event.data, event.id);
    }
    callerEventBuffers.delete(callId);
    return true;
  } catch {
    callerStreams.delete(callId);
    return false;
  }
}

export function clearCallerEventBuffer(callId: string): void {
  callerEventBuffers.delete(callId);
}

function bufferCallerEvent(callId: string, event: string, data: unknown, id?: string): void {
  const existing = callerEventBuffers.get(callId) ?? [];
  existing.push({ event, data, id });
  while (existing.length > MAX_BUFFERED_CALLER_EVENTS) {
    existing.shift();
  }
  callerEventBuffers.set(callId, existing);
}

export function pushToCallerStream(callId: string, event: string, data: unknown, id?: string): boolean {
  const entry = callerStreams.get(callId);
  if (!entry) {
    bufferCallerEvent(callId, event, data, id);
    return true;
  }
  try {
    writeSseEvent(entry.res, event, data, id);
    return true;
  } catch {
    callerStreams.delete(callId);
    bufferCallerEvent(callId, event, data, id);
    return false;
  }
}

export function registerPairingWatcher(pairingId: string, res: ServerResponse, watchTokenHash?: string): void {
  const entry: WatcherEntry = { res, pairingId, watchTokenHash };
  const watchers = pairingWatchers.get(pairingId) ?? new Set<WatcherEntry>();
  watchers.add(entry);
  pairingWatchers.set(pairingId, watchers);
  const buffered = pairingEventBuffers.get(pairingId);
  if (buffered?.length) {
    for (const ev of buffered) {
      try {
        writeSseEvent(res, ev.event, ev.data);
      } catch {
        watchers.delete(entry);
        break;
      }
    }
    if (watchers.size === 0) {
      pairingWatchers.delete(pairingId);
    }
  }
}

export function unregisterPairingWatcher(pairingId: string, res?: ServerResponse): void {
  const watchers = pairingWatchers.get(pairingId);
  if (!watchers) {
    return;
  }
  if (!res) {
    pairingWatchers.delete(pairingId);
    return;
  }
  for (const watcher of [...watchers]) {
    if (watcher.res === res) {
      watchers.delete(watcher);
    }
  }
  if (watchers.size === 0) {
    pairingWatchers.delete(pairingId);
  }
}

export function pushToPairingWatcher(pairingId: string, event: string, data: unknown): boolean {
  const watchers = pairingWatchers.get(pairingId);
  if (!watchers || watchers.size === 0) {
    const existing = pairingEventBuffers.get(pairingId) ?? [];
    existing.push({ event, data });
    pairingEventBuffers.set(pairingId, existing);
    return true;
  }
  let delivered = false;
  for (const entry of [...watchers]) {
    try {
      writeSseEvent(entry.res, event, data);
      delivered = true;
    } catch {
      watchers.delete(entry);
    }
  }
  if (watchers.size === 0) {
    pairingWatchers.delete(pairingId);
  }
  return delivered;
}

export function updateClientDiscovery(
  clientInstanceId: string,
  pairCode: string | undefined,
  expiresAt: number | undefined,
  discoverySessionId: string | undefined,
): boolean {
  const state = clientStreams.get(clientInstanceId);
  if (!state) return false;
  state.discoverable = !!pairCode;
  state.pairCode = pairCode;
  state.pairCodeExpiresAt = expiresAt;
  state.discoverySessionId = discoverySessionId;
  return true;
}

export function clearClientDiscovery(clientInstanceId: string, discoverySessionId?: string): boolean {
  const state = clientStreams.get(clientInstanceId);
  if (!state) {
    return false;
  }
  if (
    discoverySessionId
    && state.discoverySessionId
    && state.discoverySessionId !== discoverySessionId
  ) {
    return false;
  }
  state.discoverable = false;
  state.pairCode = undefined;
  state.pairCodeExpiresAt = undefined;
  state.discoverySessionId = undefined;
  return true;
}

export function consumeClientDiscovery(clientInstanceId: string): void {
  const state = clientStreams.get(clientInstanceId);
  if (state) {
    state.discoverable = false;
    state.pairCodeExpiresAt = undefined;
    state.discoverySessionId = undefined;
  }
}

export type FindPairCodeResult =
  | { found: true; client: ClientStreamState }
  | { found: false; reason: 'not_found' | 'expired' | 'consumed' };

export function findClientByPairCode(pairCode: string): FindPairCodeResult {
  for (const state of clientStreams.values()) {
    if (state.pairCode === pairCode) {
      if (!state.discoverable) {
        return { found: false, reason: 'consumed' };
      }
      const now = Date.now();
      if (state.pairCodeExpiresAt && state.pairCodeExpiresAt <= now) {
        return { found: false, reason: 'expired' };
      }
      return { found: true, client: state };
    }
  }
  return { found: false, reason: 'not_found' };
}

export function getOnlineClientCount(): number {
  return clientStreams.size;
}
