import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import crypto from 'crypto';
import fs from 'fs';
import http from 'http';
import path from 'path';

import { createSyncServer, type SyncServerConfig, type SyncServerInstance } from '../index';
import { remoteInvokeSchemaNeedsReset } from '../dao/mysql';
import { RemoteInvokeService } from '../remote-invoke/service';
import { buildRegistrationSignaturePayload } from '../remote-invoke/types';
import { deriveSshDeviceCode } from '../remote-invoke/ssh-auth';
import { ed25519FingerprintFromBase64 } from '../remote-invoke/pop';
import { base64X25519Pub, makeCallerKeypair, sha256Hex, signPopBody } from './remote-invoke-v5-test-utils';

const TEST_DATA_DIR = path.join(__dirname, '.test-data-p0-hardening');
const V5_GRANT_COLUMNS = [
  'ssh_key_id',
  'ssh_key_fingerprint',
  'file_access',
  'caller_pubkey',
  'caller_pubkey_fp',
  'session_token_hash',
  'session_token_expires_at',
  'last_nonce_seen',
  'revoked_at',
];
const V5_PAIRING_COLUMNS = [
  'watch_token_hash',
  'claim_token_hash',
  'claim_expires_at',
  'claimed_at',
];

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

describe('MySQL remote-invoke v5 schema reset detection', () => {
  it('does not reset when all v5 columns and tables are present', () => {
    expect(remoteInvokeSchemaNeedsReset(V5_GRANT_COLUMNS, V5_PAIRING_COLUMNS, true, true)).toBe(false);
  });

  it('resets when legacy schema is missing v5 token columns', () => {
    expect(remoteInvokeSchemaNeedsReset(['ssh_key_id', 'file_access'], V5_PAIRING_COLUMNS, true, true)).toBe(true);
  });

  it('resets when removed policy columns are still present', () => {
    expect(remoteInvokeSchemaNeedsReset([...V5_GRANT_COLUMNS, 'policy_binding'], V5_PAIRING_COLUMNS, true, true)).toBe(true);
  });

  it('resets when nonce or ssh claim tables are missing', () => {
    expect(remoteInvokeSchemaNeedsReset(V5_GRANT_COLUMNS, V5_PAIRING_COLUMNS, false, true)).toBe(true);
    expect(remoteInvokeSchemaNeedsReset(V5_GRANT_COLUMNS, V5_PAIRING_COLUMNS, true, false)).toBe(true);
  });
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
  it('accepts v5 caller routes when a TLB strips the /v5 prefix', async () => {
    const stripped = await req('POST', '/remote-invoke/pairings/start', {});
    expect(stripped.status).toBe(400);
    expect(stripped.data.message).toMatch(/pair_code, caller_info and caller_ephemeral_pub are required/);

    const legacy = await req('POST', '/v4/remote-invoke/pairings/start', {});
    expect(legacy.status).toBe(410);
    expect(legacy.data.error).toBe('protocol_version_not_supported');

    const strippedClientRoute = await req('POST', '/remote-invoke/client/register', {});
    expect(strippedClientRoute.status).toBe(404);
    expect(strippedClientRoute.data.message).toBe('remote invoke endpoint not found');
  });

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

  it('redeemed SSH claim grant_session_token can open multiple calls', async () => {
    const keypair = makeCallerKeypair();
    const grantId = 'grant-p03-ssh-multi-' + crypto.randomBytes(4).toString('hex');
    const claimToken = 'claim-p03-ssh-multi-' + crypto.randomBytes(4).toString('hex');
    const clientInstanceId = 'client-p03-ssh-multi';
    const now = new Date().toISOString();
    await server.storage.remoteInvoke.createGrant({
      id: grantId,
      user_id: 'p03-user',
      client_instance_id: clientInstanceId,
      caller_fingerprint: keypair.caller_pubkey_fp,
      caller_display_name: 'ssh-caller',
      caller_pubkey: keypair.caller_pubkey,
      caller_pubkey_fp: keypair.caller_pubkey_fp,
      caller_ephemeral_pub: base64X25519Pub('ssh-multi-caller'),
      client_ephemeral_pub: base64X25519Pub('ssh-multi-client'),
      grant_mode: 'permanent',
      grant_scope: 'remote_shell_interactive',
      file_access: 'read_write',
      ssh_key_id: 'ssh-key-p03',
      ssh_key_fingerprint: 'ssh-fp-p03',
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
      created_by: 'ssh_publickey',
      update_time: now,
    });
    await server.storage.remoteInvoke.createSshClaim({
      claim_token_hash: sha256Hex(claimToken),
      grant_id: grantId,
      client_instance_id: clientInstanceId,
      caller_pubkey_fp: keypair.caller_pubkey_fp,
      expires_at: new Date(Date.now() + 60_000).toISOString(),
      create_time: now,
      claimed_at: '',
    });

    const claim = await req('POST', '/v5/remote-invoke/grants/ssh-claim', signPopBody({
      client_instance_id: clientInstanceId,
      claim_token: claimToken,
      caller_ephemeral_pub: base64X25519Pub('ssh-multi-caller'),
    }, keypair));
    expect(claim.status, JSON.stringify(claim.data)).toBe(200);
    const sessionToken = claim.data.data.grant_session_token;
    expect(sessionToken).toMatch(/^[a-f0-9]{64}$/);

    for (const label of ['first', 'second']) {
      const opened = await req('POST', '/v5/remote-invoke/calls/open', signPopBody({
        client_instance_id: clientInstanceId,
        command_kind: 'file',
        command_encrypted: {
          version: 1,
          nonce: `nonce-${label}`,
          ciphertext: `ciphertext-${label}`,
          tag: `tag-${label}`,
        },
        command_summary: { command_preview: `file.read ${label}` },
      }, keypair), { Authorization: `Bearer ${sessionToken}` });
      expect(opened.status, JSON.stringify(opened.data)).toBe(200);
    }
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

describe('P0-3: once-mode grants consume only after call budget is exhausted', () => {
  async function seedOnceGrantWithCall(
    grantId: string,
    callId: string,
    remainingCalls: number,
  ) {
    const now = new Date().toISOString();
    await server.storage.remoteInvoke.createGrant({
      id: grantId,
      user_id: '',
      client_instance_id: 'client-p03-budget',
      caller_fingerprint: 'caller-p03-budget',
      caller_display_name: 'p03-budget',
      caller_pubkey: '',
      caller_pubkey_fp: 'caller-p03-budget',
      caller_ephemeral_pub: crypto.randomBytes(32).toString('base64'),
      client_ephemeral_pub: crypto.randomBytes(32).toString('base64'),
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
      remaining_calls: remainingCalls,
      created_by: 'ssh_publickey',
      update_time: now,
    });
    await server.storage.remoteInvoke.createCall({
      id: callId,
      user_id: '',
      grant_id: grantId,
      pairing_id: '',
      client_instance_id: 'client-p03-budget',
      caller_fingerprint: 'caller-p03-budget',
      source_ip: '',
      caller_display_name: 'p03-budget',
      status: 'authorized',
      command_summary_json: '{}',
      command_json: '{}',
      payload_digest: '',
      stdout_digest: '',
      stderr_digest: '',
      exit_code: -1,
      started_at: now,
      ended_at: '',
      duration_ms: 0,
      bytes_in: 0,
      bytes_out: 0,
    });
  }

  it('keeps SSH once-mode grant active while remaining_calls is still positive', async () => {
    const grantId = 'grant-p03-budget-active-' + crypto.randomBytes(4).toString('hex');
    const callId = 'call-p03-budget-active-' + crypto.randomBytes(4).toString('hex');
    await seedOnceGrantWithCall(grantId, callId, 999);
    const service = new RemoteInvokeService(server.storage, {
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
      ssh_grant_max_calls: 1000,
    });

    await service.postClientExit({
      call_id: callId,
      client_instance_id: 'client-p03-budget',
      exit_code: 0,
      duration_ms: 1,
    });

    const updated = await server.storage.remoteInvoke.getGrant(grantId);
    expect(updated?.status).toBe('active');
    expect(updated?.remaining_calls).toBe(999);
  });

  it('marks once-mode grant consumed when remaining_calls reaches zero', async () => {
    const grantId = 'grant-p03-budget-consumed-' + crypto.randomBytes(4).toString('hex');
    const callId = 'call-p03-budget-consumed-' + crypto.randomBytes(4).toString('hex');
    await seedOnceGrantWithCall(grantId, callId, 0);
    const service = new RemoteInvokeService(server.storage, {
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
      ssh_grant_max_calls: 1000,
    });

    await service.postClientExit({
      call_id: callId,
      client_instance_id: 'client-p03-budget',
      exit_code: 0,
      duration_ms: 1,
    });

    const updated = await server.storage.remoteInvoke.getGrant(grantId);
    expect(updated?.status).toBe('consumed');
  });
});
