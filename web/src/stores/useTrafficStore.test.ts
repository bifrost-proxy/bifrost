import { describe, expect, it, vi } from "vitest";
import type {
  FilterCondition,
  ToolbarFilters,
  TrafficQueryResponse,
  TrafficSummary,
  TrafficDeltaData,
} from "../types";

const apiMocks = vi.hoisted(() => ({
  getTrafficPage: vi.fn(),
}));

vi.mock("../api", () => apiMocks);

vi.mock("../services/pushService", () => ({
  default: {
    onTrafficUpdates: vi.fn(),
    onTrafficDelta: vi.fn(),
    onTrafficDeleted: vi.fn(),
    updateSubscription: vi.fn(),
    connect: vi.fn(),
    disconnectIfIdle: vi.fn(),
  },
}));

import {
  filterRecords,
  isFilterConditionApplicable,
  useTrafficStore,
} from "./useTrafficStore";
import { MAX_TRAFFIC_WINDOW_RECORDS } from "./trafficWindow";

const toolbar: ToolbarFilters = {
  rule: [],
  protocol: [],
  type: [],
  status: [],
  imported: [],
};

const makeRecord = (
  id: string,
  path: string,
  overrides: Partial<TrafficSummary> = {},
): TrafficSummary => ({
  id,
  sequence: Number(id),
  timestamp: 1,
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
  start_time: "2026-05-09T00:00:00Z",
  end_time: "2026-05-09T00:00:00Z",
  ...overrides,
});

describe("Traffic filter condition enabled state", () => {
  it("ignores disabled filter conditions", () => {
    const records = [makeRecord("1", "/keep"), makeRecord("2", "/other")];
    const conditions: FilterCondition[] = [
      {
        id: "disabled-path",
        field: "path",
        operator: "contains",
        value: "/keep",
        enabled: false,
      },
    ];

    expect(filterRecords(records, toolbar, conditions)).toEqual(records);
    expect(isFilterConditionApplicable(conditions[0])).toBe(false);
  });

  it("treats legacy conditions without enabled as active", () => {
    const records = [makeRecord("1", "/keep"), makeRecord("2", "/other")];
    const conditions: FilterCondition[] = [
      {
        id: "legacy-path",
        field: "path",
        operator: "contains",
        value: "/keep",
      },
    ];

    expect(filterRecords(records, toolbar, conditions)).toEqual([records[0]]);
    expect(isFilterConditionApplicable(conditions[0])).toBe(true);
  });

  it("filters records by selected proxy port panel filters", () => {
    const records = [
      makeRecord("1", "/main", { listener_port: 9900 }),
      makeRecord("2", "/temp", { listener_port: 58344 }),
      makeRecord("3", "/other-temp", { listener_port: 58345 }),
    ];

    expect(
      filterRecords(records, toolbar, [], {
        clientIps: [],
        proxyPorts: ["58344"],
        clientApps: [],
        accountNames: [],
        domains: [],
      }),
    ).toEqual([records[1]]);
  });

  it("filters records by selected account name panel filters", () => {
    const records = [
      makeRecord("1", "/main", { account_name: "alice" }),
      makeRecord("2", "/temp", { account_name: "bob" }),
      makeRecord("3", "/other-temp"),
    ];

    expect(
      filterRecords(records, toolbar, [], {
        clientIps: [],
        proxyPorts: [],
        clientApps: [],
        accountNames: ["bob"],
        domains: [],
      }),
    ).toEqual([records[1]]);
  });
});

const makePage = (
  records: TrafficSummary[],
  hasMore: boolean,
  direction: "backward" | "forward",
): TrafficQueryResponse => ({
  records: (direction === "backward" ? records.slice().reverse() : records).map(
    (record) => ({
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
    }),
  ),
  next_cursor: records.at(-1)?.sequence ?? null,
  prev_cursor: records[0]?.sequence ?? null,
  has_more: hasMore,
  total: 3_000,
  server_sequence: 3_000,
});

const makeRecordRange = (start: number, end: number): TrafficSummary[] =>
  Array.from({ length: end - start + 1 }, (_, index) =>
    makeRecord(String(start + index), `/${start + index}`),
  );

const makeDelta = (
  records: TrafficSummary[],
  serverTotal: number,
  serverSequence: number,
  oldestSequence: number,
): TrafficDeltaData => ({
  inserts: makePage(records, false, "forward").records,
  updates: [],
  has_more: false,
  server_total: serverTotal,
  server_sequence: serverSequence,
  oldest_sequence: oldestSequence,
});

const flushTrafficBatch = () =>
  new Promise<void>((resolve) => window.setTimeout(resolve, 180));

