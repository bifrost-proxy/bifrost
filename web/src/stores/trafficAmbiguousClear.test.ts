import "./trafficStoreTestFixture";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useTrafficStore } from "./useTrafficStore";
import {
  apiMocks,
  pushMocks,
  clearAndSynchronizeForTest,
  makePage,
  makeRecord,
  makeRecordRange,
} from "./trafficStoreTestFixture";
import type { TrafficUpdatesResponseCompact } from "../types";

beforeEach(async () => {
  useTrafficStore.getState().disablePush();
  apiMocks.getTrafficUpdates.mockReset();
  apiMocks.clearTraffic.mockReset().mockResolvedValue(undefined);
  await clearAndSynchronizeForTest();
  useTrafficStore.setState(useTrafficStore.getInitialState());
  apiMocks.getTrafficUpdates.mockReset();
  apiMocks.queryTraffic.mockReset();
  pushMocks.resetTrafficCursor.mockClear();
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
  records: ReturnType<typeof makeRecord>[],
  total = records.length,
  sequence = 5,
): TrafficUpdatesResponseCompact => ({
  new_records: makePage(records, false, "forward").records,
  updated_records: [],
  has_more: false,
  server_total: total,
  server_sequence: sequence,
});
const seed = (paused = false) => {
  const records = makeRecordRange(1, 3);
  records[1].status = 0;
  useTrafficStore.setState({
    records,
    recordsMap: new Map(records.map((r) => [r.id, r])),
    serverTotal: 3,
    serverSequence: 4,
    usePush: false,
    paused,
  });
  return records;
};

it.each([true, false])(
  "reconciles a committed full clear whose response is lost without push (paused=%s)",
  async (paused) => {
    const records = seed(paused);
    const later = makeRecord("4", "/after-clear");
    apiMocks.clearTraffic.mockRejectedValueOnce(
      new Error("DELETE response lost"),
    );
    apiMocks.getTrafficUpdates.mockResolvedValueOnce(
      snapshot([records[1], later]),
    );
    apiMocks.queryTraffic.mockResolvedValueOnce({
      records: [],
      total: 0,
      has_more: false,
      server_sequence: 5,
    });
    expect(await clearAndSynchronizeForTest()).toBe(false);
    expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
      "2",
      "4",
    ]);
    expect(useTrafficStore.getState()).toMatchObject({
      serverTotal: 2,
      clearing: false,
      error: "DELETE response lost",
      paused,
    });
    expect(useTrafficStore.getState().pendingIds.has("2")).toBe(true);
    expect(apiMocks.queryTraffic).toHaveBeenCalledWith({
      record_ids: ["1", "3"],
      limit: 2,
    });
    expect(pushMocks.resetTrafficCursor).toHaveBeenCalledOnce();
  },
);

it("keeps all rows when an unsuccessful full DELETE did not commit", async () => {
  const records = seed();
  apiMocks.clearTraffic.mockResolvedValueOnce({
    success: false,
    message: "injected rejection",
  });
  apiMocks.getTrafficUpdates.mockResolvedValueOnce(snapshot(records, 3, 4));
  expect(await clearAndSynchronizeForTest()).toBe(false);
  expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual([
    "1",
    "2",
    "3",
  ]);
  expect(useTrafficStore.getState()).toMatchObject({
    serverTotal: 3,
    error: "injected rejection",
    clearing: false,
  });
});

it("retains writes arriving during ambiguous full-clear membership recovery", async () => {
  seed();
  apiMocks.clearTraffic.mockRejectedValueOnce(
    new Error("DELETE response lost"),
  );
  let finish!: (value: TrafficUpdatesResponseCompact) => void;
  const response = new Promise<TrafficUpdatesResponseCompact>((resolve) => {
    finish = resolve;
  });
  apiMocks.getTrafficUpdates.mockReturnValueOnce(response);
  const later = makeRecord("4", "/after-clear", { response_size: 999 });
  apiMocks.queryTraffic.mockResolvedValueOnce({
    records: makePage([later], false, "forward").records,
    total: 1,
    has_more: false,
    server_sequence: 5,
  });
  const clear = clearAndSynchronizeForTest();
  for (let i = 0; i < 10; i++) await Promise.resolve();
  useTrafficStore
    .getState()
    .handleTrafficPush({
      new_records: [later],
      updated_records: [],
      has_more: false,
      server_total: 1,
      server_sequence: 5,
    });
  finish(snapshot([], 0, 4));
  expect(await clear).toBe(false);
  await vi.advanceTimersByTimeAsync(200);
  expect(useTrafficStore.getState().records.map((r) => r.id)).toEqual(["4"]);
  expect(useTrafficStore.getState().recordsMap.get("4")?.response_size).toBe(
    999,
  );
  expect(useTrafficStore.getState().serverTotal).toBe(1);
});

it("removes an independently selected deleted detail outside the loaded window", async () => {
  seed();
  const detail = makeRecord("90", "/outside-window");
  useTrafficStore.setState({
    selectedId: "90",
    currentRecord: detail as never,
    requestBody: "old request",
    responseBody: "old response",
  });
  apiMocks.clearTraffic.mockRejectedValueOnce(
    new Error("DELETE response lost"),
  );
  apiMocks.getTrafficUpdates.mockResolvedValueOnce(snapshot([], 0));
  apiMocks.queryTraffic.mockResolvedValueOnce({
    records: [],
    total: 0,
    has_more: false,
    server_sequence: 5,
  });
  expect(await clearAndSynchronizeForTest()).toBe(false);
  expect(apiMocks.queryTraffic.mock.calls[0][0].record_ids).toContain("90");
  expect(useTrafficStore.getState()).toMatchObject({
    selectedId: undefined,
    currentRecord: null,
    requestBody: null,
    responseBody: null,
  });
});
