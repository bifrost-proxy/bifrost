import { Table, Tag, Typography, Tooltip, Badge } from "antd";
import { ThunderboltOutlined } from "@ant-design/icons";
import type { ColumnsType } from "antd/es/table";
import type { TrafficSummary } from "../../types";
import {
  formatDurationCompact,
  formatDurationDetailed,
  getEffectiveDurationMs,
  isLiveStreamingTraffic,
} from "../../utils/duration";
import { formatTimestamp } from "../../utils/datetime";
import { useLiveNow } from "../../hooks/useLiveNow";

const { Text } = Typography;

interface TrafficTableProps {
  data: TrafficSummary[];
  loading?: boolean;
  onSelect?: (record: TrafficSummary) => void;
  selectedId?: string;
}

export default function TrafficTable({
  data,
  loading,
  onSelect,
  selectedId,
}: TrafficTableProps) {
  const hasLiveDuration = data.some((record) => isLiveStreamingTraffic(record));
  const liveNow = useLiveNow(hasLiveDuration);

  const getStatusColor = (status: number) => {
    if (status >= 500) return "error";
    if (status >= 400) return "warning";
    if (status >= 300) return "processing";
    if (status >= 200) return "success";
    return "default";
  };

  const getMethodColor = (method: string) => {
    const colors: Record<string, string> = {
      GET: "green",
      POST: "blue",
      PUT: "orange",
      DELETE: "red",
      PATCH: "purple",
      OPTIONS: "default",
      HEAD: "cyan",
      CONNECT: "magenta",
    };
    return colors[method.toUpperCase()] || "default";
  };

  const formatSize = (bytes: number) => {
    if (bytes === 0) return "-";
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  };

  const columns: ColumnsType<TrafficSummary> = [
    {
      title: "",
      dataIndex: "has_rule_hit",
      key: "has_rule_hit",
      width: 28,
      align: "center",
      render: (hit: boolean, record) => (
        <Tooltip
          title={
            hit
              ? `${record.matched_rule_count} rule(s): ${record.matched_protocols?.join(", ") || ""}`
              : "No rules matched"
          }
        >
          <Badge
            status={hit ? "success" : "default"}
            style={{ cursor: "pointer" }}
          />
        </Tooltip>
      ),
    },
    {
      title: "Protocol",
      dataIndex: "protocol",
      key: "protocol",
      width: 60,
      render: (protocol: string) => (
        <Text type="secondary" style={{ fontSize: 11 }}>
          {protocol?.replace("HTTP/", "") || "-"}
        </Text>
      ),
    },
    {
      title: "Method",
      dataIndex: "method",
      key: "method",
      width: 70,
      render: (method: string) => (
        <Tag color={getMethodColor(method)} style={{ margin: 0, fontSize: 11 }}>
          {method}
        </Tag>
      ),
    },
    {
      title: "Status",
      dataIndex: "status",
      key: "status",
      width: 55,
      align: "center",
      render: (status: number) =>
        status > 0 ? (
          <Tag
            color={getStatusColor(status)}
            style={{ margin: 0, fontSize: 11 }}
          >
            {status}
          </Tag>
        ) : (
          <Text type="secondary">-</Text>
        ),
    },
    {
      title: "Client",
      dataIndex: "client_app",
      key: "client",
      width: 100,
      ellipsis: true,
      render: (_: string, record: TrafficSummary) => {
        const clientApp = record.client_app || "";
        const clientIp = record.client_ip || "";
        const display = clientApp || clientIp || "-";
        const tooltip = clientApp ? `${clientApp} (PID: ${record.client_pid})` : clientIp || "-";
        return (
          <Tooltip title={tooltip}>
            <Text type="secondary" style={{ fontSize: 11 }}>
              {display}
            </Text>
          </Tooltip>
        );
      },
    },
    {
      title: "Rules",
      dataIndex: "has_rule_hit",
      key: "rules",
      width: 60,
      align: "center",
      render: (_: boolean, record: TrafficSummary) =>
        record.has_rule_hit ? (
          <Tooltip
            title={
              <div>
                <div>{record.matched_rule_count} rule(s) matched</div>
                {record.matched_protocols.length > 0 && (
                  <div style={{ marginTop: 4 }}>
                    {record.matched_protocols.join(", ")}
                  </div>
                )}
              </div>
            }
          >
            <Badge count={record.matched_rule_count} size="small" color="blue">
              <ThunderboltOutlined style={{ fontSize: 14, color: "#1890ff" }} />
            </Badge>
          </Tooltip>
        ) : (
          <Text type="secondary">-</Text>
        ),
    },
    {
      title: "Host",
      dataIndex: "host",
      key: "host",
      width: 160,
      ellipsis: true,
      render: (host: string) => (
        <Tooltip title={host}>
          <Text style={{ fontSize: 12 }}>{host}</Text>
        </Tooltip>
      ),
    },
    {
      title: "Path",
      dataIndex: "path",
      key: "path",
      width: 250,
      ellipsis: true,
      render: (path: string) => (
        <Tooltip title={path}>
          <Text style={{ fontSize: 12 }}>{path}</Text>
        </Tooltip>
      ),
    },
    {
      title: "Type",
      dataIndex: "content_type",
      key: "content_type",
      width: 80,
      ellipsis: true,
      render: (ct: string | null) => {
        const short = ct?.split(";")[0]?.split("/").pop() || "-";
        return (
          <Text type="secondary" style={{ fontSize: 11 }}>
            {short}
          </Text>
        );
      },
    },
    {
      title: "Size",
      dataIndex: "response_size",
      key: "response_size",
      width: 65,
      align: "right",
      render: (_: number, record: TrafficSummary) => {
        const size = (record.is_websocket || record.is_sse || record.is_tunnel) && record.socket_status
          ? record.socket_status.send_bytes + record.socket_status.receive_bytes
          : record.response_size;
        return (
          <Text type="secondary" style={{ fontSize: 11 }}>
            {formatSize(size)}
          </Text>
        );
      },
    },
    {
      title: "Time",
      dataIndex: "duration_ms",
      key: "duration_ms",
      width: 55,
      align: "right",
      render: (_: number, record: TrafficSummary) => {
        const durationMs = getEffectiveDurationMs(record, liveNow);
        const compact = formatDurationCompact(durationMs);
        return (
          <Tooltip title={formatDurationDetailed(durationMs)}>
            <Text
              type={durationMs > 1000 ? "warning" : "secondary"}
              style={{ fontSize: 11 }}
            >
              {compact}
            </Text>
          </Tooltip>
        );
      },
    },
    {
      title: "Start Time",
      dataIndex: "timestamp",
      key: "start_time",
      width: 160,
      render: (_: number, record: TrafficSummary) => {
        const time = formatTimestamp(record.timestamp);
        return (
          <Tooltip title={time}>
            <Text type="secondary" style={{ fontSize: 11, fontFamily: "monospace" }}>
              {time}
            </Text>
          </Tooltip>
        );
      },
    },
    {
      title: "End Time",
      dataIndex: "timestamp",
      key: "end_time",
      width: 160,
      render: (_: number, record: TrafficSummary) => {
        const time =
          record.duration_ms > 0
            ? formatTimestamp(record.timestamp + record.duration_ms)
            : "-";
        return (
          <Tooltip title={time}>
            <Text type="secondary" style={{ fontSize: 11, fontFamily: "monospace" }}>
              {time}
            </Text>
          </Tooltip>
        );
      },
    },
  ];

  return (
    <Table
      columns={columns}
      dataSource={data}
      rowKey="id"
      loading={loading}
      pagination={false}
      size="small"
      scroll={{ x: "max-content", y: "calc(100vh - 150px)" }}
      onRow={(record) => {
        const isImported = record.id.startsWith("OUT-") || record.client_app === "Bifrost Import";
        return {
          onClick: () => onSelect?.(record),
          style: {
            cursor: "pointer",
            background: record.id === selectedId 
              ? "#e6f7ff" 
              : isImported 
                ? "#fff7e6" 
                : undefined,
          },
        };
      }}
      rowClassName={(record) => {
        const isImported = record.id.startsWith("OUT-") || record.client_app === "Bifrost Import";
        if (record.id === selectedId) return "selected-row";
        if (isImported) return "imported-row";
        return "";
      }}
    />
  );
}
