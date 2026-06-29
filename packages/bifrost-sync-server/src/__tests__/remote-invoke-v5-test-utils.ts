import crypto from 'crypto';
import fs from 'fs';
import http from 'http';
import path from 'path';
import { expect } from 'vitest';

import { createSyncServer, type SyncServerConfig, type SyncServerInstance } from '../index';
import type { GrantMode, RemoteInvokeGrant, RemoteInvokePairing } from '../types';
import { canonicalJson, ed25519FingerprintFromBase64 } from '../remote-invoke/pop';

export const REMOTE_INVOKE_TEST_CONFIG = (dataDir: string): SyncServerConfig => ({
  server: { port: 0, host: '127.0.0.1', trust_forwarded_for: true },
  storage: { type: 'sqlite', sqlite: { data_dir: dataDir } },
  auth: { mode: 'password' },
  remote_invoke: {
    enabled: true,
    sse_keepalive_ms: 30_000,
    pair_code_ttl_secs: 120,
    max_active_calls_per_client: 5,
    max_grants_per_client: 20,
    retention_days: 90,
    max_records: 10_000,
    max_sse_connections_per_client: 4,
    max_sse_connections_per_ip: 10,
    pair_rate_limit_per_ip: 20,
    pair_rate_limit_global_per_client: 20,
  },
});

export interface TestServer {
  server: SyncServerInstance;
  baseUrl: string;
  request: (
    method: string,
    urlPath: string,
    body?: unknown,
    headers?: Record<string, string>,
  ) => Promise<{ status: number; data: { code?: number; message?: string; error?: string; data?: any } }>;
  close: () => Promise<void>;
}

export async function bootRemoteInvokeServer(testName: string): Promise<TestServer> {
  const dataDir = path.join(__dirname, `.test-data-${testName}`);
  fs.rmSync(dataDir, { recursive: true, force: true });
  const server = createSyncServer(REMOTE_INVOKE_TEST_CONFIG(dataDir));
  await new Promise<void>((resolve) => {
    server.server.listen(0, '127.0.0.1', () => {
      const addr = server.server.address();
      if (addr && typeof addr === 'object') {
        server.port = addr.port;
      }
      resolve();
    });
  });
  const baseUrl = `http://127.0.0.1:${server.port}`;
  return {
    server,
    baseUrl,
    request: (method, urlPath, body, headers = {}) => requestJson(baseUrl, method, urlPath, body, headers),
    close: async () => {
      await server.close();
      fs.rmSync(dataDir, { recursive: true, force: true });
    },
  };
}

function requestJson(
  baseUrl: string,
  method: string,
  urlPath: string,
  body?: unknown,
  headers: Record<string, string> = {},
): Promise<{ status: number; data: { code?: number; message?: string; error?: string; data?: any } }> {
  return new Promise((resolve, reject) => {
    const url = new URL(urlPath, baseUrl);
    const request = http.request({
      method,
      hostname: url.hostname,
      port: url.port,
      path: `${url.pathname}${url.search}`,
      headers: {
        'Content-Type': 'application/json',
        ...headers,
      },
    }, (res) => {
      let chunks = '';
      res.on('data', (chunk) => {
        chunks += chunk;
      });
      res.on('end', () => {
        try {
          resolve({ status: res.statusCode ?? 0, data: JSON.parse(chunks) });
        } catch {
          resolve({ status: res.statusCode ?? 0, data: { message: chunks } });
        }
      });
    });
    request.on('error', reject);
    if (body !== undefined) {
      request.write(JSON.stringify(body));
    }
    request.end();
  });
}

export function makeCallerKeypair() {
  const pair = crypto.generateKeyPairSync('ed25519');
  const caller_pubkey = pair.publicKey.export({ format: 'der', type: 'spki' }).toString('base64');
  return {
    ...pair,
    caller_pubkey,
    caller_pubkey_fp: ed25519FingerprintFromBase64(caller_pubkey),
  };
}

export function signPopBody(
  body: Record<string, unknown>,
  keypair: ReturnType<typeof makeCallerKeypair>,
): Record<string, unknown> {
  const envelope = {
    ts: Date.now(),
    nonce: crypto.randomBytes(16).toString('hex'),
    caller_pubkey: keypair.caller_pubkey,
    ...body,
  };
  const signature = crypto.sign(null, Buffer.from(canonicalJson(envelope), 'utf8'), keypair.privateKey).toString('base64');
  return {
    ...envelope,
    signature,
  };
}

