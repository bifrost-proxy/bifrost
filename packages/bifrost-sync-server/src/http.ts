import type { IncomingMessage, ServerResponse } from 'http';
import type { IStorage } from './dao/types';
import type { User } from './types';

export const MAX_BODY_SIZE = 2 * 1024 * 1024;

export interface RequestContext {
  req: IncomingMessage;
  res: ServerResponse;
  url: URL;
  body: string;
  clientIp: string;
  trustForwardedFor: boolean;
  user?: User;
}

export function sendJson(res: ServerResponse, statusCode: number, body: unknown) {
  res.writeHead(statusCode, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify(body));
}

export function sendError(res: ServerResponse, statusCode: number, message: string) {
  sendJson(res, statusCode, { code: -1, message });
}

export function sendUnauthorized(res: ServerResponse) {
  sendJson(res, 401, { code: -10001, message: 'unauthorized' });
}

export function sendRateLimited(res: ServerResponse, retryAfterMs: number) {
  const retryAfterSec = Math.ceil(retryAfterMs / 1000);
  res.setHeader('Retry-After', String(retryAfterSec));
  sendJson(res, 429, { code: -1, message: `too many requests, retry after ${retryAfterSec}s` });
}

export async function readBody(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of req) {
    const buf = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    size += buf.length;
    if (size > MAX_BODY_SIZE) {
      throw new Error('BODY_TOO_LARGE');
    }
    chunks.push(buf);
  }
  return Buffer.concat(chunks).toString('utf8');
}

export async function requireAuth(ctx: RequestContext, storage: IStorage): Promise<boolean> {
  const token = ctx.req.headers['x-bifrost-token'] as string | undefined;
  if (!token) {
    sendUnauthorized(ctx.res);
    return false;
  }
  const user = await storage.user.findByToken(token);
  if (!user) {
    sendUnauthorized(ctx.res);
    return false;
  }
  ctx.user = user;
  return true;
}

export async function optionalAuth(ctx: RequestContext, storage: IStorage): Promise<void> {
  const token = ctx.req.headers['x-bifrost-token'] as string | undefined;
  if (token) {
    const user = await storage.user.findByToken(token);
    if (user) {
      ctx.user = user;
    }
  }
}

export function parseQuery(url: URL, key: string): string | undefined {
  return url.searchParams.get(key) ?? undefined;
}

export function parseQueryAll(url: URL, key: string): string[] {
  return url.searchParams.getAll(key);
}

export function parseJsonBody<T>(body: string): T | null {
  try {
    return JSON.parse(body) as T;
  } catch {
    return null;
  }
}

export function extractPathParam(pathname: string, prefix: string): string {
  return pathname.slice(prefix.length).replace(/^\//, '').replace(/\/$/, '');
}

export function setSecurityHeaders(res: ServerResponse) {
  res.setHeader('X-Content-Type-Options', 'nosniff');
  res.setHeader('X-Frame-Options', 'DENY');
  res.setHeader('X-XSS-Protection', '1; mode=block');
  res.setHeader('Referrer-Policy', 'strict-origin-when-cross-origin');
  res.setHeader('Cache-Control', 'no-store');
}

export function setHtmlSecurityHeaders(res: ServerResponse) {
  setSecurityHeaders(res);
  res.setHeader(
    'Content-Security-Policy',
    "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
  );
}

export function openSse(res: ServerResponse) {
  res.writeHead(200, {
    'Content-Type': 'text/event-stream',
    'Cache-Control': 'no-cache, no-transform',
    Connection: 'keep-alive',
    'X-Accel-Buffering': 'no',
  });
  res.flushHeaders();
}

export function writeSseEvent(res: ServerResponse, event: string, data: unknown, id?: string) {
  if (id) res.write(`id: ${id}\n`);
  res.write(`event: ${event}\n`);
  res.write(`data: ${JSON.stringify(data)}\n\n`);
}

export function writeSseComment(res: ServerResponse, comment: string) {
  res.write(`: ${comment}\n\n`);
}

export function closeSse(res: ServerResponse) {
  try {
    res.end();
  } catch {}
}

export function extractBearerToken(req: IncomingMessage): string | undefined {
  const auth = req.headers['authorization'];
  if (!auth || !auth.startsWith('Bearer ')) return undefined;
  return auth.slice(7);
}
