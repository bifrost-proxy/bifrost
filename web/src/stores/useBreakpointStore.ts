import { create } from "zustand";
import type {
  BreakpointSettings,
  PendingBreakpoint,
} from "../api/breakpoint";
import {
  getBreakpointSettings,
  getPendingBreakpoints,
  updateBreakpointSettings,
  resumeBreakpoint,
} from "../api/breakpoint";
import { pushService } from "../services/pushService";
import type {
  BreakpointPausedPushData,
  BreakpointSettingsPushData,
  BreakpointResumedPushData,
} from "../services/pushService";
import { useTrafficStore } from "./useTrafficStore";

export interface PausedBreakpoint {
  requestId: string;
  phase: "request" | "response";
  method?: string;
  originalMethod?: string;
  url?: string;
  originalUrl?: string;
  status?: number;
  originalStatus?: number;
  headers: [string, string][];
  originalHeaders: [string, string][];
  body: string;
  originalBody: string;
  bodyOmitted: boolean;
  bodySize?: number;
  maxBodyBytes: number;
  contentEncoding?: string;
  pausedAtMs: number;
  deadlineAtMs: number;
  localDeadlineAtMs: number;
}

type BreakpointPhase = "request" | "response";

interface BreakpointState {
  enabled: boolean;
  maxBodyBytes: number;
  loading: boolean;
  pendingLoading: boolean;
  pausedRequests: Map<string, PausedBreakpoint>;
  pausedResponses: Map<string, PausedBreakpoint>;
  pendingRevision: number;
  pushInitialized: boolean;

  fetchSettings: () => Promise<void>;
  fetchPending: () => Promise<void>;
  toggleEnabled: (enabled: boolean) => Promise<void>;
  applySettings: (settings: BreakpointSettings) => void;
  updatePausedBody: (requestId: string, phase: BreakpointPhase, body: string) => void;
  updatePausedHeaders: (
    requestId: string,
    phase: BreakpointPhase,
    headers: [string, string][],
  ) => void;
  updatePausedMetadata: (
    requestId: string,
    phase: BreakpointPhase,
    patch: Partial<Pick<PausedBreakpoint, "method" | "url" | "status">>,
  ) => void;
  removePaused: (requestId: string, phase?: BreakpointPhase) => void;
  resume: (
    requestId: string,
    phase: BreakpointPhase,
    applyEdits: boolean,
  ) => Promise<boolean>;
  connectPush: () => void;
}

type SnapshotLike = PendingBreakpoint | BreakpointPausedPushData;

const fromSnapshot = (data: SnapshotLike): PausedBreakpoint => {
  const headers = data.headers.map(([name, value]) => [name, value] as [string, string]);
  const body = data.body ?? "";
  return {
    requestId: data.request_id,
    phase: data.phase,
    method: data.method,
    originalMethod: data.method,
    url: data.url,
    originalUrl: data.url,
    status: data.status,
    originalStatus: data.status,
    headers,
    originalHeaders: headers.map(([name, value]) => [name, value]),
    body,
    originalBody: body,
    bodyOmitted: !!data.body_omitted,
    bodySize: data.body_size,
    maxBodyBytes: data.max_body_bytes ?? 1024 * 1024,
    contentEncoding: data.content_encoding,
    pausedAtMs: data.paused_at_ms,
    deadlineAtMs: data.deadline_at_ms,
    localDeadlineAtMs:
      Date.now() + Math.max(0, data.deadline_at_ms - data.server_now_ms),
  };
};

const mapsFromSnapshots = (items: SnapshotLike[]) => {
  const pausedRequests = new Map<string, PausedBreakpoint>();
  const pausedResponses = new Map<string, PausedBreakpoint>();
  for (const item of items) {
    const paused = fromSnapshot(item);
    (paused.phase === "request" ? pausedRequests : pausedResponses).set(
      paused.requestId,
      paused,
    );
  }
  return { pausedRequests, pausedResponses };
};

const applyPausedToTrafficDetail = (paused: PausedBreakpoint) => {
  useTrafficStore.setState((state) => {
    if (state.currentRecord?.id !== paused.requestId) return {};
    if (paused.phase === "request") {
      return {
        currentRecord: {
          ...state.currentRecord,
          method: paused.method ?? state.currentRecord.method,
          url: paused.url ?? state.currentRecord.url,
          request_headers: paused.headers,
        },
        requestBody: paused.bodyOmitted ? state.requestBody : paused.body,
      };
    }
    return {
      currentRecord: {
        ...state.currentRecord,
        status: paused.status ?? state.currentRecord.status,
        response_headers: paused.headers,
        original_response_headers:
          state.currentRecord.original_response_headers ?? paused.originalHeaders,
      },
      responseBody: paused.bodyOmitted ? state.responseBody : paused.body,
    };
  });
};

const scheduleTrafficRefetch = (requestId: string, delay = 500, retries = 4) => {
  setTimeout(() => {
    const state = useTrafficStore.getState();
    if (state.currentRecord?.id !== requestId) return;
    void state.fetchTrafficDetail(requestId);
    if (retries > 1) scheduleTrafficRefetch(requestId, delay * 2, retries - 1);
  }, delay);
};

