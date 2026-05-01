/**
 * Active Sessions Section - Manage active in-memory sessions
 */
import { useCallback, useEffect, useState } from "react";
import {
  Button,
  Card,
  Col,
  Divider,
  Empty,
  Modal,
  Popconfirm,
  Row,
  Space,
  Spin,
  Table,
  Tag,
  Tooltip,
  Typography,
  message,
  theme,
} from "antd";
import {
  ClearOutlined,
  DeleteOutlined,
  EyeOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import { get, del } from "../../../../api/client";
import { BASE, type SessionInfo, type SessionDetail } from "./types";

const { Text } = Typography;

export default function SessionsSection() {
  const { token } = theme.useToken();
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailOpen, setDetailOpen] = useState(false);
  const [detail, setDetail] = useState<SessionDetail | null>(null);

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

  const handleViewDetail = async (key: string) => {
    setDetailLoading(true);
    setDetailOpen(true);
    setDetail(null);
    try {
      const data = await get<SessionDetail>(
        `${BASE}/agent/sessions/${encodeURIComponent(key)}`,
      );
      setDetail(data);
    } catch {
      message.error("Failed to load session detail");
      setDetailOpen(false);
    } finally {
      setDetailLoading(false);
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
          <Tooltip title="View messages">
            <Button
              size="small"
              icon={<EyeOutlined />}
              onClick={() => handleViewDetail(record.session_key)}
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

      {/* Session Detail Modal */}
      <Modal
        title={
          detail ? (
            <Space>
              <EyeOutlined />
              <span>Session Detail</span>
              <Tag>{detail.message_count} messages</Tag>
            </Space>
          ) : (
            "Session Detail"
          )
        }
        open={detailOpen}
        onCancel={() => setDetailOpen(false)}
        footer={null}
        width={720}
      >
        {detailLoading ? (
          <Spin style={{ display: "block", margin: "40px auto" }} />
        ) : detail ? (
          <Space direction="vertical" style={{ width: "100%" }} size="middle">
            {/* Meta info */}
            <Row gutter={[16, 8]}>
              <Col span={24}>
                <Text type="secondary" style={{ fontSize: 12 }}>Session Key</Text>
                <br />
                <Text code style={{ fontSize: 11 }}>{detail.session_key}</Text>
              </Col>
              <Col span={24}>
                <Text type="secondary" style={{ fontSize: 12 }}>Working Directory</Text>
                <br />
                {detail.work_dir ? (
                  <Text code style={{ fontSize: 11 }}>{detail.work_dir}</Text>
                ) : (
                  <Text type="secondary" style={{ fontSize: 11 }}>Using default from config</Text>
                )}
              </Col>
              <Col span={8}>
                <Text type="secondary" style={{ fontSize: 12 }}>Created</Text>
                <br />
                <Text>{formatTs(detail.created_at)}</Text>
              </Col>
              <Col span={8}>
                <Text type="secondary" style={{ fontSize: 12 }}>Last Active</Text>
                <br />
                <Text>{formatTs(detail.last_active_at)}</Text>
              </Col>
              <Col span={8}>
                <Text type="secondary" style={{ fontSize: 12 }}>Compactions</Text>
                <br />
                <Text>{detail.compaction_count}</Text>
              </Col>
            </Row>

            <Divider style={{ margin: "8px 0" }} />

            {/* Messages */}
            <Text strong>Messages ({detail.messages.length})</Text>
            <div
              style={{
                maxHeight: 400,
                overflowY: "auto",
                display: "flex",
                flexDirection: "column",
                gap: 8,
              }}
            >
              {detail.messages.length === 0 ? (
                <Empty description="No messages in this session" />
              ) : (
                detail.messages.map((msg, idx) => (
                  <Card
                    key={idx}
                    size="small"
                    style={{
                      borderColor:
                        msg.role === "assistant"
                          ? token.colorPrimaryBorder
                          : msg.role === "user"
                            ? token.colorSuccessBorder
                            : token.colorBorderSecondary,
                      background:
                        msg.role === "assistant"
                          ? token.colorPrimaryBg
                          : msg.role === "user"
                            ? token.colorSuccessBg
                            : undefined,
                    }}
                  >
                    <Space
                      direction="vertical"
                      size={2}
                      style={{ width: "100%" }}
                    >
                      <Tag
                        color={
                          msg.role === "assistant"
                            ? "blue"
                            : msg.role === "user"
                              ? "green"
                              : msg.role === "system"
                                ? "purple"
                                : "default"
                        }
                        style={{ fontSize: 11 }}
                      >
                        {msg.role}
                      </Tag>
                      <Text
                        style={{
                          fontSize: 12,
                          whiteSpace: "pre-wrap",
                          wordBreak: "break-word",
                        }}
                      >
                        {msg.content || "(empty)"}
                      </Text>
                      {msg.tool_calls && msg.tool_calls.length > 0 && (
                        <div>
                          <Text type="secondary" style={{ fontSize: 11 }}>
                            Tool calls:
                          </Text>
                          {msg.tool_calls.map((tc, i) => (
                            <Text
                              key={i}
                              code
                              style={{
                                fontSize: 10,
                                display: "block",
                                marginTop: 2,
                              }}
                            >
                              {tc}
                            </Text>
                          ))}
                        </div>
                      )}
                    </Space>
                  </Card>
                ))
              )}
            </div>
          </Space>
        ) : null}
      </Modal>
    </Space>
  );
}