export function base64X25519Pub(seed = ''): string {
  return crypto.createHash('sha256').update(`x25519:${seed}:${crypto.randomBytes(8).toString('hex')}`).digest().toString('base64');
}

export function sha256Hex(value: string): string {
  return crypto.createHash('sha256').update(value, 'utf8').digest('hex');
}

export async function seedActiveGrant(
  instance: SyncServerInstance,
  fields: Partial<RemoteInvokeGrant> = {},
): Promise<RemoteInvokeGrant> {
  const now = new Date().toISOString();
  const grant: RemoteInvokeGrant = {
    id: fields.id ?? `grant-${crypto.randomBytes(4).toString('hex')}`,
    user_id: fields.user_id ?? '',
    client_instance_id: fields.client_instance_id ?? 'client-v5',
    caller_fingerprint: fields.caller_fingerprint ?? fields.caller_pubkey_fp ?? 'caller-fp',
    caller_display_name: fields.caller_display_name ?? 'Caller',
    caller_pubkey: fields.caller_pubkey ?? '',
    caller_pubkey_fp: fields.caller_pubkey_fp ?? '',
    caller_ephemeral_pub: fields.caller_ephemeral_pub ?? base64X25519Pub('caller-existing'),
    client_ephemeral_pub: fields.client_ephemeral_pub ?? base64X25519Pub('client-existing'),
    grant_mode: fields.grant_mode ?? ('reusable' as GrantMode),
    grant_scope: fields.grant_scope ?? 'remote_shell_exec',
    file_access: fields.file_access ?? 'read_write',
    ssh_key_id: fields.ssh_key_id ?? '',
    ssh_key_fingerprint: fields.ssh_key_fingerprint ?? '',
    status: fields.status ?? 'active',
    first_authorized_at: fields.first_authorized_at ?? now,
    expires_at: fields.expires_at ?? '',
    session_token_hash: fields.session_token_hash ?? '',
    session_token_expires_at: fields.session_token_expires_at ?? '',
    last_nonce_seen: fields.last_nonce_seen ?? '',
    revoked_at: fields.revoked_at ?? '',
    last_used_at: fields.last_used_at ?? now,
    max_calls: fields.max_calls ?? 999999,
    remaining_calls: fields.remaining_calls ?? 999999,
    created_by: fields.created_by ?? 'test',
    update_time: fields.update_time ?? now,
  };
  await instance.storage.remoteInvoke.createGrant(grant);
  return grant;
}

export async function seedApprovedPairingWithGrant(
  instance: SyncServerInstance,
  fields: {
    claimToken: string;
    grantId?: string;
    pairCode?: string;
    clientInstanceId?: string;
    callerPubkey?: string;
    callerPubkeyFp?: string;
    clientEphemeralPub?: string;
  },
): Promise<{ pairing: RemoteInvokePairing; grant: RemoteInvokeGrant }> {
  const now = new Date().toISOString();
  const grant = await seedActiveGrant(instance, {
    id: fields.grantId ?? `grant-${crypto.randomBytes(4).toString('hex')}`,
    client_instance_id: fields.clientInstanceId ?? 'client-v5',
    caller_pubkey_fp: fields.callerPubkeyFp ?? '',
    client_ephemeral_pub: fields.clientEphemeralPub ?? base64X25519Pub('client-claim'),
  });
  const pairing: RemoteInvokePairing = {
    id: `pairing-${crypto.randomBytes(4).toString('hex')}`,
    user_id: '',
    client_instance_id: grant.client_instance_id,
    caller_fingerprint: 'caller-fp',
    pair_code: fields.pairCode ?? 'PAIR123',
    status: 'approved',
    caller_pubkey: fields.callerPubkey ?? '',
    caller_ephemeral_pub: '',
    client_ephemeral_pub: grant.client_ephemeral_pub,
    caller_info_json: '{}',
    command_summary_json: '{}',
    command_json: '{}',
    relay_token: '',
    call_id: '',
    grant_id: grant.id,
    watch_token_hash: '',
    claim_token_hash: sha256Hex(fields.claimToken),
    claim_expires_at: new Date(Date.now() + 60_000).toISOString(),
    claimed_at: '',
    expires_at: new Date(Date.now() + 120_000).toISOString(),
    create_time: now,
    update_time: now,
  };
  await instance.storage.remoteInvoke.createPairing(pairing);
  return { pairing, grant };
}

export function expectOk(response: { status: number; data: { code?: number } }) {
  expect(response.status).toBe(200);
  expect(response.data.code).toBe(0);
}
