/**
 * Unified Sessions Section - Single table for all sessions (active + history)
 * Data comes from the unified backend API /agent/sessions/all
 * Sorted by most recently updated first.
 */
import { useCallback, useEffect, useState } from "react";
import {
  Button,
  Col,
  Empty,
  Popconfirm,
  Row,
  Space,
  Table,
  Tag,
  Tooltip,
  Typography,
  message,
} from "antd";
import {
  ClearOutlined,
  DeleteOutlined,
  EyeOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import { get, del } from "../../../../api/client";
import { BASE } from "./types";

const { Text } = Typography;

/** Row shape returned by backend /agent/sessions/all */
interface UnifiedSession {
  session_key: string;
  status: "active" | "ended";
  source?: string;
  work_dir?: string;
  turns?: number;
  tokens?: number;
  start_time?: number;
  last_active_time?: number;
  duration_secs?: number;
  compaction_count?: number;
  estimated_tokens?: number;
  /** Session title (intent/topic) set by the agent */
  title?: string;
  /** Only for ended sessions — file path for loading detail */
  history_path?: string;
}

interface UnifiedSessionsSectionProps {
  onOpenSession?: (
    sessionKey: string,
    view: "active" | "history",
    filePath?: string,
  ) => void;
}

export default function UnifiedSessionsSection({
  onOpenSession,
}: UnifiedSessionsSectionProps) {
  const [sessions, setSessions] = useState<UnifiedSession[]>([]);
  const [activeCount, setActiveCount] = useState(0);
  const [historyCount, setHistoryCount] = useState(0);
  const [loading, setLoading] = useState(false);

  const fetchAll = useCallback(async () => {
    setLoading(true);
    try {
      const data = await get<{
        sessions: UnifiedSession[];
        total: number;
        active_count: number;
        history_count: number;
      }>(`${BASE}/agent/sessions/all`);
      setSessions(data.sessions || []);
      setActiveCount(data.active_count ?? 0);
      setHistoryCount(data.history_count ?? 0);
    } catch {
      // silent
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchAll();
  }, [fetchAll]);

  const handleDeleteActive = async (key: string) => {
    try {
      await del(`${BASE}/agent/sessions/${encodeURIComponent(key)}`);
      message.success("Session deleted");
      fetchAll();
    } catch {
      message.error("Failed to delete session");
    }
  };

  const handleDeleteHistory = async (path: string) => {
    try {
      await del(
        `${BASE}/agent/sessions/history/${encodeURIComponent(path)}`,
      );
      message.success("History file deleted");
      fetchAll();
    } catch {
      message.error("Failed to delete history file");
    }
  };

  const handleClearAllActive = async () => {
    try {
      await del(`${BASE}/agent/sessions`);
      message.success("All active sessions cleared");
      fetchAll();
    } catch {
      message.error("Failed to clear sessions");
    }
  };

  const formatTs = (ts?: number) => {
    if (!ts) return "-";
    const d = new Date(ts * 1000);
    const MM = String(d.getMonth() + 1).padStart(2, "0");
    const dd = String(d.getDate()).padStart(2, "0");
    const HH = String(d.getHours()).padStart(2, "0");
    const mm = String(d.getMinutes()).padStart(2, "0");
    return `${MM}-${dd} ${HH}:${mm}`;
  };

  /** Shorten session key for display: first 6 + last 6 chars */
  const shortenKey = (key: string) => {
    if (key.length <= 16) return key;
    return `${key.slice(0, 6)}…${key.slice(-6)}`;
  };

  const formatDuration = (secs?: number) => {
    if (!secs) return "-";
    if (secs < 60) return `${secs}s`;
    if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
    return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
  };

  const columns = [
    {
      title: "Status",
      dataIndex: "status",
      key: "status",
      width: 80,
      filters: [
        { text: "Active", value: "active" },
        { text: "Ended", value: "ended" },
      ],
      onFilter: (value: unknown, record: UnifiedSession) =>
        record.status === value,
      render: (val: string) =>
        val === "active" ? (
          <Tag color="green" style={{ fontSize: 10, margin: 0 }}>
            Active
          </Tag>
        ) : (
          <Tag color="default" style={{ fontSize: 10, margin: 0 }}>
            Ended
          </Tag>
        ),
    },
    {
      title: "Session Key",
      dataIndex: "session_key",
      key: "session_key",
      width: 140,
      ellipsis: true,
      render: (val: string) => (
        <Tooltip title={val}>
          <Text code style={{ fontSize: 11 }}>
            {shortenKey(val)}
          </Text>
        </Tooltip>
      ),
    },
    {
      title: "Source",
      dataIndex: "source",
      key: "source",
      width: 84,
      filters: [
        { text: "Feishu", value: "feishu" },
        { text: "API", value: "api" },
      ],
      onFilter: (value: unknown, record: UnifiedSession) =>
        record.source === value,
      render: (val?: string) => {
        if (val === "feishu")
          return (
            <Tag color="blue" style={{ fontSize: 10, margin: 0 }}>
              Feishu
            </Tag>
          );
        if (val === "api")
          return (
            <Tag color="green" style={{ fontSize: 10, margin: 0 }}>
              API
            </Tag>
          );
        return (
          <Tag style={{ fontSize: 10, margin: 0 }}>{val || "—"}</Tag>
        );
      },
    },
    {
      title: "Title",
      dataIndex: "title",
      key: "title",
      width: 180,
      ellipsis: true,
      render: (val?: string) =>
        val ? (
          <Tooltip title={val}>
            <Text style={{ fontSize: 11 }}>{val}</Text>
          </Tooltip>
        ) : (
          <Text type="secondary" style={{ fontSize: 10 }}>
            —
          </Text>
        ),
    },
    {
      title: "Work Dir",
      dataIndex: "work_dir",
      key: "work_dir",
      ellipsis: true,
      render: (val?: string) =>
        val ? (
          <Tooltip title={val}>
            <Text code style={{ fontSize: 10 }}>
              {val}
            </Text>
          </Tooltip>
        ) : (
          <Text type="secondary" style={{ fontSize: 10 }}>
            default
          </Text>
        ),
    },
    {
      title: "Turns",
      dataIndex: "turns",
      key: "turns",
      width: 72,
      align: "right" as const,
      sorter: (a: UnifiedSession, b: UnifiedSession) =>
        (a.turns ?? 0) - (b.turns ?? 0),
      render: (val?: number) => val || "-",
    },
    {
      title: "Tokens",
      dataIndex: "tokens",
      key: "tokens",
      width: 86,
      align: "right" as const,
      sorter: (a: UnifiedSession, b: UnifiedSession) =>
        (a.tokens ?? 0) - (b.tokens ?? 0),
      render: (val?: number) => (val ? val.toLocaleString() : "-"),
    },
    {
      title: "Started",
      dataIndex: "start_time",
      key: "start_time",
      width: 110,
      sorter: (a: UnifiedSession, b: UnifiedSession) =>
        (a.start_time ?? 0) - (b.start_time ?? 0),
      render: (val?: number) => (
        <Text style={{ fontSize: 11, whiteSpace: "nowrap" }}>
          {formatTs(val)}
        </Text>
      ),
    },
    {
      title: "Last Active",
      dataIndex: "last_active_time",
      key: "last_active_time",
      width: 110,
      defaultSortOrder: "descend" as const,
      sorter: (a: UnifiedSession, b: UnifiedSession) =>
        (a.last_active_time ?? 0) - (b.last_active_time ?? 0),
      render: (val?: number) => (
        <Text style={{ fontSize: 11, whiteSpace: "nowrap" }}>
          {formatTs(val)}
        </Text>
      ),
    },
    {
      title: "Duration",
      dataIndex: "duration_secs",
      key: "duration_secs",
      width: 100,
      align: "right" as const,
      sorter: (a: UnifiedSession, b: UnifiedSession) =>
        (a.duration_secs ?? 0) - (b.duration_secs ?? 0),
      render: (val?: number) => (
        <Text style={{ whiteSpace: "nowrap" }}>{formatDuration(val)}</Text>
      ),
    },
    {
      title: "Actions",
      key: "actions",
      width: 80,
      render: (_: unknown, record: UnifiedSession) => (
        <Space size={4}>
          <Tooltip title="View details">
            <Button
              size="small"
              icon={<EyeOutlined />}
              onClick={() => {
                if (record.status === "active") {
                  onOpenSession?.(record.session_key, "active");
                } else {
                  onOpenSession?.(
                    record.session_key,
                    "history",
                    record.history_path,
                  );
                }
              }}
            />
          </Tooltip>
          {record.status === "active" ? (
            <Popconfirm
              title="Delete this active session?"
              onConfirm={() => handleDeleteActive(record.session_key)}
            >
              <Button size="small" danger icon={<DeleteOutlined />} />
            </Popconfirm>
          ) : record.history_path ? (
            <Popconfirm
              title="Delete this history file?"
              onConfirm={() => handleDeleteHistory(record.history_path!)}
            >
              <Button size="small" danger icon={<DeleteOutlined />} />
            </Popconfirm>
          ) : null}
        </Space>
      ),
    },
  ];

  return (
    <Space direction="vertical" style={{ width: "100%" }}>
      <Row justify="space-between" align="middle">
        <Col>
          <Space size="small">
            <Text type="secondary" style={{ fontSize: 12 }}>
              {sessions.length} session{sessions.length !== 1 ? "s" : ""}
            </Text>
            <Tag color="green" style={{ fontSize: 10 }}>
              {activeCount} active
            </Tag>
            <Tag style={{ fontSize: 10 }}>{historyCount} history</Tag>
          </Space>
        </Col>
        <Col>
          <Space size="small">
            {activeCount > 0 && (
              <Popconfirm
                title="Clear ALL active sessions?"
                description="This will remove all active sessions from memory."
                onConfirm={handleClearAllActive}
              >
                <Button icon={<ClearOutlined />} size="small" danger>
                  Clear Active
                </Button>
              </Popconfirm>
            )}
            <Button
              icon={<ReloadOutlined />}
              size="small"
              onClick={fetchAll}
              loading={loading}
            >
              Refresh
            </Button>
          </Space>
        </Col>
      </Row>
      <Table
        dataSource={sessions}
        columns={columns}
        rowKey={(r) =>
          r.status === "active"
            ? `active-${r.session_key}`
            : `history-${r.history_path ?? r.session_key}`
        }
        size="small"
        loading={loading}
        pagination={{ pageSize: 10, size: "small", showSizeChanger: false }}
        locale={{ emptyText: <Empty description="No sessions" /> }}
        scroll={{ x: 1250 }}
      />
    </Space>
  );
}
