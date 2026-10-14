import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  FilterCondition,
  ToolbarFilters,
  TrafficQueryResponse,
  TrafficSummary,
  TrafficDeltaData,
  TrafficStatistics,
  TrafficUpdatesResponseCompact,
} from "../types";

const apiMocks = vi.hoisted(() => ({
  getTrafficDetail: vi.fn(),
  getRequestBody: vi.fn(),
  getResponseBody: vi.fn(),
  getTrafficPage: vi.fn(),
  getTrafficUpdates: vi.fn(),
  queryTraffic: vi.fn(),
  clearTraffic: vi.fn().mockResolvedValue(undefined),
  getTrafficStatistics: vi.fn().mockResolvedValue({
    total_requests: 0,
    server_sequence: 0,
    client_ips: {},
    proxy_ports: {},
    applications: {},
    account_names: {},
    domains: {},
  }),
}));

vi.mock("../api", () => apiMocks);

const pushMocks = vi.hoisted(() => ({
  onTrafficUpdates: vi.fn().mockReturnValue(() => {}),
  onTrafficDelta: vi.fn().mockReturnValue(() => {}),
  onTrafficDeleted: vi.fn().mockReturnValue(() => {}),
  onTrafficStatistics: vi.fn().mockReturnValue(() => {}),
  onConnectionChange: vi.fn().mockReturnValue(() => {}),
  updateSubscription: vi.fn(),
  connect: vi.fn(),
  disconnectIfIdle: vi.fn(),
  resetTrafficCursor: vi.fn(),
}));
vi.mock("../services/pushService", () => ({ default: pushMocks }));

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

// Clear reports the DELETE result independently of background reconciliation.
// Existing membership assertions explicitly wait for that separate read.
const clearAndSynchronizeForTest = async (ids?: string[]) => {
  const reload = useTrafficStore.getState().reloadRecords;
  let synchronization = Promise.resolve();
  useTrafficStore.setState({
    reloadRecords: (...args) => {
      synchronization = reload(...args);
      return synchronization;
    },
  });
  try {
    const result = await useTrafficStore.getState().clearTraffic(ids);
    await synchronization;
    return result;
  } finally {
    useTrafficStore.setState({ reloadRecords: reload });
  }
};

