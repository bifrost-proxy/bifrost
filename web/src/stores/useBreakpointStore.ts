import { create } from "zustand";
import type { BreakpointSettings } from "../api/breakpoint";
import {
  getBreakpointSettings,
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

interface PausedBreakpoint {
  requestId: string;
  method?: string;
  url?: string;
  status?: number;
  headers: [string, string][];
  body: string | null;
  originalBody: string | null;
}

interface BreakpointState {
  enabled: boolean;
  hookRequest: boolean;
  hookResponse: boolean;
  loading: boolean;
  pausedRequests: Map<string, PausedBreakpoint>;
  pausedResponses: Map<string, PausedBreakpoint>;
  pushInitialized: boolean;

  fetchSettings: () => Promise<void>;
  toggleEnabled: (enabled: boolean) => Promise<void>;
  toggleHookRequest: (hook: boolean) => Promise<void>;
  toggleHookResponse: (hook: boolean) => Promise<void>;
  applySettings: (settings: BreakpointSettings) => void;

  addPausedRequest: (data: PausedBreakpoint) => void;
  addPausedResponse: (data: PausedBreakpoint) => void;
  updatePausedBody: (
    requestId: string,
    phase: "request" | "response",
    body: string,
  ) => void;
  removePaused: (requestId: string) => void;
  resume: (
    requestId: string,
    phase: string,
    headers: [string, string][],
    body?: string,
  ) => Promise<boolean>;
  connectPush: () => void;
}

const applyResumeToTrafficDetail = (
  requestId: string,
  phase: string,
  headers: [string, string][],
  body?: string | null,
) => {
  useTrafficStore.setState((state) => {
    if (state.currentRecord?.id !== requestId) {
      return {};
    }

    const fallbackBody =
      body ??
      (phase === "request" ? state.requestBody : state.responseBody) ??
      null;

    if (phase === "request") {
      return {
        currentRecord: {
          ...state.currentRecord,
          request_headers: headers,
        },
        requestBody: fallbackBody,
      };
    }

    return {
      currentRecord: {
        ...state.currentRecord,
        response_headers: headers,
        original_response_headers:
          state.currentRecord.original_response_headers ?? headers,
      },
      responseBody: fallbackBody,
    };
  });
};

export const useBreakpointStore = create<BreakpointState>((set, get) => ({
  enabled: false,
  hookRequest: false,
  hookResponse: false,
  loading: false,
  pausedRequests: new Map(),
  pausedResponses: new Map(),
  pushInitialized: false,

  fetchSettings: async () => {
    try {
      const settings = await getBreakpointSettings();
      set({
        enabled: settings.enabled,
        hookRequest: settings.hook_request,
        hookResponse: settings.hook_response,
        loading: false,
      });
    } catch {
      set({ loading: false });
    }
  },

  toggleEnabled: async (enabled: boolean) => {
    const state = get();
    set({ loading: true });
    try {
      const settings = await updateBreakpointSettings({
        enabled,
        hook_request: enabled || state.hookRequest,
        hook_response: enabled || state.hookResponse,
      });
      set({
        enabled: settings.enabled,
        hookRequest: settings.hook_request,
        hookResponse: settings.hook_response,
        loading: false,
      });
      if (!settings.enabled) {
        set({ pausedRequests: new Map(), pausedResponses: new Map() });
      }
    } catch {
      set({ loading: false });
    }
  },

  toggleHookRequest: async (hook: boolean) => {
    const state = get();
    set({ loading: true });
    try {
      const settings = await updateBreakpointSettings({
        enabled: state.enabled,
        hook_request: hook,
        hook_response: state.hookResponse,
      });
      set({
        hookRequest: settings.hook_request,
        loading: false,
      });
    } catch {
      set({ loading: false });
    }
  },

  toggleHookResponse: async (hook: boolean) => {
    const state = get();
    set({ loading: true });
    try {
      const settings = await updateBreakpointSettings({
        enabled: state.enabled,
        hook_request: state.hookRequest,
        hook_response: hook,
      });
      set({
        hookResponse: settings.hook_response,
        loading: false,
      });
    } catch {
      set({ loading: false });
    }
  },

  applySettings: (settings: BreakpointSettings) => {
    set({
      enabled: settings.enabled,
      hookRequest: settings.hook_request,
      hookResponse: settings.hook_response,
    });
    if (!settings.enabled) {
      set({ pausedRequests: new Map(), pausedResponses: new Map() });
    }
  },

  addPausedRequest: (data: PausedBreakpoint) => {
    const next = new Map(get().pausedRequests);
    next.set(data.requestId, data);
    set({ pausedRequests: next });
  },

  addPausedResponse: (data: PausedBreakpoint) => {
    const next = new Map(get().pausedResponses);
    next.set(data.requestId, data);
    set({ pausedResponses: next });
  },

  updatePausedBody: (requestId, phase, body) => {
    if (phase === "request") {
      const next = new Map(get().pausedRequests);
      const current = next.get(requestId);
      if (!current) return;
      next.set(requestId, {
        ...current,
        body: body === (current.originalBody ?? "") ? null : body,
      });
      set({ pausedRequests: next });
      return;
    }

    const next = new Map(get().pausedResponses);
    const current = next.get(requestId);
    if (!current) return;
    next.set(requestId, {
      ...current,
      body: body === (current.originalBody ?? "") ? null : body,
    });
    set({ pausedResponses: next });
  },

  removePaused: (requestId: string) => {
    const reqs = new Map(get().pausedRequests);
    reqs.delete(requestId);
    const resps = new Map(get().pausedResponses);
    resps.delete(requestId);
    set({ pausedRequests: reqs, pausedResponses: resps });
  },

  resume: async (
    requestId: string,
    phase: string,
    headers: [string, string][],
    body?: string,
  ) => {
    const pausedData =
      phase === "request"
        ? get().pausedRequests.get(requestId)
        : get().pausedResponses.get(requestId);
    try {
      const bodyToSend =
        body ?? (pausedData?.body == null ? pausedData?.originalBody : pausedData.body) ?? undefined;
      const result = await resumeBreakpoint({
        request_id: requestId,
        phase,
        headers,
        body: bodyToSend,
      });
      if (result.resumed) {
        (globalThis as { __bifrostLogNextHljsHtml?: boolean }).__bifrostLogNextHljsHtml = true;
        applyResumeToTrafficDetail(
          requestId,
          phase,
          headers,
          bodyToSend ?? pausedData?.originalBody ?? null,
        );
        get().removePaused(requestId);
        return true;
      }
      return false;
    } catch {
      return false;
    }
  },

  connectPush: () => {
    if (get().pushInitialized) return;
    set({ pushInitialized: true });

    const pendingRefetches = new Set<string>();

    pushService.onBreakpointPaused((data: BreakpointPausedPushData) => {
      const detailState = useTrafficStore.getState();
      const fallbackBody =
        data.body ??
        (data.phase === "request"
          ? detailState.currentRecord?.id === data.request_id
            ? detailState.requestBody
            : null
          : detailState.currentRecord?.id === data.request_id
            ? detailState.responseBody
            : null) ??
        null;
      const paused: PausedBreakpoint = {
        requestId: data.request_id,
        method: data.method,
        url: data.url,
        status: data.status,
        headers: data.headers,
        body: null,
        originalBody: fallbackBody,
      };
      if (data.phase === "request") {
        get().addPausedRequest(paused);
      } else if (data.phase === "response") {
        get().addPausedResponse(paused);
      }

      useTrafficStore.setState((state) => {
        if (state.currentRecord?.id !== data.request_id) {
          return {};
        }

        if (data.phase === "request") {
          return {
            currentRecord: {
              ...state.currentRecord,
              request_headers: data.headers,
            },
            requestBody: fallbackBody ?? state.requestBody,
          };
        }

        return {
          currentRecord: {
            ...state.currentRecord,
            status: data.status ?? state.currentRecord.status,
            response_headers: data.headers,
            original_response_headers:
              state.currentRecord.original_response_headers ?? data.headers,
          },
          responseBody: fallbackBody ?? state.responseBody,
        };
      });
    });

    pushService.onBreakpointSettingsUpdated((data: BreakpointSettingsPushData) => {
      get().applySettings({
        enabled: data.enabled,
        hook_request: data.hook_request,
        hook_response: data.hook_response,
      });
    });

    pushService.onBreakpointResumed((data: BreakpointResumedPushData) => {
      get().removePaused(data.request_id);
      pendingRefetches.add(data.request_id);
      setTimeout(() => {
        if (pendingRefetches.has(data.request_id)) {
          pendingRefetches.delete(data.request_id);
          useTrafficStore.getState().fetchTrafficDetail(data.request_id);
        }
      }, 3000);
    });

    pushService.onTrafficUpdates((update) => {
      if (pendingRefetches.size === 0) return;
      const all = [
        ...(update.new_records || []),
        ...(update.updated_records || []),
      ];
      for (const id of pendingRefetches) {
        if (all.some((r) => r.id === id)) {
          pendingRefetches.delete(id);
          useTrafficStore.getState().fetchTrafficDetail(id);
        }
      }
    });

    pushService.onTrafficDelta((delta) => {
      if (pendingRefetches.size === 0) return;
      const all = [
        ...(delta.inserts || []),
        ...(delta.updates || []),
      ];
      for (const id of pendingRefetches) {
        if (all.some((r) => r.id === id)) {
          pendingRefetches.delete(id);
          useTrafficStore.getState().fetchTrafficDetail(id);
        }
      }
    });
  },
}));
