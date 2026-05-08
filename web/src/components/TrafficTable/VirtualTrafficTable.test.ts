import { describe, expect, it } from "vitest";

import type { TrafficSummary } from "../../types";
import { areTrafficRowPropsEqual } from "./VirtualTrafficTable";

const createRecord = (overrides: Partial<TrafficSummary> = {}): TrafficSummary => ({
  id: "REQ-1",
  sequence: 1,
  timestamp: 1710000000,
  method: "GET",
  url: "http://example.com/hello",
  status: 200,
  content_type: "application/json",
  request_size: 12,
  response_size: 34,
  duration_ms: 56,
  listener_port: 8801,
  host: "example.com",
  path: "/hello",
  protocol: "HTTP",
  client_ip: "127.0.0.1",
  has_rule_hit: false,
  matched_rule_count: 0,
  matched_protocols: [],
  start_time: "2026-05-07T00:00:00Z",
  ...overrides,
});

const createRowProps = (record: TrafficSummary) => ({
  record,
  liveNow: 1710000001,
  isSelected: false,
  isMultiSelected: false,
  isImported: false,
  translateY: 0,
  rowIndex: 0,
  borderColor: "#ddd",
  selectedBg: "#fff",
  multiSelectedBg: "#fff",
  importedBg: "#fff",
  evenBg: "#fff",
  oddBg: "#fafafa",
  textSecondary: "#999",
  onRowClick: () => {},
  onRowDoubleClick: () => {},
  onRowContextMenu: () => {},
});

describe("areTrafficRowPropsEqual", () => {
  it("returns false when listener_port changes", () => {
    const prev = createRowProps(createRecord({ listener_port: 8801 }));
    const next = createRowProps(createRecord({ listener_port: 8802 }));

    expect(areTrafficRowPropsEqual(prev, next)).toBe(false);
  });

  it("returns true when row props are unchanged", () => {
    const prev = createRowProps(createRecord());
    const next = createRowProps(createRecord());

    expect(areTrafficRowPropsEqual(prev, next)).toBe(true);
  });
});
