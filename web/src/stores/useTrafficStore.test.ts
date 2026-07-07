import { describe, expect, it, vi } from "vitest";
import type { FilterCondition, ToolbarFilters, TrafficSummary } from "../types";

vi.mock("../api", () => ({}));

vi.mock("../services/pushService", () => ({
  default: {
    onTrafficUpdates: vi.fn(),
    onTrafficDelta: vi.fn(),
    onTrafficDeleted: vi.fn(),
  },
}));

import { filterRecords, isFilterConditionApplicable } from "./useTrafficStore";

const toolbar: ToolbarFilters = {
  rule: [],
  protocol: [],
  type: [],
  status: [],
  imported: [],
};

const makeRecord = (
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

describe("Traffic filter condition enabled state", () => {
  it("ignores disabled filter conditions", () => {
    const records = [makeRecord("1", "/keep"), makeRecord("2", "/other")];
    const conditions: FilterCondition[] = [
      {
        id: "disabled-path",
        field: "path",
        operator: "contains",
        value: "/keep",
        enabled: false,
      },
    ];

    expect(filterRecords(records, toolbar, conditions)).toEqual(records);
    expect(isFilterConditionApplicable(conditions[0])).toBe(false);
  });

  it("treats legacy conditions without enabled as active", () => {
    const records = [makeRecord("1", "/keep"), makeRecord("2", "/other")];
    const conditions: FilterCondition[] = [
      {
        id: "legacy-path",
        field: "path",
        operator: "contains",
        value: "/keep",
      },
    ];

    expect(filterRecords(records, toolbar, conditions)).toEqual([records[0]]);
    expect(isFilterConditionApplicable(conditions[0])).toBe(true);
  });

  it("filters records by selected proxy port panel filters", () => {
    const records = [
      makeRecord("1", "/main", { listener_port: 9900 }),
      makeRecord("2", "/temp", { listener_port: 58344 }),
      makeRecord("3", "/other-temp", { listener_port: 58345 }),
    ];

    expect(
      filterRecords(records, toolbar, [], {
        clientIps: [],
        proxyPorts: ["58344"],
        clientApps: [],
        domains: [],
      }),
    ).toEqual([records[1]]);
  });
});
