#!/usr/bin/env node
import path from 'path';
import { startSyncServer } from './index';
import { loadConfig } from './config';
import type { SyncServerConfig } from './types';

const args = process.argv.slice(2);

function getArg(name: string, defaultValue: string): string {
  const idx = args.indexOf(name);
  if (idx !== -1 && idx + 1 < args.length) {
    return args[idx + 1];
  }
  return defaultValue;
}

function getArgOptional(name: string): string | undefined {
  const idx = args.indexOf(name);
  if (idx !== -1 && idx + 1 < args.length) {
    return args[idx + 1];
  }
  return undefined;
}

function hasFlag(name: string): boolean {
  return args.includes(name);
}

if (hasFlag('--help') || hasFlag('-h')) {
  console.log(`
bifrost-sync-server - Standalone Bifrost rule sync server

Usage:
  bifrost-sync-server [options]

Options:
  -c, --config <path>            Path to config file (yaml/json, default: config.yaml)
  -p, --port <port>              Port to listen on (default: 8686)
  -H, --host <host>              Host to bind to (default: 0.0.0.0)
  -d, --data-dir <dir>           Data directory for SQLite (default: ./bifrost-sync-data)
  --trust-forwarded-for          Trust X-Forwarded-For / X-Real-IP headers (DANGEROUS)
  -h, --help                     Show this help message

Config File:
  The server loads configuration from a YAML/JSON file. By default it looks
  for config.yaml, config.yml, or config.json in the current directory.
  Use --config to specify a custom path.

  CLI arguments override config file values.

Security:
  --trust-forwarded-for is a DANGEROUS option. Only enable it when the server
  is deployed behind a trusted reverse proxy (e.g., Nginx, Cloudflare).
  Without a trusted proxy, attackers can forge X-Forwarded-For headers to
  bypass IP-based rate limiting and brute-force protection.

Examples:
  # Start with defaults (SQLite, password auth)
  $ bifrost-sync-server

  # Start with a config file
  $ bifrost-sync-server -c /path/to/config.yaml

  # Override port from config file
  $ bifrost-sync-server -c config.yaml -p 9090

  # Deploy behind a reverse proxy
  $ bifrost-sync-server --trust-forwarded-for
`);
  process.exit(0);
}

const configPath = getArgOptional('-c') ?? getArgOptional('--config');

async function main() {
  const config: SyncServerConfig = loadConfig(configPath);

  const portOverride = getArgOptional('-p') ?? getArgOptional('--port');
  if (portOverride) config.server.port = parseInt(portOverride, 10);

  const hostOverride = getArgOptional('-H') ?? getArgOptional('--host');
  if (hostOverride) config.server.host = hostOverride;

  const dataDirOverride = getArgOptional('-d') ?? getArgOptional('--data-dir');
  if (dataDirOverride) {
    config.storage.type = 'sqlite';
    config.storage.sqlite = { data_dir: path.resolve(dataDirOverride) };
  }

  if (hasFlag('--trust-forwarded-for')) {
    config.server.trust_forwarded_for = true;
  }

  if (hasFlag('--enable-remote-invoke')) {
    config.remote_invoke = {
      enabled: true,
      sse_keepalive_ms: 30000,
      pair_code_ttl_secs: 120,
      max_active_calls_per_client: 5,
      retention_days: 90,
      max_records: 10000,
      max_sse_connections_per_client: 2,
      max_sse_connections_per_ip: 10,
      pair_rate_limit_per_ip: 5,
      pair_rate_limit_global_per_client: 10,
    };
  }

  console.log(`[bifrost-sync-server] starting...`);
  console.log(`  port:     ${config.server.port}`);
  console.log(`  host:     ${config.server.host}`);
  console.log(`  storage:  ${config.storage.type}`);
  console.log(`  auth:     ${config.auth.mode}`);
  console.log(`  remote-invoke: ${config.remote_invoke?.enabled ? 'enabled' : 'disabled'}`);
  if (config.server.trust_forwarded_for) {
    console.warn(`\x1b[33m  ⚠ trust-forwarded-for: ENABLED (DANGEROUS — only use behind a trusted reverse proxy)\x1b[0m`);
  }
  if (config.storage.type === 'sqlite') {
    console.log(`  data-dir: ${config.storage.sqlite?.data_dir}`);
  }
  if (config.storage.type === 'mysql' && config.storage.mysql) {
    console.log(`  mysql:    ${config.storage.mysql.host}:${config.storage.mysql.port}/${config.storage.mysql.database}`);
  }

  const instance = await startSyncServer(config);

  console.log(`[bifrost-sync-server] listening on http://${config.server.host}:${instance.port}`);

  if (config.auth.mode === 'password') {
    console.log(`[bifrost-sync-server] register a user via:`);
    console.log(
      `  curl -X POST http://localhost:${instance.port}/v4/sso/register \\`,
    );
    console.log(`    -H "Content-Type: application/json" \\`);
    console.log(`    -d '{"user_id": "your-username", "password": "your-password", "nickname": "Your Name"}'`);
  } else if (config.auth.mode === 'oauth2') {
    console.log(`[bifrost-sync-server] OAuth2 login: http://localhost:${instance.port}/v4/sso/login`);
    console.log(`[bifrost-sync-server] OAuth2 callback: http://localhost:${instance.port}/v4/sso/callback`);
  }

  const shutdown = () => {
    console.log('\n[bifrost-sync-server] shutting down...');
    instance.close().then(() => {
      console.log('[bifrost-sync-server] stopped');
      process.exit(0);
    });
  };

  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);
}

main().catch((err) => {
  console.error('[bifrost-sync-server] fatal:', err);
  process.exit(1);
});
