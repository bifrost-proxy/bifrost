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
import { useI18n, type TFunction } from "../../../../i18n";

const { Text } = Typography;

const DEFAULT_RUNNER: ExternalCliAgentSettings = {
  enabled: true,
  adapter: "codex",
  adapterConfig: {},
  injectBifrostTools: false,
  skillPaths: [],
  deliveryMode: "final_reply",
};

const ADAPTER_OPTIONS = [
  { label: "Codex CLI", value: "codex" },
  { label: "Traex CLI", value: "traex" },
  { label: "Claude Code", value: "claude_code" },
  { label: "ChatGPT Web", value: "chatgpt_web" },
];

function getDeliveryOptions(t: TFunction) {
  return [
    { label: t("imGateway.externalCli.deliveryFinalReply"), value: "final_reply" },
    { label: t("imGateway.externalCli.deliveryProgressCard"), value: "progress_card" },
    { label: t("imGateway.externalCli.deliveryNoIm"), value: "no_im" },
  ];
}

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
    injectBifrostTools: false,
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
  const { t } = useI18n();
  const deliveryOptions = useMemo(() => getDeliveryOptions(t), [t]);
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
      message.error(normalizeApiErrorMessage(err, t("imGateway.externalCli.loadFailed")));
    }
  }, [selectedProviderId, syncChannelForm, t]);

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
      if (showToast) message.success(t("imGateway.externalCli.saved"));
      onRefresh();
      return saved;
    } catch (err) {
      setSaveState("error");
      message.error(normalizeApiErrorMessage(err, t("imGateway.externalCli.saveFailed")));
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
      message.error(t("imGateway.externalCli.runnerIdFirst"));
      return;
    }
    setAuthChecking(true);
    try {
      const status = await imGatewayApi.getChatGptWebAuthStatus(runnerId);
      setAuthStatus(status);
      message[status.loggedIn ? "success" : "warning"](
        status.loggedIn ? t("imGateway.externalCli.loginValid") : t("imGateway.externalCli.loginRequired"),
      );
    } catch (err) {
      message.error(normalizeApiErrorMessage(err, t("imGateway.externalCli.checkLoginFailed")));
    } finally {
      setAuthChecking(false);
    }
  };

  const handleOpenChatGptLogin = async () => {
    const runnerId = currentModalRunnerId();
    if (!runnerId) {
      message.error(t("imGateway.externalCli.runnerIdFirst"));
      return;
    }
    loginStopRequested.current = false;
    setLoginOpening(true);
    try {
      const status = await imGatewayApi.openChatGptWebLogin(runnerId);
      setAuthStatus(status);
      message[status.loggedIn ? "success" : "warning"](
        status.loggedIn ? t("imGateway.externalCli.loginCaptured") : t("imGateway.externalCli.loginStillRequired"),
      );
    } catch (err) {
      if (!loginStopRequested.current) {
        message.error(normalizeApiErrorMessage(err, t("imGateway.externalCli.openLoginFailed")));
      }
    } finally {
      loginStopRequested.current = false;
      setLoginOpening(false);
    }
  };

  const handleStopChatGptLogin = async () => {
    const runnerId = currentModalRunnerId();
    if (!runnerId) {
      message.error(t("imGateway.externalCli.runnerIdFirst"));
      return;
    }
    loginStopRequested.current = true;
    try {
      const status = await imGatewayApi.stopChatGptWebLogin(runnerId);
      setAuthStatus(status);
      message.info(t("imGateway.externalCli.loginStopped"));
    } catch (err) {
      message.error(normalizeApiErrorMessage(err, t("imGateway.externalCli.stopLoginFailed")));
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
        message.error(t("imGateway.externalCli.runnerIdRequired"));
        return;
      }
      if (!editingRunnerId && config.runners[runnerId]) {
        message.error(t("imGateway.externalCli.runnerExists", { id: runnerId }));
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
        message.error(normalizeApiErrorMessage(err, t("imGateway.externalCli.saveRunnerFailed")));
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
      message.success(t("imGateway.externalCli.testCompleted"));
    } catch (err) {
      message.error(normalizeApiErrorMessage(err, t("imGateway.externalCli.testFailed")));
    } finally {
      setTesting(false);
    }
  };

  return (
    <div>
      <SectionHeader description={t("imGateway.externalCli.description")}>
        <Button icon={<ReloadOutlined />} onClick={fetchConfig} loading={loading || saving}>
          {t("common.refresh")}
        </Button>
      </SectionHeader>
      <div style={{ display: "grid", gap: 12 }}>
        <Card
          size="small"
          title={t("imGateway.externalCli.runners")}
          extra={
            <Space>
              <Text type={saveState === "error" ? "danger" : "secondary"}>
                {saveState === "saving"
                  ? t("imGateway.externalCli.autoSaving")
                  : saveState === "saved"
                    ? t("common.saved")
                    : saveState === "error"
                      ? t("imGateway.externalCli.saveStateFailed")
                      : t("imGateway.externalCli.changesAutoSave")}
              </Text>
              <Button icon={<PlusOutlined />} onClick={openCreateRunner} type="primary">
                {t("imGateway.externalCli.addRunner")}
              </Button>
            </Space>
          }
        >
          <Space direction="vertical" style={{ width: "100%" }} size={12}>
            <Descriptions size="small" column={1}>
              <Descriptions.Item label={t("imGateway.externalCli.workingDirectory")}>
                <Text type="secondary">
                  {t("imGateway.externalCli.workingDirectoryInherited")}
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
                  title: t("imGateway.externalCli.runnerId"),
                  dataIndex: "id",
                  render: (id: string) => <Text code>{id}</Text>,
                },
                {
                  title: t("imGateway.externalCli.executor"),
                  dataIndex: ["runner", "adapter"],
                  render: (adapter: string) => <Tag>{adapter}</Tag>,
                },
                {
                  title: t("common.enabled"),
                  dataIndex: ["runner", "enabled"],
                  render: (enabled: boolean) => (
                    <Tag color={enabled ? "green" : "default"}>{enabled ? t("imGateway.externalCli.stateEnabled") : t("imGateway.externalCli.stateDisabled")}</Tag>
                  ),
                },
                {
                  title: t("imGateway.externalCli.delivery"),
                  dataIndex: ["runner", "deliveryMode"],
                  render: (mode: string) => <Tag>{mode}</Tag>,
                },
                {
                  title: t("common.actions"),
                  key: "actions",
                  align: "right" as const,
                  render: (_: unknown, row: { id: string }) => (
                    <Space>
                      <Button
                        size="small"
                        icon={<EditOutlined />}
                        onClick={() => openEditRunner(row.id)}
                      >
                        {t("common.edit")}
                      </Button>
                      <Popconfirm
                        title={t("imGateway.externalCli.deleteConfirm")}
                        okText={t("common.delete")}
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
                          {t("common.delete")}
                        </Button>
                      </Popconfirm>
                    </Space>
                  ),
                },
              ]}
            />
          </Space>
        </Card>

        <Card size="small" title={t("imGateway.externalCli.channelCardTitle")}>
          <Space direction="vertical" style={{ width: "100%" }} size={12}>
            <Select
              allowClear
              style={{ width: "100%" }}
              placeholder={t("imGateway.externalCli.selectChannel")}
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
              <Descriptions.Item label={t("imGateway.externalCli.effectiveRunner")}>
                <Text code>{effectiveRunnerId}</Text>
              </Descriptions.Item>
              <Descriptions.Item label={t("imGateway.externalCli.effectiveAdapter")}>
                <Text code>{effectiveRunner.adapter}</Text>
              </Descriptions.Item>
              <Descriptions.Item label={t("imGateway.externalCli.deliveryMode")}>
                <Tag>{selectedOverride?.deliveryMode || effectiveRunner.deliveryMode}</Tag>
              </Descriptions.Item>
            </Descriptions>
            <Form form={channelForm} layout="vertical" onValuesChange={scheduleChannelSave}>
              <Form.Item name="runnerId" label={t("imGateway.externalCli.runnerOverride")}>
                <Select
                  disabled={!selectedProviderId}
                  options={[
                    { label: t("imGateway.externalCli.inheritGlobalRunner"), value: "__inherit__" },
                    ...runnerIds.map((id) => ({ label: id, value: id })),
                  ]}
                />
              </Form.Item>
              <Form.Item name="enabledMode" label={t("imGateway.externalCli.enabledOverride")}>
                <Select
                  disabled={!selectedProviderId}
                  options={[
                    { label: t("imGateway.externalCli.inheritRunnerSetting"), value: "__inherit__" },
                    { label: t("imGateway.externalCli.enabledForChannel"), value: "true" },
                    { label: t("imGateway.externalCli.disabledForChannel"), value: "false" },
                  ]}
                />
              </Form.Item>
              <Form.Item name="deliveryMode" label={t("imGateway.externalCli.deliveryOverride")}>
                <Select allowClear disabled={!selectedProviderId} options={deliveryOptions} />
              </Form.Item>
              <Button
                type="primary"
                onClick={() => void saveChannel(true)}
                disabled={!selectedProviderId}
                loading={saving}
              >
                {t("imGateway.externalCli.saveChannel")}
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
              {t("imGateway.externalCli.runTestMessage")}
            </Button>
            {testResult && (
              <Card size="small" title={t("imGateway.externalCli.runTitle", { id: testResult.runId })}>
                <Space direction="vertical" style={{ width: "100%" }}>
                  <Tag>{testResult.status}</Tag>
                  <Typography.Paragraph>{testResult.response}</Typography.Paragraph>
                  <Text type="secondary">{t("imGateway.externalCli.eventsCaptured", { count: testResult.events.length })}</Text>
                </Space>
              </Card>
            )}
          </Space>
        </Card>
      </div>

      <Modal
        title={editingRunnerId ? t("imGateway.externalCli.editRunnerTitle", { id: editingRunnerId }) : t("imGateway.externalCli.addRunner")}
        open={runnerModalOpen}
        onCancel={() => setRunnerModalOpen(false)}
        onOk={handleSaveRunner}
        okText={editingRunnerId ? t("imGateway.externalCli.saveRunner") : t("imGateway.externalCli.createRunner")}
        confirmLoading={saving}
        width={760}
        destroyOnClose
      >
        <Form form={runnerForm} layout="vertical">
          <Form.Item
            name="runnerId"
            label={t("imGateway.externalCli.runnerId")}
            rules={[
              { required: true, message: t("imGateway.externalCli.runnerIdRequired") },
              {
                pattern: /^[a-zA-Z0-9][a-zA-Z0-9._-]*$/,
                message: t("imGateway.externalCli.runnerIdPattern"),
              },
            ]}
          >
            <Input disabled={Boolean(editingRunnerId)} placeholder="Codex" />
          </Form.Item>
          <Form.Item name="enabled" label={t("imGateway.externalCli.runnerEnabled")} valuePropName="checked">
            <Switch />
          </Form.Item>
          <Form.Item name="adapter" label={t("imGateway.externalCli.adapter")}>
            <Select options={ADAPTER_OPTIONS} />
          </Form.Item>
          <Form.Item shouldUpdate noStyle>
            {({ getFieldValue }) =>
              getFieldValue("adapter") === "chatgpt_web" ? (
                <Space direction="vertical" style={{ width: "100%" }} size={12}>
                  <Descriptions size="small" column={1} bordered>
                    <Descriptions.Item label={t("imGateway.externalCli.loginState")}>
                      <Space wrap>
                        <Tag color={authStatus?.loggedIn ? "green" : "orange"}>
                          {authStatus?.state || t("imGateway.externalCli.notChecked")}
                        </Tag>
                        {authStatus && (
                          <Text type="secondary">
                            {t("imGateway.externalCli.cookiesHeaders", {
                              cookies: authStatus.cookieCount,
                              headers: authStatus.capturedHeaderNames.length,
                            })}
                          </Text>
                        )}
                      </Space>
                    </Descriptions.Item>
                    <Descriptions.Item label={t("imGateway.externalCli.profile")}>
                      <Text type="secondary">{authStatus?.profileDir || t("imGateway.externalCli.bifrostDataDir")}</Text>
                    </Descriptions.Item>
                  </Descriptions>
                  <Space>
                    <Button
                      icon={<ReloadOutlined />}
                      loading={authChecking}
                      onClick={handleCheckChatGptLogin}
                    >
                      {t("imGateway.externalCli.checkLogin")}
                    </Button>
                    <Button
                      icon={<LoginOutlined />}
                      loading={loginOpening}
                      onClick={handleOpenChatGptLogin}
                    >
                      {t("imGateway.externalCli.openLoginBrowser")}
                    </Button>
                    {loginOpening && (
                      <Button danger onClick={handleStopChatGptLogin}>
                        {t("imGateway.externalCli.stopLogin")}
                      </Button>
                    )}
                  </Space>
                  <Form.Item name="browserChannel" label={t("imGateway.externalCli.browser")}>
                    <Select
                      options={[
                        { label: "Microsoft Edge", value: "edge" },
                        { label: "Google Chrome", value: "chrome" },
                      ]}
                    />
                  </Form.Item>
                  <Form.Item name="browserProfileDir" label={t("imGateway.externalCli.browserProfileDir")}>
                    <Input placeholder={t("imGateway.externalCli.browserProfileDirPlaceholder")} />
                  </Form.Item>
                  <Form.Item name="executionMode" label={t("imGateway.externalCli.executionMode")}>
                    <Select
                      options={[
                        { label: t("imGateway.externalCli.headless"), value: "headless" },
                        { label: t("imGateway.externalCli.headed"), value: "headed" },
                      ]}
                    />
                  </Form.Item>
                  <Form.Item name="chatgptBaseUrl" label={t("imGateway.externalCli.baseUrl")}>
                    <Input placeholder="https://chatgpt.com" />
                  </Form.Item>
                  <Form.Item name="chatgptTimeoutSecs" label={t("imGateway.externalCli.runTimeoutSeconds")}>
                    <Input type="number" min={60} />
                  </Form.Item>
                  <Form.Item name="pollIntervalMs" label={t("imGateway.externalCli.pollIntervalMs")}>
                    <Input type="number" min={250} />
                  </Form.Item>
                </Space>
              ) : (
                <>
                  <Form.Item name="executable" label={t("imGateway.externalCli.executable")}>
                    <Input placeholder="Codex" />
                  </Form.Item>
                  <Form.Item name="args" label={t("imGateway.externalCli.arguments")}>
                    <Input.TextArea rows={3} placeholder={t("imGateway.externalCli.argumentsPlaceholder")} />
                  </Form.Item>
                  <Form.Item name="permissionMode" label={t("imGateway.externalCli.permissionMode")}>
                    <Select
                      allowClear
                      options={[
                        { label: t("imGateway.externalCli.permHeadlessDefault"), value: "" },
                        { label: t("imGateway.externalCli.permPlan"), value: "plan" },
                        { label: t("imGateway.externalCli.permBypass"), value: "bypass_permissions" },
                        { label: t("imGateway.externalCli.permAuto"), value: "auto" },
                        { label: t("imGateway.externalCli.permCustom"), value: "custom" },
                      ]}
                    />
                  </Form.Item>
                  <Form.Item name="skillPaths" label={t("imGateway.externalCli.skillPaths")}>
                    <Input.TextArea rows={3} placeholder={t("imGateway.externalCli.skillPathsPlaceholder")} />
                  </Form.Item>
                </>
              )
            }
          </Form.Item>
          <Form.Item name="deliveryMode" label={t("imGateway.externalCli.defaultImDelivery")}>
            <Select options={deliveryOptions} />
          </Form.Item>
          <Form.Item name="instructions">
            <LongTextModalField
              label={t("imGateway.externalCli.runnerInstructions")}
              placeholder={t("imGateway.externalCli.runnerInstructionsPlaceholder")}
              description={t("imGateway.externalCli.runnerInstructionsDescription")}
              testId="settings-im-external-cli-instructions"
            />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
