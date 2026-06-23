import { getClientId } from '../services/clientId';
import { getAdminToken } from '../services/adminAuth';
import { resolveRequestUrl } from '../runtime';
import { ADMIN_CSRF_HEADER, getAdminCsrfToken, isUnsafeHttpMethod } from './csrf';

export async function apiFetch(input: RequestInfo | URL, init: RequestInit = {}) {
  const headers = new Headers(init.headers);
  headers.set('X-Client-Id', getClientId());
  const token = getAdminToken();
  if (token && !headers.has('Authorization')) {
    headers.set('Authorization', `Bearer ${token}`);
  }
  if (isUnsafeHttpMethod(init.method) && !headers.has(ADMIN_CSRF_HEADER)) {
    headers.set(ADMIN_CSRF_HEADER, await getAdminCsrfToken());
  }
  return fetch(resolveRequestUrl(input), { ...init, headers });
}
