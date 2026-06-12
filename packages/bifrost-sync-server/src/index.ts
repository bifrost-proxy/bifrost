import { createServer, IncomingMessage, ServerResponse, Server } from 'http';
import { createStorage, type IStorage } from './dao';
import { handleSso } from './routes/sso';
import { handleOAuth2 } from './routes/oauth2';
import { handleEnv } from './routes/env';
import { handleGroup } from './routes/group';
import { handleUser } from './routes/user';
import { handleRemoteInvoke } from './routes/remote-invoke';
import { readBody, sendJson, sendError, sendRateLimited, setSecurityHeaders, type RequestContext } from './http';
import { RateLimiter, AccountLockManager, getClientIp } from './security';
import type { SyncServerConfig } from './types';

export type { SyncServerConfig, MysqlConfig, OAuth2Config, AuthConfig, StorageConfig, ServerConfig } from './types';
export type { Env, User, CreateEnvReq, UpdateEnvReq, SearchEnvQuery, ApiResponse, Group, GroupMember, GroupSetting, CreateGroupReq, UpdateGroupReq, SearchGroupQuery, InviteGroupReq, UpdateGroupSettingReq } from './types';
export type { IStorage, IUserDao, IEnvDao, IGroupDao, IGroupMemberDao, IGroupSettingDao } from './dao';
export { createStorage } from './dao';
export { SqliteStorage } from './dao/sqlite';
export { MysqlStorage } from './dao/mysql';
export { loadConfig } from './config';

export interface SyncServerInstance {
  server: Server;
  storage: IStorage;
  port: number;
  close: () => Promise<void>;
}

function isRemoteInvokePath(pathname: string): boolean {
  return pathname.startsWith('/v4/remote-invoke/');
}

export function createSyncServer(config: SyncServerConfig): SyncServerInstance {
  const storage = createStorage(config.storage);
  const authMode = config.auth.mode;
  const oauth2Config = config.auth.oauth2;
  const trustForwardedFor = config.server.trust_forwarded_for ?? false;
  const rateLimitPerIp = Number.isFinite(config.server.rate_limit_per_ip)
    && (config.server.rate_limit_per_ip ?? 0) > 0
    ? Math.floor(config.server.rate_limit_per_ip!)
    : 200;
  const authRateLimitPerIp = Number.isFinite(config.server.auth_rate_limit_per_ip)
    && (config.server.auth_rate_limit_per_ip ?? 0) > 0
    ? Math.floor(config.server.auth_rate_limit_per_ip!)
    : 20;

  const globalLimiter = new RateLimiter(rateLimitPerIp, 60_000);
  const authLimiter = new RateLimiter(authRateLimitPerIp, 60_000);
  const accountLock = new AccountLockManager(5, 15 * 60_000, 30 * 60_000);

  const server = createServer(async (req: IncomingMessage, res: ServerResponse) => {
    setSecurityHeaders(res);
    res.setHeader('Access-Control-Allow-Origin', '*');
    res.setHeader(
      'Access-Control-Allow-Methods',
      'GET, POST, PUT, PATCH, DELETE, OPTIONS',
    );
    res.setHeader(
      'Access-Control-Allow-Headers',
      'Content-Type, x-bifrost-token, Authorization',
    );

    if (req.method === 'OPTIONS') {
      res.writeHead(204);
      res.end();
      return;
    }

    const clientIp = getClientIp(req, trustForwardedFor);
    const url = new URL(req.url ?? '/', `http://${req.headers.host ?? 'localhost'}`);

    // Remote-invoke has route-specific rate controls keyed by client/call/grant.
    // Avoid sharing the coarse global IP limiter with those workflows, otherwise
    // users behind the same NAT can throttle each other.
    if (!isRemoteInvokePath(url.pathname)) {
      const globalCheck = globalLimiter.check(clientIp);
      if (!globalCheck.allowed) {
        sendRateLimited(res, globalCheck.retryAfterMs);
        return;
      }
    }

    const isAuthPath = url.pathname === '/v4/sso/login' || url.pathname === '/v4/sso/register';
    if (isAuthPath && req.method === 'POST') {
      const authCheck = authLimiter.check(clientIp);
      if (!authCheck.allowed) {
        sendRateLimited(res, authCheck.retryAfterMs);
        return;
      }
    }

    let body: string;
    try {
      body = await readBody(req);
    } catch (e: unknown) {
      if (e instanceof Error && e.message === 'BODY_TOO_LARGE') {
        sendError(res, 413, 'request body too large (max 1MB)');
        return;
      }
      sendError(res, 400, 'failed to read request body');
      return;
    }

    const ctx: RequestContext = { req, res, url, body, clientIp, trustForwardedFor };

    try {
      if (authMode === 'oauth2' && oauth2Config) {
        if (await handleOAuth2(ctx, storage, oauth2Config)) return;
      }

      if (await handleSso(ctx, storage, accountLock)) return;
      if (await handleUser(ctx, storage)) return;
      if (await handleEnv(ctx, storage)) return;
      if (await handleGroup(ctx, storage)) return;
      if (await handleRemoteInvoke(ctx, storage, config.remote_invoke)) return;

      sendJson(res, 404, { code: -1, message: 'Not Found' });
    } catch (e: unknown) {
      console.error('[bifrost-sync-server] unhandled error:', e);
      sendError(res, 500, 'Internal Server Error');
    }
  });

  const result: SyncServerInstance = {
    server,
    storage,
    port: config.server.port,
    close: async () => {
      globalLimiter.destroy();
      authLimiter.destroy();
      accountLock.destroy();
      await storage.close();
      await new Promise<void>((resolve, reject) => {
        server.close((err) => (err ? reject(err) : resolve()));
      });
    },
  };

  return result;
}

export async function startSyncServer(config: SyncServerConfig): Promise<SyncServerInstance> {
  const instance = createSyncServer(config);
  await new Promise<void>((resolve) => {
    instance.server.listen(config.server.port, config.server.host, () => {
      const addr = instance.server.address();
      if (addr && typeof addr === 'object') {
        instance.port = addr.port;
      }
      resolve();
    });
  });
  return instance;
}
