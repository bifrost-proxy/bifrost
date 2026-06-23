import { buildApiUrl } from '../runtime';
import { getClientId } from '../services/clientId';

export const ADMIN_CSRF_HEADER = 'X-Bifrost-CSRF';

type CsrfResponse = {
  csrf_token: string;
  header_name?: string;
};

let cachedToken: string | null = null;
let inFlight: Promise<string> | null = null;

export function isUnsafeHttpMethod(method?: string): boolean {
  const normalized = (method || 'GET').toUpperCase();
  return normalized !== 'GET' && normalized !== 'HEAD' && normalized !== 'OPTIONS';
}

export async function getAdminCsrfToken(): Promise<string> {
  if (cachedToken) {
    return cachedToken;
  }
  if (inFlight) {
    return inFlight;
  }

  inFlight = fetch(buildApiUrl('/security/csrf'), {
    method: 'GET',
    cache: 'no-store',
    headers: buildCsrfFetchHeaders(),
  })
    .then(async (response) => {
      const data = (await response.json()) as CsrfResponse;
      if (!response.ok || !data.csrf_token) {
        throw new Error('Failed to obtain admin CSRF token');
      }
      cachedToken = data.csrf_token;
      return data.csrf_token;
    })
    .finally(() => {
      inFlight = null;
    });

  return inFlight;
}

function buildCsrfFetchHeaders(): Headers {
  const headers = new Headers();
  headers.set('X-Client-Id', getClientId());
  return headers;
}
