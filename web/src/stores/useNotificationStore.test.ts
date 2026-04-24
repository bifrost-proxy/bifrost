import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../api/notifications', () => ({
  getNotifications: vi.fn(),
  getClientTrust: vi.fn(),
  markAllRead: vi.fn(),
  updateNotificationStatus: vi.fn(),
  getUnreadCount: vi.fn(),
}));

vi.mock('../api/client', () => ({
  isConnectionIssueError: vi.fn(() => false),
}));

import { useNotificationStore } from './useNotificationStore';
import * as notificationApi from '../api/notifications';

describe('useNotificationStore', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useNotificationStore.setState({
      notifications: [],
      clientTrust: [],
      untrustedCount: 0,
      unreadCount: 0,
      total: 0,
      activeTab: 'all',
      loading: false,
    });
    vi.mocked(notificationApi.getNotifications).mockResolvedValue({
      total: 1,
      unread_count: 1,
      items: [
        {
          id: 1,
          notification_type: 'tls_trust_change',
          title: 'TLS notification',
          message: 'Needs action',
          metadata: null,
          status: 'unread',
          action_taken: null,
          created_at: 1,
          updated_at: 1,
        },
      ],
      limit: 100,
      offset: 0,
    });
    vi.mocked(notificationApi.markAllRead).mockResolvedValue({ success: true, updated: 1 });
    vi.mocked(notificationApi.updateNotificationStatus).mockResolvedValue({ success: true });
  });

  it('passes tab and status filters to getNotifications', async () => {
    await useNotificationStore.getState().fetchNotifications('pending_authorization', 'unread');

    expect(notificationApi.getNotifications).toHaveBeenCalledWith({
      type: 'pending_authorization',
      status: 'unread',
      limit: 100,
    });
    expect(useNotificationStore.getState().total).toBe(1);
    expect(useNotificationStore.getState().unreadCount).toBe(1);
  });

  it('omits type and status when all filters are selected', async () => {
    await useNotificationStore.getState().fetchNotifications('all', 'all');

    expect(notificationApi.getNotifications).toHaveBeenCalledWith({
      type: undefined,
      status: undefined,
      limit: 100,
    });
  });

  it('refreshes the current filtered list after mark all read', async () => {
    await useNotificationStore.getState().handleMarkAllRead('tls_trust_change', 'unread');

    expect(notificationApi.markAllRead).toHaveBeenCalledWith('tls_trust_change');
    expect(notificationApi.getNotifications).toHaveBeenCalledWith({
      type: 'tls_trust_change',
      status: 'unread',
      limit: 100,
    });
  });

  it('refreshes the current filtered list after updating a notification', async () => {
    await useNotificationStore
      .getState()
      .handleUpdateStatus(7, 'read', 'passthrough', 'tls_trust_change', 'read');

    expect(notificationApi.updateNotificationStatus).toHaveBeenCalledWith(
      7,
      'read',
      'passthrough',
    );
    expect(notificationApi.getNotifications).toHaveBeenCalledWith({
      type: 'tls_trust_change',
      status: 'read',
      limit: 100,
    });
  });
});
