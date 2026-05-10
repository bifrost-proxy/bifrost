import { useCallback, useEffect, useState } from "react";
import type { ReactNode } from "react";
import { useSearchParams } from "react-router-dom";
import {
  Badge,
  Button,
  Card,
  Descriptions,
  Empty,
  Form,
  Grid,
  Input,
  Modal,
  Popconfirm,
  Select,
  Space,
  Spin,
  Switch,
  Table,
  Tabs,
  Tag,
  Tooltip,
  Typography,
  message,
  theme,
} from "antd";
import {
  ApiOutlined,
  CloudOutlined,
  DeleteOutlined,
  EditOutlined,
  HistoryOutlined,
  PauseCircleOutlined,
  PlayCircleOutlined,
  PlusOutlined,
  ReloadOutlined,
  RocketOutlined,
  SendOutlined,
} from "@ant-design/icons";
import * as imGatewayApi from "../../../api/imGateway";
import { get, normalizeApiErrorMessage } from "../../../api/client";
import type {
  ImProviderConfig,
  ImTarget,
  ImRoute,
  ImSchedule,
  ImTaskRun,
  ImEvent,
  ConnectionStatus,
} from "../../../api/imGateway";
import type { AgentConfig } from "./agent/types";
import LongTextModalField from "./agent/LongTextModalField";

const { Text } = Typography;
const { useBreakpoint } = Grid;

type ImGatewaySectionId =
  | "connections"
  | "targets"
  | "routes"
  | "schedules"
  | "history";

const IM_GATEWAY_SECTION_NAV: Array<{
  id: ImGatewaySectionId;
  label: string;
  icon: ReactNode;
}> = [
  { id: "connections", label: "Connections", icon: <ApiOutlined /> },
  { id: "targets", label: "Targets", icon: <SendOutlined /> },
  { id: "routes", label: "Routes", icon: <RocketOutlined /> },
  { id: "schedules", label: "Schedules", icon: <CloudOutlined /> },
  { id: "history", label: "History", icon: <HistoryOutlined /> },
];

// ─── Connections Panel ───────────────────────────────────────────────────────

