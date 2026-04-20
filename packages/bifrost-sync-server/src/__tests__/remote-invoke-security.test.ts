import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import crypto from 'crypto';
import fs from 'fs';
import http from 'http';
import path from 'path';

import { createSyncServer, type SyncServerConfig, type SyncServerInstance } from '../index';
import { buildRegistrationSignaturePayload } from '../remote-invoke/types';

const TEST_DATA_DIR = path.join(__dirname, '.test-data-remote-invoke-security');

let server: SyncServerInstance;
let baseUrl: string;

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

async function registerUser(userId: string, password: string): Promise<string> {
  const response = await req('POST', '/v4/sso/register', {
    user_id: userId,
    password,
  });
  expect(response.status).toBe(200);
  expect(response.data.code).toBe(0);
  return response.data.data.token as string;
}

function generateClientKeypair() {
  return crypto.generateKeyPairSync('ed25519');
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
  return response.data.data as {
    challenge_id: string;
    challenge: string;
    expires_at: number;
    algorithm: string;
  };
}

async function registerClient(
  clientInstanceId: string,
  publicKeyDerBase64: string,
  privateKey: crypto.KeyObject,
  token: string,
  overrides: Partial<{
    device_name: string;
    platform: string;
    bifrost_version: string;
    signature: string;
    timestamp: number;
    challenge_id: string;
    challenge: string;
  }> = {},
) {
  const challenge = await requestRegistrationChallenge(clientInstanceId, token);
  const timestamp = overrides.timestamp ?? Math.floor(Date.now() / 1000);
  const deviceName = overrides.device_name ?? 'security-test-device';
  const platform = overrides.platform ?? 'macos';
  const bifrostVersion = overrides.bifrost_version ?? '0.0.0-test';
  const signaturePayload = buildRegistrationSignaturePayload(
    overrides.challenge_id ?? challenge.challenge_id,
    overrides.challenge ?? challenge.challenge,
    clientInstanceId,
    deviceName,
    platform,
    bifrostVersion,
    publicKeyDerBase64,
    timestamp,
  );
  const signature = overrides.signature ?? crypto.sign(
    null,
    Buffer.from(signaturePayload, 'utf8'),
    privateKey,
  ).toString('base64');

  return req(
    'POST',
    '/v4/remote-invoke/client/register',
    {
      challenge_id: challenge.challenge_id,
      client_instance_id: clientInstanceId,
      client_long_term_pubkey: publicKeyDerBase64,
      device_name: deviceName,
      platform,
      bifrost_version: bifrostVersion,
      signature,
      timestamp,
    },
    { 'x-bifrost-token': token },
  );
}

beforeAll(async () => {
  if (fs.existsSync(TEST_DATA_DIR)) {
    fs.rmSync(TEST_DATA_DIR, { recursive: true });
  }
  fs.mkdirSync(TEST_DATA_DIR, { recursive: true });

  const config: SyncServerConfig = {
    server: { port: 0, host: '127.0.0.1' },
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
    fs.rmSync(TEST_DATA_DIR, { recursive: true });
  }
});

