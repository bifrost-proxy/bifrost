import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  TrafficDeltaData,
  TrafficQueryResponse,
  TrafficStatistics,
  TrafficSummary,
  TrafficRecord,
  TrafficUpdatesResponseCompact,
} from "../types";

const api = vi.hoisted(() => ({
  getTrafficStatistics: vi.fn(),
  getTrafficUpdates: vi.fn(),
  getTrafficPage: vi.fn(),
  queryTraffic: vi.fn(),
  clearTraffic: vi.fn().mockResolvedValue(undefined),
  getTrafficDetail: vi.fn(),
  getRequestBody: vi.fn().mockResolvedValue(null),
  getResponseBody: vi.fn().mockResolvedValue(null),
  getRequestBodyContent: vi.fn().mockResolvedValue(null),
  getResponseBodyContent: vi.fn().mockResolvedValue(null),
}));
const push = vi.hoisted(() => ({
  onTrafficUpdates: vi.fn().mockReturnValue(() => {}),
  onTrafficDelta: vi.fn().mockReturnValue(() => {}),
  onTrafficDeleted: vi.fn().mockReturnValue(() => {}),
  onTrafficStatistics: vi.fn().mockReturnValue(() => {}),
  onConnectionChange: vi.fn().mockReturnValue(() => {}),
  connect: vi.fn(),
  disconnectIfIdle: vi.fn(),
  updateSubscription: vi.fn(),
  resetTrafficCursor: vi.fn(),
}));
vi.mock("../api", () => api);
vi.mock("../services/pushService", () => ({ default: push }));
import { useTrafficStore } from "./useTrafficStore";

const OLD = "9170043d-160c-435c-a171-000000000001";
const NEW = "9170043d-160c-435c-a171-000000000002";
const OTHER = "9170043d-160c-435c-a171-000000000003";
const statistics = (epoch = OLD, sequence = 2001, total = 2000): TrafficStatistics => ({
  database_epoch: epoch,
  server_sequence: sequence,
  total_requests: total,
  client_ips: {}, proxy_ports: {}, applications: {}, account_names: {}, domains: {},
});
const records = (process: string, start: number, end: number): TrafficSummary[] =>
  Array.from({ length: end - start + 1 }, (_, index) => ({
    id: `REQ-${process}-${String(start + index).padStart(8, "0")}`,
    sequence: start + index,
    timestamp: 1, method: "GET", url: `http://example.test/${start + index}`,
    host: "example.test", path: `/${start + index}`, status: 200,
    content_type: "text/plain", request_size: 0, response_size: 2, duration_ms: 1,
    protocol: "HTTP/1.1", client_ip: "127.0.0.1", has_rule_hit: false,
    matched_rule_count: 0, matched_protocols: [], start_time: "2026-10-05T00:00:00Z",
  }));
const page = (rows: TrafficSummary[]): TrafficQueryResponse => ({
  records: rows.map((r) => ({
    id: r.id, seq: r.sequence, ts: r.timestamp, m: r.method, h: r.host, p: r.path,
    s: r.status, req_sz: 0, res_sz: 2, dur: 1, proto: r.protocol, cip: r.client_ip,
    flags: 0, fc: 0, st: r.start_time, rc: 0, rp: [],
  })),
  total: rows.length, server_sequence: rows.at(-1)?.sequence ?? 1,
  has_more: false, next_cursor: null, prev_cursor: null,
});
const delta = (epoch: string, rows: TrafficSummary[], sequence = 2001): TrafficDeltaData => ({
  database_epoch: epoch, inserts: page(rows).records, updates: [], has_more: false,
  server_total: sequence - 1, server_sequence: sequence, oldest_sequence: 1,
});
const updates = (epoch: string, rows: TrafficSummary[]): TrafficUpdatesResponseCompact => ({
  database_epoch: epoch, new_records: page(rows).records, updated_records: [],
  has_more: true, server_total: 2000, server_sequence: 2001,
});
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}
const flush = () => vi.advanceTimersByTimeAsync(200);
function seed(history = false) {
  const rows = records("OLDPROCESS", 1001, 2000);
  useTrafficStore.setState({
    records: rows, recordsMap: new Map(rows.map((row) => [row.id, row])),
    trafficDatabaseEpoch: OLD, serverTotal: 2000, serverSequence: 2001,
    oldestSequence: 1001, serverOldestSequence: 1001, lastSequence: 2000,
    lastId: rows.at(-1)!.id, hasMore: true, hasNewer: history,
  });
  useTrafficStore.getState().enablePush();
  const onConnection = push.onConnectionChange.mock.calls.at(-1)![0];
  onConnection({ connected: false });
  onConnection({ connected: true });
  return rows;
}

