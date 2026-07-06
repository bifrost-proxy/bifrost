import { create } from "zustand";
import type { PendingAuth } from "../types";
import * as api from "../api";
import { getClientId } from "../services/clientId";
import { buildApiUrl } from "../runtime";
import { isConnectionIssueError } from "../api/client";

interface PendingAuthEvent {
  event_type: "new" | "approved" | "rejected";
  pending_auth: PendingAuth;
  total_pending: number;
}

interface PendingAuthState {
  pendingList: PendingAuth[];
  pendingCount: number;
  isConnected: boolean;
  eventSource: EventSource | null;
  notificationPermission: NotificationPermission;
  fetchPendingList: () => Promise<void>;
  approvePending: (ip: string) => Promise<boolean>;
  rejectPending: (ip: string) => Promise<boolean>;
  clearPending: () => Promise<boolean>;
  startSSE: () => void;
  stopSSE: () => void;
  requestNotificationPermission: () => Promise<void>;
}

const showNotification = (title: string, body: string) => {
  if (Notification.permission === "granted") {
    new Notification(title, {
      body,
      icon: "/favicon.ico",
      tag: "pending-auth",
      requireInteraction: true,
    });
  }
};

export const usePendingAuthStore = create<PendingAuthState>((set, get) => ({
  pendingList: [],
  pendingCount: 0,
  isConnected: false,
  eventSource: null,
  notificationPermission:
    typeof Notification !== "undefined" ? Notification.permission : "denied",

  fetchPendingList: async () => {
    try {
      const list = await api.getPendingAuthorizations();
      set({ pendingList: list, pendingCount: list.length });
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to fetch pending list:", e);
      }
    }
  },

  approvePending: async (ip: string) => {
    try {
      await api.approvePending(ip);
      await get().fetchPendingList();
      return true;
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to approve pending:", e);
      }
      await get().fetchPendingList();
      return false;
    }
  },

  rejectPending: async (ip: string) => {
    try {
      await api.rejectPending(ip);
      await get().fetchPendingList();
      return true;
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to reject pending:", e);
      }
      await get().fetchPendingList();
      return false;
    }
  },

  clearPending: async () => {
    try {
      await api.clearPending();
      set({ pendingList: [], pendingCount: 0 });
      return true;
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to clear pending:", e);
      }
      await get().fetchPendingList();
      return false;
    }
  },

  startSSE: () => {
    const { eventSource } = get();
    if (eventSource) {
      return;
    }

    const url = `${buildApiUrl('/whitelist/pending/stream')}?x_client_id=${encodeURIComponent(getClientId())}`;

    const es = new EventSource(url);

    es.onopen = () => {
      set({ isConnected: true });
    };

    es.onmessage = (event) => {
      try {
        const data: PendingAuthEvent = JSON.parse(event.data);
        const { pendingList } = get();

        switch (data.event_type) {
          case "new": {
            const exists = pendingList.some(
              (p) => p.ip === data.pending_auth.ip,
            );
            if (!exists) {
              set({
                pendingList: [...pendingList, data.pending_auth],
                pendingCount: data.total_pending,
              });
              showNotification(
                "New Device Connection Request",
                `Device ${data.pending_auth.ip} is requesting proxy access`,
              );
            }
            break;
          }
          case "approved":
          case "rejected": {
            set({
              pendingList: pendingList.filter(
                (p) => p.ip !== data.pending_auth.ip,
              ),
              pendingCount: data.total_pending,
            });
            break;
          }
        }
      } catch (e) {
        console.error("Failed to parse SSE event:", e);
      }
    };

    es.onerror = () => {
      set({ isConnected: false });
      es.close();
      set({ eventSource: null });
      setTimeout(() => {
        get().startSSE();
      }, 5000);
    };

    set({ eventSource: es });
  },

  stopSSE: () => {
    const { eventSource } = get();
    if (eventSource) {
      eventSource.close();
      set({ eventSource: null, isConnected: false });
    }
  },

  requestNotificationPermission: async () => {
    if (typeof Notification === "undefined") {
      return;
    }

    if (Notification.permission === "default") {
      const permission = await Notification.requestPermission();
      set({ notificationPermission: permission });
    }
  },
}));
