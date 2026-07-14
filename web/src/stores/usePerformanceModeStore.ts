import { create } from "zustand";
import { getPerformanceConfig } from "../api/config";

interface PerformanceModeState {
  superPerformanceMode: boolean | null;
  fetchPerformanceMode: (force?: boolean) => Promise<boolean>;
  setSuperPerformanceMode: (enabled: boolean) => void;
}

let performanceModeRequest: Promise<boolean> | null = null;

export const usePerformanceModeStore = create<PerformanceModeState>((set, get) => ({
  superPerformanceMode: null,

  fetchPerformanceMode: async (force = false) => {
    const current = get().superPerformanceMode;
    if (!force && current !== null) {
      return current;
    }
    if (performanceModeRequest) {
      return performanceModeRequest;
    }

    performanceModeRequest = getPerformanceConfig()
      .then((config) => {
        const enabled = config.traffic.super_performance_mode;
        set({ superPerformanceMode: enabled });
        return enabled;
      })
      .catch(() => {
        const fallback = get().superPerformanceMode ?? false;
        set({ superPerformanceMode: fallback });
        return fallback;
      })
      .finally(() => {
        performanceModeRequest = null;
      });

    return performanceModeRequest;
  },

  setSuperPerformanceMode: (enabled) => {
    set({ superPerformanceMode: enabled });
  },
}));
