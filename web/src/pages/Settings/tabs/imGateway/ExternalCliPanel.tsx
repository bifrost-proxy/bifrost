import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  Button,
  Card,
  Descriptions,
  Form,
  Input,
  Modal,
  Popconfirm,
  Select,
  Space,
  Switch,
  Table,
  Tag,
  Typography,
  message,
} from "antd";
import {
  DeleteOutlined,
  EditOutlined,
  LoginOutlined,
  PlusOutlined,
  ReloadOutlined,
  RocketOutlined,
} from "@ant-design/icons";
import * as imGatewayApi from "../../../../api/imGateway";
import { normalizeApiErrorMessage } from "../../../../api/client";
import type {
  ExternalCliAgentSettings,
  ChatGptWebAuthStatus,
  ExternalCliChannelSettings,
  ExternalCliGatewayConfig,
  ExternalCliRunResult,
  ImProviderConfig,
} from "../../../../api/imGateway";
import LongTextModalField from "../agent/LongTextModalField";

const { Text } = Typography;

const DEFAULT_RUNNER: ExternalCliAgentSettings = {
  enabled: true,
  adapter: "codex",
  adapterConfig: {},
  injectBifrostTools: true,
  skillPaths: [],
  deliveryMode: "final_reply",
};

const ADAPTER_OPTIONS = [
  { label: "Codex CLI", value: "codex" },
  { label: "TreeX CLI", value: "traex" },
  { label: "ChatGPT Web", value: "chatgpt_web" },
];

const DELIVERY_OPTIONS = [
  { label: "Final reply", value: "final_reply" },
  { label: "Progress card", value: "progress_card" },
  { label: "No IM", value: "no_im" },
];

function SectionHeader({ description, children }: { description: string; children?: ReactNode }) {
  return (
    <div
      style={{
        display: "flex",
        justifyContent: "space-between",
        gap: 12,
        marginBottom: 12,
        alignItems: "center",
      }}
    >
      <Text type="secondary">{description}</Text>
      <Space>{children}</Space>
    </div>
  );
}

function lines(value: unknown): string[] {
  return String(value || "")
    .split("\n")
    .map((item) => item.trim())
    .filter(Boolean);
}

function runnerToFormValues(runner: ExternalCliAgentSettings) {
  return {
    enabled: runner.enabled,
    adapter: runner.adapter,
    executable: runner.adapterConfig?.executable,
    args: runner.adapterConfig?.args?.join("\n"),
    browserChannel: runner.adapterConfig?.browser?.channel || "edge",
    browserProfileDir: runner.adapterConfig?.browser?.profileDir,
    executionMode: runner.adapterConfig?.browser?.executionMode || "headed",
    chatgptBaseUrl: runner.adapterConfig?.chatgpt?.baseUrl || "https://chatgpt.com",
    pollIntervalMs: runner.adapterConfig?.chatgpt?.pollIntervalMs || 1000,
    chatgptTimeoutSecs:
      runner.adapterConfig?.timeoutSecs || runner.adapterConfig?.chatgpt?.timeoutSecs || 7200,
    permissionMode:
      runner.adapterConfig?.permissionMode && runner.adapterConfig.permissionMode !== "default"
        ? runner.adapterConfig.permissionMode
        : undefined,
    instructions: runner.instructions,
    skillPaths: runner.skillPaths?.join("\n"),
    injectBifrostTools: runner.injectBifrostTools,
    deliveryMode: runner.deliveryMode,
  };
}

