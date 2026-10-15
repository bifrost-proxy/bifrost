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
import type { TrafficRecord } from "../types";

beforeEach(async () => {
  useTrafficStore.getState().disablePush();
  apiMocks.getTrafficUpdates.mockReset();
  apiMocks.clearTraffic.mockResolvedValue(undefined);
  await clearAndSynchronizeForTest();
  useTrafficStore.setState(useTrafficStore.getInitialState());
  apiMocks.getTrafficUpdates.mockReset();
  apiMocks.queryTraffic.mockReset();
  apiMocks.getTrafficDetail.mockReset();
  apiMocks.getRequestBody.mockReset().mockResolvedValue(null);
  apiMocks.getResponseBody.mockReset().mockResolvedValue(null);
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

const record = makeRecord("1", "/selected");
const detail: TrafficRecord = {
  ...record,
  request_headers: [],
  response_headers: [],
  request_body: "request",
  response_body: "response",
  request_content_type: null,
  matched_rules: [],
};
const rawBody = { data: "raw" };
const select = () =>
  useTrafficStore.setState({
    records: [record],
    recordsMap: new Map([[record.id, record]]),
    currentRecord: detail,
    selectedId: record.id,
    requestBody: "request",
    responseBody: "response",
    requestRawBody: rawBody,
    responseRawBody: rawBody,
    detailLoading: true,
    serverTotal: 1,
    serverSequence: 2,
  });
const expectRemovedDetail = () =>
  expect(useTrafficStore.getState()).toMatchObject({
    currentRecord: null,
    selectedId: undefined,
    requestBody: null,
    responseBody: null,
    requestRawBody: null,
    responseRawBody: null,
    detailLoading: false,
    detailError: "Request was deleted",
  });

it.each([false, true])(
  "clears detail and bodies after a confirmed membership miss (GET fails=%s)",
  async (readFails) => {
    select();
    const requestedIds = new Set([record.id]);
    apiMocks.queryTraffic.mockImplementationOnce(async () => {
      if (readFails)
        useTrafficStore.getState().handleTrafficDeleted(["unrelated"], 0);
      return { records: [], total: 0, has_more: false, server_sequence: 2 };
    });
    apiMocks.getTrafficUpdates.mockResolvedValueOnce({
      new_records: [],
      updated_records: [],
      has_more: false,
      server_total: 0,
      server_sequence: 2,
    });
    if (readFails)
      apiMocks.getTrafficUpdates.mockRejectedValueOnce(
        new Error("replacement failed"),
      );
    await useTrafficStore.getState().reloadRecords(true, {
      ids: new Set(),
      requestedIds,
      liveRecords: new Map(),
    });
    expect(useTrafficStore.getState().recordsMap.has(record.id)).toBe(false);
    expectRemovedDetail();
  },
);

it("does not restore an in-flight detail response after membership removes the selection", async () => {
  select();
  let finish!: (record: TrafficRecord) => void;
  apiMocks.getTrafficDetail.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  apiMocks.getRequestBody.mockResolvedValueOnce("late request");
  apiMocks.getResponseBody.mockResolvedValueOnce("late response");
  const fetch = useTrafficStore.getState().fetchTrafficDetail(record.id);
  apiMocks.getTrafficUpdates.mockResolvedValueOnce({
    new_records: [],
    updated_records: [],
    has_more: false,
    server_total: 0,
    server_sequence: 2,
  });
  apiMocks.queryTraffic.mockResolvedValueOnce({
    records: [],
    total: 0,
    has_more: false,
    server_sequence: 2,
  });
  await useTrafficStore.getState().reloadRecords(true, {
    ids: new Set(),
    requestedIds: new Set([record.id]),
    liveRecords: new Map(),
  });
  finish(detail);
  await fetch;
  expectRemovedDetail();
});

it("retains detail and bodies for a surviving selected row outside the latest window", async () => {
  select();
  const latest = makeRecordRange(2, 1001);
  apiMocks.getTrafficUpdates.mockResolvedValueOnce({
    new_records: makePage(latest, false, "forward").records,
    updated_records: [],
    has_more: true,
    server_total: 1001,
    server_sequence: 1002,
  });
  await useTrafficStore.getState().reloadRecords(true);
  expect(useTrafficStore.getState().recordsMap.has(record.id)).toBe(false);
  expect(useTrafficStore.getState()).toMatchObject({
    currentRecord: detail,
    selectedId: record.id,
    requestBody: "request",
    responseBody: "response",
    requestRawBody: rawBody,
    responseRawBody: rawBody,
  });
});

it("clears selected detail below the authoritative retention floor", async () => {
  select();
  useTrafficStore.setState({
    records: [],
    recordsMap: new Map(),
    serverOldestSequence: 2,
  });
  const survivor = makeRecord("2", "/retained");
  apiMocks.getTrafficUpdates.mockResolvedValueOnce({
    new_records: makePage([survivor], false, "forward").records,
    updated_records: [],
    has_more: false,
    server_total: 1,
    server_sequence: 3,
  });
  await useTrafficStore.getState().reloadRecords(true);
  expectRemovedDetail();
});

it("retires a confirmed pending ID when the list is already empty and replacement GET fails", async () => {
  select();
  useTrafficStore.setState({
    records: [],
    recordsMap: new Map(),
    serverTotal: 0,
    pendingIds: new Set([record.id]),
  });
  pushMocks.updateSubscription.mockClear();
  apiMocks.getTrafficUpdates.mockResolvedValueOnce({
    new_records: [],
    updated_records: [],
    has_more: false,
    server_total: 0,
    server_sequence: 2,
  });
  apiMocks.getTrafficUpdates.mockRejectedValueOnce(
    new Error("replacement failed"),
  );
  apiMocks.queryTraffic.mockImplementationOnce(async () => {
    useTrafficStore.getState().handleTrafficDeleted(["unrelated"], 0);
    return { records: [], total: 0, has_more: false, server_sequence: 2 };
  });
  await useTrafficStore.getState().reloadRecords(true, {
    ids: new Set(),
    requestedIds: new Set([record.id]),
    liveRecords: new Map(),
  });
  expect(useTrafficStore.getState().pendingIds.size).toBe(0);
  expect(pushMocks.updateSubscription).toHaveBeenLastCalledWith(
    expect.objectContaining({ pending_ids: [] }),
  );
  expectRemovedDetail();
});

it("clears detail when a complete non-preserving snapshot proves the selected row absent", async () => {
  select();
  apiMocks.getTrafficUpdates.mockResolvedValueOnce({
    new_records: [],
    updated_records: [],
    has_more: false,
    server_total: 0,
    server_sequence: 2,
  });
  await useTrafficStore.getState().reloadRecords();
  expectRemovedDetail();
});

it("retains detail outside a bounded non-preserving snapshot", async () => {
  select();
  apiMocks.getTrafficUpdates.mockResolvedValueOnce({
    new_records: makePage(makeRecordRange(2, 501), false, "forward").records,
    updated_records: [],
    has_more: true,
    server_total: 1001,
    server_sequence: 1002,
  });
  await useTrafficStore.getState().reloadRecords();
  expect(useTrafficStore.getState().recordsMap.has(record.id)).toBe(false);
  expect(useTrafficStore.getState()).toMatchObject({
    currentRecord: detail,
    selectedId: record.id,
    requestBody: "request",
    responseBody: "response",
  });
});

it("does not use an old complete snapshot to clear detail with a newly accepted write", async () => {
  select();
  let finish!: (value: unknown) => void;
  apiMocks.getTrafficUpdates.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const reload = useTrafficStore.getState().reloadRecords();
  useTrafficStore.getState().handleTrafficPush({
    new_records: [],
    updated_records: [{ ...record, response_size: 777 }],
    has_more: false,
    server_total: 1,
    server_sequence: 2,
  });
  finish({
    new_records: [],
    updated_records: [],
    has_more: false,
    server_total: 0,
    server_sequence: 2,
  });
  await reload;
  expect(useTrafficStore.getState()).toMatchObject({
    currentRecord: detail,
    selectedId: record.id,
    requestBody: "request",
    responseBody: "response",
  });
});

it("clears a late detail response received before a complete missing-row snapshot commits", async () => {
  select();
  let finishDetail!: (value: TrafficRecord) => void;
  let finishSnapshot!: (value: unknown) => void;
  apiMocks.getTrafficDetail.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finishDetail = resolve;
      }),
  );
  const fetch = useTrafficStore.getState().fetchTrafficDetail(record.id);
  apiMocks.getTrafficUpdates.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finishSnapshot = resolve;
      }),
  );
  const reload = useTrafficStore.getState().reloadRecords();
  finishDetail({ ...detail, response_size: 1234 });
  await fetch;
  finishSnapshot({
    new_records: [],
    updated_records: [],
    has_more: false,
    server_total: 0,
    server_sequence: 2,
  });
  await reload;
  expectRemovedDetail();
});