describe('Remote Invoke security', () => {
  it('requires x-bifrost-token for client registration challenge and register', async () => {
    const challengeResponse = await req(
      'POST',
      '/v4/remote-invoke/client/register/challenge',
      { client_instance_id: 'client-missing-token' },
    );
    expect(challengeResponse.status).toBe(401);

    const registerResponse = await req(
      'POST',
      '/v4/remote-invoke/client/register',
      {
        challenge_id: 'missing',
        client_instance_id: 'client-missing-token',
        client_long_term_pubkey: 'Zm9v',
        device_name: 'device',
        platform: 'macos',
        bifrost_version: '0.0.0-test',
        signature: 'YmFy',
        timestamp: Math.floor(Date.now() / 1000),
      },
    );
    expect(registerResponse.status).toBe(401);
  });

  it('requires a valid private-key signature and rejects challenge replay', async () => {
    const token = await registerUser('ri_security_user', 'password123');
    const { publicKey, privateKey } = generateClientKeypair();
    const publicKeyDerBase64 = publicKey.export({ type: 'spki', format: 'der' }).toString('base64');

    const success = await registerClient(
      'ri-client-security-1',
      publicKeyDerBase64,
      privateKey,
      token,
    );
    expect(success.status).toBe(200);
    expect(success.data.code).toBe(0);
    expect(success.data.data.client_auth_token).toBeTruthy();

    const replayChallenge = await requestRegistrationChallenge('ri-client-security-1-replay', token);
    const replayTimestamp = Math.floor(Date.now() / 1000);
    const replayPayload = buildRegistrationSignaturePayload(
      replayChallenge.challenge_id,
      replayChallenge.challenge,
      'ri-client-security-1-replay',
      'security-test-device',
      'macos',
      '0.0.0-test',
      publicKeyDerBase64,
      replayTimestamp,
    );
    const replaySignature = crypto.sign(
      null,
      Buffer.from(replayPayload, 'utf8'),
      privateKey,
    ).toString('base64');

    const firstUse = await req(
      'POST',
      '/v4/remote-invoke/client/register',
      {
        challenge_id: replayChallenge.challenge_id,
        client_instance_id: 'ri-client-security-1-replay',
        client_long_term_pubkey: publicKeyDerBase64,
        device_name: 'security-test-device',
        platform: 'macos',
        bifrost_version: '0.0.0-test',
        signature: replaySignature,
        timestamp: replayTimestamp,
      },
      { 'x-bifrost-token': token },
    );
    expect(firstUse.status).toBe(200);

    const replayUse = await req(
      'POST',
      '/v4/remote-invoke/client/register',
      {
        challenge_id: replayChallenge.challenge_id,
        client_instance_id: 'ri-client-security-1-replay',
        client_long_term_pubkey: publicKeyDerBase64,
        device_name: 'security-test-device',
        platform: 'macos',
        bifrost_version: '0.0.0-test',
        signature: replaySignature,
        timestamp: replayTimestamp,
      },
      { 'x-bifrost-token': token },
    );
    expect(replayUse.status).toBe(400);
    expect(replayUse.data.message).toBe('registration_challenge_not_found');

    const invalidSignature = await registerClient(
      'ri-client-security-2',
      publicKeyDerBase64,
      privateKey,
      token,
      { signature: Buffer.from('forged-signature').toString('base64') },
    );
    expect(invalidSignature.status).toBe(401);
    expect(invalidSignature.data.message).toBe('invalid_registration_signature');
  });

  it('scopes client call detail lookups to the authenticated client', async () => {
    const token = await registerUser('ri_scope_user', 'password123');

    const clientAKeys = generateClientKeypair();
    const clientAPubkey = clientAKeys.publicKey.export({ type: 'spki', format: 'der' }).toString('base64');
    const clientARegistration = await registerClient(
      'ri-client-a',
      clientAPubkey,
      clientAKeys.privateKey,
      token,
      { device_name: 'client-a' },
    );
    const clientAToken = clientARegistration.data.data.client_auth_token as string;

    const clientBKeys = generateClientKeypair();
    const clientBPubkey = clientBKeys.publicKey.export({ type: 'spki', format: 'der' }).toString('base64');
    const clientBRegistration = await registerClient(
      'ri-client-b',
      clientBPubkey,
      clientBKeys.privateKey,
      token,
      { device_name: 'client-b' },
    );
    const clientBToken = clientBRegistration.data.data.client_auth_token as string;

    const now = new Date().toISOString();
    await server.storage.remoteInvoke.createCall({
      id: 'ri-call-owned-by-a',
      user_id: '',
      grant_id: 'grant-1',
      pairing_id: '',
      client_instance_id: 'ri-client-a',
      caller_fingerprint: 'caller-fingerprint-a',
      source_ip: '127.0.0.1',
      caller_display_name: 'caller-a',
      status: 'completed',
      command_summary_json: JSON.stringify({ command_preview: 'status' }),
      command_json: JSON.stringify({ command: 'status' }),
      payload_digest: '',
      stdout_digest: '',
      stderr_digest: '',
      exit_code: 0,
      started_at: now,
      ended_at: now,
      duration_ms: 1,
      bytes_in: 0,
      bytes_out: 0,
    });

    const unauthorizedLookup = await req(
      'GET',
      '/v4/remote-invoke/client/calls/ri-call-owned-by-a?client_instance_id=ri-client-b',
      undefined,
      { Authorization: `Bearer ${clientBToken}` },
    );
    expect(unauthorizedLookup.status).toBe(404);

    const authorizedLookup = await req(
      'GET',
      '/v4/remote-invoke/client/calls/ri-call-owned-by-a?client_instance_id=ri-client-a',
      undefined,
      { Authorization: `Bearer ${clientAToken}` },
    );
    expect(authorizedLookup.status).toBe(200);
    expect(authorizedLookup.data.data.call_id).toBe('ri-call-owned-by-a');
  });

  it('rejects pairing decisions and call updates from a different authenticated client', async () => {
    const token = await registerUser('ri_owner_guard_user', 'password123');

    const clientAKeys = generateClientKeypair();
    const clientAPubkey = clientAKeys.publicKey.export({ type: 'spki', format: 'der' }).toString('base64');
    const clientARegistration = await registerClient(
      'ri-owner-client-a',
      clientAPubkey,
      clientAKeys.privateKey,
      token,
      { device_name: 'owner-client-a' },
    );
    const clientAToken = clientARegistration.data.data.client_auth_token as string;

    const clientBKeys = generateClientKeypair();
    const clientBPubkey = clientBKeys.publicKey.export({ type: 'spki', format: 'der' }).toString('base64');
    const clientBRegistration = await registerClient(
      'ri-owner-client-b',
      clientBPubkey,
      clientBKeys.privateKey,
      token,
      { device_name: 'owner-client-b' },
    );
    const clientBToken = clientBRegistration.data.data.client_auth_token as string;

    const now = new Date().toISOString();
    await server.storage.remoteInvoke.createPairing({
      id: 'ri-pairing-owned-by-a',
      user_id: '',
      client_instance_id: 'ri-owner-client-a',
      caller_fingerprint: 'caller-fingerprint-a',
      pair_code: '123456',
      status: 'pending_approval',
      caller_pubkey: '',
      client_ephemeral_pub: '',
      caller_info_json: JSON.stringify({ fingerprint: 'caller-fingerprint-a', display_name: 'caller-a' }),
      command_summary_json: '{}',
      command_json: '{}',
      relay_token: '',
      call_id: '',
      grant_id: '',
      expires_at: new Date(Date.now() + 60_000).toISOString(),
      create_time: now,
      update_time: now,
    });

    const crossClientDecision = await req(
      'POST',
      '/v4/remote-invoke/client/grants/ri-pairing-owned-by-a/decision',
      { decision: 'approve', grant_mode: 'once', client_instance_id: 'ri-owner-client-b' },
      { Authorization: `Bearer ${clientBToken}` },
    );
    expect(crossClientDecision.status).toBe(403);
    expect(crossClientDecision.data.message).toBe('client_mismatch');

    await server.storage.remoteInvoke.createCall({
      id: 'ri-call-owned-by-owner-a',
      user_id: '',
      grant_id: 'grant-owner-a',
      pairing_id: '',
      client_instance_id: 'ri-owner-client-a',
      caller_fingerprint: 'caller-fingerprint-a',
      source_ip: '127.0.0.1',
      caller_display_name: 'caller-a',
      status: 'authorized',
      command_summary_json: JSON.stringify({ command_preview: 'status' }),
      command_json: JSON.stringify({ command: 'status' }),
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

    const crossClientFrame = await req(
      'POST',
      '/v4/remote-invoke/client/calls/ri-call-owned-by-owner-a/frame',
      { client_instance_id: 'ri-owner-client-b', envelope_json: '{"kind":"stdout"}' },
      { Authorization: `Bearer ${clientBToken}` },
    );
    expect(crossClientFrame.status).toBe(403);
    expect(crossClientFrame.data.message).toBe('client_mismatch');

    const crossClientExit = await req(
      'POST',
      '/v4/remote-invoke/client/calls/ri-call-owned-by-owner-a/exit',
      { client_instance_id: 'ri-owner-client-b', exit_code: 0 },
      { Authorization: `Bearer ${clientBToken}` },
    );
    expect(crossClientExit.status).toBe(403);
    expect(crossClientExit.data.message).toBe('client_mismatch');

    const ownerFrame = await req(
      'POST',
      '/v4/remote-invoke/client/calls/ri-call-owned-by-owner-a/frame',
      { client_instance_id: 'ri-owner-client-a', envelope_json: '{"kind":"stdout"}' },
      { Authorization: `Bearer ${clientAToken}` },
    );
    expect(ownerFrame.status).toBe(200);
  });
});
