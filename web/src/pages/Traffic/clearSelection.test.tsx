import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearTraffic as clearTrafficApi,
  getTrafficUpdates,
  getTrafficDetail,
  getRequestBody,
  getResponseBody,
  getTrafficPage,
  getTrafficStatistics,
  queryTraffic,
} from "../../api/traffic";
import { useFilterPanelStore } from "../../stores/useFilterPanelStore";
import { useBreakpointStore } from "../../stores/useBreakpointStore";
import { usePerformanceModeStore } from "../../stores/usePerformanceModeStore";
import { useTrafficStore } from "../../stores/useTrafficStore";
import type {
  TrafficQueryResponse,
  TrafficSummary,
  TrafficRecord,
} from "../../types";
import Traffic from "./index";

vi.mock("../../api/traffic", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../api/traffic")>()),
  getTrafficPage: vi.fn(),
  getTrafficStatistics: vi.fn(),
  queryTraffic: vi.fn(),
  clearTraffic: vi.fn().mockResolvedValue({ success: true }),
  getTrafficUpdates: vi.fn(),
  getTrafficDetail: vi.fn(),
  getRequestBody: vi.fn(),
  getResponseBody: vi.fn(),
}));
vi.mock("../../services/pushService", () => ({
  default: {
    connect: vi.fn(),
    getSubscription: () => ({}),
    updateSubscription: vi.fn(),
    onSettingsUpdate: () => () => {},
    onTrafficUpdates: () => () => {},
    onTrafficDelta: () => () => {},
    onTrafficDeleted: () => () => {},
    onTrafficStatistics: () => () => {},
    onConnectionChange: () => () => {},
    disconnectIfIdle: vi.fn(),
    resetTrafficCursor: vi.fn(),
  },
}));
vi.mock("antd", () => ({
  theme: { useToken: () => ({ token: {} }) },
  message: { error: vi.fn(), info: vi.fn(), success: vi.fn() },
  Button: () => null,
  Spin: () => null,
}));
vi.mock("@ant-design/icons", () => ({ ThunderboltOutlined: () => null }));
vi.mock("../../components/TrafficDetail", () => ({
  default: function SelectedDetail() {
    const detail = useTrafficStore((state) => state.currentRecord);
    const body = useTrafficStore((state) => state.requestBody);
    return (
      <output data-testid="selected-detail">
        {detail?.id}:{body}
      </output>
    );
  },
}));
vi.mock("../../components/Toolbar", () => ({
  default: ({
    onClearAll,
    clearing,
  }: {
    onClearAll: () => Promise<void>;
    clearing: boolean;
  }) => (
    <button
      data-testid="clear"
      disabled={clearing}
      onClick={() => void onClearAll()}
    >
      Clear
    </button>
  ),
}));
vi.mock("../../components/FilterBar", () => ({ default: () => null }));
vi.mock("../../components/FilterPanel", () => ({ default: () => null }));
vi.mock("../../components/SearchMode", () => ({ default: () => null }));
vi.mock("../../components/ThreeSplitPane", () => ({
  default: ({ center, right }: { center: ReactNode; right: ReactNode }) => (
    <>
      {center}
      {right}
    </>
  ),
}));
vi.mock("../../components/TrafficTable/VirtualTrafficTable", () => ({
  default: ({
    data,
    onSelect,
  }: {
    data: TrafficSummary[];
    onSelect: (row: TrafficSummary) => void;
  }) => (
    <>
      {data.map((row) => (
        <button
          key={row.id}
          data-testid={`select-${row.id}`}
          onClick={() => onSelect(row)}
        >
          Select {row.id}
        </button>
      ))}
    </>
  ),
}));

function record(sequence: number, match = true): TrafficSummary {
  return {
    id: `record-${sequence}`,
    sequence,
    timestamp: sequence,
    method: "GET",
    url: `http://example.test/${match ? "target" : "noise"}/${sequence}`,
    status: 200,
    content_type: "text/plain",
    request_size: 0,
    response_size: 2,
    duration_ms: 1,
    host: "example.test",
    path: `/${match ? "target" : "noise"}/${sequence}`,
    protocol: "HTTP/1.1",
    client_ip: "127.0.0.1",
    has_rule_hit: false,
    matched_rule_count: 0,
    matched_protocols: [],
    start_time: "2026-10-05T00:00:00Z",
  };
}