describe("Traffic store bounded history paging", () => {
  it("uses the authoritative server snapshot for filter and Activity counts", async () => {
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({
      total_requests: 2_500,
      server_sequence: 2_501,
      client_ips: { "127.0.0.1": 2_400, "10.0.0.2": 100 },
      proxy_ports: { "9900": 2_500 },
      applications: { Codex: 1_750, "Lark Helper": 750 },
      account_names: { eden: 2_500 },
      domains: { "api.example.test": 2_000, "other.test": 500 },
    });

    await useTrafficStore.getState().fetchTrafficStatistics();

    const state = useTrafficStore.getState();
    expect(state.trafficStatisticsLoaded).toBe(true);
    expect(state.trafficStatisticsTotal).toBe(2_500);
    expect(state.trafficStatisticsSequence).toBe(2_501);
    expect(state.clientAppCounts.get("Codex")).toBe(1_750);
    expect(state.clientIpCounts.get("10.0.0.2")).toBe(100);
    expect(state.proxyPortCounts.get("9900")).toBe(2_500);
    expect(state.accountNameCounts.get("eden")).toBe(2_500);
    expect(state.domainCounts.get("api.example.test")).toBe(2_000);

    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 2_501,
      server_sequence: 2_502,
      client_ips: { "127.0.0.1": 2_401, "10.0.0.2": 100 },
      proxy_ports: { "9900": 2_501 },
      applications: { Codex: 1_751, "Lark Helper": 750 },
      account_names: { eden: 2_501 },
      domains: { "api.example.test": 2_001, "other.test": 500 },
    });

    const pushedState = useTrafficStore.getState();
    expect(pushedState.trafficStatisticsTotal).toBe(2_501);
    expect(pushedState.trafficStatisticsSequence).toBe(2_502);
    expect(pushedState.clientAppCounts.get("Codex")).toBe(1_751);
    expect(pushedState.domainCounts.get("api.example.test")).toBe(2_001);
  });

  it("does not synthesize statistics while an authoritative clear update is pending", async () => {
    const records = makeRecordRange(1, 2);
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((record) => [record.id, record])),
    });
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 2,
      server_sequence: 2,
      client_ips: { "127.0.0.1": 2 },
      proxy_ports: { "9900": 2 },
      applications: { Codex: 2 },
      account_names: {},
      domains: { "api.example.test": 2 },
    });

    await clearAndSynchronizeForTest();

    const state = useTrafficStore.getState();
    expect(state.records).toEqual([]);
    expect(state.trafficStatisticsTotal).toBe(2);
    expect(state.trafficStatisticsSequence).toBe(2);
    expect(state.clientAppCounts.get("Codex")).toBe(2);
    expect(state.domainCounts.get("api.example.test")).toBe(2);
    expect(apiMocks.clearTraffic).toHaveBeenCalled();
  });

  it("loads one older page, trims the newer side, and keeps the live cursor monotonic", async () => {
    const current = makeRecordRange(1001, 2000);
    apiMocks.getTrafficPage.mockResolvedValueOnce(
      makePage(makeRecordRange(501, 1000), true, "backward"),
    );
    useTrafficStore.setState({
      records: current,
      recordsMap: new Map(current.map((record) => [record.id, record])),
      hasMore: true,
      hasNewer: false,
      oldestSequence: 1001,
      lastSequence: 2000,
      lastId: "2000",
      historyLoading: false,
    });

    await useTrafficStore.getState().backfillHistory();

    const state = useTrafficStore.getState();
    expect(state.records).toHaveLength(MAX_TRAFFIC_WINDOW_RECORDS);
    expect(state.records[0]?.sequence).toBe(501);
    expect(state.records.at(-1)?.sequence).toBe(1500);
    expect(state.recordsMap.size).toBe(MAX_TRAFFIC_WINDOW_RECORDS);
    expect(state.hasNewer).toBe(true);
    expect(state.lastSequence).toBe(2000);
    expect(state.lastId).toBe("2000");
  });

  it("loads forward from a historical window and trims the older side", async () => {
    const current = makeRecordRange(501, 1500);
    apiMocks.getTrafficPage.mockResolvedValueOnce(
      makePage(makeRecordRange(1501, 2000), false, "forward"),
    );
    useTrafficStore.setState({
      records: current,
      recordsMap: new Map(current.map((record) => [record.id, record])),
      hasMore: false,
      hasNewer: true,
      oldestSequence: 501,
      lastSequence: 2000,
      lastId: "2000",
      historyLoading: false,
    });

    await useTrafficStore.getState().loadNewer();

    const state = useTrafficStore.getState();
    expect(state.records).toHaveLength(MAX_TRAFFIC_WINDOW_RECORDS);
    expect(state.records[0]?.sequence).toBe(1001);
    expect(state.records.at(-1)?.sequence).toBe(2000);
    expect(state.hasMore).toBe(true);
    expect(state.hasNewer).toBe(false);
    expect(state.lastSequence).toBe(2000);
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

    useTrafficStore
      .getState()
      .handleTrafficDelta(
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

    useTrafficStore
      .getState()
      .handleTrafficDelta(
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
      useTrafficStore
        .getState()
        .handleTrafficDelta(
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

describe("Traffic store missed-deletion reconciliation", () => {
  beforeEach(async () => {
    useTrafficStore.getState().disablePush();
    await clearAndSynchronizeForTest();
    useTrafficStore.setState(useTrafficStore.getInitialState());
    apiMocks.queryTraffic.mockReset();
    pushMocks.resetTrafficCursor.mockClear();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  const seed = () => {
    const records = makeRecordRange(1, 3);
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((record) => [record.id, record])),
      serverTotal: 3,
      serverSequence: 4,
      serverOldestSequence: 1,
      oldestSequence: 1,
      lastSequence: 3,
      lastId: "3",
      selectedId: "2",
      requestBody: "deleted request",
      responseBody: "deleted response",
      requestRawBody: { data: "request bytes", size: 13 },
      responseRawBody: { data: "response bytes", size: 14 },
    });
    return records;
  };

  const emptyStatistics: TrafficStatistics = {
    total_requests: 0,
    server_sequence: 4,
    client_ips: {},
    proxy_ports: {},
    applications: {},
    account_names: {},
    domains: {},
  };

  it.each(["queued", "committed"])(
    "finishes a bounded reconnect with older history after %s chunks",
    async (phase) => {
      const records = makeRecordRange(1001, 2000);
      const filters: FilterCondition[] = [
        { id: "filter", field: "path", operator: "contains", value: "/1500" },
      ];
      useTrafficStore.setState({
        records,
        recordsMap: new Map(records.map((record) => [record.id, record])),
        serverTotal: 2000,
        serverSequence: 2001,
        serverOldestSequence: 1,
        lastSequence: 2000,
        lastId: "2000",
        hasMore: true,
        hasNewer: true,
        filterConditions: filters,
      });
      useTrafficStore.getState().enablePush();
      const connectionChanged = pushMocks.onConnectionChange.mock.calls.at(-1)![0];
      connectionChanged({ connected: false });
      connectionChanged({ connected: true });
      apiMocks.queryTraffic.mockImplementation(async ({ record_ids }) => {
        const survivors = records.filter(
          (record) => record_ids.includes(record.id) && record.id !== "1500",
        );
        return { ...makePage(survivors, false, "forward"), total: survivors.length };
      });
      // The bounded initial query has older history beyond its 1,000-row tail.
      // Every chunk therefore has has_more=true, including the final one.
      for (const [start, end] of [[2001, 2500], [2501, 3000]]) {
        useTrafficStore.getState().handleTrafficDelta({
          ...makeDelta(makeRecordRange(start, end), 2999, 3001, 1),
          has_more: true,
        });
        if (phase === "committed") await flushTrafficBatch();
      }
      expect(apiMocks.queryTraffic).not.toHaveBeenCalled();
      useTrafficStore.getState().handleTrafficStatistics({
        ...emptyStatistics,
        total_requests: 2999,
        server_sequence: 3001,
      });
      await flushTrafficBatch();

      const state = useTrafficStore.getState();
      expect(state.records.map((record) => record.id)).toEqual(
        records.filter((record) => record.id !== "1500").map((record) => record.id),
      );
      expect(state.serverTotal).toBe(2999);
      expect(state.lastSequence).toBe(3000);
      expect(state.hasMore).toBe(true);
      expect(state.hasNewer).toBe(true);
      expect(state.filterConditions).toEqual(filters);
      expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(2);
      for (const [query] of apiMocks.queryTraffic.mock.calls) {
        expect(Object.keys(query).sort()).toEqual(["limit", "record_ids"]);
        expect(query.record_ids).toHaveLength(500);
        expect(query.limit).toBe(500);
      }
    },
  );

  it.each(["queued", "committed", "full window", "deleted window"])(
    "accepts low-sequence traffic after an empty restart with %s old traffic",
    async (phase) => {
      const records = phase === "full window"
        ? makeRecordRange(1001, 2000)
        : makeRecordRange(101, 103);
      useTrafficStore.setState({
        records,
        recordsMap: new Map(records.map((record) => [record.id, record])),
        serverTotal: records.length,
        serverSequence: records.at(-1)!.sequence + 1,
        serverOldestSequence: records[0].sequence,
        lastSequence: records.at(-1)!.sequence,
        lastId: records.at(-1)!.id,
        hasMore: true,
        hasNewer: true,
        pendingIds: new Set(["103"]),
      });
      useTrafficStore.getState().enablePush();
      if (phase === "queued" || phase === "committed") {
        useTrafficStore.getState().handleTrafficDelta({
          ...makeDelta(makeRecordRange(104, 104), 4, 105, 101),
          has_more: true,
        });
      }
      if (phase === "committed") await flushTrafficBatch();
      if (phase === "deleted window") {
        useTrafficStore.getState().handleTrafficDeleted(records.map((record) => record.id));
      }
      const connectionChanged = pushMocks.onConnectionChange.mock.calls.at(-1)![0];
      connectionChanged({ connected: false });
      connectionChanged({ connected: true });
      apiMocks.getTrafficStatistics.mockResolvedValueOnce({
        ...emptyStatistics,
        server_sequence: 1,
      });
      useTrafficStore.getState().handleTrafficStatistics({
        ...emptyStatistics,
        server_sequence: 1,
      });
      await flushTrafficBatch();

      const emptyState = useTrafficStore.getState();
      useTrafficStore.getState().handleTrafficStatistics({ ...emptyStatistics, server_sequence: 1 });
      const [arrival] = makeRecordRange(1, 1);
      useTrafficStore.getState().handleTrafficDelta(makeDelta([arrival], 1, 2, 1));
      useTrafficStore.getState().handleTrafficStatistics({
        ...emptyStatistics, total_requests: 1, server_sequence: 2,
      });
      // A late zero snapshot from before this arrival cannot reset it again.
      useTrafficStore.getState().handleTrafficStatistics({
        ...emptyStatistics,
        server_sequence: 1,
      });
      await flushTrafficBatch();
      const state = useTrafficStore.getState();
      expect(emptyState.records).toEqual([]);
      expect(emptyState.lastSequence).toBeNull();
      expect(emptyState.lastId).toBeNull();
      expect(emptyState.serverSequence).toBe(1);
      expect(emptyState.serverOldestSequence).toBeNull();
      expect(emptyState.hasMore).toBe(false);
      expect(emptyState.hasNewer).toBe(false);
      expect(emptyState.pendingIds.size).toBe(0);
      expect(pushMocks.resetTrafficCursor).toHaveBeenCalledTimes(1);
      expect(apiMocks.queryTraffic).not.toHaveBeenCalled();
      expect(state.records.map((record) => record.id)).toEqual(["1"]);
      expect(state.recordsMap.size).toBe(1);
      expect(state.serverTotal).toBe(1);
      expect(state.trafficStatisticsTotal).toBe(1);
      expect(state.serverSequence).toBe(2);
      expect(state.serverOldestSequence).toBe(1);
      expect(state.lastSequence).toBe(1);
      expect(state.lastId).toBe("1");
      expect(pushMocks.updateSubscription).toHaveBeenLastCalledWith({
        last_traffic_id: "1", last_sequence: 1, pending_ids: [],
      });
      useTrafficStore.getState().disablePush();
      useTrafficStore.getState().enablePush();
      expect(pushMocks.connect).toHaveBeenLastCalledWith({
        last_traffic_id: "1", last_sequence: 1, pending_ids: [], need_traffic: true,
      });
    },
  );

  it.each(["reconnect", "shared-socket resume"])(
    "removes all stale rows after %s when the server sends no traffic delta",
    async (kind) => {
      const records = seed();
      const filters: FilterCondition[] = [
        { id: "filter", field: "path", operator: "contains", value: "/2" },
      ];
      useTrafficStore.setState({
        pendingIds: new Set(["2", "3"]),
        hasMore: true,
        hasNewer: true,
        filterConditions: filters,
        currentRecord: {
          ...records[1],
          request_headers: [],
          response_headers: [],
          request_body: "deleted request",
          response_body: "deleted response",
          request_content_type: null,
          matched_rules: [],
        },
      });
      useTrafficStore.getState().enablePush();
      if (kind === "reconnect") {
        const connectionChanged =
          pushMocks.onConnectionChange.mock.calls.at(-1)![0];
        connectionChanged({ connected: false });
        connectionChanged({ connected: true });
      } else {
        useTrafficStore.getState().disablePush();
        useTrafficStore.getState().enablePush();
        // Other subscriptions keep this socket open, so no connected event fires.
      }
      apiMocks.queryTraffic.mockResolvedValueOnce({
        ...makePage([], false, "forward"),
        total: 0,
      });
      const statisticsReceived =
        pushMocks.onTrafficStatistics.mock.calls.at(-1)![0];
      // Empty storage suppresses traffic_delta, but initial statistics are sent.
      statisticsReceived(emptyStatistics);
      await flushTrafficBatch();

      const state = useTrafficStore.getState();
      expect(state.records).toEqual([]);
      expect(state.recordsMap.size).toBe(0);
      expect(state.pendingIds.size).toBe(0);
      expect(state.serverTotal).toBe(0);
      expect(state.trafficStatisticsTotal).toBe(0);
      expect(state.trafficStatisticsSequence).toBe(4);
      expect(state.serverSequence).toBe(4);
      expect(state.currentRecord).toBeNull();
      expect(state.selectedId).toBeUndefined();
      expect(state.requestBody).toBeNull();
      expect(state.responseBody).toBeNull();
      expect(state.requestRawBody).toBeNull();
      expect(state.responseRawBody).toBeNull();
      expect(state.recordsMutation.deletedIds).toEqual(["1", "2", "3"]);
      expect(state.filterConditions).toEqual(filters);
      expect(state.hasMore).toBe(true);
      expect(state.hasNewer).toBe(true);
      expect(state.lastSequence).toBe(3);
      expect(state.lastId).toBe("3");
      expect(apiMocks.queryTraffic).toHaveBeenCalledExactlyOnceWith({
        record_ids: ["1", "2", "3"],
        limit: 3,
      });
    },
  );

  it.each([
    { resume: "reconnect", phase: "no delta" },
    { resume: "reconnect", phase: "queued" },
    { resume: "reconnect", phase: "committed" },
    { resume: "shared-socket resume", phase: "committed" },
  ])("recovers a populated replacement database after $resume with $phase metadata", async ({ resume, phase }) => {
    const records = makeRecordRange(1001, 2000);
    const filters: FilterCondition[] = [
      { id: "filter", field: "path", operator: "contains", value: "/50" },
    ];
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((record) => [record.id, record])),
      serverTotal: 1000,
      serverSequence: 2001,
      serverOldestSequence: 1001,
      oldestSequence: 1001,
      lastSequence: 2000,
      lastId: "2000",
      filterConditions: filters,
      hasMore: true,
      hasNewer: true,
      pendingIds: new Set(["2000"]),
    });
    useTrafficStore.getState().enablePush();
    if (resume === "reconnect") {
      const connectionChanged = pushMocks.onConnectionChange.mock.calls.at(-1)![0];
      connectionChanged({ connected: false });
      connectionChanged({ connected: true });
    } else {
      useTrafficStore.getState().disablePush();
      useTrafficStore.getState().enablePush();
    }
    const replacement = { ...emptyStatistics, total_requests: 600, server_sequence: 601 };
    apiMocks.getTrafficStatistics.mockResolvedValueOnce(replacement);
    const replacementRecords = makeRecordRange(1, 601);
    apiMocks.queryTraffic.mockImplementation(async ({ record_ids }: { record_ids: string[] }) => {
      const matches = replacementRecords.filter((record) => record_ids.includes(record.id));
      return { ...makePage(matches, false, "forward"), total: matches.length };
    });
    const statisticsCalls = apiMocks.getTrafficStatistics.mock.calls.length;
    if (phase !== "no delta") {
      // The server retains the old subscription cursor and filters every new
      // record, but still sends current metadata before initial statistics.
      useTrafficStore.getState().handleTrafficDelta(makeDelta([], 600, 601, 1));
    }
    if (phase === "committed") {
      await flushTrafficBatch();
      expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(2);
      expect(useTrafficStore.getState().records).toEqual([]);
      expect(useTrafficStore.getState().lastSequence).toBe(2000);
    }
    useTrafficStore.getState().handleTrafficStatistics(replacement);
    await flushTrafficBatch();

    const reset = useTrafficStore.getState();
    expect(pushMocks.resetTrafficCursor).toHaveBeenCalledTimes(1);
    expect(reset.records).toEqual([]);
    expect(reset.recordsMap.size).toBe(0);
    expect(reset.lastSequence).toBeNull();
    expect(reset.lastId).toBeNull();
    expect(reset.serverSequence).toBe(601);
    expect(reset.serverOldestSequence).toBeNull();
    expect(reset.pendingIds.size).toBe(0);
    expect(reset.serverTotal).toBe(600);
    expect(reset.trafficStatisticsTotal).toBe(600);
    expect(reset.trafficEpochVersion).toBe(1);
    expect(reset.hasMore).toBe(true);
    expect(reset.hasNewer).toBe(false);
    expect(reset.filterConditions).toEqual(filters);

    // Cursor-free replay recovers existing rows, then newly created traffic
    // advances the replacement epoch's cursor for the next reconnect.
    useTrafficStore.getState().handleTrafficDelta(makeDelta(makeRecordRange(101, 600), 600, 601, 1));
    await flushTrafficBatch();
    useTrafficStore.getState().handleTrafficDelta(makeDelta(makeRecordRange(601, 601), 601, 602, 1));
    useTrafficStore.getState().handleTrafficStatistics({ ...replacement, total_requests: 601, server_sequence: 602 });
    useTrafficStore.getState().handleTrafficStatistics(replacement);
    await flushTrafficBatch();
    const state = useTrafficStore.getState();
    expect(state.records.map((record) => record.sequence)).toEqual(makeRecordRange(101, 601).map((record) => record.sequence));
    expect(state.lastSequence).toBe(601);
    expect(state.serverSequence).toBe(602);
    expect(state.serverTotal).toBe(601);
    expect(state.trafficEpochVersion).toBe(1);
    expect(pushMocks.resetTrafficCursor).toHaveBeenCalledTimes(1);
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    expect(pushMocks.connect).toHaveBeenLastCalledWith({
      last_traffic_id: "601", last_sequence: 601, pending_ids: [], need_traffic: true,
    });
    useTrafficStore.getState().handleTrafficStatistics({ ...replacement, total_requests: 601, server_sequence: 602 });
    await flushTrafficBatch();
    expect(pushMocks.resetTrafficCursor).toHaveBeenCalledTimes(1);
    expect(apiMocks.getTrafficStatistics.mock.calls.length - statisticsCalls).toBe(1);
    expect(useTrafficStore.getState().records).toEqual(state.records);
  });

  it.each([0, 1])("keeps the current epoch when a fresh read disproves stale statistics with total %i", async (total) => {
    const records = seed();
    useTrafficStore.setState({ hasMore: true, hasNewer: true });
    useTrafficStore.getState().enablePush();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({
      ...emptyStatistics, total_requests: 3,
    });
    apiMocks.queryTraffic.mockResolvedValueOnce({
      ...makePage(records, false, "forward"), total: 3,
    });
    const calls = apiMocks.getTrafficStatistics.mock.calls.length;
    useTrafficStore.getState().handleTrafficStatistics({ ...emptyStatistics, total_requests: total, server_sequence: total + 1 });
    await flushTrafficBatch();
    const state = useTrafficStore.getState();
    expect(apiMocks.getTrafficStatistics.mock.calls.length - calls).toBe(1);
    expect(state.records).toEqual(records);
    expect(state.serverTotal).toBe(3);
    expect(state.serverSequence).toBe(4);
    expect(state.lastSequence).toBe(3);
    expect(state.serverOldestSequence).toBe(1);
    expect(state.hasMore).toBe(true);
    expect(state.hasNewer).toBe(true);
    expect(pushMocks.resetTrafficCursor).not.toHaveBeenCalled();
  });

  it.each([0, 1])("preserves the window after confirmation failure and retries later statistics with total %i", async (total) => {
    const records = seed();
    useTrafficStore.getState().enablePush();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    apiMocks.getTrafficStatistics.mockRejectedValueOnce(new Error("offline"));
    const restarted = { ...emptyStatistics, total_requests: total, server_sequence: total + 1 };
    useTrafficStore.getState().handleTrafficStatistics(restarted);
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records).toEqual(records);
    expect(useTrafficStore.getState().lastSequence).toBe(3);
    expect(pushMocks.resetTrafficCursor).not.toHaveBeenCalled();
    apiMocks.getTrafficStatistics.mockResolvedValueOnce(restarted);
    useTrafficStore.getState().handleTrafficStatistics(restarted);
    useTrafficStore.getState().handleTrafficStatistics(restarted);
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(pushMocks.resetTrafficCursor).toHaveBeenCalledTimes(1);
  });

  it("recovers an arrival created before restart confirmation returns", async () => {
    seed();
    useTrafficStore.getState().enablePush();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    let resolveStatistics!: (statistics: TrafficStatistics) => void;
    apiMocks.getTrafficStatistics.mockImplementationOnce(
      () => new Promise<TrafficStatistics>((resolve) => { resolveStatistics = resolve; }),
    );
    useTrafficStore.getState().handleTrafficStatistics({ ...emptyStatistics, server_sequence: 1 });
    // The stale server-side cursor filtered out the first insert, but this fresh
    // read proves the new epoch even though the database is no longer empty.
    resolveStatistics({ ...emptyStatistics, total_requests: 1, server_sequence: 2 });
    await flushTrafficBatch();
    expect(pushMocks.resetTrafficCursor).toHaveBeenCalledTimes(1);
    expect(useTrafficStore.getState().serverTotal).toBe(1);
    useTrafficStore.getState().handleTrafficDelta(makeDelta(makeRecordRange(1, 1), 1, 2, 1));
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records.map((record) => record.id)).toEqual(["1"]);
    expect(useTrafficStore.getState().lastSequence).toBe(1);
  });

  it("starts a fresh confirmation when another reconnect interrupts the previous read", async () => {
    seed();
    useTrafficStore.getState().enablePush();
    const connectionChanged = pushMocks.onConnectionChange.mock.calls.at(-1)![0];
    connectionChanged({ connected: false });
    connectionChanged({ connected: true });
    let resolveOld!: (statistics: TrafficStatistics) => void;
    let resolveCurrent!: (statistics: TrafficStatistics) => void;
    apiMocks.getTrafficStatistics.mockImplementationOnce(
      () => new Promise<TrafficStatistics>((resolve) => { resolveOld = resolve; }),
    ).mockImplementationOnce(
      () => new Promise<TrafficStatistics>((resolve) => { resolveCurrent = resolve; }),
    );
    const restarted = { ...emptyStatistics, server_sequence: 1 };
    useTrafficStore.getState().handleTrafficStatistics(restarted);
    connectionChanged({ connected: false });
    connectionChanged({ connected: true });
    useTrafficStore.getState().handleTrafficStatistics(restarted);
    resolveOld({ ...emptyStatistics, total_requests: 3 });
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records).toHaveLength(3);
    useTrafficStore.getState().handleTrafficStatistics(restarted);
    resolveCurrent(restarted);
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(useTrafficStore.getState().lastSequence).toBeNull();
    expect(useTrafficStore.getState().serverSequence).toBe(1);
    expect(pushMocks.resetTrafficCursor).toHaveBeenCalledTimes(1);
    expect(apiMocks.queryTraffic).not.toHaveBeenCalled();
  });

  it.each([0, 1])("retains a newer epoch snapshot while confirming a replacement with total %i", async (total) => {
    seed();
    useTrafficStore.getState().enablePush();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    let resolveStatistics!: (statistics: TrafficStatistics) => void;
    apiMocks.getTrafficStatistics.mockImplementationOnce(
      () => new Promise<TrafficStatistics>((resolve) => { resolveStatistics = resolve; }),
    ).mockResolvedValueOnce({ ...emptyStatistics, total_requests: total + 1, server_sequence: total + 2 });
    const calls = apiMocks.getTrafficStatistics.mock.calls.length;
    useTrafficStore.getState().handleTrafficStatistics({ ...emptyStatistics, total_requests: total, server_sequence: total + 1 });
    // The old server cursor filters the insert, but metadata/statistics advance.
    useTrafficStore.getState().handleTrafficDelta(makeDelta([], total + 1, total + 2, 1));
    useTrafficStore.getState().handleTrafficStatistics({
      ...emptyStatistics, total_requests: total + 1, server_sequence: total + 2,
    });
    resolveStatistics({ ...emptyStatistics, total_requests: total, server_sequence: total + 1 });
    await flushTrafficBatch();
    expect(apiMocks.getTrafficStatistics.mock.calls.length - calls).toBe(2);
    expect(pushMocks.resetTrafficCursor).toHaveBeenCalledTimes(1);
    useTrafficStore.getState().handleTrafficDelta(makeDelta(makeRecordRange(1, total + 1), total + 1, total + 2, 1));
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records.map((record) => record.id)).toEqual(makeRecordRange(1, total + 1).map((record) => record.id));
    expect(useTrafficStore.getState().lastSequence).toBe(total + 1);
  });

  it.each([0, 1])("does not apply a confirmation with total %i captured before newer same-epoch traffic", async (total) => {
    const records = seed();
    useTrafficStore.getState().enablePush();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    let resolveStatistics!: (statistics: TrafficStatistics) => void;
    apiMocks.getTrafficStatistics.mockImplementationOnce(
      () => new Promise<TrafficStatistics>((resolve) => { resolveStatistics = resolve; }),
    );
    useTrafficStore.getState().handleTrafficStatistics({ ...emptyStatistics, total_requests: total, server_sequence: total + 1 });
    const [arrival] = makeRecordRange(4, 4);
    useTrafficStore.getState().handleTrafficDelta({
      ...makeDelta([arrival], 4, 5, 1), has_more: true,
    });
    resolveStatistics({ ...emptyStatistics, total_requests: total, server_sequence: total + 1 });
    await flushTrafficBatch();
    expect(pushMocks.resetTrafficCursor).not.toHaveBeenCalled();
    expect(useTrafficStore.getState().records.map((record) => record.id)).toEqual(["1", "2", "3", "4"]);
    apiMocks.queryTraffic.mockResolvedValueOnce({
      ...makePage([...records, arrival], false, "forward"), total: 4,
    });
    useTrafficStore.getState().handleTrafficStatistics({
      ...emptyStatistics, total_requests: 4, server_sequence: 5,
    });
    await flushTrafficBatch();
    expect(useTrafficStore.getState().serverTotal).toBe(4);
    expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(1);
  });

  it("ignores a previous epoch's membership response after resetting", async () => {
    const records = seed();
    useTrafficStore.getState().enablePush();
    let resolveQuery!: (page: TrafficQueryResponse) => void;
    apiMocks.queryTraffic.mockImplementationOnce(
      () => new Promise<TrafficQueryResponse>((resolve) => { resolveQuery = resolve; }),
    );
    useTrafficStore.getState().handleTrafficDelta(makeDelta([], 1, 4, 1));
    await flushTrafficBatch();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({ ...emptyStatistics, server_sequence: 1 });
    useTrafficStore.getState().handleTrafficStatistics({ ...emptyStatistics, server_sequence: 1 });
    await flushTrafficBatch();
    useTrafficStore.getState().handleTrafficDelta(makeDelta(makeRecordRange(1, 1), 1, 2, 1));
    await flushTrafficBatch();
    resolveQuery({ ...makePage([records[1]], false, "forward"), total: 1 });
    await useTrafficStore.getState().reconcileTrafficWindow();
    expect(useTrafficStore.getState().records.map((record) => record.id)).toEqual(["1"]);
    expect(useTrafficStore.getState().lastSequence).toBe(1);
  });

  it("ignores a jump-to-live response from the previous database epoch", async () => {
    const records = seed();
    useTrafficStore.setState({ hasNewer: true });
    let resolveReload!: (response: TrafficUpdatesResponseCompact) => void;
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () => new Promise<TrafficUpdatesResponseCompact>((resolve) => { resolveReload = resolve; }),
    );
    const reload = useTrafficStore.getState().reloadRecords();
    useTrafficStore.getState().enablePush();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    const replacement = { ...emptyStatistics, total_requests: 1, server_sequence: 2 };
    apiMocks.getTrafficStatistics.mockResolvedValueOnce(replacement);
    useTrafficStore.getState().handleTrafficStatistics(replacement);
    await flushTrafficBatch();
    const arrival = makeRecord("replacement", "/new", { sequence: 1 });
    useTrafficStore.getState().handleTrafficDelta(makeDelta([arrival], 1, 2, 1));
    await flushTrafficBatch();
    const recovered = useTrafficStore.getState();
    expect(recovered.records.map((record) => record.id)).toEqual(["replacement"]);
    const subscriptionUpdates = pushMocks.updateSubscription.mock.calls.length;
    resolveReload({
      new_records: makePage(records, false, "forward").records,
      updated_records: [],
      has_more: true,
      server_total: 3,
      server_sequence: 4,
    });
    await reload;
    expect(useTrafficStore.getState().records).toEqual(recovered.records);
    expect(useTrafficStore.getState().recordsMutation).toEqual(recovered.recordsMutation);
    expect(useTrafficStore.getState().serverTotal).toBe(1);
    expect(useTrafficStore.getState().serverSequence).toBe(2);
    expect(useTrafficStore.getState().lastSequence).toBe(1);
    expect(useTrafficStore.getState().lastId).toBe("replacement");
    expect(pushMocks.updateSubscription.mock.calls.length).toBe(subscriptionUpdates);
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    expect(pushMocks.connect).toHaveBeenLastCalledWith({
      last_traffic_id: "replacement", last_sequence: 1, pending_ids: [], need_traffic: true,
    });
  });

  it("still applies a jump-to-live response within the current database epoch", async () => {
    seed();
    useTrafficStore.setState({ hasNewer: true });
    const records = makeRecordRange(4, 6);
    apiMocks.getTrafficUpdates.mockResolvedValueOnce({
      new_records: makePage(records, false, "forward").records,
      updated_records: [],
      has_more: true,
      server_total: 6,
      server_sequence: 7,
    });
    await useTrafficStore.getState().reloadRecords();
    const state = useTrafficStore.getState();
    expect(state.records.map((record) => record.id)).toEqual(["4", "5", "6"]);
    expect(state.serverTotal).toBe(6);
    expect(state.serverSequence).toBe(7);
    expect(state.lastSequence).toBe(6);
    expect(state.hasNewer).toBe(false);
    expect(state.trafficEpochVersion).toBe(0);
    expect(pushMocks.updateSubscription).toHaveBeenLastCalledWith({
      last_traffic_id: "6", last_sequence: 6, pending_ids: [],
    });
  });

  it("ignores ordinary statistics responses captured before an epoch reset", async () => {
    seed();
    let resolveStatistics!: (statistics: TrafficStatistics) => void;
    apiMocks.getTrafficStatistics.mockImplementationOnce(
      () => new Promise<TrafficStatistics>((resolve) => { resolveStatistics = resolve; }),
    );
    const pending = useTrafficStore.getState().fetchTrafficStatistics();
    useTrafficStore.getState().enablePush();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    const restarted = { ...emptyStatistics, server_sequence: 1 };
    apiMocks.getTrafficStatistics.mockResolvedValueOnce(restarted);
    useTrafficStore.getState().handleTrafficStatistics(restarted);
    await flushTrafficBatch();
    resolveStatistics({ ...emptyStatistics, total_requests: 3 });
    await pending;
    expect(useTrafficStore.getState().trafficStatisticsTotal).toBe(0);
    expect(useTrafficStore.getState().trafficStatisticsSequence).toBe(1);
    expect(useTrafficStore.getState().trafficEpochVersion).toBe(1);
  });

  it("waits for fresh confirmation before treating lower nonzero statistics as a restart", async () => {
    const records = seed();
    useTrafficStore.getState().enablePush();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    let resolveStatistics!: (statistics: TrafficStatistics) => void;
    apiMocks.getTrafficStatistics.mockImplementationOnce(
      () => new Promise<TrafficStatistics>((resolve) => { resolveStatistics = resolve; }),
    );
    const calls = apiMocks.getTrafficStatistics.mock.calls.length;
    useTrafficStore.getState().handleTrafficStatistics({
      ...emptyStatistics, total_requests: 1, server_sequence: 2,
    });
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records).toEqual(records);
    expect(useTrafficStore.getState().serverTotal).toBe(3);
    expect(useTrafficStore.getState().lastSequence).toBe(3);
    expect(apiMocks.getTrafficStatistics.mock.calls.length - calls).toBe(1);
    expect(apiMocks.queryTraffic).not.toHaveBeenCalled();
    expect(pushMocks.resetTrafficCursor).not.toHaveBeenCalled();

    apiMocks.queryTraffic.mockResolvedValueOnce({
      ...makePage(records, false, "forward"), total: 3,
    });
    resolveStatistics({ ...emptyStatistics, total_requests: 3 });
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records).toEqual(records);
    expect(useTrafficStore.getState().serverSequence).toBe(4);
    expect(useTrafficStore.getState().trafficEpochVersion).toBe(0);
    expect(pushMocks.resetTrafficCursor).not.toHaveBeenCalled();
  });

  it("keeps concurrent arrivals outside an empty-reconnect membership query", async () => {
    seed();
    useTrafficStore.getState().enablePush();
    const connectionChanged =
      pushMocks.onConnectionChange.mock.calls.at(-1)![0];
    connectionChanged({ connected: false });
    connectionChanged({ connected: true });
    let resolveQuery!: (page: TrafficQueryResponse) => void;
    apiMocks.queryTraffic.mockImplementationOnce(
      () =>
        new Promise<TrafficQueryResponse>((resolve) => {
          resolveQuery = resolve;
        }),
    );
    useTrafficStore.getState().handleTrafficStatistics(emptyStatistics);
    await flushTrafficBatch();
    expect(apiMocks.queryTraffic).toHaveBeenCalledExactlyOnceWith({
      record_ids: ["1", "2", "3"],
      limit: 3,
    });

    useTrafficStore
      .getState()
      .handleTrafficDelta(makeDelta(makeRecordRange(4, 4), 1, 5, 4));
    useTrafficStore.getState().handleTrafficStatistics({
      ...emptyStatistics,
      total_requests: 1,
      server_sequence: 5,
    });
    await flushTrafficBatch();
    resolveQuery({ ...makePage([], false, "forward"), total: 0 });
    await useTrafficStore.getState().reconcileTrafficWindow();

    const state = useTrafficStore.getState();
    expect(state.records.map((record) => record.id)).toEqual(["4"]);
    expect(state.recordsMap.has("4")).toBe(true);
    expect(state.serverTotal).toBe(1);
    expect(state.trafficStatisticsTotal).toBe(1);
    expect(state.lastSequence).toBe(4);
    expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(1);
  });

  it.each([
    { hasMore: false, serverSequence: 5 },
    { hasMore: true, serverSequence: 5 },
    { hasMore: false, serverSequence: 1 },
    { hasMore: true, serverSequence: 1 },
  ])(
    "reconciles an interrupted backlog without a final delta: %j",
    async ({ hasMore, serverSequence }) => {
      seed();
      useTrafficStore.getState().enablePush();
      useTrafficStore.getState().handleTrafficDelta({
        ...makeDelta(makeRecordRange(4, 4), 4, 5, 1),
        has_more: hasMore,
      });
      // The old batch is still waiting for RAF when Traffic is interrupted.
      useTrafficStore.getState().disablePush();
      useTrafficStore.getState().enablePush();
      apiMocks.queryTraffic.mockResolvedValueOnce({
        ...makePage([], false, "forward"),
        total: 0,
      });
      if (serverSequence === 1) {
        apiMocks.getTrafficStatistics.mockResolvedValueOnce({ ...emptyStatistics, server_sequence: 1 });
      }
      useTrafficStore.getState().handleTrafficStatistics({
        ...emptyStatistics,
        // Restarting with empty storage resets the server sequence to 1.
        server_sequence: serverSequence,
      });
      await flushTrafficBatch();

      const state = useTrafficStore.getState();
      expect(state.records).toEqual([]);
      expect(state.recordsMap.size).toBe(0);
      expect(state.serverTotal).toBe(0);
      expect(state.trafficStatisticsTotal).toBe(0);
      expect(state.lastSequence).toBe(serverSequence === 1 ? null : 4);
      if (serverSequence === 1) {
        expect(apiMocks.queryTraffic).not.toHaveBeenCalled();
      } else {
        expect(apiMocks.queryTraffic).toHaveBeenCalledExactlyOnceWith({
          record_ids: ["1", "2", "3", "4"],
          limit: 4,
        });
      }
    },
  );

  it.each(["queued", "committed"])(
    "does not finish a newer %s backlog using older empty statistics",
    async (phase) => {
      const records = seed();
      const [arrival] = makeRecordRange(4, 4);
      useTrafficStore.getState().enablePush();
      const connectionChanged =
        pushMocks.onConnectionChange.mock.calls.at(-1)![0];
      connectionChanged({ connected: false });
      connectionChanged({ connected: true });
      apiMocks.queryTraffic.mockResolvedValueOnce({
        ...makePage([records[0], records[2], arrival], false, "forward"),
        total: 3,
      });
      useTrafficStore.getState().handleTrafficDelta({
        ...makeDelta([arrival], 3, 5, 1),
        has_more: true,
      });
      if (phase === "committed") await flushTrafficBatch();
      // A periodic statistics snapshot can be captured before a newer delta.
      useTrafficStore.getState().handleTrafficStatistics(emptyStatistics);
      await flushTrafficBatch();
      expect(useTrafficStore.getState().serverTotal).toBe(3);
      expect(apiMocks.queryTraffic).not.toHaveBeenCalled();

      useTrafficStore.getState().handleTrafficDelta(makeDelta([], 3, 5, 1));
      await flushTrafficBatch();
      expect(
        useTrafficStore.getState().records.map((record) => record.id),
      ).toEqual(["1", "3", "4"]);
      expect(useTrafficStore.getState().serverTotal).toBe(3);
      expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(1);
    },
  );

  it.each(["reconnect", "shared-socket resume"])(
    "reconciles arrivals queued before %s when no rows have committed yet",
    async (kind) => {
      useTrafficStore.getState().enablePush();
      useTrafficStore.getState().handleTrafficDelta({
        ...makeDelta(makeRecordRange(1, 1), 1, 2, 1),
        has_more: true,
      });
      if (kind === "reconnect") {
        const connectionChanged =
          pushMocks.onConnectionChange.mock.calls.at(-1)![0];
        connectionChanged({ connected: false });
        connectionChanged({ connected: true });
      } else {
        useTrafficStore.getState().disablePush();
        useTrafficStore.getState().enablePush();
      }
      apiMocks.queryTraffic.mockResolvedValueOnce({
        ...makePage([], false, "forward"),
        total: 0,
      });
      useTrafficStore.getState().handleTrafficStatistics({
        ...emptyStatistics,
        server_sequence: 2,
      });
      await flushTrafficBatch();

      expect(useTrafficStore.getState().records).toEqual([]);
      expect(useTrafficStore.getState().serverTotal).toBe(0);
      expect(apiMocks.queryTraffic).toHaveBeenCalledExactlyOnceWith({
        record_ids: ["1"],
        limit: 1,
      });
    },
  );

  it("checks an empty reconnect in at most two 500-ID queries", async () => {
    const records = makeRecordRange(1, MAX_TRAFFIC_WINDOW_RECORDS);
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((record) => [record.id, record])),
      serverTotal: records.length,
    });
    useTrafficStore.getState().enablePush();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    apiMocks.queryTraffic.mockResolvedValue({
      ...makePage([], false, "forward"),
      total: 0,
    });
    useTrafficStore.getState().handleTrafficStatistics(emptyStatistics);
    useTrafficStore.getState().handleTrafficStatistics(emptyStatistics);
    await flushTrafficBatch();
    useTrafficStore.getState().handleTrafficStatistics(emptyStatistics);
    await flushTrafficBatch();

    expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(2);
    for (const [query] of apiMocks.queryTraffic.mock.calls) {
      expect(Object.keys(query).sort()).toEqual(["limit", "record_ids"]);
      expect(query.record_ids).toHaveLength(500);
      expect(query.limit).toBe(500);
    }
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(useTrafficStore.getState().serverTotal).toBe(0);
  });

  it("does not query membership for uninterrupted statistics updates", async () => {
    seed();
    useTrafficStore.getState().enablePush();
    const connectionChanged =
      pushMocks.onConnectionChange.mock.calls.at(-1)![0];
    connectionChanged({ connected: true });
    useTrafficStore.getState().handleTrafficStatistics({
      ...emptyStatistics,
      total_requests: 3,
    });
    useTrafficStore.getState().handleTrafficDeleted(["1", "2", "3"]);
    useTrafficStore.getState().handleTrafficStatistics(emptyStatistics);
    await flushTrafficBatch();
    expect(apiMocks.queryTraffic).not.toHaveBeenCalled();
    expect(useTrafficStore.getState().serverTotal).toBe(0);
    expect(useTrafficStore.getState().trafficStatisticsTotal).toBe(0);
  });

  it("keeps the oldest survivor instead of guessing which rows disappeared offline", async () => {
    const [keep] = seed();
    apiMocks.queryTraffic.mockResolvedValueOnce({
      ...makePage([keep], false, "forward"),
      total: 1,
    });

    useTrafficStore.getState().handleTrafficDelta(makeDelta([], 1, 4, 1));
    await flushTrafficBatch();

    const state = useTrafficStore.getState();
    expect(state.records.map((record) => record.id)).toEqual(["1"]);
    expect(state.recordsMap.has("1")).toBe(true);
    expect(state.recordsMap.has("2")).toBe(false);
    expect(state.recordsMap.has("3")).toBe(false);
    expect(state.serverTotal).toBe(1);
    expect(state.lastSequence).toBe(3);
    expect(state.lastId).toBe("3");
    expect(state.selectedId).toBeUndefined();
    expect(state.requestBody).toBeNull();
    expect(state.responseBody).toBeNull();
    expect(state.requestRawBody).toBeNull();
    expect(state.responseRawBody).toBeNull();
    expect(state.recordsMutation.deletedIds).toEqual(["2", "3"]);
    expect(apiMocks.queryTraffic).toHaveBeenCalledExactlyOnceWith({
      record_ids: ["1", "2", "3"],
      limit: 3,
    });
  });

  it("reconciles equal-total replacements after an automatic reconnect, once backlog ends", async () => {
    const records = seed();
    useTrafficStore.setState({ serverTotal: 100, hasMore: true });
    useTrafficStore.getState().enablePush();
    const connectionChanged =
      pushMocks.onConnectionChange.mock.calls.at(-1)![0];
    connectionChanged({ connected: true });
    useTrafficStore.getState().handleTrafficDelta(makeDelta([], 100, 4, 1));
    await flushTrafficBatch();
    expect(apiMocks.queryTraffic).not.toHaveBeenCalled();

    connectionChanged({ connected: false });
    connectionChanged({ connected: true });
    const [arrival] = makeRecordRange(4, 4);
    apiMocks.queryTraffic.mockResolvedValueOnce({
      ...makePage([records[0], records[2], arrival], false, "forward"),
      total: 3,
    });
    useTrafficStore.getState().handleTrafficDelta({
      ...makeDelta([arrival], 100, 5, 1),
      has_more: true,
    });
    await flushTrafficBatch();
    expect(apiMocks.queryTraffic).not.toHaveBeenCalled();
    useTrafficStore.getState().handleTrafficDelta(makeDelta([], 100, 5, 1));
    await flushTrafficBatch();
    expect(apiMocks.queryTraffic).toHaveBeenCalledExactlyOnceWith({
      record_ids: ["1", "2", "3", "4"],
      limit: 4,
    });
    const state = useTrafficStore.getState();
    expect(state.records.map((record) => record.id)).toEqual(["1", "3", "4"]);
    expect(state.serverTotal).toBe(100);
    expect(state.lastSequence).toBe(4);
    expect(state.hasMore).toBe(true);
  });

  it("rechecks after another reconnect finishes during an in-flight lookup", async () => {
    const records = seed();
    useTrafficStore.setState({ serverTotal: 100 });
    useTrafficStore.getState().enablePush();
    const connectionChanged =
      pushMocks.onConnectionChange.mock.calls.at(-1)![0];
    let resolveQuery!: (page: TrafficQueryResponse) => void;
    apiMocks.queryTraffic.mockImplementationOnce(
      () =>
        new Promise<TrafficQueryResponse>((resolve) => {
          resolveQuery = resolve;
        }),
    );
    apiMocks.queryTraffic.mockResolvedValueOnce({
      ...makePage([records[0]], false, "forward"),
      total: 1,
    });
    connectionChanged({ connected: false });
    connectionChanged({ connected: true });
    useTrafficStore.getState().handleTrafficDelta(makeDelta([], 100, 4, 1));
    await flushTrafficBatch();
    connectionChanged({ connected: false });
    connectionChanged({ connected: true });
    useTrafficStore.getState().handleTrafficDelta(makeDelta([], 100, 4, 1));
    await flushTrafficBatch();
    expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(1);
    resolveQuery({
      ...makePage([records[0], records[2]], false, "forward"),
      total: 2,
    });
    await flushTrafficBatch();
    expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(2);
    expect(
      useTrafficStore.getState().records.map((record) => record.id),
    ).toEqual(["1"]);
    expect(useTrafficStore.getState().serverTotal).toBe(100);
  });

  it("checks cached membership after push is disabled and resumed", async () => {
    const [keep] = seed();
    useTrafficStore.setState({ serverTotal: 100 });
    useTrafficStore.getState().enablePush();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    apiMocks.queryTraffic.mockResolvedValueOnce({
      ...makePage([keep], false, "forward"),
      total: 1,
    });
    // Two unseen inserts elsewhere offset the two deleted cached records.
    useTrafficStore.getState().handleTrafficDelta(makeDelta([], 100, 6, 1));
    await flushTrafficBatch();
    expect(
      useTrafficStore.getState().records.map((record) => record.id),
    ).toEqual(["1"]);
    expect(useTrafficStore.getState().serverTotal).toBe(100);
    expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(1);
  });

  it("reconciles missed deletes when only part of the server history is loaded", async () => {
    const records = seed();
    useTrafficStore.setState({ serverTotal: 100, hasMore: true });
    apiMocks.queryTraffic.mockResolvedValueOnce({
      ...makePage([records[0], records[2]], false, "forward"),
      total: 2,
    });
    useTrafficStore.getState().handleTrafficDelta(makeDelta([], 99, 4, 1));
    await flushTrafficBatch();
    const state = useTrafficStore.getState();
    expect(state.records.map((record) => record.id)).toEqual(["1", "3"]);
    expect(state.serverTotal).toBe(99);
    expect(state.hasMore).toBe(true);
    expect(state.selectedId).toBeUndefined();
    expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(1);
  });

  it.each(["append", "retention"])(
    "does not query membership for ordinary %s deltas",
    async (kind) => {
      seed();
      const delta =
        kind === "append"
          ? makeDelta(makeRecordRange(4, 5), 5, 6, 1)
          : makeDelta([], 2, 4, 2);
      useTrafficStore.getState().handleTrafficDelta(delta);
      await flushTrafficBatch();
      expect(apiMocks.queryTraffic).not.toHaveBeenCalled();
      expect(
        useTrafficStore.getState().records.map((record) => record.id),
      ).toEqual(kind === "append" ? ["1", "2", "3", "4", "5"] : ["2", "3"]);
    },
  );

  it("does not query when retention only removes history outside the loaded window", async () => {
    const records = makeRecordRange(501, 1000);
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((record) => [record.id, record])),
      serverTotal: 1000,
      serverSequence: 1001,
      serverOldestSequence: 1,
      lastSequence: 1000,
      lastId: "1000",
    });
    useTrafficStore
      .getState()
      .handleTrafficDelta(makeDelta([], 500, 1001, 501));
    await flushTrafficBatch();
    expect(apiMocks.queryTraffic).not.toHaveBeenCalled();
    expect(useTrafficStore.getState().records).toHaveLength(500);
    expect(useTrafficStore.getState().records[0].id).toBe("501");
  });

  it("deduplicates lookups and preserves historical paging and filters", async () => {
    const [keep] = seed();
    const filters: FilterCondition[] = [
      { id: "filter", field: "path", operator: "contains", value: "/1" },
    ];
    useTrafficStore.setState({
      hasMore: true,
      hasNewer: true,
      filterConditions: filters,
    });
    let resolveQuery!: (page: TrafficQueryResponse) => void;
    apiMocks.queryTraffic.mockImplementationOnce(
      () =>
        new Promise<TrafficQueryResponse>((resolve) => {
          resolveQuery = resolve;
        }),
    );
    useTrafficStore.getState().handleTrafficDelta(makeDelta([], 1, 4, 1));
    await flushTrafficBatch();
    const duplicate = useTrafficStore.getState().reconcileTrafficWindow();
    expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(1);
    expect(
      useTrafficStore.getState().records.map((record) => record.id),
    ).toEqual(["1", "2", "3"]);

    // A live update advances the cursor without replacing a historical window.
    useTrafficStore.getState().handleTrafficDelta({
      ...makeDelta([], 2, 5, 1),
      updates: makePage(makeRecordRange(4, 4), false, "forward").records,
    });
    await flushTrafficBatch();
    resolveQuery({ ...makePage([keep], false, "forward"), total: 1 });
    await duplicate;

    const state = useTrafficStore.getState();
    expect(state.records.map((record) => record.id)).toEqual(["1"]);
    expect(state.lastSequence).toBe(4);
    expect(state.serverTotal).toBe(2);
    expect(state.hasMore).toBe(true);
    expect(state.hasNewer).toBe(true);
    expect(state.filterConditions).toEqual(filters);
  });

  it("does not prune an arrival outside the in-flight query", async () => {
    const [keep] = seed();
    let resolveQuery!: (page: TrafficQueryResponse) => void;
    apiMocks.queryTraffic.mockImplementationOnce(
      () =>
        new Promise<TrafficQueryResponse>((resolve) => {
          resolveQuery = resolve;
        }),
    );
    useTrafficStore.getState().handleTrafficDelta(makeDelta([], 1, 4, 1));
    await flushTrafficBatch();
    useTrafficStore
      .getState()
      .handleTrafficDelta(makeDelta(makeRecordRange(4, 4), 2, 5, 1));
    await flushTrafficBatch();
    resolveQuery({ ...makePage([keep], false, "forward"), total: 1 });
    await useTrafficStore.getState().reconcileTrafficWindow();
    const state = useTrafficStore.getState();
    expect(state.records.map((record) => record.id)).toEqual(["1", "4"]);
    expect(state.lastSequence).toBe(4);
    expect(state.serverTotal).toBe(2);
  });

  it.each([
    { has_more: true, total: 1 },
    { has_more: false, total: 2 },
  ])(
    "keeps the window if membership results are incomplete: %j",
    async (metadata) => {
      const [keep] = seed();
      apiMocks.queryTraffic.mockResolvedValueOnce({
        ...makePage([keep], false, "forward"),
        ...metadata,
      });
      useTrafficStore.getState().handleTrafficDelta(makeDelta([], 1, 4, 1));
      await flushTrafficBatch();
      expect(
        useTrafficStore.getState().records.map((record) => record.id),
      ).toEqual(["1", "2", "3"]);
      expect(useTrafficStore.getState().selectedId).toBe("2");
    },
  );

  it("preserves data on query failure and retries an unchanged delta", async () => {
    const [keep] = seed();
    apiMocks.queryTraffic.mockRejectedValueOnce(new Error("offline"));
    useTrafficStore.getState().handleTrafficDelta(makeDelta([], 1, 4, 1));
    await flushTrafficBatch();
    expect(
      useTrafficStore.getState().records.map((record) => record.id),
    ).toEqual(["1", "2", "3"]);
    apiMocks.queryTraffic.mockResolvedValueOnce({
      ...makePage([keep], false, "forward"),
      total: 1,
    });
    useTrafficStore.getState().handleTrafficDelta(makeDelta([], 1, 4, 1));
    await flushTrafficBatch();
    expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(2);
    expect(
      useTrafficStore.getState().records.map((record) => record.id),
    ).toEqual(["1"]);
  });

  it("ignores a membership response from before a clear/reset", async () => {
    const [keep] = seed();
    let resolveQuery!: (page: TrafficQueryResponse) => void;
    apiMocks.queryTraffic.mockImplementationOnce(
      () =>
        new Promise<TrafficQueryResponse>((resolve) => {
          resolveQuery = resolve;
        }),
    );
    const pending = useTrafficStore.getState().reconcileTrafficWindow();
    await clearAndSynchronizeForTest();
    seed();
    resolveQuery({ ...makePage([keep], false, "forward"), total: 1 });
    await pending;
    expect(
      useTrafficStore.getState().records.map((record) => record.id),
    ).toEqual(["1", "2", "3"]);
    expect(useTrafficStore.getState().serverTotal).toBe(3);
  });

  it("checks a full bounded window in complete ID-only chunks", async () => {
    const records = makeRecordRange(1, MAX_TRAFFIC_WINDOW_RECORDS);
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((record) => [record.id, record])),
      serverTotal: records.length,
      serverSequence: records.length + 1,
      serverOldestSequence: 1,
    });
    apiMocks.queryTraffic.mockImplementation(
      async ({ record_ids }: { record_ids: string[] }) => {
        const survivors = records.filter(
          (record) => record_ids.includes(record.id) && record.id !== "2",
        );
        return {
          ...makePage(survivors, false, "forward"),
          total: survivors.length,
        };
      },
    );
    useTrafficStore
      .getState()
      .handleTrafficDelta(
        makeDelta([], records.length - 1, records.length + 1, 1),
      );
    await flushTrafficBatch();
    expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(2);
    for (const [query] of apiMocks.queryTraffic.mock.calls) {
      expect(Object.keys(query).sort()).toEqual(["limit", "record_ids"]);
      expect(query.record_ids).toHaveLength(500);
      expect(query.limit).toBe(500);
    }
    expect(useTrafficStore.getState().records).toHaveLength(
      MAX_TRAFFIC_WINDOW_RECORDS - 1,
    );
    expect(useTrafficStore.getState().recordsMap.has("1")).toBe(true);
    expect(useTrafficStore.getState().recordsMap.has("2")).toBe(false);
    expect(useTrafficStore.getState().serverTotal).toBe(
      MAX_TRAFFIC_WINDOW_RECORDS - 1,
    );
  });
});

