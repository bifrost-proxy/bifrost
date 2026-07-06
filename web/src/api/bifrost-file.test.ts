import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  exportNetwork,
  formatBifrostFileError,
  formatImportSuccessMessage,
  getEmptyExportMessage,
  getExportItemCount,
  getImportedItemCount,
  importFile,
  previewFile,
  type ImportResponse,
} from './bifrost-file';
import { clearAdminCsrfToken } from './csrf';

const originalFetch = globalThis.fetch;

beforeEach(() => {
  clearAdminCsrfToken();
});

afterEach(() => {
  clearAdminCsrfToken();
  globalThis.fetch = originalFetch;
  vi.restoreAllMocks();
});

describe('bifrost file import helpers', () => {
  it('reports zero network imports as an empty package', () => {
    const result: ImportResponse = {
      success: true,
      file_type: 'network',
      data: { record_count: 0 },
    };

    expect(getImportedItemCount(result)).toBe(0);
    expect(formatImportSuccessMessage(result, 'empty.bifrost')).toBe(
      'empty.bifrost contains no network items to import',
    );
  });

  it('includes the actual imported item count for non-empty packages', () => {
    const result: ImportResponse = {
      success: true,
      file_type: 'network',
      data: { record_count: 2 },
    };

    expect(getImportedItemCount(result)).toBe(2);
    expect(formatImportSuccessMessage(result, 'traffic.bifrost')).toBe(
      'Imported traffic.bifrost successfully (2 items)',
    );
  });

  it('extracts backend error messages from axios responses', () => {
    const error = {
      isAxiosError: true,
      message: 'Request failed with status code 400',
      response: {
        data: {
          error: 'Network file contains 0 records; nothing to import.',
        },
      },
    };

    expect(formatBifrostFileError(error)).toBe(
      'Network file contains 0 records; nothing to import.',
    );
  });
});

describe('bifrost file export helpers', () => {
  it('blocks empty network exports before they can create a package', () => {
    const request = { record_ids: [], include_body: true };

    expect(getExportItemCount('network', request)).toBe(0);
    expect(getEmptyExportMessage('network', request)).toBe(
      'Select at least one Network record before exporting a .bifrost file',
    );
  });

  it('allows network exports with at least one selected record', () => {
    const request = { record_ids: ['REQ-1'], include_body: true };

    expect(getExportItemCount('network', request)).toBe(1);
    expect(getEmptyExportMessage('network', request)).toBeUndefined();
  });

});

describe('bifrost file API requests', () => {
  it('previews files through the CSRF-aware API client', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      void init;
      const url = String(input);
      if (url.includes('/security/csrf')) {
        return new Response(JSON.stringify({ csrf_token: 'csrf-token-for-preview' }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (url.includes('/bifrost-file/preview')) {
        return new Response(
          JSON.stringify({
            file_type: 'rules',
            meta: {},
            rules: {
              name: 'Default',
              enabled: true,
              line_count: 1,
              content: 'example.com proxy://127.0.0.1:8080',
            },
          }),
          {
            status: 200,
            headers: { 'Content-Type': 'application/json' },
          },
        );
      }
      return new Response('not found', { status: 404 });
    });
    globalThis.fetch = fetchMock as typeof fetch;

    await expect(previewFile('rules package')).resolves.toMatchObject({
      file_type: 'rules',
      rules: { name: 'Default' },
    });

    expect(fetchMock).toHaveBeenCalledTimes(2);
    const [, previewInit] = fetchMock.mock.calls[1];
    const headers = new Headers(previewInit?.headers);
    expect(previewInit?.method).toBe('POST');
    expect(headers.get('Content-Type')).toBe('text/plain');
    expect(headers.get('X-Bifrost-CSRF')).toBe('csrf-token-for-preview');
  });

  it('imports files through the CSRF-aware API client', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      void init;
      const url = String(input);
      if (url.includes('/security/csrf')) {
        return new Response(JSON.stringify({ csrf_token: 'csrf-token-for-import' }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (url.includes('/bifrost-file/import')) {
        return new Response(
          JSON.stringify({
            success: true,
            file_type: 'rules',
            data: { rule_count: 1 },
          }),
          {
            status: 200,
            headers: { 'Content-Type': 'application/json' },
          },
        );
      }
      return new Response('not found', { status: 404 });
    });
    globalThis.fetch = fetchMock as typeof fetch;

    await expect(importFile('rules package')).resolves.toMatchObject({
      success: true,
      file_type: 'rules',
      data: { rule_count: 1 },
    });

    expect(fetchMock).toHaveBeenCalledTimes(2);
    const [, importInit] = fetchMock.mock.calls[1];
    const headers = new Headers(importInit?.headers);
    expect(importInit?.method).toBe('POST');
    expect(headers.get('Content-Type')).toBe('text/plain');
    expect(headers.get('X-Bifrost-CSRF')).toBe('csrf-token-for-import');
  });

  it('exports network files through the CSRF-aware API client', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      void init;
      const url = String(input);
      if (url.includes('/security/csrf')) {
        return new Response(JSON.stringify({ csrf_token: 'csrf-token-for-export' }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      if (url.includes('/bifrost-file/export/network')) {
        return new Response('network package', { status: 200 });
      }
      return new Response('not found', { status: 404 });
    });
    globalThis.fetch = fetchMock as typeof fetch;

    await expect(exportNetwork({ record_ids: ['REQ-1'], include_body: true })).resolves.toBe(
      'network package',
    );

    expect(fetchMock).toHaveBeenCalledTimes(2);
    const [, exportInit] = fetchMock.mock.calls[1];
    const headers = new Headers(exportInit?.headers);
    expect(exportInit?.method).toBe('POST');
    expect(headers.get('X-Bifrost-CSRF')).toBe('csrf-token-for-export');
  });
});