function ConnectionsPanel({
  providers,
  loading,
  onRefresh,
}: {
  providers: ImProviderConfig[];
  loading: boolean;
  onRefresh: () => void;
}) {
  const { token } = theme.useToken();
  const [statusMap, setStatusMap] = useState<
    Record<string, ConnectionStatus>
  >({});
  const [addModalOpen, setAddModalOpen] = useState(false);
  const [editingProvider, setEditingProvider] =
    useState<ImProviderConfig | null>(null);
  const [agentDefaults, setAgentDefaults] = useState<AgentConfig | null>(null);
  const [form] = Form.useForm();

  const fetchStatuses = useCallback(async () => {
    const map: Record<string, ConnectionStatus> = {};
    for (const p of providers) {
      try {
        map[p.id] = await imGatewayApi.getProviderStatus(p.id);
      } catch {
        // ignore
      }
    }
    setStatusMap(map);
  }, [providers]);

  const fetchAgentDefaults = useCallback(async () => {
    try {
      setAgentDefaults(await get<AgentConfig>("/im-gateway/agent"));
    } catch {
      setAgentDefaults(null);
    }
  }, []);

  useEffect(() => {
    if (providers.length === 0) return;
    const timer = window.setTimeout(() => {
      void fetchStatuses();
    }, 0);
    return () => window.clearTimeout(timer);
  }, [providers, fetchStatuses]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void fetchAgentDefaults();
    }, 0);
    return () => window.clearTimeout(timer);
  }, [fetchAgentDefaults]);

  const inheritedWorkDir = agentDefaults?.work_dir || "Process working directory";
  const inheritedBaseInstructions =
    agentDefaults?.base_instructions ||
    agentDefaults?.default_base_instructions ||
    "Built-in default Agent prompt";
  const inheritedDeveloperInstructions =
    agentDefaults?.developer_instructions || "No global developer instructions";
  const inheritedUserInstructions =
    agentDefaults?.user_instructions || "No global user instructions";

  const normalizeProviderValues = (
    values: Record<string, unknown>,
    clearEmptyAgentConfig: boolean,
  ) => {
    const payload: Record<string, unknown> = { ...values };
    const agentConfig = values.agent_config as
      | {
          work_dir?: string;
          base_instructions?: string;
          developer_instructions?: string;
          user_instructions?: string;
        }
      | undefined;
    const workDir = agentConfig?.work_dir?.trim();
    const baseInstructions = agentConfig?.base_instructions?.trim();
    const developerInstructions = agentConfig?.developer_instructions?.trim();
    const userInstructions = agentConfig?.user_instructions?.trim();
    if (clearEmptyAgentConfig) {
      if (workDir || baseInstructions || developerInstructions || userInstructions) {
        payload.agent_config = {
          work_dir: workDir || null,
          base_instructions: baseInstructions || null,
          developer_instructions: developerInstructions || null,
          user_instructions: userInstructions || null,
        };
      } else {
        payload.agent_config = null;
      }
      return payload;
    }
    if (workDir || baseInstructions || developerInstructions || userInstructions) {
      payload.agent_config = {
        ...(workDir ? { work_dir: workDir } : {}),
        ...(baseInstructions ? { base_instructions: baseInstructions } : {}),
        ...(developerInstructions ? { developer_instructions: developerInstructions } : {}),
        ...(userInstructions ? { user_instructions: userInstructions } : {}),
      };
    } else {
      delete payload.agent_config;
    }
    return payload;
  };

  const openAddModal = () => {
    setEditingProvider(null);
    form.resetFields();
    form.setFieldsValue({
      provider_type: "feishu",
      enabled: true,
      event_connection_enabled: true,
    });
    setAddModalOpen(true);
  };

  const openEditModal = (provider: ImProviderConfig) => {
    setEditingProvider(provider);
    form.setFieldsValue({
      ...provider,
      app_secret: undefined,
      agent_config: {
        work_dir: provider.agent_config?.work_dir,
        base_instructions: provider.agent_config?.base_instructions,
        developer_instructions: provider.agent_config?.developer_instructions,
        user_instructions: provider.agent_config?.user_instructions,
      },
    });
    setAddModalOpen(true);
  };

  const handleSaveProvider = async () => {
    try {
      const values = await form.validateFields();
      if (editingProvider) {
        const appSecret =
          typeof values.app_secret === "string" ? values.app_secret.trim() : "";
        const payload = normalizeProviderValues(
          {
            display_name: values.display_name,
            enabled: values.enabled,
            app_id: values.app_id,
            owner_open_id: values.owner_open_id,
            agent_config: values.agent_config,
            ...(appSecret ? { app_secret: appSecret } : {}),
          },
          true,
        );
        await imGatewayApi.updateProvider(
          editingProvider.id,
          payload as Partial<ImProviderConfig>,
        );
        message.success("Provider updated");
      } else {
        const appSecret =
          typeof values.app_secret === "string" ? values.app_secret.trim() : "";
        await imGatewayApi.createProvider(
          normalizeProviderValues(values, false) as Partial<ImProviderConfig>,
        );
        if (values.enabled && values.event_connection_enabled && appSecret) {
          await imGatewayApi.connectProvider(values.id);
          message.success("Provider created and connected");
        } else {
          message.success("Provider created");
        }
      }
      setAddModalOpen(false);
      setEditingProvider(null);
      form.resetFields();
      onRefresh();
    } catch (err) {
      if (err && typeof err === "object" && "errorFields" in err) return;
      message.error(normalizeApiErrorMessage(err, "Failed to save provider"));
    }
  };

  const handleConnect = async (id: string) => {
    try {
      await imGatewayApi.connectProvider(id);
      message.success("Provider connected");
      onRefresh();
      void fetchStatuses();
    } catch (err) {
      message.error(normalizeApiErrorMessage(err, "Failed to connect provider"));
    }
  };

  const handleDisconnect = async (id: string) => {
    try {
      await imGatewayApi.disconnectProvider(id);
      message.success("Provider disconnected");
      onRefresh();
      void fetchStatuses();
    } catch (err) {
      message.error(normalizeApiErrorMessage(err, "Failed to disconnect provider"));
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await imGatewayApi.deleteProvider(id);
      message.success("Provider deleted");
      onRefresh();
    } catch {
      message.error("Failed to delete provider");
    }
  };

  const handleToggle = async (id: string, enabled: boolean) => {
    try {
      await imGatewayApi.updateProvider(id, { enabled });
      message.success(enabled ? "Provider enabled" : "Provider disabled");
      onRefresh();
    } catch {
      message.error("Failed to update provider");
    }
  };

  const getStateBadge = (state?: string) => {
    switch (state) {
      case "connected":
        return <Badge status="success" text="Connected" />;
      case "connecting":
      case "reconnecting":
        return <Badge status="processing" text={state} />;
      case "disconnected":
        return <Badge status="default" text="Disconnected" />;
      case "failed":
        return <Badge status="error" text="Failed" />;
      default:
        return <Badge status="default" text="Unknown" />;
    }
  };

  return (
    <div>
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          marginBottom: 16,
        }}
      >
        <Text type="secondary">Manage Feishu bot connections</Text>
        <Space>
          <Button icon={<ReloadOutlined />} onClick={onRefresh} loading={loading}>
            Refresh
          </Button>
          <Button
            type="primary"
            icon={<PlusOutlined />}
            onClick={openAddModal}
          >
            Add Provider
          </Button>
        </Space>
      </div>

      {loading && providers.length === 0 ? (
        <Spin style={{ display: "block", margin: "40px auto" }} />
      ) : providers.length === 0 ? (
        <Empty description="No Feishu bots configured" />
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
          {providers.map((p) => {
            const status = statusMap[p.id];
            return (
              <Card
                key={p.id}
                size="small"
                style={{
                  borderColor: token.colorBorderSecondary,
                }}
                title={
                  <Space>
                    <ApiOutlined />
                    <span>{p.display_name || p.id}</span>
                    <Tag>{p.provider_type}</Tag>
                  </Space>
                }
                extra={
                  <Space>
                    {status?.state === "connected" ||
                    status?.state === "connecting" ||
                    status?.state === "reconnecting" ? (
                      <Tooltip title="Disconnect">
                        <Button
                          size="small"
                          icon={<PauseCircleOutlined />}
                          onClick={() => handleDisconnect(p.id)}
                          data-testid={`settings-im-provider-disconnect-${p.id}`}
                        />
                      </Tooltip>
                    ) : (
                      <Tooltip title="Connect">
                        <Button
                          size="small"
                          icon={<PlayCircleOutlined />}
                          onClick={() => handleConnect(p.id)}
                          disabled={!p.secret_configured}
                          data-testid={`settings-im-provider-connect-${p.id}`}
                        />
                      </Tooltip>
                    )}
                    <Tooltip title="Edit">
                      <Button
                        size="small"
                        icon={<EditOutlined />}
                        onClick={() => openEditModal(p)}
                        data-testid={`settings-im-provider-edit-${p.id}`}
                      />
                    </Tooltip>
                    <Switch
                      size="small"
                      checked={p.enabled}
                      onChange={(checked) => handleToggle(p.id, checked)}
                    />
                    <Popconfirm
                      title="Delete this provider?"
                      onConfirm={() => handleDelete(p.id)}
                    >
                      <Button
                        size="small"
                        danger
                        icon={<DeleteOutlined />}
                      />
                    </Popconfirm>
                  </Space>
                }
              >
                <Descriptions size="small" column={3}>
                  <Descriptions.Item label="Status">
                    {getStateBadge(status?.state)}
                  </Descriptions.Item>
                  <Descriptions.Item label="App ID">
                    <Text code>
                      {p.app_id
                        ? `${p.app_id.slice(0, 8)}***`
                        : "-"}
                    </Text>
                  </Descriptions.Item>
                  <Descriptions.Item label="Secret">
                    {p.secret_configured ? (
                      <Tag color="green">Configured</Tag>
                    ) : (
                      <Tag color="red">Not Set</Tag>
                    )}
                  </Descriptions.Item>
                  <Descriptions.Item label="Owner">
                    {p.owner_open_id ? (
                      <Text code style={{ fontSize: 11 }}>
                        {`${p.owner_open_id.slice(0, 12)}...`}
                      </Text>
                    ) : (
                      <Text type="secondary">Auto-detect on connect</Text>
                    )}
                  </Descriptions.Item>
                  <Descriptions.Item label="Connection Mode">
                    {p.event_connection_enabled
                      ? "Long Connection"
                      : "Webhook"}
                  </Descriptions.Item>
                  <Descriptions.Item label="Provider Enabled">
                    {p.enabled ? <Tag color="green">Enabled</Tag> : <Tag>Disabled</Tag>}
                  </Descriptions.Item>
                  <Descriptions.Item label="Agent Work Dir">
                    {p.agent_config?.work_dir ? (
                      <Text code style={{ fontSize: 11 }}>
                        {p.agent_config.work_dir}
                      </Text>
                    ) : (
                      <Text type="secondary">Global default</Text>
                    )}
                  </Descriptions.Item>
                  <Descriptions.Item label="Agent Base Prompt">
                    {p.agent_config?.base_instructions ? (
                      <Tag color="blue">Configured</Tag>
                    ) : (
                      <Text type="secondary">Global default</Text>
                    )}
                  </Descriptions.Item>
                  <Descriptions.Item label="Agent Developer/User">
                    {p.agent_config?.developer_instructions || p.agent_config?.user_instructions ? (
                      <Tag color="purple">Configured</Tag>
                    ) : (
                      <Text type="secondary">Global default</Text>
                    )}
                  </Descriptions.Item>
                  {status?.reconnect_count != null &&
                    status.reconnect_count > 0 && (
                      <Descriptions.Item label="Reconnects">
                        {status.reconnect_count}
                      </Descriptions.Item>
                    )}
                  {status?.last_error && (
                    <Descriptions.Item label="Last Error">
                      <Text type="danger" ellipsis style={{ maxWidth: 300 }}>
                        {status.last_error}
                      </Text>
                    </Descriptions.Item>
                  )}
                </Descriptions>
              </Card>
            );
          })}
        </div>
      )}

      <Modal
        title={editingProvider ? "Edit IM Provider" : "Add IM Provider"}
        open={addModalOpen}
        onOk={handleSaveProvider}
        onCancel={() => {
          setAddModalOpen(false);
          setEditingProvider(null);
          form.resetFields();
        }}
        okText={editingProvider ? "Save" : "Create"}
      >
        <Form form={form} layout="vertical">
          {editingProvider ? (
            <Descriptions size="small" column={1} style={{ marginBottom: 16 }}>
              <Descriptions.Item label="Provider ID">
                <Text code>{editingProvider.id}</Text>
              </Descriptions.Item>
              <Descriptions.Item label="Type">
                <Tag>{editingProvider.provider_type}</Tag>
              </Descriptions.Item>
              <Descriptions.Item label="App ID">
                <Text code>
                  {editingProvider.app_id
                    ? `${editingProvider.app_id.slice(0, 8)}***`
                    : "-"}
                </Text>
              </Descriptions.Item>
              <Descriptions.Item label="Secret">
                {editingProvider.secret_configured ? (
                  <Tag color="green">Configured</Tag>
                ) : (
                  <Tag color="red">Not Set</Tag>
                )}
              </Descriptions.Item>
              <Descriptions.Item label="Connection Mode">
                {editingProvider.event_connection_enabled
                  ? "Long Connection"
                  : "Webhook"}
              </Descriptions.Item>
            </Descriptions>
          ) : (
            <>
              <Form.Item
                name="id"
                label="Provider ID"
                rules={[{ required: true, message: "Required" }]}
              >
                <Input placeholder="e.g. feishu-main" />
              </Form.Item>
              <Form.Item
                name="provider_type"
                label="Type"
                initialValue="feishu"
                rules={[{ required: true, message: "Required" }]}
              >
                <Select
                  placeholder="Select provider type"
                  options={[{ label: "Feishu", value: "feishu" }]}
                />
              </Form.Item>
              <Form.Item>
                <Text type="secondary" style={{ fontSize: 13 }}>
                  前往{" "}
                  <a
                    href="https://open.larkoffice.com/page/launcher?from=backend_oneclick"
                    target="_blank"
                    rel="noopener noreferrer"
                  >
                    飞书开放平台
                  </a>{" "}
                  一键创建机器人应用并获取 App ID 和 App Secret。
                </Text>
              </Form.Item>
            </>
          )}
          <Form.Item name="display_name" label="Display Name">
            <Input placeholder="Optional display name" />
          </Form.Item>
          <Form.Item name="enabled" label="Enabled" valuePropName="checked">
            <Switch />
          </Form.Item>
          <Form.Item name="owner_open_id" label="Owner Open ID">
            <Input placeholder="Optional owner open_id" />
          </Form.Item>
          <Form.Item name="app_id" label="App ID">
            <Input placeholder="Application ID" />
          </Form.Item>
          <Form.Item
            name="app_secret"
            label="App Secret"
            extra={
              editingProvider
                ? "Leave blank to keep the existing secret. Enter a new value to replace it."
                : undefined
            }
          >
            <Input.Password placeholder="Application secret (stored securely)" />
          </Form.Item>
          <Form.Item
            name={["agent_config", "work_dir"]}
            label="Agent Working Directory"
            extra={
              <Text type="secondary">
                Inherits: <Text code>{inheritedWorkDir}</Text>. Enter a value to override for this
                provider.
              </Text>
            }
          >
            <Input placeholder={inheritedWorkDir} />
          </Form.Item>
          <Form.Item
            name={["agent_config", "base_instructions"]}
            extra={
              <Text type="secondary">
                Inherits the global base instructions when empty. Enter text to override for this provider.
              </Text>
            }
          >
            <LongTextModalField
              label="Base Instructions / System Prompt"
              placeholder={inheritedBaseInstructions}
              testId="settings-im-provider-base-instructions"
              copyPlaceholderLabel="Copy inherited into editor"
            />
          </Form.Item>
          <Form.Item
            name={["agent_config", "developer_instructions"]}
            extra={
              <Text type="secondary">
                Inherits: {inheritedDeveloperInstructions}. Enter text to override for this provider.
              </Text>
            }
          >
            <LongTextModalField
              label="Developer Instructions"
              placeholder={inheritedDeveloperInstructions}
              testId="settings-im-provider-developer-instructions"
            />
          </Form.Item>
          <Form.Item
            name={["agent_config", "user_instructions"]}
            extra={
              <Text type="secondary">
                Inherits: {inheritedUserInstructions}. Enter text to override for this provider.
              </Text>
            }
          >
            <LongTextModalField
              label="User Instructions"
              placeholder={inheritedUserInstructions}
              testId="settings-im-provider-user-instructions"
            />
          </Form.Item>
          <Form.Item
            name="event_connection_enabled"
            hidden
            initialValue={true}
          >
            <Switch />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}

