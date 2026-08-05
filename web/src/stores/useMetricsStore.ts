import { create } from 'zustand';
import type { MetricsSnapshot, SystemOverview } from '../types';
import * as api from '../api';
import { isConnectionIssueError } from '../api/client';
import pushService, {
  type OverviewData,
  type MetricsData,
  type HistoryData,
  METRICS_INTERVAL_DEFAULT_MS,
} from '../services/pushService';

interface MetricsState {
  current: MetricsSnapshot | null;
  history: MetricsSnapshot[];
  overview: SystemOverview | null;
  loading: boolean;
  error: string | null;
  usePush: boolean;
  pushRefCount: number;
  overviewUnsubscribe: (() => void) | null;
  metricsUnsubscribe: (() => void) | null;
  historyUnsubscribe: (() => void) | null;
  fetchMetrics: () => Promise<void>;
  fetchHistory: (limit?: number) => Promise<void>;
  fetchOverview: () => Promise<void>;
  clearError: () => void;
  enablePush: (options?: { needOverview?: boolean; needMetrics?: boolean; needHistory?: boolean; historyLimit?: number; metricsIntervalMs?: number }) => void;
  disablePush: () => void;
  handleOverviewPush: (data: OverviewData) => void;
  handleMetricsPush: (data: MetricsData) => void;
  handleHistoryPush: (data: HistoryData) => void;
}

export const useMetricsStore = create<MetricsState>((set, get) => ({
  current: null,
  history: [],
  overview: null,
  loading: false,
  error: null,
  usePush: true,
  pushRefCount: 0,
  overviewUnsubscribe: null,
  metricsUnsubscribe: null,
  historyUnsubscribe: null,

  fetchMetrics: async () => {
    try {
      const metrics = await api.getMetrics();
      set({ current: metrics });
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message });
    }
  },

  fetchHistory: async (limit?: number) => {
    set({ loading: true, error: null });
    try {
      const history = await api.getMetricsHistory(limit);
      set({ history, loading: false });
    } catch (e) {
      set({
        error: isConnectionIssueError(e) ? null : (e as Error).message,
        loading: false,
      });
    }
  },

  fetchOverview: async () => {
    set({ loading: true, error: null });
    try {
      const overview = await api.getSystemOverview();
      set({ overview, current: overview.metrics, loading: false });
    } catch (e) {
      set({
        error: isConnectionIssueError(e) ? null : (e as Error).message,
        loading: false,
      });
    }
  },

  clearError: () => set({ error: null }),

  enablePush: (options = {}) => {
    const state = get();

    const newRefCount = state.pushRefCount + 1;
    set({ pushRefCount: newRefCount });

    const {
      needOverview = true,
      needMetrics = true,
      needHistory = false,
      historyLimit = 3600,
      metricsIntervalMs = METRICS_INTERVAL_DEFAULT_MS,
    } = options;

    const subscription = {
      need_overview: needOverview,
      need_metrics: needMetrics,
      need_history: needHistory,
      history_limit: historyLimit,
      metrics_interval_ms: metricsIntervalMs,
    };

    if (newRefCount === 1) {
      pushService.connect(subscription);
    } else {
      pushService.updateSubscription(subscription);
    }

    if (needOverview && !state.overviewUnsubscribe) {
      const unsub = pushService.onOverviewUpdate((data) => {
        get().handleOverviewPush(data);
      });
      set({ overviewUnsubscribe: unsub });
    }

    if (needMetrics && !state.metricsUnsubscribe) {
      const unsub = pushService.onMetricsUpdate((data) => {
        get().handleMetricsPush(data);
      });
      set({ metricsUnsubscribe: unsub });
    }

    if (needHistory && !state.historyUnsubscribe) {
      const unsub = pushService.onHistoryUpdate((data) => {
        get().handleHistoryPush(data);
      });
      set({ historyUnsubscribe: unsub });
    }
  },

  disablePush: () => {
    const state = get();
    const newRefCount = Math.max(0, state.pushRefCount - 1);
    set({ pushRefCount: newRefCount });

    if (newRefCount > 0) {
      return;
    }

    if (state.overviewUnsubscribe) {
      state.overviewUnsubscribe();
      set({ overviewUnsubscribe: null });
    }
    if (state.metricsUnsubscribe) {
      state.metricsUnsubscribe();
      set({ metricsUnsubscribe: null });
    }
    if (state.historyUnsubscribe) {
      state.historyUnsubscribe();
      set({ historyUnsubscribe: null });
    }
    pushService.disconnectIfIdle();
  },

  handleOverviewPush: (data: OverviewData) => {
    const overview: SystemOverview = {
      system: data.system,
      metrics: data.metrics,
      rules: data.rules,
      traffic: data.traffic,
      server: data.server,
      pending_authorizations: data.pending_authorizations,
    };
    set((state) => ({
      overview,
      current: state.metricsUnsubscribe ? state.current : data.metrics,
    }));
  },

  handleMetricsPush: (data: MetricsData) => {
    set((state) => {
      const overview = state.overview
        ? {
            ...state.overview,
            metrics: data.metrics,
            traffic: {
              ...state.overview.traffic,
              recorded: data.recorded_traffic,
            },
          }
        : null;
      if (state.history.length === 0) {
        return { current: data.metrics, overview };
      }

      const last = state.history[state.history.length - 1];
      if (last?.timestamp === data.metrics.timestamp) {
        return {
          current: data.metrics,
          overview,
          history: [...state.history.slice(0, -1), data.metrics],
        };
      }

      return {
        current: data.metrics,
        overview,
        history: [...state.history.slice(1), data.metrics],
      };
    });
  },

  handleHistoryPush: (data: HistoryData) => {
    set({ history: data.history });
  },
}));
