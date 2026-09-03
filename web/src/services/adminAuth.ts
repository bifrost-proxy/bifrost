import { apiFetch } from '../api/apiFetch';

const TOKEN_KEY = 'bifrost.admin.jwt';

export type AdminAuthStatus = {
  remote_access_enabled: boolean;
  auth_required: boolean;
  username: string;
  has_password: boolean;
  locked_out: boolean;
  failed_attempts: number;
  max_attempts: number;
  min_password_length: number;
};

export function getAdminToken(): string | null {
  try {
    const v = window.localStorage.getItem(TOKEN_KEY);
    return v && v.trim() ? v : null;
  } catch {
    return null;
  }
}

export function setAdminToken(token: string): void {
  try {
    window.localStorage.setItem(TOKEN_KEY, token);
  } catch {
    // ignore
  }
}

export function clearAdminToken(): void {
  try {
    window.localStorage.removeItem(TOKEN_KEY);
  } catch {
    // ignore
  }
}

export async function fetchAdminAuthStatus(): Promise<AdminAuthStatus> {
  const resp = await apiFetch('/api/auth/status', { method: 'GET' });
  if (!resp.ok) {
    throw new Error(`Failed to load auth status: ${resp.status}`);
  }
  return (await resp.json()) as AdminAuthStatus;
}

export async function ensureAdminBrowserSession(): Promise<void> {
  const resp = await apiFetch('/api/auth/session', {
    method: 'GET',
    cache: 'no-store',
  });
  if (!resp.ok) {
    throw new Error(`Failed to establish browser session: ${resp.status}`);
  }
}

export async function changeAdminPassword(
  password: string,
  username?: string,
): Promise<void> {
  const body: Record<string, string> = { password };
  if (username) {
    body.username = username;
  }
  const resp = await apiFetch('/api/auth/passwd', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!resp.ok) {
    const data = (await resp.json().catch(() => ({}))) as { error?: string };
    throw new Error(data.error || `Failed to change password: ${resp.status}`);
  }
}

export async function setRemoteAccess(
  enabled: boolean,
): Promise<AdminAuthStatus> {
  const resp = await apiFetch('/api/auth/remote', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ enabled }),
  });
  if (!resp.ok) {
    const data = (await resp.json().catch(() => ({}))) as { error?: string };
    throw new Error(
      data.error || `Failed to set remote access: ${resp.status}`,
    );
  }
  return (await resp.json()) as AdminAuthStatus;
}

export async function revokeAllSessions(): Promise<void> {
  const resp = await apiFetch('/api/auth/revoke-all', {
    method: 'POST',
  });
  if (!resp.ok) {
    const data = (await resp.json().catch(() => ({}))) as { error?: string };
    throw new Error(data.error || `Failed to revoke sessions: ${resp.status}`);
  }
}

export type LoginAuditEntry = {
  id: number;
  ts: number;
  username: string;
  ip: string;
  ua: string;
  success: boolean;
};

export type LoginAuditResponse = {
  total: number;
  items: LoginAuditEntry[];
  limit: number;
  offset: number;
};

export async function fetchLoginAudit(
  limit = 20,
  offset = 0,
): Promise<LoginAuditResponse> {
  const resp = await apiFetch(
    `/api/admin/audit?limit=${limit}&offset=${offset}`,
    { method: 'GET' },
  );
  if (!resp.ok) {
    throw new Error(`Failed to load audit logs: ${resp.status}`);
  }
  return (await resp.json()) as LoginAuditResponse;
}
