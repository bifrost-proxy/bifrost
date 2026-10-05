import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getTrafficPage } from "../../api/traffic";
import { useFilterPanelStore } from "../../stores/useFilterPanelStore";
import { useBreakpointStore } from "../../stores/useBreakpointStore";
import { usePerformanceModeStore } from "../../stores/usePerformanceModeStore";
import { useTrafficStore } from "../../stores/useTrafficStore";
import type { TrafficQueryResponse, TrafficSummary } from "../../types";
import Traffic from "./index";

vi.mock("../../api/traffic", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../api/traffic")>()),
  getTrafficPage: vi.fn(),
}));
vi.mock("../../services/pushService", () => ({
  default: {
    connect: vi.fn(),
    getSubscription: () => ({}),
    updateSubscription: vi.fn(),
    onSettingsUpdate: () => () => {},
  },
}));
vi.mock("antd", () => ({
  theme: { useToken: () => ({ token: {} }) },
  message: { error: vi.fn(), info: vi.fn(), success: vi.fn() },
  Button: () => null,
  Spin: () => null,
}));
vi.mock("@ant-design/icons", () => ({ ThunderboltOutlined: () => null }));
vi.mock("../../components/TrafficDetail", () => ({ default: () => null }));
vi.mock("../../components/Toolbar", () => ({ default: () => null }));
vi.mock("../../components/FilterBar", () => ({ default: () => null }));
vi.mock("../../components/FilterPanel", () => ({ default: () => null }));
vi.mock("../../components/SearchMode", () => ({ default: () => null }));
vi.mock("../../components/ThreeSplitPane", () => ({
  default: ({ center }: { center: ReactNode }) => center,
}));
vi.mock("../../components/TrafficTable/VirtualTrafficTable", () => ({
  default: ({
    data,
    hasOlder,
    onLoadOlder,
  }: {
    data: TrafficSummary[];
    hasOlder: boolean;
    onLoadOlder: () => Promise<void>;
  }) => (
    <>
      <output data-testid="traffic-ids">
        {data.map((record) => record.id).join(",")}
      </output>
      <button disabled={!hasOlder} onClick={() => void onLoadOlder()}>
        Load older
      </button>
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

function page(records: TrafficSummary[], hasMore = false): TrafficQueryResponse {
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

function deferredPage() {
  let resolve!: (page: TrafficQueryResponse) => void;
  const promise = new Promise<TrafficQueryResponse>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("Traffic filters across confirmed database epochs", () => {
  let root: Root;
  let container: HTMLDivElement;

  async function render() {
    await act(async () => {
      root.render(
        <MemoryRouter>
          <Traffic />
        </MemoryRouter>,
      );
    });
  }

  function displayedIds() {
    const text = container.querySelector(
      '[data-testid="traffic-ids"]',
    )!.textContent;
    return text ? text.split(",") : [];
  }

  async function resetEpoch() {
    await act(async () => {
      useTrafficStore.setState((state) => ({
        records: [],
        recordsMap: new Map(),
        serverOldestSequence: null,
        trafficEpochVersion: state.trafficEpochVersion + 1,
        recordsMutation: {
          version: state.recordsMutation.version + 1,
          reset: true,
          inserted: [],
          updated: [],
          deletedIds: [],
        },
      }));
    });
  }

  beforeEach(() => {
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    vi.useFakeTimers();
    vi.clearAllMocks();
    vi.mocked(getTrafficPage).mockReset();
    useBreakpointStore.setState({
      connectPush: vi.fn(),
      fetchSettings: vi.fn().mockResolvedValue(undefined),
    });
    useTrafficStore.setState({
      ...useTrafficStore.getInitialState(),
      filterConditions: [
        { id: "target", field: "path", operator: "contains", value: "/target" },
      ],
    });
    useFilterPanelStore.setState({
      ...useFilterPanelStore.getInitialState(),
      initialized: true,
    });
    usePerformanceModeStore.setState({ superPerformanceMode: false });
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("rescans unchanged filters beyond the new epoch's latest 500 records", async () => {
    const tailResponse = deferredPage();
    const latestTail = Array.from({ length: 500 }, (_, index) =>
      record(101 + index, false),
    );
    vi.mocked(getTrafficPage)
      .mockResolvedValueOnce(page([record(10_000)]))
      .mockReturnValueOnce(tailResponse.promise)
      .mockResolvedValueOnce(page([record(50)]));
    await render();
    expect(displayedIds()).toEqual(["record-10000"]);

    await resetEpoch();
    expect(getTrafficPage).toHaveBeenCalledTimes(2);
    expect(displayedIds()).toEqual([]);
    expect(
      container.querySelector('[data-testid="traffic-filter-loading"]'),
    ).not.toBeNull();

    // Reconnect may push its latest 500 rows while the full-history scan waits.
    await act(async () => {
      useTrafficStore.setState((state) => ({
        records: latestTail,
        recordsMutation: {
          version: state.recordsMutation.version + 1,
          reset: false,
          inserted: latestTail,
          updated: [],
          deletedIds: [],
        },
      }));
      tailResponse.resolve(page(latestTail, true));
    });
    await act(async () => vi.runAllTimersAsync());

    expect(getTrafficPage).toHaveBeenLastCalledWith({
      cursor: 101,
      limit: 500,
      direction: "backward",
    });
    expect(displayedIds()).toEqual(["record-50"]);
    expect(
      container.querySelector('[data-testid="traffic-filter-loading"]'),
    ).toBeNull();
  });

  it("discards an old epoch's pending scan after the replacement scan completes", async () => {
    const oldResponse = deferredPage();
    vi.mocked(getTrafficPage)
      .mockReturnValueOnce(oldResponse.promise)
      .mockResolvedValueOnce(page([record(50)]));
    await render();
    await resetEpoch();
    expect(displayedIds()).toEqual(["record-50"]);

    await act(async () => oldResponse.resolve(page([record(10_000)])));
    expect(displayedIds()).toEqual(["record-50"]);
  });

  it("retains filtered pagination during ordinary membership reconciliation", async () => {
    const initial = Array.from({ length: 500 }, (_, index) => record(501 + index));
    vi.mocked(getTrafficPage)
      .mockResolvedValueOnce(page(initial, true))
      .mockResolvedValueOnce(page([record(50)]));
    await render();
    expect(displayedIds()).toHaveLength(500);

    await act(async () => {
      useTrafficStore.setState((state) => ({
        recordsMutation: {
          version: state.recordsMutation.version + 1,
          reset: false,
          inserted: [],
          updated: [],
          deletedIds: ["record-600"],
        },
      }));
    });
    expect(getTrafficPage).toHaveBeenCalledOnce();
    expect(displayedIds()).toHaveLength(499);
    expect(displayedIds()).not.toContain("record-600");
    expect(container.querySelector("button")!.disabled).toBe(false);

    await act(async () => container.querySelector("button")!.click());
    expect(getTrafficPage).toHaveBeenLastCalledWith({
      cursor: 501,
      limit: 500,
      direction: "backward",
    });
    expect(displayedIds()).toHaveLength(500);
    expect(displayedIds()[0]).toBe("record-50");
    expect(displayedIds()).not.toContain("record-600");
  });

  it("still cancels a pending scan when traffic is explicitly cleared", async () => {
    const response = deferredPage();
    vi.mocked(getTrafficPage).mockReturnValueOnce(response.promise);
    await render();
    await act(async () => {
      useTrafficStore.setState((state) => ({
        records: [],
        recordsMutation: {
          version: state.recordsMutation.version + 1,
          reset: true,
          inserted: [],
          updated: [],
          deletedIds: [],
        },
      }));
    });
    await act(async () => response.resolve(page([record(50)])));
    expect(displayedIds()).toEqual([]);
    expect(getTrafficPage).toHaveBeenCalledOnce();
    expect(
      container.querySelector('[data-testid="traffic-filter-loading"]'),
    ).toBeNull();
  });
});
