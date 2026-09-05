import { describe, expect, it } from 'vitest';
import type { TrafficRecord } from '../types';
import { buildReplayRequestFromTrafficRecord } from './useReplayStore';

const trafficRecord = (requestHeaders: [string, string][]): TrafficRecord => ({
  id: 'REQ-compressed',
  sequence: 1,
  timestamp: 1,
  host: 'example.test',
  method: 'POST',
  url: 'https://example.test/api',
  path: '/api',
  status: 200,
  protocol: 'https',
  content_type: 'application/json',
  request_content_type: 'application/json',
  request_size: 32,
  response_size: 2,
  upload_bytes: 32,
  download_bytes: 2,
  duration_ms: 1,
  listener_port: 8800,
  client_ip: '127.0.0.1',
  has_rule_hit: false,
  matched_rule_count: 0,
  matched_protocols: [],
  start_time: new Date(0).toISOString(),
  request_headers: requestHeaders,
  response_headers: [],
  request_body: null,
  response_body: null,
  matched_rules: null,
});

describe('buildReplayRequestFromTrafficRecord', () => {
  it('imports decoded body without stale content encoding or length headers', () => {
    const { request } = buildReplayRequestFromTrafficRecord(
      trafficRecord([
        ['Content-Type', 'application/json'],
        ['Content-Encoding', 'gzip'],
        ['CONTENT-LENGTH', '32'],
        ['X-Test', 'keep'],
      ]),
      '{"value":1}',
    );

    expect(request.headers.map(({ key, value }) => [key, value])).toEqual([
      ['Content-Type', 'application/json'],
      ['X-Test', 'keep'],
    ]);
    expect(request.body).toEqual({ type: 'raw', raw_type: 'json', content: '{"value":1}' });
  });
});
