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

const TEST_CALL_IDS = [
  'ri-test-stream-frame-buffered',
  'ri-test-stream-frame-order',
  'ri-test-stream-frame-passthrough',
];

afterEach(() => {
  for (const callId of TEST_CALL_IDS) {
    unregisterCallerEventStream(callId);
    clearCallerEventBuffer(callId);
  }
});

describe('PR#6a caller SSE stream_frame event plumbing', () => {
  it('buffers stream_frame events until the caller stream is registered', () => {
    const callId = TEST_CALL_IDS[0];

    const frameJson = JSON.stringify({
      kind: 'stdout',
      seq: 0,
      offset: 0,
      data_b64: 'aGVsbG8=',
    });
    expect(
      pushToCallerStream(callId, 'stream_frame', {
        call_id: callId,
        frame_json: frameJson,
      }),
    ).toBe(true);

    const res = new FakeServerResponse();
    registerCallerEventStream(callId, res as any);
    expect(flushCallerEventStream(callId)).toBe(true);

    const body = res.body();
    expect(body).toContain('event: stream_frame');
    expect(body).toContain(`"call_id":"${callId}"`);
    // frame_json is an inner JSON string that gets JSON-escaped in the
    // outer payload: inner `"kind":"stdout"` => `\"kind\":\"stdout\"`.
    expect(body).toContain('aGVsbG8=');
    expect(body).toContain('\\"kind\\":\\"stdout\\"');
  });

  it('preserves stream_frame ordering across stdout/stderr/done', () => {
    const callId = TEST_CALL_IDS[1];

    const chunks = [
      { kind: 'stdout', seq: 0, offset: 0, data_b64: 'AAAA' },
      { kind: 'stderr', seq: 0, offset: 0, data_b64: 'BBBB' },
      { kind: 'stdout', seq: 1, offset: 3, data_b64: 'CCCC' },
      {
        kind: 'done',
        exit_code: 0,
        total_stdout: 6,
        total_stderr: 3,
        stdout_sha256: 'd1',
        stderr_sha256: 'd2',
        duration_ms: 7,
      },
    ];
    for (const frame of chunks) {
      pushToCallerStream(callId, 'stream_frame', {
        call_id: callId,
        frame_json: JSON.stringify(frame),
      });
    }

    const res = new FakeServerResponse();
    registerCallerEventStream(callId, res as any);
    flushCallerEventStream(callId);

    const body = res.body();
    const a = body.indexOf('AAAA');
    const b = body.indexOf('BBBB');
    const c = body.indexOf('CCCC');
    const done = body.indexOf('\\"kind\\":\\"done\\"');

    expect(a).toBeGreaterThanOrEqual(0);
    expect(b).toBeGreaterThan(a);
    expect(c).toBeGreaterThan(b);
    expect(done).toBeGreaterThan(c);
  });

  it('forwards stream_frame payload verbatim as frame_json string', () => {
    const callId = TEST_CALL_IDS[2];

    const frameJson = JSON.stringify({
      kind: 'reconnect',
      reason: 'relay_wall_clock',
      stdout_offset: 1024,
      stderr_offset: 512,
    });

    const res = new FakeServerResponse();
    registerCallerEventStream(callId, res as any);
    pushToCallerStream(callId, 'stream_frame', {
      call_id: callId,
      frame_json: frameJson,
    });

    const body = res.body();
    expect(body).toContain('event: stream_frame');
    // Inner JSON's quotes are escaped when wrapped into the outer JSON.
    expect(body).toContain('\\"kind\\":\\"reconnect\\"');
    expect(body).toContain('\\"stdout_offset\\":1024');
    expect(body).toContain('\\"stderr_offset\\":512');
  });
});