// ─── Targets Panel ───────────────────────────────────────────────────────────

function TargetsPanel({
  targets,
  providers,
  loading,
  onRefresh,
}: {
  targets: ImTarget[];
  providers: ImProviderConfig[];
  loading: boolean;
  onRefresh: () => void;
}) {
  const [addModalOpen, setAddModalOpen] = useState(false);
  const [form] = Form.useForm();

  const handleAdd = async () => {
    try {
      const values = await form.validateFields();
      await imGatewayApi.createTarget(values);
      message.success("Target created");
      setAddModalOpen(false);
      form.resetFields();
      onRefresh();
    } catch (err) {
      if (err && typeof err === "object" && "errorFields" in err) return;
      message.error("Failed to create target");
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await imGatewayApi.deleteTarget(id);
      message.success("Target deleted");
      onRefresh();
    } catch {
      message.error("Failed to delete target");
    }
  };

  const columns = [
    {
      title: "ID",
      dataIndex: "id",
      key: "id",
      width: 160,
    },
    {
      title: "Provider",
      dataIndex: "provider_id",
      key: "provider_id",
      width: 140,
    },
    {
      title: "ID Type",
      dataIndex: "receive_id_type",
      key: "receive_id_type",
      width: 120,
    },
    {
      title: "Receive ID",
      dataIndex: "receive_id",
      key: "receive_id",
      render: (val: string) => (
        <Text code style={{ fontSize: 12 }}>
          {val && val.length > 12 ? `${val.slice(0, 12)}***` : val}
        </Text>
      ),
    },
    {
      title: "Enabled",
      dataIndex: "enabled",
      key: "enabled",
      width: 80,
      render: (val: boolean) => (
        <Tag color={val ? "green" : "default"}>{val ? "Yes" : "No"}</Tag>
      ),
    },
    {
      title: "Actions",
      key: "actions",
      width: 80,
      render: (_: unknown, record: ImTarget) => (
        <Popconfirm
          title="Delete this target?"
          onConfirm={() => handleDelete(record.id)}
        >
          <Button size="small" danger icon={<DeleteOutlined />} />
        </Popconfirm>
      ),
    },
  ];

  return (
    <div>
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          marginBottom: 16,
        }}
      >
        <Text type="secondary">
          Message targets define where to send notifications
        </Text>
        <Space>
          <Button icon={<ReloadOutlined />} onClick={onRefresh} loading={loading}>
            Refresh
          </Button>
          <Button
            type="primary"
            icon={<PlusOutlined />}
            onClick={() => setAddModalOpen(true)}
          >
            Add Target
          </Button>
        </Space>
      </div>

      <Table
        dataSource={targets}
        columns={columns}
        rowKey="id"
        size="small"
        loading={loading}
        pagination={false}
        locale={{ emptyText: <Empty description="No targets configured" /> }}
      />

      <Modal
        title="Add Target"
        open={addModalOpen}
        onOk={handleAdd}
        onCancel={() => {
          setAddModalOpen(false);
          form.resetFields();
        }}
        okText="Create"
      >
        <Form form={form} layout="vertical">
          <Form.Item
            name="id"
            label="Target ID"
            rules={[{ required: true, message: "Required" }]}
          >
            <Input placeholder="e.g. oncall-group" />
          </Form.Item>
          <Form.Item
            name="provider_id"
            label="Provider"
            rules={[{ required: true, message: "Required" }]}
          >
            <Select
              placeholder="Select provider"
              options={providers.map((p) => ({
                label: p.display_name || p.id,
                value: p.id,
              }))}
            />
          </Form.Item>
          <Form.Item
            name="receive_id_type"
            label="Receive ID Type"
            rules={[{ required: true, message: "Required" }]}
          >
            <Select
              placeholder="Select type"
              options={[
                { label: "Chat ID", value: "chat_id" },
                { label: "User ID", value: "open_id" },
                { label: "Email", value: "email" },
                { label: "Union ID", value: "union_id" },
              ]}
            />
          </Form.Item>
          <Form.Item
            name="receive_id"
            label="Receive ID"
            rules={[{ required: true, message: "Required" }]}
          >
            <Input placeholder="e.g. oc_xxxxx" />
          </Form.Item>
          <Form.Item name="display_name" label="Display Name">
            <Input placeholder="Optional display name" />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}

