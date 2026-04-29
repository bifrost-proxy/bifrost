import type { CSSProperties } from "react";
import { Empty, Typography } from "antd";
import type { DebugNetworkEvent } from "../../../api/devtools";
import { HighlightedText, filterBySearch } from "./shared";

const { Text } = Typography;

export function NetworkList({
  events,
  searchQuery,
  onOpenTraffic,
}: {
  events: DebugNetworkEvent[];
  searchQuery: string;
  onOpenTraffic?: (event: DebugNetworkEvent) => void;
}) {
  if (!events.length) return <Empty description="No network events yet" />;
  const filtered = filterBySearch(events, searchQuery, (event) =>
    [event.method, event.status, event.resource_type, event.url].join(" "),
  );
  return (
    <div style={tableStyle}>
      <div style={tableHeaderStyle}>
        <Text strong>Method</Text>
        <Text strong>Status</Text>
        <Text strong>Type</Text>
        <Text strong>URL</Text>
      </div>
      {filtered.slice().reverse().map((event, index) => (
        <button
          key={`${event.url}-${event.at_ms}-${index}`}
          type="button"
          style={tableRowStyle}
          onClick={() => onOpenTraffic?.(event)}
          title={
            event.traffic_id || event.client_req_id
              ? "Open matching Traffic record"
              : "This browser resource has no DevTools request id"
          }
        >
          <Text code><HighlightedText text={event.method || "GET"} query={searchQuery} /></Text>
          <Text><HighlightedText text={String(event.status ?? "-")} query={searchQuery} /></Text>
          <Text><HighlightedText text={event.resource_type || "resource"} query={searchQuery} /></Text>
          <Text ellipsis title={event.url}><HighlightedText text={event.url} query={searchQuery} /></Text>
        </button>
      ))}
    </div>
  );
}



const tableStyle: CSSProperties = {
  display: "grid",
  alignContent: "start",
  gap: 0,
  minHeight: "100%",
  border: "1px solid #d9e2ef",
  borderRadius: 6,
  overflow: "hidden",
};

const tableHeaderStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "96px 80px 130px minmax(260px, 1fr)",
  gap: 8,
  padding: "8px 10px",
  background: "#eef4fb",
};

const tableRowStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "96px 80px 130px minmax(260px, 1fr)",
  gap: 8,
  padding: "8px 10px",
  borderTop: "1px solid #e7edf5",
  minWidth: 560,
  width: "100%",
  borderRight: 0,
  borderBottom: 0,
  borderLeft: 0,
  background: "transparent",
  textAlign: "left",
  cursor: "pointer",
};
