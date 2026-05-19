import type { TrafficRecord } from '../types';

interface HARLog {
  log: {
    version: string;
    creator: {
      name: string;
      version: string;
    };
    entries: HAREntry[];
  };
}

interface HAREntry {
  startedDateTime: string;
  time: number;
  request: {
    method: string;
    url: string;
    httpVersion: string;
    cookies: HARCookie[];
    headers: HARHeader[];
    queryString: HARQueryString[];
    postData?: {
      mimeType: string;
      text: string;
    };
    headersSize: number;
    bodySize: number;
  };
  response: {
    status: number;
    statusText: string;
    httpVersion: string;
    cookies: HARCookie[];
    headers: HARHeader[];
    content: {
      size: number;
      mimeType: string;
      text?: string;
    };
    redirectURL: string;
    headersSize: number;
    bodySize: number;
  };
  cache: object;
  timings: {
    send: number;
    wait: number;
    receive: number;
  };
}

interface HARHeader {
  name: string;
  value: string;
}

interface HARCookie {
  name: string;
  value: string;
}

interface HARQueryString {
  name: string;
  value: string;
}

function parseQueryString(url: string): HARQueryString[] {
  try {
    const urlObj = new URL(url);
    const params: HARQueryString[] = [];
    urlObj.searchParams.forEach((value, name) => {
      params.push({ name, value });
    });
    return params;
  } catch {
    return [];
  }
}

function parseCookies(headers: [string, string][] | null, cookieHeader: string): HARCookie[] {
  if (!headers) return [];
  const cookies: HARCookie[] = [];
  
  for (const [name, value] of headers) {
    if (name.toLowerCase() === cookieHeader) {
      const cookiePairs = value.split(';');
      for (const pair of cookiePairs) {
        const [cookieName, ...cookieValue] = pair.split('=');
        if (cookieName) {
          cookies.push({
            name: cookieName.trim(),
            value: cookieValue.join('=').trim(),
          });
        }
      }
    }
  }
  
  return cookies;
}

function getStatusText(status: number): string {
  const statusTexts: Record<number, string> = {
    200: 'OK',
    201: 'Created',
    204: 'No Content',
    301: 'Moved Permanently',
    302: 'Found',
    304: 'Not Modified',
    400: 'Bad Request',
    401: 'Unauthorized',
    403: 'Forbidden',
    404: 'Not Found',
    500: 'Internal Server Error',
    502: 'Bad Gateway',
    503: 'Service Unavailable',
    504: 'Gateway Timeout',
  };
  return statusTexts[status] || '';
}

function convertHeaders(headers: [string, string][] | null): HARHeader[] {
  if (!headers) return [];
  return headers.map(([name, value]) => ({ name, value }));
}

function getContentType(headers: [string, string][] | null): string {
  if (!headers) return 'application/octet-stream';
  for (const [name, value] of headers) {
    if (name.toLowerCase() === 'content-type') {
      return value.split(';')[0].trim();
    }
  }
  return 'application/octet-stream';
}

function calculateHeadersSize(headers: [string, string][] | null): number {
  if (!headers) return -1;
  let size = 0;
  for (const [name, value] of headers) {
    size += name.length + value.length + 4;
  }
  return size;
}

function effectiveHeaders(
  primary: [string, string][] | null | undefined,
  fallback: [string, string][] | null | undefined,
): [string, string][] | null {
  if (primary && primary.length > 0) return primary;
  return fallback ?? null;
}

export function recordToHAREntry(record: TrafficRecord): HAREntry {
  const requestHeaders = effectiveHeaders(record.request_headers, record.original_request_headers);
  const responseHeaders = effectiveHeaders(record.response_headers, record.original_response_headers);

  const requestContentType = getContentType(requestHeaders);
  const responseContentType = getContentType(responseHeaders);

  const timing = record.timing;
  const sendTime = timing?.send_ms ?? 0;
  const waitTime = timing?.wait_ms ?? 0;
  const receiveTime = timing?.receive_ms ?? 0;

  return {
    startedDateTime: record.timestamp
      ? new Date(record.timestamp).toISOString()
      : new Date().toISOString(),
    time: record.duration_ms,
    request: {
      method: record.method,
      url: record.url,
      httpVersion: record.protocol || 'HTTP/1.1',
      cookies: parseCookies(requestHeaders, 'cookie'),
      headers: convertHeaders(requestHeaders),
      queryString: parseQueryString(record.url),
      ...(record.request_body ? {
        postData: {
          mimeType: requestContentType,
          text: record.request_body,
        },
      } : {}),
      headersSize: calculateHeadersSize(requestHeaders),
      bodySize: record.request_size,
    },
    response: {
      status: record.status,
      statusText: getStatusText(record.status),
      httpVersion: record.protocol || 'HTTP/1.1',
      cookies: parseCookies(responseHeaders, 'set-cookie'),
      headers: convertHeaders(responseHeaders),
      content: {
        size: record.response_size,
        mimeType: responseContentType,
        ...(record.response_body ? { text: record.response_body } : {}),
      },
      redirectURL: '',
      headersSize: calculateHeadersSize(responseHeaders),
      bodySize: record.response_size,
    },
    cache: {},
    timings: {
      send: sendTime,
      wait: waitTime,
      receive: receiveTime,
    },
  };
}

export function generateHAR(records: TrafficRecord[]): HARLog {
  return {
    log: {
      version: '1.2',
      creator: {
        name: 'Bifrost',
        version: '1.0',
      },
      entries: records.map(recordToHAREntry),
    },
  };
}

export function downloadHAR(records: TrafficRecord[], filename?: string): void {
  const har = generateHAR(records);
  const blob = new Blob([JSON.stringify(har, null, 2)], { type: 'application/json' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename || `bifrost-${new Date().toISOString().slice(0, 19).replace(/[:-]/g, '')}.har`;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}
