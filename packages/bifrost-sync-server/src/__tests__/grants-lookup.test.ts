import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import {
  base64X25519Pub,
  bootRemoteInvokeServer,
  expectOk,
  makeCallerKeypair,
  seedActiveGrant,
  signPopBody,
  type TestServer,
} from './remote-invoke-v5-test-utils';

describe('remote invoke v5 grants lookup', () => {
  let app: TestServer;

  beforeEach(async () => {
    app = await bootRemoteInvokeServer('grants-lookup');
  });

  afterEach(async () => {
    await app.close();
  });

  it('requires caller_ephemeral_pub', async () => {
    const keypair = makeCallerKeypair();
    await seedActiveGrant(app.server, {
      client_instance_id: 'lookup-client',
      caller_pubkey_fp: keypair.caller_pubkey_fp,
    });

    const response = await app.request('POST', '/v5/remote-invoke/grants/lookup', signPopBody({
      client_instance_id: 'lookup-client',
    }, keypair));

    expect(response.status).toBe(400);
    expect(response.data.message).toBe('caller_ephemeral_pub_required');
  });

  it('returns 404 when no active grant matches the PoP caller', async () => {
    const keypair = makeCallerKeypair();
    const response = await app.request('POST', '/v5/remote-invoke/grants/lookup', signPopBody({
      client_instance_id: 'lookup-client',
      caller_ephemeral_pub: base64X25519Pub('lookup-miss'),
    }, keypair));

    expect(response.status).toBe(404);
    expect(response.data.message).toBe('grant_not_found');
  });

  it('mints a session token and includes client_ephemeral_pub', async () => {
    const keypair = makeCallerKeypair();
    const clientEphemeralPub = base64X25519Pub('lookup-client');
    await seedActiveGrant(app.server, {
      client_instance_id: 'lookup-client',
      caller_pubkey_fp: keypair.caller_pubkey_fp,
      client_ephemeral_pub: clientEphemeralPub,
    });

    const response = await app.request('POST', '/v5/remote-invoke/grants/lookup', signPopBody({
      client_instance_id: 'lookup-client',
      caller_ephemeral_pub: base64X25519Pub('lookup-caller-1'),
    }, keypair));

    expectOk(response);
    expect(response.data.data.grant_session_token).toMatch(/^[a-f0-9]{64}$/);
    expect(response.data.data.grant_summary.client_ephemeral_pub).toBe(clientEphemeralPub);
  });

  it('garbage collects PoP nonces older than 60 seconds before marking the new nonce', async () => {
    const keypair = makeCallerKeypair();
    await seedActiveGrant(app.server, {
      client_instance_id: 'lookup-client',
      caller_pubkey_fp: keypair.caller_pubkey_fp,
      client_ephemeral_pub: base64X25519Pub('lookup-client'),
    });
    await app.server.storage.remoteInvoke.markNonceUsed(
      keypair.caller_pubkey_fp,
      'oldnonceoldnonceoldnonceoldnonce',
      new Date(Date.now() - 120_000).toISOString(),
    );

    const response = await app.request('POST', '/v5/remote-invoke/grants/lookup', signPopBody({
      client_instance_id: 'lookup-client',
      caller_ephemeral_pub: base64X25519Pub('lookup-caller-gc'),
    }, keypair));

    expectOk(response);
    const remainingOldNonces = await app.server.storage.remoteInvoke.gcNonces(
      new Date(Date.now() - 60_000).toISOString(),
    );
    expect(remainingOldNonces).toBe(0);
  });

  it('overwrites caller_ephemeral_pub on repeated lookup for the same caller key', async () => {
    const keypair = makeCallerKeypair();
    const grant = await seedActiveGrant(app.server, {
      client_instance_id: 'lookup-client',
      caller_pubkey_fp: keypair.caller_pubkey_fp,
      caller_ephemeral_pub: base64X25519Pub('lookup-old'),
    });
    const nextEphemeralPub = base64X25519Pub('lookup-new');

    const response = await app.request('POST', '/v5/remote-invoke/grants/lookup', signPopBody({
      client_instance_id: 'lookup-client',
      caller_ephemeral_pub: nextEphemeralPub,
    }, keypair));

    expectOk(response);
    const updated = await app.server.storage.remoteInvoke.getGrant(grant.id);
    expect(updated?.caller_ephemeral_pub).toBe(nextEphemeralPub);
  });
});