// ─── Routes Panel ────────────────────────────────────────────────────────────

function RoutesPanel({
  routes,
  loading,
  onRefresh,
}: {
  routes: ImRoute[];
  loading: boolean;
  onRefresh: () => void;
}) {
  const { token } = theme.useToken();

  const handlePause = async (id: string) => {
    try {
      await imGatewayApi.pauseRoute(id);
      message.success("Route paused");
      onRefresh();
    } catch {
      message.error("Failed to pause route");
    }
  };

  const handleResume = async (id: string) => {
    try {
      await imGatewayApi.resumeRoute(id);
      message.success("Route resumed");
      onRefresh();
    } catch {
      message.error("Failed to resume route");
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await imGatewayApi.deleteRoute(id);
      message.success("Route deleted");
      onRefresh();
    } catch {
      message.error("Failed to delete route");
    }
  };

  return (
    <div>
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          marginBottom: 16,
        }}
      >
        <Text type="secondary">
          Event routes map incoming messages to script actions
        </Text>
        <Button icon={<ReloadOutlined />} onClick={onRefresh} loading={loading}>
          Refresh
        </Button>
      </div>

      {loading && routes.length === 0 ? (
        <Spin style={{ display: "block", margin: "40px auto" }} />
      ) : routes.length === 0 ? (
        <Empty description="No routes configured" />
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
          {routes.map((r) => (
            <Card
              key={r.id}
              size="small"
              style={{ borderColor: token.colorBorderSecondary }}
              title={
                <Space>
                  <RocketOutlined />
                  <span>{r.name || r.id}</span>
                  <Tag color={r.enabled ? "green" : "default"}>
                    {r.enabled ? "Active" : "Paused"}
                  </Tag>
                </Space>
              }
              extra={
                <Space>
                  {r.enabled ? (
                    <Tooltip title="Pause">
                      <Button
                        size="small"
                        icon={<PauseCircleOutlined />}
                        onClick={() => handlePause(r.id)}
                      />
                    </Tooltip>
                  ) : (
                    <Tooltip title="Resume">
                      <Button
                        size="small"
                        icon={<PlayCircleOutlined />}
                        onClick={() => handleResume(r.id)}
                      />
                    </Tooltip>
                  )}
                  <Popconfirm
                    title="Delete this route?"
                    onConfirm={() => handleDelete(r.id)}
                  >
                    <Button size="small" danger icon={<DeleteOutlined />} />
                  </Popconfirm>
                </Space>
              }
            >
              <Descriptions size="small" column={3}>
                <Descriptions.Item label="Provider">
                  {r.provider_id}
                </Descriptions.Item>
                <Descriptions.Item label="Event">
                  <Tag>{r.event_type}</Tag>
                </Descriptions.Item>
                <Descriptions.Item label="Matcher">
                  {r.matcher?.regex && (
                    <Tag color="blue">regex: {r.matcher.regex}</Tag>
                  )}
                  {r.matcher?.keyword && (
                    <Tag color="cyan">kw: {r.matcher.keyword}</Tag>
                  )}
                  {r.matcher?.chat_ids?.length > 0 && (
                    <Tag>chats: {r.matcher.chat_ids.length}</Tag>
                  )}
                  {!r.matcher?.regex &&
                    !r.matcher?.keyword &&
                    (!r.matcher?.chat_ids || r.matcher.chat_ids.length === 0) && (
                      <Text type="secondary">*</Text>
                    )}
                </Descriptions.Item>
                <Descriptions.Item label="Action">
                  <Tag>{r.action?.type || "script"}</Tag>
                </Descriptions.Item>
                <Descriptions.Item label="Timeout">
                  {r.timeout_ms ? `${r.timeout_ms}ms` : "-"}
                </Descriptions.Item>
              </Descriptions>
            </Card>
          ))}
        </div>
      )}
    </div>
  );
}