it("does not clear a different selection made while a complete snapshot is pending", async () => {
  select();
  let finish!: (value: unknown) => void;
  apiMocks.getTrafficUpdates.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const reload = useTrafficStore.getState().reloadRecords();
  const next = { ...detail, ...makeRecord("2", "/new-selection") };
  useTrafficStore.setState({
    selectedId: next.id,
    currentRecord: next,
    requestBody: "new request",
    responseBody: "new response",
  });
  finish({
    new_records: [],
    updated_records: [],
    has_more: false,
    server_total: 0,
    server_sequence: 2,
  });
  await reload;
  expect(useTrafficStore.getState()).toMatchObject({
    selectedId: next.id,
    currentRecord: next,
    requestBody: "new request",
    responseBody: "new response",
  });
});

it("retires an independently selected pending detail absent from a complete snapshot", async () => {
  useTrafficStore.setState({ selectedId: record.id });
  let finishDetail!: (value: TrafficRecord) => void;
  apiMocks.getTrafficDetail.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finishDetail = resolve;
      }),
  );
  const fetch = useTrafficStore.getState().fetchTrafficDetail(record.id);
  apiMocks.getTrafficUpdates.mockResolvedValueOnce({
    new_records: [],
    updated_records: [],
    has_more: false,
    server_total: 0,
    server_sequence: 2,
  });
  await useTrafficStore.getState().reloadRecords();
  expectRemovedDetail();
  finishDetail(detail);
  await fetch;
  expectRemovedDetail();
});
