import { create } from 'zustand';
import type { NotificationRecord, ClientTrustSummary } from '../api/notifications';
import {
  getNotifications,
  getClientTrust,
  markAllRead,
  updateNotificationStatus,
  getUnreadCount,
} from '../api/notifications';
import { isConnectionIssueError } from '../api/client';

interface NotificationState {
  notifications: NotificationRecord[];
  clientTrust: ClientTrustSummary[];
  untrustedCount: number;
  unreadCount: number;
  total: number;
  activeTab: string;
  loading: boolean;
  fetchNotifications: (type?: string, status?: string) => Promise<void>;
  fetchClientTrust: () => Promise<void>;
  fetchUnreadCount: () => Promise<void>;
  handleMarkAllRead: (type?: string, status?: string) => Promise<void>;
  handleUpdateStatus: (
    id: number,
    status: string,
    action?: string,
    type?: string,
    filterStatus?: string,
  ) => Promise<void>;
  setActiveTab: (tab: string) => void;
}

export const useNotificationStore = create<NotificationState>((set, get) => ({
  notifications: [],
  clientTrust: [],
  untrustedCount: 0,
  unreadCount: 0,
  total: 0,
  activeTab: 'all',
  loading: false,

  fetchNotifications: async (type?: string, status?: string) => {
    set({ loading: true });
    try {
      const res = await getNotifications({
        type: type === 'all' ? undefined : type,
        status: status === 'all' ? undefined : status,
        limit: 100,
      });
      set({
        notifications: res.items,
        total: Number(res.total),
        unreadCount: Number(res.unread_count),
        loading: false,
      });
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error('Failed to fetch notifications:', e);
      }
      set({ loading: false });
    }
  },

  fetchClientTrust: async () => {
    try {
      const res = await getClientTrust();
      set({ clientTrust: res.items, untrustedCount: res.untrusted_count });
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error('Failed to fetch client trust:', e);
      }
    }
  },

  fetchUnreadCount: async () => {
    try {
      const res = await getUnreadCount();
      set({ unreadCount: res.unread_count });
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error('Failed to fetch unread count:', e);
      }
    }
  },

  handleMarkAllRead: async (type?: string, status?: string) => {
    try {
      await markAllRead(type);
      await get().fetchNotifications(type ?? get().activeTab, status);
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error('Failed to mark all read:', e);
      }
    }
  },

  handleUpdateStatus: async (
    id: number,
    status: string,
    action?: string,
    type?: string,
    filterStatus?: string,
  ) => {
    try {
      await updateNotificationStatus(id, status, action);
      await get().fetchNotifications(type ?? get().activeTab, filterStatus);
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error('Failed to update notification status:', e);
      }
    }
  },

  setActiveTab: (tab: string) => {
    set({ activeTab: tab });
  },
}));
