import { describe, expect, it, vi } from "vitest";
import type {
  FilterCondition,
  ToolbarFilters,
  TrafficQueryResponse,
  TrafficSummary,
} from "../types";
import { scanBoundedTrafficMatches } from "./boundedTrafficFilter";

const toolbar: ToolbarFilters = {
  rule: [],
  protocol: [],
  type: [],
  status: [],
  imported: [],
};

const pathCondition: FilterCondition[] = [
  {
    id: "historic-target",
    field: "path",
    operator: "contains",
    value: "/historic-target",
  },
];

const panel = {
  clientIps: [],
  proxyPorts: [],
  clientApps: [],
  accountNames: [],
  domains: [],
};

const makeRecord = (sequence: number, path: string): TrafficSummary => ({
  id: `traffic-${sequence}`,
  sequence,
  timestamp: sequence,
  method: "GET",
  url: `http://example.test${path}`,
  status: 200,
  content_type: "text/plain",
  request_size: 0,
  response_size: 2,
  duration_ms: 1,
  host: "example.test",
  path,
  protocol: "HTTP/1.1",
  client_ip: "127.0.0.1",
  has_rule_hit: false,
  matched_rule_count: 0,
  matched_protocols: [],
  start_time: "2026-08-05T00:00:00Z",
  end_time: "2026-08-05T00:00:00Z",
});

const page = (
  records: TrafficSummary[],
  hasMore: boolean,
): TrafficQueryResponse => ({
  records: records
    .slice()
    .reverse()
    .map((record) => ({
      id: record.id,
      seq: record.sequence,
      ts: record.timestamp,
      m: record.method,
      h: record.host,
      p: record.path,
      s: record.status,
      ct: record.content_type,
      req_ct: record.request_content_type,
      req_sz: record.request_size,
      res_sz: record.response_size,
      up: record.upload_bytes ?? record.request_size,
      down: record.download_bytes ?? record.response_size,
      dur: record.duration_ms,
      lp: record.listener_port ?? 0,
      proto: record.protocol,
      cip: record.client_ip,
      capp: record.client_app,
      cpid: record.client_pid,
      acct: record.account_name,
      flags: 0,
      fc: record.frame_count ?? 0,
      st: record.start_time,
      et: record.end_time,
      rc: record.matched_rule_count,
      rp: record.matched_protocols,
    })),
  next_cursor: records[0]?.sequence ?? null,
  prev_cursor: records.at(-1)?.sequence ?? null,
  has_more: hasMore,
  total: records.length,
  server_sequence: records.at(-1)?.sequence ?? 0,
});

describe("bounded full-history traffic filtering", () => {
  it("releases non-matching pages and still finds a match outside the initial window", async () => {
    const latestNoise = Array.from({ length: 500 }, (_, index) =>
      makeRecord(1001 + index, `/noise-${index}`),
    );
    const historic = [
      makeRecord(999, "/historic-target/one"),
      makeRecord(1000, "/other"),
    ];
    const fetchPage = vi
      .fn()
      .mockResolvedValueOnce(page(latestNoise, true))
      .mockResolvedValueOnce(page(historic, false));

    const result = await scanBoundedTrafficMatches({
      fetchPage,
      toolbar,
      conditions: pathCondition,
      panel,
      targetMatches: 500,
      maxResults: 2_000,
      isCurrent: () => true,
      yieldToBrowser: async () => {},
    });

    expect(fetchPage).toHaveBeenCalledTimes(2);
    expect(result.records.map((record) => record.path)).toEqual([
      "/historic-target/one",
    ]);
    expect(result.hasMore).toBe(false);
    expect(result.scannedCount).toBe(502);
  });

  it("stops committing when a newer filter generation replaces the scan", async () => {
    let current = true;
    const fetchPage = vi.fn().mockImplementation(async () => {
      current = false;
      return page([makeRecord(1, "/historic-target/one")], false);
    });

    const result = await scanBoundedTrafficMatches({
      fetchPage,
      toolbar,
      conditions: pathCondition,
      panel,
      targetMatches: 500,
      maxResults: 2_000,
      isCurrent: () => current,
      yieldToBrowser: async () => {},
    });

    expect(result.cancelled).toBe(true);
    expect(result.records).toEqual([]);
  });
});
