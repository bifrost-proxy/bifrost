import { vi } from "vitest";
import type {
  ToolbarFilters,
  TrafficQueryResponse,
  TrafficSummary,
  TrafficDeltaData,
} from "../types";

const apiMocks = vi.hoisted(() => ({
  getTrafficDetail: vi.fn(),
  getRequestBody: vi.fn(),
  getResponseBody: vi.fn(),
  getTrafficPage: vi.fn(),
  getTrafficUpdates: vi.fn(),
  queryTraffic: vi.fn(),
  clearTraffic: vi.fn().mockResolvedValue(undefined),
  getTrafficStatistics: vi.fn().mockResolvedValue({
    total_requests: 0,
    server_sequence: 0,
    client_ips: {},
    proxy_ports: {},
    applications: {},
    account_names: {},
    domains: {},
  }),
}));

vi.mock("../api", () => apiMocks);

const pushMocks = vi.hoisted(() => ({
  onTrafficUpdates: vi.fn().mockReturnValue(() => {}),
  onTrafficDelta: vi.fn().mockReturnValue(() => {}),
  onTrafficDeleted: vi.fn().mockReturnValue(() => {}),
  onTrafficStatistics: vi.fn().mockReturnValue(() => {}),
  onConnectionChange: vi.fn().mockReturnValue(() => {}),
  updateSubscription: vi.fn(),
  connect: vi.fn(),
  disconnectIfIdle: vi.fn(),
  resetTrafficCursor: vi.fn(),
}));
vi.mock("../services/pushService", () => ({ default: pushMocks }));

import { useTrafficStore } from "./useTrafficStore";

export const toolbar: ToolbarFilters = {
  rule: [],
  protocol: [],
  type: [],
  status: [],
  imported: [],
};

export const makeRecord = (
  id: string,
  path: string,
  overrides: Partial<TrafficSummary> = {},
): TrafficSummary => ({
  id,
  sequence: Number(id),
  timestamp: 1,
  method: "GET",
  url: `http://example.test${path}`,
  status: 200,
  content_type: "text/plain",
  request_size: 0,
  response_size: 2,
  duration_ms: 1,
  host: "example.test",
  path,
  protocol: "HTTP/1.1",
  client_ip: "127.0.0.1",
  has_rule_hit: false,
  matched_rule_count: 0,
  matched_protocols: [],
  start_time: "2026-05-09T00:00:00Z",
  end_time: "2026-05-09T00:00:00Z",
  ...overrides,
});

export const makePage = (
  records: TrafficSummary[],
  hasMore: boolean,
  direction: "backward" | "forward",
): TrafficQueryResponse => ({
  records: (direction === "backward" ? records.slice().reverse() : records).map(
    (record) => ({
      id: record.id,
      seq: record.sequence,
      ts: record.timestamp,
      m: record.method,
      h: record.host,
      p: record.path,
      s: record.status,
      ct: record.content_type,
      req_ct: record.request_content_type,
      req_sz: record.request_size,
      res_sz: record.response_size,
      up: record.upload_bytes ?? record.request_size,
      down: record.download_bytes ?? record.response_size,
      dur: record.duration_ms,
      lp: record.listener_port ?? 0,
      proto: record.protocol,
      cip: record.client_ip,
      capp: record.client_app,
      cpid: record.client_pid,
      acct: record.account_name,
      flags: 0,
      fc: record.frame_count ?? 0,
      st: record.start_time,
      et: record.end_time,
      rc: record.matched_rule_count,
      rp: record.matched_protocols,
    }),
  ),
  next_cursor: records.at(-1)?.sequence ?? null,
  prev_cursor: records[0]?.sequence ?? null,
  has_more: hasMore,
  total: 3_000,
  server_sequence: 3_000,
});

export const makeRecordRange = (start: number, end: number): TrafficSummary[] =>
  Array.from({ length: end - start + 1 }, (_, index) =>
    makeRecord(String(start + index), `/${start + index}`),
  );

export const makeDelta = (
  records: TrafficSummary[],
  serverTotal: number,
  serverSequence: number,
  oldestSequence: number,
): TrafficDeltaData => ({
  inserts: makePage(records, false, "forward").records,
  updates: [],
  has_more: false,
  server_total: serverTotal,
  server_sequence: serverSequence,
  oldest_sequence: oldestSequence,
});

export const flushTrafficBatch = () =>
  new Promise<void>((resolve) => window.setTimeout(resolve, 180));

// Clear reports the DELETE result independently of background reconciliation.
// Existing membership assertions explicitly wait for that separate read.
export const clearAndSynchronizeForTest = async (ids?: string[]) => {
  const reload = useTrafficStore.getState().reloadRecords;
  let synchronization = Promise.resolve();
  useTrafficStore.setState({
    reloadRecords: (...args) => {
      synchronization = reload(...args);
      return synchronization;
    },
  });
  try {
    const result = await useTrafficStore.getState().clearTraffic(ids);
    await synchronization;
    return result;
  } finally {
    useTrafficStore.setState({ reloadRecords: reload });
  }
};

export { apiMocks, pushMocks };
