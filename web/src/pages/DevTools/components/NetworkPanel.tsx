import { useMemo, useState } from "react";
import { Empty } from "antd";
import VirtualTrafficTable from "../../../components/TrafficTable/VirtualTrafficTable";
import type { DebugNetworkEvent } from "../../../api/devtools";
import type { TrafficSummary } from "../../../types";
import { filterBySearch } from "./shared";

const METHOD_COLORS: Record<string, string> = {
  GET: "green",
  POST: "blue",
  PUT: "orange",
  DELETE: "red",
  PATCH: "purple",
  OPTIONS: "default",
  HEAD: "cyan",
  CONNECT: "magenta",
};

export function NetworkList({
  events,
  searchQuery,
  onOpenTraffic,
}: {
  events: DebugNetworkEvent[];
  searchQuery: string;
  onOpenTraffic?: (event: DebugNetworkEvent) => void;
}) {
  const [selectedId, setSelectedId] = useState<string | undefined>();
  const filteredEvents = useMemo(
    () =>
      filterBySearch(events, searchQuery, (event) =>
        [
          event.method,
          event.status,
          event.resource_type,
          event.url,
          event.traffic_id,
          event.client_req_id,
          event.from_cache ? "from cache" : "",
          ...(event.query_params ?? []).flat(),
          ...(event.request_headers ?? []).flat(),
          ...(event.response_headers ?? []).flat(),
        ].join(" "),
      ).slice().reverse(),
    [events, searchQuery],
  );
  const rows = useMemo(
    () => filteredEvents.map((event, index) => networkEventToTrafficSummary(event, index, filteredEvents.length)),
    [filteredEvents],
  );
  const eventByRowId = useMemo(() => {
    const map = new Map<string, DebugNetworkEvent>();
    rows.forEach((row, index) => {
      const event = filteredEvents[index];
      if (event) map.set(row.id, event);
    });
    return map;
  }, [filteredEvents, rows]);

  if (!events.length) return <Empty description="No network events yet" />;

  return (
    <div data-testid="devtools-network-traffic-table" style={{ height: "100%", minHeight: 0 }}>
      <VirtualTrafficTable
        data={rows}
        selectedId={selectedId}
        autoScroll={false}
        onSelect={(record) => {
          setSelectedId(record.id);
          const event = eventByRowId.get(record.id);
          if (event) onOpenTraffic?.(event);
        }}
      />
    </div>
  );
}

function networkEventToTrafficSummary(event: DebugNetworkEvent, index: number, total: number): TrafficSummary {
  const parsed = parseUrl(event.url);
  const status = event.status ?? 0;
  const method = (event.method || "GET").toUpperCase();
  const id = event.traffic_id || event.client_req_id || `devtools-network-${event.at_ms}-${index}`;
  const startDate = new Date(event.at_ms || Date.now());

  return {
    id,
    sequence: total - index,
    timestamp: event.at_ms || Date.now(),
    method,
    url: event.url,
    status,
    content_type: event.resource_type || null,
    request_size: 0,
    response_size: 0,
    duration_ms: 0,
    host: parsed.host,
    path: parsed.path,
    protocol: parsed.protocol,
    client_ip: "page",
    has_rule_hit: Boolean(event.traffic_id),
    matched_rule_count: event.traffic_id ? 1 : 0,
    matched_protocols: event.traffic_id ? ["traffic"] : [],
    start_time: formatNetworkTime(startDate),
    end_time: null,
    _displayProtocol: parsed.protocol.toUpperCase(),
    _methodColor: METHOD_COLORS[method] || "default",
    _statusColor: getStatusColor(status),
    _statusDotColor: getStatusDotColor(status),
    _displaySize: "-",
    _contentTypeShort: event.resource_type || "-",
    _clientDisplay: "page",
    _clientTooltip: event.client_req_id
      ? `DevTools client request id: ${event.client_req_id}`
      : "Captured from target page bridge",
  };
}

function parseUrl(url: string): { host: string; path: string; protocol: string } {
  try {
    const parsed = new URL(url);
    return {
      host: parsed.host || "-",
      path: `${parsed.pathname || "/"}${parsed.search || ""}`,
      protocol: parsed.protocol.replace(":", "") || "http",
    };
  } catch {
    return {
      host: "-",
      path: url || "-",
      protocol: "http",
    };
  }
}

function formatNetworkTime(date: Date): string {
  const pad = (value: number, size = 2) => String(value).padStart(size, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}.${pad(date.getMilliseconds(), 3)}`;
}

function getStatusColor(status: number): string {
  if (status >= 500) return "error";
  if (status >= 400) return "warning";
  if (status >= 300) return "processing";
  if (status >= 200) return "success";
  return "default";
}

function getStatusDotColor(status: number): string {
  if (status === 0) return "#d9d9d9";
  if (status >= 100 && status < 200) return "#73d13d";
  if (status >= 200 && status < 300) return "#52c41a";
  if (status >= 300 && status < 400) return "#faad14";
  if (status >= 400 && status < 500) return "#fa8c16";
  if (status >= 500) return "#f5222d";
  return "#d9d9d9";
}
