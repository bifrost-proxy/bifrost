import { afterEach, describe, expect, it } from 'vitest';
import http from 'http';
import fs from 'fs';
import path from 'path';
import { createSyncServer, type SyncServerConfig, type SyncServerInstance } from '../index';

const TEST_DATA_DIR = path.join(__dirname, '.test-data-rate-limit');

let server: SyncServerInstance | undefined;

function rawGet(baseUrl: string, pathName: string): Promise<number> {
  return new Promise((resolve, reject) => {
    const url = new URL(pathName, baseUrl);
    const req = http.request(
      {
        method: 'GET',
        hostname: url.hostname,
        port: url.port,
        path: url.pathname + url.search,
      },
      (res) => {
        res.resume();
        res.on('end', () => resolve(res.statusCode ?? 0));
      },
    );
    req.on('error', reject);
    req.end();
  });
}

function rawPost(baseUrl: string, pathName: string, body: unknown): Promise<number> {
  return new Promise((resolve, reject) => {
    const url = new URL(pathName, baseUrl);
    const payload = JSON.stringify(body);
    const req = http.request(
      {
        method: 'POST',
        hostname: url.hostname,
        port: url.port,
        path: url.pathname + url.search,
        headers: {
          'Content-Type': 'application/json',
          'Content-Length': Buffer.byteLength(payload),
        },
      },
      (res) => {
        res.resume();
        res.on('end', () => resolve(res.statusCode ?? 0));
      },
    );
    req.on('error', reject);
    req.write(payload);
    req.end();
  });
}

async function startWithLimits(rateLimitPerIp: number, authRateLimitPerIp: number = 20): Promise<string> {
  if (fs.existsSync(TEST_DATA_DIR)) fs.rmSync(TEST_DATA_DIR, { recursive: true });
  fs.mkdirSync(TEST_DATA_DIR, { recursive: true });

  const config: SyncServerConfig = {
    server: {
      port: 0,
      host: '127.0.0.1',
      rate_limit_per_ip: rateLimitPerIp,
      auth_rate_limit_per_ip: authRateLimitPerIp,
    },
    storage: { type: 'sqlite', sqlite: { data_dir: TEST_DATA_DIR } },
    auth: { mode: 'password' },
  };

  server = createSyncServer(config);
  await new Promise<void>((resolve) => {
    server!.server.listen(0, '127.0.0.1', () => {
      const addr = server!.server.address();
      if (addr && typeof addr === 'object') {
        server!.port = addr.port;
      }
      resolve();
    });
  });
  return `http://127.0.0.1:${server.port}`;
}

afterEach(async () => {
  await server?.close();
  server = undefined;
  if (fs.existsSync(TEST_DATA_DIR)) fs.rmSync(TEST_DATA_DIR, { recursive: true });
});

describe('global rate limit', () => {
  it('uses server.rate_limit_per_ip for non remote-invoke paths', async () => {
    const baseUrl = await startWithLimits(2);

    expect(await rawGet(baseUrl, '/v4/sso/check')).toBe(401);
    expect(await rawGet(baseUrl, '/v4/sso/check')).toBe(401);
    expect(await rawGet(baseUrl, '/v4/sso/check')).toBe(429);
  });

  it('uses server.auth_rate_limit_per_ip for register requests', async () => {
    const baseUrl = await startWithLimits(100, 1);

    expect(await rawPost(baseUrl, '/v4/sso/register', {
      user_id: 'auth_limit_a',
      password: 'password123',
    })).toBe(200);
    expect(await rawPost(baseUrl, '/v4/sso/register', {
      user_id: 'auth_limit_b',
      password: 'password123',
    })).toBe(429);
  });
});
