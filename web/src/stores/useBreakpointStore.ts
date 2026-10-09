import { create } from "zustand";
import type { BreakpointSettings, PendingBreakpoint } from "../api/breakpoint";
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
import { useFilterPanelStore } from "./useFilterPanelStore";
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
  bodyEncoding: "utf8" | "base64";
  bodyRepresentation: "decoded" | "raw";
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
  settingsRevision: number;
  pushInitialized: boolean;
  autoSelected: boolean;
  pendingOnly: boolean;
  resumeError: string | null;
  setPendingOnly: (value: boolean) => void;
  updateBodyEncoding: (
    requestId: string,
    phase: BreakpointPhase,
    encoding: "utf8" | "base64",
    body: string,
  ) => void;

  fetchSettings: () => Promise<void>;
  fetchPending: () => Promise<void>;
  toggleEnabled: (enabled: boolean) => Promise<void>;
  applySettings: (settings: BreakpointSettings) => void;
  updatePausedBody: (
    requestId: string,
    phase: BreakpointPhase,
    body: string,
  ) => void;
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
  const headers = data.headers.map(
    ([name, value]) => [name, value] as [string, string],
  );
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
    bodyEncoding: data.body_encoding ?? "utf8",
    bodyRepresentation: data.body_representation ?? "decoded",
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

const mergeSnapshot = (data: SnapshotLike, previous: BreakpointState) => {
  const incoming = fromSnapshot(data);
  const existing = (
    incoming.phase === "request"
      ? previous.pausedRequests
      : previous.pausedResponses
  ).get(incoming.requestId);
  return existing?.pausedAtMs === incoming.pausedAtMs
    ? {
        ...existing,
        deadlineAtMs: incoming.deadlineAtMs,
        localDeadlineAtMs: incoming.localDeadlineAtMs,
      }
    : incoming;
};

const selectFirstPaused = (paused: PausedBreakpoint) => {
  useFilterPanelStore.getState().setDetailPanelCollapsed(false);
  useTrafficStore.getState().setSelectedId(paused.requestId);
};

const mapsFromSnapshots = (
  items: SnapshotLike[],
  previous: BreakpointState,
) => {
  const pausedRequests = new Map<string, PausedBreakpoint>();
  const pausedResponses = new Map<string, PausedBreakpoint>();
  for (const item of items) {
    const paused = mergeSnapshot(item, previous);
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
          state.currentRecord.original_response_headers ??
          paused.originalHeaders,
      },
      responseBody: paused.bodyOmitted ? state.responseBody : paused.body,
    };
  });
};

const scheduleTrafficRefetch = (
  requestId: string,
  delay = 500,
  retries = 4,
) => {
  setTimeout(() => {
    const state = useTrafficStore.getState();
    if (state.selectedId !== requestId || state.currentRecord?.id !== requestId)
      return;
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
  settingsRevision: 0,
  pushInitialized: false,
  autoSelected: false,
  pendingOnly: false,
  resumeError: null,
  setPendingOnly: (pendingOnly) => set({ pendingOnly }),
  updateBodyEncoding: (requestId, phase, bodyEncoding, body) => {
    updateMapItem(get, set, requestId, phase, (current) => ({
      ...current,
      bodyEncoding,
      body,
    }));
  },

  fetchSettings: async () => {
    if (get().loading) return;
    const revision = get().settingsRevision;
    try {
      const settings = await getBreakpointSettings();
      if (revision !== get().settingsRevision) return;
      get().applySettings(settings);
      set({ loading: false });
    } catch {
      if (revision === get().settingsRevision) set({ loading: false });
    }
  },

  fetchPending: async () => {
    if (get().loading) return;
    const revision = get().pendingRevision;
    set({ pendingLoading: true });
    try {
      const pending = await getPendingBreakpoints();
      if (get().pendingRevision !== revision) {
        set({ pendingLoading: false });
        queueMicrotask(() => void get().fetchPending());
        return;
      }
      const maps = mapsFromSnapshots(pending, get());
      set({ ...maps, pendingLoading: false });
      const first =
        maps.pausedRequests.values().next().value ??
        maps.pausedResponses.values().next().value;
      if (first && !get().autoSelected) {
        set({ autoSelected: true });
        selectFirstPaused(first);
      }
      for (const paused of [
        ...maps.pausedRequests.values(),
        ...maps.pausedResponses.values(),
      ])
        applyPausedToTrafficDetail(paused);
    } catch {
      set({ pendingLoading: false });
    }
  },

  toggleEnabled: async (enabled) => {
    const revision = get().settingsRevision + 1;
    set({ loading: true, settingsRevision: revision });
    try {
      const settings = await updateBreakpointSettings({
        enabled,
        max_body_bytes: get().maxBodyBytes,
      });
      if (revision !== get().settingsRevision) return;
      get().applySettings(settings);
      if (settings.enabled) void get().fetchPending();
    } catch (error) {
      if (revision === get().settingsRevision) set({ loading: false });
      throw error;
    }
  },

  applySettings: (settings) => {
    set({
      settingsRevision: get().settingsRevision + 1,
      loading: false,
      enabled: settings.enabled,
      autoSelected:
        settings.enabled === get().enabled ? get().autoSelected : false,
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
    set({ resumeError: null });
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
              body_encoding: paused.bodyOmitted
                ? undefined
                : paused.bodyEncoding,
              body_representation: paused.bodyOmitted
                ? undefined
                : paused.bodyRepresentation,
            }
          : {}),
      });
      if (!result.resumed) return false;
      if (applyEdits) applyPausedToTrafficDetail(paused);
      get().removePaused(requestId, phase);
      scheduleTrafficRefetch(requestId);
      return true;
    } catch (error) {
      const failure = error as {
        response?: { data?: { error?: string } };
        message?: string;
      };
      set({
        resumeError:
          failure.response?.data?.error ??
          failure.message ??
          "Unable to resume breakpoint",
      });
      await get().fetchPending();
      return false;
    }
  },

  connectPush: () => {
    if (get().pushInitialized) return;
    set({ pushInitialized: true });

    pushService.onBreakpointPaused((data) => {
      if (!get().enabled) return;
      const paused = mergeSnapshot(data, get());
      const key =
        paused.phase === "request" ? "pausedRequests" : "pausedResponses";
      const next = new Map(get()[key]);
      next.set(paused.requestId, paused);
      set({
        [key]: next,
        pendingRevision: get().pendingRevision + 1,
      } as Partial<BreakpointState>);
      if (!get().autoSelected) {
        set({ autoSelected: true });
        selectFirstPaused(paused);
      }
      applyPausedToTrafficDetail(paused);
      void useTrafficStore.getState().reloadRecords();
    });

    pushService.onBreakpointSettingsUpdated(
      (data: BreakpointSettingsPushData) => {
        get().applySettings({
          enabled: data.enabled,
          max_body_bytes: data.max_body_bytes,
        });
      },
    );

    pushService.onBreakpointResumed((data: BreakpointResumedPushData) => {
      get().removePaused(data.request_id, data.phase);
      scheduleTrafficRefetch(data.request_id);
    });

    pushService.onConnectionChange(({ connected }) => {
      if (connected) {
        void get()
          .fetchSettings()
          .then(() => get().fetchPending());
      }
    });

    void get()
      .fetchSettings()
      .then(() => get().fetchPending());
  },
}));
