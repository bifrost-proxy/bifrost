import { useCallback, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { useSearchParams } from "react-router-dom";
import {
  Alert,
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
  QRCode,
  Select,
  Space,
  Spin,
  Steps,
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
  CopyOutlined,
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
  ExternalCliGatewayConfig,
} from "../../../api/imGateway";
import type { AgentConfig } from "./agent/types";
import LongTextModalField from "./agent/LongTextModalField";
import {
  IM_GATEWAY_SECTION_NAV as IM_GATEWAY_SECTION_LABELS,
  type ImGatewaySectionId,
} from "./aiSections";

const { Text } = Typography;
const { useBreakpoint } = Grid;

const AGENT_RUNNER_OPTIONS = [
  { label: "Inherit global default", value: "__inherit__" },
];

type ProviderCreateStep = 0 | 1 | 2;
type SetupProviderType = "weixin" | "feishu";

function defaultProviderId(type: SetupProviderType): string {
  return type === "weixin" ? "weixin-main" : "feishu-main";
}

function nextProviderId(type: SetupProviderType, providers: ImProviderConfig[]): string {
  const base = defaultProviderId(type);
  const used = new Set(providers.map((provider) => provider.id));
  if (!used.has(base)) return base;
  let index = 2;
  while (used.has(`${base}${index}`)) {
    index += 1;
  }
  return `${base}${index}`;
}

function OverflowText({
  value,
  code,
  width = 240,
}: {
  value?: string;
  code?: boolean;
  width?: number | string;
}) {
  const { token } = theme.useToken();
  const [tip, setTip] = useState<{ x: number; y: number } | null>(null);
  const text = value?.trim() || "-";
  const canShowTip = text !== "-";
  const maxWidth = typeof width === "number" ? `${width}px` : width;

  return (
    <>
      <span
        onMouseEnter={(event) => {
          if (!canShowTip) return;
          const rect = event.currentTarget.getBoundingClientRect();
          const maxWidth = 520;
          setTip({
            x: Math.max(8, Math.min(rect.left, window.innerWidth - maxWidth - 8)),
            y: rect.bottom + 8,
          });
        }}
        onMouseLeave={() => setTip(null)}
        style={{
          display: "block",
          width: "100%",
          maxWidth,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          verticalAlign: "bottom",
        }}
      >
        {code ? (
          <Text code style={{ whiteSpace: "nowrap" }}>
            {text}
          </Text>
        ) : (
          text
        )}
      </span>
      {canShowTip && tip && (
        <div
          className="im-gateway-overflow-tooltip"
          role="tooltip"
          style={{
            position: "fixed",
            left: tip.x,
            top: tip.y,
            zIndex: 3000,
            maxWidth: 520,
            maxHeight: 320,
            overflow: "auto",
            padding: "8px 10px",
            borderRadius: token.borderRadius,
            background: token.colorBgElevated,
            boxShadow: token.boxShadowSecondary,
            color: token.colorText,
            fontSize: token.fontSizeSM,
            lineHeight: 1.5,
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
          }}
        >
          {text}
        </div>
      )}
    </>
  );
}

function CopyableOverflowText({
  value,
  displayValue,
  code,
  width = "100%",
  testId,
}: {
  value?: string;
  displayValue?: string;
  code?: boolean;
  width?: number | string;
  testId?: string;
}) {
  const [hovered, setHovered] = useState(false);
  const copyValue = value?.trim();
  const canCopy = Boolean(copyValue);

  const handleCopy = async () => {
    if (!copyValue) return;
    try {
      await navigator.clipboard.writeText(copyValue);
      message.success("Copied");
    } catch {
      message.error("Copy failed");
    }
  };

  return (
    <span
      data-testid={testId}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        minWidth: 0,
        maxWidth: width,
      }}
    >
      <span style={{ flex: "1 1 auto", minWidth: 0 }}>
        <OverflowText value={displayValue ?? value} code={code} width="100%" />
      </span>
      {canCopy && (
        <Tooltip title="Copy">
          <Button
            aria-label="Copy"
            type="text"
            size="small"
            icon={<CopyOutlined />}
            onClick={handleCopy}
            onFocus={() => setHovered(true)}
            onBlur={() => setHovered(false)}
            data-testid={testId ? `${testId}-copy` : undefined}
            style={{
              flex: "0 0 auto",
              opacity: hovered ? 1 : 0,
              pointerEvents: hovered ? "auto" : "none",
              transition: "opacity 0.15s ease",
            }}
          />
        </Tooltip>
      )}
    </span>
  );
}

function SecondaryInline({ children }: { children: ReactNode }) {
  return (
    <Text type="secondary" style={{ whiteSpace: "nowrap" }}>
      {children}
    </Text>
  );
}

function ProviderFieldList({
  items,
}: {
  items: Array<{
    label: string;
    value: ReactNode;
  }>;
}) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 10, minWidth: 0 }}>
      {items.map((item) => (
        <div
          key={item.label}
          style={{
            display: "flex",
            alignItems: "baseline",
            gap: 8,
            minWidth: 0,
          }}
        >
          <Text
            type="secondary"
            style={{
              flex: "0 0 150px",
              maxWidth: "42%",
              whiteSpace: "nowrap",
            }}
          >
            {item.label}:
          </Text>
          <div style={{ flex: "1 1 auto", minWidth: 0 }}>{item.value}</div>
        </div>
      ))}
    </div>
  );
}

function ResponsiveCardGrid({ children }: { children: ReactNode }) {
  return (
    <div
      data-testid="settings-im-card-grid"
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fit, minmax(min(100%, 340px), 1fr))",
        gap: 12,
        width: "100%",
      }}
    >
      {children}
    </div>
  );
}

function PanelHeader({
  description,
  children,
}: {
  description: ReactNode;
  children: ReactNode;
}) {
  return (
    <div
      style={{
        display: "flex",
        justifyContent: "space-between",
        alignItems: "center",
        gap: 12,
        marginBottom: 16,
        flexWrap: "wrap",
      }}
    >
      <Text type="secondary" style={{ flex: "1 1 220px" }}>
        {description}
      </Text>
      <Space wrap>{children}</Space>
    </div>
  );
}

function DetailGrid({
  items,
  minColumnWidth = 320,
}: {
  items: Array<{
    label: string;
    value: ReactNode;
  }>;
  minColumnWidth?: number;
}) {
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: `repeat(auto-fit, minmax(min(100%, ${minColumnWidth}px), 1fr))`,
        gap: "14px 24px",
        minWidth: 0,
      }}
    >
      {items.map((item) => (
        <div
          key={item.label}
          style={{
            display: "flex",
            gap: 8,
            alignItems: "baseline",
            minWidth: 0,
            overflow: "hidden",
          }}
        >
          <Text type="secondary" style={{ flex: "0 0 auto", whiteSpace: "nowrap" }}>
            {item.label}:
          </Text>
          <div style={{ flex: "1 1 auto", minWidth: 0, overflow: "hidden" }}>
            {item.value}
          </div>
        </div>
      ))}
    </div>
  );
}

const IM_GATEWAY_SECTION_NAV: Array<{
  id: ImGatewaySectionId;
  label: string;
  icon: ReactNode;
}> = [
  { ...IM_GATEWAY_SECTION_LABELS[0], icon: <ApiOutlined /> },
  { ...IM_GATEWAY_SECTION_LABELS[1], icon: <SendOutlined /> },
  { ...IM_GATEWAY_SECTION_LABELS[2], icon: <RocketOutlined /> },
  { ...IM_GATEWAY_SECTION_LABELS[3], icon: <CloudOutlined /> },
  { ...IM_GATEWAY_SECTION_LABELS[4], icon: <HistoryOutlined /> },
];

// ─── Connections Panel ───────────────────────────────────────────────────────