beforeEach(async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "requestAnimationFrame", "cancelAnimationFrame"] });
  useTrafficStore.getState().disablePush();
  await useTrafficStore.getState().clearTraffic();
  useTrafficStore.setState(useTrafficStore.getInitialState());
  vi.clearAllMocks();
  api.getTrafficStatistics.mockReset();
  api.getTrafficUpdates.mockReset();
  api.getTrafficPage.mockReset();
  api.queryTraffic.mockReset();
  api.getTrafficDetail.mockReset();
  api.getRequestBody.mockReset().mockResolvedValue(null);
  api.getResponseBody.mockReset().mockResolvedValue(null);
  api.getRequestBodyContent.mockReset().mockResolvedValue(null);
  api.getResponseBodyContent.mockReset().mockResolvedValue(null);
});
afterEach(async () => {
  useTrafficStore.getState().stopPolling();
  useTrafficStore.getState().disablePush();
  await useTrafficStore.getState().clearTraffic();
  vi.useRealTimers();
});

describe("Persistent traffic database identity", () => {
  it.each([
    { sequence: 2001, history: false }, { sequence: 2601, history: false },
    { sequence: 2001, history: true }, { sequence: 2601, history: true },
  ])("recovers sequence $sequence in history=$history without confusing process IDs", async ({ sequence, history }) => {
    seed(history);
    const fresh = statistics(NEW, sequence, sequence - 1);
    api.getTrafficStatistics.mockResolvedValue(fresh);
    useTrafficStore.getState().handleTrafficStatistics(fresh);
    await flush();
    expect(push.resetTrafficCursor).toHaveBeenCalledOnce();
    expect(useTrafficStore.getState()).toMatchObject({
      trafficDatabaseEpoch: NEW, trafficEpochVersion: 1, lastSequence: null,
      lastId: null, serverOldestSequence: null, oldestSequence: null,
      hasNewer: false, serverSequence: sequence,
    });
    const tail = records("NEWPROCESS", 1, 500);
    useTrafficStore.getState().handleTrafficDelta(delta(NEW, tail, sequence));
    await flush();
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual(tail.map((r) => r.id));
    expect(useTrafficStore.getState().lastSequence).toBe(500);
    useTrafficStore.getState().disablePush();
    useTrafficStore.getState().enablePush();
    useTrafficStore.getState().handleTrafficStatistics(fresh);
    await flush();
    expect(push.resetTrafficCursor).toHaveBeenCalledOnce();
    expect(push.connect).toHaveBeenLastCalledWith(expect.objectContaining({ last_sequence: 500 }));
  });

  it("preserves history and cursor when the same database restarts", async () => {
    const rows = seed(true);
    api.queryTraffic.mockImplementation(async ({ record_ids }: { record_ids: string[] }) => page(rows.filter((r) => record_ids.includes(r.id))));
    useTrafficStore.getState().handleTrafficStatistics(statistics());
    await flush();
    expect(useTrafficStore.getState().records).toEqual(rows);
    expect(useTrafficStore.getState()).toMatchObject({ lastSequence: 2000, serverOldestSequence: 1001, hasNewer: true, trafficEpochVersion: 0 });
    expect(push.resetTrafficCursor).not.toHaveBeenCalled();
  });

  it("does not infer replacement from deletion of every cached ID", async () => {
    seed(true);
    api.queryTraffic.mockResolvedValue(page([]));
    useTrafficStore.getState().handleTrafficStatistics(statistics(OLD, 3000, 500));
    await flush();
    expect(useTrafficStore.getState()).toMatchObject({ records: [], lastSequence: 2000, serverOldestSequence: 1001, hasNewer: true, trafficEpochVersion: 0 });
    expect(push.resetTrafficCursor).not.toHaveBeenCalled();
  });

  it.each(["statistics", "delta", "reload", "queued legacy delta"])("confirms a lower modern %s epoch without assigning it to previously untagged legacy rows", async (source) => {
    const rows = seed();
    useTrafficStore.setState({ trafficDatabaseEpoch: null });
    const confirmation = deferred<TrafficStatistics>();
    api.getTrafficStatistics.mockReturnValueOnce(confirmation.promise).mockResolvedValue(statistics(NEW, 601, 600));
    if (source === "queued legacy delta") {
      useTrafficStore.getState().handleTrafficDelta({ ...delta(OLD, rows), database_epoch: undefined });
      useTrafficStore.getState().handleTrafficStatistics(statistics(NEW, 601, 600));
    } else if (source === "statistics") {
      useTrafficStore.getState().handleTrafficStatistics(statistics(NEW, 601, 600));
    } else if (source === "delta") {
      useTrafficStore.getState().handleTrafficDelta(delta(NEW, records("NEWPROCESS", 101, 600), 601));
    } else {
      api.getTrafficUpdates.mockResolvedValueOnce({ ...updates(NEW, records("NEWPROCESS", 101, 600)), server_sequence: 601, server_total: 600 });
      await useTrafficStore.getState().reloadRecords();
    }
    expect(useTrafficStore.getState().trafficDatabaseEpoch).toBeNull();
    expect(useTrafficStore.getState().records).toEqual(rows);
    confirmation.resolve(statistics(NEW, 601, 600));
    await flush();
    expect(useTrafficStore.getState().trafficDatabaseEpoch).toBe(NEW);
    expect(useTrafficStore.getState().lastSequence).toBeNull();
    expect(push.resetTrafficCursor).toHaveBeenCalledOnce();
  });

  it("does not apply a legacy confirmation after a different first identity was established", async () => {
    const rows = seed();
    useTrafficStore.setState({ trafficDatabaseEpoch: null });
    const pending = deferred<TrafficStatistics>();
    api.getTrafficStatistics.mockReturnValueOnce(pending.promise);
    api.queryTraffic.mockImplementation(async ({ record_ids }: { record_ids: string[] }) => page(rows.filter((r) => record_ids.includes(r.id))));
    useTrafficStore.getState().handleTrafficStatistics(statistics(NEW, 601, 600));
    useTrafficStore.getState().handleTrafficStatistics(statistics(OTHER, 2501, 2500));
    pending.resolve(statistics(NEW, 601, 600));
    await flush();
    expect(useTrafficStore.getState().trafficDatabaseEpoch).toBe(OTHER);
    expect(push.resetTrafficCursor).not.toHaveBeenCalled();
  });

  it("keeps cached rows until confirmation succeeds and retries after a request failure", async () => {
    const rows = seed();
    api.getTrafficStatistics.mockRejectedValueOnce(new Error("offline"));
    useTrafficStore.getState().handleTrafficStatistics(statistics(NEW));
    await flush();
    expect(useTrafficStore.getState().records).toEqual(rows);
    expect(useTrafficStore.getState().trafficDatabaseEpoch).toBe(OLD);
    api.getTrafficStatistics.mockResolvedValueOnce(statistics(NEW));
    useTrafficStore.getState().handleTrafficStatistics(statistics(NEW));
    await flush();
    expect(push.resetTrafficCursor).toHaveBeenCalledOnce();
  });

  it("rejects an obsolete epoch notification when fresh HTTP still names the current database", async () => {
    const rows = seed();
    api.getTrafficStatistics.mockResolvedValue(statistics());
    api.queryTraffic.mockImplementation(async ({ record_ids }: { record_ids: string[] }) => page(rows.filter((r) => record_ids.includes(r.id))));
    useTrafficStore.getState().handleTrafficStatistics(statistics(NEW, 2501, 2500));
    await flush();
    expect(push.resetTrafficCursor).not.toHaveBeenCalled();
    expect(useTrafficStore.getState().records).toEqual(rows);
    expect(useTrafficStore.getState().trafficDatabaseEpoch).toBe(OLD);
  });

  it("does not confirm a candidate using a response from another database", async () => {
    const rows = seed();
    const first = deferred<TrafficStatistics>();
    const second = deferred<TrafficStatistics>();
    api.getTrafficStatistics.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    useTrafficStore.getState().handleTrafficStatistics(statistics(NEW));
    useTrafficStore.getState().handleTrafficStatistics(statistics(OTHER));
    first.resolve(statistics(NEW));
    await flush();
    expect(push.resetTrafficCursor).not.toHaveBeenCalled();
    expect(useTrafficStore.getState().records).toEqual(rows);
    second.resolve(statistics(OTHER));
    await flush();
    expect(useTrafficStore.getState().trafficDatabaseEpoch).toBe(OTHER);
    expect(push.resetTrafficCursor).toHaveBeenCalledOnce();
  });

  it.each([false, true])("captures initial HTTP row provenance before first statistics (empty=%s)", async (empty) => {
    api.getTrafficUpdates.mockResolvedValueOnce(updates(OLD, empty ? [] : records("OLDPROCESS", 1001, 2000)));
    api.getTrafficStatistics.mockResolvedValue(statistics(NEW));
    await useTrafficStore.getState().fetchInitialData();
    await flush();
    expect(useTrafficStore.getState().trafficDatabaseEpoch).toBe(NEW);
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(push.resetTrafficCursor).toHaveBeenCalledOnce();
  });

  it("captures a queued initial delta's epoch before statistics bootstrap", async () => {
    useTrafficStore.getState().handleTrafficDelta(delta(OLD, records("OLDPROCESS", 1001, 2000)));
    api.getTrafficStatistics.mockResolvedValue(statistics(NEW));
    useTrafficStore.getState().handleTrafficStatistics(statistics(NEW));
    await flush();
    expect(useTrafficStore.getState().records).toEqual([]);
    expect(useTrafficStore.getState().trafficDatabaseEpoch).toBe(NEW);
    expect(push.resetTrafficCursor).toHaveBeenCalledOnce();
  });

  it("recovers a replacement below this connection's old queued sequence watermark", async () => {
    seed();
    useTrafficStore.getState().handleTrafficDelta(delta(OLD, records("OLDPROCESS", 2001, 2001), 2002));
    api.getTrafficStatistics.mockResolvedValue(statistics(NEW, 601, 600));
    useTrafficStore.getState().handleTrafficStatistics(statistics(NEW, 601, 600));
    await flush();
    useTrafficStore.getState().handleTrafficDelta(delta(NEW, records("NEWPROCESS", 101, 600), 601));
    await flush();
    expect(useTrafficStore.getState().records).toHaveLength(500);
    expect(useTrafficStore.getState().lastSequence).toBe(600);
    expect(push.resetTrafficCursor).toHaveBeenCalledOnce();
  });

  it.each(["fetchUpdates", "catchUpUpdates"] as const)("ignores stale %s responses while polling continues after reset", async (operation) => {
    seed();
    useTrafficStore.setState({ polling: true, usePush: false });
    const old = deferred<TrafficUpdatesResponseCompact>();
    const replay = deferred<TrafficUpdatesResponseCompact>();
    api.getTrafficUpdates.mockReturnValueOnce(old.promise).mockReturnValueOnce(replay.promise);
    const pending = useTrafficStore.getState()[operation]();
    api.getTrafficStatistics.mockResolvedValue(statistics(NEW));
    useTrafficStore.getState().handleTrafficStatistics(statistics(NEW));
    await flush();
    expect(api.getTrafficUpdates).toHaveBeenLastCalledWith(expect.objectContaining({ after_seq: undefined, after_id: undefined }));
    old.resolve(updates(OLD, records("OLDPROCESS", 2001, 2001)));
    await pending;
    expect(useTrafficStore.getState().records).toEqual([]);
    replay.resolve({ ...updates(NEW, records("NEWPROCESS", 1, 1)), has_more: false });
    await flush();
    expect(useTrafficStore.getState().records.map((row) => row.id)).toEqual(["REQ-NEWPROCESS-00000001"]);
    expect(useTrafficStore.getState().pollTimeoutId).not.toBeNull();
  });

  it("retains history and accepts next live row after same-identity unused allocations disappear", async () => {
    seed(true);
    const rows = records("OLDPROCESS", 1, 3);
    useTrafficStore.setState({
      records: rows, recordsMap: new Map(rows.map((row) => [row.id, row])),
      serverSequence: 1001, lastSequence: 3, lastId: rows.at(-1)!.id,
      serverOldestSequence: 1, oldestSequence: 1, serverTotal: 3,
    });
    api.queryTraffic.mockResolvedValue(page(rows));
    useTrafficStore.getState().handleTrafficStatistics(statistics(OLD, 4, 3));
    await flush();
    expect(useTrafficStore.getState().records).toEqual(rows);
    expect(useTrafficStore.getState().hasNewer).toBe(true);
    useTrafficStore.getState().handleTrafficDelta(delta(OLD, records("NEWPROCESS", 4, 4), 5));
    useTrafficStore.getState().handleTrafficStatistics(statistics(OLD, 5, 4));
    await flush();
    expect(useTrafficStore.getState()).toMatchObject({
      records: rows, lastSequence: 4, lastId: "REQ-NEWPROCESS-00000004",
      serverOldestSequence: 1, oldestSequence: 1, hasNewer: true, trafficEpochVersion: 0,
    });
    expect(push.resetTrafficCursor).not.toHaveBeenCalled();
    expect(api.getTrafficStatistics).not.toHaveBeenCalled();
  });

  it("confirms a new delta identity without waiting for a prior ordinary statistics request", async () => {
    const rows = seed();
    const old = deferred<TrafficStatistics>();
    api.getTrafficStatistics.mockReturnValueOnce(old.promise).mockResolvedValue(statistics(NEW));
    const pending = useTrafficStore.getState().fetchTrafficStatistics();
    useTrafficStore.getState().handleTrafficDelta(delta(NEW, records("NEWPROCESS", 1, 1)));
    useTrafficStore.getState().handleTrafficDelta(delta(NEW, records("NEWPROCESS", 2, 2)));
    expect(useTrafficStore.getState().records).toEqual(rows);
    await flush();
    expect(push.resetTrafficCursor).toHaveBeenCalledOnce();
    old.resolve(statistics());
    await pending;
    expect(useTrafficStore.getState().trafficDatabaseEpoch).toBe(NEW);
    expect(useTrafficStore.getState().trafficStatisticsSequence).toBe(2001);
  });

  it("recognizes an empty poll response's new identity and retries confirmation failure", async () => {
    seed();
    useTrafficStore.setState({ polling: true, usePush: false });
    api.getTrafficUpdates.mockResolvedValue({ ...updates(NEW, []), has_more: false, server_total: 0, server_sequence: 1 });
    api.getTrafficStatistics.mockRejectedValueOnce(new Error("offline")).mockResolvedValue(statistics(NEW, 1, 0));
    await useTrafficStore.getState().fetchUpdates();
    await flush();
    expect(push.resetTrafficCursor).not.toHaveBeenCalled();
    expect(useTrafficStore.getState().pollTimeoutId).not.toBeNull();
    await vi.advanceTimersByTimeAsync(1000);
    expect(push.resetTrafficCursor).toHaveBeenCalledOnce();
    expect(useTrafficStore.getState().trafficDatabaseEpoch).toBe(NEW);
    expect(useTrafficStore.getState().pollTimeoutId).not.toBeNull();
  });

  it("does not let a prior reconnect confirmation supersede a newer connection", async () => {
    seed();
    const first = deferred<TrafficStatistics>();
    const second = deferred<TrafficStatistics>();
    api.getTrafficStatistics.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    useTrafficStore.getState().handleTrafficStatistics(statistics(NEW));
    const onConnection = push.onConnectionChange.mock.calls.at(-1)![0];
    onConnection({ connected: false });
    onConnection({ connected: true });
    useTrafficStore.getState().handleTrafficStatistics(statistics(OTHER));
    first.resolve(statistics(NEW));
    await flush();
    expect(push.resetTrafficCursor).not.toHaveBeenCalled();
    second.resolve(statistics(OTHER));
    await flush();
    expect(useTrafficStore.getState().trafficDatabaseEpoch).toBe(OTHER);
    expect(push.resetTrafficCursor).toHaveBeenCalledOnce();
  });

  it.each(["detail", "bodies"])("discards old database %s that resolve after reset", async (phase) => {
    const [row] = seed();
    const detail = deferred<TrafficRecord>();
    const body = deferred<string>();
    const raw = deferred<{ data: string; size: number }>();
    const record: TrafficRecord = {
      ...row, request_headers: [], response_headers: [], request_body: null,
      response_body: null, request_content_type: null, matched_rules: [],
      raw_request_body_ref: { Inline: { data: "old-request" } },
      raw_response_body_ref: { Inline: { data: "old-response" } },
    };
    api.getTrafficDetail.mockReturnValueOnce(detail.promise);
    api.getRequestBody.mockReturnValueOnce(body.promise);
    api.getResponseBody.mockReturnValueOnce(body.promise);
    api.getRequestBodyContent.mockReturnValueOnce(raw.promise);
    api.getResponseBodyContent.mockReturnValueOnce(raw.promise);
    const pending = useTrafficStore.getState().fetchTrafficDetail(row.id);
    if (phase === "bodies") { detail.resolve(record); await pending; }
    api.getTrafficStatistics.mockResolvedValue(statistics(NEW));
    useTrafficStore.getState().handleTrafficStatistics(statistics(NEW));
    await flush();
    detail.resolve(record);
    body.resolve("old body");
    raw.resolve({ data: "old raw body", size: 12 });
    await pending;
    await flush();
    expect(useTrafficStore.getState()).toMatchObject({ currentRecord: null, requestBody: null, responseBody: null, requestRawBody: null, responseRawBody: null, detailError: null, detailLoading: false });
  });
});
