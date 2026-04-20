import { create } from "zustand";
import {
  getPendingPairings,
  approvePairing as apiApprovePairing,
  rejectPairing as apiRejectPairing,
  type PairingRequest,
  type GrantMode,
} from "../api/remoteInvoke";
import { isConnectionIssueError } from "../api/client";

interface PairingRequestState {
  pendingList: PairingRequest[];
  ignoredIds: string[];
  pollTimer: number | null;
  knownIds: string[];
  fetchPendingList: () => Promise<void>;
  approvePairing: (pairingId: string, grantMode: GrantMode) => Promise<boolean>;
  rejectPairing: (pairingId: string) => Promise<boolean>;
  ignorePairing: (pairingId: string) => void;
  ignoreAll: () => void;
  startPolling: () => void;
  stopPolling: () => void;
}

const showBrowserNotification = (title: string, body: string) => {
  if (typeof Notification !== "undefined" && Notification.permission === "granted") {
    new Notification(title, {
      body,
      icon: "/favicon.ico",
      tag: "pairing-request",
      requireInteraction: true,
    });
  }
};

export const usePairingRequestStore = create<PairingRequestState>((set, get) => ({
  pendingList: [],
  ignoredIds: [],
  pollTimer: null,
  knownIds: [],

  fetchPendingList: async () => {
    try {
      const res = await getPendingPairings();
      const list = res.pairings ?? [];
      const { ignoredIds, knownIds, pendingList } = get();

      const currentIds = list.map((p) => p.pairing_id);
      const prevIds = pendingList.map((p) => p.pairing_id);
      const listChanged =
        currentIds.length !== prevIds.length ||
        currentIds.some((id, i) => id !== prevIds[i]);

      const newPairings = list.filter(
        (p) => !knownIds.includes(p.pairing_id),
      );
      for (const p of newPairings) {
        showBrowserNotification(
          "Remote Command Authorization Request",
          `${p.caller_info.display_name || "Unknown caller"} is requesting to execute: ${p.command_summary.command_preview || p.command.command}`,
        );
      }

      if (!listChanged && newPairings.length === 0) {
        return;
      }

      const currentIdSet = new Set(currentIds);
      const validIgnored = ignoredIds.filter((id) => currentIdSet.has(id));

      set({
        pendingList: list,
        ignoredIds: validIgnored,
        knownIds: currentIds,
      });
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to fetch pending pairings:", e);
      }
    }
  },

  approvePairing: async (pairingId: string, grantMode: GrantMode) => {
    try {
      await apiApprovePairing(pairingId, grantMode);
      const { ignoredIds } = get();
      set({ ignoredIds: ignoredIds.filter((id) => id !== pairingId) });
      await get().fetchPendingList();
      return true;
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to approve pairing:", e);
      }
      await get().fetchPendingList();
      return false;
    }
  },

  rejectPairing: async (pairingId: string) => {
    try {
      await apiRejectPairing(pairingId);
      const { ignoredIds } = get();
      set({ ignoredIds: ignoredIds.filter((id) => id !== pairingId) });
      await get().fetchPendingList();
      return true;
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to reject pairing:", e);
      }
      await get().fetchPendingList();
      return false;
    }
  },

  ignorePairing: (pairingId: string) => {
    const { ignoredIds } = get();
    if (!ignoredIds.includes(pairingId)) {
      set({ ignoredIds: [...ignoredIds, pairingId] });
    }
  },

  ignoreAll: () => {
    const { pendingList } = get();
    set({ ignoredIds: pendingList.map((p) => p.pairing_id) });
  },

  startPolling: () => {
    const { pollTimer } = get();
    if (pollTimer) return;

    void get().fetchPendingList();
    const timer = window.setInterval(() => {
      void get().fetchPendingList();
    }, 3000);
    set({ pollTimer: timer });
  },

  stopPolling: () => {
    const { pollTimer } = get();
    if (pollTimer) {
      window.clearInterval(pollTimer);
      set({ pollTimer: null });
    }
  },
}));