// ─── Schedules Panel ─────────────────────────────────────────────────────────

function SchedulesPanel({
  schedules,
  loading,
  onRefresh,
}: {
  schedules: ImSchedule[];
  loading: boolean;
  onRefresh: () => void;
}) {
  const handlePause = async (id: string) => {
    try {
      await imGatewayApi.pauseSchedule(id);
      message.success("Schedule paused");
      onRefresh();
    } catch {
      message.error("Failed to pause schedule");
    }
  };

  const handleResume = async (id: string) => {
    try {
      await imGatewayApi.resumeSchedule(id);
      message.success("Schedule resumed");
      onRefresh();
    } catch {
      message.error("Failed to resume schedule");
    }
  };

  const handleRun = async (id: string) => {
    try {
      await imGatewayApi.runSchedule(id);
      message.success("Schedule triggered");
      onRefresh();
    } catch {
      message.error("Failed to trigger schedule");
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await imGatewayApi.deleteSchedule(id);
      message.success("Schedule deleted");
      onRefresh();
    } catch {
      message.error("Failed to delete schedule");
    }
  };

  const columns = [
    {
      title: "Name",
      dataIndex: "name",
      key: "name",
      width: 160,
    },
    {
      title: "Target",
      dataIndex: "target_id",
      key: "target_id",
      width: 140,
    },
    {
      title: "Trigger",
      key: "trigger",
      width: 160,
      render: (_: unknown, record: ImSchedule) => {
        if (record.trigger?.type === "cron") {
          return <Text code style={{ fontSize: 12 }}>{record.trigger.expr}</Text>;
        }
        if (record.trigger?.type === "interval" && record.trigger.every_ms) {
          return <Text code style={{ fontSize: 12 }}>every {record.trigger.every_ms}ms</Text>;
        }
        return "-";
      },
    },
    {
      title: "Enabled",
      dataIndex: "enabled",
      key: "enabled",
      width: 80,
      render: (val: boolean) => (
        <Tag color={val ? "green" : "default"}>{val ? "Yes" : "No"}</Tag>
      ),
    },
    {
      title: "Timeout",
      dataIndex: "timeout_ms",
      key: "timeout_ms",
      width: 100,
      render: (val: number) => (val ? `${val}ms` : "-"),
    },
    {
      title: "Actions",
      key: "actions",
      width: 160,
      render: (_: unknown, record: ImSchedule) => (
        <Space size="small">
          <Tooltip title="Run now">
            <Button
              size="small"
              icon={<PlayCircleOutlined />}
              onClick={() => handleRun(record.id)}
            />
          </Tooltip>
          {record.enabled ? (
            <Tooltip title="Pause">
              <Button
                size="small"
                icon={<PauseCircleOutlined />}
                onClick={() => handlePause(record.id)}
              />
            </Tooltip>
          ) : (
            <Tooltip title="Resume">
              <Button
                size="small"
                type="primary"
                ghost
                icon={<PlayCircleOutlined />}
                onClick={() => handleResume(record.id)}
              />
            </Tooltip>
          )}
          <Popconfirm
            title="Delete this schedule?"
            onConfirm={() => handleDelete(record.id)}
          >
            <Button size="small" danger icon={<DeleteOutlined />} />
          </Popconfirm>
        </Space>
      ),
    },
  ];

  return (
    <div>
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          marginBottom: 16,
        }}
      >
        <Text type="secondary">
          Scheduled tasks run scripts on a cron/interval basis
        </Text>
        <Button icon={<ReloadOutlined />} onClick={onRefresh} loading={loading}>
          Refresh
        </Button>
      </div>

      <Table
        dataSource={schedules}
        columns={columns}
        rowKey="id"
        size="small"
        loading={loading}
        pagination={false}
        locale={{ emptyText: <Empty description="No schedules configured" /> }}
      />
    </div>
  );
}