function formToRunner(values: Record<string, unknown>): ExternalCliAgentSettings {
  const args = lines(values.args);
  const adapter = String(values.adapter || "codex").trim() || "codex";
  if (adapter === "chatgpt_web") {
    return {
      enabled: Boolean(values.enabled),
      adapter,
      instructions: String(values.instructions || "").trim() || undefined,
      adapterConfig: {
        timeoutSecs: Number(values.chatgptTimeoutSecs || 7200),
        browser: {
          channel: String(values.browserChannel || "edge"),
          ...(values.browserProfileDir
            ? { profileDir: String(values.browserProfileDir).trim() }
            : {}),
          openOnAuthRequired: true,
          executionMode: String(values.executionMode || "headed"),
          closeTargetAfterRun: true,
        },
        chatgpt: {
          baseUrl: String(values.chatgptBaseUrl || "https://chatgpt.com").trim(),
          pollIntervalMs: Number(values.pollIntervalMs || 1000),
          timeoutSecs: Number(values.chatgptTimeoutSecs || 7200),
        },
      },
      injectBifrostTools: false,
      skillPaths: [],
      deliveryMode:
        (values.deliveryMode as ExternalCliAgentSettings["deliveryMode"]) || "final_reply",
    };
  }
  return {
    enabled: Boolean(values.enabled),
    adapter,
    instructions: String(values.instructions || "").trim() || undefined,
    adapterConfig: {
      ...(values.executable ? { executable: String(values.executable).trim() } : {}),
      ...(args.length ? { args } : {}),
      ...(values.permissionMode && String(values.permissionMode).trim() !== "default"
        ? { permissionMode: String(values.permissionMode).trim() }
        : {}),
    },
    injectBifrostTools: values.injectBifrostTools !== false,
    skillPaths: lines(values.skillPaths),
    deliveryMode:
      (values.deliveryMode as ExternalCliAgentSettings["deliveryMode"]) || "final_reply",
  };
}