function page(
  records: TrafficSummary[],
  hasMore = false,
): TrafficQueryResponse {
  return {
    records: records
      .slice()
      .reverse()
      .map((record) => ({
        id: record.id,
        seq: record.sequence,
        ts: record.timestamp,
        m: record.method,
        h: record.host,
        p: record.path,
        s: record.status,
        ct: record.content_type,
        req_sz: record.request_size,
        res_sz: record.response_size,
        dur: record.duration_ms,
        proto: record.protocol,
        cip: record.client_ip,
        flags: 0,
        fc: 0,
        st: record.start_time,
        rc: 0,
        rp: [],
      })),
    next_cursor: records[0]?.sequence ?? null,
    prev_cursor: records.at(-1)?.sequence ?? null,
    has_more: hasMore,
    total: records.length,
    server_sequence: records.at(-1)?.sequence ?? 0,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
function statistics(epoch = "old-db", count = 1, sequence = 501) {
  return {
    database_epoch: epoch,
    total_requests: count,
    server_sequence: sequence,
    client_ips: {},
    proxy_ports: {},
    applications: {},
    account_names: {},
    domains: {},
  };
}
function detail(row: TrafficSummary): TrafficRecord {
  return {
    ...row,
    request_headers: [],
    response_headers: [],
    request_body: null,
    response_body: null,
    request_content_type: null,
    matched_rules: [],
  };
}

describe("Traffic clear callback preserves newer selections", () => {
  let root: Root;
  let container: HTMLDivElement;
  const original = record(1);

  beforeEach(async () => {
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    vi.useFakeTimers({
      toFake: [
        "setTimeout",
        "clearTimeout",
        "requestAnimationFrame",
        "cancelAnimationFrame",
      ],
    });
    vi.mocked(clearTrafficApi).mockReset().mockResolvedValue({ success: true });
    vi.mocked(getTrafficUpdates).mockReset().mockResolvedValue({
      new_records: [],
      updated_records: [],
      has_more: false,
      server_total: 0,
      server_sequence: 0,
    });
    vi.mocked(getTrafficStatistics).mockReset().mockResolvedValue(statistics());
    useTrafficStore.getState().disablePush();
    await act(async () => {
      await useTrafficStore.getState().clearTraffic();
    });
    vi.clearAllMocks();
    useTrafficStore.setState({
      ...useTrafficStore.getInitialState(),
      initialized: true,
      records: [original],
      recordsMap: new Map([[original.id, original]]),
      serverTotal: 1,
      serverSequence: 501,
      trafficDatabaseEpoch: "old-db",
      usePush: false,
    });
    vi.mocked(getTrafficPage).mockResolvedValue(page([original]));
    vi.mocked(queryTraffic).mockResolvedValue(page([original]));
    vi.mocked(getTrafficDetail).mockImplementation(async (id) =>
      detail(record(Number(id.split("-").at(-1)))),
    );
    vi.mocked(getRequestBody).mockImplementation(async (id) => `request:${id}`);
    vi.mocked(getResponseBody).mockImplementation(
      async (id) => `response:${id}`,
    );
    vi.mocked(getTrafficUpdates).mockImplementation(async () => {
      const state = useTrafficStore.getState();
      return {
        new_records: page(state.records).records,
        updated_records: [],
        has_more: false,
        server_total: state.serverTotal,
        server_sequence: state.serverSequence,
      };
    });
    useBreakpointStore.setState({
      connectPush: vi.fn(),
      fetchSettings: vi.fn().mockResolvedValue(undefined),
    });
    useFilterPanelStore.setState({
      ...useFilterPanelStore.getInitialState(),
      initialized: true,
      detailPanelCollapsed: false,
    });
    usePerformanceModeStore.setState({ superPerformanceMode: false });
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
    await act(async () => {
      root.render(
        <MemoryRouter>
          <Traffic />
        </MemoryRouter>,
      );
    });
  });
  afterEach(async () => {
    await act(async () => {
      useTrafficStore.getState().disablePush();
      root.unmount();
    });
    container.remove();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });
  async function click(testId: string) {
    const button = container.querySelector<HTMLButtonElement>(
      `[data-testid="${testId}"]`,
    );
    expect(button).not.toBeNull();
    await act(async () => {
      button!.click();
    });
  }
  async function pushAndSelect(row: TrafficSummary, epoch = "old-db") {
    await act(async () => {
      useTrafficStore.getState().handleTrafficPush({
        database_epoch: epoch,
        new_records: [row],
        updated_records: [],
        has_more: false,
        server_total: 1,
        server_sequence: 502,
      });
      await vi.advanceTimersByTimeAsync(200);
    });
    await click(`select-${row.id}`);
    expect(useTrafficStore.getState().selectedId).toBe(row.id);
    expect(
      container.querySelector('[data-testid="selected-detail"]')?.textContent,
    ).toBe(`${row.id}:request:${row.id}`);
  }
  function expectSelected(row: TrafficSummary) {
    expect(useTrafficStore.getState()).toMatchObject({
      selectedId: row.id,
      currentRecord: { id: row.id },
      requestBody: `request:${row.id}`,
      responseBody: `response:${row.id}`,
    });
    expect(
      container.querySelector('[data-testid="selected-detail"]')?.textContent,
    ).toBe(`${row.id}:request:${row.id}`);
  }

  it("keeps the replacement database selection when the old clear response completes", async () => {
    const response = deferred<{ success: boolean }>();
    vi.mocked(clearTrafficApi).mockReturnValueOnce(response.promise);
    await click("clear");
    expect(useTrafficStore.getState().clearing).toBe(true);
    const fresh = record(2);
    vi.mocked(getTrafficStatistics).mockResolvedValue(
      statistics("new-db", 1, 502),
    );
    await act(async () => {
      useTrafficStore
        .getState()
        .handleTrafficStatistics(statistics("new-db", 1, 502));
      await vi.advanceTimersByTimeAsync(200);
    });
    expect(useTrafficStore.getState().trafficDatabaseEpoch).toBe("new-db");
    await pushAndSelect(fresh, "new-db");
    await act(async () => {
      response.resolve({ success: true });
    });
    expectSelected(fresh);
    expect(useTrafficStore.getState().recordsMap.has(fresh.id)).toBe(true);
  });

  it("keeps a new same-database selection while the page still awaits the clear result", async () => {
    const pageBoundary = deferred<void>();
    const clear = useTrafficStore.getState().clearTraffic;
    await act(async () => {
      useTrafficStore.setState({
        clearTraffic: async () => {
          const result = await clear();
          await pageBoundary.promise;
          return result;
        },
      });
    });
    await click("select-record-1");
    await click("clear");
    expect(useTrafficStore.getState().selectedId).toBeUndefined();
    const fresh = record(2);
    await pushAndSelect(fresh);
    await act(async () => {
      pageBoundary.resolve();
    });
    expectSelected(fresh);
  });

  it("still removes the cleared selection through the successful store action", async () => {
    await click("select-record-1");
    expectSelected(original);
    await click("clear");
    expect(useTrafficStore.getState()).toMatchObject({
      selectedId: undefined,
      currentRecord: null,
      requestBody: null,
      responseBody: null,
    });
    expect(
      container.querySelector('[data-testid="selected-detail"]')?.textContent ??
        "",
    ).not.toContain(original.id);
  });

  it("preserves the existing selection when clearing fails", async () => {
    await click("select-record-1");
    vi.mocked(clearTrafficApi).mockRejectedValueOnce(
      new Error("clear rejected"),
    );
    await click("clear");
    expectSelected(original);
    expect(useTrafficStore.getState().error).toBe("clear rejected");
  });
});
