import crypto from 'crypto';
import type { IStorage } from '../dao/types';
import type { RequestContext } from '../http';
import {
  sendJson,
  sendError,
  requireAuth,
  parseJsonBody,
} from '../http';
import type { BasicConfigKey, RemoteInvokeConfig } from '../types';

const BASIC_CONFIG_KEYS = new Set<BasicConfigKey>([
  'app_allowlist',
  'domain_allowlist',
  'blacklist',
]);

const FORBIDDEN_CONFIG_KEYWORDS = [
  'token',
  'password',
  'secret',
  'credential',
  'cookie',
  'authorization',
  'private_key',
  'access_key',
];

interface SyncBasicConfigReq {
  user_ids?: string[];
  check_list?: Array<{
    id?: string;
    user_id: string;
    config_key?: string;
    update_time?: string;
    hash?: string;
  }>;
  update_list?: Array<{
    id?: string;
    user_id: string;
    config_key?: string;
    value_json: string;
    hash?: string;
    update_time?: string;
  }>;
  delete_list?: Array<{
    id?: string;
    user_id: string;
    config_key?: string;
    delete_time?: string;
  }>;
}

function normalizeConfigKey(id?: string, configKey?: string): BasicConfigKey | undefined {
  const key = (configKey || id || '').trim();
  if (BASIC_CONFIG_KEYS.has(key as BasicConfigKey)) {
    return key as BasicConfigKey;
  }
  return undefined;
}

function hasForbiddenConfigKey(value: unknown): boolean {
  if (Array.isArray(value)) {
    return value.some(hasForbiddenConfigKey);
  }
  if (!value || typeof value !== 'object') {
    return false;
  }
  for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
    const lower = key.toLowerCase();
    if (FORBIDDEN_CONFIG_KEYWORDS.some((keyword) => lower.includes(keyword))) {
      return true;
    }
    if (hasForbiddenConfigKey(child)) {
      return true;
    }
  }
  return false;
}

function validateConfigJson(valueJson: string): string | undefined {
  if (valueJson.length > 64 * 1024) {
    return 'config payload too large';
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(valueJson);
  } catch {
    return 'value_json must be valid JSON';
  }
  if (hasForbiddenConfigKey(parsed)) {
    return 'config payload contains a forbidden sensitive key';
  }
  return undefined;
}

function hashValue(valueJson: string): string {
  return crypto.createHash('sha256').update(valueJson).digest('hex');
}

export async function handleConfig(
  ctx: RequestContext,
  storage: IStorage,
  remoteInvokeConfig?: RemoteInvokeConfig,
): Promise<boolean> {
  const { url, req } = ctx;
  const method = req.method ?? 'GET';
  const pathname = url.pathname.replace(/\/$/, '') || '/';

  if (pathname === '/v4/capabilities' && method === 'GET') {
    sendJson(ctx.res, 200, {
      code: 0,
      message: 'ok',
      data: {
        provider_type: 'bifrost_cloud',
        capabilities: {
          rules_sync: true,
          config_sync: true,
          remote_invoke: remoteInvokeConfig?.enabled === true,
        },
        config_keys: Array.from(BASIC_CONFIG_KEYS),
      },
    });
    return true;
  }

  if (pathname === '/v4/config/sync' && method === 'POST') {
    return handleConfigSync(ctx, storage);
  }

  return false;
}

async function handleConfigSync(ctx: RequestContext, storage: IStorage): Promise<boolean> {
  if (!(await requireAuth(ctx, storage))) return true;

  const body = parseJsonBody<SyncBasicConfigReq>(ctx.body);
  if (!body) {
    sendError(ctx.res, 400, 'invalid JSON body');
    return true;
  }

  const resultList: Array<{
    type: number;
    status: number;
    msg?: string;
    user_id?: string;
    id?: string;
    config_key?: string;
    value_json?: string;
    hash?: string;
    create_time?: string;
    update_time?: string;
  }> = [];
  const localUpdateList: unknown[] = [];
  const localDeleteList: string[] = [];
  const currentUserId = ctx.user!.user_id;

  for (const item of body.delete_list ?? []) {
    const key = normalizeConfigKey(item.id, item.config_key);
    if (!key) {
      resultList.push({ type: 0, user_id: item.user_id, id: item.id, status: 1, msg: 'unsupported config key' });
      continue;
    }
    if (item.user_id !== currentUserId) {
      resultList.push({ type: 0, user_id: item.user_id, id: key, config_key: key, status: 1, msg: `access ${item.user_id} denied` });
      continue;
    }
    await storage.basicConfig.delete(item.user_id, key);
    resultList.push({ type: 0, user_id: item.user_id, id: key, config_key: key, status: 0 });
  }

  for (const item of body.update_list ?? []) {
    const key = normalizeConfigKey(item.id, item.config_key);
    if (!key) {
      resultList.push({ type: 1, user_id: item.user_id, id: item.id, status: 1, msg: 'unsupported config key' });
      continue;
    }
    if (item.user_id !== currentUserId) {
      resultList.push({ type: 1, user_id: item.user_id, id: key, config_key: key, status: 1, msg: `access ${item.user_id} denied` });
      continue;
    }
    const validationError = validateConfigJson(item.value_json);
    if (validationError) {
      resultList.push({ type: 1, user_id: item.user_id, id: key, config_key: key, status: 1, msg: validationError });
      continue;
    }
    const existing = await storage.basicConfig.find(item.user_id, key);
    const saved = await storage.basicConfig.upsert({
      user_id: item.user_id,
      config_key: key,
      value_json: item.value_json,
      hash: item.hash || hashValue(item.value_json),
    });
    resultList.push({ type: existing ? 1 : 3, status: 0, ...saved });
  }

  for (const item of body.check_list ?? []) {
    const key = normalizeConfigKey(item.id, item.config_key);
    if (!key) {
      continue;
    }
    if (item.user_id !== currentUserId) {
      continue;
    }
    const remoteConfig = await storage.basicConfig.find(item.user_id, key);
    if (!remoteConfig) {
      localDeleteList.push(key);
      continue;
    }
    if (remoteConfig.update_time !== item.update_time || remoteConfig.hash !== item.hash) {
      localUpdateList.push(remoteConfig);
    }
  }

  sendJson(ctx.res, 200, {
    code: 0,
    message: 'ok',
    data: {
      result_list: resultList,
      local_update_list: localUpdateList,
      local_delete_list: localDeleteList,
    },
  });
  return true;
}
