import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import http from 'http';
import fs from 'fs';
import path from 'path';
import { createSyncServer, type SyncServerInstance, type SyncServerConfig } from '../index';

const TEST_DATA_DIR = path.join(__dirname, '.test-data-config-sync');
const TEST_PORT = 0;

let server: SyncServerInstance;
let baseUrl: string;

function req(
  method: string,
  urlPath: string,
  body?: unknown,
  token?: string,
): Promise<{ status: number; data: { code: number; message: string; data?: unknown } }> {
  return new Promise((resolve, reject) => {
    const url = new URL(urlPath, baseUrl);
    const options: http.RequestOptions = {
      method,
      hostname: url.hostname,
      port: url.port,
      path: url.pathname + url.search,
      headers: { 'Content-Type': 'application/json' },
    };
    if (token) {
      (options.headers as Record<string, string>)['x-bifrost-token'] = token;
    }
    const r = http.request(options, (res) => {
      let chunks = '';
      res.on('data', (c) => (chunks += c));
      res.on('end', () => {
        try {
          resolve({ status: res.statusCode!, data: JSON.parse(chunks) });
        } catch {
          resolve({ status: res.statusCode!, data: { code: -1, message: chunks } });
        }
      });
    });
    r.on('error', reject);
    if (body !== undefined) {
      r.write(JSON.stringify(body));
    }
    r.end();
  });
}

async function registerUser(userId: string, password: string): Promise<string> {
  const res = await req('POST', '/v4/sso/register', { user_id: userId, password });
  expect(res.data.code, JSON.stringify(res.data)).toBe(0);
  return (res.data.data as { token: string }).token;
}

beforeAll(async () => {
  if (fs.existsSync(TEST_DATA_DIR)) {
    fs.rmSync(TEST_DATA_DIR, { recursive: true, force: true });
  }
  fs.mkdirSync(TEST_DATA_DIR, { recursive: true });

  const config: SyncServerConfig = {
    server: {
      port: TEST_PORT,
      host: '127.0.0.1',
      rate_limit_per_ip: 1000,
      auth_rate_limit_per_ip: 1000,
    },
    storage: { type: 'sqlite', sqlite: { data_dir: TEST_DATA_DIR } },
    auth: { mode: 'password' },
  };

  server = createSyncServer(config);
  await new Promise<void>((resolve) => {
    server.server.listen(0, '127.0.0.1', () => {
      const addr = server.server.address();
      if (addr && typeof addr === 'object') {
        server.port = addr.port;
      }
      resolve();
    });
  });
  baseUrl = `http://127.0.0.1:${server.port}`;
});

afterAll(async () => {
  await server.close();
  if (fs.existsSync(TEST_DATA_DIR)) {
    fs.rmSync(TEST_DATA_DIR, { recursive: true, force: true });
  }
});

