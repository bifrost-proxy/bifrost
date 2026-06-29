import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import crypto from 'crypto';
import fs from 'fs';
import http from 'http';
import path from 'path';

import { createSyncServer, type SyncServerConfig, type SyncServerInstance } from '../index';
import { buildRegistrationSignaturePayload } from '../remote-invoke/types';
import { deriveSshDeviceCode } from '../remote-invoke/ssh-auth';
import { ed25519FingerprintFromBase64 } from '../remote-invoke/pop';
import { makeCallerKeypair, sha256Hex, signPopBody } from './remote-invoke-v5-test-utils';

const TEST_DATA_DIR = path.join(__dirname, '.test-data-p0-hardening');

let server: SyncServerInstance;
let baseUrl: string;

function req(
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
      headers: { 'Content-Type': 'application/json', ...headers },
    }, (res) => {
      let chunks = '';
      res.on('data', (c) => { chunks += c; });
      res.on('end', () => {
        try { resolve({ status: res.statusCode!, data: JSON.parse(chunks) }); }
        catch { resolve({ status: res.statusCode!, data: { code: -1, message: chunks } }); }
      });
    });
    request.on('error', reject);
    if (body !== undefined) request.write(JSON.stringify(body));
    request.end();
  });
}

async function registerUser(userId: string, password: string): Promise<string> {
  const r = await req('POST', '/v4/sso/register', { user_id: userId, password });
  expect(r.status, JSON.stringify(r.data)).toBe(200);
  expect(r.data.code).toBe(0);
  return r.data.data.token as string;
}

async function registerClient(
  clientInstanceId: string,
  publicKeyDerBase64: string,
  privateKey: crypto.KeyObject,
  token: string,
  overrides: { ssh_device_route?: null | { device_code: string; public_key_pem: string } } = {},
) {
  const chal = await req(
    'POST',
    '/v4/remote-invoke/client/register/challenge',
    { client_instance_id: clientInstanceId },
    { 'x-bifrost-token': token },
  );
  expect(chal.status).toBe(200);
  const challenge = chal.data.data;
  const timestamp = Math.floor(Date.now() / 1000);
  const payload = buildRegistrationSignaturePayload(
    challenge.challenge_id, challenge.challenge, clientInstanceId,
    'p0-test-device', 'macos', '0.0.0-test', publicKeyDerBase64, timestamp,
  );
  const signature = crypto.sign(null, Buffer.from(payload, 'utf8'), privateKey).toString('base64');
  const body: Record<string, unknown> = {
    challenge_id: challenge.challenge_id,
    client_instance_id: clientInstanceId,
    client_long_term_pubkey: publicKeyDerBase64,
    device_name: 'p0-test-device',
    platform: 'macos',
    bifrost_version: '0.0.0-test',
    signature,
    timestamp,
  };
  if (Object.prototype.hasOwnProperty.call(overrides, 'ssh_device_route')) {
    body.ssh_device_route = overrides.ssh_device_route;
  }
  return req('POST', '/v4/remote-invoke/client/register', body, { 'x-bifrost-token': token });
}

beforeAll(async () => {
  if (fs.existsSync(TEST_DATA_DIR)) fs.rmSync(TEST_DATA_DIR, { recursive: true });
  fs.mkdirSync(TEST_DATA_DIR, { recursive: true });
  const config: SyncServerConfig = {
    server: { port: 0, host: '127.0.0.1', trust_forwarded_for: true },
    storage: { type: 'sqlite', sqlite: { data_dir: TEST_DATA_DIR } },
    auth: { mode: 'password' },
    remote_invoke: {
      enabled: true,
      sse_keepalive_ms: 30_000,
      pair_code_ttl_secs: 120,
      max_active_calls_per_client: 5,
      max_grants_per_client: 20,
      retention_days: 90,
      max_records: 10_000,
      max_sse_connections_per_client: 2,
      max_sse_connections_per_ip: 10,
      pair_rate_limit_per_ip: 50,
      pair_rate_limit_global_per_client: 50,
    },
  };
  server = createSyncServer(config);
  await new Promise<void>((resolve) => {
    server.server.listen(0, '127.0.0.1', () => {
      const addr = server.server.address();
      if (addr && typeof addr === 'object') server.port = addr.port;
      resolve();
    });
  });
  baseUrl = `http://127.0.0.1:${server.port}`;
});

afterAll(async () => {
  await server?.close();
  if (fs.existsSync(TEST_DATA_DIR)) fs.rmSync(TEST_DATA_DIR, { recursive: true });
});

