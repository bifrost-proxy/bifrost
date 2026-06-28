import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import {
  base64X25519Pub,
  bootRemoteInvokeServer,
  expectOk,
  makeCallerKeypair,
  seedApprovedPairingWithGrant,
  signPopBody,
  type TestServer,
} from './remote-invoke-v5-test-utils';

describe('remote invoke v5 grants claim', () => {
  let app: TestServer;

  beforeEach(async () => {
    app = await bootRemoteInvokeServer('grants-claim');
  });

  afterEach(async () => {
    await app.close();
  });

  it('requires caller_ephemeral_pub in the PoP-protected claim body', async () => {
    const keypair = makeCallerKeypair();
    await seedApprovedPairingWithGrant(app.server, {
      claimToken: 'claim-required',
      callerPubkeyFp: keypair.caller_pubkey_fp,
    });

    const response = await app.request('POST', '/v5/remote-invoke/grants/claim', signPopBody({
      client_instance_id: 'client-v5',
      pair_code: 'PAIR123',
      claim_token: 'claim-required',
    }, keypair));

    expect(response.status).toBe(400);
    expect(response.data.message).toBe('caller_ephemeral_pub_required');
  });

  it('rejects caller_ephemeral_pub that is not a 32-byte base64 X25519 public key', async () => {
    const keypair = makeCallerKeypair();
    await seedApprovedPairingWithGrant(app.server, {
      claimToken: 'claim-invalid',
      callerPubkeyFp: keypair.caller_pubkey_fp,
    });

    const response = await app.request('POST', '/v5/remote-invoke/grants/claim', signPopBody({
      client_instance_id: 'client-v5',
      pair_code: 'PAIR123',
      claim_token: 'claim-invalid',
      caller_ephemeral_pub: 'not-base64',
    }, keypair));

    expect(response.status).toBe(400);
    expect(response.data.message).toBe('caller_ephemeral_pub_invalid');
  });

  it('binds caller pubkey and caller ephemeral pub, then returns client_ephemeral_pub in the grant summary', async () => {
    const keypair = makeCallerKeypair();
    const callerEphemeralPub = base64X25519Pub('claim-caller');
    const clientEphemeralPub = base64X25519Pub('claim-client');
    const { grant } = await seedApprovedPairingWithGrant(app.server, {
      claimToken: 'claim-ok',
      callerPubkeyFp: keypair.caller_pubkey_fp,
      clientEphemeralPub,
    });

    const response = await app.request('POST', '/v5/remote-invoke/grants/claim', signPopBody({
      client_instance_id: grant.client_instance_id,
      pair_code: 'PAIR123',
      claim_token: 'claim-ok',
      caller_ephemeral_pub: callerEphemeralPub,
    }, keypair));

    expectOk(response);
    expect(response.data.data.grant_session_token).toMatch(/^[a-f0-9]{64}$/);
    expect(response.data.data.grant_summary.client_ephemeral_pub).toBe(clientEphemeralPub);
    expect(response.data.data.grant_summary).not.toHaveProperty('grant_id');

    const updated = await app.server.storage.remoteInvoke.getGrant(grant.id);
    expect(updated?.caller_pubkey).toBe(keypair.caller_pubkey);
    expect(updated?.caller_pubkey_fp).toBe(keypair.caller_pubkey_fp);
    expect(updated?.caller_ephemeral_pub).toBe(callerEphemeralPub);
  });
});