// ─── History Panel ───────────────────────────────────────────────────────────

function HistoryPanel({
  events,
  runs,
  loading,
  onRefresh,
}: {
  events: ImEvent[];
  runs: ImTaskRun[];
  loading: boolean;
  onRefresh: () => void;
}) {
  const formatTs = (ts?: number) => {
    if (!ts) return "-";
    const secs = ts > 1_000_000_000_000 ? ts / 1000 : ts;
    return new Date(secs * 1000).toLocaleString();
  };

  const eventColumns = [
    {
      title: "Event ID",
      dataIndex: "event_id",
      key: "event_id",
      width: 120,
      render: (val: string) => (
        <Text code style={{ fontSize: 11 }}>
          {val?.slice(0, 10) || "-"}
        </Text>
      ),
    },
    {
      title: "Provider",
      dataIndex: "provider_id",
      key: "provider_id",
      width: 120,
    },
    {
      title: "Event Type",
      dataIndex: "event_type",
      key: "event_type",
      width: 160,
      render: (val: string) => <Tag>{val}</Tag>,
    },
    {
      title: "Source",
      key: "source",
      width: 140,
      render: (_: unknown, record: ImEvent) =>
        record.source?.chat_id || record.source?.user_id || "-",
    },
    {
      title: "Message",
      key: "message",
      render: (_: unknown, record: ImEvent) => (
        <Text ellipsis style={{ maxWidth: 200 }}>
          {record.message?.text || "-"}
        </Text>
      ),
    },
    {
      title: "Time",
      dataIndex: "received_at",
      key: "received_at",
      width: 160,
      render: (val: number) => formatTs(val),
    },
  ];

  const runColumns = [
    {
      title: "Run ID",
      dataIndex: "run_id",
      key: "run_id",
      width: 120,
      render: (val: string) => (
        <Text code style={{ fontSize: 11 }}>
          {val?.slice(0, 10) || "-"}
        </Text>
      ),
    },
    {
      title: "Trigger",
      dataIndex: "trigger_source",
      key: "trigger_source",
      width: 100,
      render: (val: string) => <Tag>{val}</Tag>,
    },
    {
      title: "Status",
      dataIndex: "status",
      key: "status",
      width: 100,
      render: (val: string) => {
        const colorMap: Record<string, string> = {
          success: "green",
          completed: "green",
          running: "processing",
          failed: "red",
          error: "red",
        };
        return <Tag color={colorMap[val] || "default"}>{val}</Tag>;
      },
    },
    {
      title: "Duration",
      dataIndex: "duration_ms",
      key: "duration_ms",
      width: 100,
      render: (val: number) => (val != null ? `${val}ms` : "-"),
    },
    {
      title: "Exit Code",
      dataIndex: "exit_code",
      key: "exit_code",
      width: 80,
      render: (val: number) => (val != null ? val : "-"),
    },
    {
      title: "Output",
      key: "output",
      render: (_: unknown, record: ImTaskRun) => (
        <Text ellipsis style={{ maxWidth: 200, fontSize: 12 }}>
          {record.stdout_preview || record.error || "-"}
        </Text>
      ),
    },
    {
      title: "Time",
      dataIndex: "started_at",
      key: "started_at",
      width: 160,
      render: (val: number) => formatTs(val),
    },
  ];

  const historyTabItems = [
    {
      key: "events",
      label: "Events",
      children: (
        <Table
          dataSource={events}
          columns={eventColumns}
          rowKey="event_id"
          size="small"
          loading={loading}
          pagination={{ pageSize: 20, size: "small" }}
          locale={{ emptyText: <Empty description="No events recorded" /> }}
        />
      ),
    },
    {
      key: "runs",
      label: "Task Runs",
      children: (
        <Table
          dataSource={runs}
          columns={runColumns}
          rowKey="run_id"
          size="small"
          loading={loading}
          pagination={{ pageSize: 20, size: "small" }}
          locale={{ emptyText: <Empty description="No task runs recorded" /> }}
        />
      ),
    },
  ];

  return (
    <div>
      <div
        style={{
          display: "flex",
          justifyContent: "flex-end",
          marginBottom: 16,
        }}
      >
        <Button icon={<ReloadOutlined />} onClick={onRefresh} loading={loading}>
          Refresh
        </Button>
      </div>
      <Tabs items={historyTabItems} size="small" />
    </div>
  );
}