const updateMapItem = (
  get: () => BreakpointState,
  set: (patch: Partial<BreakpointState>) => void,
  requestId: string,
  phase: BreakpointPhase,
  updater: (current: PausedBreakpoint) => PausedBreakpoint,
) => {
  const key = phase === "request" ? "pausedRequests" : "pausedResponses";
  const next = new Map(get()[key]);
  const current = next.get(requestId);
  if (!current) return;
  next.set(requestId, updater(current));
  set({ [key]: next } as Partial<BreakpointState>);
};

export const useBreakpointStore = create<BreakpointState>((set, get) => ({
  enabled: false,
  maxBodyBytes: 1024 * 1024,
  loading: false,
  pendingLoading: false,
  pausedRequests: new Map(),
  pausedResponses: new Map(),
  pendingRevision: 0,
  pushInitialized: false,

  fetchSettings: async () => {
    try {
      const settings = await getBreakpointSettings();
      set({
        enabled: settings.enabled,
        maxBodyBytes: settings.max_body_bytes,
        loading: false,
      });
    } catch {
      set({ loading: false });
    }
  },

  fetchPending: async () => {
    const revision = get().pendingRevision;
    set({ pendingLoading: true });
    try {
      const pending = await getPendingBreakpoints();
      if (get().pendingRevision !== revision) {
        set({ pendingLoading: false });
        queueMicrotask(() => void get().fetchPending());
        return;
      }
      set({ ...mapsFromSnapshots(pending), pendingLoading: false });
      for (const item of pending) applyPausedToTrafficDetail(fromSnapshot(item));
    } catch {
      set({ pendingLoading: false });
    }
  },

  toggleEnabled: async (enabled) => {
    set({ loading: true });
    try {
      const settings = await updateBreakpointSettings({
        enabled,
        max_body_bytes: get().maxBodyBytes,
      });
      set({
        enabled: settings.enabled,
        maxBodyBytes: settings.max_body_bytes,
        loading: false,
        ...(settings.enabled
          ? {}
          : {
              pausedRequests: new Map(),
              pausedResponses: new Map(),
              pendingRevision: get().pendingRevision + 1,
            }),
      });
    } catch (error) {
      set({ loading: false });
      throw error;
    }
  },

  applySettings: (settings) => {
    set({
      enabled: settings.enabled,
      maxBodyBytes: settings.max_body_bytes,
      ...(settings.enabled
        ? {}
        : {
            pausedRequests: new Map(),
            pausedResponses: new Map(),
            pendingRevision: get().pendingRevision + 1,
          }),
    });
  },

  updatePausedBody: (requestId, phase, body) => {
    updateMapItem(get, set, requestId, phase, (current) =>
      current.bodyOmitted ? current : { ...current, body },
    );
  },

  updatePausedHeaders: (requestId, phase, headers) => {
    updateMapItem(get, set, requestId, phase, (current) => ({
      ...current,
      headers,
    }));
  },

  updatePausedMetadata: (requestId, phase, patch) => {
    updateMapItem(get, set, requestId, phase, (current) => ({
      ...current,
      ...patch,
    }));
  },

  removePaused: (requestId, phase) => {
    const pendingRevision = get().pendingRevision + 1;
    if (!phase || phase === "request") {
      const requests = new Map(get().pausedRequests);
      requests.delete(requestId);
      set({ pausedRequests: requests, pendingRevision });
    }
    if (!phase || phase === "response") {
      const responses = new Map(get().pausedResponses);
      responses.delete(requestId);
      set({ pausedResponses: responses, pendingRevision });
    }
  },

  resume: async (requestId, phase, applyEdits) => {
    const paused =
      phase === "request"
        ? get().pausedRequests.get(requestId)
        : get().pausedResponses.get(requestId);
    if (!paused) return false;
    try {
      const result = await resumeBreakpoint({
        request_id: requestId,
        phase,
        ...(applyEdits
          ? {
              method: phase === "request" ? paused.method : undefined,
              url: phase === "request" ? paused.url : undefined,
              status: phase === "response" ? paused.status : undefined,
              headers: paused.headers,
              body: paused.bodyOmitted ? undefined : paused.body,
            }
          : {}),
      });
      if (!result.resumed) return false;
      if (applyEdits) applyPausedToTrafficDetail(paused);
      get().removePaused(requestId, phase);
      scheduleTrafficRefetch(requestId);
      return true;
    } catch {
      await get().fetchPending();
      return false;
    }
  },

  connectPush: () => {
    if (get().pushInitialized) return;
    set({ pushInitialized: true });

    pushService.onBreakpointPaused((data) => {
      const paused = fromSnapshot(data);
      const key = paused.phase === "request" ? "pausedRequests" : "pausedResponses";
      const next = new Map(get()[key]);
      next.set(paused.requestId, paused);
      set({
        [key]: next,
        pendingRevision: get().pendingRevision + 1,
      } as Partial<BreakpointState>);
      applyPausedToTrafficDetail(paused);
      void useTrafficStore.getState().reloadRecords();
    });

    pushService.onBreakpointSettingsUpdated((data: BreakpointSettingsPushData) => {
      get().applySettings({
        enabled: data.enabled,
        max_body_bytes: data.max_body_bytes,
      });
    });

    pushService.onBreakpointResumed((data: BreakpointResumedPushData) => {
      get().removePaused(data.request_id, data.phase);
      scheduleTrafficRefetch(data.request_id);
    });

    pushService.onConnectionChange(({ connected }) => {
      if (connected) {
        void get().fetchSettings();
        void get().fetchPending();
      }
    });

    void get().fetchPending();
  },
}));