function ConnectionsPanel({
  providers,
  loading,
  onRefresh,
  cardGrid = false,
}: {
  providers: ImProviderConfig[];
  loading: boolean;
  onRefresh: () => void;
  cardGrid?: boolean;
}) {
  const { token } = theme.useToken();
  const [statusMap, setStatusMap] = useState<
    Record<string, ConnectionStatus>
  >({});
  const [addModalOpen, setAddModalOpen] = useState(false);
  const [editingProvider, setEditingProvider] =
    useState<ImProviderConfig | null>(null);
  const [createStep, setCreateStep] = useState<ProviderCreateStep>(0);
  const [setupProviderType, setSetupProviderType] =
    useState<SetupProviderType>("weixin");
  const [createdProvider, setCreatedProvider] = useState<ImProviderConfig | null>(null);
  const [feishuSetup, setFeishuSetup] = useState<{
    sessionId: string;
    verificationUrl: string;
    expiresAt: number;
    intervalSeconds: number;
    status: "pending" | "expired" | "confirmed" | "error";
    appId?: string;
    ownerOpenId?: string;
    baseUrl?: string;
    errorMessage?: string;
  } | null>(null);
  const [feishuSetupLoading, setFeishuSetupLoading] = useState(false);
  const [feishuAutoConnecting, setFeishuAutoConnecting] = useState(false);
  const [feishuAutoConnectAttemptedSessionId, setFeishuAutoConnectAttemptedSessionId] =
    useState<string | null>(null);
  const [weixinLogin, setWeixinLogin] = useState<{
    providerId: string;
    scanUrl: string;
    expiresInSeconds: number;
    expiresAt: number;
    status: "pending" | "expired" | "checking";
  } | null>(null);
  const [weixinLoginLoading, setWeixinLoginLoading] = useState(false);
  const [autoPromptedWeixinIds, setAutoPromptedWeixinIds] = useState<Set<string>>(
    () => new Set(),
  );
  const [agentDefaults, setAgentDefaults] = useState<AgentConfig | null>(null);
  const [externalCliConfig, setExternalCliConfig] =
    useState<ExternalCliGatewayConfig | null>(null);
  const [form] = Form.useForm();
  const selectedProviderType =
    (Form.useWatch("provider_type", form) as string | undefined) ||
    editingProvider?.provider_type ||
    setupProviderType ||
    "feishu";

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

  const fetchExternalCliConfig = useCallback(async () => {
    try {
      setExternalCliConfig(await imGatewayApi.getExternalCliConfig());
    } catch {
      setExternalCliConfig(null);
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
      void fetchExternalCliConfig();
    }, 0);
    return () => window.clearTimeout(timer);
  }, [fetchAgentDefaults, fetchExternalCliConfig]);

  const inheritedWorkDir =
    agentDefaults?.work_dir || agentDefaults?.resolved_work_dir || "Process working directory";
  const inheritedBaseInstructions =
    agentDefaults?.base_instructions ||
    agentDefaults?.default_base_instructions ||
    "Built-in default Agent prompt";
  const inheritedDeveloperInstructions =
    agentDefaults?.developer_instructions || "No global developer instructions";
  const inheritedUserInstructions =
    agentDefaults?.user_instructions || "No global user instructions";
  const runnerIds = useMemo(
    () => Object.keys(externalCliConfig?.runners || {}).sort(),
    [externalCliConfig?.runners],
  );
  const agentRunnerOptions = useMemo(
    () => [
      ...AGENT_RUNNER_OPTIONS,
      ...runnerIds.map((id) => ({ label: id, value: id })),
    ],
    [runnerIds],
  );
  const inheritedRunner = agentDefaults?.runner || externalCliConfig?.defaultRunnerId || "Codex";

  const displayRunnerLabel = (provider: ImProviderConfig) => {
    if (provider.agent_config?.runner) return provider.agent_config.runner;
    if (externalCliConfig?.channels[provider.id]?.runnerId) {
      return externalCliConfig.channels[provider.id].runnerId;
    }
    return undefined;
  };

  const providerRunnerFormValue = useCallback((provider: ImProviderConfig) => {
    if (provider.agent_config?.runner) return provider.agent_config.runner;
    if (externalCliConfig?.channels[provider.id]?.runnerId) {
      return externalCliConfig.channels[provider.id].runnerId;
    }
    return "__inherit__";
  }, [externalCliConfig?.channels]);

  const normalizeProviderValues = (
    values: Record<string, unknown>,
    clearEmptyAgentConfig: boolean,
  ) => {
    const payload: Record<string, unknown> = { ...values };
    const agentConfig = values.agent_config as
      | {
          work_dir?: string;
          runner?: string;
          base_instructions?: string;
          developer_instructions?: string;
          user_instructions?: string;
        }
      | undefined;
    const workDir = agentConfig?.work_dir?.trim();
    const runnerValue = agentConfig?.runner;
    const runner = runnerValue === "__inherit__" ? undefined : runnerValue;
    const baseInstructions = agentConfig?.base_instructions?.trim();
    const developerInstructions = agentConfig?.developer_instructions?.trim();
    const userInstructions = agentConfig?.user_instructions?.trim();
    if (clearEmptyAgentConfig) {
      if (runner || workDir || baseInstructions || developerInstructions || userInstructions) {
        payload.agent_config = {
          runner: runner || null,
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
    if (runner || workDir || baseInstructions || developerInstructions || userInstructions) {
      payload.agent_config = {
        ...(runner ? { runner } : {}),
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

  const syncProviderRunnerOverride = async (providerId: string, runnerValue?: string) => {
    if (!externalCliConfig) return;
    const channels = { ...externalCliConfig.channels };
    const current = channels[providerId] || {};
    if (runnerValue && runnerValue !== "__inherit__") {
      channels[providerId] = {
        ...current,
        runnerId: runnerValue,
      };
    } else if (channels[providerId]?.runnerId) {
      const next = { ...channels[providerId] };
      delete next.runnerId;
      if (next.enabled === undefined && next.deliveryMode === undefined) {
        delete channels[providerId];
      } else {
        channels[providerId] = next;
      }
    } else {
      return;
    }
    const saved = await imGatewayApi.updateExternalCliConfig({
      ...externalCliConfig,
      channels,
    });
    setExternalCliConfig(saved);
  };

  const openAddModal = () => {
    setEditingProvider(null);
    setCreateStep(0);
    setSetupProviderType("weixin");
    setCreatedProvider(null);
    setFeishuSetup(null);
    setFeishuAutoConnectAttemptedSessionId(null);
    setFeishuAutoConnecting(false);
    form.resetFields();
    form.setFieldsValue({
      provider_type: "weixin",
      enabled: true,
      event_connection_enabled: true,
      agent_config: {
        runner: "__inherit__",
      },
    });
    setAddModalOpen(true);
  };

  const openEditModal = (provider: ImProviderConfig) => {
    setEditingProvider(provider);
    setCreateStep(2);
    setCreatedProvider(null);
    setFeishuSetup(null);
    setFeishuAutoConnectAttemptedSessionId(null);
    setFeishuAutoConnecting(false);
    form.setFieldsValue({
      ...provider,
      app_secret: undefined,
      agent_config: {
        runner: providerRunnerFormValue(provider),
        work_dir: provider.agent_config?.work_dir,
        base_instructions: provider.agent_config?.base_instructions,
        developer_instructions: provider.agent_config?.developer_instructions,
        user_instructions: provider.agent_config?.user_instructions,
      },
    });
    setAddModalOpen(true);
  };

  const startFeishuSetupSession = async () => {
    try {
      setFeishuSetupLoading(true);
      setFeishuSetup(null);
      setFeishuAutoConnectAttemptedSessionId(null);
      const result = await imGatewayApi.startFeishuSetup({ brand: "feishu" });
      setFeishuSetup({
        sessionId: result.session_id,
        verificationUrl: result.verification_url,
        expiresAt: result.expires_at,
        intervalSeconds: result.interval_seconds,
        status: "pending",
      });
    } catch (err) {
      const errorMessage = normalizeApiErrorMessage(err, "Failed to start Feishu setup");
      setFeishuSetup({
        sessionId: "",
        verificationUrl: "",
        expiresAt: Date.now(),
        intervalSeconds: 5,
        status: "error",
        errorMessage,
      });
      setFeishuAutoConnectAttemptedSessionId(null);
      message.error(errorMessage);
    } finally {
      setFeishuSetupLoading(false);
    }
  };

  const beginCreateConnectionStep = async () => {
    const nextType = setupProviderType;
    form.resetFields();
    form.setFieldsValue({
      id: nextProviderId(nextType, providers),
      provider_type: nextType,
      enabled: true,
      event_connection_enabled: true,
      agent_config: {
        runner: "__inherit__",
      },
    });
    setCreatedProvider(null);
    setCreateStep(1);
    if (nextType === "feishu") {
      await startFeishuSetupSession();
    }
  };

  const startWeixinProviderSetup = async () => {
    try {
      const values = await form.validateFields(["id", "display_name"]);
      await imGatewayApi.createProvider({
        id: values.id,
        provider_type: "weixin",
        display_name: values.display_name,
        enabled: true,
        event_connection_enabled: true,
        event_types: ["message.receive"],
      });
      const provider = await imGatewayApi.getProvider(values.id);
      const result = await imGatewayApi.startWeixinLogin(values.id);
      setCreatedProvider(provider);
      setWeixinLogin({
        providerId: values.id,
        scanUrl: result.scan_url,
        expiresInSeconds: result.expires_in_seconds,
        expiresAt: Date.now() + result.expires_in_seconds * 1000,
        status: "pending",
      });
      setAutoPromptedWeixinIds((prev) => new Set(prev).add(values.id));
      message.success("Provider created. Scan the Weixin QR code to continue.");
      onRefresh();
    } catch (err) {
      if (err && typeof err === "object" && "errorFields" in err) return;
      message.error(normalizeApiErrorMessage(err, "Failed to start Weixin setup"));
    }
  };

  const createConnectedFeishuProvider = useCallback(async () => {
    if (!feishuSetup || feishuSetup.status !== "confirmed") return;
    try {
      setFeishuAutoConnecting(true);
      const values = await form.validateFields(["id", "display_name"]);
      const result = await imGatewayApi.createFeishuSetupProvider(feishuSetup.sessionId, {
        id: values.id,
        provider_type: "feishu",
        display_name: values.display_name,
        enabled: true,
        event_connection_enabled: true,
        event_types: ["message.receive"],
      });
      await imGatewayApi.connectProvider(result.provider.id);
      const provider = await imGatewayApi.getProvider(result.provider.id);
      setCreatedProvider(provider);
      form.setFieldsValue({
        ...provider,
        app_secret: undefined,
        agent_config: {
          runner: providerRunnerFormValue(provider),
          work_dir: provider.agent_config?.work_dir,
          base_instructions: provider.agent_config?.base_instructions,
          developer_instructions: provider.agent_config?.developer_instructions,
          user_instructions: provider.agent_config?.user_instructions,
        },
      });
      setCreateStep(2);
      message.success("Feishu provider connected. Finish the optional configuration.");
      onRefresh();
      void fetchStatuses();
    } catch (err) {
      if (err && typeof err === "object" && "errorFields" in err) return;
      message.error(normalizeApiErrorMessage(err, "Failed to create Feishu provider"));
    } finally {
      setFeishuAutoConnecting(false);
    }
  }, [feishuSetup, fetchStatuses, form, onRefresh, providerRunnerFormValue]);

  const handleSaveProvider = async () => {
    try {
      if (!editingProvider && createStep === 0) {
        await beginCreateConnectionStep();
        return;
      }
      if (!editingProvider && createStep === 1) {
        if (setupProviderType === "weixin") {
          await startWeixinProviderSetup();
        } else {
          await createConnectedFeishuProvider();
        }
        return;
      }
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
        await syncProviderRunnerOverride(editingProvider.id, values.agent_config?.runner);
        message.success("Provider updated");
      } else if (createdProvider) {
        const payload = normalizeProviderValues(
          {
            display_name: values.display_name,
            enabled: values.enabled,
            owner_open_id: values.owner_open_id,
            agent_config: values.agent_config,
          },
          true,
        );
        await imGatewayApi.updateProvider(
          createdProvider.id,
          payload as Partial<ImProviderConfig>,
        );
        await syncProviderRunnerOverride(createdProvider.id, values.agent_config?.runner);
        message.success("Provider configured");
      } else {
        const appSecret =
          typeof values.app_secret === "string" ? values.app_secret.trim() : "";
        await imGatewayApi.createProvider(
          normalizeProviderValues(values, false) as Partial<ImProviderConfig>,
        );
        await syncProviderRunnerOverride(values.id, values.agent_config?.runner);
        if (values.provider_type === "weixin") {
          const result = await imGatewayApi.startWeixinLogin(values.id);
          setWeixinLogin({
            providerId: values.id,
            scanUrl: result.scan_url,
            expiresInSeconds: result.expires_in_seconds,
            expiresAt: Date.now() + result.expires_in_seconds * 1000,
            status: "pending",
          });
          setAutoPromptedWeixinIds((prev) => new Set(prev).add(values.id));
          message.success("Provider created. Scan the Weixin QR code to finish login.");
        } else if (values.enabled && values.event_connection_enabled && appSecret) {
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
      void fetchExternalCliConfig();
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

  const handleStartWeixinLogin = async (provider: ImProviderConfig) => {
    try {
      setWeixinLoginLoading(true);
      const result = await imGatewayApi.startWeixinLogin(provider.id);
      setWeixinLogin({
        providerId: provider.id,
        scanUrl: result.scan_url,
        expiresInSeconds: result.expires_in_seconds,
        expiresAt: Date.now() + result.expires_in_seconds * 1000,
        status: "pending",
      });
      message.success("Weixin QR code generated");
    } catch (err) {
      message.error(normalizeApiErrorMessage(err, "Failed to start Weixin login"));
    } finally {
      setWeixinLoginLoading(false);
    }
  };

  const refreshWeixinLogin = useCallback(
    async (providerId: string, showToast = false) => {
      setWeixinLoginLoading(true);
      try {
        const result = await imGatewayApi.startWeixinLogin(providerId);
        setWeixinLogin({
          providerId,
          scanUrl: result.scan_url,
          expiresInSeconds: result.expires_in_seconds,
          expiresAt: Date.now() + result.expires_in_seconds * 1000,
          status: "pending",
        });
        setAutoPromptedWeixinIds((prev) => new Set(prev).add(providerId));
        if (showToast) {
          message.success("Weixin QR code refreshed");
        }
      } catch (err) {
        message.error(normalizeApiErrorMessage(err, "Failed to refresh Weixin QR"));
      } finally {
        setWeixinLoginLoading(false);
      }
    },
    [],
  );

  useEffect(() => {
    if (!weixinLogin || weixinLoginLoading) return;
    const timer = window.setInterval(async () => {
      if (!weixinLogin) return;
      const msRemaining = weixinLogin.expiresAt - Date.now();
      if (msRemaining <= 0) {
        setWeixinLogin((current) =>
          current?.providerId === weixinLogin.providerId
            ? { ...current, status: "expired" }
            : current,
        );
        await refreshWeixinLogin(weixinLogin.providerId);
        return;
      }
      if (msRemaining <= 10_000) {
        await refreshWeixinLogin(weixinLogin.providerId);
        return;
      }
      try {
        setWeixinLogin((current) =>
          current?.providerId === weixinLogin.providerId
            ? { ...current, status: "checking" }
            : current,
        );
        const status = await imGatewayApi.getWeixinLoginStatus(weixinLogin.providerId);
        if (status.status === "confirmed" || status.status === "authorized") {
          await imGatewayApi.connectProvider(weixinLogin.providerId);
          const provider = status.provider || (await imGatewayApi.getProvider(weixinLogin.providerId));
          setCreatedProvider(provider);
          form.setFieldsValue({
            ...provider,
            app_secret: undefined,
            agent_config: {
              runner: providerRunnerFormValue(provider),
              work_dir: provider.agent_config?.work_dir,
              base_instructions: provider.agent_config?.base_instructions,
              developer_instructions: provider.agent_config?.developer_instructions,
              user_instructions: provider.agent_config?.user_instructions,
            },
          });
          setCreateStep(2);
          setWeixinLogin(null);
          message.success("Weixin login completed. Finish the optional configuration.");
          onRefresh();
          void fetchStatuses();
          return;
        }
        if (status.status === "expired") {
          setWeixinLogin((current) =>
            current?.providerId === weixinLogin.providerId
              ? { ...current, status: "expired" }
              : current,
          );
          await refreshWeixinLogin(weixinLogin.providerId);
          return;
        }
        setWeixinLogin((current) =>
          current?.providerId === weixinLogin.providerId
            ? { ...current, status: "pending" }
            : current,
        );
      } catch {
        setWeixinLogin((current) =>
          current?.providerId === weixinLogin.providerId
            ? { ...current, status: "pending" }
            : current,
        );
      }
    }, 2000);
    return () => window.clearInterval(timer);
  }, [
    fetchStatuses,
    form,
    onRefresh,
    providerRunnerFormValue,
    refreshWeixinLogin,
    weixinLogin,
    weixinLoginLoading,
  ]);

  useEffect(() => {
    if (!feishuSetup || feishuSetup.status !== "pending" || feishuSetupLoading) return;
    const delay = Math.max(2, feishuSetup.intervalSeconds || 5) * 1000;
    const timer = window.setInterval(async () => {
      try {
        const result = await imGatewayApi.getFeishuSetupStatus(feishuSetup.sessionId);
        if (result.status === "confirmed") {
          setFeishuSetup((current) =>
            current?.sessionId === feishuSetup.sessionId
              ? {
                  ...current,
                  status: "confirmed",
                  appId: result.app_id,
                  ownerOpenId: result.owner_open_id,
                  baseUrl: result.base_url,
                }
              : current,
          );
          message.success("Feishu app created. Connecting provider...");
          return;
        }
        if (result.status === "expired") {
          setFeishuSetup((current) =>
            current?.sessionId === feishuSetup.sessionId
              ? { ...current, status: "expired" }
              : current,
          );
        }
      } catch {
        // keep polling; transient network errors are common while the user is in the browser
      }
    }, delay);
    return () => window.clearInterval(timer);
  }, [feishuSetup, feishuSetupLoading]);

  useEffect(() => {
    if (
      editingProvider ||
      createStep !== 1 ||
      setupProviderType !== "feishu" ||
      !feishuSetup ||
      feishuSetup.status !== "confirmed" ||
      createdProvider ||
      feishuAutoConnecting ||
      feishuAutoConnectAttemptedSessionId === feishuSetup.sessionId
    ) {
      return;
    }
    setFeishuAutoConnectAttemptedSessionId(feishuSetup.sessionId);
    void createConnectedFeishuProvider();
  }, [
    createStep,
    createdProvider,
    editingProvider,
    feishuAutoConnectAttemptedSessionId,
    feishuAutoConnecting,
    feishuSetup,
    createConnectedFeishuProvider,
    setupProviderType,
  ]);

  useEffect(() => {
    if (weixinLogin || weixinLoginLoading) return;
    const provider = providers.find((p) => {
      if (p.provider_type !== "weixin" || !p.enabled) return false;
      if (autoPromptedWeixinIds.has(p.id)) return false;
      const status = statusMap[p.id]?.state;
      return !p.secret_configured || status === "failed";
    });
    if (!provider) return;
    void refreshWeixinLogin(provider.id);
  }, [
    autoPromptedWeixinIds,
    providers,
    refreshWeixinLogin,
    statusMap,
    weixinLogin,
    weixinLoginLoading,
  ]);

  const handleDelete = async (id: string) => {
    try {
      await imGatewayApi.deleteProvider(id);
      message.success("Provider deleted");
      onRefresh();
    } catch {
      message.error("Failed to delete provider");
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

  const addProviderOkText = editingProvider
    ? "Save"
    : createStep === 0
    ? "Next"
    : createStep === 1
    ? setupProviderType === "weixin"
      ? createdProvider
        ? "Waiting for Scan"
        : "Create and Show QR"
      : feishuAutoConnecting
      ? "Connecting"
      : feishuSetup?.status === "confirmed"
      ? "Retry Connect"
      : "Waiting for Setup"
    : "Save Configuration";
  const addProviderOkDisabled =
    !editingProvider &&
    createStep === 1 &&
    ((setupProviderType === "weixin" && !!createdProvider) ||
      (setupProviderType === "feishu" &&
        (feishuSetup?.status !== "confirmed" || feishuAutoConnecting)));

  return (
    <div>
      <PanelHeader description="Manage IM bot connections">
        <Button icon={<ReloadOutlined />} onClick={onRefresh} loading={loading}>
          Refresh
        </Button>
        <Button type="primary" icon={<PlusOutlined />} onClick={openAddModal}>
          Add Provider
        </Button>
      </PanelHeader>

      {loading && providers.length === 0 ? (
        <Spin style={{ display: "block", margin: "40px auto" }} />
      ) : providers.length === 0 ? (
        <Empty description="No IM providers configured" />
      ) : (
        <div data-testid="settings-im-provider-list">
          {cardGrid ? (
            <ResponsiveCardGrid>
              {providers.map((p) => {
                const status = statusMap[p.id];
                return (
                  <Card
                    key={p.id}
                    size="small"
                    data-testid={`settings-im-provider-card-${p.id}`}
                    style={{
                      borderColor: token.colorBorderSecondary,
                      height: "100%",
                    }}
                    title={
                      <Space wrap size={[8, 6]} style={{ minWidth: 0 }}>
                        <ApiOutlined style={{ color: token.colorTextSecondary }} />
                        <Text strong style={{ maxWidth: 180 }} ellipsis>
                          {p.display_name || p.id}
                        </Text>
                        <Tag>{p.provider_type}</Tag>
                        {getStateBadge(status?.state)}
                      </Space>
                    }
                    extra={
                      <Space wrap size={8}>
                        {status?.state === "connected" ||
                        status?.state === "connecting" ||
                        status?.state === "reconnecting" ? (
                          <Tooltip title="Disconnect provider">
                            <Button
                              size="small"
                              icon={<PauseCircleOutlined />}
                              onClick={() => handleDisconnect(p.id)}
                              data-testid={`settings-im-provider-disconnect-${p.id}`}
                            />
                          </Tooltip>
                        ) : (
                          <Tooltip title="Connect provider">
                            <Button
                              size="small"
                              icon={<PlayCircleOutlined />}
                              onClick={() => handleConnect(p.id)}
                              disabled={!p.secret_configured}
                              data-testid={`settings-im-provider-connect-${p.id}`}
                            />
                          </Tooltip>
                        )}
                        {p.provider_type === "weixin" && (
                          <Tooltip title="Scan Weixin QR">
                            <Button
                              size="small"
                              icon={<CloudOutlined />}
                              onClick={() => handleStartWeixinLogin(p)}
                              loading={weixinLoginLoading && weixinLogin?.providerId === p.id}
                              data-testid={`settings-im-provider-weixin-login-${p.id}`}
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
                    <Space wrap size={[4, 6]} style={{ marginBottom: 12 }}>
                      <Tag color={p.enabled ? "green" : undefined}>
                        {p.enabled ? "Enabled" : "Disabled"}
                      </Tag>
                      <Tag>
                        {p.event_connection_enabled ? "Long Connection" : "Webhook"}
                      </Tag>
                      {displayRunnerLabel(p) ? (
                        <Tag color="geekblue">
                          {displayRunnerLabel(p)}
                        </Tag>
                      ) : null}
                    </Space>
                    <ProviderFieldList
                      items={[
                        {
                          label: "App ID",
                          value: (
                            <CopyableOverflowText
                              value={p.app_id}
                              displayValue={p.app_id ? `${p.app_id.slice(0, 8)}***` : "-"}
                              code
                              width="100%"
                              testId={`settings-im-provider-${p.id}-app-id`}
                            />
                          ),
                        },
                        {
                          label: "Secret",
                          value: p.secret_configured ? (
                            <Tag color="green">Configured</Tag>
                          ) : (
                            <Tag color="red">Not Set</Tag>
                          ),
                        },
                        {
                          label: "Owner ID",
                          value: p.owner_open_id ? (
                            <CopyableOverflowText
                              value={p.owner_open_id}
                              code
                              width="100%"
                              testId={`settings-im-provider-${p.id}-owner-id`}
                            />
                          ) : (
                            <SecondaryInline>Auto-detect</SecondaryInline>
                          ),
                        },
                        {
                          label: "Work Dir",
                          value: (
                            <CopyableOverflowText
                              value={p.agent_config?.work_dir || inheritedWorkDir}
                              code
                              width="100%"
                              testId={`settings-im-provider-${p.id}-work-dir`}
                            />
                          ),
                        },
                      ]}
                    />
                  </Card>
                );
              })}
            </ResponsiveCardGrid>
          ) : (
            <div
              style={{
                display: "flex",
                flexDirection: "column",
                gap: 12,
                maxWidth: 750,
                width: "100%",
                margin: "0 auto",
              }}
            >
              {providers.map((p) => {
                const status = statusMap[p.id];
                return (
                  <Card
                    key={p.id}
                    size="small"
                    data-testid={`settings-im-provider-card-${p.id}`}
                    style={{
                      borderColor: token.colorBorderSecondary,
                    }}
                    title={
                      <Space wrap size={[8, 6]} style={{ minWidth: 0 }}>
                        <ApiOutlined style={{ color: token.colorTextSecondary }} />
                        <Text strong style={{ maxWidth: 280 }} ellipsis>
                          {p.display_name || p.id}
                        </Text>
                        <Tag>{p.provider_type}</Tag>
                        {getStateBadge(status?.state)}
                        <Tag color={p.enabled ? "green" : undefined}>
                          {p.enabled ? "Enabled" : "Disabled"}
                        </Tag>
                        <Tag>
                          {p.event_connection_enabled ? "Long Connection" : "Webhook"}
                        </Tag>
                      </Space>
                    }
                    extra={
                      <Space wrap size={8}>
                        {status?.state === "connected" ||
                        status?.state === "connecting" ||
                        status?.state === "reconnecting" ? (
                          <Tooltip title="Disconnect provider">
                            <Button
                              size="small"
                              icon={<PauseCircleOutlined />}
                              onClick={() => handleDisconnect(p.id)}
                              data-testid={`settings-im-provider-disconnect-${p.id}`}
                            />
                          </Tooltip>
                        ) : (
                          <Tooltip title="Connect provider">
                            <Button
                              size="small"
                              icon={<PlayCircleOutlined />}
                              onClick={() => handleConnect(p.id)}
                              disabled={!p.secret_configured}
                              data-testid={`settings-im-provider-connect-${p.id}`}
                            />
                          </Tooltip>
                        )}
                        {p.provider_type === "weixin" && (
                          <Tooltip title="Scan Weixin QR">
                            <Button
                              size="small"
                              icon={<CloudOutlined />}
                              onClick={() => handleStartWeixinLogin(p)}
                              loading={weixinLoginLoading && weixinLogin?.providerId === p.id}
                              data-testid={`settings-im-provider-weixin-login-${p.id}`}
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
                        <Popconfirm
                          title="Delete this provider?"
                          onConfirm={() => handleDelete(p.id)}
                        >
                          <Button size="small" danger icon={<DeleteOutlined />} />
                        </Popconfirm>
                      </Space>
                    }
                  >
                    <ProviderFieldList
                      items={[
                        {
                          label: "App ID",
                          value: (
                            <CopyableOverflowText
                              value={p.app_id}
                              displayValue={p.app_id ? `${p.app_id.slice(0, 8)}***` : "-"}
                              code
                              testId={`settings-im-provider-${p.id}-app-id`}
                            />
                          ),
                        },
                        {
                          label: "Secret",
                          value: p.secret_configured ? (
                            <Tag color="green">Configured</Tag>
                          ) : (
                            <Tag color="red">Not Set</Tag>
                          ),
                        },
                        {
                          label: "Owner ID",
                          value: p.owner_open_id ? (
                            <CopyableOverflowText
                              value={p.owner_open_id}
                              code
                              testId={`settings-im-provider-${p.id}-owner-id`}
                            />
                          ) : (
                            <SecondaryInline>Auto-detect on connect</SecondaryInline>
                          ),
                        },
                        {
                          label: "Agent Runner",
                          value: displayRunnerLabel(p) ? (
                            <Tag color="geekblue">
                              {displayRunnerLabel(p)}
                            </Tag>
                          ) : (
                            <SecondaryInline>Global default</SecondaryInline>
                          ),
                        },
                        {
                          label: "Agent Work Dir",
                          value: p.agent_config?.work_dir ? (
                            <CopyableOverflowText
                              value={p.agent_config.work_dir}
                              code
                              testId={`settings-im-provider-${p.id}-work-dir`}
                            />
                          ) : (
                            <CopyableOverflowText
                              value={inheritedWorkDir}
                              code
                              testId={`settings-im-provider-${p.id}-work-dir`}
                            />
                          ),
                        },
                        {
                          label: "Agent Base Prompt",
                          value: p.agent_config?.base_instructions ? (
                            <Tag color="blue">Configured</Tag>
                          ) : (
                            <SecondaryInline>Global default</SecondaryInline>
                          ),
                        },
                        {
                          label: "Agent Developer/User",
                          value:
                            p.agent_config?.developer_instructions ||
                            p.agent_config?.user_instructions ? (
                              <Tag color="purple">Configured</Tag>
                            ) : (
                              <SecondaryInline>Global default</SecondaryInline>
                            ),
                        },
                        ...(status?.reconnect_count != null && status.reconnect_count > 0
                          ? [{ label: "Reconnects", value: status.reconnect_count }]
                          : []),
                        ...(status?.last_error
                          ? [
                              {
                                label: "Last Error",
                                value: (
                                  <Text type="danger" ellipsis style={{ maxWidth: 300 }}>
                                    {status.last_error}
                                  </Text>
                                ),
                              },
                            ]
                          : []),
                      ]}
                    />
                  </Card>
                );
              })}
            </div>
          )}
        </div>
      )}

      <Modal
        title={editingProvider ? "Edit IM Provider" : "Add IM Provider"}
        open={addModalOpen}
        onOk={handleSaveProvider}
        onCancel={() => {
          setAddModalOpen(false);
          setEditingProvider(null);
          setCreateStep(0);
          setCreatedProvider(null);
          setFeishuSetup(null);
          setFeishuAutoConnectAttemptedSessionId(null);
          setFeishuAutoConnecting(false);
          form.resetFields();
        }}
        okText={addProviderOkText}
        okButtonProps={{ disabled: addProviderOkDisabled }}
        width={editingProvider || createStep === 2 ? 720 : 560}
      >
        {!editingProvider && (
          <Steps
            size="small"
            current={createStep}
            style={{ marginBottom: 20 }}
            items={[
              { title: "Type" },
              { title: "Connect" },
              { title: "Configure" },
            ]}
          />
        )}
        <Form
          form={form}
          layout="vertical"
          size={editingProvider ? "small" : "middle"}
          className={editingProvider ? "im-provider-edit-form-compact" : undefined}
        >
          {!editingProvider && createStep === 0 ? (
            <Space direction="vertical" size={12} style={{ width: "100%" }}>
              <Text type="secondary">
                Choose the IM platform first. Bifrost will only ask for platform-specific setup on
                the next step.
              </Text>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
                {([
                  {
                    type: "weixin" as const,
                    title: "Weixin",
                    desc: "Create the provider, then scan the QR code with Weixin.",
                  },
                  {
                    type: "feishu" as const,
                    title: "Feishu",
                    desc: "Scan a Lark CLI style QR code to create the bot app automatically.",
                  },
                ]).map((option) => (
                  <Card
                    key={option.type}
                    size="small"
                    hoverable
                    onClick={() => setSetupProviderType(option.type)}
                    style={{
                      borderColor:
                        setupProviderType === option.type
                          ? token.colorPrimary
                          : token.colorBorderSecondary,
                    }}
                  >
                    <Space direction="vertical" size={4}>
                      <Space>
                        <ApiOutlined
                          style={{
                            color:
                              setupProviderType === option.type
                                ? token.colorPrimary
                                : token.colorTextSecondary,
                          }}
                        />
                        <Text strong>{option.title}</Text>
                      </Space>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        {option.desc}
                      </Text>
                    </Space>
                  </Card>
                ))}
              </div>
            </Space>
          ) : !editingProvider && createStep === 1 ? (
            <>
              <Form.Item
                name="id"
                label="Provider ID"
                rules={[{ required: true, message: "Required" }]}
              >
                <Input
                  placeholder={nextProviderId(setupProviderType, providers)}
                  disabled={!!createdProvider || feishuAutoConnecting}
                />
              </Form.Item>
              <Form.Item name="display_name" label="Display Name">
                <Input
                  placeholder="Optional display name"
                  disabled={!!createdProvider || feishuAutoConnecting}
                />
              </Form.Item>
              {setupProviderType === "weixin" ? (
                <Alert
                  type={createdProvider ? "success" : "info"}
                  showIcon
                  message={
                    createdProvider
                      ? "Provider created. Complete the QR scan in the Weixin dialog."
                      : "Create the provider first, then Bifrost will immediately show the Weixin QR code."
                  }
                />
              ) : (
                <Space direction="vertical" size={14} style={{ width: "100%", alignItems: "center" }}>
                  {feishuSetup?.verificationUrl ? (
                    <>
                      <QRCode value={feishuSetup.verificationUrl} size={220} />
                      <Text code style={{ wordBreak: "break-all", textAlign: "center" }}>
                        {feishuSetup.verificationUrl}
                      </Text>
                      <Button
                        href={feishuSetup.verificationUrl}
                        target="_blank"
                        rel="noopener noreferrer"
                      >
                        Open Setup Page
                      </Button>
                    </>
                  ) : feishuSetup?.status === "error" ? null : (
                    <Spin />
                  )}
                  <Alert
                    type={
                      feishuSetup?.status === "confirmed"
                        ? "success"
                        : feishuSetup?.status === "expired"
                        ? "warning"
                        : feishuSetup?.status === "error"
                        ? "error"
                        : "info"
                    }
                    showIcon
                    style={{ width: "100%" }}
                    message={
                      feishuSetup?.status === "confirmed"
                        ? "App created. Bifrost has the App ID and Secret on the server."
                        : feishuSetup?.status === "expired"
                        ? "The setup QR code expired. Close and start again."
                        : feishuSetup?.status === "error"
                        ? "Failed to start Feishu setup."
                        : "Create the bot app in the opened page. Bifrost will detect completion automatically."
                    }
                    description={
                      feishuSetup?.status === "error" ? (
                        <Space direction="vertical" size={8}>
                          <Text type="secondary">{feishuSetup.errorMessage}</Text>
                          <Button
                            size="small"
                            loading={feishuSetupLoading}
                            onClick={() => void startFeishuSetupSession()}
                          >
                            Retry Setup
                          </Button>
                        </Space>
                      ) : feishuSetup?.appId ? (
                        <Text code>{feishuSetup.appId}</Text>
                      ) : undefined
                    }
                  />
                </Space>
              )}
            </>
          ) : editingProvider ? (
            <Descriptions
              size="small"
              column={1}
              className="im-provider-edit-summary"
              style={{ marginBottom: 10 }}
            >
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
            createdProvider ? (
              <Descriptions size="small" column={1} style={{ marginBottom: 16 }}>
                <Descriptions.Item label="Provider ID">
                  <Text code>{createdProvider.id}</Text>
                </Descriptions.Item>
                <Descriptions.Item label="Type">
                  <Tag>{createdProvider.provider_type}</Tag>
                </Descriptions.Item>
                <Descriptions.Item label="Connection">
                  <Tag color="green">Connected</Tag>
                </Descriptions.Item>
              </Descriptions>
            ) : null
          )}
          {(editingProvider || createStep === 2) && (
            <>
          <Form.Item name="display_name" label="Display Name">
            <Input placeholder="Optional display name" />
          </Form.Item>
          <Form.Item name="enabled" label="Enabled" valuePropName="checked">
            <Switch />
          </Form.Item>
          <Form.Item name="owner_open_id" label="Owner Open ID">
            <Input placeholder="Optional owner open_id" />
          </Form.Item>
          {editingProvider && (
            <>
              <Form.Item name="app_id" label="App ID">
                <Input
                  placeholder={
                    selectedProviderType === "weixin"
                      ? "Filled after QR login"
                      : "Application ID"
                  }
                  disabled={selectedProviderType === "weixin"}
                />
              </Form.Item>
              <Form.Item
                name="app_secret"
                label={selectedProviderType === "weixin" ? "Bot Token" : "App Secret"}
                extra={
                  selectedProviderType === "weixin"
                    ? "Filled by QR login. Manual replacement is only for debugging."
                    : "Leave blank to keep the existing secret. Enter a new value to replace it."
                }
              >
                <Input.Password
                  placeholder={
                    selectedProviderType === "weixin"
                      ? "Filled by QR login"
                      : "Application secret (stored securely)"
                  }
                />
              </Form.Item>
            </>
          )}
          <Form.Item
            name={["agent_config", "runner"]}
            label="Agent Runner"
            extra={
              <Text type="secondary">
                Inherits: {inheritedRunner}. Custom runners are managed under Agent Runners.
              </Text>
            }
          >
            <Select options={agentRunnerOptions} />
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
            </>
          )}
        </Form>
      </Modal>
      <Modal
        title="Scan Weixin QR"
        open={!!weixinLogin}
        footer={
          <Space>
            <Button onClick={() => setWeixinLogin(null)}>Close</Button>
            <Button
              type="primary"
              loading={weixinLoginLoading}
              onClick={() =>
                weixinLogin && void refreshWeixinLogin(weixinLogin.providerId, true)
              }
            >
              Refresh QR
            </Button>
          </Space>
        }
        onCancel={() => setWeixinLogin(null)}
      >
        {weixinLogin && (
          <Space direction="vertical" size={16} style={{ width: "100%", alignItems: "center" }}>
            <QRCode value={weixinLogin.scanUrl} size={220} />
            <Text type="secondary">
              {weixinLogin.status === "checking"
                ? "Checking authorization..."
                : weixinLogin.status === "expired"
                ? "QR code expired. Refreshing automatically..."
                : `Expires in about ${Math.max(
                    0,
                    Math.ceil((weixinLogin.expiresAt - Date.now()) / 1000),
                  )} seconds. Scan with Weixin; Bifrost will connect automatically.`}
            </Text>
            <Text code style={{ wordBreak: "break-all" }}>
              {weixinLogin.scanUrl}
            </Text>
          </Space>
        )}
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
      <PanelHeader description="Message targets define where to send notifications">
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
      </PanelHeader>

      <Table
        dataSource={targets}
        columns={columns}
        rowKey="id"
        size="small"
        loading={loading}
        pagination={false}
        locale={{ emptyText: <Empty description="No targets configured" /> }}
        scroll={{ x: 760 }}
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
      <PanelHeader description="Event routes map incoming messages to script actions">
        <Button icon={<ReloadOutlined />} onClick={onRefresh} loading={loading}>
          Refresh
        </Button>
      </PanelHeader>

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
                <Space wrap style={{ minWidth: 0 }}>
                  <RocketOutlined />
                  <span>{r.name || r.id}</span>
                  <Tag color={r.enabled ? "green" : "default"}>
                    {r.enabled ? "Active" : "Paused"}
                  </Tag>
                </Space>
              }
              extra={
                <Space wrap size={8}>
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
              <DetailGrid
                items={[
                  {
                    label: "Provider",
                    value: <OverflowText value={r.provider_id} code width={180} />,
                  },
                  { label: "Event", value: <Tag>{r.event_type}</Tag> },
                  {
                    label: "Matcher",
                    value: (
                      <Space wrap size={[4, 4]}>
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
                      </Space>
                    ),
                  },
                  { label: "Action", value: <Tag>{r.action?.type || "script"}</Tag> },
                  { label: "Timeout", value: r.timeout_ms ? `${r.timeout_ms}ms` : "-" },
                ]}
              />
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
  providers,
  externalCliConfig,
  loading,
  onRefresh,
}: {
  schedules: ImSchedule[];
  providers: ImProviderConfig[];
  externalCliConfig: ExternalCliGatewayConfig | null;
  loading: boolean;
  onRefresh: () => void;
}) {
  const [addModalOpen, setAddModalOpen] = useState(false);
  const [detailOpen, setDetailOpen] = useState(false);
  const [selectedSchedule, setSelectedSchedule] = useState<ImSchedule | null>(null);
  const [scheduleRuns, setScheduleRuns] = useState<ImTaskRun[]>([]);
  const [runsLoading, setRunsLoading] = useState(false);
  const [form] = Form.useForm();
  const taskType = Form.useWatch("task_type", form) || "script";
  const triggerType = Form.useWatch("trigger_type", form) || "interval";
  const selectedRunnerId = Form.useWatch("agent_runner_id", form) || externalCliConfig?.defaultRunnerId || "Codex";
  const scheduleRunnerOptions = useMemo(
    () => [
      ...Object.keys(externalCliConfig?.runners || {})
        .sort()
        .map((id) => ({ label: id, value: id })),
    ],
    [externalCliConfig?.runners],
  );
  const selectedRunner = externalCliConfig?.runners?.[selectedRunnerId];
  const selectedRunnerIsChatGptWeb = selectedRunner?.adapter === "chatgpt_web";
  const providerById = useMemo(
    () => new Map(providers.map((provider) => [provider.id, provider])),
    [providers],
  );
  const channelOptions = useMemo(
    () =>
      providers
        .filter((provider) => provider.owner_open_id?.trim())
        .map((provider) => ({
          value: provider.id,
          label: `${provider.display_name || provider.id} (${provider.id})`,
        })),
    [providers],
  );

  const resolveChannelPayload = (value?: string) => {
    if (!value) return null;
    const provider = providerById.get(value);
    if (!provider?.owner_open_id?.trim()) return null;
    return {
      messageChannel: {
        provider_id: provider.id,
        target_id: "owner",
        target_mode: "owner" as const,
      },
    };
  };

  const renderScheduleChannel = (schedule: ImSchedule) => {
    const channel = schedule.message_channel;
    if (channel) {
      const provider = providerById.get(channel.provider_id);
      return provider?.display_name || channel.provider_id || "-";
    }
    return "-";
  };

  const handleAdd = async () => {
    try {
      const values = await form.validateFields();
      const channel = resolveChannelPayload(values.target_channel);
      const payload: Partial<ImSchedule> = {
        id: values.id?.trim() || undefined,
        name: values.name,
        enabled: values.enabled ?? true,
        message_channel: channel?.messageChannel,
        task_type: values.task_type,
        trigger:
          values.trigger_type === "cron"
            ? {
                type: "cron",
                expr: values.cron_expr,
                timezone: values.timezone || "UTC",
              }
            : {
                type: "interval",
                every_ms: Number(values.every_ms),
              },
        timeout_ms: Number(values.timeout_ms || 30000),
        max_output_bytes: Number(values.max_output_bytes || 1048576),
      };
      if (values.task_type === "agent") {
        payload.agent = {
          prompt: values.agent_prompt,
          runner_id: values.agent_runner_id || undefined,
          initial_prompt: selectedRunnerIsChatGptWeb
            ? values.agent_initial_prompt || undefined
            : undefined,
          session_key: values.agent_session_key || undefined,
          work_dir: values.agent_work_dir || undefined,
          system_prompt: values.agent_system_prompt || undefined,
        };
      } else {
        payload.script = {
          script_text: values.script_text || undefined,
          script_file: values.script_file || undefined,
          cwd: values.cwd || undefined,
        };
      }
      await imGatewayApi.createSchedule(payload);
      message.success("Schedule created");
      setAddModalOpen(false);
      form.resetFields();
      onRefresh();
    } catch (err) {
      if ((err as { errorFields?: unknown }).errorFields) return;
      message.error(err instanceof Error ? err.message : "Failed to create schedule");
    }
  };

  const openScheduleDetail = async (schedule: ImSchedule) => {
    setSelectedSchedule(schedule);
    setDetailOpen(true);
    setRunsLoading(true);
    try {
      setScheduleRuns(await imGatewayApi.getScheduleRuns(schedule.id));
    } catch (err) {
      message.error(err instanceof Error ? err.message : "Failed to load schedule runs");
      setScheduleRuns([]);
    } finally {
      setRunsLoading(false);
    }
  };

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

  const handleRun = async (schedule: ImSchedule) => {
    try {
      await imGatewayApi.runSchedule(schedule.id);
      message.success("Schedule triggered");
      onRefresh();
      await openScheduleDetail(schedule);
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
      title: "Type",
      dataIndex: "task_type",
      key: "task_type",
      width: 90,
      render: (val: string | undefined) => (
        <Tag color={val === "agent" ? "blue" : "default"}>
          {val === "agent" ? "Agent" : "Script"}
        </Tag>
      ),
    },
    {
      title: "IM Channel",
      key: "message_channel",
      width: 140,
      render: (_: unknown, record: ImSchedule) => renderScheduleChannel(record),
    },
    {
      title: "Runner",
      key: "runner",
      width: 130,
      render: (_: unknown, record: ImSchedule) =>
        record.task_type === "agent" ? record.agent?.runner_id || externalCliConfig?.defaultRunnerId || "Codex" : "-",
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
        <Space size="small" onClick={(event) => event.stopPropagation()}>
          <Tooltip title="Run now">
            <Button
              size="small"
              icon={<PlayCircleOutlined />}
              onClick={() => handleRun(record)}
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
      <PanelHeader description="Scheduled tasks run scripts or Agent prompts on a cron/interval basis">
        <Button
          type="primary"
          icon={<PlusOutlined />}
          onClick={() => {
            form.setFieldsValue({
              enabled: true,
              task_type: "script",
              agent_runner_id: externalCliConfig?.defaultRunnerId || "Codex",
              trigger_type: "interval",
              every_ms: 60000,
              timezone: "UTC",
              timeout_ms: 30000,
              max_output_bytes: 1048576,
            });
            setAddModalOpen(true);
          }}
        >
          Add
        </Button>
        <Button icon={<ReloadOutlined />} onClick={onRefresh} loading={loading}>
          Refresh
        </Button>
      </PanelHeader>

      <Table
        dataSource={schedules}
        columns={columns}
        rowKey="id"
        size="small"
        loading={loading}
        pagination={false}
        onRow={(record) => ({
          onClick: () => openScheduleDetail(record),
          style: { cursor: "pointer" },
        })}
        locale={{ emptyText: <Empty description="No schedules configured" /> }}
        scroll={{ x: 1020 }}
      />

      <Modal
        title="Add Schedule"
        open={addModalOpen}
        onCancel={() => setAddModalOpen(false)}
        onOk={handleAdd}
        okText="Create"
        width={720}
        destroyOnHidden
      >
        <Form form={form} layout="vertical">
          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
            <Form.Item name="id" label="ID">
              <Input placeholder="Optional stable id" />
            </Form.Item>
            <Form.Item
              name="name"
              label="Name"
              rules={[{ required: true, message: "Name is required" }]}
            >
              <Input placeholder="Daily summary" />
            </Form.Item>
            <Form.Item name="task_type" label="Task Type" initialValue="script">
              <Select
                options={[
                  { value: "script", label: "Script" },
                  { value: "agent", label: "Agent" },
                ]}
              />
            </Form.Item>
            <Form.Item
              name="target_channel"
              label="IM Channel"
              rules={[
                ({ getFieldValue }) => ({
                  validator(_, value) {
                    if (value) return Promise.resolve();
                    const currentTaskType = getFieldValue("task_type");
                    if (currentTaskType === "agent" || currentTaskType === "script") {
                      return Promise.reject(new Error("IM channel is required"));
                    }
                    return Promise.resolve();
                  },
                }),
              ]}
            >
              <Select
                allowClear
                placeholder="Select a connection"
                options={channelOptions}
              />
            </Form.Item>
            <Form.Item name="trigger_type" label="Trigger" initialValue="interval">
              <Select
                options={[
                  { value: "interval", label: "Interval" },
                  { value: "cron", label: "Cron" },
                ]}
              />
            </Form.Item>
            {triggerType === "interval" ? (
              <Form.Item
                name="every_ms"
                label="Every (ms)"
                rules={[{ required: true, message: "Interval is required" }]}
              >
                <Input type="number" min={1} />
              </Form.Item>
            ) : (
              <Form.Item
                name="cron_expr"
                label="Cron"
                rules={[{ required: true, message: "Cron expression is required" }]}
              >
                <Input placeholder="*/5 * * * *" />
              </Form.Item>
            )}
            {triggerType === "cron" && (
              <Form.Item name="timezone" label="Timezone">
                <Input placeholder="UTC" />
              </Form.Item>
            )}
            <Form.Item name="timeout_ms" label="Timeout (ms)">
              <Input type="number" min={1} />
            </Form.Item>
            <Form.Item name="max_output_bytes" label="Max Output Bytes">
              <Input type="number" min={1} />
            </Form.Item>
            <Form.Item name="enabled" label="Enabled" valuePropName="checked">
              <Switch />
            </Form.Item>
          </div>

          {taskType === "agent" ? (
            <>
              <Form.Item name="agent_runner_id" label="Runner" initialValue={externalCliConfig?.defaultRunnerId || "Codex"}>
                <Select options={scheduleRunnerOptions} />
              </Form.Item>
              {selectedRunnerIsChatGptWeb && (
                <Form.Item name="agent_initial_prompt" label="First-round Prompt">
                  <Input.TextArea
                    rows={3}
                    placeholder="Optional persona or initialization prompt for this ChatGPT Web Runner"
                  />
                </Form.Item>
              )}
              <Form.Item
                name="agent_prompt"
                label="Preset Prompt"
                rules={[{ required: true, message: "Preset prompt is required" }]}
              >
                <Input.TextArea rows={5} placeholder="Run the scheduled Agent task..." />
              </Form.Item>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
                <Form.Item name="agent_session_key" label="Session Key">
                  <Input placeholder="Optional session key" />
                </Form.Item>
                <Form.Item name="agent_work_dir" label="Default Execution Directory">
                  <Input placeholder="Project path used for runner execution" />
                </Form.Item>
              </div>
            </>
          ) : (
            <>
              <Form.Item
                name="script_text"
                label="Script Text"
                rules={[
                  ({ getFieldValue }) => ({
                    validator(_, value) {
                      if (value || getFieldValue("script_file")) return Promise.resolve();
                      return Promise.reject(new Error("Script text or file is required"));
                    },
                  }),
                ]}
              >
                <Input.TextArea rows={5} placeholder="echo hello" />
              </Form.Item>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
                <Form.Item name="script_file" label="Script File">
                  <Input placeholder="/path/to/script.sh" />
                </Form.Item>
                <Form.Item name="cwd" label="Working Directory">
                  <Input placeholder="Optional cwd" />
                </Form.Item>
              </div>
            </>
          )}
        </Form>
      </Modal>

      <ScheduleDetailModal
        open={detailOpen}
        schedule={selectedSchedule}
        providers={providers}
        runs={scheduleRuns}
        loading={runsLoading}
        onClose={() => setDetailOpen(false)}
        onRefresh={() => selectedSchedule && openScheduleDetail(selectedSchedule)}
      />
    </div>
  );
}

function ScheduleDetailModal({
  open,
  schedule,
  providers,
  runs,
  loading,
  onClose,
  onRefresh,
}: {
  open: boolean;
  schedule: ImSchedule | null;
  providers: ImProviderConfig[];
  runs: ImTaskRun[];
  loading: boolean;
  onClose: () => void;
  onRefresh: () => void;
}) {
  if (!schedule) return null;

  const renderTime = (ts?: number) => {
    if (!ts) return "-";
    return new Date(ts).toLocaleString();
  };

  const renderTarget = () => {
    const channel = schedule.message_channel;
    if (channel) {
      const provider = providers.find((item) => item.id === channel.provider_id);
      return provider?.display_name || channel.provider_id;
    }
    return "-";
  };

  const renderConversationRef = () => {
    const ref = schedule.agent?.conversation_ref;
    if (!ref) return "-";
    const id = ref.conversationId || ref.threadId;
    if (!id) return "-";
    return `${ref.adapter}: ${id}`;
  };

  const renderBlock = (value?: string) => (
    <pre
      style={{
        margin: 0,
        maxHeight: 180,
        overflow: "auto",
        whiteSpace: "pre-wrap",
        wordBreak: "break-word",
        background: "var(--color-bg-elevated, rgba(128, 128, 128, 0.08))",
        border: "1px solid var(--color-border, rgba(128, 128, 128, 0.28))",
        borderRadius: 6,
        padding: 10,
      }}
    >
      {value || "-"}
    </pre>
  );

  return (
    <Modal
      title={`Schedule Detail · ${schedule.name}`}
      open={open}
      onCancel={onClose}
      footer={[
        <Button key="refresh" icon={<ReloadOutlined />} onClick={onRefresh} loading={loading}>
          Refresh
        </Button>,
        <Button key="close" onClick={onClose}>
          Close
        </Button>,
      ]}
      width={920}
      destroyOnHidden
    >
      <Descriptions size="small" bordered column={2}>
        <Descriptions.Item label="ID">{schedule.id}</Descriptions.Item>
        <Descriptions.Item label="Type">
          <Tag color={schedule.task_type === "agent" ? "blue" : "default"}>
            {schedule.task_type === "agent" ? "Agent" : "Script"}
          </Tag>
        </Descriptions.Item>
        <Descriptions.Item label="Enabled">
          {schedule.enabled ? "Yes" : "No"}
        </Descriptions.Item>
        <Descriptions.Item label="IM Channel">{renderTarget()}</Descriptions.Item>
        {schedule.task_type === "agent" && (
          <Descriptions.Item label="Runner">
            {schedule.agent?.runner_id || "Default external runner"}
          </Descriptions.Item>
        )}
        {schedule.task_type === "agent" && (
          <Descriptions.Item label="Conversation">{renderConversationRef()}</Descriptions.Item>
        )}
        <Descriptions.Item label="Trigger">
          {schedule.trigger.type === "cron"
            ? `${schedule.trigger.expr || "-"} (${schedule.trigger.timezone || "UTC"})`
            : `every ${schedule.trigger.every_ms}ms`}
        </Descriptions.Item>
        <Descriptions.Item label="Next Run">{renderTime(schedule.next_run_at)}</Descriptions.Item>
        <Descriptions.Item label="Last Run">{renderTime(schedule.last_run_at)}</Descriptions.Item>
        <Descriptions.Item label="Timeout">{schedule.timeout_ms}ms</Descriptions.Item>
      </Descriptions>

      <div style={{ marginTop: 16 }}>
        <Text strong>Task</Text>
        <div style={{ marginTop: 8 }}>
          {schedule.task_type === "agent"
            ? renderBlock(schedule.agent?.prompt)
            : renderBlock(schedule.script?.script_text || schedule.script?.script_file)}
        </div>
      </div>
      {schedule.task_type === "agent" && schedule.agent?.initial_prompt && (
        <div style={{ marginTop: 16 }}>
          <Text strong>First-round Prompt</Text>
          <div style={{ marginTop: 8 }}>{renderBlock(schedule.agent.initial_prompt)}</div>
        </div>
      )}

      <div style={{ marginTop: 20, marginBottom: 8 }}>
        <Text strong>Run History</Text>
      </div>
      <Spin spinning={loading}>
        {runs.length === 0 ? (
          <Empty description="No runs yet" />
        ) : (
          <Space direction="vertical" style={{ width: "100%" }} size={12}>
            {runs
              .slice()
              .reverse()
              .map((run) => (
                <RunDetailCard key={run.run_id} run={run} />
              ))}
          </Space>
        )}
      </Spin>
    </Modal>
  );
}

function RunDetailCard({ run }: { run: ImTaskRun }) {
  const isAgent = run.task_type === "agent" || !!run.agent_final_response;
  const formatRunTs = (ts?: number) => {
    if (!ts) return "-";
    const ms = ts > 1_000_000_000_000 ? ts : ts * 1000;
    return new Date(ms).toLocaleString();
  };
  const renderBlock = (value?: string) => (
    <pre
      style={{
        margin: 0,
        maxHeight: 220,
        overflow: "auto",
        whiteSpace: "pre-wrap",
        wordBreak: "break-word",
        background: "var(--color-bg-elevated, rgba(128, 128, 128, 0.08))",
        border: "1px solid var(--color-border, rgba(128, 128, 128, 0.28))",
        borderRadius: 6,
        padding: 10,
      }}
    >
      {value || "-"}
    </pre>
  );
  return (
    <Card size="small" title={`${run.run_id} · ${run.status}`}>
      <Descriptions size="small" column={3} bordered>
        <Descriptions.Item label="Run ID">{run.run_id}</Descriptions.Item>
        <Descriptions.Item label="Type">
          <Tag color={isAgent ? "blue" : "default"}>{isAgent ? "Agent" : "Script"}</Tag>
        </Descriptions.Item>
        <Descriptions.Item label="Status">
          <Tag color={run.status === "success" || run.status === "completed" ? "green" : run.status === "failed" || run.status === "error" ? "red" : "processing"}>
            {run.status}
          </Tag>
        </Descriptions.Item>
        <Descriptions.Item label="Source">{run.trigger_source}</Descriptions.Item>
        <Descriptions.Item label="Schedule">{run.schedule_id || "-"}</Descriptions.Item>
        <Descriptions.Item label="Route">{run.route_id || "-"}</Descriptions.Item>
        <Descriptions.Item label="Provider">{run.provider_id || "-"}</Descriptions.Item>
        <Descriptions.Item label="Target">{run.target_id || "-"}</Descriptions.Item>
        {isAgent && (
          <Descriptions.Item label="Runner">
            {run.runner_id || "Default external runner"}
          </Descriptions.Item>
        )}
        <Descriptions.Item label="Started">
          {formatRunTs(run.started_at)}
        </Descriptions.Item>
        <Descriptions.Item label="Ended">
          {formatRunTs(run.ended_at)}
        </Descriptions.Item>
        <Descriptions.Item label="Duration">
          {run.duration_ms !== undefined ? `${run.duration_ms}ms` : "-"}
        </Descriptions.Item>
        <Descriptions.Item label="Exit Code">
          {run.exit_code !== undefined ? run.exit_code : "-"}
        </Descriptions.Item>
        <Descriptions.Item label="Stdout SHA1">
          <Text code>{run.stdout_digest || "-"}</Text>
        </Descriptions.Item>
        <Descriptions.Item label="Stderr SHA1">
          <Text code>{run.stderr_digest || "-"}</Text>
        </Descriptions.Item>
        <Descriptions.Item label="Error">{run.error || "-"}</Descriptions.Item>
      </Descriptions>

      {isAgent ? (
        <Space direction="vertical" style={{ width: "100%", marginTop: 12 }} size={12}>
          <div>
            <Text strong>Input</Text>
            <div style={{ marginTop: 6 }}>{renderBlock(run.input_preview)}</div>
          </div>
          <div>
            <Text strong>Final Result</Text>
            <div style={{ marginTop: 6 }}>{renderBlock(run.agent_final_response || run.stdout_preview)}</div>
          </div>
          {run.agent_plan_steps && run.agent_plan_steps.length > 0 && (
            <div>
              <Text strong>Plan Trace</Text>
              <div style={{ marginTop: 6 }}>
                {run.agent_plan_steps.map((step, idx) => (
                  <div key={`${step.step}-${idx}`}>
                    <Tag>{step.status}</Tag>
                    <Text>{step.step}</Text>
                  </div>
                ))}
              </div>
            </div>
          )}
          {run.agent_tool_calls && run.agent_tool_calls.length > 0 && (
            <div>
              <Text strong>Tool Calls</Text>
              <Space direction="vertical" style={{ width: "100%", marginTop: 6 }} size={8}>
                {run.agent_tool_calls.map((tool, idx) => (
                  <Card key={`${tool.tool_name}-${idx}`} size="small" type="inner" title={`${tool.tool_name} · ${tool.success ? "success" : "failed"}`}>
                    <Text type="secondary">Arguments</Text>
                    {renderBlock(tool.arguments)}
                    <div style={{ marginTop: 8 }}>
                      <Text type="secondary">Result</Text>
                    </div>
                    {renderBlock(tool.result)}
                  </Card>
                ))}
              </Space>
            </div>
          )}
        </Space>
      ) : (
        <Space direction="vertical" style={{ width: "100%", marginTop: 12 }} size={12}>
          <div>
            <Text strong>Input</Text>
            <div style={{ marginTop: 6 }}>{renderBlock(run.input_preview)}</div>
          </div>
          <div>
            <Text strong>Stdout</Text>
            <div style={{ marginTop: 6 }}>{renderBlock(run.stdout_preview)}</div>
          </div>
          <div>
            <Text strong>Stderr</Text>
            <div style={{ marginTop: 6 }}>{renderBlock(run.stderr_preview)}</div>
          </div>
        </Space>
      )}
    </Card>
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
  const [selectedRun, setSelectedRun] = useState<ImTaskRun | null>(null);
  const formatTs = (ts?: number) => {
    if (!ts) return "-";
    const secs = ts > 1_000_000_000_000 ? ts / 1000 : ts;
    return new Date(secs * 1000).toLocaleString();
  };
  const renderOverflowText = (
    value: string | undefined,
    options?: { code?: boolean; width?: number },
  ) => {
    return (
      <OverflowText
        value={value}
        code={options?.code}
        width={options?.width ?? 240}
      />
    );
  };

  const eventColumns = [
    {
      title: "Event ID",
      dataIndex: "event_id",
      key: "event_id",
      width: 160,
      render: (val: string) => renderOverflowText(val, { code: true, width: 130 }),
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
      width: 170,
      render: (val: string) => <Tag>{val}</Tag>,
    },
    {
      title: "Source",
      key: "source",
      width: 360,
      render: (_: unknown, record: ImEvent) =>
        renderOverflowText(record.source?.chat_id || record.source?.user_id, {
          width: 330,
        }),
    },
    {
      title: "Message",
      key: "message",
      width: 420,
      render: (_: unknown, record: ImEvent) =>
        renderOverflowText(record.message?.text, { width: 390 }),
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
          pagination={{ pageSize: 20, size: "small", showSizeChanger: false }}
          locale={{ emptyText: <Empty description="No events recorded" /> }}
          scroll={{ x: 1390 }}
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
          pagination={{ pageSize: 20, size: "small", showSizeChanger: false }}
          locale={{ emptyText: <Empty description="No task runs recorded" /> }}
          scroll={{ x: 900 }}
          onRow={(record) => ({
            onClick: () => setSelectedRun(record),
            style: { cursor: "pointer" },
          })}
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
      <Modal
        title={selectedRun ? `Task Run Detail · ${selectedRun.run_id}` : "Task Run Detail"}
        open={!!selectedRun}
        onCancel={() => setSelectedRun(null)}
        footer={[
          <Button key="close" onClick={() => setSelectedRun(null)}>
            Close
          </Button>,
        ]}
        width={920}
        destroyOnHidden
      >
        <div style={{ maxHeight: "70vh", overflowY: "auto", paddingRight: 4 }}>
          {selectedRun && <RunDetailCard run={selectedRun} />}
        </div>
      </Modal>
    </div>
  );
}

// ─── Main Tab Component ──────────────────────────────────────────────────────

interface ImGatewayTabProps {
  hideSectionNav?: boolean;
  visibleSections?: ImGatewaySectionId[];
  cardGrid?: boolean;
}

export default function ImGatewayTab({
  hideSectionNav = false,
  visibleSections,
  cardGrid = false,
}: ImGatewayTabProps) {
  const [searchParams, setSearchParams] = useSearchParams();
  const sectionFromUrl = searchParams.get("imGatewaySection");
  const activeSection =
    IM_GATEWAY_SECTION_NAV.find((section) => section.id === sectionFromUrl)?.id ??
    "connections";
  const renderedSections = useMemo(
    () => visibleSections ?? [activeSection],
    [activeSection, visibleSections],
  );
  const { token } = theme.useToken();
  const screens = useBreakpoint();
  const isCompactNav = !screens.lg;

  const [providers, setProviders] = useState<ImProviderConfig[]>([]);
  const [targets, setTargets] = useState<ImTarget[]>([]);
  const [routes, setRoutes] = useState<ImRoute[]>([]);
  const [schedules, setSchedules] = useState<ImSchedule[]>([]);
  const [events, setEvents] = useState<ImEvent[]>([]);
  const [runs, setRuns] = useState<ImTaskRun[]>([]);
  const [externalCliConfig, setExternalCliConfig] =
    useState<ExternalCliGatewayConfig | null>(null);
  const [loading, setLoading] = useState(false);

  const fetchData = useCallback(async () => {
    setLoading(true);
    try {
      const needs = new Set(renderedSections);
      const needsProviders =
        needs.has("connections") || needs.has("targets") || needs.has("schedules");
      const tasks: Array<Promise<void>> = [];
      if (needsProviders) {
        tasks.push(imGatewayApi.listProviders().then(setProviders));
      }
      if (needs.has("targets")) {
        tasks.push(imGatewayApi.listTargets().then(setTargets));
      }
      if (needs.has("routes")) {
        tasks.push(imGatewayApi.listRoutes().then(setRoutes));
      }
      if (needs.has("schedules")) {
        tasks.push(
          Promise.all([
            imGatewayApi.listSchedules(),
            imGatewayApi.getExternalCliConfig().catch(() => null),
          ]).then(([schedulesData, externalConfig]) => {
            setSchedules(schedulesData);
            setExternalCliConfig(externalConfig);
          }),
        );
      }
      if (needs.has("history")) {
        tasks.push(
          Promise.all([
            imGatewayApi.listHistoryEvents(),
            imGatewayApi.listHistoryRuns(),
          ]).then(([eventsData, runsData]) => {
            setEvents(eventsData);
            setRuns(runsData);
          }),
        );
      }
      await Promise.all(tasks);
    } catch (err) {
      console.error("Failed to fetch IM Gateway data:", err);
      message.error(err instanceof Error ? err.message : "Failed to fetch IM Gateway data");
    } finally {
      setLoading(false);
    }
  }, [renderedSections]);

  const fetchActiveSectionData = useCallback(async () => {
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
        case "schedules": {
          const [s, p, externalConfig] = await Promise.all([
            imGatewayApi.listSchedules(),
            imGatewayApi.listProviders(),
            imGatewayApi.getExternalCliConfig().catch(() => null),
          ]);
          setSchedules(s);
          setProviders(p);
          setExternalCliConfig(externalConfig);
          break;
        }
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
    if (visibleSections) {
      fetchData();
      return;
    }
    fetchActiveSectionData();
  }, [fetchActiveSectionData, fetchData, visibleSections]);

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
        overflowY: isCompactNav ? undefined : "auto",
        padding: isCompactNav ? "0 0 4px" : 0,
        minHeight: 0,
        maxHeight: "100%",
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

  const renderSectionContent = (section: ImGatewaySectionId) => {
    switch (section) {
      case "connections":
        return (
          <div data-testid="im-gateway-section-connections">
        <ConnectionsPanel
          providers={providers}
          loading={loading}
          onRefresh={fetchData}
          cardGrid={cardGrid}
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
          providers={providers}
          externalCliConfig={externalCliConfig}
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
  };

  const content = visibleSections ? (
    <Space direction="vertical" size={16} style={{ width: "100%" }}>
      {renderedSections.map((section) => (
        <div key={section}>{renderSectionContent(section)}</div>
      ))}
    </Space>
  ) : (
    renderSectionContent(activeSection)
  );

  return (
    <div
      data-testid="im-gateway-layout"
      style={{
        height: hideSectionNav ? undefined : "100%",
        minHeight: 0,
        overflow: hideSectionNav ? "visible" : "hidden",
      }}
    >
      <div
        style={{
          display: "grid",
          gridTemplateColumns:
            hideSectionNav || isCompactNav ? "1fr" : "168px minmax(0, 1fr)",
          gridTemplateRows:
            !hideSectionNav && isCompactNav ? "auto minmax(0, 1fr)" : undefined,
          gap: 16,
          height: hideSectionNav ? undefined : "100%",
          minHeight: 0,
          overflow: hideSectionNav ? "visible" : "hidden",
        }}
      >
        {!hideSectionNav && (
          <div style={{ height: "100%", minHeight: 0, overflow: "hidden" }}>
            {nav}
          </div>
        )}
        <div
          data-testid="im-gateway-section-content"
          style={{
            height: hideSectionNav ? undefined : "100%",
            minHeight: 0,
            overflowY: hideSectionNav ? "visible" : "auto",
            overflowX: "auto",
            paddingRight: isCompactNav ? 0 : 4,
          }}
        >
          {content}
        </div>
      </div>
    </div>
  );
}