// ─── Main Tab Component ──────────────────────────────────────────────────────

export default function ImGatewayTab() {
  const [searchParams, setSearchParams] = useSearchParams();
  const sectionFromUrl = searchParams.get("imGatewaySection");
  const activeSection =
    IM_GATEWAY_SECTION_NAV.find((section) => section.id === sectionFromUrl)?.id ??
    "connections";
  const { token } = theme.useToken();
  const screens = useBreakpoint();
  const isCompactNav = !screens.lg;

  const [providers, setProviders] = useState<ImProviderConfig[]>([]);
  const [targets, setTargets] = useState<ImTarget[]>([]);
  const [routes, setRoutes] = useState<ImRoute[]>([]);
  const [schedules, setSchedules] = useState<ImSchedule[]>([]);
  const [events, setEvents] = useState<ImEvent[]>([]);
  const [runs, setRuns] = useState<ImTaskRun[]>([]);
  const [loading, setLoading] = useState(false);

  const fetchData = useCallback(async () => {
    setLoading(true);
    try {
      switch (activeSection) {
        case "connections":
          setProviders(await imGatewayApi.listProviders());
          break;
        case "targets": {
          const [t, p] = await Promise.all([
            imGatewayApi.listTargets(),
            imGatewayApi.listProviders(),
          ]);
          setTargets(t);
          setProviders(p);
          break;
        }
        case "routes":
          setRoutes(await imGatewayApi.listRoutes());
          break;
        case "schedules":
          setSchedules(await imGatewayApi.listSchedules());
          break;
        case "history": {
          const [e, r] = await Promise.all([
            imGatewayApi.listHistoryEvents(),
            imGatewayApi.listHistoryRuns(),
          ]);
          setEvents(e);
          setRuns(r);
          break;
        }
      }
    } catch (err) {
      console.error("Failed to fetch IM Gateway data:", err);
      message.error(err instanceof Error ? err.message : "Failed to fetch IM Gateway data");
    } finally {
      setLoading(false);
    }
  }, [activeSection]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  const handleSelectSection = useCallback(
    (section: ImGatewaySectionId) => {
      setLoading(false);
      setSearchParams(
        (prev) => {
          prev.set("imGatewaySection", section);
          return prev;
        },
        { replace: false },
      );
    },
    [setSearchParams],
  );

  const nav = (
    <nav
      aria-label="IM Gateway settings sections"
      data-testid="im-gateway-section-nav"
      style={{
        display: "flex",
        flexDirection: isCompactNav ? "row" : "column",
        gap: 6,
        overflowX: isCompactNav ? "auto" : undefined,
        padding: isCompactNav ? "0 0 4px" : 0,
      }}
    >
      {IM_GATEWAY_SECTION_NAV.map((section) => {
        const active = activeSection === section.id;
        return (
          <button
            key={section.id}
            type="button"
            data-testid={`im-gateway-nav-${section.id}`}
            aria-current={active ? "true" : undefined}
            onClick={() => handleSelectSection(section.id)}
            style={{
              width: isCompactNav ? "auto" : "100%",
              minWidth: isCompactNav ? 118 : undefined,
              border: `1px solid ${
                active ? token.colorPrimaryBorder : token.colorBorderSecondary
              }`,
              borderRadius: 6,
              background: active ? token.colorPrimaryBg : token.colorBgContainer,
              color: active ? token.colorPrimaryText : token.colorTextSecondary,
              cursor: "pointer",
              font: "inherit",
              fontSize: 12,
              fontWeight: active ? 600 : 400,
              lineHeight: "18px",
              padding: "7px 10px",
              textAlign: "left",
              whiteSpace: "nowrap",
              transition: "background 0.2s, border-color 0.2s, color 0.2s",
            }}
          >
            <Space size={6}>
              {section.icon}
              <span>{section.label}</span>
            </Space>
          </button>
        );
      })}
    </nav>
  );

  const content = (() => {
    switch (activeSection) {
      case "connections":
        return (
          <div data-testid="im-gateway-section-connections">
        <ConnectionsPanel
          providers={providers}
          loading={loading}
          onRefresh={fetchData}
        />
          </div>
        );
      case "targets":
        return (
          <div data-testid="im-gateway-section-targets">
        <TargetsPanel
          targets={targets}
          providers={providers}
          loading={loading}
          onRefresh={fetchData}
        />
          </div>
        );
      case "routes":
        return (
          <div data-testid="im-gateway-section-routes">
        <RoutesPanel routes={routes} loading={loading} onRefresh={fetchData} />
          </div>
        );
      case "schedules":
        return (
          <div data-testid="im-gateway-section-schedules">
        <SchedulesPanel
          schedules={schedules}
          loading={loading}
          onRefresh={fetchData}
        />
          </div>
        );
      case "history":
        return (
          <div data-testid="im-gateway-section-history">
        <HistoryPanel
          events={events}
          runs={runs}
          loading={loading}
          onRefresh={fetchData}
        />
          </div>
        );
    }
  })();

  return (
    <div
      data-testid="im-gateway-layout"
      style={{ height: "100%", minHeight: 0, overflow: "hidden" }}
    >
      <div
        style={{
          display: "grid",
          gridTemplateColumns: isCompactNav ? "1fr" : "168px minmax(0, 1fr)",
          gap: 16,
          height: "100%",
          minHeight: 0,
        }}
      >
        <div>{nav}</div>
        <div
          data-testid="im-gateway-section-content"
          style={{
            minHeight: 0,
            overflowY: "auto",
            overflowX: "hidden",
            paddingRight: isCompactNav ? 0 : 4,
          }}
        >
          {content}
        </div>
      </div>
    </div>
  );
}
