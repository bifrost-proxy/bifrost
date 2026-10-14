import "./trafficStoreTestFixture";
import { useTrafficStore } from "./useTrafficStore";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type {
  TrafficQueryResponse,
  TrafficSummary,
  TrafficUpdatesResponseCompact,
} from "../types";
import {
  apiMocks,
  pushMocks,
  makeRecord,
  makePage,
  makeDelta,
  flushTrafficBatch,
  clearAndSynchronizeForTest,
} from "./trafficStoreTestFixture";

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
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((r) => [r.id, r])),
      clearing: false,
    });
    apiMocks.clearTraffic.mockResolvedValueOnce({
      success: false,
      message: "clear rejected",
    });
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
  apiMocks.getTrafficUpdates.mockImplementationOnce(
    () =>
      new Promise<TrafficUpdatesResponseCompact>((resolve) => {
        resolveRead = resolve;
      }),
  );
  const clear = clearAndSynchronizeForTest();
  await Promise.resolve();
  await Promise.resolve();
  const live = makeRecord("506", "/live-during-read");
  useTrafficStore.setState({
    records: [live],
    recordsMap: new Map([[live.id, live]]),
    serverSequence: 507,
  });
  resolveRead({
    new_records: makePage([makeRecord("505", "/snapshot")], false, "forward")
      .records,
    updated_records: [],
    has_more: false,
    server_total: 1,
    server_sequence: 506,
  });
  expect(await clear).toBe(true);
  expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
    "505",
    "506",
  ]);
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
    useTrafficStore.getState().handleTrafficStatistics({
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
