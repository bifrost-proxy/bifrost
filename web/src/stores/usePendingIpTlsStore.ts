import { create } from "zustand";
import type { PendingIpTls } from "../api/config";
import { getIpTlsPending, approveIpTls, skipIpTls, clearIpTlsPending } from "../api/config";
import { getClientId } from "../services/clientId";
import { buildApiUrl } from "../runtime";
import { isConnectionIssueError } from "../api/client";
import { t } from "../i18n";

interface PendingIpTlsEvent {
  event_type: "new" | "approved" | "skipped";
  pending: PendingIpTls;
  total_pending: number;
}

interface PendingIpTlsState {
  pendingList: PendingIpTls[];
  pendingCount: number;
  isConnected: boolean;
  eventSource: EventSource | null;
  fetchPendingList: () => Promise<void>;
  approvePending: (ip: string) => Promise<boolean>;
  skipPending: (ip: string) => Promise<boolean>;
  clearPending: () => Promise<boolean>;
  startSSE: () => void;
  stopSSE: () => void;
}

export const usePendingIpTlsStore = create<PendingIpTlsState>((set, get) => ({
  pendingList: [],
  pendingCount: 0,
  isConnected: false,
  eventSource: null,

  fetchPendingList: async () => {
    try {
      const list = await getIpTlsPending();
      set({ pendingList: list, pendingCount: list.length });
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to fetch pending IP TLS list:", e);
      }
    }
  },

  approvePending: async (ip: string) => {
    try {
      await approveIpTls(ip);
      await get().fetchPendingList();
      return true;
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to approve IP TLS:", e);
      }
      return false;
    }
  },

  skipPending: async (ip: string) => {
    try {
      await skipIpTls(ip);
      await get().fetchPendingList();
      return true;
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to skip IP TLS:", e);
      }
      return false;
    }
  },

  clearPending: async () => {
    try {
      await clearIpTlsPending();
      set({ pendingList: [], pendingCount: 0 });
      return true;
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to clear pending IP TLS:", e);
      }
      return false;
    }
  },

  startSSE: () => {
    const { eventSource } = get();
    if (eventSource) {
      return;
    }

    const url = `${buildApiUrl('/config/ip-tls/pending/stream')}?x_client_id=${encodeURIComponent(getClientId())}`;

    const es = new EventSource(url);

    es.onopen = () => {
      set({ isConnected: true });
    };

    es.onmessage = (event) => {
      try {
        const data: PendingIpTlsEvent = JSON.parse(event.data);
        const { pendingList } = get();

        switch (data.event_type) {
          case "new": {
            const exists = pendingList.some(
              (p) => p.ip === data.pending.ip,
            );
            if (!exists) {
              set({
                pendingList: [...pendingList, data.pending],
                pendingCount: data.total_pending,
              });
              if (Notification.permission === "granted") {
                new Notification(t("shared.pendingIpTls.notifyTitle"), {
                  body: t("shared.pendingIpTls.notifyBody", { ip: data.pending.ip }),
                  icon: "/favicon.ico",
                  tag: "pending-ip-tls",
                  requireInteraction: true,
                });
              }
            }
            break;
          }
          case "approved":
          case "skipped": {
            set({
              pendingList: pendingList.filter(
                (p) => p.ip !== data.pending.ip,
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
}));
