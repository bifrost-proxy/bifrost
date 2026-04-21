import type { ServerResponse } from 'http';
import { writeSseEvent, writeSseComment, closeSse } from '../http';
import type { ClientStreamState } from './types';

const clientStreams = new Map<string, ClientStreamState>();
const callerStreams = new Map<string, { res: ServerResponse; callId: string }>();
const callerEventBuffers = new Map<string, Array<{ event: string; data: unknown; id?: string }>>();
const pairingWatchers = new Map<string, { res: ServerResponse; pairingId: string }>();
const MAX_BUFFERED_CALLER_EVENTS = 256;

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
    for (const [key, watcher] of pairingWatchers) {
      try {
        writeSseComment(watcher.res, 'keepalive');
      } catch {
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
    console.log(`[pushToClient] NO stream for ${clientInstanceId}, event=${event}, streams=[${[...clientStreams.keys()].join(',')}]`);
    return false;
  }
  try {
    console.log(`[pushToClient] writing event=${event} to client=${clientInstanceId}, streamId=${state.streamId}`);
    writeSseEvent(state.res, event, data, id);
    console.log(`[pushToClient] SUCCESS event=${event} to client=${clientInstanceId}`);
    return true;
  } catch (e) {
    console.log(`[pushToClient] FAILED event=${event} to client=${clientInstanceId}: ${e}`);
    clientStreams.delete(clientInstanceId);
    return false;
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

export function registerPairingWatcher(pairingId: string, res: ServerResponse): void {
  pairingWatchers.set(pairingId, { res, pairingId });
}

export function unregisterPairingWatcher(pairingId: string): void {
  pairingWatchers.delete(pairingId);
}

export function pushToPairingWatcher(pairingId: string, event: string, data: unknown): boolean {
  const entry = pairingWatchers.get(pairingId);
  if (!entry) return false;
  try {
    writeSseEvent(entry.res, event, data);
    return true;
  } catch {
    pairingWatchers.delete(pairingId);
    return false;
  }
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

export function clearClientDiscovery(clientInstanceId: string): void {
  const state = clientStreams.get(clientInstanceId);
  if (state) {
    state.discoverable = false;
    state.pairCode = undefined;
    state.pairCodeExpiresAt = undefined;
    state.discoverySessionId = undefined;
  }
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
