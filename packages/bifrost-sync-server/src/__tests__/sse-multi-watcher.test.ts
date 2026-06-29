import { afterEach, describe, expect, it } from 'vitest';

import {
  pushToPairingWatcher,
  registerPairingWatcher,
  unregisterPairingWatcher,
} from '../remote-invoke/sse';
import {
  bootRemoteInvokeServer,
  seedApprovedPairingWithGrant,
  sha256Hex,
  type TestServer,
} from './remote-invoke-v5-test-utils';

class FakeServerResponse {
  chunks: string[] = [];

  write(chunk: string) {
    this.chunks.push(String(chunk));
    return true;
  }

  end() {}

  body() {
    return this.chunks.join('');
  }
}

const PAIRING_ID = 'pairing-multi-watcher';

afterEach(() => {
  unregisterPairingWatcher(PAIRING_ID);
});

describe('remote invoke pairing SSE multi watcher', () => {
  it('delivers approved events to every watcher without sensitive grant or ephemeral fields', () => {
    const first = new FakeServerResponse();
    const second = new FakeServerResponse();
    registerPairingWatcher(PAIRING_ID, first as any, 'watch-hash-1');
    registerPairingWatcher(PAIRING_ID, second as any, 'watch-hash-2');

    const payload = {
      type: 'approved',
      claim_token: 'claim-token-once',
      claim_expires_at: '2026-06-29T01:00:00.000Z',
      grant_summary: {
        scope: 'remote_shell_exec',
        mode: 'permanent',
        file_access: 'read_write',
      },
    };
    expect(pushToPairingWatcher(PAIRING_ID, 'approved', payload)).toBe(true);

    for (const body of [first.body(), second.body()]) {
      expect(body).toContain('event: approved');
      expect(body).toContain('"claim_token":"claim-token-once"');
      expect(body).not.toContain('grant_id');
      expect(body).not.toContain('caller_ephemeral_pub');
      expect(body).not.toContain('client_ephemeral_pub');
    }
  });

  it('rejects pairing watch requests with an invalid watch_token', async () => {
    const app: TestServer = await bootRemoteInvokeServer('sse-multi-watcher');
    try {
      const { pairing } = await seedApprovedPairingWithGrant(app.server, {
        claimToken: 'claim-watch',
      });
      await app.server.storage.remoteInvoke.updatePairing(pairing.id, {
        watch_token_hash: sha256Hex('correct-watch-token'),
      });

      const response = await app.request(
        'GET',
        `/v5/remote-invoke/pairings/${pairing.id}/watch?watch_token=wrong-watch-token`,
      );
      expect(response.status).toBe(401);
      expect(response.data.message).toBe('watch_token_invalid');
    } finally {
      await app.close();
    }
  });
});