describe("Traffic detail selection races", () => {
  it("does not hydrate an old selected record body while the next selection loads", async () => {
    let resolveBody!: (body: string) => void;
    const body = new Promise<string>((resolve) => {
      resolveBody = resolve;
    });
    apiMocks.getRequestBody.mockReturnValue(body);
    apiMocks.getResponseBody.mockReturnValue(body);
    apiMocks.getTrafficDetail.mockResolvedValue({
      ...makeRecord("paused", "/old"),
      request_headers: null,
      response_headers: null,
      request_body: null,
      response_body: null,
      matched_rules: null,
      request_content_type: null,
    });
    useTrafficStore.setState({ selectedId: "paused", currentRecord: null });
    await useTrafficStore.getState().fetchTrafficDetail("paused");
    useTrafficStore.setState({ selectedId: "other" });
    resolveBody("stale body");
    await Promise.resolve();
    expect(useTrafficStore.getState().requestBody).toBeNull();
    expect(useTrafficStore.getState().responseBody).toBeNull();
    apiMocks.getTrafficDetail.mockReset();
    apiMocks.getRequestBody.mockReset();
    apiMocks.getResponseBody.mockReset();
  });
  it.each(["paused", "other"])(
    "ignores a scheduled refresh for an old selection before touching %s detail state",
    async (currentId) => {
      apiMocks.getTrafficDetail.mockClear();
      const current = {
        ...makeRecord(currentId, "/current"),
        request_headers: null,
        response_headers: null,
        request_body: null,
        response_body: null,
        matched_rules: null,
        request_content_type: null,
      };
      useTrafficStore.setState({
        selectedId: "other",
        currentRecord: current,
        detailLoading: false,
        detailError: "current state",
        requestBody: "current request body",
        responseBody: "current response body",
      });
      await useTrafficStore.getState().fetchTrafficDetail("paused");
      expect(apiMocks.getTrafficDetail).not.toHaveBeenCalled();
      expect(useTrafficStore.getState()).toMatchObject({
        selectedId: "other",
        currentRecord: current,
        detailLoading: false,
        detailError: "current state",
        requestBody: "current request body",
        responseBody: "current response body",
      });
    },
  );
  it.each(["other", undefined])(
    "ignores a late detail after selection changes to %s",
    async (selectedId) => {
      let resolveDetail!: (value: ReturnType<typeof makeRecord>) => void;
      apiMocks.getTrafficDetail.mockReturnValue(
        new Promise((resolve) => {
          resolveDetail = resolve;
        }),
      );
      useTrafficStore.setState({ selectedId: "paused", currentRecord: null });
      const pending = useTrafficStore.getState().fetchTrafficDetail("paused");
      useTrafficStore.setState({ selectedId });
      resolveDetail(makeRecord("paused", "/pending"));
      await pending;
      expect(useTrafficStore.getState().currentRecord).toBeNull();
      apiMocks.getTrafficDetail.mockReset();
    },
  );
  it("ignores an old request error after another selection", async () => {
    let rejectDetail!: (reason: Error) => void;
    apiMocks.getTrafficDetail.mockReturnValue(
      new Promise((_, reject) => {
        rejectDetail = reject;
      }),
    );
    useTrafficStore.setState({ selectedId: "paused", currentRecord: null });
    const pending = useTrafficStore.getState().fetchTrafficDetail("paused");
    useTrafficStore.setState({
      selectedId: "other",
      detailError: "keep current detail",
    });
    rejectDetail(new Error("stale failure"));
    await pending;
    expect(useTrafficStore.getState().detailError).toBe("keep current detail");
    apiMocks.getTrafficDetail.mockReset();
  });
});

