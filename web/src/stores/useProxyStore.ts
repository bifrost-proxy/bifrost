import { create } from "zustand";
import type { CliProxyStatus, SystemProxyLaunchdStatus, SystemProxyStatus } from "../api/proxy";
import {
  getCliProxyStatus,
  getSystemProxyLaunchdStatus,
  getSystemProxyStatus,
  setSystemProxy,
  setSystemProxyLaunchd,
} from "../api/proxy";
import { isConnectionIssueError } from "../api/client";

interface ProxyState {
  systemProxy: SystemProxyStatus | null;
  systemProxyLaunchd: SystemProxyLaunchdStatus | null;
  cliProxy: CliProxyStatus | null;
  loading: boolean;
  launchdLoading: boolean;
  error: string | null;
  applySystemProxySnapshot: (status: SystemProxyStatus) => void;
  applyCliProxySnapshot: (status: CliProxyStatus) => void;
  fetchSystemProxy: () => Promise<void>;
  fetchSystemProxyLaunchd: () => Promise<void>;
  fetchCliProxy: () => Promise<void>;
  toggleSystemProxy: (enabled: boolean) => Promise<boolean>;
  toggleSystemProxyLaunchd: (enabled: boolean) => Promise<boolean>;
  clearError: () => void;
}

export const useProxyStore = create<ProxyState>((set, get) => ({
  systemProxy: null,
  systemProxyLaunchd: null,
  cliProxy: null,
  loading: false,
  launchdLoading: false,
  error: null,

  applySystemProxySnapshot: (status) => {
    set({ systemProxy: status, loading: false, error: null });
  },

  applyCliProxySnapshot: (status) => {
    set({ cliProxy: status, error: null });
  },

  fetchSystemProxy: async () => {
    try {
      const status = await getSystemProxyStatus();
      set({ systemProxy: status, error: null });
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message });
    }
  },

  fetchSystemProxyLaunchd: async () => {
    try {
      const status = await getSystemProxyLaunchdStatus();
      set({ systemProxyLaunchd: status, error: null });
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message });
    }
  },

  fetchCliProxy: async () => {
    try {
      const status = await getCliProxyStatus();
      set({ cliProxy: status, error: null });
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message });
    }
  },

  toggleSystemProxy: async (enabled: boolean) => {
    const currentState = get().systemProxy;
    set({ loading: true, error: null });
    try {
      const status = await setSystemProxy({ enabled });
      set({
        systemProxy: status,
        loading: false,
        error:
          doesSystemProxyMatchRequest(status, enabled)
            ? null
            : `System proxy is still ${status.enabled ? "enabled" : "disabled"}`,
      });
      return doesSystemProxyMatchRequest(status, enabled);
    } catch (e) {
      set({
        error: isConnectionIssueError(e) ? null : (e as Error).message,
        loading: false,
        systemProxy: currentState,
      });
      return false;
    }
  },

  toggleSystemProxyLaunchd: async (enabled: boolean) => {
    const currentState = get().systemProxyLaunchd;
    set({ launchdLoading: true, error: null });
    try {
      const status = await setSystemProxyLaunchd({ enabled });
      const matches = enabled
        ? status.installed && status.loaded && !status.needs_upgrade
        : !status.installed && !status.loaded;
      set({
        systemProxyLaunchd: status,
        launchdLoading: false,
        error: matches ? null : status.message || "System cleanup fallback did not reach the requested state",
      });
      return matches;
    } catch (e) {
      set({
        error: isConnectionIssueError(e) ? null : (e as Error).message,
        launchdLoading: false,
        systemProxyLaunchd: currentState,
      });
      return false;
    }
  },

  clearError: () => set({ error: null }),
}));

export function doesSystemProxyMatchRequest(
  status: SystemProxyStatus,
  enabled: boolean,
): boolean {
  if (enabled) {
    return isSystemProxyLiveEnabledByBifrost(status);
  }

  return !status.enabled || status.managed_by_bifrost === false;
}

export function isSystemProxyConfiguredEnabled(status: SystemProxyStatus): boolean {
  return status.configured_enabled ?? isSystemProxyLiveEnabledByBifrost(status);
}

export function isSystemProxyLiveEnabledByBifrost(status: SystemProxyStatus): boolean {
  return status.enabled && status.managed_by_bifrost !== false;
}
