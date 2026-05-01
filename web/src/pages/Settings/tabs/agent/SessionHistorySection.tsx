/**
 * Session History Section - Manage persisted JSONL session files
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
  CheckCircleOutlined,
  ClockCircleOutlined,
  CloseCircleOutlined,
  CodeOutlined,
  CompressOutlined,
  DeleteOutlined,
  EyeOutlined,
  FileTextOutlined,
  PlayCircleOutlined,
  ReloadOutlined,
  RobotOutlined,
  StopOutlined,
  ToolOutlined,
  UserOutlined,
} from "@ant-design/icons";
import { get, del } from "../../../../api/client";
import { BASE, type ConversationEvent, type HistoryFileInfo } from "./types";

const { Text } = Typography;

function EventCard({
  event,
  token,
}: {
  event: ConversationEvent;
  token: ReturnType<typeof theme.useToken>["token"];
}) {
  const content = event.content as Record<string, unknown> | null;
  const ts = new Date(event.timestamp * 1000).toLocaleTimeString();

  switch (event.event_type) {
    case "session_start": {
      const model = content?.model as string | undefined;
      const provider = content?.provider as string | undefined;
      return (
        <Card
          size="small"
          style={{
            borderColor: token.colorBorderSecondary,
            background: token.colorFillQuaternary,
          }}
        >
          <Space direction="vertical" size={2} style={{ width: "100%" }}>
            <Space size="small">
              <PlayCircleOutlined style={{ color: token.colorTextSecondary }} />
              <Tag color="default" style={{ fontSize: 11 }}>session_start</Tag>
              <Text type="secondary" style={{ fontSize: 11 }}>{ts}</Text>
            </Space>
            {(model || provider) && (
              <Text style={{ fontSize: 12 }}>
                {model && <Tag>{model}</Tag>}
                {provider && <Tag color="cyan">{provider}</Tag>}
              </Text>
            )}
          </Space>
        </Card>
      );
    }

    case "user_message": {
      const msg = (content?.message as string) || "";
      return (
        <Card
          size="small"
          style={{
            borderColor: token.colorSuccessBorder,
            background: token.colorSuccessBg,
          }}
        >
          <Space direction="vertical" size={2} style={{ width: "100%" }}>
            <Space size="small">
              <UserOutlined style={{ color: token.colorSuccess }} />
              <Tag color="green" style={{ fontSize: 11 }}>user</Tag>
              <Text type="secondary" style={{ fontSize: 11 }}>{ts}</Text>
            </Space>
            <Text
              style={{
                fontSize: 12,
                whiteSpace: "pre-wrap",
                wordBreak: "break-word",
              }}
            >
              {msg || "(empty)"}
            </Text>
          </Space>
        </Card>
      );
    }

    case "assistant_message": {
      const msg = (content?.message as string) || "";
      return (
        <Card
          size="small"
          style={{
            borderColor: token.colorPrimaryBorder,
            background: token.colorPrimaryBg,
          }}
        >
          <Space direction="vertical" size={2} style={{ width: "100%" }}>
            <Space size="small">
              <RobotOutlined style={{ color: token.colorPrimary }} />
              <Tag color="blue" style={{ fontSize: 11 }}>assistant</Tag>
              <Text type="secondary" style={{ fontSize: 11 }}>{ts}</Text>
            </Space>
            <Text
              style={{
                fontSize: 12,
                whiteSpace: "pre-wrap",
                wordBreak: "break-word",
              }}
            >
              {msg || "(empty)"}
            </Text>
          </Space>
        </Card>
      );
    }

    case "tool_call": {
      const toolName = (content?.tool_name as string) || "unknown";
      const args = (content?.arguments as string) || "";
      return (
        <Card
          size="small"
          style={{
            borderColor: token.colorWarningBorder,
            background: token.colorWarningBg,
          }}
        >
          <Space direction="vertical" size={2} style={{ width: "100%" }}>
            <Space size="small">
              <ToolOutlined style={{ color: token.colorWarning }} />
              <Tag color="orange" style={{ fontSize: 11 }}>tool_call</Tag>
              <Tag style={{ fontSize: 11 }}>{toolName}</Tag>
              <Text type="secondary" style={{ fontSize: 11 }}>{ts}</Text>
            </Space>
            {args && (
              <pre
                style={{
                  fontSize: 11,
                  margin: 0,
                  padding: "4px 8px",
                  borderRadius: token.borderRadiusSM,
                  background: token.colorFillSecondary,
                  overflowX: "auto",
                  maxHeight: 120,
                  whiteSpace: "pre-wrap",
                  wordBreak: "break-word",
                }}
              >
                {args.length > 500 ? args.slice(0, 500) + "…" : args}
              </pre>
            )}
          </Space>
        </Card>
      );
    }

    case "tool_result": {
      const toolName = (content?.tool_name as string) || "unknown";
      const result = (content?.result as string) || "";
      const success = content?.success as boolean;
      return (
        <Card
          size="small"
          style={{
            borderColor: success
              ? token.colorSuccessBorder
              : token.colorErrorBorder,
            background: success
              ? token.colorSuccessBg
              : token.colorErrorBg,
          }}
        >
          <Space direction="vertical" size={2} style={{ width: "100%" }}>
            <Space size="small">
              {success ? (
                <CheckCircleOutlined style={{ color: token.colorSuccess }} />
              ) : (
                <CloseCircleOutlined style={{ color: token.colorError }} />
              )}
              <Tag color={success ? "green" : "red"} style={{ fontSize: 11 }}>
                tool_result
              </Tag>
              <Tag style={{ fontSize: 11 }}>{toolName}</Tag>
              <Text type="secondary" style={{ fontSize: 11 }}>{ts}</Text>
            </Space>
            {result && (
              <pre
                style={{
                  fontSize: 11,
                  margin: 0,
                  padding: "4px 8px",
                  borderRadius: token.borderRadiusSM,
                  background: token.colorFillSecondary,
                  overflowX: "auto",
                  maxHeight: 120,
                  whiteSpace: "pre-wrap",
                  wordBreak: "break-word",
                }}
              >
                {result.length > 500 ? result.slice(0, 500) + "…" : result}
              </pre>
            )}
          </Space>
        </Card>
      );
    }

    case "compaction": {
      const preTokens = content?.pre_tokens as number | undefined;
      const postTokens = content?.post_tokens as number | undefined;
      const removed = content?.messages_removed as number | undefined;
      return (
        <Card
          size="small"
          style={{
            borderColor: token.colorInfoBorder,
            background: token.colorInfoBg,
          }}
        >
          <Space direction="vertical" size={2} style={{ width: "100%" }}>
            <Space size="small">
              <CompressOutlined style={{ color: token.colorInfo }} />
              <Tag color="purple" style={{ fontSize: 11 }}>compaction</Tag>
              <Text type="secondary" style={{ fontSize: 11 }}>{ts}</Text>
            </Space>
            <Text style={{ fontSize: 12 }}>
              {preTokens != null && postTokens != null && (
                <span>
                  Tokens: {preTokens.toLocaleString()} → {postTokens.toLocaleString()}
                  {" "}(saved {(preTokens - postTokens).toLocaleString()})
                </span>
              )}
              {removed != null && <span> · {removed} messages removed</span>}
            </Text>
          </Space>
        </Card>
      );
    }

    case "session_end": {
      const totalTokens = content?.total_tokens as number | undefined;
      return (
        <Card
          size="small"
          style={{
            borderColor: token.colorBorderSecondary,
            background: token.colorFillQuaternary,
          }}
        >
          <Space size="small">
            <StopOutlined style={{ color: token.colorTextSecondary }} />
            <Tag color="default" style={{ fontSize: 11 }}>session_end</Tag>
            {totalTokens != null && (
              <Text style={{ fontSize: 12 }}>
                Total tokens: {totalTokens.toLocaleString()}
              </Text>
            )}
            <Text type="secondary" style={{ fontSize: 11 }}>{ts}</Text>
          </Space>
        </Card>
      );
    }

    case "mcp_tools_loaded":
    case "skills_loaded":
    default: {
      return (
        <Card
          size="small"
          style={{
            borderColor: token.colorBorderSecondary,
            background: token.colorFillQuaternary,
          }}
        >
          <Space size="small">
            <CodeOutlined style={{ color: token.colorTextSecondary }} />
            <Tag color="default" style={{ fontSize: 11 }}>{event.event_type}</Tag>
            <Text type="secondary" style={{ fontSize: 11 }}>{ts}</Text>
          </Space>
        </Card>
      );
    }
  }
}

export default function SessionHistorySection() {
  const { token } = theme.useToken();
  const [files, setFiles] = useState<HistoryFileInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [detailOpen, setDetailOpen] = useState(false);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailEvents, setDetailEvents] = useState<ConversationEvent[]>([]);
  const [detailFile, setDetailFile] = useState<HistoryFileInfo | null>(null);

  const fetchHistory = useCallback(async () => {
    setLoading(true);
    try {
      const data = await get<{ history: HistoryFileInfo[]; total: number }>(
        `${BASE}/agent/sessions/history`,
      );
      setFiles(data.history || []);
    } catch {
      // silent
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchHistory();
  }, [fetchHistory]);

  const handleView = async (file: HistoryFileInfo) => {
    setDetailFile(file);
    setDetailOpen(true);
    setDetailLoading(true);
    setDetailEvents([]);
    try {
      const data = await get<{ events: ConversationEvent[]; count: number }>(
        `${BASE}/agent/sessions/history/${encodeURIComponent(file.path)}`,
      );
      setDetailEvents(data.events || []);
    } catch {
      message.error("Failed to load session history");
      setDetailOpen(false);
    } finally {
      setDetailLoading(false);
    }
  };

  const handleDelete = async (file: HistoryFileInfo) => {
    try {
      await del(`${BASE}/agent/sessions/history/${encodeURIComponent(file.path)}`);
      message.success("History file deleted");
      fetchHistory();
    } catch {
      message.error("Failed to delete history file");
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
      title: "Created",
      dataIndex: "timestamp",
      key: "timestamp",
      width: 170,
      render: (val?: number) => formatTs(val),
    },
    {
      title: "Filename",
      dataIndex: "filename",
      key: "filename",
      ellipsis: true,
      render: (val: string) => (
        <Tooltip title={val}>
          <Text style={{ fontSize: 11 }}>{val}</Text>
        </Tooltip>
      ),
    },
    {
      title: "Actions",
      key: "actions",
      width: 100,
      render: (_: unknown, record: HistoryFileInfo) => (
        <Space size="small">
          <Tooltip title="View events">
            <Button
              size="small"
              icon={<EyeOutlined />}
              onClick={() => handleView(record)}
            />
          </Tooltip>
          <Popconfirm
            title="Delete this history file?"
            onConfirm={() => handleDelete(record)}
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
            {files.length} persisted session{files.length !== 1 ? "s" : ""}
          </Text>
        </Col>
        <Col>
          <Button
            icon={<ReloadOutlined />}
            size="small"
            onClick={fetchHistory}
            loading={loading}
          >
            Refresh
          </Button>
        </Col>
      </Row>
      <Table
        dataSource={files}
        columns={columns}
        rowKey="path"
        size="small"
        loading={loading}
        pagination={{ pageSize: 10, size: "small" }}
        locale={{ emptyText: <Empty description="No persisted sessions" /> }}
        scroll={{ x: 700 }}
      />

      {/* History Detail Modal - Event Timeline */}
      <Modal
        title={
          detailFile ? (
            <Space>
              <FileTextOutlined />
              <span>Session Timeline</span>
              <Tag>
                <ClockCircleOutlined /> {detailEvents.length} events
              </Tag>
            </Space>
          ) : (
            "Session Timeline"
          )
        }
        open={detailOpen}
        onCancel={() => setDetailOpen(false)}
        footer={null}
        width={780}
      >
        {detailLoading ? (
          <Spin style={{ display: "block", margin: "40px auto" }} />
        ) : (
          <Space direction="vertical" style={{ width: "100%" }} size="middle">
            {detailFile && (
              <Row gutter={[16, 8]}>
                <Col span={12}>
                  <Text type="secondary" style={{ fontSize: 12 }}>Session Key</Text>
                  <br />
                  <Text code style={{ fontSize: 11 }}>{detailFile.session_key}</Text>
                </Col>
                <Col span={12}>
                  <Text type="secondary" style={{ fontSize: 12 }}>Created</Text>
                  <br />
                  <Text>{formatTs(detailFile.timestamp)}</Text>
                </Col>
              </Row>
            )}

            <Divider style={{ margin: "8px 0" }} />

            <Text strong>Events ({detailEvents.length})</Text>
            <div
              style={{
                maxHeight: 500,
                overflowY: "auto",
                display: "flex",
                flexDirection: "column",
                gap: 8,
              }}
            >
              {detailEvents.length === 0 ? (
                <Empty description="No events in this session" />
              ) : (
                detailEvents.map((evt, idx) => (
                  <EventCard key={idx} event={evt} token={token} />
                ))
              )}
            </div>
          </Space>
        )}
      </Modal>
    </Space>
  );
}
