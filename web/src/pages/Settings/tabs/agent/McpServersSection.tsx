/**
 * MCP Servers Section - Manage MCP server configurations
 */
import { useCallback, useEffect, useState } from "react";
import {
  Button,
  Card,
  Col,
  Empty,
  Input,
  Modal,
  Popconfirm,
  Row,
  Space,
  Switch,
  Tag,
  Tooltip,
  Typography,
  message,
  theme,
} from "antd";
import {
  CheckCircleOutlined,
  CloseCircleOutlined,
  DeleteOutlined,
  EditOutlined,
  LoadingOutlined,
  PauseCircleOutlined,
  PlusOutlined,
  ReloadOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import { BASE, type McpServerConfig } from "./types";
import { get } from "../../../../api/client";

const { Text } = Typography;
const { TextArea } = Input;

interface McpServersSectionProps {
  servers: Record<string, McpServerConfig>;
  onUpdate: (servers: Record<string, McpServerConfig>) => void;
}

type McpAvailabilityStatus = "available" | "unavailable" | "disabled";

interface McpServerAvailability {
  name: string;
  enabled: boolean;
  status: McpAvailabilityStatus;
  tool_count: number;
  error?: string;
}

interface McpAvailabilityResponse {
  servers: McpServerAvailability[];
}

export default function McpServersSection({
  servers,
  onUpdate,
}: McpServersSectionProps) {
  const { token } = theme.useToken();
  const [editModalOpen, setEditModalOpen] = useState(false);
  const [editName, setEditName] = useState("");
  const [editJson, setEditJson] = useState("");
  const [isNew, setIsNew] = useState(false);
  const [checking, setChecking] = useState(false);
  const [checkError, setCheckError] = useState<string | null>(null);
  const [availability, setAvailability] = useState<
    Record<string, McpServerAvailability>
  >({});

  const refreshAvailability = useCallback(async () => {
    const names = Object.keys(servers);
    if (names.length === 0) {
      setAvailability({});
      setCheckError(null);
      return;
    }

    setChecking(true);
    setCheckError(null);
    try {
      const data = await get<McpAvailabilityResponse>(`${BASE}/agent/mcp-status`);
      const next = Object.fromEntries(
        (data.servers || []).map((server) => [server.name, server]),
      );
      setAvailability(next);
    } catch {
      setCheckError("Failed to check MCP server availability");
    } finally {
      setChecking(false);
    }
  }, [servers]);

  useEffect(() => {
    refreshAvailability();
  }, [refreshAvailability]);

  const handleEdit = (name: string) => {
    setEditName(name);
    setEditJson(JSON.stringify(servers[name], null, 2));
    setIsNew(false);
    setEditModalOpen(true);
  };

  const handleAdd = () => {
    setEditName("");
    setEditJson(
      JSON.stringify(
        { enabled: true, command: "", args: [] },
        null,
        2,
      ),
    );
    setIsNew(true);
    setEditModalOpen(true);
  };

  const handleSave = () => {
    if (!editName.trim()) {
      message.warning("Server name is required");
      return;
    }
    try {
      const parsed = JSON.parse(editJson) as McpServerConfig;
      const updated = { ...servers, [editName.trim()]: parsed };
      onUpdate(updated);
      setEditModalOpen(false);
    } catch {
      message.error("Invalid JSON");
    }
  };

  const handleDelete = (name: string) => {
    const updated = { ...servers };
    delete updated[name];
    onUpdate(updated);
  };

  const handleToggle = (name: string, enabled: boolean) => {
    const updated = {
      ...servers,
      [name]: { ...servers[name], enabled },
    };
    onUpdate(updated);
  };

  const entries = Object.entries(servers);
  const renderAvailability = (name: string, cfg: McpServerConfig) => {
    const status = availability[name];
    if (!status) {
      if (checking) {
        return (
          <Tag icon={<LoadingOutlined />} color="processing">
            Checking
          </Tag>
        );
      }
      return <Tag color="default">{cfg.enabled ? "Not checked" : "Disabled"}</Tag>;
    }
    if (status.status === "available") {
      return (
        <Tag icon={<CheckCircleOutlined />} color="success">
          Available
        </Tag>
      );
    }
    if (status.status === "disabled") {
      return (
        <Tag icon={<PauseCircleOutlined />} color="default">
          Disabled
        </Tag>
      );
    }
    return (
      <Tag icon={<CloseCircleOutlined />} color="error">
        Unavailable
      </Tag>
    );
  };

  return (
    <Space direction="vertical" style={{ width: "100%" }}>
      <Row justify="space-between" align="middle">
        <Col>
          <Text type="secondary" style={{ fontSize: 12 }}>
            MCP servers provide tool capabilities to the agent
          </Text>
        </Col>
        <Col>
          <Space size="small">
            <Button
              icon={<ReloadOutlined />}
              size="small"
              loading={checking}
              onClick={refreshAvailability}
              data-testid="settings-agent-mcp-refresh"
            >
              Refresh Status
            </Button>
            <Button icon={<PlusOutlined />} size="small" onClick={handleAdd}>
              Add Server
            </Button>
          </Space>
        </Col>
      </Row>

      {entries.length === 0 ? (
        <Empty description="No MCP servers configured" />
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          {checkError && (
            <Text type="danger" style={{ fontSize: 12 }}>
              {checkError}
            </Text>
          )}
          {entries.map(([name, cfg]) => (
            <Card
              key={name}
              data-testid={`settings-agent-mcp-server-${name}`}
              size="small"
              style={{ borderColor: token.colorBorderSecondary }}
              title={
                <Space>
                  <ThunderboltOutlined />
                  <Text strong>{name}</Text>
                  <Tag color={cfg.url ? "blue" : "geekblue"}>
                    {cfg.url ? "HTTP" : "Stdio"}
                  </Tag>
                  <span data-testid={`settings-agent-mcp-status-${name}`}>
                    {renderAvailability(name, cfg)}
                  </span>
                </Space>
              }
              extra={
                <Space size="small">
                  <Switch
                    size="small"
                    checked={cfg.enabled}
                    onChange={(checked) => handleToggle(name, checked)}
                  />
                  <Tooltip title="Edit">
                    <Button
                      size="small"
                      icon={<EditOutlined />}
                      onClick={() => handleEdit(name)}
                    />
                  </Tooltip>
                  <Popconfirm
                    title="Delete this MCP server?"
                    onConfirm={() => handleDelete(name)}
                  >
                    <Button size="small" danger icon={<DeleteOutlined />} />
                  </Popconfirm>
                </Space>
              }
            >
              <Space direction="vertical" size={2} style={{ width: "100%" }}>
                {availability[name]?.status === "available" && (
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    Tools: <Text code>{availability[name].tool_count}</Text>
                  </Text>
                )}
                {availability[name]?.status === "unavailable" && (
                  <Text type="danger" style={{ fontSize: 12 }}>
                    {availability[name].error || "MCP server is unavailable"}
                  </Text>
                )}
                {cfg.url && (
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    URL: <Text code>{cfg.url}</Text>
                  </Text>
                )}
                {cfg.command && (
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    Command: <Text code>{cfg.command} {cfg.args?.join(" ") || ""}</Text>
                  </Text>
                )}
                {cfg.cwd && (
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    CWD: <Text code>{cfg.cwd}</Text>
                  </Text>
                )}
              </Space>
            </Card>
          ))}
        </div>
      )}

      <Modal
        title={isNew ? "Add MCP Server" : `Edit: ${editName}`}
        open={editModalOpen}
        onOk={handleSave}
        onCancel={() => setEditModalOpen(false)}
        okText="Save"
        width={560}
      >
        {isNew && (
          <div style={{ marginBottom: 12 }}>
            <Text strong style={{ display: "block", marginBottom: 4 }}>
              Server Name
            </Text>
            <Input
              value={editName}
              onChange={(e) => setEditName(e.target.value)}
              placeholder="e.g. my-mcp-server"
            />
          </div>
        )}
        <Text strong style={{ display: "block", marginBottom: 4 }}>
          Configuration (JSON)
        </Text>
        <TextArea
          value={editJson}
          onChange={(e) => setEditJson(e.target.value)}
          rows={12}
          style={{ fontFamily: "monospace", fontSize: 12 }}
        />
      </Modal>
    </Space>
  );
}
