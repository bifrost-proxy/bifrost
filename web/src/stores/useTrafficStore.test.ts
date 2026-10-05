import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  FilterCondition,
  ToolbarFilters,
  TrafficQueryResponse,
  TrafficSummary,
  TrafficDeltaData,
  TrafficStatistics,
} from "../types";

const apiMocks = vi.hoisted(() => ({
  getTrafficPage: vi.fn(),
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

    await useTrafficStore.getState().clearTraffic();

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
    await useTrafficStore.getState().clearTraffic();
    useTrafficStore.setState(useTrafficStore.getInitialState());
    apiMocks.queryTraffic.mockReset();
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
      expect(state.lastSequence).toBe(4);
      expect(apiMocks.queryTraffic).toHaveBeenCalledExactlyOnceWith({
        record_ids: ["1", "2", "3", "4"],
        limit: 4,
      });
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
    await useTrafficStore.getState().clearTraffic();
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
