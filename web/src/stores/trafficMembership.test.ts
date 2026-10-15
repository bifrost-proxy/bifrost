import "./trafficStoreTestFixture";
import { useTrafficStore } from "./useTrafficStore";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  FilterCondition,
  TrafficQueryResponse,
  TrafficStatistics,
  TrafficUpdatesResponseCompact,
} from "../types";
import {
  apiMocks,
  pushMocks,
  makeRecord,
  makePage,
  makeRecordRange,
  makeDelta,
  flushTrafficBatch,
  clearAndSynchronizeForTest,
} from "./trafficStoreTestFixture";
import { MAX_TRAFFIC_WINDOW_RECORDS } from "./trafficWindow";

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
      const connectionChanged =
        pushMocks.onConnectionChange.mock.calls.at(-1)![0];
      connectionChanged({ connected: false });
      connectionChanged({ connected: true });
      apiMocks.queryTraffic.mockImplementation(async ({ record_ids }) => {
        const survivors = records.filter(
          (record) => record_ids.includes(record.id) && record.id !== "1500",
        );
        return {
          ...makePage(survivors, false, "forward"),
          total: survivors.length,
        };
      });
      // The bounded initial query has older history beyond its 1,000-row tail.
      // Every chunk therefore has has_more=true, including the final one.
      for (const [start, end] of [
        [2001, 2500],
        [2501, 3000],
      ]) {
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
        records
          .filter((record) => record.id !== "1500")
          .map((record) => record.id),
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
      const records =
        phase === "full window"
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
        useTrafficStore
          .getState()
          .handleTrafficDeleted(records.map((record) => record.id));
      }
      const connectionChanged =
        pushMocks.onConnectionChange.mock.calls.at(-1)![0];
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
      useTrafficStore
        .getState()
        .handleTrafficStatistics({ ...emptyStatistics, server_sequence: 1 });
      const [arrival] = makeRecordRange(1, 1);
      useTrafficStore
        .getState()
        .handleTrafficDelta(makeDelta([arrival], 1, 2, 1));
      useTrafficStore.getState().handleTrafficStatistics({
        ...emptyStatistics,
        total_requests: 1,
        server_sequence: 2,
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
        last_traffic_id: "1",
        last_sequence: 1,
        pending_ids: [],
      });
      useTrafficStore.getState().disablePush();
      useTrafficStore.getState().enablePush();
      expect(pushMocks.connect).toHaveBeenLastCalledWith({
        last_traffic_id: "1",
        last_sequence: 1,
        pending_ids: [],
        need_traffic: true,
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
  ])(
    "recovers a populated replacement database after $resume with $phase metadata",
    async ({ resume, phase }) => {
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
        const connectionChanged =
          pushMocks.onConnectionChange.mock.calls.at(-1)![0];
        connectionChanged({ connected: false });
        connectionChanged({ connected: true });
      } else {
        useTrafficStore.getState().disablePush();
        useTrafficStore.getState().enablePush();
      }
      const replacement = {
        ...emptyStatistics,
        total_requests: 600,
        server_sequence: 601,
      };
      apiMocks.getTrafficStatistics.mockResolvedValueOnce(replacement);
      const replacementRecords = makeRecordRange(1, 601);
      apiMocks.queryTraffic.mockImplementation(
        async ({ record_ids }: { record_ids: string[] }) => {
          const matches = replacementRecords.filter((record) =>
            record_ids.includes(record.id),
          );
          return {
            ...makePage(matches, false, "forward"),
            total: matches.length,
          };
        },
      );
      const statisticsCalls = apiMocks.getTrafficStatistics.mock.calls.length;
      if (phase !== "no delta") {
        // The server retains the old subscription cursor and filters every new
        // record, but still sends current metadata before initial statistics.
        useTrafficStore
          .getState()
          .handleTrafficDelta(makeDelta([], 600, 601, 1));
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
      useTrafficStore
        .getState()
        .handleTrafficDelta(makeDelta(makeRecordRange(101, 600), 600, 601, 1));
      await flushTrafficBatch();
      useTrafficStore
        .getState()
        .handleTrafficDelta(makeDelta(makeRecordRange(601, 601), 601, 602, 1));
      useTrafficStore.getState().handleTrafficStatistics({
        ...replacement,
        total_requests: 601,
        server_sequence: 602,
      });
      useTrafficStore.getState().handleTrafficStatistics(replacement);
      await flushTrafficBatch();
      const state = useTrafficStore.getState();
      expect(state.records.map((record) => record.sequence)).toEqual(
        makeRecordRange(101, 601).map((record) => record.sequence),
      );
      expect(state.lastSequence).toBe(601);
      expect(state.serverSequence).toBe(602);
      expect(state.serverTotal).toBe(601);
      expect(state.trafficEpochVersion).toBe(1);
      expect(pushMocks.resetTrafficCursor).toHaveBeenCalledTimes(1);
      useTrafficStore.getState().disablePush();
      useTrafficStore.getState().enablePush();
      expect(pushMocks.connect).toHaveBeenLastCalledWith({
        last_traffic_id: "601",
        last_sequence: 601,
        pending_ids: [],
        need_traffic: true,
      });
      useTrafficStore.getState().handleTrafficStatistics({
        ...replacement,
        total_requests: 601,
        server_sequence: 602,
      });
      await flushTrafficBatch();
      expect(pushMocks.resetTrafficCursor).toHaveBeenCalledTimes(1);
      expect(
        apiMocks.getTrafficStatistics.mock.calls.length - statisticsCalls,
      ).toBe(1);
      expect(useTrafficStore.getState().records).toEqual(state.records);
    },
  );

  it.each([0, 1])(
    "keeps the current epoch when a fresh read disproves stale statistics with total %i",
    async (total) => {
      const records = seed();
      useTrafficStore.setState({ hasMore: true, hasNewer: true });
      useTrafficStore.getState().enablePush();
      useTrafficStore.getState().disablePush();
      useTrafficStore.getState().enablePush();
      apiMocks.getTrafficStatistics.mockResolvedValueOnce({
        ...emptyStatistics,
        total_requests: 3,
      });
      apiMocks.queryTraffic.mockResolvedValueOnce({
        ...makePage(records, false, "forward"),
        total: 3,
      });
      const calls = apiMocks.getTrafficStatistics.mock.calls.length;
      useTrafficStore.getState().handleTrafficStatistics({
        ...emptyStatistics,
        total_requests: total,
        server_sequence: total + 1,
      });
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
    },
  );

  it.each([0, 1])(
    "preserves the window after confirmation failure and retries later statistics with total %i",
    async (total) => {
      const records = seed();
      useTrafficStore.getState().enablePush();
      useTrafficStore.getState().disablePush();
      useTrafficStore.getState().enablePush();
      apiMocks.getTrafficStatistics.mockRejectedValueOnce(new Error("offline"));
      const restarted = {
        ...emptyStatistics,
        total_requests: total,
        server_sequence: total + 1,
      };
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
    },
  );

  it("recovers an arrival created before restart confirmation returns", async () => {
    seed();
    useTrafficStore.getState().enablePush();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    let resolveStatistics!: (statistics: TrafficStatistics) => void;
    apiMocks.getTrafficStatistics.mockImplementationOnce(
      () =>
        new Promise<TrafficStatistics>((resolve) => {
          resolveStatistics = resolve;
        }),
    );
    useTrafficStore
      .getState()
      .handleTrafficStatistics({ ...emptyStatistics, server_sequence: 1 });
    // The stale server-side cursor filtered out the first insert, but this fresh
    // read proves the new epoch even though the database is no longer empty.
    resolveStatistics({
      ...emptyStatistics,
      total_requests: 1,
      server_sequence: 2,
    });
    await flushTrafficBatch();
    expect(pushMocks.resetTrafficCursor).toHaveBeenCalledTimes(1);
    expect(useTrafficStore.getState().serverTotal).toBe(1);
    useTrafficStore
      .getState()
      .handleTrafficDelta(makeDelta(makeRecordRange(1, 1), 1, 2, 1));
    await flushTrafficBatch();
    expect(
      useTrafficStore.getState().records.map((record) => record.id),
    ).toEqual(["1"]);
    expect(useTrafficStore.getState().lastSequence).toBe(1);
  });

  it("starts a fresh confirmation when another reconnect interrupts the previous read", async () => {
    seed();
    useTrafficStore.getState().enablePush();
    const connectionChanged =
      pushMocks.onConnectionChange.mock.calls.at(-1)![0];
    connectionChanged({ connected: false });
    connectionChanged({ connected: true });
    let resolveOld!: (statistics: TrafficStatistics) => void;
    let resolveCurrent!: (statistics: TrafficStatistics) => void;
    apiMocks.getTrafficStatistics
      .mockImplementationOnce(
        () =>
          new Promise<TrafficStatistics>((resolve) => {
            resolveOld = resolve;
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise<TrafficStatistics>((resolve) => {
            resolveCurrent = resolve;
          }),
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

  it.each([0, 1])(
    "retains a newer epoch snapshot while confirming a replacement with total %i",
    async (total) => {
      seed();
      useTrafficStore.getState().enablePush();
      useTrafficStore.getState().disablePush();
      useTrafficStore.getState().enablePush();
      let resolveStatistics!: (statistics: TrafficStatistics) => void;
      apiMocks.getTrafficStatistics
        .mockImplementationOnce(
          () =>
            new Promise<TrafficStatistics>((resolve) => {
              resolveStatistics = resolve;
            }),
        )
        .mockResolvedValueOnce({
          ...emptyStatistics,
          total_requests: total + 1,
          server_sequence: total + 2,
        });
      const calls = apiMocks.getTrafficStatistics.mock.calls.length;
      useTrafficStore.getState().handleTrafficStatistics({
        ...emptyStatistics,
        total_requests: total,
        server_sequence: total + 1,
      });
      // The old server cursor filters the insert, but metadata/statistics advance.
      useTrafficStore
        .getState()
        .handleTrafficDelta(makeDelta([], total + 1, total + 2, 1));
      useTrafficStore.getState().handleTrafficStatistics({
        ...emptyStatistics,
        total_requests: total + 1,
        server_sequence: total + 2,
      });
      resolveStatistics({
        ...emptyStatistics,
        total_requests: total,
        server_sequence: total + 1,
      });
      await flushTrafficBatch();
      expect(apiMocks.getTrafficStatistics.mock.calls.length - calls).toBe(2);
      expect(pushMocks.resetTrafficCursor).toHaveBeenCalledTimes(1);
      useTrafficStore
        .getState()
        .handleTrafficDelta(
          makeDelta(makeRecordRange(1, total + 1), total + 1, total + 2, 1),
        );
      await flushTrafficBatch();
      expect(
        useTrafficStore.getState().records.map((record) => record.id),
      ).toEqual(makeRecordRange(1, total + 1).map((record) => record.id));
      expect(useTrafficStore.getState().lastSequence).toBe(total + 1);
    },
  );

  it.each([0, 1])(
    "does not apply a confirmation with total %i captured before newer same-epoch traffic",
    async (total) => {
      const records = seed();
      useTrafficStore.getState().enablePush();
      useTrafficStore.getState().disablePush();
      useTrafficStore.getState().enablePush();
      let resolveStatistics!: (statistics: TrafficStatistics) => void;
      apiMocks.getTrafficStatistics.mockImplementationOnce(
        () =>
          new Promise<TrafficStatistics>((resolve) => {
            resolveStatistics = resolve;
          }),
      );
      useTrafficStore.getState().handleTrafficStatistics({
        ...emptyStatistics,
        total_requests: total,
        server_sequence: total + 1,
      });
      const [arrival] = makeRecordRange(4, 4);
      useTrafficStore.getState().handleTrafficDelta({
        ...makeDelta([arrival], 4, 5, 1),
        has_more: true,
      });
      resolveStatistics({
        ...emptyStatistics,
        total_requests: total,
        server_sequence: total + 1,
      });
      await flushTrafficBatch();
      expect(pushMocks.resetTrafficCursor).not.toHaveBeenCalled();
      expect(
        useTrafficStore.getState().records.map((record) => record.id),
      ).toEqual(["1", "2", "3", "4"]);
      apiMocks.queryTraffic.mockResolvedValueOnce({
        ...makePage([...records, arrival], false, "forward"),
        total: 4,
      });
      useTrafficStore.getState().handleTrafficStatistics({
        ...emptyStatistics,
        total_requests: 4,
        server_sequence: 5,
      });
      await flushTrafficBatch();
      expect(useTrafficStore.getState().serverTotal).toBe(4);
      expect(apiMocks.queryTraffic).toHaveBeenCalledTimes(1);
    },
  );

  it("ignores a previous epoch's membership response after resetting", async () => {
    const records = seed();
    useTrafficStore.getState().enablePush();
    let resolveQuery!: (page: TrafficQueryResponse) => void;
    apiMocks.queryTraffic.mockImplementationOnce(
      () =>
        new Promise<TrafficQueryResponse>((resolve) => {
          resolveQuery = resolve;
        }),
    );
    useTrafficStore.getState().handleTrafficDelta(makeDelta([], 1, 4, 1));
    await flushTrafficBatch();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({
      ...emptyStatistics,
      server_sequence: 1,
    });
    useTrafficStore
      .getState()
      .handleTrafficStatistics({ ...emptyStatistics, server_sequence: 1 });
    await flushTrafficBatch();
    useTrafficStore
      .getState()
      .handleTrafficDelta(makeDelta(makeRecordRange(1, 1), 1, 2, 1));
    await flushTrafficBatch();
    resolveQuery({ ...makePage([records[1]], false, "forward"), total: 1 });
    await useTrafficStore.getState().reconcileTrafficWindow();
    expect(
      useTrafficStore.getState().records.map((record) => record.id),
    ).toEqual(["1"]);
    expect(useTrafficStore.getState().lastSequence).toBe(1);
  });

  it("ignores a jump-to-live response from the previous database epoch", async () => {
    const records = seed();
    useTrafficStore.setState({ hasNewer: true });
    let resolveReload!: (response: TrafficUpdatesResponseCompact) => void;
    apiMocks.getTrafficUpdates.mockImplementationOnce(
      () =>
        new Promise<TrafficUpdatesResponseCompact>((resolve) => {
          resolveReload = resolve;
        }),
    );
    const reload = useTrafficStore.getState().reloadRecords();
    useTrafficStore.getState().enablePush();
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    const replacement = {
      ...emptyStatistics,
      total_requests: 1,
      server_sequence: 2,
    };
    apiMocks.getTrafficStatistics.mockResolvedValueOnce(replacement);
    useTrafficStore.getState().handleTrafficStatistics(replacement);
    await flushTrafficBatch();
    const arrival = makeRecord("replacement", "/new", { sequence: 1 });
    useTrafficStore
      .getState()
      .handleTrafficDelta(makeDelta([arrival], 1, 2, 1));
    await flushTrafficBatch();
    const recovered = useTrafficStore.getState();
    expect(recovered.records.map((record) => record.id)).toEqual([
      "replacement",
    ]);
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
    expect(useTrafficStore.getState().recordsMutation).toEqual(
      recovered.recordsMutation,
    );
    expect(useTrafficStore.getState().serverTotal).toBe(1);
    expect(useTrafficStore.getState().serverSequence).toBe(2);
    expect(useTrafficStore.getState().lastSequence).toBe(1);
    expect(useTrafficStore.getState().lastId).toBe("replacement");
    expect(pushMocks.updateSubscription.mock.calls.length).toBe(
      subscriptionUpdates,
    );
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    expect(pushMocks.connect).toHaveBeenLastCalledWith({
      last_traffic_id: "replacement",
      last_sequence: 1,
      pending_ids: [],
      need_traffic: true,
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
      last_traffic_id: "6",
      last_sequence: 6,
      pending_ids: [],
    });
  });

  it("ignores ordinary statistics responses captured before an epoch reset", async () => {
    seed();
    let resolveStatistics!: (statistics: TrafficStatistics) => void;
    apiMocks.getTrafficStatistics.mockImplementationOnce(
      () =>
        new Promise<TrafficStatistics>((resolve) => {
          resolveStatistics = resolve;
        }),
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
      () =>
        new Promise<TrafficStatistics>((resolve) => {
          resolveStatistics = resolve;
        }),
    );
    const calls = apiMocks.getTrafficStatistics.mock.calls.length;
    useTrafficStore.getState().handleTrafficStatistics({
      ...emptyStatistics,
      total_requests: 1,
      server_sequence: 2,
    });
    await flushTrafficBatch();
    expect(useTrafficStore.getState().records).toEqual(records);
    expect(useTrafficStore.getState().serverTotal).toBe(3);
    expect(useTrafficStore.getState().lastSequence).toBe(3);
    expect(apiMocks.getTrafficStatistics.mock.calls.length - calls).toBe(1);
    expect(apiMocks.queryTraffic).not.toHaveBeenCalled();
    expect(pushMocks.resetTrafficCursor).not.toHaveBeenCalled();

    apiMocks.queryTraffic.mockResolvedValueOnce({
      ...makePage(records, false, "forward"),
      total: 3,
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
        apiMocks.getTrafficStatistics.mockResolvedValueOnce({
          ...emptyStatistics,
          server_sequence: 1,
        });
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
