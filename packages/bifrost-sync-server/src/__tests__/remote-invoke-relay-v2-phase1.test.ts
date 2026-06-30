import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import crypto from 'crypto';
import fs from 'fs';
import http from 'http';
import path from 'path';

import { createSyncServer, type SyncServerConfig, type SyncServerInstance } from '../index';
import { buildRegistrationSignaturePayload, grantScopeAllowsCommand, normalizeGrantScope, resolveCommandKind } from '../remote-invoke/types';
import { base64X25519Pub, makeCallerKeypair, seedActiveGrant, sha256Hex, signPopBody } from './remote-invoke-v5-test-utils';

const TEST_DATA_DIR = path.join(__dirname, '.test-data-remote-invoke-relay-v2-phase1');
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

async function restartServer() {
  await server.close();
  await bootServer();
}

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

function openSseWithEvents(
  urlPath: string,
  headers: Record<string, string> = {},
): Promise<{
  status: number;
  nextEvent: (eventName: string, timeoutMs?: number) => Promise<unknown>;
  close: () => void;
}> {
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
      const pending = new Map<string, Array<(data: unknown) => void>>();
      const bufferedEvents = new Map<string, unknown[]>();
      let buffer = '';
      let closed = false;

      const emit = (eventName: string, data: unknown) => {
        const queue = pending.get(eventName);
        const resolver = queue?.shift();
        if (resolver) {
          resolver(data);
          return;
        }
        const buffered = bufferedEvents.get(eventName) ?? [];
        buffered.push(data);
        bufferedEvents.set(eventName, buffered);
      };

      res.setEncoding('utf8');
      res.on('data', (chunk) => {
        buffer += chunk;
        while (buffer.includes('\n\n')) {
          const boundary = buffer.indexOf('\n\n');
          const rawEvent = buffer.slice(0, boundary);
          buffer = buffer.slice(boundary + 2);

          let eventName = 'message';
          const dataLines: string[] = [];
          for (const line of rawEvent.split('\n')) {
            if (line.startsWith('event:')) {
              eventName = line.slice(6).trim();
            } else if (line.startsWith('data:')) {
              dataLines.push(line.slice(5).trim());
            }
          }

          if (!dataLines.length) continue;
          const rawData = dataLines.join('\n');
          let parsed: unknown = rawData;
          try {
            parsed = JSON.parse(rawData);
          } catch {
            parsed = rawData;
          }
          emit(eventName, parsed);
        }
      });

      resolve({
        status: res.statusCode ?? 0,
        nextEvent: (eventName: string, timeoutMs = 2_000) => new Promise((resolveEvent, rejectEvent) => {
          if (closed) {
            rejectEvent(new Error('sse_closed'));
            return;
          }
          const buffered = bufferedEvents.get(eventName) ?? [];
          if (buffered.length > 0) {
            const data = buffered.shift();
            bufferedEvents.set(eventName, buffered);
            resolveEvent(data);
            return;
          }
          const timer = setTimeout(() => {
            const queue = pending.get(eventName) ?? [];
            pending.set(eventName, queue.filter((entry) => entry !== wrappedResolve));
            rejectEvent(new Error(`timeout waiting for ${eventName}`));
          }, timeoutMs);
          const wrappedResolve = (data: unknown) => {
            clearTimeout(timer);
            resolveEvent(data);
          };
          const queue = pending.get(eventName) ?? [];
          queue.push(wrappedResolve);
          pending.set(eventName, queue);
        }),
        close: () => {
          closed = true;
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
  expect(response.status, JSON.stringify(response.data)).toBe(200);
  expect(response.data.code, JSON.stringify(response.data)).toBe(0);
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
  return response.data.data as {
    challenge_id: string;
    challenge: string;
  };
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
    'phase1-client',
    'macos',
    '0.0.0-test',
    publicKeyDerBase64,
    timestamp,
  );
  const signature = crypto.sign(
    null,
    Buffer.from(payload, 'utf8'),
    privateKey,
  ).toString('base64');

  const response = await req(
    'POST',
    '/v4/remote-invoke/client/register',
    {
      challenge_id: challenge.challenge_id,
      client_instance_id: clientInstanceId,
      client_long_term_pubkey: publicKeyDerBase64,
      device_name: 'phase1-client',
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

function createGrantSession(sessionToken: string): { session_token_hash: string; session_token_expires_at: string } {
  return {
    session_token_hash: sha256Hex(sessionToken),
    session_token_expires_at: new Date(Date.now() + 60_000).toISOString(),
  };
}

function postOpenCallV5(
  sessionToken: string,
  callerKeys: ReturnType<typeof makeCallerKeypair>,
  body: Record<string, unknown>,
) {
  return req(
    'POST',
    '/v5/remote-invoke/calls/open',
    signPopBody(body, callerKeys),
    { Authorization: `Bearer ${sessionToken}` },
  );
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
});

describe('remote invoke relay v2 phase 1', () => {
  it('enforces encrypted-only helper-level scope and command-kind rules', () => {
    expect(normalizeGrantScope('remote_query')).toBe('remote_query');
    expect(() => normalizeGrantScope('query')).toThrow('invalid_grant_scope');
    expect(resolveCommandKind('shell.exec')).toBe('shell.exec');
    expect(() => resolveCommandKind(undefined)).toThrow('command_kind_required');
    expect(grantScopeAllowsCommand('remote_query', 'shell.exec')).toBe(false);
    expect(grantScopeAllowsCommand('remote_shell_exec', 'shell.exec')).toBe(true);
    expect(grantScopeAllowsCommand('remote_shell_exec', 'power.mgmt')).toBe(false);
    expect(grantScopeAllowsCommand('remote_power_mgmt', 'power.mgmt')).toBe(true);
    expect(grantScopeAllowsCommand('remote_shell_interactive', 'power.mgmt')).toBe(true);
  });

  it('does not register legacy plaintext openCall routes', async () => {
    const now = new Date().toISOString();
    await server.storage.remoteInvoke.createGrant({
      id: 'ri-plaintext-grant',
      user_id: 'ri_phase1_owner',
      client_instance_id: 'ri-phase1-client',
      caller_fingerprint: 'caller-phase1-plaintext',
      caller_display_name: 'caller-phase1-plaintext',
      grant_mode: 'permanent',
      grant_scope: 'remote_query',
      status: 'active',
      first_authorized_at: now,
      expires_at: '',
      last_used_at: now,
      max_calls: 999999,
      remaining_calls: 999999,
      created_by: 'test',
      update_time: now,
    });

    const response = await req('POST', '/v4/remote-invoke/calls/open', {
      grant_id: 'ri-plaintext-grant',
      client_instance_id: 'ri-phase1-client',
      caller_fingerprint: 'caller-phase1-plaintext',
      command_kind: 'query.readonly',
      command: { command: 'status' },
      command_summary: { command_preview: 'status' },
    });

    expect(response.status).toBe(404);
    expect(response.data.message).toBe('remote invoke endpoint not found');
  });

  it('persists only route-level metadata for shell.exec encrypted openCall', async () => {
    const token = await registerUser('ri_phase1_owner', 'password123');
    const clientKeys = generateClientKeypair();
    const clientPubkey = clientKeys.publicKey.export({ type: 'spki', format: 'der' }).toString('base64');
    const clientAuthToken = await registerClient('ri-phase1-client', clientPubkey, clientKeys.privateKey, token);
    const now = new Date().toISOString();
    const callerKeys = makeCallerKeypair();
    const sessionToken = 'ri-phase1-shell-session';

    await server.storage.remoteInvoke.createGrant({
      id: 'ri-phase1-shell-grant',
      user_id: 'ri_phase1_owner',
      client_instance_id: 'ri-phase1-client',
      caller_fingerprint: 'phase1-caller',
      caller_display_name: 'phase1-caller',
      caller_pubkey: callerKeys.caller_pubkey,
      caller_pubkey_fp: callerKeys.caller_pubkey_fp,
      caller_ephemeral_pub: base64X25519Pub('phase1-shell-caller'),
      client_ephemeral_pub: base64X25519Pub('phase1-shell-client'),
      grant_mode: 'permanent',
      grant_scope: 'remote_shell_exec',
      status: 'active',
      first_authorized_at: now,
      expires_at: '',
      session_token_hash: sha256Hex(sessionToken),
      session_token_expires_at: new Date(Date.now() + 60_000).toISOString(),
      last_used_at: now,
      max_calls: 999999,
      remaining_calls: 999999,
      created_by: 'test',
      update_time: now,
    });

    const openResponse = await req('POST', '/v5/remote-invoke/calls/open', signPopBody({
      client_instance_id: 'ri-phase1-client',
      command_kind: 'shell.exec',
      command_encrypted: {
        version: 2,
        nonce: 'phase1-nonce',
        ciphertext: 'PHASE1-OPAQUE-CMD',
        tag: 'phase1-tag',
      },
      command_summary: {
        command_preview: 'deploy api',
      },
      timeout_hint_ms: 60_000,
      pty_enabled: true,
    }, callerKeys), { Authorization: `Bearer ${sessionToken}` });

    expect(openResponse.status).toBe(200);
    expect(openResponse.data.data.call_meta.command_kind).toBe('shell.exec');

    const callId = openResponse.data.data.call_id as string;
    const callDetail = await req(
      'GET',
      `/v4/remote-invoke/client/calls/${callId}?client_instance_id=ri-phase1-client`,
      undefined,
      { Authorization: `Bearer ${clientAuthToken}` },
    );

    expect(callDetail.status).toBe(200);
    expect(callDetail.data.data.command_kind).toBe('shell.exec');
    expect(callDetail.data.data.command_detail).toEqual({ kind: 'shell.exec' });
    expect(JSON.stringify(callDetail.data.data)).not.toContain('PHASE1-OPAQUE-CMD');
  });

  it('updates only the minimal relay grant scope through the client grant PATCH route', async () => {
    const token = await registerUser('ri_phase1_update_owner', 'password123');
    const clientKeys = generateClientKeypair();
    const clientPubkey = clientKeys.publicKey.export({ type: 'spki', format: 'der' }).toString('base64');
    const clientAuthToken = await registerClient(
      'ri-phase1-update-client',
      clientPubkey,
      clientKeys.privateKey,
      token,
    );
    const now = new Date().toISOString();

    await server.storage.remoteInvoke.createGrant({
      id: 'ri-phase1-update-grant',
      user_id: 'ri_phase1_update_owner',
      client_instance_id: 'ri-phase1-update-client',
      caller_fingerprint: 'phase1-update-caller',
      caller_display_name: 'phase1-update-caller',
      grant_mode: 'permanent',
      grant_scope: 'remote_query',
      status: 'active',
      first_authorized_at: now,
      expires_at: '',
      last_used_at: now,
      max_calls: 999999,
      remaining_calls: 999999,
      created_by: 'test',
      update_time: now,
    });

    const response = await req(
      'PATCH',
      '/v4/remote-invoke/client/grants/ri-phase1-update-grant',
      {
        client_instance_id: 'ri-phase1-update-client',
        grant_scope: 'remote_shell_exec',
      },
      { Authorization: `Bearer ${clientAuthToken}` },
    );

    expect(response.status).toBe(200);
    expect(response.data.data.grant_scope).toBe('remote_shell_exec');

    const storedGrant = await server.storage.remoteInvoke.getGrant('ri-phase1-update-grant');
    expect(storedGrant?.grant_scope).toBe('remote_shell_exec');
  });

  it('replaces older active grants for the same caller when a new pairing approval succeeds', async () => {
    const token = await registerUser(`ri_phase1_replace_owner_${Date.now()}`, 'password123');
    const clientKeys = generateClientKeypair();
    const clientPubkey = clientKeys.publicKey.export({ type: 'spki', format: 'der' }).toString('base64');
    const clientInstanceId = `ri-phase1-replace-client-${Date.now()}`;
    const clientAuthToken = await registerClient(
      clientInstanceId,
      clientPubkey,
      clientKeys.privateKey,
      token,
    );
    const now = new Date().toISOString();

    await server.storage.remoteInvoke.createGrant({
      id: 'ri-phase1-old-grant',
      user_id: '',
      client_instance_id: clientInstanceId,
      caller_fingerprint: 'phase1-replace-caller',
      caller_display_name: 'phase1-replace-caller',
      caller_ephemeral_pub: 'old-caller-epk',
      client_ephemeral_pub: 'old-client-epk',
      grant_mode: 'permanent',
      grant_scope: 'remote_query',
      ssh_key_id: '',
      ssh_key_fingerprint: '',
      status: 'active',
      first_authorized_at: new Date(Date.now() - 5_000).toISOString(),
      expires_at: '',
      last_used_at: now,
      max_calls: 999999,
      remaining_calls: 999999,
      created_by: 'test',
      update_time: now,
    });

    await server.storage.remoteInvoke.createPairing({
      id: 'ri-phase1-replace-pairing',
      user_id: '',
      client_instance_id: clientInstanceId,
      caller_fingerprint: 'phase1-replace-caller',
      pair_code: '654321',
      status: 'pending_approval',
      caller_pubkey: '',
      caller_ephemeral_pub: 'new-caller-epk',
      client_ephemeral_pub: '',
      caller_info_json: JSON.stringify({ fingerprint: 'phase1-replace-caller', display_name: 'replace-caller' }),
      command_summary_json: '{}',
      command_json: '{}',
      relay_token: '',
      call_id: '',
      grant_id: '',
      expires_at: new Date(Date.now() + 60_000).toISOString(),
      create_time: now,
      update_time: now,
    });

    const response = await req(
      'POST',
      '/v4/remote-invoke/client/grants/ri-phase1-replace-pairing/decision',
      {
        decision: 'approve',
        grant_mode: 'permanent',
        grant_scope: 'remote_shell_exec',
        client_instance_id: clientInstanceId,
        client_ephemeral_pub: 'new-client-epk',
      },
      { Authorization: `Bearer ${clientAuthToken}` },
    );

    expect(response.status, JSON.stringify(response.data)).toBe(200);

    const legacyReusable = await req(
      'GET',
      `/v4/remote-invoke/grants/reusable?client_instance_id=${encodeURIComponent(clientInstanceId)}&caller_fingerprint=${encodeURIComponent('phase1-replace-caller')}`,
    );
    expect(legacyReusable.status).toBe(404);
    expect(legacyReusable.data.message).toBe('remote invoke endpoint not found');

    const activeGrants = await server.storage.remoteInvoke.listActiveGrantsForClient(clientInstanceId);
    expect(activeGrants).toHaveLength(1);
    expect(activeGrants[0].id).not.toBe('ri-phase1-old-grant');
    expect(activeGrants[0].caller_ephemeral_pub).toBe('new-caller-epk');
    expect(activeGrants[0].client_ephemeral_pub).toBe('new-client-epk');

    const oldGrant = await server.storage.remoteInvoke.getGrant('ri-phase1-old-grant');
    expect(oldGrant?.status).toBe('removed');
  });

  it('forwards exit_encrypted to caller without persisting plaintext payload', async () => {
    const token = await registerUser('ri_phase1_exit_owner', 'password123');
    const clientKeys = generateClientKeypair();
    const clientPubkey = clientKeys.publicKey.export({ type: 'spki', format: 'der' }).toString('base64');
    const clientAuthToken = await registerClient('ri-phase1-exit-client', clientPubkey, clientKeys.privateKey, token);
    const now = new Date().toISOString();
    const callerKeys = makeCallerKeypair();
    const sessionToken = 'ri-phase1-exit-session';

    await server.storage.remoteInvoke.createGrant({
      id: 'ri-phase1-exit-grant',
      user_id: 'ri_phase1_exit_owner',
      client_instance_id: 'ri-phase1-exit-client',
      caller_fingerprint: 'phase1-exit-caller',
      caller_display_name: 'phase1-exit-caller',
      caller_pubkey: callerKeys.caller_pubkey,
      caller_pubkey_fp: callerKeys.caller_pubkey_fp,
      caller_ephemeral_pub: base64X25519Pub('phase1-exit-caller'),
      client_ephemeral_pub: base64X25519Pub('phase1-exit-client'),
      grant_mode: 'permanent',
      grant_scope: 'remote_shell_exec',
      status: 'active',
      first_authorized_at: now,
      expires_at: '',
      ...createGrantSession(sessionToken),
      last_used_at: now,
      max_calls: 999999,
      remaining_calls: 999999,
      created_by: 'test',
      update_time: now,
    });

    const openResponse = await postOpenCallV5(sessionToken, callerKeys, {
      client_instance_id: 'ri-phase1-exit-client',
      command_kind: 'shell.exec',
      command_encrypted: {
        version: 2,
        nonce: 'exit-nonce',
        ciphertext: 'opaque-command',
        tag: 'exit-tag',
      },
      command_summary: {
        command_preview: 'collect logs',
      },
    });
    expect(openResponse.status).toBe(200);

    const callId = openResponse.data.data.call_id as string;
    const relayToken = openResponse.data.data.relay_token as string;
    const callerEvents = await openSseWithEvents(
      `/v4/remote-invoke/calls/${callId}/events`,
      { Authorization: `Bearer ${relayToken}` },
    );
    expect(callerEvents.status).toBe(200);
    await callerEvents.nextEvent('connected');

    const exitResponse = await req(
      'POST',
      `/v4/remote-invoke/client/calls/${callId}/exit`,
      {
        client_instance_id: 'ri-phase1-exit-client',
        exit_encrypted: {
          version: 2,
          nonce: 'exit-nonce-2',
          ciphertext: 'PHASE1-OPAQUE-EXIT',
          tag: 'exit-tag-2',
        },
      },
      { Authorization: `Bearer ${clientAuthToken}` },
    );
    expect(exitResponse.status).toBe(200);

    const exitEvent = await callerEvents.nextEvent('exit');
    expect(exitEvent).toMatchObject({
      call_id: callId,
      exit_encrypted: {
        version: 2,
        nonce: 'exit-nonce-2',
        ciphertext: 'PHASE1-OPAQUE-EXIT',
        tag: 'exit-tag-2',
      },
    });

    const call = await server.storage.remoteInvoke.getCall(callId);
    expect(call).toBeTruthy();
    expect(call?.stderr_digest).toBe('');
    expect(JSON.stringify(call)).not.toContain('PHASE1-OPAQUE-EXIT');
    callerEvents.close();
  });

  it('completes an encrypted query.readonly roundtrip across caller and client streams', async () => {
    const token = await registerUser('ri_phase1_roundtrip_owner', 'password123');
    const clientKeys = generateClientKeypair();
    const clientPubkey = clientKeys.publicKey.export({ type: 'spki', format: 'der' }).toString('base64');
    const clientAuthToken = await registerClient('ri-phase1-roundtrip-client', clientPubkey, clientKeys.privateKey, token);
    const clientEvents = await openSseWithEvents(
      '/v4/remote-invoke/client/stream?client_instance_id=ri-phase1-roundtrip-client&stream_id=ri-phase1-roundtrip-stream',
      { Authorization: `Bearer ${clientAuthToken}` },
    );
    expect(clientEvents.status).toBe(200);
    await clientEvents.nextEvent('client_hello_ack');

    const now = new Date().toISOString();
    const callerKeys = makeCallerKeypair();
    const sessionToken = 'ri-phase1-roundtrip-session';
    const callerEphemeralPub = base64X25519Pub('phase1-roundtrip-caller');
    const clientEphemeralPub = base64X25519Pub('phase1-roundtrip-client');
    await server.storage.remoteInvoke.createGrant({
      id: 'ri-phase1-roundtrip-grant',
      user_id: 'ri_phase1_roundtrip_owner',
      client_instance_id: 'ri-phase1-roundtrip-client',
      caller_fingerprint: 'phase1-roundtrip-caller',
      caller_display_name: 'phase1-roundtrip-caller',
      caller_pubkey: callerKeys.caller_pubkey,
      caller_pubkey_fp: callerKeys.caller_pubkey_fp,
      caller_ephemeral_pub: callerEphemeralPub,
      client_ephemeral_pub: clientEphemeralPub,
      grant_mode: 'permanent',
      grant_scope: 'remote_query',
      status: 'active',
      first_authorized_at: now,
      expires_at: '',
      ...createGrantSession(sessionToken),
      last_used_at: now,
      max_calls: 999999,
      remaining_calls: 999999,
      created_by: 'test',
      update_time: now,
    });

    const openResponse = await postOpenCallV5(sessionToken, callerKeys, {
      client_instance_id: 'ri-phase1-roundtrip-client',
      command_kind: 'query.readonly',
      command_encrypted: {
        version: 2,
        nonce: 'roundtrip-open-nonce',
        ciphertext: 'PHASE1-ROUNDTRIP-CMD',
        tag: 'roundtrip-open-tag',
      },
      command_summary: {
        command_preview: 'status',
      },
      timeout_hint_ms: 15_000,
    });

    expect(openResponse.status).toBe(200);
    const callId = openResponse.data.data.call_id as string;
    const relayToken = openResponse.data.data.relay_token as string;

    const callOpenEvent = await clientEvents.nextEvent('call_open');
    expect(callOpenEvent).toMatchObject({
      call_id: callId,
      grant_id: 'ri-phase1-roundtrip-grant',
      grant_scope: 'remote_query',
      caller_fingerprint: 'phase1-roundtrip-caller',
      caller_ephemeral_pub: callerEphemeralPub,
      client_ephemeral_pub: clientEphemeralPub,
      command_kind: 'query.readonly',
      command_encrypted: {
        version: 2,
        nonce: 'roundtrip-open-nonce',
        ciphertext: 'PHASE1-ROUNDTRIP-CMD',
        tag: 'roundtrip-open-tag',
      },
      timeout_hint_ms: 15_000,
    });
    expect(callOpenEvent).not.toHaveProperty('command');

    const frameEnvelope = JSON.stringify({
      version: 2,
      call_id: callId,
      seq: 1,
      direction: 'client_to_caller',
      nonce: 'roundtrip-frame-nonce',
      ciphertext: 'PHASE1-ROUNDTRIP-FRAME',
      tag: 'roundtrip-frame-tag',
      aad: {
        frame_type: 'stdout',
        command_kind: 'query.readonly',
      },
    });

    const frameResponse = await req(
      'POST',
      `/v4/remote-invoke/client/calls/${callId}/frame`,
      {
        client_instance_id: 'ri-phase1-roundtrip-client',
        envelope_json: frameEnvelope,
      },
      { Authorization: `Bearer ${clientAuthToken}` },
    );
    expect(frameResponse.status).toBe(200);

    const callerEvents = await openSseWithEvents(
      `/v4/remote-invoke/calls/${callId}/events`,
      { Authorization: `Bearer ${relayToken}` },
    );
    expect(callerEvents.status).toBe(200);
    await callerEvents.nextEvent('connected');

    const frameEvent = await callerEvents.nextEvent('frame');
    expect(frameEvent).toMatchObject({
      call_id: callId,
      envelope_json: frameEnvelope,
    });

    const exitResponse = await req(
      'POST',
      `/v4/remote-invoke/client/calls/${callId}/exit`,
      {
        client_instance_id: 'ri-phase1-roundtrip-client',
        exit_code: 0,
        duration_ms: 321,
        bytes_out: 42,
        exit_encrypted: {
          version: 2,
          nonce: 'roundtrip-exit-nonce',
          ciphertext: 'PHASE1-ROUNDTRIP-EXIT',
          tag: 'roundtrip-exit-tag',
        },
      },
      { Authorization: `Bearer ${clientAuthToken}` },
    );
    expect(exitResponse.status).toBe(200);

    const exitEvent = await callerEvents.nextEvent('exit');
    expect(exitEvent).toMatchObject({
      call_id: callId,
      exit_code: 0,
      duration_ms: 321,
      exit_encrypted: {
        version: 2,
        nonce: 'roundtrip-exit-nonce',
        ciphertext: 'PHASE1-ROUNDTRIP-EXIT',
        tag: 'roundtrip-exit-tag',
      },
    });

    const call = await server.storage.remoteInvoke.getCall(callId);
    expect(call).toBeTruthy();
    expect(call?.status).toBe('completed');
    expect(JSON.stringify(call)).not.toContain('PHASE1-ROUNDTRIP-CMD');
    expect(JSON.stringify(call)).not.toContain('PHASE1-ROUNDTRIP-FRAME');
    expect(JSON.stringify(call)).not.toContain('PHASE1-ROUNDTRIP-EXIT');

    const events = await server.storage.remoteInvoke.listCallEvents(callId, { offset: 0, limit: 20 });
    expect(events.list.map((event: { event_type: string }) => event.event_type)).toEqual([
      'call_opened',
      'call_frame_out',
      'call_completed',
    ]);
    expect(JSON.stringify(events.list)).not.toContain('PHASE1-ROUNDTRIP-FRAME');
    expect(JSON.stringify(events.list)).not.toContain('PHASE1-ROUNDTRIP-EXIT');

    callerEvents.close();
    clientEvents.close();
  });

  it('keeps encrypted call records route-only after relay restart', async () => {
    const token = await registerUser('ri_phase1_restart_owner', 'password123');
    const clientKeys = generateClientKeypair();
    const clientPubkey = clientKeys.publicKey.export({ type: 'spki', format: 'der' }).toString('base64');
    const clientAuthToken = await registerClient('ri-phase1-restart-client', clientPubkey, clientKeys.privateKey, token);
    const now = new Date().toISOString();
    const callerKeys = makeCallerKeypair();
    const sessionToken = 'ri-phase1-restart-session';

    await server.storage.remoteInvoke.createGrant({
      id: 'ri-phase1-restart-grant',
      user_id: 'ri_phase1_restart_owner',
      client_instance_id: 'ri-phase1-restart-client',
      caller_fingerprint: 'phase1-restart-caller',
      caller_display_name: 'phase1-restart-caller',
      caller_pubkey: callerKeys.caller_pubkey,
      caller_pubkey_fp: callerKeys.caller_pubkey_fp,
      caller_ephemeral_pub: base64X25519Pub('phase1-restart-caller'),
      client_ephemeral_pub: base64X25519Pub('phase1-restart-client'),
      grant_mode: 'permanent',
      grant_scope: 'remote_shell_exec',
      status: 'active',
      first_authorized_at: now,
      expires_at: '',
      ...createGrantSession(sessionToken),
      last_used_at: now,
      max_calls: 999999,
      remaining_calls: 999999,
      created_by: 'test',
      update_time: now,
    });

    const openResponse = await postOpenCallV5(sessionToken, callerKeys, {
      client_instance_id: 'ri-phase1-restart-client',
      command_kind: 'shell.exec',
      command_encrypted: {
        version: 2,
        nonce: 'restart-open-nonce',
        ciphertext: 'PHASE1-RESTART-CMD',
        tag: 'restart-open-tag',
      },
      command_summary: {
        command_preview: 'restart-safe-command',
      },
    });
    expect(openResponse.status).toBe(200);
    const callId = openResponse.data.data.call_id as string;

    const exitResponse = await req(
      'POST',
      `/v4/remote-invoke/client/calls/${callId}/exit`,
      {
        client_instance_id: 'ri-phase1-restart-client',
        exit_code: 0,
        exit_encrypted: {
          version: 2,
          nonce: 'restart-exit-nonce',
          ciphertext: 'PHASE1-RESTART-EXIT',
          tag: 'restart-exit-tag',
        },
      },
      { Authorization: `Bearer ${clientAuthToken}` },
    );
    expect(exitResponse.status).toBe(200);

    await restartServer();

    const callAfterRestart = await server.storage.remoteInvoke.getCall(callId);
    expect(callAfterRestart).toBeTruthy();
    expect(callAfterRestart?.id).toBe(callId);
    expect(JSON.parse(callAfterRestart!.command_json)).toEqual({ kind: 'shell.exec' });
    expect(JSON.stringify(callAfterRestart)).not.toContain('PHASE1-RESTART-CMD');
    expect(JSON.stringify(callAfterRestart)).not.toContain('PHASE1-RESTART-EXIT');
  });

  it('opens v5 calls through bearer plus PoP and rejects grant_id in the caller body', async () => {
    const keypair = makeCallerKeypair();
    const sessionToken = 'phase1-v5-session-token';
    const grant = await seedActiveGrant(server, {
      id: 'ri-phase1-v5-open-grant',
      client_instance_id: 'ri-phase1-v5-client',
      caller_pubkey: keypair.caller_pubkey,
      caller_pubkey_fp: keypair.caller_pubkey_fp,
      caller_ephemeral_pub: base64X25519Pub('phase1-v5-caller'),
      client_ephemeral_pub: base64X25519Pub('phase1-v5-client'),
      session_token_hash: sha256Hex(sessionToken),
      session_token_expires_at: new Date(Date.now() + 60_000).toISOString(),
    });

    const rejected = await req('POST', '/v5/remote-invoke/calls/open', signPopBody({
      grant_id: grant.id,
      client_instance_id: grant.client_instance_id,
      command_kind: 'shell.exec',
      command_encrypted: {
        version: 2,
        nonce: 'phase1-v5-open-nonce',
        ciphertext: 'PHASE1-V5-CMD',
        tag: 'phase1-v5-open-tag',
      },
    }, keypair), { Authorization: `Bearer ${sessionToken}` });
    expect(rejected.status).toBe(400);
    expect(rejected.data.message).toBe('grant_id is not accepted in v5');

    const opened = await req('POST', '/v5/remote-invoke/calls/open', signPopBody({
      client_instance_id: grant.client_instance_id,
      command_kind: 'shell.exec',
      command_encrypted: {
        version: 2,
        nonce: 'phase1-v5-open-nonce',
        ciphertext: 'PHASE1-V5-CMD',
        tag: 'phase1-v5-open-tag',
      },
      command_summary: {
        command_preview: 'v5-safe-command',
      },
    }, keypair), { Authorization: `Bearer ${sessionToken}` });
    expect(opened.status).toBe(200);
    expect(opened.data.data.call_id).toBeTruthy();
    expect(opened.data.data).not.toHaveProperty('grant_id');

    const call = await server.storage.remoteInvoke.getCall(opened.data.data.call_id);
    expect(call?.grant_id).toBe(grant.id);
    expect(call?.command_json).toBe(JSON.stringify({ kind: 'shell.exec' }));
  });

  it('atomically consumes once grants under concurrent v5 openCall requests', async () => {
    const keypair = makeCallerKeypair();
    const sessionToken = 'phase1-v5-once-session-token';
    const grant = await seedActiveGrant(server, {
      id: 'ri-phase1-v5-once-open-grant',
      client_instance_id: 'ri-phase1-v5-once-client',
      caller_pubkey: keypair.caller_pubkey,
      caller_pubkey_fp: keypair.caller_pubkey_fp,
      caller_ephemeral_pub: base64X25519Pub('phase1-v5-once-caller'),
      client_ephemeral_pub: base64X25519Pub('phase1-v5-once-client'),
      grant_mode: 'once',
      max_calls: 1,
      remaining_calls: 1,
      session_token_hash: sha256Hex(sessionToken),
      session_token_expires_at: new Date(Date.now() + 60_000).toISOString(),
    });

    const makeBody = (label: string) => signPopBody({
      client_instance_id: grant.client_instance_id,
      command_kind: 'query.readonly',
      command_encrypted: {
        version: 2,
        nonce: `phase1-v5-once-${label}`,
        ciphertext: `PHASE1-V5-ONCE-${label}`,
        tag: `phase1-v5-once-tag-${label}`,
      },
      command_summary: {
        command_preview: `once-${label}`,
      },
    }, keypair);

    const results = await Promise.all([
      req('POST', '/v5/remote-invoke/calls/open', makeBody('a'), { Authorization: `Bearer ${sessionToken}` }),
      req('POST', '/v5/remote-invoke/calls/open', makeBody('b'), { Authorization: `Bearer ${sessionToken}` }),
    ]);

    const successCount = results.filter((r) => r.status === 200).length;
    const rejectedCount = results.filter((r) => (
      (r.status === 403 && r.data.message === 'grant_consumed')
      || (r.status === 401 && r.data.message === 'grant_session_token_invalid')
    )).length;
    expect(successCount).toBe(1);
    expect(rejectedCount).toBe(1);

    const storedGrant = await server.storage.remoteInvoke.getGrant(grant.id);
    expect(storedGrant?.remaining_calls).toBe(0);
    const calls = await server.storage.remoteInvoke.listCalls('', {
      client_instance_id: grant.client_instance_id,
      offset: 0,
      limit: 10,
    });
    expect(calls.total).toBe(1);
  });

});
