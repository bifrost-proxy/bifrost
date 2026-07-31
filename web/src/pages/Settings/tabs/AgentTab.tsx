/**
 * Agent Tab — Main configuration component
 * Sub-components are in ./agent/ directory
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router-dom";
import {
  Button,
  Card,
  Col,
  Divider,
  Empty,
  Grid,
  Input,
  InputNumber,
  Row,
  Select,
  Space,
  Spin,
  Switch,
  Tag,
  Tooltip,
  Typography,
  message,
  theme,
} from "antd";
import {
  ReloadOutlined,
  ReadOutlined,
  RobotOutlined,
  SettingOutlined,
} from "@ant-design/icons";
import { get, patch } from "../../../api/client";
import {
  BASE,
  type AgentConfig,
} from "./agent/types";
import * as imGatewayApi from "../../../api/imGateway";
import type { ExternalCliGatewayConfig, ImProviderConfig } from "../../../api/imGateway";
import {
  AGENT_SECTION_NAV,
  type AgentSectionId,
} from "./aiSections";
import SkillsSection from "./agent/SkillsSection";
import UnifiedSessionsSection from "./agent/UnifiedSessionsSection";
import SessionDetailPage from "./agent/SessionDetailPage";
import LongTextModalField from "./agent/LongTextModalField";
import ExternalCliPanel from "./imGateway/ExternalCliPanel";
import AgentChatSection from "../../AI/AgentChatSection";

const { Text } = Typography;
const { useBreakpoint } = Grid;

// ─── Main Component ──────────────────────────────────────────────────────────

interface AgentTabProps {
  hideSectionNav?: boolean;
  visibleSections?: AgentSectionId[];
}

export default function AgentTab({ hideSectionNav = false, visibleSections }: AgentTabProps) {
  const [config, setConfig] = useState<AgentConfig | null>(null);
  const [loading, setLoading] = useState(false);
  const [runnerConfig, setRunnerConfig] = useState<ExternalCliGatewayConfig | null>(null);
  const [imProviders, setImProviders] = useState<ImProviderConfig[]>([]);
  const [activeSection, setActiveSection] = useState<AgentSectionId>("general");
  const updateTimers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  const pendingFieldValues = useRef<Record<string, unknown>>({});
  const { token } = theme.useToken();
  const screens = useBreakpoint();
  const isCompactNav = !screens.lg;

  const fetchConfig = useCallback(async () => {
    setLoading(true);
    try {
      const data = await get<AgentConfig>(`${BASE}/agent`);
      setConfig(data);
    } catch {
      message.error("Failed to load agent config");
    } finally {
      setLoading(false);
    }
  }, []);

  const fetchRunnerConfig = useCallback(async () => {
    try {
      const [runnerData, providerData] = await Promise.all([
        imGatewayApi.getExternalCliConfig(),
        imGatewayApi.listProviders(),
      ]);
      setRunnerConfig(runnerData);
      setImProviders(providerData);
    } catch {
      setRunnerConfig(null);
      setImProviders([]);
    }
  }, []);

  useEffect(() => {
    fetchConfig();
    fetchRunnerConfig();
  }, [fetchConfig, fetchRunnerConfig]);

  useEffect(() => {
    const timers = updateTimers.current;
    return () => {
      Object.values(timers).forEach(clearTimeout);
    };
  }, []);

  const patchField = useCallback(
    async (field: string, value: unknown) => {
      pendingFieldValues.current[field] = value;
      try {
        const updated = await patch<AgentConfig>(`${BASE}/agent`, {
          [field]: value,
        });
        setConfig((prev) => {
          const next = { ...updated } as AgentConfig;
          if (Object.is(pendingFieldValues.current[field], value)) {
            delete pendingFieldValues.current[field];
          }
          for (const [pendingField, pendingValue] of Object.entries(
            pendingFieldValues.current,
          )) {
            (next as unknown as Record<string, unknown>)[pendingField] =
              pendingValue === null ? undefined : pendingValue;
          }
          return prev ? next : updated;
        });
        message.success(`Updated ${field.replace(/_/g, " ")}`);
      } catch {
        message.error(`Failed to update ${field.replace(/_/g, " ")}`);
      }
    },
    [],
  );

  const debouncedPatch = useCallback(
    (field: string, value: unknown, delay = 600) => {
      const existing = updateTimers.current[field];
      if (existing) clearTimeout(existing);
      pendingFieldValues.current[field] = value;
      updateTimers.current[field] = setTimeout(() => {
        patchField(field, value);
      }, delay);
    },
    [patchField],
  );

  const handleStringChange = (field: string, value: string) => {
    setConfig((prev) => (prev ? { ...prev, [field]: value } : prev));
    debouncedPatch(field, value, 800);
  };

  const runnerOptions = useMemo(() => {
    const runnerIds = Object.keys(runnerConfig?.runners || {}).sort();
    return runnerIds.map((id) => ({ label: id, value: id }));
  }, [runnerConfig?.runners]);

  const selectedRunnerValue = config?.runner || runnerConfig?.defaultRunnerId || runnerOptions[0]?.value;

  const handleDefaultRunnerChange = async (value: string) => {
    setConfig((prev) => (prev ? { ...prev, runner: value } : prev));
    await patchField("runner", value);
    if (runnerConfig && runnerConfig.defaultRunnerId !== value) {
      try {
        const saved = await imGatewayApi.updateExternalCliConfig({
          ...runnerConfig,
          defaultRunnerId: value,
        });
        setRunnerConfig(saved);
      } catch {
        message.error("Failed to update default runner");
      }
    }
  };

  const handleSwitchChange = (field: string, value: boolean) => {
    setConfig((prev) => (prev ? { ...prev, [field]: value } : prev));
    patchField(field, value);
  };

  // Session detail navigation via URL params
  const [searchParams, setSearchParams] = useSearchParams();
  const selectedSession = searchParams.get("session");
  const sessionView = (searchParams.get("view") || "active") as "active" | "history";
  const historyFilePath = searchParams.get("historyPath") || undefined;
  const sectionFromUrl = searchParams.get("agentSection");

  useEffect(() => {
    const nextSection = AGENT_SECTION_NAV.find(
      (section) => section.id === sectionFromUrl,
    )?.id;
    setActiveSection(hideSectionNav && nextSection === "chat" ? "general" : nextSection ?? "general");
  }, [hideSectionNav, sectionFromUrl]);

  const handleSelectSection = useCallback(
    (section: AgentSectionId) => {
      setActiveSection(section);
      setSearchParams(
        (prev) => {
          prev.set("agentSection", section);
          prev.delete("session");
          prev.delete("view");
          prev.delete("historyPath");
          return prev;
        },
        { replace: false },
      );
    },
    [setSearchParams],
  );

  const handleOpenSession = useCallback(
    (sessionKey: string, view: "active" | "history", filePath?: string) => {
      setSearchParams(
        (prev) => {
          prev.set("session", sessionKey);
          prev.set("view", view);
          if (filePath) prev.set("historyPath", filePath);
          else prev.delete("historyPath");
          return prev;
        },
        { replace: false },
      );
    },
    [setSearchParams],
  );

  const handleBackFromSession = useCallback(() => {
    setSearchParams(
      (prev) => {
        prev.delete("session");
        prev.delete("view");
        prev.delete("historyPath");
        return prev;
      },
      { replace: false },
    );
  }, [setSearchParams]);

  if (loading && !config) {
    return <Spin style={{ display: "block", margin: "60px auto" }} />;
  }

  if (!config) {
    return <Empty description="Unable to load agent configuration" />;
  }

  if (selectedSession && !hideSectionNav) {
    return (
      <SessionDetailPage
        sessionKey={selectedSession}
        view={sessionView}
        historyFilePath={historyFilePath}
        onBack={handleBackFromSession}
      />
    );
  }

  const isSectionVisible = (section: AgentSectionId) =>
    visibleSections ? visibleSections.includes(section) : activeSection === section;

  const nav = (
    <nav
      aria-label="Agent settings sections"
      data-testid="agent-settings-section-nav"
      style={{
        height: isCompactNav ? undefined : "100%",
        zIndex: 1,
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
      {AGENT_SECTION_NAV.map((section) => {
        const active = activeSection === section.id;
        return (
          <button
            key={section.id}
            type="button"
            data-testid={`agent-settings-nav-${section.id}`}
            aria-current={active ? "true" : undefined}
            onClick={() => handleSelectSection(section.id)}
            style={{
              width: isCompactNav ? "auto" : "100%",
              minWidth: isCompactNav ? 112 : undefined,
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
            {section.label}
          </button>
        );
      })}
    </nav>
  );

  return (
    <div
      data-testid="agent-settings-layout"
      style={{
        height: "100%",
        minHeight: 0,
        overflow: "hidden",
      }}
    >
      <Row gutter={[16, 16]} style={{ height: "100%", minHeight: 0, overflow: "hidden" }}>
        {!hideSectionNav && (
          <Col xs={24} lg={5} xl={4} style={{ height: "100%", minHeight: 0 }}>
            {nav}
          </Col>
        )}
        <Col
          xs={24}
          lg={hideSectionNav ? 24 : 19}
          xl={hideSectionNav ? 24 : 20}
          style={{ height: "100%", minHeight: 0 }}
        >
          <div
            data-testid="agent-settings-section-content"
            style={{
              height: "100%",
              minHeight: 0,
              overflowY: "auto",
              overflowX: "hidden",
              paddingRight: isCompactNav ? 0 : 4,
            }}
          >
            <Row gutter={[16, 16]}>
        {/* Chat */}
        {isSectionVisible("chat") && (
        <Col
          xs={24}
          id="agent-settings-chat"
          data-agent-section="chat"
          data-testid="agent-settings-section-chat"
        >
          <AgentChatSection />
        </Col>
        )}

        {/* General Settings */}
        {isSectionVisible("general") && (
        <Col
          xs={24}
          id="agent-settings-general"
          data-agent-section="general"
          data-testid="agent-settings-section-general"
        >
          <Card
            title={
              <Space>
                <RobotOutlined />
                <span>General</span>
              </Space>
            }
            size="small"
            extra={
              <Space>
                <Tag color={config.enabled ? "green" : "default"}>
                  {config.enabled ? "Enabled" : "Disabled"}
                </Tag>
                <Button
                  icon={<ReloadOutlined />}
                  onClick={fetchConfig}
                  loading={loading}
                  size="small"
                >
                  Refresh
                </Button>
              </Space>
            }
          >
            <Space direction="vertical" style={{ width: "100%" }}>
              <Row justify="space-between" align="middle">
                <Col>
                  <Text>Enable Agent</Text>
                </Col>
                <Col>
                  <Switch
                    checked={config.enabled}
                    onChange={(checked) => handleSwitchChange("enabled", checked)}
                  />
                </Col>
              </Row>
              <Text type="secondary" style={{ fontSize: 12 }}>
                Enable or disable external runner sessions
              </Text>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle" gutter={16}>
                <Col flex="none">
                  <Text>Default Runner</Text>
                </Col>
                <Col flex="auto" style={{ textAlign: "right" }}>
                  <Select
                    value={selectedRunnerValue}
                    onChange={(val) => void handleDefaultRunnerChange(val)}
                    options={runnerOptions}
                    style={{ minWidth: 220, maxWidth: 300 }}
                    size="small"
                  />
                </Col>
              </Row>
              <Text type="secondary" style={{ fontSize: 12 }}>
                Selects the default runner for IM agent messages. Custom runners are managed in this Agent Runners section.
              </Text>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle" gutter={16}>
                <Col flex="none">
                  <Text>Working Directory</Text>
                </Col>
                <Col flex="auto" style={{ textAlign: "right" }}>
                  <Input
                    value={config.work_dir || ""}
                    onChange={(e) => handleStringChange("work_dir", e.target.value)}
                    placeholder="/path/to/workdir"
                    style={{ maxWidth: 480 }}
                    size="small"
                  />
                </Col>
              </Row>
              <Text type="secondary" style={{ fontSize: 12 }}>
                Default working directory passed to external runners
              </Text>

              <Divider style={{ margin: "12px 0" }} />

              <LongTextModalField
                label="Base Instructions / System Prompt"
                value={config.base_instructions ?? ""}
                onChange={(value) => handleStringChange("base_instructions", value)}
                placeholder="Optional instructions for external runners"
                description="Sent only with the first message of a new external-runner session. When empty, Bifrost adds nothing."
                testId="settings-agent-base-instructions"
              />

              <Divider style={{ margin: "12px 0" }} />

              <LongTextModalField
                label="Developer Instructions"
                value={config.developer_instructions || ""}
                onChange={(value) => handleStringChange("developer_instructions", value)}
                placeholder="Optional developer-level instructions"
                description="Sent with every external-runner message after the optional base instructions. Empty values add nothing."
                testId="settings-agent-developer-instructions"
              />

              <Divider style={{ margin: "12px 0" }} />

              <LongTextModalField
                label="User Instructions"
                value={config.user_instructions || ""}
                onChange={(value) => handleStringChange("user_instructions", value)}
                placeholder="Optional user-level instructions"
                description="Sent with every external-runner message after developer instructions. Empty values add nothing."
                testId="settings-agent-user-instructions"
              />
            </Space>
          </Card>
        </Col>
        )}

        {/* History & Session */}
        {isSectionVisible("history") && (
        <Col
          xs={24}
          id="agent-settings-history"
          data-agent-section="history"
          data-testid="agent-settings-section-history"
        >
          <Card
            title={
              <Space>
                <SettingOutlined />
                <span>History & Session</span>
              </Space>
            }
            size="small"
          >
            <Space direction="vertical" style={{ width: "100%" }}>
              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="When enabled, session is not persisted on disk">
                    <Text>Ephemeral Mode</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <Switch
                    checked={config.ephemeral ?? false}
                    onChange={(checked) => {
                      setConfig((prev) =>
                        prev ? { ...prev, ephemeral: checked } : prev,
                      );
                      patchField("ephemeral", checked);
                    }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Text>History Persistence</Text>
                </Col>
                <Col>
                  <Select
                    value={config.history?.persistence ?? "save-all"}
                    onChange={(val) => {
                      const newHistory = {
                        ...(config.history ?? {}),
                        persistence: val,
                      };
                      setConfig((prev) =>
                        prev ? { ...prev, history: newHistory } : prev,
                      );
                      patchField("history", newHistory);
                    }}
                    options={[
                      { label: "Save All", value: "save-all" },
                      { label: "Last 90 Days", value: "last-90-days" },
                      { label: "None", value: "none" },
                    ]}
                    style={{ width: 140 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Maximum history file size in bytes. Oldest entries dropped when exceeded.">
                    <Text>History Max Bytes</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <InputNumber
                    value={config.history?.max_bytes}
                    onChange={(val) => {
                      const newHistory = {
                        ...(config.history ?? {}),
                        max_bytes: val ?? undefined,
                      };
                      setConfig((prev) =>
                        prev ? { ...prev, history: newHistory } : prev,
                      );
                      patchField("history", newHistory);
                    }}
                    min={0}
                    step={1048576}
                    placeholder="Unlimited"
                    style={{ width: 140 }}
                    size="small"
                  />
                </Col>
              </Row>
            </Space>
          </Card>
        </Col>
        )}

        {/* Skills */}
        {isSectionVisible("skills") && (
        <Col
          xs={24}
          id="agent-settings-skills"
          data-agent-section="skills"
          data-testid="agent-settings-section-skills"
        >
          <Card
            title={
              <Space>
                <ReadOutlined />
                <span>Skills</span>
              </Space>
            }
            size="small"
          >
            <SkillsSection />
          </Card>
        </Col>
        )}

        {/* Runners */}
        {isSectionVisible("runners") && (
        <Col
          xs={24}
          id="agent-settings-runners"
          data-agent-section="runners"
          data-testid="agent-settings-section-runners"
        >
          <ExternalCliPanel
            providers={imProviders}
            loading={loading}
            onRefresh={fetchRunnerConfig}
          />
        </Col>
        )}

        {/* Sessions (Unified — Active + History) */}
        {isSectionVisible("sessions") && (
        <Col
          xs={24}
          id="agent-settings-sessions"
          data-agent-section="sessions"
          data-testid="agent-settings-section-sessions"
        >
          <Card
            title={
              <Space>
                <RobotOutlined />
                <span>Sessions</span>
              </Space>
            }
            size="small"
          >
            <UnifiedSessionsSection onOpenSession={handleOpenSession} />
          </Card>
        </Col>
        )}
            </Row>
          </div>
        </Col>
      </Row>
    </div>
  );
}
