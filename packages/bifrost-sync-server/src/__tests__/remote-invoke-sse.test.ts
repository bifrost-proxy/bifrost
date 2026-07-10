import { afterEach, describe, expect, it } from 'vitest';

import {
  clearCallerEventBuffer,
  clearReplaySafeClientEvents,
  flushReplaySafeClientEvents,
  flushCallerEventStream,
  pushReplaySafeToClient,
  pushToCallerStream,
  registerClientStream,
  registerCallerEventStream,
  unregisterClientStream,
  unregisterCallerEventStream,
} from '../remote-invoke/sse';

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

const TEST_CALL_IDS = ['ri-test-call-buffered', 'ri-test-call-order'];
const TEST_CLIENT_IDS = ['ri-test-client-buffered', 'ri-test-client-reconnect'];

afterEach(() => {
  for (const callId of TEST_CALL_IDS) {
    unregisterCallerEventStream(callId);
    clearCallerEventBuffer(callId);
    clearReplaySafeClientEvents(callId);
  }
  for (const clientId of TEST_CLIENT_IDS) {
    unregisterClientStream(clientId);
  }
});

describe('remote invoke caller SSE buffering', () => {
  it('buffers caller events until the caller stream is registered', () => {
    const callId = TEST_CALL_IDS[0];

    expect(pushToCallerStream(callId, 'frame', { seq: 1, body: 'hello' })).toBe(true);

    const res = new FakeServerResponse();
    registerCallerEventStream(callId, res as any);
    expect(flushCallerEventStream(callId)).toBe(true);

    const body = res.body();
    expect(body).toContain('event: frame');
    expect(body).toContain('"body":"hello"');
  });

  it('flushes buffered caller events in original order', () => {
    const callId = TEST_CALL_IDS[1];

    pushToCallerStream(callId, 'frame', { seq: 1 });
    pushToCallerStream(callId, 'frame', { seq: 2 });
    pushToCallerStream(callId, 'exit', { exit_code: 0 });

    const res = new FakeServerResponse();
    registerCallerEventStream(callId, res as any);
    flushCallerEventStream(callId);

    const body = res.body();
    const frame1 = body.indexOf('"seq":1');
    const frame2 = body.indexOf('"seq":2');
    const exit = body.indexOf('"exit_code":0');

    expect(frame1).toBeGreaterThanOrEqual(0);
    expect(frame2).toBeGreaterThan(frame1);
    expect(exit).toBeGreaterThan(frame2);
  });
});

describe('remote invoke replay-safe target SSE buffering', () => {
  it('buffers caller stdin until the target stream reconnects', () => {
    const callId = TEST_CALL_IDS[0];
    const clientId = TEST_CLIENT_IDS[0];
    expect(pushReplaySafeToClient(clientId, callId, 'call_frame', {
      call_id: callId,
      envelope_json: '{"seq":1}',
    })).toBe(true);

    const res = new FakeServerResponse();
    registerClientStream({
      clientInstanceId: clientId,
      streamId: 'stream-buffered',
      res,
    } as any);
    expect(flushReplaySafeClientEvents(clientId)).toBe(true);
    expect(res.body()).toContain('event: call_frame');
    expect(res.body()).toContain('\\"seq\\":1');
  });

  it('rejects an unavailable target when the frame cannot fit the replay buffer', () => {
    const callId = TEST_CALL_IDS[0];
    const clientId = TEST_CLIENT_IDS[0];
    expect(pushReplaySafeToClient(clientId, callId, 'call_frame', {
      call_id: callId,
      envelope_json: 'x'.repeat(512 * 1024),
    })).toBe(false);
  });

  it('replays a recently sent stdin frame on a replacement target stream', () => {
    const callId = TEST_CALL_IDS[1];
    const clientId = TEST_CLIENT_IDS[1];
    const first = new FakeServerResponse();
    registerClientStream({
      clientInstanceId: clientId,
      streamId: 'stream-first',
      res: first,
    } as any);
    expect(pushReplaySafeToClient(clientId, callId, 'call_frame', {
      call_id: callId,
      envelope_json: '{"seq":1}',
    })).toBe(true);
    expect(first.body()).toContain('event: call_frame');

    unregisterClientStream(clientId, 'stream-first');
    const replacement = new FakeServerResponse();
    registerClientStream({
      clientInstanceId: clientId,
      streamId: 'stream-replacement',
      res: replacement,
    } as any);
    expect(flushReplaySafeClientEvents(clientId)).toBe(true);
    expect(replacement.body()).toContain('event: call_frame');

    clearReplaySafeClientEvents(callId);
    const afterClear = new FakeServerResponse();
    registerClientStream({
      clientInstanceId: clientId,
      streamId: 'stream-after-clear',
      res: afterClear,
    } as any);
    expect(flushReplaySafeClientEvents(clientId)).toBe(true);
    expect(afterClear.body()).not.toContain('event: call_frame');
  });
});