describe("clear API completion and recovery", () => {
  afterEach(() => {
    useTrafficStore.setState({ clearing: false });
    apiMocks.clearTraffic.mockResolvedValue(undefined);
  });

  it("waits for the API, rejects overlapping calls and preserves rows on failure", async () => {
    const records = [makeRecord("501", "/keep")];
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((r) => [r.id, r])),
      clearing: false,
    });
    let reject!: (error: Error) => void;
    apiMocks.clearTraffic.mockImplementationOnce(
      () =>
        new Promise<void>((_, fail) => {
          reject = fail;
        }),
    );
    const count = apiMocks.clearTraffic.mock.calls.length;
    let finished = false;
    const pending = useTrafficStore
      .getState()
      .clearTraffic()
      .then((result) => {
        finished = true;
        return result;
      });
    expect(useTrafficStore.getState().clearing).toBe(true);
    expect(await useTrafficStore.getState().clearTraffic()).toBe(false);
    expect(apiMocks.clearTraffic.mock.calls.length).toBe(count + 1);
    expect(finished).toBe(false);
    expect(useTrafficStore.getState().records).toEqual(records);
    reject(new Error("clear failed"));
    expect(await pending).toBe(false);
    expect(useTrafficStore.getState().error).toBe("clear failed");
    expect(useTrafficStore.getState().records).toEqual(records);
    expect(useTrafficStore.getState().clearing).toBe(false);
    expect(await useTrafficStore.getState().clearTraffic()).toBe(true);
    expect(useTrafficStore.getState().clearing).toBe(false);
  });

  it("treats an unsuccessful API result as failure", async () => {
    const records = [makeRecord("504", "/keep")];
    useTrafficStore.setState({ records, recordsMap: new Map(records.map(r => [r.id, r])), clearing: false });
    apiMocks.clearTraffic.mockResolvedValueOnce({ success: false, message: "clear rejected" });
    expect(await useTrafficStore.getState().clearTraffic()).toBe(false);
    expect(useTrafficStore.getState().records).toEqual(records);
    expect(useTrafficStore.getState().error).toBe("clear rejected");
    expect(useTrafficStore.getState().clearing).toBe(false);
  });

  it("keeps selected rows when deletion fails", async () => {
    const records = [makeRecord("502", "/keep")];
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((r) => [r.id, r])),
      selectedId: "502",
      clearing: false,
    });
    apiMocks.clearTraffic.mockRejectedValueOnce(new Error("delete failed"));
    expect(await useTrafficStore.getState().clearTraffic(["502"])).toBe(false);
    expect(useTrafficStore.getState().records).toEqual(records);
    expect(useTrafficStore.getState().selectedId).toBe("502");
  });

  it("reconciles records captured between server clear and API completion", async () => {
    apiMocks.getTrafficUpdates.mockResolvedValueOnce({
      new_records: makePage(
        [makeRecord("503", "/after-clear")],
        false,
        "forward",
      ).records,
      updated_records: [],
      has_more: false,
      server_total: 1,
      server_sequence: 504,
    });
    expect(await clearAndSynchronizeForTest()).toBe(true);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "503",
    ]);
    expect(pushMocks.resetTrafficCursor).toHaveBeenCalled();
  });
});


