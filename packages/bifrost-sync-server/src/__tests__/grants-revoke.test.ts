import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import {
  bootRemoteInvokeServer,
  expectOk,
  makeCallerKeypair,
  seedActiveGrant,
  sha256Hex,
  signPopBody,
  type TestServer,
} from './remote-invoke-v5-test-utils';

describe('remote invoke v5 grants revoke', () => {
  let app: TestServer;

  beforeEach(async () => {
    app = await bootRemoteInvokeServer('grants-revoke');
  });

  afterEach(async () => {
    await app.close();
  });

  it('revokes the grant resolved from Bearer token when PoP matches the stored caller key', async () => {
    const keypair = makeCallerKeypair();
    const token = 'session-token-ok';
    const grant = await seedActiveGrant(app.server, {
      caller_pubkey_fp: keypair.caller_pubkey_fp,
      session_token_hash: sha256Hex(token),
      session_token_expires_at: new Date(Date.now() + 60_000).toISOString(),
    });

    const response = await app.request('POST', '/v5/remote-invoke/grants/revoke', signPopBody({
      client_instance_id: grant.client_instance_id,
    }, keypair), { Authorization: `Bearer ${token}` });

    expectOk(response);
    const updated = await app.server.storage.remoteInvoke.getGrant(grant.id);
    expect(updated?.status).toBe('revoked');
    expect(updated?.revoked_at).toBeTruthy();
    expect(updated?.session_token_hash).toBe('');
  });

  it('rejects an unknown Bearer token before PoP can authorize revoke', async () => {
    const keypair = makeCallerKeypair();
    const response = await app.request('POST', '/v5/remote-invoke/grants/revoke', signPopBody({
      client_instance_id: 'client-v5',
    }, keypair), { Authorization: 'Bearer missing-token' });

    expect(response.status).toBe(401);
    expect(response.data.message).toBe('grant_session_token_invalid');
  });

  it('rejects PoP signed by a different caller key', async () => {
    const owner = makeCallerKeypair();
    const attacker = makeCallerKeypair();
    const token = 'session-token-cross-caller';
    await seedActiveGrant(app.server, {
      caller_pubkey_fp: owner.caller_pubkey_fp,
      session_token_hash: sha256Hex(token),
      session_token_expires_at: new Date(Date.now() + 60_000).toISOString(),
    });

    const response = await app.request('POST', '/v5/remote-invoke/grants/revoke', signPopBody({
      client_instance_id: 'client-v5',
    }, attacker), { Authorization: `Bearer ${token}` });

    expect(response.status).toBe(403);
    expect(response.data.message).toBe('caller_pubkey_mismatch');
  });
});
