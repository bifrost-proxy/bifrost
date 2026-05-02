/**
 * Active Sessions Section - Manage active in-memory sessions
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
import { BASE, type SessionInfo } from "./types";

const { Text } = Typography;

interface SessionsSectionProps {
  onOpenSession?: (sessionKey: string, view: "active" | "history") => void;
}

export default function SessionsSection({ onOpenSession }: SessionsSectionProps) {
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [loading, setLoading] = useState(false);

  const fetchSessions = useCallback(async () => {
    setLoading(true);
    try {
      const data = await get<{ sessions: SessionInfo[] }>(
        `${BASE}/agent/sessions`,
      );
      setSessions(data.sessions || []);
    } catch {
      // silent
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchSessions();
  }, [fetchSessions]);

  const handleDeleteSession = async (key: string) => {
    try {
      await del(`${BASE}/agent/sessions/${encodeURIComponent(key)}`);
      message.success("Session deleted");
      fetchSessions();
    } catch {
      message.error("Failed to delete session");
    }
  };

  const handleClearAll = async () => {
    try {
      await del(`${BASE}/agent/sessions`);
      message.success("All sessions cleared");
      setSessions([]);
    } catch {
      message.error("Failed to clear sessions");
    }
  };

  const formatTs = (ts?: number) => {
    if (!ts) return "-";
    return new Date(ts * 1000).toLocaleString();
  };

  const columns = [
    {
      title: "Session Key",
      dataIndex: "session_key",
      key: "session_key",
      ellipsis: true,
      render: (val: string) => (
        <Text code style={{ fontSize: 11 }}>
          {val}
        </Text>
      ),
    },
    {
      title: "Source",
      dataIndex: "source",
      key: "source",
      width: 80,
      render: (val?: string) => {
        if (val === "feishu") return <Tag color="blue" style={{ fontSize: 10 }}>Feishu</Tag>;
        if (val === "api") return <Tag color="green" style={{ fontSize: 10 }}>API</Tag>;
        return <Tag style={{ fontSize: 10 }}>{val || "—"}</Tag>;
      },
    },
    {
      title: "Work Dir",
      dataIndex: "work_dir",
      key: "work_dir",
      ellipsis: true,
      width: 180,
      render: (val?: string) =>
        val ? (
          <Tooltip title={val}>
            <Text code style={{ fontSize: 10 }}>
              {val}
            </Text>
          </Tooltip>
        ) : (
          <Text type="secondary" style={{ fontSize: 11 }}>default</Text>
        ),
    },
    {
      title: "Messages",
      dataIndex: "message_count",
      key: "message_count",
      width: 80,
      render: (val?: number) => val ?? "-",
    },
    {
      title: "Tokens",
      dataIndex: "total_tokens_used",
      key: "total_tokens_used",
      width: 100,
      render: (val?: number) => (val != null ? val.toLocaleString() : "-"),
    },
    {
      title: "Est. Tokens",
      dataIndex: "estimated_tokens",
      key: "estimated_tokens",
      width: 100,
      render: (val?: number) => (val != null ? val.toLocaleString() : "-"),
    },
    {
      title: "Created",
      dataIndex: "created_at",
      key: "created_at",
      width: 170,
      render: (val?: number) => formatTs(val),
    },
    {
      title: "Last Active",
      dataIndex: "last_active_at",
      key: "last_active_at",
      width: 170,
      render: (val?: number) => formatTs(val),
    },
    {
      title: "Actions",
      key: "actions",
      width: 100,
      render: (_: unknown, record: SessionInfo) => (
        <Space size="small">
          <Tooltip title="View details">
            <Button
              size="small"
              icon={<EyeOutlined />}
              onClick={() => onOpenSession?.(record.session_key, "active")}
            />
          </Tooltip>
          <Popconfirm
            title="Delete this session?"
            onConfirm={() => handleDeleteSession(record.session_key)}
          >
            <Button size="small" danger icon={<DeleteOutlined />} />
          </Popconfirm>
        </Space>
      ),
    },
  ];

  return (
    <Space direction="vertical" style={{ width: "100%" }}>
      <Row justify="space-between" align="middle">
        <Col>
          <Text type="secondary" style={{ fontSize: 12 }}>
            {sessions.length} active session{sessions.length !== 1 ? "s" : ""}
          </Text>
        </Col>
        <Col>
          <Space size="small">
            {sessions.length > 0 && (
              <Popconfirm
                title="Clear ALL sessions?"
                description="This will remove all active sessions and their history."
                onConfirm={handleClearAll}
              >
                <Button icon={<ClearOutlined />} size="small" danger>
                  Clear All
                </Button>
              </Popconfirm>
            )}
            <Button
              icon={<ReloadOutlined />}
              size="small"
              onClick={fetchSessions}
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
        rowKey="session_key"
        size="small"
        loading={loading}
        pagination={{ pageSize: 10, size: "small" }}
        locale={{ emptyText: <Empty description="No active sessions" /> }}
        scroll={{ x: 1100 }}
      />
    </Space>
  );
}
