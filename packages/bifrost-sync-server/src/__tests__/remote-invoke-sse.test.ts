import { afterEach, describe, expect, it } from 'vitest';

import {
  clearCallerEventBuffer,
  flushCallerEventStream,
  pushToCallerStream,
  registerCallerEventStream,
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

afterEach(() => {
  for (const callId of TEST_CALL_IDS) {
    unregisterCallerEventStream(callId);
    clearCallerEventBuffer(callId);
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
