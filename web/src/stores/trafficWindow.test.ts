import { describe, expect, it } from "vitest";
import type { TrafficSummary } from "../types";
import {
  MAX_TRAFFIC_WINDOW_RECORDS,
  getTrafficWindowBoundaries,
  mergeBoundedTrafficWindow,
} from "./trafficWindow";

const makeRecord = (sequence: number): TrafficSummary => ({
  id: `traffic-${sequence}`,
  sequence,
  timestamp: sequence,
  method: "GET",
  url: `http://example.test/${sequence}`,
  status: 200,
  content_type: "text/plain",
  request_size: 0,
  response_size: 2,
  duration_ms: 1,
  host: "example.test",
  path: `/${sequence}`,
  protocol: "HTTP/1.1",
  client_ip: "127.0.0.1",
  has_rule_hit: false,
  matched_rule_count: 0,
  matched_protocols: [],
  start_time: "2026-08-05T00:00:00Z",
  end_time: "2026-08-05T00:00:00Z",
});

const makeRange = (start: number, end: number): TrafficSummary[] =>
  Array.from({ length: end - start + 1 }, (_, index) =>
    makeRecord(start + index),
  );

describe("bounded traffic window", () => {
  it("keeps the oldest side when older pages exceed the hard limit", () => {
    const current = makeRange(1001, 2000);
    const older = makeRange(501, 1000);

    const result = mergeBoundedTrafficWindow(current, older, "older");

    expect(result.records).toHaveLength(MAX_TRAFFIC_WINDOW_RECORDS);
    expect(result.records[0]?.sequence).toBe(501);
    expect(result.records.at(-1)?.sequence).toBe(1500);
    expect(result.trimmed).toBe(500);
    expect(result.trimmedSide).toBe("newer");
  });

  it("keeps the newest side when forward pages exceed the hard limit", () => {
    const current = makeRange(501, 1500);
    const newer = makeRange(1501, 2000);

    const result = mergeBoundedTrafficWindow(current, newer, "newer");

    expect(result.records).toHaveLength(MAX_TRAFFIC_WINDOW_RECORDS);
    expect(result.records[0]?.sequence).toBe(1001);
    expect(result.records.at(-1)?.sequence).toBe(2000);
    expect(result.trimmed).toBe(500);
    expect(result.trimmedSide).toBe("older");
  });

  it("deduplicates page overlap and lets the incoming record replace stale data", () => {
    const stale = makeRecord(2);
    const updated = { ...makeRecord(2), status: 204 };

    const result = mergeBoundedTrafficWindow(
      [makeRecord(1), stale, makeRecord(3)],
      [updated, makeRecord(4)],
      "newer",
      10,
    );

    expect(result.records.map((record) => record.sequence)).toEqual([1, 2, 3, 4]);
    expect(result.records.find((record) => record.sequence === 2)?.status).toBe(204);
    expect(result.trimmed).toBe(0);
  });

  it("reports independent display boundaries for the current sliding window", () => {
    expect(getTrafficWindowBoundaries([])).toEqual({
      oldestSequence: null,
      newestSequence: null,
      newestId: null,
    });
    expect(getTrafficWindowBoundaries(makeRange(40, 42))).toEqual({
      oldestSequence: 40,
      newestSequence: 42,
      newestId: "traffic-42",
    });
  });
});
