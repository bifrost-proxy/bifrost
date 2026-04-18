import type { ServerResponse } from 'http';
import { writeSseEvent, writeSseComment, closeSse } from '../http';
import type { ClientStreamState } from './types';

const clientStreams = new Map<string, ClientStreamState>();
const callerStreams = new Map<string, { res: ServerResponse; callId: string }>();
const pairingWatchers = new Map<string, { res: ServerResponse; pairingId: string }>();

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
  if (!state) return false;
  try {
    writeSseEvent(state.res, event, data, id);
    return true;
  } catch {
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

export function pushToCallerStream(callId: string, event: string, data: unknown, id?: string): boolean {
  const entry = callerStreams.get(callId);
  if (!entry) return false;
  try {
    writeSseEvent(entry.res, event, data, id);
    return true;
  } catch {
    callerStreams.delete(callId);
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