describe('Basic config sync API', () => {
  it('returns public provider capabilities', async () => {
    const res = await req('GET', '/v4/capabilities');
    expect(res.status).toBe(200);
    expect(res.data.code).toBe(0);
    const data = res.data.data as {
      provider_type: string;
      capabilities: { rules_sync: boolean; config_sync: boolean; remote_invoke: boolean };
      config_keys: string[];
    };
    expect(data.provider_type).toBe('bifrost_cloud');
    expect(data.capabilities.rules_sync).toBe(true);
    expect(data.capabilities.config_sync).toBe(true);
    expect(data.capabilities.remote_invoke).toBe(false);
    expect(data.config_keys).toContain('app_allowlist');
  });

  it('rejects unauthenticated sync requests', async () => {
    const res = await req('POST', '/v4/config/sync', {
      check_list: [],
      update_list: [],
      delete_list: [],
    });
    expect(res.status).toBe(401);
  });

  it('syncs create, check, remote update, and delete for allowed config keys', async () => {
    const token = await registerUser('config_owner', 'password123');
    const valueJson = JSON.stringify({ domains: ['example.com', '*.internal.test'] });
    const createRes = await req('POST', '/v4/config/sync', {
      check_list: [],
      update_list: [{
        user_id: 'config_owner',
        config_key: 'domain_allowlist',
        value_json: valueJson,
        hash: 'domain-hash-v1',
      }],
      delete_list: [],
    }, token);

    expect(createRes.status).toBe(200);
    expect(createRes.data.code).toBe(0);
    const createData = createRes.data.data as { result_list: Array<{ type: number; status: number; update_time: string }> };
    expect(createData.result_list[0].type).toBe(3);
    expect(createData.result_list[0].status).toBe(0);

    const sameCheckRes = await req('POST', '/v4/config/sync', {
      check_list: [{
        user_id: 'config_owner',
        config_key: 'domain_allowlist',
        update_time: createData.result_list[0].update_time,
        hash: 'domain-hash-v1',
      }],
      update_list: [],
      delete_list: [],
    }, token);
    const sameCheckData = sameCheckRes.data.data as { local_update_list: unknown[]; local_delete_list: string[] };
    expect(sameCheckData.local_update_list).toHaveLength(0);
    expect(sameCheckData.local_delete_list).toHaveLength(0);

    const staleCheckRes = await req('POST', '/v4/config/sync', {
      check_list: [{
        user_id: 'config_owner',
        config_key: 'domain_allowlist',
        update_time: '2020-01-01T00:00:00.000Z',
        hash: 'old-hash',
      }],
      update_list: [],
      delete_list: [],
    }, token);
    const staleCheckData = staleCheckRes.data.data as { local_update_list: Array<{ config_key: string; value_json: string }> };
    expect(staleCheckData.local_update_list[0].config_key).toBe('domain_allowlist');
    expect(staleCheckData.local_update_list[0].value_json).toBe(valueJson);

    const deleteRes = await req('POST', '/v4/config/sync', {
      check_list: [],
      update_list: [],
      delete_list: [{ user_id: 'config_owner', config_key: 'domain_allowlist' }],
    }, token);
    expect((deleteRes.data.data as { result_list: Array<{ status: number }> }).result_list[0].status).toBe(0);

    const deletedCheckRes = await req('POST', '/v4/config/sync', {
      check_list: [{ user_id: 'config_owner', config_key: 'domain_allowlist', update_time: 'stale', hash: 'stale' }],
      update_list: [],
      delete_list: [],
    }, token);
    expect((deletedCheckRes.data.data as { local_delete_list: string[] }).local_delete_list).toEqual(['domain_allowlist']);
  });

  it('rejects unsupported keys and sensitive payload keys', async () => {
    const token = await registerUser('config_validator', 'password123');
    const res = await req('POST', '/v4/config/sync', {
      check_list: [],
      update_list: [
        { user_id: 'config_validator', config_key: 'proxy_credentials', value_json: '{}' },
        { user_id: 'config_validator', config_key: 'blacklist', value_json: JSON.stringify({ api_token: 'secret' }) },
      ],
      delete_list: [],
    }, token);

    const data = res.data.data as { result_list: Array<{ status: number; msg: string }> };
    expect(data.result_list).toHaveLength(2);
    expect(data.result_list[0].status).toBe(1);
    expect(data.result_list[0].msg).toContain('unsupported config key');
    expect(data.result_list[1].status).toBe(1);
    expect(data.result_list[1].msg).toContain('forbidden sensitive key');
  });

  it('rejects writes for another user', async () => {
    const token = await registerUser('config_actor', 'password123');
    const res = await req('POST', '/v4/config/sync', {
      check_list: [],
      update_list: [{
        user_id: 'config_target',
        config_key: 'app_allowlist',
        value_json: JSON.stringify({ apps: ['com.example.App'] }),
      }],
      delete_list: [],
    }, token);

    const data = res.data.data as { result_list: Array<{ status: number; msg: string }> };
    expect(data.result_list[0].status).toBe(1);
    expect(data.result_list[0].msg).toContain('access config_target denied');
  });
});
