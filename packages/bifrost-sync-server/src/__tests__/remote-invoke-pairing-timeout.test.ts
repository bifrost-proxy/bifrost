import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import crypto from 'crypto';
import fs from 'fs';
import http from 'http';
import path from 'path';

import { createSyncServer, type SyncServerConfig, type SyncServerInstance } from '../index';
import { buildRegistrationSignaturePayload } from '../remote-invoke/types';

const TEST_DATA_DIR = path.join(__dirname, '.test-data-remote-invoke-pairing-timeout');
const TEST_CONFIG: SyncServerConfig = {
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
    pair_rate_limit_per_ip: 5,
    pair_rate_limit_global_per_client: 10,
  },
};

let server: SyncServerInstance;
let baseUrl: string;

async function bootServer() {
  server = createSyncServer(TEST_CONFIG);
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
}

function req(
  method: string,
  urlPath: string,
  body?: unknown,
  headers: Record<string, string> = {},
): Promise<{ status: number; data: { code: number; message: string; data?: any } }> {
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
          resolve({ status: res.statusCode!, data: JSON.parse(chunks) });
        } catch {
          resolve({
            status: res.statusCode!,
            data: { code: -1, message: chunks },
          });
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

function openSse(
  urlPath: string,
  headers: Record<string, string> = {},
): Promise<{ status: number; close: () => void }> {
  return new Promise((resolve, reject) => {
    const url = new URL(urlPath, baseUrl);
    const request = http.request({
      method: 'GET',
      hostname: url.hostname,
      port: url.port,
      path: `${url.pathname}${url.search}`,
      headers,
    });

    request.on('response', (res) => {
      res.setEncoding('utf8');
      resolve({
        status: res.statusCode ?? 0,
        close: () => {
          res.destroy();
          request.destroy();
        },
      });
    });

    request.on('error', reject);
    request.end();
  });
}

function generateClientKeypair() {
  return crypto.generateKeyPairSync('ed25519');
}

async function registerUser(userId: string, password: string): Promise<string> {
  const response = await req('POST', '/v4/sso/register', {
    user_id: userId,
    password,
  });
  expect(response.status).toBe(200);
  expect(response.data.code).toBe(0);
  return response.data.data.token as string;
}

async function requestRegistrationChallenge(clientInstanceId: string, token: string) {
  const response = await req(
    'POST',
    '/v4/remote-invoke/client/register/challenge',
    { client_instance_id: clientInstanceId },
    { 'x-bifrost-token': token },
  );
  expect(response.status).toBe(200);
  expect(response.data.code).toBe(0);
  return response.data.data as { challenge_id: string; challenge: string };
}

async function registerClient(
  clientInstanceId: string,
  publicKeyDerBase64: string,
  privateKey: crypto.KeyObject,
  token: string,
) {
  const challenge = await requestRegistrationChallenge(clientInstanceId, token);
  const timestamp = Math.floor(Date.now() / 1000);
  const payload = buildRegistrationSignaturePayload(
    challenge.challenge_id,
    challenge.challenge,
    clientInstanceId,
    'pairing-timeout-client',
    'macos',
    '0.0.0-test',
    publicKeyDerBase64,
    timestamp,
  );
  const signature = crypto.sign(null, Buffer.from(payload, 'utf8'), privateKey).toString('base64');

  const response = await req(
    'POST',
    '/v4/remote-invoke/client/register',
    {
      challenge_id: challenge.challenge_id,
      client_instance_id: clientInstanceId,
      client_long_term_pubkey: publicKeyDerBase64,
      device_name: 'pairing-timeout-client',
      platform: 'macos',
      bifrost_version: '0.0.0-test',
      signature,
      timestamp,
    },
    { 'x-bifrost-token': token },
  );
  expect(response.status).toBe(200);
  expect(response.data.code).toBe(0);
  return response.data.data.client_auth_token as string;
}

async function publishPairCode(
  clientInstanceId: string,
  clientAuthToken: string,
  pairCode: string,
  expiresAt: number,
) {
  const response = await req(
    'POST',
    '/v4/remote-invoke/client/pair-code',
    {
      client_instance_id: clientInstanceId,
      pair_code: pairCode,
      expires_at: expiresAt,
      discovery_session_id: `session-${pairCode}`,
    },
    { Authorization: `Bearer ${clientAuthToken}` },
  );
  expect(response.status).toBe(200);
  expect(response.data.code).toBe(0);
}

beforeAll(async () => {
  if (fs.existsSync(TEST_DATA_DIR)) {
    fs.rmSync(TEST_DATA_DIR, { recursive: true });
  }
  fs.mkdirSync(TEST_DATA_DIR, { recursive: true });
  await bootServer();
});

afterAll(async () => {
  await server.close();
  if (fs.existsSync(TEST_DATA_DIR)) {
    fs.rmSync(TEST_DATA_DIR, { recursive: true });
  }
}, 20_000);

describe('remote invoke pairing timeout cleanup', () => {
  it('removes expired pending pairings from slot occupancy and pending list', async () => {
    const token = await registerUser(`pairing-timeout-owner-${Date.now()}`, 'password123');
    const clientInstanceId = `pairing-timeout-client-${Date.now()}`;
    const clientKeys = generateClientKeypair();
    const clientPubkey = clientKeys.publicKey.export({ type: 'spki', format: 'der' }).toString('base64');
    const clientAuthToken = await registerClient(clientInstanceId, clientPubkey, clientKeys.privateKey, token);

    const stream = await openSse(
      `/v4/remote-invoke/client/stream?client_instance_id=${encodeURIComponent(clientInstanceId)}&stream_id=pairing-timeout-stream&client_auth_token=${encodeURIComponent(clientAuthToken)}`,
    );
    expect(stream.status).toBe(200);

    const pairCode = '654321';
    await publishPairCode(clientInstanceId, clientAuthToken, pairCode, Date.now() + 60_000);

    const expiredNow = new Date(Date.now() - 60_000).toISOString();
    await server.storage.remoteInvoke.createPairing({
      id: 'expired-pending-pairing',
      user_id: '',
      client_instance_id: clientInstanceId,
      caller_fingerprint: 'caller-expired',
      pair_code: '111111',
      status: 'pending_approval',
      caller_pubkey: '',
      caller_ephemeral_pub: 'expired-ephemeral',
      client_ephemeral_pub: '',
      caller_info_json: JSON.stringify({ fingerprint: 'caller-expired', display_name: 'expired-caller' }),
      command_summary_json: '{}',
      command_json: '{}',
      relay_token: '',
      call_id: '',
      grant_id: '',
      expires_at: expiredNow,
      create_time: expiredNow,
      update_time: expiredNow,
    });

    const startPairingResponse = await req('POST', '/v4/remote-invoke/pairings/start', {
      pair_code: pairCode,
      caller_info: {
        fingerprint: 'caller-fresh',
        display_name: 'fresh-caller',
      },
      caller_ephemeral_pub: 'fresh-ephemeral',
    });
    expect(startPairingResponse.status).toBe(200);
    expect(startPairingResponse.data.code).toBe(0);

    const pendingResponse = await req(
      'GET',
      `/v4/remote-invoke/client/pending-pairings?client_instance_id=${encodeURIComponent(clientInstanceId)}`,
      undefined,
      { Authorization: `Bearer ${clientAuthToken}` },
    );
    expect(pendingResponse.status).toBe(200);
    expect(pendingResponse.data.code).toBe(0);
    expect(pendingResponse.data.data).toHaveLength(1);
    expect(pendingResponse.data.data[0].pairing_id).toBe(startPairingResponse.data.data.pairing_id);

    const expiredPairing = await server.storage.remoteInvoke.getPairing('expired-pending-pairing');
    expect(expiredPairing?.status).toBe('expired');

    stream.close();
  });

  it('returns 410 for expired pairing decisions and persists expired status', async () => {
    const token = await registerUser(`pairing-decision-owner-${Date.now()}`, 'password123');
    const clientInstanceId = `pairing-decision-client-${Date.now()}`;
    const clientKeys = generateClientKeypair();
    const clientPubkey = clientKeys.publicKey.export({ type: 'spki', format: 'der' }).toString('base64');
    const clientAuthToken = await registerClient(clientInstanceId, clientPubkey, clientKeys.privateKey, token);

    const expiredNow = new Date(Date.now() - 60_000).toISOString();
    await server.storage.remoteInvoke.createPairing({
      id: 'expired-decision-pairing',
      user_id: '',
      client_instance_id: clientInstanceId,
      caller_fingerprint: 'caller-expired-decision',
      pair_code: '222222',
      status: 'pending_approval',
      caller_pubkey: '',
      caller_ephemeral_pub: 'expired-decision-ephemeral',
      client_ephemeral_pub: '',
      caller_info_json: JSON.stringify({ fingerprint: 'caller-expired-decision', display_name: 'expired-decision' }),
      command_summary_json: '{}',
      command_json: '{}',
      relay_token: '',
      call_id: '',
      grant_id: '',
      expires_at: expiredNow,
      create_time: expiredNow,
      update_time: expiredNow,
    });

    const response = await req(
      'POST',
      '/v4/remote-invoke/client/grants/expired-decision-pairing/decision',
      {
        decision: 'approve',
        client_instance_id: clientInstanceId,
        grant_mode: 'once',
        client_ephemeral_pub: 'client-ephemeral',
      },
      { Authorization: `Bearer ${clientAuthToken}` },
    );

    expect(response.status).toBe(410);
    expect(response.data.message).toBe('pairing_expired');

    const pairing = await server.storage.remoteInvoke.getPairing('expired-decision-pairing');
    expect(pairing?.status).toBe('expired');
  });
});