describe("Traffic store bounded history paging", () => {
  it("loads one older page, trims the newer side, and keeps the live cursor monotonic", async () => {
    const current = makeRecordRange(1001, 3000);
    apiMocks.getTrafficPage.mockResolvedValueOnce(
      makePage(makeRecordRange(501, 1000), true, "backward"),
    );
    useTrafficStore.setState({
      records: current,
      recordsMap: new Map(current.map((record) => [record.id, record])),
      hasMore: true,
      hasNewer: false,
      oldestSequence: 1001,
      lastSequence: 3000,
      lastId: "3000",
      historyLoading: false,
    });

    await useTrafficStore.getState().backfillHistory();

    const state = useTrafficStore.getState();
    expect(state.records).toHaveLength(MAX_TRAFFIC_WINDOW_RECORDS);
    expect(state.records[0]?.sequence).toBe(501);
    expect(state.records.at(-1)?.sequence).toBe(2500);
    expect(state.recordsMap.size).toBe(MAX_TRAFFIC_WINDOW_RECORDS);
    expect(state.hasNewer).toBe(true);
    expect(state.lastSequence).toBe(3000);
    expect(state.lastId).toBe("3000");
  });

  it("loads forward from a historical window and trims the older side", async () => {
    const current = makeRecordRange(501, 2500);
    apiMocks.getTrafficPage.mockResolvedValueOnce(
      makePage(makeRecordRange(2501, 3000), false, "forward"),
    );
    useTrafficStore.setState({
      records: current,
      recordsMap: new Map(current.map((record) => [record.id, record])),
      hasMore: false,
      hasNewer: true,
      oldestSequence: 501,
      lastSequence: 3000,
      lastId: "3000",
      historyLoading: false,
    });

    await useTrafficStore.getState().loadNewer();

    const state = useTrafficStore.getState();
    expect(state.records).toHaveLength(MAX_TRAFFIC_WINDOW_RECORDS);
    expect(state.records[0]?.sequence).toBe(1001);
    expect(state.records.at(-1)?.sequence).toBe(3000);
    expect(state.hasMore).toBe(true);
    expect(state.hasNewer).toBe(false);
    expect(state.lastSequence).toBe(3000);
  });
});

describe("Traffic store rolling retention and burst catch-up", () => {
  it("applies a floor-only delta when rolling retention deletes old rows", async () => {
    const current = makeRecordRange(1, 100);
    useTrafficStore.setState({
      records: current,
      recordsMap: new Map(current.map((record) => [record.id, record])),
      serverTotal: 100,
      serverSequence: 101,
      serverOldestSequence: 1,
      hasMore: false,
      hasNewer: false,
      lastSequence: 100,
      lastId: "100",
      autoScroll: true,
      newRecordsCount: 0,
    });

    useTrafficStore.getState().handleTrafficDelta({
      inserts: [],
      updates: [],
      has_more: false,
      server_total: 80,
      server_sequence: 101,
      oldest_sequence: 21,
    });
    await flushTrafficBatch();

    const state = useTrafficStore.getState();
    expect(state.records).toHaveLength(80);
    expect(state.records[0]?.sequence).toBe(21);
    expect(state.recordsMap.size).toBe(80);
    expect(state.serverOldestSequence).toBe(21);
    expect(state.lastSequence).toBe(100);
  });

  it("drops records below the server oldest-sequence floor in the latest window", async () => {
    const current = makeRecordRange(1, 1000);
    useTrafficStore.setState({
      records: current,
      recordsMap: new Map(current.map((record) => [record.id, record])),
      serverTotal: 1000,
      serverSequence: 1001,
      serverOldestSequence: 1,
      hasMore: false,
      hasNewer: false,
      lastSequence: 1000,
      lastId: "1000",
      autoScroll: true,
      newRecordsCount: 0,
    });

    useTrafficStore.getState().handleTrafficDelta(
      makeDelta(makeRecordRange(1001, 1500), 900, 1501, 601),
    );
    await flushTrafficBatch();

    const state = useTrafficStore.getState();
    expect(state.records).toHaveLength(900);
    expect(state.records[0]?.sequence).toBe(601);
    expect(state.records.at(-1)?.sequence).toBe(1500);
    expect(state.recordsMap.size).toBe(900);
    expect(state.serverOldestSequence).toBe(601);
    expect(state.serverSequence).toBe(1501);
    expect(state.lastSequence).toBe(1500);
  });

  it("removes evicted rows without inserting live records into a historical window", async () => {
    const current = makeRecordRange(1, 1000);
    useTrafficStore.setState({
      records: current,
      recordsMap: new Map(current.map((record) => [record.id, record])),
      serverTotal: 1000,
      serverSequence: 1001,
      serverOldestSequence: 1,
      hasMore: false,
      hasNewer: true,
      lastSequence: 1000,
      lastId: "1000",
      autoScroll: false,
      newRecordsCount: 0,
    });

    useTrafficStore.getState().handleTrafficDelta(
      makeDelta(makeRecordRange(1001, 1500), 900, 1501, 601),
    );
    await flushTrafficBatch();

    const state = useTrafficStore.getState();
    expect(state.records).toHaveLength(400);
    expect(state.records[0]?.sequence).toBe(601);
    expect(state.records.at(-1)?.sequence).toBe(1000);
    expect(state.hasNewer).toBe(true);
    expect(state.newRecordsCount).toBe(500);
    expect(state.lastSequence).toBe(1500);
  });

  it("coalesces a 5000-record wake-up burst into the latest bounded server window", async () => {
    const current = makeRecordRange(1, 500);
    useTrafficStore.setState({
      records: current,
      recordsMap: new Map(current.map((record) => [record.id, record])),
      serverTotal: 500,
      serverSequence: 501,
      serverOldestSequence: 1,
      hasMore: false,
      hasNewer: false,
      lastSequence: 500,
      lastId: "500",
      autoScroll: true,
      newRecordsCount: 0,
    });

    for (let start = 501; start <= 5000; start += 500) {
      const end = start + 499;
      const floor = Math.max(1, end - 999);
      useTrafficStore.getState().handleTrafficDelta(
        makeDelta(makeRecordRange(start, end), 1000, end + 1, floor),
      );
    }
    await flushTrafficBatch();

    const state = useTrafficStore.getState();
    expect(state.records).toHaveLength(1000);
    expect(state.records[0]?.sequence).toBe(4001);
    expect(state.records.at(-1)?.sequence).toBe(5000);
    expect(state.recordsMap.size).toBe(1000);
    expect(state.serverOldestSequence).toBe(4001);
    expect(state.lastSequence).toBe(5000);
    expect(state.serverSequence).toBe(5001);
  });
});