it("retains live records arriving during the clear reconciliation read", async () => {
  useTrafficStore.setState({ clearing: false });
  let resolveRead!: (response: TrafficUpdatesResponseCompact) => void;
  apiMocks.getTrafficUpdates.mockImplementationOnce(() => new Promise<TrafficUpdatesResponseCompact>(resolve => { resolveRead = resolve; }));
  const clear = clearAndSynchronizeForTest();
  await Promise.resolve();
  await Promise.resolve();
  const live = makeRecord("506", "/live-during-read");
  useTrafficStore.setState({ records: [live], recordsMap: new Map([[live.id, live]]), serverSequence: 507 });
  resolveRead({ new_records: makePage([makeRecord("505", "/snapshot")], false, "forward").records, updated_records: [], has_more: false, server_total: 1, server_sequence: 506 });
  expect(await clear).toBe(true);
  expect(useTrafficStore.getState().records.map(r => r.id)).toEqual(["505", "506"]);
  expect(useTrafficStore.getState().lastSequence).toBe(506);
  expect(useTrafficStore.getState().serverSequence).toBe(507);
});

describe("clear reconciliation overlapping membership changes", () => {
  beforeEach(async () => {
    useTrafficStore.getState().disablePush();
    apiMocks.getTrafficUpdates.mockReset();
    apiMocks.clearTraffic.mockResolvedValue(undefined);
    await clearAndSynchronizeForTest();
    useTrafficStore.setState({
      paused: false,
      serverOldestSequence: null,
      trafficDatabaseEpoch: null,
    });
    apiMocks.getTrafficUpdates.mockReset();
    apiMocks.queryTraffic.mockResolvedValue({
      records: [],
      total: 0,
      has_more: false,
      server_sequence: 700,
    });
  });

  const response = (
    records: TrafficSummary[],
    total = records.length,
    sequence = 700,
  ): TrafficUpdatesResponseCompact => ({
    new_records: makePage(records, false, "forward").records,
    updated_records: [],
    has_more: false,
    server_total: total,
    server_sequence: sequence,
  });

  it("does not resurrect an unseen snapshot row deleted by another client", async () => {
    let resolveRead!: (value: TrafficUpdatesResponseCompact) => void;
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(response([]));
    const clear = clearAndSynchronizeForTest();
    await Promise.resolve();
    useTrafficStore.getState().handleTrafficDeleted(["601"]);
    resolveRead(response([makeRecord("601", "/deleted")]));
    expect(await clear).toBe(true);
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(useTrafficStore.getState().serverTotal).toBe(0);
    expect(apiMocks.getTrafficUpdates).toHaveBeenCalledTimes(2);
  });

  it("does not restore snapshot or live rows after another client clears", async () => {
    let resolveRead!: (value: TrafficUpdatesResponseCompact) => void;
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(response([]));
    const clear = clearAndSynchronizeForTest();
    await Promise.resolve();
    const live = makeRecord("602", "/cleared-live");
    useTrafficStore.setState({
      records: [live],
      recordsMap: new Map([[live.id, live]]),
    });
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 0,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    resolveRead(response([makeRecord("601", "/cleared-snapshot")]));
    expect(await clear).toBe(true);
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(useTrafficStore.getState().serverTotal).toBe(0);
  });

  it("uses the retention floor received while reading and preserves newer inserts", async () => {
    let resolveRead!: (value: TrafficUpdatesResponseCompact) => void;
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(
      response([makeRecord("605", "/retained")], 1, 706),
    );
    const clear = clearAndSynchronizeForTest();
    await Promise.resolve();
    useTrafficStore
      .getState()
      .handleTrafficDelta(
        makeDelta([makeRecord("606", "/newer")], 2, 707, 605),
      );
    await flushTrafficBatch();
    resolveRead(
      response(
        [makeRecord("601", "/expired"), makeRecord("605", "/retained")],
        2,
        706,
      ),
    );
    expect(await clear).toBe(true);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "605",
      "606",
    ]);
    expect(useTrafficStore.getState().serverTotal).toBe(2);
    expect(useTrafficStore.getState().serverOldestSequence).toBe(605);
  });

  it("preserves post-delete inserts and updates while retrying a stale read", async () => {
    let resolveRead!: (value: TrafficUpdatesResponseCompact) => void;
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(
      response([makeRecord("605", "/updated", { status: 200 })], 1, 706),
    );
    const clear = clearAndSynchronizeForTest();
    await Promise.resolve();
    useTrafficStore.getState().handleTrafficDeleted(["601"]);
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("606", "/inserted")],
      updated_records: [makeRecord("605", "/updated", { status: 201 })],
      has_more: false,
      server_total: 2,
      server_sequence: 707,
    });
    await flushTrafficBatch();
    resolveRead(
      response(
        [
          makeRecord("601", "/deleted"),
          makeRecord("605", "/updated", { status: 200 }),
        ],
        2,
        706,
      ),
    );
    expect(await clear).toBe(true);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "605",
      "606",
    ]);
    expect(useTrafficStore.getState().recordsMap.get("605")?.status).toBe(201);
    expect(useTrafficStore.getState().serverTotal).toBe(2);
  });

  it("cancels a queued pre-delete insert and its obsolete total", async () => {
    let resolveRead!: (value: TrafficUpdatesResponseCompact) => void;
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(response([]));
    const clear = clearAndSynchronizeForTest();
    await Promise.resolve();
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("601", "/queued-deleted")],
      updated_records: [],
      has_more: false,
      server_total: 1,
      server_sequence: 700,
    });
    useTrafficStore.getState().handleTrafficDeleted(["601"]);
    resolveRead(response([makeRecord("601", "/queued-deleted")]));
    expect(await clear).toBe(true);
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(useTrafficStore.getState().serverTotal).toBe(0);
  });

  it("re-reads a reduced total even without an explicit ID deletion", async () => {
    let resolveRead!: (value: TrafficUpdatesResponseCompact) => void;
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(
      response([makeRecord("605", "/survives")], 1),
    );
    const clear = clearAndSynchronizeForTest();
    await Promise.resolve();
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 1,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    resolveRead(
      response(
        [makeRecord("601", "/removed"), makeRecord("605", "/survives")],
        2,
      ),
    );
    expect(await clear).toBe(true);
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "605",
    ]);
    expect(useTrafficStore.getState().serverTotal).toBe(1);
  });

  it("releases invalidation state after a failed replacement read", async () => {
    let resolveRead!: (value: TrafficUpdatesResponseCompact) => void;
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );
    apiMocks.getTrafficUpdates.mockRejectedValueOnce(
      new Error("replacement unavailable"),
    );
    const clear = clearAndSynchronizeForTest();
    await Promise.resolve();
    useTrafficStore.getState().handleTrafficDeleted(["601"]);
    resolveRead(response([makeRecord("601", "/deleted")]));
    expect(await clear).toBe(true);
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(useTrafficStore.getState().clearing).toBe(false);
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(
      response([makeRecord("601", "/new-incarnation")]),
    );
    await useTrafficStore.getState().reloadRecords(true);
    expect(useTrafficStore.getState().records[0]?.path).toBe(
      "/new-incarnation",
    );
  });

  it("observes a delayed clear packet even after a newer insertion advanced the cursor", async () => {
    let resolveRead!: (value: TrafficUpdatesResponseCompact) => void;
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(
      response([makeRecord("700", "/after-peer-clear")], 1, 701),
    );
    const clear = clearAndSynchronizeForTest();
    await Promise.resolve();
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("700", "/after-peer-clear")],
      updated_records: [],
      has_more: false,
      server_total: 1,
      server_sequence: 701,
    });
    await flushTrafficBatch();
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 0,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    resolveRead(response([makeRecord("601", "/before-peer-clear")], 1, 700));
    expect(await clear).toBe(true);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "700",
    ]);
    expect(useTrafficStore.getState().serverTotal).toBe(1);
    expect(useTrafficStore.getState().serverSequence).toBe(701);
  });

  it("invalidates a replacement snapshot when another deletion overlaps that read", async () => {
    let resolveFirst!: (value: TrafficUpdatesResponseCompact) => void;
    let resolveSecond!: (value: TrafficUpdatesResponseCompact) => void;
    let secondStarted!: () => void;
    const started = new Promise<void>((resolve) => {
      secondStarted = resolve;
    });
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveFirst = resolve;
        }),
    );
    apiMocks.getTrafficUpdates.mockImplementationOnce(() => {
      secondStarted();
      return new Promise((resolve) => {
        resolveSecond = resolve;
      });
    });
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(response([]));
    const clear = clearAndSynchronizeForTest();
    await Promise.resolve();
    useTrafficStore.getState().handleTrafficDeleted(["601"]);
    resolveFirst(response([makeRecord("601", "/first-delete")]));
    await started;
    useTrafficStore.getState().handleTrafficDeleted(["605"]);
    resolveSecond(response([makeRecord("605", "/second-delete")]));
    expect(await clear).toBe(true);
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(useTrafficStore.getState().serverTotal).toBe(0);
    expect(apiMocks.getTrafficUpdates).toHaveBeenCalledTimes(3);
  });

  it("confirms live membership outside a bounded snapshot without losing newer fields", async () => {
    let resolveRead!: (value: TrafficUpdatesResponseCompact) => void;
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(
      response([makeRecord("606", "/latest-window")], 2),
    );
    apiMocks.queryTraffic.mockResolvedValueOnce({
      records: makePage(
        [makeRecord("605", "/outside-window")],
        false,
        "forward",
      ).records,
      total: 1,
      has_more: false,
      server_sequence: 700,
    });
    const clear = clearAndSynchronizeForTest();
    await Promise.resolve();
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("601", "/removed")],
      updated_records: [makeRecord("605", "/outside-window", { status: 201 })],
      has_more: false,
      server_total: 3,
      server_sequence: 700,
    });
    await flushTrafficBatch();
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 2,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    resolveRead(
      response(
        [
          makeRecord("601", "/removed"),
          makeRecord("605", "/outside-window"),
          makeRecord("606", "/latest-window"),
        ],
        3,
      ),
    );
    expect(await clear).toBe(true);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "605",
      "606",
    ]);
    expect(useTrafficStore.getState().recordsMap.get("605")?.status).toBe(201);
    expect(useTrafficStore.getState().serverTotal).toBe(2);
  });

  it("keeps writes received after a membership query snapshot", async () => {
    let resolveRead!: (value: TrafficUpdatesResponseCompact) => void;
    let resolveMembership!: (value: TrafficQueryResponse) => void;
    let membershipStarted!: () => void;
    const started = new Promise<void>((resolve) => {
      membershipStarted = resolve;
    });
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(
      response([makeRecord("606", "/survivor")], 1),
    );
    apiMocks.queryTraffic.mockImplementationOnce(() => {
      membershipStarted();
      return new Promise((resolve) => {
        resolveMembership = resolve;
      });
    });
    const clear = clearAndSynchronizeForTest();
    await Promise.resolve();
    useTrafficStore.getState().handleTrafficPush({
      new_records: [],
      updated_records: [makeRecord("605", "/old")],
      has_more: false,
      server_total: 2,
      server_sequence: 700,
    });
    await flushTrafficBatch();
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 1,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    resolveRead(
      response([makeRecord("605", "/old"), makeRecord("606", "/survivor")], 2),
    );
    await started;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("607", "/new", { sequence: 702 })],
      updated_records: [
        makeRecord("605", "/reinserted", { sequence: 701, status: 201 }),
      ],
      has_more: false,
      server_total: 3,
      server_sequence: 703,
    });
    resolveMembership({
      ...makePage([], false, "forward"),
      total: 0,
      server_sequence: 700,
    });
    expect(await clear).toBe(true);
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "606",
      "605",
      "607",
    ]);
    expect(useTrafficStore.getState().recordsMap.get("605")?.status).toBe(201);
    expect(useTrafficStore.getState().serverTotal).toBe(3);
  });

  it("finishes an empty read under repeated empty statistics", async () => {
    apiMocks.getTrafficUpdates.mockImplementation(() => {
      if (apiMocks.getTrafficUpdates.mock.calls.length > 3)
        throw new Error("non-convergent empty reads");
      useTrafficStore.getState().handleTrafficStatistics({
        total_requests: 0,
        server_sequence: 700,
        client_ips: {},
        proxy_ports: {},
        applications: {},
        account_names: {},
        domains: {},
      });
      return Promise.resolve(response([]));
    });
    expect(await clearAndSynchronizeForTest()).toBe(true);
    expect(apiMocks.getTrafficUpdates).toHaveBeenCalledTimes(1);
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(useTrafficStore.getState().clearing).toBe(false);
  });

  it("does not promote a zero-total cached update after a missing membership snapshot", async () => {
    let resolveRead!: (value: TrafficUpdatesResponseCompact) => void;
    let resolveMembership!: (value: TrafficQueryResponse) => void;
    let membershipStarted!: () => void;
    const started = new Promise<void>((resolve) => {
      membershipStarted = resolve;
    });
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(response([]));
    apiMocks.queryTraffic.mockImplementationOnce(() => {
      membershipStarted();
      return new Promise((resolve) => {
        resolveMembership = resolve;
      });
    });
    const clear = clearAndSynchronizeForTest();
    await Promise.resolve();
    useTrafficStore.getState().handleTrafficPush({
      new_records: [],
      updated_records: [makeRecord("605", "/removed")],
      has_more: false,
      server_total: 1,
      server_sequence: 700,
    });
    await flushTrafficBatch();
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 0,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    resolveRead(response([makeRecord("605", "/removed")]));
    await started;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [],
      updated_records: [makeRecord("605", "/removed", { status: 201 })],
      has_more: false,
      server_total: 0,
      server_sequence: 700,
    });
    resolveMembership({
      ...makePage([], false, "forward"),
      total: 0,
      server_sequence: 700,
    });
    expect(await clear).toBe(true);
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(useTrafficStore.getState().serverTotal).toBe(0);
    expect(useTrafficStore.getState().clearing).toBe(false);
  });

  it("discards nonempty rows even when a post-clear aggregate already reports zero", async () => {
    let resolveRead!: (value: TrafficUpdatesResponseCompact) => void;
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(response([]));
    const clear = clearAndSynchronizeForTest();
    await Promise.resolve();
    useTrafficStore
      .getState()
      .handleTrafficStatistics({
        total_requests: 0,
        server_sequence: 700,
        client_ips: {},
        proxy_ports: {},
        applications: {},
        account_names: {},
        domains: {},
      });
    resolveRead(response([makeRecord("601", "/captured-before-clear")], 0));
    expect(await clear).toBe(true);
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(useTrafficStore.getState().serverTotal).toBe(0);
    expect(apiMocks.getTrafficUpdates).toHaveBeenCalledTimes(2);
  });
});

