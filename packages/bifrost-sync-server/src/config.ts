import fs from 'fs';
import path from 'path';
import yaml from 'js-yaml';
import type { SyncServerConfig } from './types';

const DEFAULT_CONFIG: SyncServerConfig = {
  server: {
    port: 8686,
    host: '0.0.0.0',
    // DANGEROUS: only enable when deployed behind a trusted reverse proxy.
    trust_forwarded_for: false,
    rate_limit_per_ip: 200,
    auth_rate_limit_per_ip: 20,
  },
  storage: {
    type: 'sqlite',
    sqlite: { data_dir: './bifrost-sync-data' },
  },
  auth: {
    mode: 'password',
  },
};

function deepMerge(target: Record<string, unknown>, source: Record<string, unknown>): Record<string, unknown> {
  const result = { ...target };
  for (const key of Object.keys(source)) {
    const sv = source[key];
    const tv = target[key];
    if (sv && typeof sv === 'object' && !Array.isArray(sv) && tv && typeof tv === 'object' && !Array.isArray(tv)) {
      result[key] = deepMerge(tv as Record<string, unknown>, sv as Record<string, unknown>);
    } else {
      result[key] = sv;
    }
  }
  return result;
}

export function loadConfig(configPath?: string): SyncServerConfig {
  if (!configPath) {
    const candidates = ['config.yaml', 'config.yml', 'config.json'];
    for (const name of candidates) {
      const p = path.resolve(name);
      if (fs.existsSync(p)) {
        configPath = p;
        break;
      }
    }
  }

  if (!configPath || !fs.existsSync(configPath)) {
    console.log('[bifrost-sync-server] no config file found, using defaults');
    return { ...DEFAULT_CONFIG };
  }

  console.log(`[bifrost-sync-server] loading config from ${configPath}`);
  const raw = fs.readFileSync(configPath, 'utf8');
  const ext = path.extname(configPath).toLowerCase();

  let parsed: Record<string, unknown>;
  if (ext === '.json') {
    parsed = JSON.parse(raw) as Record<string, unknown>;
  } else {
    parsed = yaml.load(raw) as Record<string, unknown>;
  }

  if (!parsed || typeof parsed !== 'object') {
    console.warn('[bifrost-sync-server] config file is empty or invalid, using defaults');
    return { ...DEFAULT_CONFIG };
  }

  const merged = deepMerge(DEFAULT_CONFIG as unknown as Record<string, unknown>, parsed) as unknown as SyncServerConfig;

  if (merged.storage.type === 'sqlite' && !merged.storage.sqlite) {
    merged.storage.sqlite = { data_dir: './bifrost-sync-data' };
  }

  if (merged.storage.sqlite?.data_dir) {
    merged.storage.sqlite.data_dir = path.resolve(merged.storage.sqlite.data_dir);
  }

  if (merged.auth.mode === 'oauth2' && !merged.auth.oauth2) {
    console.error('[bifrost-sync-server] auth.mode is "oauth2" but auth.oauth2 is not configured');
    process.exit(1);
  }

  return merged;
}
