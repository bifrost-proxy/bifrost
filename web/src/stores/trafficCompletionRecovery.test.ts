import "./trafficStoreTestFixture";
import { useTrafficStore } from "./useTrafficStore";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  TrafficSummary,
  TrafficStatistics,
  TrafficUpdatesResponseCompact,
} from "../types";
import {
  apiMocks,
  makeRecord,
  makePage,
  makeRecordRange,
  clearAndSynchronizeForTest,
} from "./trafficStoreTestFixture";

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
    let reject!: (reason: Error) => void;
    let started!: () => void;
    const ready = new Promise<void>((done) => {
      started = done;
    });
    apiMocks.getTrafficUpdates.mockImplementationOnce(() => {
      started();
      return new Promise((done, fail) => {
        resolve = done;
        reject = fail;
      });
    });
    return {
      ready,
      resolve: (value: TrafficUpdatesResponseCompact) => resolve(value),
      reject: (reason: Error) => reject(reason),
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

  it.each([
    { frameBeforeResponse: true, readFails: false },
    { frameBeforeResponse: true, readFails: true },
    { frameBeforeResponse: false, readFails: false },
    { frameBeforeResponse: false, readFails: true },
  ])(
    "keeps selected deletion tombstones before journal creation ($frameBeforeResponse/$readFails)",
    async ({ frameBeforeResponse, readFails }) => {
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
            finishDelete = () => done({ success: true });
          }),
      );
      apiMocks.getTrafficStatistics.mockResolvedValueOnce({
        total_requests: 2,
        server_sequence: 4,
        client_ips: {},
        proxy_ports: {},
        applications: {},
        account_names: {},
        domains: {},
      });
      const held = holdRead();
      const clear = useTrafficStore.getState().clearTraffic(["2"]);
      if (frameBeforeResponse) {
        useTrafficStore.getState().handleTrafficDeleted(["2"]);
        useTrafficStore.getState().handleTrafficPush({
          new_records: [],
          updated_records: [{ ...records[1], status: 298 }],
          has_more: false,
          server_total: 2,
          server_sequence: 4,
        });
      }
      finishDelete();
      expect(await clear).toBe(true);
      await held.ready;
      useTrafficStore.getState().handleTrafficPush({
        new_records: [],
        updated_records: [{ ...records[1], status: 299 }],
        has_more: false,
        server_total: 2,
        server_sequence: 4,
      });
      apiMocks.queryTraffic.mockRejectedValueOnce(
        new Error("membership unavailable"),
      );
      if (readFails) held.reject(new Error("snapshot unavailable"));
      else
        held.resolve(
          snapshot(
            records.filter((r) => r.id !== "2"),
            2,
            4,
          ),
        );
      await vi.advanceTimersByTimeAsync(200);
      expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
        "1",
        "3",
      ]);
      expect(useTrafficStore.getState().recordsMap.has("2")).toBe(false);
    },
  );

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

  it("preserves a surviving completion received before the selected DELETE response", async () => {
    const records = makeRecordRange(1, 3);
    records[1].status = 0;
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
          finishDelete = () => done({ success: true });
        }),
    );
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({
      total_requests: 3,
      server_sequence: 4,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    const held = holdRead();
    const clear = clearAndSynchronizeForTest(["2"]);
    useTrafficStore
      .getState()
      .handleTrafficPush({
        new_records: [],
        updated_records: [
          { ...records[1], status: 201, response_size: 8192, duration_ms: 543 },
        ],
        has_more: false,
        server_total: 3,
        server_sequence: 4,
      });
    finishDelete();
    await held.ready;
    held.resolve(snapshot(records, 3, 4));
    expect(await clear).toBe(true);
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().recordsMap.get("2")).toMatchObject({
      status: 201,
      response_size: 8192,
      duration_ms: 543,
    });
  });

  it("keeps confirmed deletion authoritative over a pending selected row's completion", async () => {
    const records = makeRecordRange(1, 3);
    records[1].status = 0;
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((r) => [r.id, r])),
      serverTotal: 3,
      serverSequence: 4,
    });
    const stats = {
      total_requests: 2,
      server_sequence: 4,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    };
    apiMocks.getTrafficStatistics
      .mockResolvedValueOnce(stats)
      .mockResolvedValueOnce(stats);
    const held = holdRead();
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(
      snapshot(
        records.filter((r) => r.id !== "2"),
        2,
        4,
      ),
    );
    const clear = clearAndSynchronizeForTest(["2"]);
    await held.ready;
    useTrafficStore.getState().handleTrafficDeleted(["2"]);
    useTrafficStore
      .getState()
      .handleTrafficPush({
        new_records: [],
        updated_records: [{ ...records[1], status: 201, response_size: 8192 }],
        has_more: false,
        server_total: 2,
        server_sequence: 4,
      });
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().recordsMap.has("2")).toBe(false);
    held.resolve(snapshot(records, 3, 4));
    expect(await clear).toBe(true);
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "1",
      "3",
    ]);
    expect(useTrafficStore.getState().pendingIds.has("2")).toBe(false);
  });

  it("confirms a selected survivor outside the GET window without losing its completion", async () => {
    const records = makeRecordRange(1, 3);
    records[1].status = 0;
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((r) => [r.id, r])),
      serverTotal: 3,
      serverSequence: 4,
    });
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({
      total_requests: 3,
      server_sequence: 4,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(
      snapshot([records[0], records[2]], 3, 4),
    );
    let finishMembership!: () => void;
    let membershipReady!: () => void;
    const ready = new Promise<void>((done) => {
      membershipReady = done;
    });
    apiMocks.queryTraffic.mockImplementationOnce(() => {
      membershipReady();
      return new Promise((done) => {
        finishMembership = () =>
          done({
            records: makePage([records[1]], false, "forward").records,
            total: 1,
            has_more: false,
            server_sequence: 4,
          });
      });
    });
    const clear = clearAndSynchronizeForTest(["2"]);
    await ready;
    useTrafficStore
      .getState()
      .handleTrafficPush({
        new_records: [],
        updated_records: [{ ...records[1], status: 201, response_size: 8192 }],
        has_more: false,
        server_total: 3,
        server_sequence: 4,
      });
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().recordsMap.has("2")).toBe(false);
    finishMembership();
    expect(await clear).toBe(true);
    await vi.advanceTimersByTimeAsync(200);
    expect(apiMocks.queryTraffic).toHaveBeenCalledWith(
      expect.objectContaining({ record_ids: ["2"] }),
    );
    expect(useTrafficStore.getState().recordsMap.get("2")).toMatchObject({
      status: 201,
      response_size: 8192,
    });
    expect(useTrafficStore.getState().serverTotal).toBe(3);
  });

  it.each(["http", "sse", "connect"])(
    "preserves the %s completion received while a selected clear GET is stale",
    async (kind) => {
      const records = makeRecordRange(1, 3);
      records[1] = {
        ...records[1],
        status: kind === "http" ? 0 : 200,
        end_time: undefined,
        response_size: 0,
        is_sse: kind === "sse",
        socket_status:
          kind === "connect"
            ? {
                is_open: true,
                send_bytes: 0,
                receive_bytes: 0,
                frame_count: 0,
                send_count: 0,
                receive_count: 0,
              }
            : undefined,
      };
      useTrafficStore.setState({
        records,
        recordsMap: new Map(records.map((r) => [r.id, r])),
        serverTotal: 3,
        serverSequence: 4,
      });
      apiMocks.getTrafficStatistics.mockResolvedValueOnce({
        total_requests: 3,
        server_sequence: 4,
        client_ips: {},
        proxy_ports: {},
        applications: {},
        account_names: {},
        domains: {},
      });
      const held = holdRead();
      const clear = clearAndSynchronizeForTest(["2"]);
      await held.ready;
      const completion = {
        ...records[1],
        status: 201,
        response_size: 8192,
        duration_ms: 543,
        end_time: "2026-10-15T00:00:01Z",
        socket_status:
          kind === "connect"
            ? {
                is_open: false,
                send_bytes: 32,
                receive_bytes: 8192,
                frame_count: 2,
                send_count: 1,
                receive_count: 1,
              }
            : undefined,
      };
      useTrafficStore.getState().handleTrafficPush({
        new_records: [],
        updated_records: [completion],
        has_more: false,
        server_total: 3,
        server_sequence: 4,
      });
      held.resolve(snapshot(records, 3, 4));
      expect(await clear).toBe(true);
      await vi.advanceTimersByTimeAsync(200);
      expect(useTrafficStore.getState().recordsMap.get("2")).toMatchObject({
        status: 201,
        response_size: 8192,
        duration_ms: 543,
        end_time: completion.end_time,
      });
      if (kind === "connect")
        expect(
          useTrafficStore.getState().recordsMap.get("2")?.socket_status
            ?.is_open,
        ).toBe(false);
      expect(useTrafficStore.getState().pendingIds.has("2")).toBe(false);
      expect(useTrafficStore.getState().serverTotal).toBe(3);
    },
  );

  it("retains the live completion of an active selected row and accepts later updates", async () => {
    const records = makeRecordRange(1, 3);
    records[1].status = 0;
    useTrafficStore.setState({
      records,
      recordsMap: new Map(records.map((r) => [r.id, r])),
      serverTotal: 3,
      serverSequence: 4,
    });
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({
      total_requests: 3,
      server_sequence: 4,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
    const held = holdRead();
    const clear = clearAndSynchronizeForTest(["2"]);
    await held.ready;
    useTrafficStore.getState().handleTrafficPush({
      new_records: [],
      updated_records: [{ ...records[1], status: 299 }],
      has_more: false,
      server_total: 3,
      server_sequence: 4,
    });
    held.resolve(snapshot(records, 3, 4));
    expect(await clear).toBe(true);
    expect(useTrafficStore.getState().recordsMap.get("2")?.status).toBe(299);
    useTrafficStore.getState().handleTrafficPush({
      new_records: [],
      updated_records: [{ ...records[1], status: 201 }],
      has_more: false,
      server_total: 3,
      server_sequence: 4,
    });
    await vi.advanceTimersByTimeAsync(200);
    expect(useTrafficStore.getState().recordsMap.get("2")?.status).toBe(201);
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