describe("reconciliation delayed delivery ordering", () => {
  beforeEach(async () => {
    useTrafficStore.getState().disablePush();
    apiMocks.getTrafficUpdates.mockReset();
    apiMocks.clearTraffic.mockResolvedValue(undefined);
    await clearAndSynchronizeForTest();
    useTrafficStore.setState(useTrafficStore.getInitialState());
    apiMocks.getTrafficUpdates.mockReset();
    vi.useFakeTimers({
      toFake: [
        "setTimeout",
        "clearTimeout",
        "requestAnimationFrame",
        "cancelAnimationFrame",
      ],
    });
  });

  afterEach(() => {
    useTrafficStore.getState().disablePush();
    vi.useRealTimers();
  });

  const snapshot = (
    records: TrafficSummary[],
    total: number,
    sequence: number,
  ): TrafficUpdatesResponseCompact => ({
    new_records: makePage(records, false, "forward").records,
    updated_records: [],
    has_more: false,
    server_total: total,
    server_sequence: sequence,
  });

  const holdRead = () => {
    let resolve!: (value: TrafficUpdatesResponseCompact) => void;
    let started!: () => void;
    const ready = new Promise<void>((done) => {
      started = done;
    });
    apiMocks.getTrafficUpdates.mockImplementationOnce(() => {
      started();
      return new Promise((done) => {
        resolve = done;
      });
    });
    return {
      ready,
      resolve: (value: TrafficUpdatesResponseCompact) => resolve(value),
    };
  };

  it("keeps deletion authoritative over a delayed pre-delete update", async () => {
    const first = holdRead();
    const replacement = holdRead();
    const reload = useTrafficStore.getState().reloadRecords(true);
    await first.ready;
    useTrafficStore.getState().handleTrafficDeleted(["601"]);
    first.resolve(snapshot([makeRecord("601", "/deleted")], 1, 700));
    await replacement.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [],
      updated_records: [makeRecord("601", "/deleted", { response_size: 999 })],
      has_more: false,
      server_total: 1,
      server_sequence: 700,
    });
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().recordsMap.has("601")).toBe(false);
    replacement.resolve(snapshot([], 0, 700));
    await reload;
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(useTrafficStore.getState().serverTotal).toBe(0);
    expect(apiMocks.getTrafficUpdates).toHaveBeenCalledTimes(2);
  });

  it("retains newer positive metadata when older statistics arrive during a bounded read", async () => {
    const held = holdRead();
    const reload = useTrafficStore.getState().reloadRecords(true);
    await held.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("700", "/new")],
      updated_records: [],
      has_more: false,
      server_total: 701,
      server_sequence: 701,
    });
    await vi.advanceTimersByTimeAsync(200);
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 700,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    held.resolve(snapshot(makeRecordRange(200, 699), 700, 700));
    await reload;
    expect(useTrafficStore.getState().records).toHaveLength(501);
    expect(useTrafficStore.getState().serverTotal).toBe(701);
    expect(useTrafficStore.getState().serverSequence).toBe(701);
  });

  it("does not let a queued batch roll back metadata committed by a newer GET", async () => {
    const held = holdRead();
    const reload = useTrafficStore.getState().reloadRecords(true);
    await held.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("701", "/live")],
      updated_records: [],
      has_more: false,
      server_total: 701,
      server_sequence: 702,
    });
    held.resolve(snapshot(makeRecordRange(201, 700), 702, 703));
    await reload;
    expect(useTrafficStore.getState().serverTotal).toBe(702);
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().recordsMap.has("701")).toBe(true);
    expect(useTrafficStore.getState().serverTotal).toBe(702);
    expect(useTrafficStore.getState().serverSequence).toBe(703);
  });
  it("combines deletion, delayed updates/statistics and a newer GET without resurrection or rollback", async () => {
    const first = holdRead();
    const replacement = holdRead();
    const reload = useTrafficStore.getState().reloadRecords(true);
    await first.ready;
    useTrafficStore.getState().handleTrafficDeleted(["601"]);
    first.resolve(snapshot(makeRecordRange(200, 699), 700, 700));
    await replacement.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("700", "/surviving-insert")],
      updated_records: [
        makeRecord("602", "/surviving-update", { response_size: 999 }),
      ],
      has_more: false,
      server_total: 700,
      server_sequence: 701,
    });
    useTrafficStore.getState().handleTrafficPush({
      new_records: [],
      updated_records: [makeRecord("601", "/deleted")],
      has_more: false,
      server_total: 700,
      server_sequence: 700,
    });
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 699,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    replacement.resolve(
      snapshot(
        makeRecordRange(201, 701).filter((r) => r.id !== "601"),
        701,
        702,
      ),
    );
    await reload;
    expect(useTrafficStore.getState().recordsMap.has("601")).toBe(false);
    expect(
      useTrafficStore.getState().recordsMap.get("602")?.response_size,
    ).toBe(999);
    expect(useTrafficStore.getState().recordsMap.get("700")?.path).toBe(
      "/surviving-insert",
    );
    expect(useTrafficStore.getState().serverTotal).toBe(701);
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().serverTotal).toBe(701);
    expect(useTrafficStore.getState().serverSequence).toBe(702);
    expect(useTrafficStore.getState().recordsMap.has("601")).toBe(false);
  });

  it("preserves a delayed zero clear signal independently of newer positive totals", async () => {
    // The GET snapshot predates the second insert; the later authoritative ID
    // query must see that persisted row, rather than model a second deletion.
    apiMocks.queryTraffic.mockResolvedValueOnce({
      records: [makeRecord("701", "/after-read-start")],
      total: 1,
      has_more: false,
      server_sequence: 702,
    });
    const first = holdRead();
    const replacement = holdRead();
    const reload = useTrafficStore.getState().reloadRecords(true);
    await first.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("700", "/after-clear")],
      updated_records: [],
      has_more: false,
      server_total: 1,
      server_sequence: 701,
    });
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 0,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 600,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    first.resolve(snapshot(makeRecordRange(100, 599), 600, 700));
    await replacement.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("701", "/after-read-start")],
      updated_records: [],
      has_more: false,
      server_total: 2,
      server_sequence: 702,
    });
    replacement.resolve(snapshot([makeRecord("700", "/after-clear")], 1, 701));
    await reload;
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "700",
      "701",
    ]);
    expect(useTrafficStore.getState().serverTotal).toBe(2);
    expect(useTrafficStore.getState().serverSequence).toBe(702);
    expect(apiMocks.getTrafficUpdates).toHaveBeenCalledTimes(2);
  });

  it("consumes a pre-read batch while retaining its insert and updated fields", async () => {
    const existing = makeRecord("699", "/existing");
    useTrafficStore.setState({
      records: [existing],
      recordsMap: new Map([[existing.id, existing]]),
      serverTotal: 700,
      serverSequence: 700,
      currentRecord: {
        ...existing,
        request_headers: [],
        response_headers: [],
        request_body: "keep body",
        response_body: null,
        request_content_type: null,
        matched_rules: [],
      },
    });
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("700", "/queued-before-read")],
      updated_records: [makeRecord("699", "/existing", { response_size: 999 })],
      has_more: false,
      server_total: 701,
      server_sequence: 701,
    });
    const held = holdRead();
    const reload = useTrafficStore.getState().reloadRecords(true);
    await held.ready;
    held.resolve(snapshot(makeRecordRange(200, 699), 702, 702));
    await reload;
    expect(
      useTrafficStore.getState().recordsMap.get("699")?.response_size,
    ).toBe(999);
    expect(useTrafficStore.getState().recordsMap.has("700")).toBe(true);
    expect(useTrafficStore.getState().currentRecord?.response_size).toBe(999);
    expect(useTrafficStore.getState().currentRecord?.request_body).toBe(
      "keep body",
    );
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().serverTotal).toBe(702);
    expect(useTrafficStore.getState().serverSequence).toBe(702);
  });

  it("consumes equal-cursor stale totals when the GET reflects a deletion", async () => {
    apiMocks.queryTraffic.mockResolvedValueOnce({ records: [makeRecord("700", "/live")], total: 1, has_more: false, server_sequence: 701 });
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({ total_requests: 700, server_sequence: 701, client_ips: {}, proxy_ports: {}, applications: {}, account_names: {}, domains: {} });
    const held = holdRead();
    const reload = useTrafficStore.getState().reloadRecords(true);
    await held.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("700", "/live")],
      updated_records: [],
      has_more: false,
      server_total: 701,
      server_sequence: 701,
    });
    held.resolve(snapshot(makeRecordRange(200, 699), 700, 701));
    await reload;
    expect(useTrafficStore.getState().serverTotal).toBe(700);
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().serverTotal).toBe(700);
    expect(useTrafficStore.getState().recordsMap.has("700")).toBe(true);
  });
  it("does not let a cancelled read's tombstone block writes in a new generation", async () => {
    const held = holdRead();
    const obsolete = useTrafficStore.getState().reloadRecords(true);
    await held.ready;
    useTrafficStore.getState().handleTrafficDeleted(["601"]);
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(
      snapshot([makeRecord("601", "/restored")], 1, 700),
    );
    expect(await clearAndSynchronizeForTest()).toBe(true);
    useTrafficStore.getState().handleTrafficPush({
      new_records: [],
      updated_records: [makeRecord("601", "/restored", { response_size: 999 })],
      has_more: false,
      server_total: 1,
      server_sequence: 700,
    });
    await vi.advanceTimersByTimeAsync(200);
    expect(
      useTrafficStore.getState().recordsMap.get("601")?.response_size,
    ).toBe(999);
    held.resolve(snapshot([], 0, 700));
    await obsolete;
    expect(
      useTrafficStore.getState().recordsMap.get("601")?.response_size,
    ).toBe(999);
  });
});

