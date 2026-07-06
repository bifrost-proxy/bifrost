import { getClientId } from '../services/clientId';
import { getAdminToken } from '../services/adminAuth';
import { resolveRequestUrl } from '../runtime';
import {
  ADMIN_CSRF_HEADER,
  clearAdminCsrfToken,
  getAdminCsrfToken,
  isInvalidAdminCsrfMessage,
  isUnsafeHttpMethod,
} from './csrf';

export async function apiFetch(input: RequestInfo | URL, init: RequestInit = {}) {
  return apiFetchWithCsrfRetry(input, init, false);
}

async function apiFetchWithCsrfRetry(
  input: RequestInfo | URL,
  init: RequestInit = {},
  csrfRetried: boolean,
): Promise<Response> {
  const headers = new Headers(init.headers);
  headers.set('X-Client-Id', getClientId());
  const token = getAdminToken();
  if (token && !headers.has('Authorization')) {
    headers.set('Authorization', `Bearer ${token}`);
  }
  if (isUnsafeHttpMethod(init.method) && !headers.has(ADMIN_CSRF_HEADER)) {
    headers.set(ADMIN_CSRF_HEADER, await getAdminCsrfToken());
  }
  const response = await fetch(resolveRequestUrl(input), { ...init, headers });
  if (!csrfRetried && isUnsafeHttpMethod(init.method) && (await isInvalidCsrfResponse(response))) {
    clearAdminCsrfToken();
    const retryHeaders = new Headers(init.headers);
    retryHeaders.set('X-Client-Id', getClientId());
    if (token && !retryHeaders.has('Authorization')) {
      retryHeaders.set('Authorization', `Bearer ${token}`);
    }
    retryHeaders.set(ADMIN_CSRF_HEADER, await getAdminCsrfToken());
    return fetch(resolveRequestUrl(input), { ...init, headers: retryHeaders });
  }
  return response;
}

async function isInvalidCsrfResponse(response: Response): Promise<boolean> {
  if (response.status !== 403) {
    return false;
  }
  const clone = response.clone();
  const contentType = clone.headers.get('Content-Type') || '';
  try {
    if (contentType.includes('application/json')) {
      const payload = (await clone.json()) as { error?: string; message?: string };
      return isInvalidAdminCsrfMessage(String(payload.error || payload.message || ''));
    }
    return isInvalidAdminCsrfMessage(await clone.text());
  } catch {
    return false;
  }
}
