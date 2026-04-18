import { get, post, put } from './client';

export interface NotificationRecord {
  id: number;
  notification_type: string;
  title: string;
  message: string;
  metadata: string | null;
  status: string;
  action_taken: string | null;
  created_at: number;
  updated_at: number;
}

export interface NotificationListResponse {
  total: number;
  unread_count: number;
  items: NotificationRecord[];
  limit: number;
  offset: number;
}

export interface DomainFailureSummary {
  domain: string;
  definite_count: number;
  probable_count: number;
  has_success: boolean;
}

export interface ClientTrustSummary {
  identifier: string;
  identifier_type: string;
  trust_status: { status: string; reason?: string; confidence?: number; sample_count?: number };
  handshake_success: number;
  handshake_fail_untrust: number;
  handshake_fail_definite_untrust: number;
  handshake_fail_probable_untrust: number;
  handshake_fail_other: number;
  first_seen: number;
  last_seen: number;
  last_failure_domain: string | null;
  last_failure_reason: string | null;
  failed_domains: DomainFailureSummary[];
}

export interface ClientTrustResponse {
  items: ClientTrustSummary[];
  untrusted_count: number;
}

export async function getNotifications(params?: {
  type?: string;
  status?: string;
  limit?: number;
  offset?: number;
}): Promise<NotificationListResponse> {
  const searchParams = new URLSearchParams();
  if (params?.type) searchParams.set('type', params.type);
  if (params?.status) searchParams.set('status', params.status);
  if (params?.limit) searchParams.set('limit', String(params.limit));
  if (params?.offset) searchParams.set('offset', String(params.offset));
  const qs = searchParams.toString();
  return get<NotificationListResponse>(`/notifications${qs ? `?${qs}` : ''}`);
}

export async function getUnreadCount(): Promise<{ unread_count: number }> {
  return get<{ unread_count: number }>('/notifications/unread-count');
}

export async function markAllRead(type?: string): Promise<{ success: boolean; updated: number }> {
  const qs = type ? `?type=${encodeURIComponent(type)}` : '';
  return post<{ success: boolean; updated: number }>(`/notifications/mark-all-read${qs}`, {});
}

export async function updateNotificationStatus(
  id: number,
  status: string,
  actionTaken?: string,
): Promise<{ success: boolean }> {
  return put<{ success: boolean }>(`/notifications/status/${id}`, {
    status,
    action_taken: actionTaken,
  });
}

export async function getClientTrust(): Promise<ClientTrustResponse> {
  return get<ClientTrustResponse>('/notifications/client-trust');
}