export default function ExternalCliPanel({
  providers,
  loading,
  onRefresh,
}: {
  providers: ImProviderConfig[];
  loading: boolean;
  onRefresh: () => void;
}) {
  const [config, setConfig] = useState<ExternalCliGatewayConfig | null>(null);
  const [selectedProviderId, setSelectedProviderId] = useState<string>();
  const [saving, setSaving] = useState(false);
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const [testing, setTesting] = useState(false);
  const [testMessage, setTestMessage] = useState("hello from WebUI");
  const [testResult, setTestResult] = useState<ExternalCliRunResult | null>(null);
  const [runnerModalOpen, setRunnerModalOpen] = useState(false);
  const [editingRunnerId, setEditingRunnerId] = useState<string | null>(null);
  const [authStatus, setAuthStatus] = useState<ChatGptWebAuthStatus | null>(null);
  const [authChecking, setAuthChecking] = useState(false);
  const [loginOpening, setLoginOpening] = useState(false);
  const [runnerForm] = Form.useForm();
  const [channelForm] = Form.useForm();
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const loginStopRequested = useRef(false);

  const runnerIds = useMemo(() => Object.keys(config?.runners || {}).sort(), [config?.runners]);
  const selectedOverride =
    selectedProviderId && config ? config.channels[selectedProviderId] : undefined;
  const effectiveRunnerId =
    selectedOverride?.runnerId || config?.defaultRunnerId || runnerIds[0] || "Codex";
  const effectiveRunner = config?.runners[effectiveRunnerId] || DEFAULT_RUNNER;

  const syncChannelForm = useCallback(
    (data: ExternalCliGatewayConfig, providerId?: string) => {
      const channel = providerId ? data.channels[providerId] || {} : {};
      channelForm.setFieldsValue({
        enabledMode:
          typeof channel.enabled === "boolean" ? String(channel.enabled) : "__inherit__",
        runnerId: channel.runnerId || "__inherit__",
        deliveryMode: channel.deliveryMode,
      });
    },
    [channelForm],
  );

  const fetchConfig = useCallback(async () => {
    try {
      const data = await imGatewayApi.getExternalCliConfig();
      setConfig(data);
      syncChannelForm(data, selectedProviderId);
    } catch (err) {
      message.error(normalizeApiErrorMessage(err, "Failed to load runners"));
    }
  }, [selectedProviderId, syncChannelForm]);

  useEffect(() => {
    void fetchConfig();
  }, [fetchConfig]);

  useEffect(() => {
    return () => {
      if (saveTimer.current) clearTimeout(saveTimer.current);
    };
  }, []);

  const saveConfig = async (next: ExternalCliGatewayConfig, showToast = false) => {
    setSaving(true);
    setSaveState("saving");
    try {
      const saved = await imGatewayApi.updateExternalCliConfig(next);
      setConfig(saved);
      syncChannelForm(saved, selectedProviderId);
      setSaveState("saved");
      if (showToast) message.success("Runners saved");
      onRefresh();
      return saved;
    } catch (err) {
      setSaveState("error");
      message.error(normalizeApiErrorMessage(err, "Failed to save runners"));
      return null;
    } finally {
      setSaving(false);
    }
  };

  const openCreateRunner = () => {
    setEditingRunnerId(null);
    setAuthStatus(null);
    runnerForm.resetFields();
    runnerForm.setFieldsValue({
      runnerId: "",
      ...runnerToFormValues({ ...DEFAULT_RUNNER, adapter: "codex" }),
    });
    setRunnerModalOpen(true);
  };

  const openEditRunner = (runnerId: string) => {
    if (!config) return;
    setEditingRunnerId(runnerId);
    setAuthStatus(null);
    runnerForm.resetFields();
    runnerForm.setFieldsValue({
      runnerId,
      ...runnerToFormValues(config.runners[runnerId] || DEFAULT_RUNNER),
    });
    setRunnerModalOpen(true);
  };

  const currentModalRunnerId = () =>
    String(runnerForm.getFieldValue("runnerId") || editingRunnerId || "")
      .trim()
      .replace(/\s+/g, "-");

  const handleCheckChatGptLogin = async () => {
    const runnerId = currentModalRunnerId();
    if (!runnerId) {
      message.error("Save or enter a Runner ID first");
      return;
    }
    setAuthChecking(true);
    try {
      const status = await imGatewayApi.getChatGptWebAuthStatus(runnerId);
      setAuthStatus(status);
      message[status.loggedIn ? "success" : "warning"](
        status.loggedIn ? "ChatGPT Web login is valid" : "ChatGPT Web login is required",
      );
    } catch (err) {
      message.error(normalizeApiErrorMessage(err, "Failed to check ChatGPT Web login"));
    } finally {
      setAuthChecking(false);
    }
  };

  const handleOpenChatGptLogin = async () => {
    const runnerId = currentModalRunnerId();
    if (!runnerId) {
      message.error("Save or enter a Runner ID first");
      return;
    }
    loginStopRequested.current = false;
    setLoginOpening(true);
    try {
      const status = await imGatewayApi.openChatGptWebLogin(runnerId);
      setAuthStatus(status);
      message[status.loggedIn ? "success" : "warning"](
        status.loggedIn ? "ChatGPT Web login captured" : "ChatGPT Web login is still required",
      );
    } catch (err) {
      if (!loginStopRequested.current) {
        message.error(normalizeApiErrorMessage(err, "Failed to open ChatGPT Web login"));
      }
    } finally {
      loginStopRequested.current = false;
      setLoginOpening(false);
    }
  };

  const handleStopChatGptLogin = async () => {
    const runnerId = currentModalRunnerId();
    if (!runnerId) {
      message.error("Save or enter a Runner ID first");
      return;
    }
    loginStopRequested.current = true;
    try {
      const status = await imGatewayApi.stopChatGptWebLogin(runnerId);
      setAuthStatus(status);
      message.info("ChatGPT Web login stopped");
    } catch (err) {
      message.error(normalizeApiErrorMessage(err, "Failed to stop ChatGPT Web login"));
    }
  };

  const handleSaveRunner = async () => {
    if (!config) return;
    try {
      const values = await runnerForm.validateFields();
      const runnerId = String(values.runnerId || "")
        .trim()
        .replace(/\s+/g, "-");
      if (!runnerId) {
        message.error("Runner ID is required");
        return;
      }
      if (!editingRunnerId && config.runners[runnerId]) {
        message.error(`Runner '${runnerId}' already exists`);
        return;
      }
      const runners = { ...config.runners };
      runners[runnerId] = formToRunner(values);
      const next = {
        ...config,
        defaultRunnerId: config.defaultRunnerId || runnerId,
        runners,
      };
      const saved = await saveConfig(next, true);
      if (saved) {
        setRunnerModalOpen(false);
        setEditingRunnerId(null);
      }
    } catch (err) {
      if (!(err && typeof err === "object" && "errorFields" in err)) {
        message.error(normalizeApiErrorMessage(err, "Failed to save runner"));
      }
    }
  };

  const handleDeleteRunner = (runnerId: string) => {
    if (!config || runnerIds.length <= 1) return;
    const runners = { ...config.runners };
    delete runners[runnerId];
    const fallbackRunnerId =
      config.defaultRunnerId === runnerId
        ? Object.keys(runners)[0] || "Codex"
        : config.defaultRunnerId;
    const channels = Object.fromEntries(
      Object.entries(config.channels).map(([providerId, channel]) => [
        providerId,
        channel.runnerId === runnerId ? { ...channel, runnerId: undefined } : channel,
      ]),
    );
    void saveConfig(
      {
        ...config,
        defaultRunnerId: fallbackRunnerId,
        runners,
        channels,
      },
      true,
    );
  };

  const buildChannelConfig = async (): Promise<ExternalCliGatewayConfig | null> => {
    if (!config || !selectedProviderId) return null;
    const values = await channelForm.validateFields();
    const override: ExternalCliChannelSettings = {};
    if (values.enabledMode === "true") override.enabled = true;
    if (values.enabledMode === "false") override.enabled = false;
    if (values.runnerId && values.runnerId !== "__inherit__") override.runnerId = values.runnerId;
    if (values.deliveryMode) override.deliveryMode = values.deliveryMode;

    const channels = { ...config.channels };
    if (Object.keys(override).length) channels[selectedProviderId] = override;
    else delete channels[selectedProviderId];
    return { ...config, channels };
  };

  const saveChannel = async (showToast: boolean) => {
    const next = await buildChannelConfig();
    if (!next) return;
    await saveConfig(next, showToast);
  };

  const scheduleChannelSave = () => {
    if (saveTimer.current) clearTimeout(saveTimer.current);
    setSaveState("idle");
    saveTimer.current = setTimeout(() => void saveChannel(false), 450);
  };

  const handleRunTest = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      const result = await imGatewayApi.runExternalCliChat({
        message: testMessage,
        providerId: selectedProviderId,
      });
      setTestResult(result);
      message.success("Runner test completed");
    } catch (err) {
      message.error(normalizeApiErrorMessage(err, "Runner test failed"));
    } finally {
      setTesting(false);
    }
  };

  return (
    <div>
      <SectionHeader description="Manage custom runners. Other modules can select Bifrost Agent or any configured runner. Working directory is inherited from IM Provider Agent settings, then global Agent settings.">
        <Button icon={<ReloadOutlined />} onClick={fetchConfig} loading={loading || saving}>
          Refresh
        </Button>
      </SectionHeader>
      <div style={{ display: "grid", gap: 12 }}>
        <Card
          size="small"
          title="Runners"
          extra={
            <Space>
              <Text type={saveState === "error" ? "danger" : "secondary"}>
                {saveState === "saving"
                  ? "Auto-saving..."
                  : saveState === "saved"
                    ? "Saved"
                    : saveState === "error"
                      ? "Save failed"
                      : "Changes auto-save"}
              </Text>
              <Button icon={<PlusOutlined />} onClick={openCreateRunner} type="primary">
                Add Runner
              </Button>
            </Space>
          }
        >
          <Space direction="vertical" style={{ width: "100%" }} size={12}>
            <Descriptions size="small" column={1}>
              <Descriptions.Item label="Working Directory">
                <Text type="secondary">
                  Inherited from IM Provider Agent settings, then global Agent settings
                </Text>
              </Descriptions.Item>
            </Descriptions>
            <Table
              rowKey="id"
              size="small"
              pagination={false}
              dataSource={runnerIds.map((id) => ({ id, runner: config?.runners[id] || DEFAULT_RUNNER }))}
              columns={[
                {
                  title: "Runner ID",
                  dataIndex: "id",
                  render: (id: string) => <Text code>{id}</Text>,
                },
                {
                  title: "Executor",
                  dataIndex: ["runner", "adapter"],
                  render: (adapter: string) => <Tag>{adapter}</Tag>,
                },
                {
                  title: "Enabled",
                  dataIndex: ["runner", "enabled"],
                  render: (enabled: boolean) => (
                    <Tag color={enabled ? "green" : "default"}>{enabled ? "enabled" : "disabled"}</Tag>
                  ),
                },
                {
                  title: "Delivery",
                  dataIndex: ["runner", "deliveryMode"],
                  render: (mode: string) => <Tag>{mode}</Tag>,
                },
                {
                  title: "Actions",
                  key: "actions",
                  align: "right" as const,
                  render: (_: unknown, row: { id: string }) => (
                    <Space>
                      <Button
                        size="small"
                        icon={<EditOutlined />}
                        onClick={() => openEditRunner(row.id)}
                      >
                        Edit
                      </Button>
                      <Popconfirm
                        title="Delete runner?"
                        okText="Delete"
                        okButtonProps={{ danger: true }}
                        disabled={runnerIds.length <= 1}
                        onConfirm={() => handleDeleteRunner(row.id)}
                      >
                        <Button
                          size="small"
                          danger
                          icon={<DeleteOutlined />}
                          disabled={runnerIds.length <= 1}
                        >
                          Delete
                        </Button>
                      </Popconfirm>
                    </Space>
                  ),
                },
              ]}
            />
          </Space>
        </Card>

        <Card size="small" title="IM Channel Runner and Test">
          <Space direction="vertical" style={{ width: "100%" }} size={12}>
            <Select
              allowClear
              style={{ width: "100%" }}
              placeholder="Select an IM provider/channel"
              value={selectedProviderId}
              onChange={(value) => {
                if (saveTimer.current) {
                  clearTimeout(saveTimer.current);
                  saveTimer.current = null;
                }
                setSelectedProviderId(value);
                if (config) {
                  syncChannelForm(config, value);
                }
              }}
              options={providers.map((provider) => ({
                label: `${provider.display_name || provider.id} (${provider.id})`,
                value: provider.id,
              }))}
            />
            <Descriptions size="small" column={1}>
              <Descriptions.Item label="Effective Runner">
                <Text code>{effectiveRunnerId}</Text>
              </Descriptions.Item>
              <Descriptions.Item label="Effective Adapter">
                <Text code>{effectiveRunner.adapter}</Text>
              </Descriptions.Item>
              <Descriptions.Item label="Delivery Mode">
                <Tag>{selectedOverride?.deliveryMode || effectiveRunner.deliveryMode}</Tag>
              </Descriptions.Item>
            </Descriptions>
            <Form form={channelForm} layout="vertical" onValuesChange={scheduleChannelSave}>
              <Form.Item name="runnerId" label="Runner Override">
                <Select
                  disabled={!selectedProviderId}
                  options={[
                    { label: "Inherit global runner", value: "__inherit__" },
                    ...runnerIds.map((id) => ({ label: id, value: id })),
                  ]}
                />
              </Form.Item>
              <Form.Item name="enabledMode" label="Enabled Override">
                <Select
                  disabled={!selectedProviderId}
                  options={[
                    { label: "Inherit runner setting", value: "__inherit__" },
                    { label: "Enabled for this channel", value: "true" },
                    { label: "Disabled for this channel", value: "false" },
                  ]}
                />
              </Form.Item>
              <Form.Item name="deliveryMode" label="Delivery Override">
                <Select allowClear disabled={!selectedProviderId} options={DELIVERY_OPTIONS} />
              </Form.Item>
              <Button
                type="primary"
                onClick={() => void saveChannel(true)}
                disabled={!selectedProviderId}
                loading={saving}
              >
                Save Channel
              </Button>
            </Form>
            <Input.TextArea
              rows={3}
              value={testMessage}
              onChange={(event) => setTestMessage(event.target.value)}
            />
            <Button
              icon={<RocketOutlined />}
              onClick={handleRunTest}
              loading={testing}
              disabled={!testMessage.trim()}
            >
              Run Test Message
            </Button>
            {testResult && (
              <Card size="small" title={`Run ${testResult.runId}`}>
                <Space direction="vertical" style={{ width: "100%" }}>
                  <Tag>{testResult.status}</Tag>
                  <Typography.Paragraph>{testResult.response}</Typography.Paragraph>
                  <Text type="secondary">{testResult.events.length} events captured</Text>
                </Space>
              </Card>
            )}
          </Space>
        </Card>
      </div>

      <Modal
        title={editingRunnerId ? `Edit Runner: ${editingRunnerId}` : "Add Runner"}
        open={runnerModalOpen}
        onCancel={() => setRunnerModalOpen(false)}
        onOk={handleSaveRunner}
        okText={editingRunnerId ? "Save Runner" : "Create Runner"}
        confirmLoading={saving}
        width={760}
        destroyOnClose
      >
        <Form form={runnerForm} layout="vertical">
          <Form.Item
            name="runnerId"
            label="Runner ID"
            rules={[
              { required: true, message: "Runner ID is required" },
              {
                pattern: /^[a-zA-Z0-9][a-zA-Z0-9._-]*$/,
                message: "Use letters, numbers, dot, underscore or dash",
              },
            ]}
          >
            <Input disabled={Boolean(editingRunnerId)} placeholder="Codex" />
          </Form.Item>
          <Form.Item name="enabled" label="Runner Enabled" valuePropName="checked">
            <Switch />
          </Form.Item>
          <Form.Item name="adapter" label="Adapter">
            <Select options={ADAPTER_OPTIONS} />
          </Form.Item>
          <Form.Item shouldUpdate noStyle>
            {({ getFieldValue }) =>
              getFieldValue("adapter") === "chatgpt_web" ? (
                <Space direction="vertical" style={{ width: "100%" }} size={12}>
                  <Descriptions size="small" column={1} bordered>
                    <Descriptions.Item label="Login State">
                      <Space wrap>
                        <Tag color={authStatus?.loggedIn ? "green" : "orange"}>
                          {authStatus?.state || "not checked"}
                        </Tag>
                        {authStatus && (
                          <Text type="secondary">
                            cookies {authStatus.cookieCount}, headers{" "}
                            {authStatus.capturedHeaderNames.length}
                          </Text>
                        )}
                      </Space>
                    </Descriptions.Item>
                    <Descriptions.Item label="Profile">
                      <Text type="secondary">{authStatus?.profileDir || "Bifrost data dir"}</Text>
                    </Descriptions.Item>
                  </Descriptions>
                  <Space>
                    <Button
                      icon={<ReloadOutlined />}
                      loading={authChecking}
                      onClick={handleCheckChatGptLogin}
                    >
                      Check Login
                    </Button>
                    <Button
                      icon={<LoginOutlined />}
                      loading={loginOpening}
                      onClick={handleOpenChatGptLogin}
                    >
                      Open Login Browser
                    </Button>
                    {loginOpening && (
                      <Button danger onClick={handleStopChatGptLogin}>
                        Stop Login
                      </Button>
                    )}
                  </Space>
                  <Form.Item name="browserChannel" label="Browser">
                    <Select
                      options={[
                        { label: "Microsoft Edge", value: "edge" },
                        { label: "Google Chrome", value: "chrome" },
                      ]}
                    />
                  </Form.Item>
                  <Form.Item name="browserProfileDir" label="Browser Profile Dir">
                    <Input placeholder="Leave empty to use Bifrost data dir" />
                  </Form.Item>
                  <Form.Item name="executionMode" label="Execution Mode">
                    <Select
                      options={[
                        { label: "Headless", value: "headless" },
                        { label: "Headed", value: "headed" },
                      ]}
                    />
                  </Form.Item>
                  <Form.Item name="chatgptBaseUrl" label="Base URL">
                    <Input placeholder="https://chatgpt.com" />
                  </Form.Item>
                  <Form.Item name="chatgptTimeoutSecs" label="Run Timeout Seconds">
                    <Input type="number" min={60} />
                  </Form.Item>
                  <Form.Item name="pollIntervalMs" label="Poll Interval Ms">
                    <Input type="number" min={250} />
                  </Form.Item>
                </Space>
              ) : (
                <>
                  <Form.Item name="executable" label="Executable">
                    <Input placeholder="Codex" />
                  </Form.Item>
                  <Form.Item name="args" label="Arguments">
                    <Input.TextArea rows={3} placeholder="One argument per line" />
                  </Form.Item>
                  <Form.Item name="permissionMode" label="Permission Mode">
                    <Select
                      allowClear
                      options={[
                        { label: "Headless default", value: "" },
                        { label: "Plan", value: "plan" },
                        { label: "Bypass Permissions", value: "bypass_permissions" },
                        { label: "Auto", value: "auto" },
                        { label: "Custom", value: "custom" },
                      ]}
                    />
                  </Form.Item>
                  <Form.Item name="skillPaths" label="Skill Paths">
                    <Input.TextArea rows={3} placeholder="Skill folder or SKILL.md path per line" />
                  </Form.Item>
                  <Form.Item
                    name="injectBifrostTools"
                    label="Inject Bifrost Tools"
                    valuePropName="checked"
                  >
                    <Switch />
                  </Form.Item>
                </>
              )
            }
          </Form.Item>
          <Form.Item name="deliveryMode" label="Default IM Delivery">
            <Select options={DELIVERY_OPTIONS} />
          </Form.Item>
          <Form.Item name="instructions">
            <LongTextModalField
              label="Runner Instructions"
              placeholder="Instructions injected before the IM message"
              testId="settings-im-external-cli-instructions"
            />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
