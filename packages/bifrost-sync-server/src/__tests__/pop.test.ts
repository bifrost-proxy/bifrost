import crypto from 'crypto';
import { describe, expect, it } from 'vitest';

import { canonicalJson, verifyPoP } from '../remote-invoke/pop';
import { makeCallerKeypair, signPopBody } from './remote-invoke-v5-test-utils';

describe('remote invoke v5 PoP', () => {
  it('canonicalizes objects with sorted keys and without signature', () => {
    const first = canonicalJson({
      z: 1,
      signature: 'ignored',
      a: { d: 4, b: 2 },
      c: [3, { y: 2, x: 1 }],
    });
    const second = canonicalJson({
      c: [3, { x: 1, y: 2 }],
      a: { b: 2, d: 4 },
      z: 1,
    });

    expect(first).toBe(second);
    expect(first).not.toContain('signature');
    expect(first).toBe('{"a":{"b":2,"d":4},"c":[3,{"x":1,"y":2}],"z":1}');
  });

  it('accepts a valid Ed25519 proof and rejects nonce replay', async () => {
    const keypair = makeCallerKeypair();
    const seen = new Set<string>();
    const body = signPopBody({ client_instance_id: 'client-1' }, keypair);

    const first = await verifyPoP(body as any, {}, (_fp, nonce) => {
      if (seen.has(nonce)) return false;
      seen.add(nonce);
      return true;
    });
    expect(first.callerPubkeyFp).toBe(keypair.caller_pubkey_fp);

    await expect(verifyPoP(body as any, {}, (_fp, nonce) => {
      if (seen.has(nonce)) return false;
      seen.add(nonce);
      return true;
    })).rejects.toThrow('replay_detected');
  });

  it('rejects tampered fields', async () => {
    const keypair = makeCallerKeypair();
    const body = signPopBody({ client_instance_id: 'client-1' }, keypair);
    body.client_instance_id = 'client-2';

    await expect(verifyPoP(body as any, {}, () => true)).rejects.toThrow('signature_invalid');
  });

  it('rejects timestamps outside the accepted window', async () => {
    const keypair = makeCallerKeypair();
    const body = signPopBody({ client_instance_id: 'client-1' }, keypair);
    body.ts = Date.now() - 120_000;

    await expect(verifyPoP(body as any, {}, () => true)).rejects.toThrow('timestamp_out_of_window');
  });

  it('rejects non-Ed25519 caller public keys', async () => {
    const rsa = crypto.generateKeyPairSync('rsa', { modulusLength: 2048 });
    const caller_pubkey = rsa.publicKey.export({ format: 'der', type: 'spki' }).toString('base64');
    const body = {
      ts: Date.now(),
      nonce: crypto.randomBytes(16).toString('hex'),
      caller_pubkey,
      client_instance_id: 'client-1',
    };
    const signature = crypto.sign('sha256', Buffer.from(canonicalJson(body), 'utf8'), rsa.privateKey).toString('base64');

    await expect(verifyPoP({ ...body, signature } as any, {}, () => true)).rejects.toThrow('invalid_caller_pubkey');
  });
});