describe("reconciliation bounded completion and failure recovery", () => {
  beforeEach(async () => {
    useTrafficStore.getState().disablePush();
    apiMocks.getTrafficUpdates.mockReset();
    apiMocks.clearTraffic.mockResolvedValue(undefined);
    await clearAndSynchronizeForTest();
    useTrafficStore.setState(useTrafficStore.getInitialState());
    apiMocks.getTrafficUpdates.mockReset();
    apiMocks.queryTraffic.mockReset();
    vi.useFakeTimers({
      toFake: [
        "setTimeout",
        "clearTimeout",
        "requestAnimationFrame",
        "cancelAnimationFrame",
      ],
    });
  });
  afterEach(() => {
    useTrafficStore.getState().disablePush();
    vi.useRealTimers();
  });
  const snapshot = (
    records: TrafficSummary[],
    total = records.length,
    sequence = 700,
  ): TrafficUpdatesResponseCompact => ({
    new_records: makePage(records, false, "forward").records,
    updated_records: [],
    has_more: false,
    server_total: total,
    server_sequence: sequence,
  });
  const holdRead = () => {
    let resolve!: (value: TrafficUpdatesResponseCompact) => void;
    let started!: () => void;
    const ready = new Promise<void>((done) => {
      started = done;
    });
    apiMocks.getTrafficUpdates.mockImplementationOnce(() => {
      started();
      return new Promise((done) => {
        resolve = done;
      });
    });
    return {
      ready,
      resolve: (value: TrafficUpdatesResponseCompact) => resolve(value),
    };
  };

  it("finishes the successful clear and bounds reads under unrelated continuous deletions", async () => {
    let release!: (value: TrafficUpdatesResponseCompact) => void;
    let deleting = true;
    let reads = 0;
    const stalled = new Promise<TrafficUpdatesResponseCompact>((done) => {
      release = done;
    });
    apiMocks.getTrafficUpdates.mockImplementation(() => {
      reads++;
      if (deleting)
        useTrafficStore.getState().handleTrafficDeleted([String(9000 + reads)]);
      return reads <= 8 ? Promise.resolve(snapshot([])) : stalled;
    });
    let finished = false;
    const clear = useTrafficStore
      .getState()
      .clearTraffic()
      .then((value) => {
        finished = true;
        return value;
      });
    try {
      for (let i = 0; i < 30; i++) await Promise.resolve();
      expect(finished).toBe(true);
      expect(useTrafficStore.getState().clearing).toBe(false);
      expect(reads).toBeLessThanOrEqual(3);
      expect(await clear).toBe(true);
    } finally {
      deleting = false;
      release(snapshot([]));
      await clear;
    }
  });

  it("counts a selected deletion once when a post-delete delta total arrives before its response", async () => {
    const records = makeRecordRange(1, 3);
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((r) => [r.id, r])),
      serverTotal: 3,
      serverSequence: 4,
    });
    let finishDelete!: () => void;
    apiMocks.clearTraffic.mockImplementationOnce(
      () =>
        new Promise((done) => {
          finishDelete = () => done(undefined);
        }),
    );
    let finishMembership!: () => void;
    apiMocks.queryTraffic.mockImplementationOnce(
      () =>
        new Promise((done) => {
          finishMembership = () =>
            done({
              records: records.filter((r) => r.id !== "2"),
              total: 2,
              has_more: false,
              server_sequence: 4,
            });
        }),
    );
    const clear = useTrafficStore.getState().clearTraffic(["2"]);
    useTrafficStore.getState().handleTrafficPush({
      new_records: [],
      updated_records: [],
      has_more: false,
      server_total: 2,
      server_sequence: 4,
    });
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().serverTotal).toBe(2);
    finishDelete();
    expect(await clear).toBe(true);
    useTrafficStore.getState().handleTrafficDeleted(["2"]);
    finishMembership();
    await useTrafficStore.getState().reconcileTrafficWindow();
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "1",
      "3",
    ]);
    expect(useTrafficStore.getState().serverTotal).toBe(2);
  });

  it("commits journaled inserts and updates when the replacement GET fails while idle", async () => {
    const first = holdRead();
    apiMocks.getTrafficUpdates.mockRejectedValueOnce(
      new Error("replacement failed"),
    );
    const reload = useTrafficStore.getState().reloadRecords(true);
    await first.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("700", "/insert")],
      updated_records: [makeRecord("700", "/insert", { response_size: 999 })],
      has_more: false,
      server_total: 1,
      server_sequence: 701,
    });
    useTrafficStore.getState().handleTrafficDeleted(["601"]);
    first.resolve(snapshot([makeRecord("601", "/deleted")]));
    await reload;
    await vi.advanceTimersByTimeAsync(200);
    expect(
      useTrafficStore.getState().recordsMap.get("700")?.response_size,
    ).toBe(999);
    expect(useTrafficStore.getState().recordsMap.has("601")).toBe(false);
    expect(useTrafficStore.getState().serverTotal).toBe(1);
    expect(useTrafficStore.getState().serverSequence).toBe(701);
  });

  it("commits journaled post-clear writes when the membership query fails while idle", async () => {
    const first = holdRead();
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(snapshot([], 0));
    apiMocks.queryTraffic.mockRejectedValueOnce(new Error("membership failed"));
    const reload = useTrafficStore.getState().reloadRecords(true);
    await first.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("700", "/after-clear")],
      updated_records: [],
      has_more: false,
      server_total: 1,
      server_sequence: 701,
    });
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 0,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    first.resolve(snapshot([makeRecord("601", "/before-clear")], 1));
    await reload;
    await vi.advanceTimersByTimeAsync(200);
    expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(1);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "700",
    ]);
    expect(useTrafficStore.getState().serverSequence).toBe(701);
  });
  it("preserves large authoritative totals and live writes when the retry budget is exhausted", async () => {
    let reads = 0;
    apiMocks.getTrafficUpdates.mockImplementation(() => {
      reads++;
      if (reads === 1)
        useTrafficStore.getState().handleTrafficPush({
          new_records: [makeRecord("700", "/keep")],
          updated_records: [],
          has_more: false,
          server_total: 1_000,
          server_sequence: 701,
        });
      useTrafficStore
        .getState()
        .handleTrafficDeleted([reads === 2 ? "601" : String(9000 + reads)]);
      return Promise.resolve(snapshot([makeRecord("601", "/obsolete")], 1));
    });
    await useTrafficStore.getState().reloadRecords(true);
    await vi.advanceTimersByTimeAsync(200);
    expect(reads).toBe(3);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "700",
    ]);
    expect(useTrafficStore.getState().serverTotal).toBe(1_000);
    expect(useTrafficStore.getState().serverSequence).toBe(701);
  });

  it("returns clear success before a stalled GET and retains writes at the deadline", async () => {
    const held = holdRead();
    const reload = useTrafficStore.getState().reloadRecords;
    let synchronization!: Promise<void>;
    useTrafficStore.setState({
      reloadRecords: (...args) => (synchronization = reload(...args)),
    });
    try {
      expect(await useTrafficStore.getState().clearTraffic()).toBe(true);
      expect(useTrafficStore.getState().clearing).toBe(false);
      await held.ready;
      useTrafficStore.getState().handleTrafficPush({
        new_records: [makeRecord("700", "/deadline-write")],
        updated_records: [],
        has_more: false,
        server_total: 1,
        server_sequence: 701,
      });
      useTrafficStore.getState().handleTrafficDeleted(["601"]);
      await vi.advanceTimersByTimeAsync(5_000);
      await synchronization;
      expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
        "700",
      ]);
      held.resolve(snapshot([makeRecord("601", "/late-obsolete")], 1));
      for (let i = 0; i < 5; i++) await Promise.resolve();
      expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
        "700",
      ]);
    } finally {
      held.resolve(snapshot([]));
      useTrafficStore.setState({ reloadRecords: reload });
    }
  });

  it("preserves accepted writes across pause/resume cancellation without applying the obsolete GET", async () => {
    const first = holdRead();
    const replacement = holdRead();
    const reload = useTrafficStore.getState().reloadRecords(true);
    await first.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("700", "/before-pause")],
      updated_records: [],
      has_more: false,
      server_total: 1,
      server_sequence: 701,
    });
    useTrafficStore.getState().handleTrafficDeleted(["601"]);
    first.resolve(snapshot([makeRecord("601", "/obsolete")], 1));
    await replacement.ready;
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("701", "/after-resume")],
      updated_records: [
        makeRecord("700", "/before-pause", { response_size: 999 }),
      ],
      has_more: false,
      server_total: 2,
      server_sequence: 702,
    });
    replacement.resolve(snapshot([makeRecord("601", "/obsolete")], 1));
    await reload;
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "700",
      "701",
    ]);
    expect(
      useTrafficStore.getState().recordsMap.get("700")?.response_size,
    ).toBe(999);
    expect(useTrafficStore.getState().serverTotal).toBe(2);
    expect(useTrafficStore.getState().serverSequence).toBe(702);
  });

  it("uses only completed membership chunks on failure and retains unqueried live rows", async () => {
    const first = holdRead();
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(snapshot([], 0, 400));
    const records = makeRecordRange(401, 1000);
    apiMocks.queryTraffic.mockResolvedValueOnce({
      records: records.slice(0, 499),
      total: 499,
      has_more: false,
      server_sequence: 1001,
    });
    apiMocks.queryTraffic.mockRejectedValueOnce(
      new Error("second chunk failed"),
    );
    const reload = useTrafficStore.getState().reloadRecords(true);
    await first.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: records,
      updated_records: [],
      has_more: false,
      server_total: 600,
      server_sequence: 1001,
    });
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 0,
      server_sequence: 400,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    first.resolve(snapshot([makeRecord("399", "/pre-clear")], 1, 400));
    await reload;
    await vi.advanceTimersByTimeAsync(200);
    expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(2);
    expect(useTrafficStore.getState().records).toHaveLength(599);
    expect(useTrafficStore.getState().recordsMap.has("900")).toBe(false);
    expect(useTrafficStore.getState().recordsMap.has("1000")).toBe(true);
    expect(useTrafficStore.getState().recordsMap.has("399")).toBe(false);
  });

  it("does not deduct a deletion broadcast from an already post-delete snapshot total", async () => {
    const records = makeRecordRange(1, 3);
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((r) => [r.id, r])),
      serverTotal: 3,
      serverSequence: 4,
    });
    let finishMembership!: () => void;
    apiMocks.queryTraffic.mockImplementationOnce(
      () =>
        new Promise((done) => {
          finishMembership = () =>
            done({
              records: records.filter((r) => r.id !== "2"),
              total: 2,
              has_more: false,
              server_sequence: 4,
            });
        }),
    );
    useTrafficStore.getState().handleTrafficPush({
      new_records: [],
      updated_records: [],
      has_more: false,
      server_total: 2,
      server_sequence: 4,
    });
    await vi.advanceTimersByTimeAsync(200);
    useTrafficStore.getState().handleTrafficDeleted(["2"]);
    finishMembership();
    await useTrafficStore.getState().reconcileTrafficWindow();
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "1",
      "3",
    ]);
    expect(useTrafficStore.getState().serverTotal).toBe(2);
  });

  it("retains active rows preserved by a selected clear's authoritative response", async () => {
    const records = makeRecordRange(1, 3);
    records[1].status = 0;
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((r) => [r.id, r])),
      serverTotal: 3,
      serverSequence: 4,
    });
    apiMocks.clearTraffic.mockResolvedValueOnce({
      success: true,
      message: "No traffic records to clear (all are active connections)",
    });
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(snapshot(records, 3, 4));
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({
      total_requests: 3,
      server_sequence: 4,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    expect(await clearAndSynchronizeForTest(["2"])).toBe(true);
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "1",
      "2",
      "3",
    ]);
    expect(useTrafficStore.getState().pendingIds.has("2")).toBe(true);
    expect(useTrafficStore.getState().serverTotal).toBe(3);
  });
  it("supersedes a pre-delete statistics request with a fresh count read", async () => {
    const records = makeRecordRange(1, 3);
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((r) => [r.id, r])),
      serverTotal: 3,
      serverSequence: 4,
    });
    let oldStatistics!: (value: TrafficStatistics) => void;
    apiMocks.getTrafficStatistics.mockImplementationOnce(
      () =>
        new Promise((done) => {
          oldStatistics = done;
        }),
    );
    const oldRead = useTrafficStore.getState().fetchTrafficStatistics();
    const fields = {
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    };
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({
      ...fields,
      total_requests: 2,
      server_sequence: 4,
    });
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(
      snapshot(
        records.filter((r) => r.id !== "2"),
        2,
        4,
      ),
    );
    expect(await clearAndSynchronizeForTest(["2"])).toBe(true);
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().serverTotal).toBe(2);
    expect(useTrafficStore.getState().trafficStatisticsTotal).toBe(2);
    oldStatistics({ ...fields, total_requests: 3, server_sequence: 4 });
    await oldRead;
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().serverTotal).toBe(2);
    expect(useTrafficStore.getState().trafficStatisticsTotal).toBe(2);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "1",
      "3",
    ]);
  });
  it("retains a post-clear commit whose sequence was allocated before the clear cursor", async () => {
    const first = holdRead();
    const record = makeRecord(
      "605",
      "/allocated-before-lock-committed-after-clear",
    );
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(snapshot([], 0, 700));
    apiMocks.queryTraffic.mockRejectedValueOnce(new Error("membership failed"));
    apiMocks.queryTraffic.mockResolvedValue({
      records: [record],
      total: 1,
      has_more: false,
      server_sequence: 700,
    });
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({
      total_requests: 1,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    const reload = useTrafficStore.getState().reloadRecords(true);
    await first.ready;
    // record() allocates sequence before acquiring write_conn. A writer may
    // persist after clear even though its sequence is below clear's cursor.
    useTrafficStore.getState().handleTrafficPush({
      new_records: [record],
      updated_records: [],
      has_more: false,
      server_total: 1,
      server_sequence: 700,
    });
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 0,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    first.resolve(snapshot([makeRecord("601", "/obsolete-snapshot")], 1, 700));
    await reload;
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "605",
    ]);
    expect(useTrafficStore.getState().serverTotal).toBe(1);
  });
  it("retains confirmed membership misses across a later retry and query failure", async () => {
    const first = holdRead();
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(snapshot([], 0));
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(snapshot([], 0));
    let finishMembership!: () => void;
    let queryStarted!: () => void;
    const ready = new Promise<void>((done) => {
      queryStarted = done;
    });
    apiMocks.queryTraffic.mockImplementationOnce(() => {
      queryStarted();
      return new Promise((done) => {
        finishMembership = () =>
          done({
            records: [],
            total: 0,
            has_more: false,
            server_sequence: 701,
          });
      });
    });
    apiMocks.queryTraffic.mockRejectedValueOnce(
      new Error("later membership failed"),
    );
    const reload = useTrafficStore.getState().reloadRecords(true);
    await first.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("700", "/subsequently-deleted")],
      updated_records: [],
      has_more: false,
      server_total: 1,
      server_sequence: 701,
    });
    useTrafficStore.getState().handleTrafficStatistics({
      total_requests: 0,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    first.resolve(snapshot([makeRecord("601", "/obsolete")], 1));
    await ready;
    useTrafficStore.getState().handleTrafficDeleted(["601"]);
    useTrafficStore.getState().handleTrafficPush({
      new_records: [makeRecord("701", "/survivor")],
      updated_records: [],
      has_more: false,
      server_total: 1,
      server_sequence: 702,
    });
    finishMembership();
    await reload;
    await vi.advanceTimersByTimeAsync(200);
    expect(apiMocks.getTrafficUpdates).toHaveBeenCalledTimes(3);
    expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(2);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "701",
    ]);
    expect(useTrafficStore.getState().serverTotal).toBe(1);
  });
  it("settles equal-cursor count disagreement with a fresh read after a live commit", async () => {
    const held = holdRead();
    const live = makeRecord("605", "/same-cursor-later-commit");
    apiMocks.queryTraffic.mockResolvedValue({
      records: [live],
      total: 1,
      has_more: false,
      server_sequence: 700,
    });
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({
      total_requests: 1,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    const reload = useTrafficStore.getState().reloadRecords(true);
    await held.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [live],
      updated_records: [],
      has_more: false,
      server_total: 1,
      server_sequence: 700,
    });
    held.resolve(snapshot([], 0, 700));
    await reload;
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "605",
    ]);
    expect(useTrafficStore.getState().serverTotal).toBe(1);
  });

  it("does not let metadata received before a replacement GET repeatedly invalidate its fresh rows", async () => {
    const held = holdRead();
    const fresh = [
      makeRecord("605", "/known"),
      makeRecord("606", "/newly-committed"),
    ];
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(snapshot(fresh, 2, 700));
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({
      total_requests: 2,
      server_sequence: 700,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    const reload = useTrafficStore.getState().reloadRecords(true);
    await held.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [fresh[0]],
      updated_records: [],
      has_more: false,
      server_total: 1,
      server_sequence: 700,
    });
    held.resolve(snapshot(fresh, 2, 700));
    await reload;
    await vi.advanceTimersByTimeAsync(200);
    expect(apiMocks.getTrafficUpdates).toHaveBeenCalledTimes(2);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "605",
      "606",
    ]);
    expect(useTrafficStore.getState().serverTotal).toBe(2);
  });
});
