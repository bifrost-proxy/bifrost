import { create } from "zustand";
import type { CliProxyStatus, SystemProxyStatus } from "../api/proxy";
import { getCliProxyStatus, getSystemProxyStatus, setSystemProxy } from "../api/proxy";
import { isConnectionIssueError } from "../api/client";

interface ProxyState {
  systemProxy: SystemProxyStatus | null;
  cliProxy: CliProxyStatus | null;
  loading: boolean;
  error: string | null;
  applySystemProxySnapshot: (status: SystemProxyStatus) => void;
  applyCliProxySnapshot: (status: CliProxyStatus) => void;
  fetchSystemProxy: () => Promise<void>;
  fetchCliProxy: () => Promise<void>;
  toggleSystemProxy: (enabled: boolean) => Promise<boolean>;
  clearError: () => void;
}

export const useProxyStore = create<ProxyState>((set, get) => ({
  systemProxy: null,
  cliProxy: null,
  loading: false,
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

  clearError: () => set({ error: null }),
}));

export function doesSystemProxyMatchRequest(
  status: SystemProxyStatus,
  enabled: boolean,
): boolean {
  if (enabled) {
    return status.enabled && status.managed_by_bifrost !== false;
  }

  return !status.enabled || status.managed_by_bifrost === false;
}
