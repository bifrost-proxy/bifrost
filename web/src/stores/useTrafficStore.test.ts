import "./trafficStoreTestFixture";
import { describe, expect, it } from "vitest";
import type { FilterCondition } from "../types";
import {
  apiMocks,
  toolbar,
  makeRecord,
  makePage,
  makeRecordRange,
  makeDelta,
  flushTrafficBatch,
  clearAndSynchronizeForTest,
} from "./trafficStoreTestFixture";
import {
  filterRecords,
  isFilterConditionApplicable,
  useTrafficStore,
} from "./useTrafficStore";
import { MAX_TRAFFIC_WINDOW_RECORDS } from "./trafficWindow";

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
