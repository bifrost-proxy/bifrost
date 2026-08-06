// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MetricsSnapshot, SystemOverview } from "../types";

vi.mock("../api", () => ({}));
vi.mock("../services/pushService", () => ({
  METRICS_INTERVAL_DEFAULT_MS: 1000,
  default: {
    connect: vi.fn(),
    updateSubscription: vi.fn(),
    disconnectIfIdle: vi.fn(),
    onOverviewUpdate: vi.fn(),
    onMetricsUpdate: vi.fn(),
    onHistoryUpdate: vi.fn(),
  },
}));

import { useMetricsStore } from "./useMetricsStore";

const trafficType = {
  requests: 0,
  bytes_sent: 0,
  bytes_received: 0,
  active_connections: 0,
};

function snapshot(overrides: Partial<MetricsSnapshot> = {}): MetricsSnapshot {
  return {
    timestamp: 1,
    memory_used: 25,
    memory_total: 100,
    memory_usage_percent: 25,
    cpu_usage: 1,
    total_requests: 10,
    active_connections: 2,
    bytes_sent: 30,
    bytes_received: 40,
    total_traffic_bytes: 70,
    bytes_sent_rate: 3,
    bytes_received_rate: 4,
    qps: 1,
    max_qps: 2,
    max_bytes_sent_rate: 5,
    max_bytes_received_rate: 6,
    http: trafficType,
    https: trafficType,
    tunnel: trafficType,
    ws: trafficType,
    wss: trafficType,
    h3: trafficType,
    h3s: trafficType,
    socks5: trafficType,
    ...overrides,
  };
}

function overview(metrics: MetricsSnapshot): SystemOverview {
  return {
    system: {
      version: "test",
      device_name: "test",
      os: "test",
      arch: "test",
      cpu_logical_cores: 1,
      memory_total_bytes: 100,
      memory_available_bytes: 75,
      uptime_secs: 1,
      pid: 1,
    },
    metrics,
    rules: { total: 1, enabled: 1 },
    traffic: { recorded: 2000 },
    server: { port: 8800, admin_url: "http://127.0.0.1:8800/_bifrost/" },
    pending_authorizations: 0,
  };
}

describe("useMetricsStore server snapshots", () => {
  beforeEach(() => {
    useMetricsStore.setState({
      current: null,
      history: [],
      overview: null,
      loading: false,
      error: null,
    });
  });

  it("merges every metrics push into current and overview without using the traffic window", () => {
    const initial = snapshot();
    const pushed = snapshot({
      timestamp: 2,
      total_requests: 1777,
      total_traffic_bytes: 999,
      memory_usage_percent: 42,
    });
    useMetricsStore.setState({ overview: overview(initial), current: initial });

    useMetricsStore.getState().handleMetricsPush({
      metrics: pushed,
      recorded_traffic: 5028,
    });

    const state = useMetricsStore.getState();
    expect(state.current).toBe(pushed);
    expect(state.overview?.metrics).toBe(pushed);
    expect(state.overview?.traffic.recorded).toBe(5028);
    expect(state.overview?.metrics.total_requests).toBe(1777);
  });

  it("updates the latest history sample while preserving the server-derived fields", () => {
    const initial = snapshot();
    const pushed = snapshot({ timestamp: 2, total_traffic_bytes: 321 });
    useMetricsStore.setState({
      overview: overview(initial),
      current: initial,
      history: [initial, snapshot({ timestamp: 2 })],
    });

    useMetricsStore.getState().handleMetricsPush({
      metrics: pushed,
      recorded_traffic: 7,
    });

    expect(useMetricsStore.getState().history.at(-1)).toBe(pushed);
    expect(useMetricsStore.getState().history.at(-1)?.total_traffic_bytes).toBe(321);
  });
});