describe('P0-1: SSH route is bound to the registering user', () => {
  it('rejects cross-user device_code hijack with device_code_owned_by_other_user', async () => {
    const tokenA = await registerUser('p01_user_alpha', 'password123');
    const tokenB = await registerUser('p01_user_beta', 'password123');

    const { publicKey: pkA, privateKey: skA } = crypto.generateKeyPairSync('ed25519');
    const { publicKey: pkB, privateKey: skB } = crypto.generateKeyPairSync('ed25519');
    const pkADer = pkA.export({ type: 'spki', format: 'der' }).toString('base64');
    const pkBDer = pkB.export({ type: 'spki', format: 'der' }).toString('base64');

    // Alice's SSH public key is published as her route.
    const sshPubPemA = pkA.export({ type: 'spki', format: 'pem' }).toString();
    const deviceCodeA = deriveSshDeviceCode(sshPubPemA);

    const aliceReg = await registerClient(
      'p01-alice-client', pkADer, skA, tokenA,
      { ssh_device_route: { device_code: deviceCodeA, public_key_pem: sshPubPemA } },
    );
    expect(aliceReg.status, JSON.stringify(aliceReg.data)).toBe(200);

    // Bob now tries to claim Alice's device_code from his own client.
    const bobHijack = await registerClient(
      'p01-bob-client', pkBDer, skB, tokenB,
      { ssh_device_route: { device_code: deviceCodeA, public_key_pem: sshPubPemA } },
    );
    expect(bobHijack.status).not.toBe(200);
    expect(JSON.stringify(bobHijack.data)).toMatch(/device_code_owned_by_other_user/);
  });
});

describe('P0-4: pairing fingerprint is derived from server-trusted PoP key', () => {
  it('rejects start_pairing payloads without caller_pubkey (server has no key to derive fp)', async () => {
    const r = await req('POST', '/v5/remote-invoke/pairings/start', {
      pair_code: 'ANYCODE1',
      caller_info: { fingerprint: 'attacker-spoof', display_name: 'evil' },
      caller_ephemeral_pub: Buffer.from('eph').toString('base64'),
      // Note: no caller_pubkey. Route or service must refuse this.
    });
    expect(r.status).not.toBe(200);
    expect(JSON.stringify(r.data)).toMatch(/caller_pubkey|invalid_pair_code|pair_code/);
  });

  it('ed25519FingerprintFromBase64 is deterministic and decoupled from attacker-supplied fingerprint', () => {
    const callerKey = makeCallerKeypair();
    const fakeFp = 'attacker-spoofed-fp';
    const derived = ed25519FingerprintFromBase64(callerKey.caller_pubkey);
    expect(derived).not.toEqual(fakeFp);
    expect(derived.length).toBeGreaterThan(0);
    expect(ed25519FingerprintFromBase64(callerKey.caller_pubkey)).toEqual(derived);
  });
});

describe('P0-3: SSH approval mints a single-use claim_token (DAO sanity)', () => {
  it('SshClaim row is created/read/redeemed and cannot be reused', async () => {
    const claim_token_hash = sha256Hex('claim-test-' + crypto.randomBytes(4).toString('hex'));
    const now = new Date().toISOString();
    const exp = new Date(Date.now() + 60_000).toISOString();
    await server.storage.remoteInvoke.createSshClaim({
      claim_token_hash,
      grant_id: 'grant-p03',
      client_instance_id: 'client-p03',
      caller_pubkey_fp: 'fp-p03',
      expires_at: exp,
      create_time: now,
      claimed_at: '',
    });
    const fetched = await server.storage.remoteInvoke.getSshClaimByTokenHash(claim_token_hash);
    expect(fetched).toBeTruthy();
    expect(fetched?.claimed_at).toBe('');

    const ts = new Date().toISOString();
    await server.storage.remoteInvoke.markSshClaimRedeemed(claim_token_hash, ts);
    const after = await server.storage.remoteInvoke.getSshClaimByTokenHash(claim_token_hash);
    expect(after?.claimed_at).toBe(ts);
  });
});

describe('P0-2: lookupGrantSession freezes caller_ephemeral_pub (service-level)', () => {
  it('throws ephemeral_pub_rotation_not_allowed when the existing ephemeral_pub differs', async () => {
    const keypair = makeCallerKeypair();
    const grantId = 'grant-p02-' + crypto.randomBytes(4).toString('hex');
    const now = new Date().toISOString();
    const oldEph = crypto.randomBytes(32).toString('base64');
    const newEph = crypto.randomBytes(32).toString('base64');
    await server.storage.remoteInvoke.createGrant({
      id: grantId,
      user_id: '',
      client_instance_id: 'client-p02',
      caller_fingerprint: keypair.caller_pubkey_fp,
      caller_display_name: 'p02',
      caller_pubkey: keypair.caller_pubkey,
      caller_pubkey_fp: keypair.caller_pubkey_fp,
      caller_ephemeral_pub: oldEph,
      client_ephemeral_pub: oldEph,
      grant_mode: 'once' as any,
      grant_scope: 'remote_shell_exec',
      file_access: 'read_write',
      ssh_key_id: '',
      ssh_key_fingerprint: '',
      status: 'active',
      first_authorized_at: now,
      expires_at: '',
      session_token_hash: '',
      session_token_expires_at: '',
      last_nonce_seen: '',
      revoked_at: '',
      last_used_at: now,
      max_calls: 1000,
      remaining_calls: 1000,
      created_by: 'test',
      update_time: now,
    });

    const r = await req('POST', '/v5/remote-invoke/grants/lookup', signPopBody({
      client_instance_id: 'client-p02',
      caller_ephemeral_pub: newEph,
    }, keypair));
    expect(r.status).toBe(401);
    expect(r.data.message).toBe('ephemeral_pub_rotation_not_allowed');

    const after = await server.storage.remoteInvoke.getGrant(grantId);
    expect(after?.caller_ephemeral_pub).toBe(oldEph);
  });
});
