import { create } from 'zustand';
import type { WhitelistStatus, AccessMode, UserPassAccountUpdate } from '../types';
import * as api from '../api';
import { isConnectionIssueError } from '../api/client';

interface WhitelistState {
  status: WhitelistStatus | null;
  loading: boolean;
  error: string | null;
  applyStatusSnapshot: (status: WhitelistStatus) => void;
  fetchStatus: () => Promise<void>;
  addToWhitelist: (ipOrCidr: string) => Promise<boolean>;
  removeFromWhitelist: (ipOrCidr: string) => Promise<boolean>;
  setMode: (mode: AccessMode) => Promise<boolean>;
  setAllowLan: (allow: boolean) => Promise<boolean>;
  setUserPassConfig: (enabled: boolean, accounts: UserPassAccountUpdate[], loopback_requires_auth: boolean) => Promise<boolean>;
  addTemporary: (ip: string) => Promise<boolean>;
  removeTemporary: (ip: string) => Promise<boolean>;
  removeSessionDenied: (ip: string) => Promise<boolean>;
  clearSessionDenied: () => Promise<boolean>;
  clearError: () => void;
}

export const useWhitelistStore = create<WhitelistState>((set) => ({
  status: null,
  loading: false,
  error: null,

  applyStatusSnapshot: (status) => {
    set({ status, loading: false, error: null });
  },

  fetchStatus: async () => {
    set({ loading: true, error: null });
    try {
      const status = await api.getWhitelistStatus();
      set({ status, loading: false });
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
    }
  },

  addToWhitelist: async (ipOrCidr: string) => {
    set({ loading: true, error: null });
    try {
      await api.addToWhitelist(ipOrCidr);
      set((state) => ({
        status: state.status
          ? {
              ...state.status,
              whitelist: state.status.whitelist.includes(ipOrCidr)
                ? state.status.whitelist
                : [...state.status.whitelist, ipOrCidr],
            }
          : state.status,
        loading: false,
      }));
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  removeFromWhitelist: async (ipOrCidr: string) => {
    set({ loading: true, error: null });
    try {
      await api.removeFromWhitelist(ipOrCidr);
      set((state) => ({
        status: state.status
          ? {
              ...state.status,
              whitelist: state.status.whitelist.filter((item) => item !== ipOrCidr),
            }
          : state.status,
        loading: false,
      }));
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  setMode: async (mode: AccessMode) => {
    set({ loading: true, error: null });
    try {
      const result = await api.setAccessMode(mode);
      set((state) => ({
        status: state.status ? { ...state.status, mode: result.mode } : state.status,
        loading: false,
      }));
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  setAllowLan: async (allow: boolean) => {
    set({ loading: true, error: null });
    try {
      const result = await api.setAllowLan(allow);
      set((state) => ({
        status: state.status
          ? { ...state.status, allow_lan: result.allow_lan }
          : state.status,
        loading: false,
      }));
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  setUserPassConfig: async (enabled: boolean, accounts: UserPassAccountUpdate[], loopback_requires_auth: boolean) => {
    set({ loading: true, error: null });
    try {
      await api.setUserPassConfig(enabled, accounts, loopback_requires_auth);
      const status = await api.getWhitelistStatus();
      set({ status, loading: false });
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  addTemporary: async (ip: string) => {
    set({ loading: true, error: null });
    try {
      await api.addTemporary(ip);
      set((state) => ({
        status: state.status
          ? {
              ...state.status,
              temporary_whitelist: [...state.status.temporary_whitelist, ip],
            }
          : state.status,
        loading: false,
      }));
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  removeTemporary: async (ip: string) => {
    set({ loading: true, error: null });
    try {
      await api.removeTemporary(ip);
      set((state) => ({
        status: state.status
          ? {
              ...state.status,
              temporary_whitelist: state.status.temporary_whitelist.filter(
                (item) => item !== ip,
              ),
            }
          : state.status,
        loading: false,
      }));
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  removeSessionDenied: async (ip: string) => {
    set({ loading: true, error: null });
    try {
      await api.removeSessionDenied(ip);
      set((state) => ({
        status: state.status
          ? {
              ...state.status,
              session_denied: state.status.session_denied.filter(
                (item) => item !== ip,
              ),
            }
          : state.status,
        loading: false,
      }));
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  clearSessionDenied: async () => {
    set({ loading: true, error: null });
    try {
      await api.clearSessionDenied();
      set((state) => ({
        status: state.status
          ? { ...state.status, session_denied: [] }
          : state.status,
        loading: false,
      }));
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : (e as Error).message, loading: false });
      return false;
    }
  },

  clearError: () => set({ error: null }),
}));
