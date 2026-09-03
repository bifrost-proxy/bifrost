import { beforeEach, describe, expect, it, vi } from 'vitest';

const fetchMocks = vi.hoisted(() => ({
  apiFetch: vi.fn(),
}));

vi.mock('../api/apiFetch', () => fetchMocks);

import { ensureAdminBrowserSession } from './adminAuth';

describe('admin browser session', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('establishes the HttpOnly cookie before native browser streams start', async () => {
    fetchMocks.apiFetch.mockResolvedValue(new Response('{}', { status: 200 }));

    await ensureAdminBrowserSession();

    expect(fetchMocks.apiFetch).toHaveBeenCalledWith('/api/auth/session', {
      method: 'GET',
      cache: 'no-store',
    });
  });

  it('rejects an invalid legacy token so the auth gate can return to login', async () => {
    fetchMocks.apiFetch.mockResolvedValue(new Response('{}', { status: 401 }));

    await expect(ensureAdminBrowserSession()).rejects.toThrow(
      'Failed to establish browser session: 401',
    );
  });
});
