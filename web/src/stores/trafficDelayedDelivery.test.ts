import "./trafficStoreTestFixture";
import { useTrafficStore } from "./useTrafficStore";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TrafficSummary, TrafficUpdatesResponseCompact } from "../types";
import {
  apiMocks,
  makeRecord,
  makePage,
  makeRecordRange,
  clearAndSynchronizeForTest,
} from "./trafficStoreTestFixture";

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
    apiMocks.queryTraffic.mockResolvedValueOnce({
      records: [makeRecord("700", "/live")],
      total: 1,
      has_more: false,
      server_sequence: 701,
    });
    apiMocks.getTrafficStatistics.mockResolvedValueOnce({
      total_requests: 700,
      server_sequence: 701,
      client_ips: {},
      proxy_ports: {},
      applications: {},
      account_names: {},
      domains: {},
    });
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
