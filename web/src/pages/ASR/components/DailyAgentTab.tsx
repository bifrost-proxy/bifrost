import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Descriptions,
  Input,
  InputNumber,
  message,
  Popconfirm,
  Select,
  Space,
  Switch,
  Table,
  Tag,
  Tooltip,
  Typography,
} from "antd";
import {
  ArrowLeftOutlined,
  DeleteOutlined,
  EditOutlined,
  PlayCircleOutlined,
  PlusOutlined,
  ReloadOutlined,
  SaveOutlined,
  SendOutlined,
  SyncOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import { useSearchParams } from "react-router-dom";
import type {
  AsrDailyAgentConfig,
  AsrDailyAgentConfigResponse,
  AsrDailyAgentInstructionsResponse,
  AsrDailyAgentItem,
  AsrDailyAgentTemplate,
} from "../../../api/asr";
import { hasRunningDailyAgent } from "../asrUtils";
import { DailyAgentResearchFields } from "./DailyAgentResearchFields";
import * as imGatewayApi from "../../../api/imGateway";
import type {
  ExternalCliGatewayConfig,
  ImProviderConfig,
  ImTarget,
} from "../../../api/imGateway";
import {
  applyDailyAgentTemplate,
  getDailyAgentConfig,
  getDailyAgentTemplates,
  getDailyAgentInstructions,
  sendDailyAgentReport,
  syncDailyAgentReports,
  triggerDailyAgentRun,
  updateDailyAgentConfig,
  updateDailyAgentInstructions,
} from "../../../api/asr";

const { Text } = Typography;
const { TextArea } = Input;
const DAILY_AGENT_TOKEN_RE = /^[A-Za-z0-9_-]+$/;

function normalizeAgentToken(value: string): string {
  return value
    .trim()
    .replace(/[^A-Za-z0-9_-]/g, "_")
    .replace(/^[_-]+|[_-]+$/g, "");
}

interface DailyAgentTabProps {
  taskId: string;
  onOpenReport?: (date: string, agentId?: string) => void;
}

interface ImChannelOption {
  label: string;
  value: string;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export default function DailyAgentTab({ taskId }: DailyAgentTabProps) {
  const [searchParams, setSearchParams] = useSearchParams();
  const routeDetailAgentId = searchParams.get("asrDailyAgentEdit");
  const [configData, setConfigData] =
    useState<AsrDailyAgentConfigResponse | null>(null);
  const [instructions, setInstructions] =
    useState<AsrDailyAgentInstructionsResponse | null>(null);
  const [runnerConfig, setRunnerConfig] =
    useState<ExternalCliGatewayConfig | null>(null);
  const [imProviders, setImProviders] = useState<ImProviderConfig[]>([]);
  const [imTargets, setImTargets] = useState<ImTarget[]>([]);
  const [templates, setTemplates] = useState<AsrDailyAgentTemplate[]>([]);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [runningAgentId, setRunningAgentId] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [instructionsText, setInstructionsText] = useState("");
  const [instructionsDirty, setInstructionsDirty] = useState(false);
  const [selectedAgentId, setSelectedAgentId] =
    useState<string>("daily_report");
  const [detailAgentId, setDetailAgentId] = useState<string | null>(null);
  const [reportSyncDir, setReportSyncDir] = useState("");
  const [reportSyncDirDirty, setReportSyncDirDirty] = useState(false);
  const [autoProcessFromDate, setAutoProcessFromDate] = useState("");
  const [autoProcessFromDateDirty, setAutoProcessFromDateDirty] =
    useState(false);
  const [terminology, setTerminology] = useState("");
  const [terminologyDirty, setTerminologyDirty] = useState(false);

  const fetchAll = useCallback(async () => {
    setLoading(true);
    try {
      const [config, runners, providers, targets, templateData] =
        await Promise.all([
          getDailyAgentConfig(taskId),
          imGatewayApi.getExternalCliConfig(),
          imGatewayApi.listProviders(),
          imGatewayApi.listTargets(),
          getDailyAgentTemplates(),
        ]);
      const agents = config.config.agents || [];
      const preferredAgentId = routeDetailAgentId || selectedAgentId;
      const nextSelectedAgentId = agents.some(
        (agent) => agent.id === preferredAgentId,
      )
        ? preferredAgentId
        : agents[0]?.id || "daily_report";
      const instr = await getDailyAgentInstructions(
        taskId,
        nextSelectedAgentId,
      );
      setConfigData(config);
      setInstructions(instr);
      setSelectedAgentId(nextSelectedAgentId);
      setInstructionsText(instr.content);
      setInstructionsDirty(false);
      setReportSyncDir(config.config.report_sync_dir || "");
      setReportSyncDirDirty(false);
      setAutoProcessFromDate(config.config.auto_process_from_date || "");
      setAutoProcessFromDateDirty(false);
      setTerminology(config.config.terminology || "");
      setTerminologyDirty(false);
      setRunnerConfig(runners);
      setImProviders(providers);
      setImTargets(targets);
      setTemplates(templateData.templates);
    } catch (error: unknown) {
      message.error(
        `Failed to load Daily Agent config: ${errorMessage(error)}`,
      );
    } finally {
      setLoading(false);
    }
  }, [routeDetailAgentId, selectedAgentId, taskId]);

  useEffect(() => {
    fetchAll();
  }, [fetchAll]);

  useEffect(() => {
    setDetailAgentId(routeDetailAgentId);
    if (routeDetailAgentId) {
      setSelectedAgentId(routeDetailAgentId);
    }
  }, [routeDetailAgentId]);

  const handleConfigUpdate = async (updates: Partial<AsrDailyAgentConfig>) => {
    setSaving(true);
    try {
      const result = await updateDailyAgentConfig(taskId, updates);
      if (result.ok) {
        message.success("Configuration saved");
        fetchAll();
      }
    } catch (error: unknown) {
      message.error(`Failed to save: ${errorMessage(error)}`);
    } finally {
      setSaving(false);
    }
  };

  const runnerOptions = useMemo(() => {
    const runnerIds = Object.keys(runnerConfig?.runners || {}).sort();
    return runnerIds.map((id) => ({ label: id, value: id }));
  }, [runnerConfig?.runners]);

  const agents = useMemo(
    () => configData?.config.agents || [],
    [configData?.config.agents],
  );

  const selectedAgent = useMemo(
    () => agents.find((agent) => agent.id === selectedAgentId) || agents[0],
    [agents, selectedAgentId],
  );

  const selectedWorkspaceAgent = useMemo(
    () =>
      configData?.workspace?.agents?.find(
        (agent) => agent.agent_id === selectedAgent?.id,
      ),
    [configData?.workspace?.agents, selectedAgent?.id],
  );

  const loadAgentInstructions = useCallback(
    async (agentId: string) => {
      const instr = await getDailyAgentInstructions(taskId, agentId);
      setInstructions(instr);
      setInstructionsText(instr.content);
      setInstructionsDirty(false);
    },
    [taskId],
  );

  const openAgentDetail = async (agentId: string) => {
    setSelectedAgentId(agentId);
    setDetailAgentId(agentId);
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.set("aiSection", "tools-asr");
        next.set("asrDailyAgentEdit", agentId);
        next.set("asrTaskTab", "daily-agent");
        return next;
      },
      { replace: false },
    );
    try {
      await loadAgentInstructions(agentId);
    } catch (error: unknown) {
      message.error(`Failed to load instructions: ${errorMessage(error)}`);
    }
  };

  const closeAgentDetail = () => {
    setDetailAgentId(null);
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.delete("asrDailyAgentEdit");
        next.set("asrTaskTab", "daily-agent");
        return next;
      },
      { replace: false },
    );
  };

  const saveAgents = async (nextAgents: AsrDailyAgentItem[]) => {
    await handleConfigUpdate({ agents: nextAgents });
  };

  const updateAgent = (agentId: string, patch: Partial<AsrDailyAgentItem>) => {
    const nextAgents = agents.map((agent) =>
      agent.id === agentId ? { ...agent, ...patch } : agent,
    );
    void saveAgents(nextAgents);
  };

  const addAgent = () => {
    const base = "custom_agent";
    let index = agents.length + 1;
    let id = `${base}_${index}`;
    while (agents.some((agent) => agent.id === id)) {
      index += 1;
      id = `${base}_${index}`;
    }
    const template = agents[0] || configData?.config;
    if (!template) return;
    const nextAgent: AsrDailyAgentItem = {
      id,
      name: id,
      enabled: true,
      runner:
        template.runner ||
        runnerConfig?.defaultRunnerId ||
        runnerOptions[0]?.value ||
        "Codex",
      timeout_ms: template.timeout_ms || 7_200_000,
      trigger_policy: "after_asr_run",
      instructions_source: "default",
      im_delivery: {
        enabled: false,
        mode: "full_report",
        send_policy: "on_success_with_report",
      },
      output_dir: id,
      dependencies: [],
      dependency_failure_policy: "skip",
    };
    setSelectedAgentId(id);
    void saveAgents([...agents, nextAgent]);
  };

  const removeAgent = (agentId: string) => {
    if (agents.length <= 1) {
      message.warning("At least one Daily Agent is required");
      return;
    }
    const nextAgents = agents.filter((agent) => agent.id !== agentId);
    if (selectedAgentId === agentId) {
      setSelectedAgentId(nextAgents[0]?.id || "daily_report");
    }
    if (detailAgentId === agentId) {
      closeAgentDetail();
    }
    void saveAgents(nextAgents);
  };

  const imChannelOptions = useMemo<ImChannelOption[]>(() => {
    const providerById = new Map(
      imProviders.map((provider) => [provider.id, provider]),
    );
    const ownerOptions = imProviders
      .filter((provider) => provider.owner_open_id?.trim())
      .map((provider) => ({
        label: `${provider.display_name || provider.id} / Owner`,
        value: `owner:${provider.id}`,
      }));
    const targetOptions = imTargets
      .filter((target) => target.enabled)
      .map((target) => {
        const provider = providerById.get(target.provider_id);
        return {
          label: `${target.display_name || target.id} (${
            provider?.display_name || target.provider_id
          })`,
          value: `target:${target.id}`,
        };
      });
    return [...ownerOptions, ...targetOptions];
  }, [imProviders, imTargets]);

  const handleSaveInstructions = async () => {
    setSaving(true);
    try {
      await updateDailyAgentInstructions(
        taskId,
        instructionsText,
        selectedAgent?.id,
      );
      message.success("Instructions saved");
      setInstructionsDirty(false);
    } catch (error: unknown) {
      message.error(`Failed to save instructions: ${errorMessage(error)}`);
    } finally {
      setSaving(false);
    }
  };

  const handleSaveReportSyncDir = async () => {
    setSaving(true);
    try {
      await updateDailyAgentConfig(taskId, { report_sync_dir: reportSyncDir });
      message.success("Report sync directory saved");
      setReportSyncDirDirty(false);
      fetchAll();
    } catch (error: unknown) {
      message.error(`Failed to save sync directory: ${errorMessage(error)}`);
    } finally {
      setSaving(false);
    }
  };

  const handleSaveAutoProcessFromDate = async () => {
    setSaving(true);
    try {
      await updateDailyAgentConfig(taskId, {
        auto_process_from_date: autoProcessFromDate,
      });
      message.success("Automatic processing start date saved");
      setAutoProcessFromDateDirty(false);
      fetchAll();
    } catch (error: unknown) {
      message.error(`Failed to save start date: ${errorMessage(error)}`);
    } finally {
      setSaving(false);
    }
  };

  const handleSaveTerminology = async () => {
    setSaving(true);
    try {
      await updateDailyAgentConfig(taskId, { terminology });
      message.success("Terminology saved");
      setTerminologyDirty(false);
      fetchAll();
    } catch (error: unknown) {
      message.error(`Failed to save terminology: ${errorMessage(error)}`);
    } finally {
      setSaving(false);
    }
  };

  const handleApplyTemplate = async (templateId: string) => {
    setSaving(true);
    try {
      await applyDailyAgentTemplate(taskId, templateId);
      message.success("Official Daily Research template applied");
      setDetailAgentId(null);
      await fetchAll();
    } catch (error: unknown) {
      message.error(`Failed to apply template: ${errorMessage(error)}`);
    } finally {
      setSaving(false);
    }
  };

  const handleSyncReports = async () => {
    setSyncing(true);
    try {
      const result = await syncDailyAgentReports(taskId);
      message.success(
        `Synced ${result.sync.copied_files} copied, ${result.sync.skipped_files} skipped`,
      );
      fetchAll();
    } catch (error: unknown) {
      message.error(`Sync failed: ${errorMessage(error)}`);
      fetchAll();
    } finally {
      setSyncing(false);
    }
  };

  const handleRun = async (force: boolean, agentId: string) => {
    setRunningAgentId(agentId);
    try {
      const result = await triggerDailyAgentRun(taskId, { force, agentId });
      message.success(result.message || "Run queued");
      // Poll status until no longer running
      const pollInterval = setInterval(async () => {
        try {
          const config = await getDailyAgentConfig(taskId);
          setConfigData(config);
          if (!hasRunningDailyAgent(config.config, agentId)) {
            clearInterval(pollInterval);
            setRunningAgentId(null);
            fetchAll();
          }
        } catch {
          clearInterval(pollInterval);
          setRunningAgentId(null);
        }
      }, 3000);
      // Safety timeout: use the agent's configured timeout_ms plus a 60s buffer
      const agent = agents.find((a) => a.id === agentId);
      const safetyTimeoutMs = (agent?.timeout_ms || 7_200_000) + 60_000;
      setTimeout(() => {
        clearInterval(pollInterval);
        setRunningAgentId(null);
        fetchAll();
      }, safetyTimeoutMs);
    } catch (error: unknown) {
      message.error(`Run failed: ${errorMessage(error)}`);
      setRunningAgentId(null);
    }
  };

  const handleSend = async () => {
    try {
      const result = await sendDailyAgentReport(taskId, selectedAgent?.id);
      if (result.ok) {
        message.success(`Sent ${result.sent_reports.length} report(s)`);
      }
    } catch (error: unknown) {
      message.error(`Send failed: ${errorMessage(error)}`);
    }
  };

  const config = configData?.config;
  const reportIndex = configData?.report_index_status;
  const activeAgentId = selectedAgent?.id || selectedAgentId;
  const reportSync =
    selectedAgent?.last_report_sync || config?.last_report_sync;
  const formatTimestamp = (value?: number) =>
    value ? new Date(value).toLocaleString() : "Never";

  if (loading && !configData) {
    return (
      <div style={{ padding: 24, textAlign: "center" }}>
        <Text type="secondary">Loading Daily Agent...</Text>
      </div>
    );
  }

  if (detailAgentId && selectedAgent) {
    return (
      <Space
        data-testid="asr-daily-agent-detail"
        direction="vertical"
        size={16}
        style={{ width: "100%" }}
      >
        <Space style={{ justifyContent: "space-between", width: "100%" }}>
          <Space>
            <Button icon={<ArrowLeftOutlined />} onClick={closeAgentDetail}>
              Daily Agents
            </Button>
            <Text strong>{selectedAgent.name}</Text>
            <Text code>{selectedAgent.id}</Text>
          </Space>
          <Button
            icon={<ReloadOutlined />}
            onClick={fetchAll}
            loading={loading}
          >
            Refresh
          </Button>
        </Space>

        <Card
          size="small"
          title="Agent Configuration"
          loading={loading && !config}
        >
          <Descriptions column={2} size="small" bordered>
            <Descriptions.Item label="Enabled">
              <Switch
                checked={selectedAgent.enabled}
                onChange={(enabled) =>
                  updateAgent(selectedAgent.id, { enabled })
                }
                loading={saving}
              />
            </Descriptions.Item>
            <Descriptions.Item label="Runner">
              <Select
                data-testid="asr-daily-agent-runner-select"
                size="small"
                value={selectedAgent.runner || undefined}
                placeholder="Select runner..."
                onChange={(runner) => updateAgent(selectedAgent.id, { runner })}
                loading={loading}
                disabled={saving}
                style={{ width: 240 }}
                options={runnerOptions}
              />
            </Descriptions.Item>
            <Descriptions.Item label="Agent Name">
              <Input
                size="small"
                value={selectedAgent.name}
                status={
                  DAILY_AGENT_TOKEN_RE.test(selectedAgent.name)
                    ? undefined
                    : "error"
                }
                onChange={(event) =>
                  updateAgent(selectedAgent.id, {
                    name: normalizeAgentToken(event.target.value),
                  })
                }
              />
            </Descriptions.Item>
            <Descriptions.Item label="Output Dir">
              <Input
                size="small"
                value={selectedAgent.output_dir}
                status={
                  DAILY_AGENT_TOKEN_RE.test(selectedAgent.output_dir)
                    ? undefined
                    : "error"
                }
                onChange={(event) =>
                  updateAgent(selectedAgent.id, {
                    output_dir: normalizeAgentToken(event.target.value),
                  })
                }
              />
            </Descriptions.Item>
            <Descriptions.Item label="Trigger Policy">
              <Select
                size="small"
                value={selectedAgent.trigger_policy}
                onChange={(trigger_policy) =>
                  updateAgent(selectedAgent.id, { trigger_policy })
                }
                style={{ width: 160 }}
                options={[
                  { label: "After ASR Run", value: "after_asr_run" },
                  { label: "Manual Only", value: "manual_only" },
                ]}
              />
            </Descriptions.Item>
            <Descriptions.Item label="Timeout">
              <InputNumber
                size="small"
                min={1}
                step={5}
                addonAfter="min"
                value={Math.round(selectedAgent.timeout_ms / 60000)}
                onChange={(minutes) =>
                  updateAgent(selectedAgent.id, {
                    timeout_ms: Number(minutes || 1) * 60000,
                  })
                }
                style={{ width: 150 }}
              />
            </Descriptions.Item>
            <Descriptions.Item label="Session Key">
              <Input
                size="small"
                value={selectedAgent.session_key || ""}
                placeholder="(auto)"
                onChange={(event) =>
                  updateAgent(selectedAgent.id, {
                    session_key: event.target.value.trim() || undefined,
                  })
                }
              />
            </Descriptions.Item>
            <Descriptions.Item label="Dependencies" span={2}>
              <Space direction="vertical" size={8} style={{ width: "100%" }}>
                <Select
                  data-testid="asr-daily-agent-dependencies-select"
                  mode="multiple"
                  allowClear
                  value={(selectedAgent.dependencies || []).map(
                    (dependency) => dependency.agent_id,
                  )}
                  placeholder="Select upstream agents..."
                  options={agents
                    .filter((agent) => agent.id !== selectedAgent.id)
                    .map((agent) => ({ label: agent.name, value: agent.id }))}
                  onChange={(dependencyIds: string[]) => {
                    const existing = new Map(
                      (selectedAgent.dependencies || []).map((dependency) => [
                        dependency.agent_id,
                        dependency,
                      ]),
                    );
                    updateAgent(selectedAgent.id, {
                      dependencies: dependencyIds.map(
                        (agent_id) =>
                          existing.get(agent_id) || {
                            agent_id,
                            include_output: true,
                          },
                      ),
                    });
                  }}
                  style={{ width: "100%" }}
                />
                {(selectedAgent.dependencies || []).map((dependency) => (
                  <Space key={dependency.agent_id}>
                    <Text code>{dependency.agent_id}</Text>
                    <Text type="secondary">Include output</Text>
                    <Switch
                      size="small"
                      checked={dependency.include_output !== false}
                      onChange={(include_output) =>
                        updateAgent(selectedAgent.id, {
                          dependencies: (selectedAgent.dependencies || []).map(
                            (item) =>
                              item.agent_id === dependency.agent_id
                                ? { ...item, include_output }
                                : item,
                          ),
                        })
                      }
                    />
                  </Space>
                ))}
              </Space>
            </Descriptions.Item>
            <Descriptions.Item label="Dependency Failure">
              <Select
                data-testid="asr-daily-agent-dependency-failure-policy"
                size="small"
                value={selectedAgent.dependency_failure_policy || "skip"}
                onChange={(dependency_failure_policy) =>
                  updateAgent(selectedAgent.id, {
                    dependency_failure_policy,
                  })
                }
                style={{ width: 180 }}
                options={[
                  { label: "Skip this agent", value: "skip" },
                  { label: "Continue", value: "continue" },
                ]}
              />
            </Descriptions.Item>
            <Descriptions.Item label="Instructions Source">
              <Tag>
                {instructions?.source || selectedAgent.instructions_source}
              </Tag>
            </Descriptions.Item>
          </Descriptions>
          <DailyAgentResearchFields
            agent={selectedAgent}
            runnerOptions={runnerOptions}
            onChange={(patch) => updateAgent(selectedAgent.id, patch)}
          />
          {selectedAgent.research_fanout ? (
            <Alert
              style={{ marginTop: 12 }}
              type="info"
              showIcon
              message="Each research question runs in its own conversation"
              description="The upstream dispatcher must output a research manifest. Independent questions run concurrently up to the configured limit and keep manifest order in the final digest. ChatGPT Web research runs are forced to Chat mode with the Pro model before a question is sent. When a Project URL is configured, every new research conversation is created inside that ChatGPT Project. GitHub access still requires the user's Connector authorization."
            />
          ) : null}
        </Card>

        <Card size="small" title="IM Delivery" loading={loading && !config}>
          <Descriptions column={2} size="small" bordered>
            <Descriptions.Item label="Enabled">
              <Switch
                checked={selectedAgent.im_delivery.enabled}
                onChange={(enabled) =>
                  updateAgent(selectedAgent.id, {
                    im_delivery: { ...selectedAgent.im_delivery, enabled },
                  })
                }
                loading={saving}
              />
            </Descriptions.Item>
            <Descriptions.Item label="Channel">
              <Select
                data-testid="asr-daily-agent-im-channel-select"
                size="small"
                allowClear
                value={selectedAgent.im_delivery.channel || undefined}
                placeholder="Select channel..."
                onChange={(channel) =>
                  updateAgent(selectedAgent.id, {
                    im_delivery: {
                      ...selectedAgent.im_delivery,
                      channel: channel || "",
                    },
                  })
                }
                loading={loading}
                disabled={saving}
                style={{ width: 260 }}
                options={imChannelOptions}
              />
            </Descriptions.Item>
            <Descriptions.Item label="Mode">
              <Select
                size="small"
                value={selectedAgent.im_delivery.mode}
                onChange={(mode) =>
                  updateAgent(selectedAgent.id, {
                    im_delivery: { ...selectedAgent.im_delivery, mode },
                  })
                }
                style={{ width: 140 }}
                options={[
                  { label: "Full Report", value: "full_report" },
                  { label: "Summary", value: "summary" },
                ]}
              />
            </Descriptions.Item>
            <Descriptions.Item label="Send Policy">
              <Select
                size="small"
                value={selectedAgent.im_delivery.send_policy}
                onChange={(send_policy) =>
                  updateAgent(selectedAgent.id, {
                    im_delivery: {
                      ...selectedAgent.im_delivery,
                      send_policy,
                    },
                  })
                }
                style={{ width: 220 }}
                options={[
                  {
                    label: "On success with report",
                    value: "on_success_with_report",
                  },
                  { label: "On success", value: "on_success" },
                  { label: "Always", value: "always" },
                ]}
              />
            </Descriptions.Item>
            {selectedAgent.im_delivery.last_send_error && (
              <Descriptions.Item label="Last Send Error" span={2}>
                <Alert
                  type="error"
                  showIcon
                  message={selectedAgent.im_delivery.last_send_error}
                />
              </Descriptions.Item>
            )}
          </Descriptions>
        </Card>

        <Card size="small" title="Last Run Status">
          <Descriptions column={3} size="small">
            <Descriptions.Item label="Status">
              <Tag
                color={
                  selectedAgent.last_status === "success"
                    ? "green"
                    : selectedAgent.last_status === "failed"
                      ? "red"
                      : selectedAgent.last_status ===
                          "skipped_dependency_failed"
                        ? "orange"
                        : "default"
                }
              >
                {selectedAgent.last_status || "never"}
              </Tag>
            </Descriptions.Item>
            <Descriptions.Item label="Run ID">
              <Text code style={{ fontSize: 11 }}>
                {selectedAgent.last_run_id?.slice(0, 16) || "-"}
              </Text>
            </Descriptions.Item>
            <Descriptions.Item label="Last Run">
              <Text>{formatTimestamp(selectedAgent.last_run_at_ms)}</Text>
            </Descriptions.Item>
            <Descriptions.Item label="Reports">
              {selectedWorkspaceAgent?.report_count ?? 0}
            </Descriptions.Item>
            <Descriptions.Item label="Output Dir">
              <Text code>
                {selectedWorkspaceAgent?.output_dir || selectedAgent.output_dir}
              </Text>
            </Descriptions.Item>
            <Descriptions.Item label="Instructions File">
              <Tag
                color={
                  selectedWorkspaceAgent?.instructions_exists
                    ? "green"
                    : "orange"
                }
              >
                {selectedWorkspaceAgent?.instructions_exists
                  ? "Ready"
                  : "Missing"}
              </Tag>
            </Descriptions.Item>
            {selectedAgent.last_error && (
              <Descriptions.Item label="Error" span={3}>
                <Alert
                  type="error"
                  message={selectedAgent.last_error}
                  showIcon
                />
              </Descriptions.Item>
            )}
          </Descriptions>
          {reportSync && (
            <>
              <Descriptions column={3} size="small" style={{ marginTop: 8 }}>
                <Descriptions.Item label="Report Sync">
                  <Tag color={reportSync.failed_files > 0 ? "red" : "green"}>
                    {reportSync.copied_files} copied / {reportSync.total_files}{" "}
                    total
                  </Tag>
                </Descriptions.Item>
                <Descriptions.Item label="Skipped">
                  {reportSync.skipped_files}
                </Descriptions.Item>
                <Descriptions.Item label="Last Sync">
                  {formatTimestamp(reportSync.synced_at_ms)}
                </Descriptions.Item>
              </Descriptions>
              {reportSync.failed_files > 0 && (
                <Alert
                  style={{ marginTop: 8 }}
                  type="error"
                  showIcon
                  message={`Report sync failed for ${reportSync.failed_files} file(s)`}
                  description={(reportSync.errors || []).slice(0, 3).join("; ")}
                />
              )}
            </>
          )}
          {reportIndex && (
            <Descriptions column={3} size="small" style={{ marginTop: 8 }}>
              <Descriptions.Item label="Indexed Reports">
                <Tag
                  color={reportIndex.unindexed_reports > 0 ? "orange" : "green"}
                >
                  {reportIndex.indexed_reports}/{reportIndex.report_files}
                </Tag>
              </Descriptions.Item>
              <Descriptions.Item label="Unindexed Reports">
                <Tag
                  color={reportIndex.unindexed_reports > 0 ? "orange" : "green"}
                >
                  {reportIndex.unindexed_reports}
                </Tag>
              </Descriptions.Item>
              <Descriptions.Item label="Missing Reports">
                <Tag
                  color={
                    reportIndex.processed_missing_report > 0 ? "red" : "green"
                  }
                >
                  {reportIndex.processed_missing_report}
                </Tag>
              </Descriptions.Item>
            </Descriptions>
          )}
        </Card>

        <Space>
          <Tooltip title="Incremental run: only this agent's Daily Agent state changes are processed.">
            <Button
              type="primary"
              icon={<PlayCircleOutlined />}
              onClick={() => handleRun(false, activeAgentId)}
              loading={runningAgentId === activeAgentId}
              disabled={!config?.enabled || !selectedAgent.enabled}
            >
              Run Now
            </Button>
          </Tooltip>
          <Tooltip title="Force run: rebuild all matching reports for this agent.">
            <Button
              icon={<ThunderboltOutlined />}
              onClick={() => handleRun(true, activeAgentId)}
              loading={runningAgentId === activeAgentId}
              disabled={!config?.enabled || !selectedAgent.enabled}
            >
              Force Run
            </Button>
          </Tooltip>
          <Tooltip title="Send this agent's latest report via IM">
            <Button
              icon={<SendOutlined />}
              onClick={handleSend}
              disabled={!selectedAgent.im_delivery.enabled}
            >
              Send Report
            </Button>
          </Tooltip>
        </Space>

        <Card
          size="small"
          title={`Custom Prompt / Instructions (${selectedAgent.name || activeAgentId})`}
          extra={
            <Button
              type="primary"
              size="small"
              icon={<SaveOutlined />}
              onClick={handleSaveInstructions}
              loading={saving}
              disabled={!instructionsDirty}
            >
              Save
            </Button>
          }
        >
          <TextArea
            data-testid="asr-daily-agent-instructions"
            value={instructionsText}
            onChange={(e) => {
              setInstructionsText(e.target.value);
              setInstructionsDirty(true);
            }}
            autoSize={{ minRows: 12 }}
            classNames={{
              textarea: "asr-daily-agent-instructions-textarea",
            }}
            styles={{
              textarea: {
                fontFamily: "monospace",
                fontSize: 12,
              },
            }}
            placeholder="Define this user's domain rules, tool routing, repository mapping, and output requirements..."
          />
          {instructions?.source && (
            <Text
              type="secondary"
              style={{ fontSize: 11, marginTop: 4, display: "block" }}
            >
              Source: {instructions.source}
            </Text>
          )}
        </Card>
      </Space>
    );
  }

  return (
    <Space direction="vertical" size={16} style={{ width: "100%" }}>
      <Card size="small" title="Daily Agents" loading={loading && !config}>
        <Space
          direction="vertical"
          size={8}
          style={{ width: "100%", marginBottom: 12 }}
        >
          <Space
            style={{
              justifyContent: "space-between",
              width: "100%",
            }}
          >
            <Space wrap>
              <Text type="secondary">Task</Text>
              <Switch
                checked={config?.enabled}
                onChange={(enabled) => handleConfigUpdate({ enabled })}
                loading={saving}
                size="small"
              />
              <Space.Compact style={{ width: 440 }}>
                <Input
                  data-testid="asr-daily-agent-report-sync-dir"
                  size="small"
                  value={reportSyncDir}
                  placeholder="Optional report sync directory"
                  onChange={(event) => {
                    setReportSyncDir(event.target.value);
                    setReportSyncDirDirty(true);
                  }}
                  onPressEnter={handleSaveReportSyncDir}
                  disabled={saving}
                />
                <Button
                  data-testid="asr-daily-agent-report-sync-dir-save"
                  size="small"
                  icon={<SaveOutlined />}
                  onClick={handleSaveReportSyncDir}
                  disabled={!reportSyncDirDirty}
                  loading={saving}
                >
                  Save
                </Button>
              </Space.Compact>
              <Button
                data-testid="asr-daily-agent-sync-reports-button"
                size="small"
                icon={<SyncOutlined />}
                onClick={handleSyncReports}
                loading={syncing}
                disabled={!config?.report_sync_dir?.trim()}
              >
                Sync Reports
              </Button>
            </Space>
            <Button
              icon={<ReloadOutlined />}
              onClick={fetchAll}
              loading={loading}
            >
              Refresh
            </Button>
          </Space>
          <div
            style={{
              alignItems: "center",
              display: "flex",
              flexWrap: "wrap",
              gap: 8,
              width: "100%",
            }}
          >
            <Text type="secondary" style={{ width: 160, lineHeight: "24px" }}>
              Auto process from
            </Text>
            <Space.Compact>
              <Input
                data-testid="asr-daily-agent-auto-process-from-date"
                type="date"
                size="small"
                value={autoProcessFromDate}
                onChange={(event) => {
                  setAutoProcessFromDate(event.target.value);
                  setAutoProcessFromDateDirty(true);
                }}
                onPressEnter={handleSaveAutoProcessFromDate}
                disabled={saving}
                style={{ width: 180 }}
              />
              <Button
                data-testid="asr-daily-agent-auto-process-from-date-save"
                size="small"
                icon={<SaveOutlined />}
                onClick={handleSaveAutoProcessFromDate}
                disabled={!autoProcessFromDateDirty}
                loading={saving}
              >
                Save
              </Button>
            </Space.Compact>
            <Text type="secondary" style={{ fontSize: 11 }}>
              Earlier dates are ignored by automatic ASR completion runs;
              explicit manual dates remain available.
            </Text>
          </div>
          <div
            style={{
              alignItems: "flex-start",
              display: "flex",
              flexWrap: "wrap",
              gap: 8,
              width: "100%",
            }}
          >
            <Text type="secondary" style={{ width: 88, lineHeight: "24px" }}>
              Terminology
            </Text>
            <TextArea
              data-testid="asr-daily-agent-terminology"
              value={terminology}
              placeholder="Project names, people, abbreviations, fixed translations..."
              autoSize={{ minRows: 2, maxRows: 5 }}
              onChange={(event) => {
                setTerminology(event.target.value);
                setTerminologyDirty(true);
              }}
              disabled={saving}
              style={{ flex: "1 1 320px", maxWidth: 720 }}
            />
            <Button
              size="small"
              icon={<SaveOutlined />}
              onClick={handleSaveTerminology}
              disabled={!terminologyDirty}
              loading={saving}
            >
              Save
            </Button>
          </div>
        </Space>
        <Alert
          type="info"
          showIcon
          style={{ marginBottom: 12 }}
          message="Stages run serially in dependency order; independent research questions can run concurrently with a configured limit."
        />
        {templates.map((template) => (
          <Alert
            key={template.id}
            data-testid={`asr-daily-agent-template-${template.id}`}
            type="success"
            showIcon
            style={{ marginBottom: 12 }}
            message={`${template.name} (Official Template)`}
            description={
              <Space direction="vertical" size={4} style={{ width: "100%" }}>
                <Text>{template.description}</Text>
                <Text type="secondary">
                  Uses minimal generic prompts. Every Agent Prompt remains
                  editable; no personal keywords, repositories, channels, or
                  ChatGPT Project are included.
                </Text>
                <Popconfirm
                  title="Replace the current Agent list with this template?"
                  description="Task terminology and report sync settings are preserved. IM delivery stays off until you configure it."
                  okText="Use template"
                  onConfirm={() => handleApplyTemplate(template.id)}
                >
                  <Button
                    data-testid={`asr-daily-agent-apply-template-${template.id}`}
                    type="primary"
                    size="small"
                    loading={saving}
                    style={{ width: "fit-content" }}
                  >
                    Use Template
                  </Button>
                </Popconfirm>
              </Space>
            }
          />
        ))}
        <Table<AsrDailyAgentItem>
          data-testid="asr-daily-agents-table"
          size="small"
          rowKey="id"
          dataSource={agents}
          pagination={false}
          scroll={{ x: 1370 }}
          columns={[
            {
              title: <span style={{ whiteSpace: "nowrap" }}>Enabled</span>,
              dataIndex: "enabled",
              width: 100,
              render: (enabled, record) => (
                <Switch
                  checked={enabled}
                  onChange={(checked) =>
                    updateAgent(record.id, { enabled: checked })
                  }
                />
              ),
            },
            {
              title: <span style={{ whiteSpace: "nowrap" }}>Agent</span>,
              dataIndex: "name",
              width: 230,
              render: (value, record) => (
                <Space direction="vertical" size={0}>
                  <Button
                    data-testid={`asr-daily-agent-edit-${record.id}`}
                    type="link"
                    style={{ padding: 0, height: "auto", whiteSpace: "nowrap" }}
                    onClick={() => void openAgentDetail(record.id)}
                  >
                    {value}
                  </Button>
                  <Text code style={{ fontSize: 11, whiteSpace: "nowrap" }}>
                    {record.id}
                  </Text>
                </Space>
              ),
            },
            {
              title: <span style={{ whiteSpace: "nowrap" }}>Output Dir</span>,
              dataIndex: "output_dir",
              width: 140,
              render: (value) => (
                <Text code style={{ whiteSpace: "nowrap" }}>
                  {value}
                </Text>
              ),
            },
            {
              title: <span style={{ whiteSpace: "nowrap" }}>Runner</span>,
              dataIndex: "runner",
              width: 140,
              render: (value) => (
                <Tag style={{ whiteSpace: "nowrap" }}>
                  {value || "Not configured"}
                </Tag>
              ),
            },
            {
              title: <span style={{ whiteSpace: "nowrap" }}>Dependencies</span>,
              dataIndex: "dependencies",
              width: 190,
              render: (_, record) =>
                record.dependencies?.length ? (
                  <Space size={[4, 4]} wrap>
                    {record.dependencies.map((dependency) => (
                      <Tag key={dependency.agent_id}>
                        {dependency.agent_id}
                        {dependency.include_output === false ? " / order" : ""}
                      </Tag>
                    ))}
                  </Space>
                ) : (
                  <Text type="secondary">None</Text>
                ),
            },
            {
              title: <span style={{ whiteSpace: "nowrap" }}>IM Delivery</span>,
              dataIndex: ["im_delivery", "enabled"],
              width: 190,
              render: (_, record) => (
                <Space size={6} style={{ whiteSpace: "nowrap" }}>
                  <Tag color={record.im_delivery.enabled ? "green" : "default"}>
                    {record.im_delivery.enabled ? "Enabled" : "Off"}
                  </Tag>
                  {record.im_delivery.channel && (
                    <Text style={{ fontSize: 12 }}>
                      {record.im_delivery.channel}
                    </Text>
                  )}
                </Space>
              ),
            },
            {
              title: <span style={{ whiteSpace: "nowrap" }}>Last Run</span>,
              width: 220,
              render: (_, record) =>
                record.last_status ? (
                  <Space size={6} style={{ whiteSpace: "nowrap" }}>
                    <Tag
                      color={
                        record.last_status === "success"
                          ? "green"
                          : record.last_status === "failed"
                            ? "red"
                            : record.last_status === "skipped_dependency_failed"
                              ? "orange"
                              : "default"
                      }
                    >
                      {record.last_status}
                    </Tag>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {formatTimestamp(record.last_run_at_ms)}
                    </Text>
                  </Space>
                ) : (
                  <Text type="secondary">Never</Text>
                ),
            },
            {
              title: <span style={{ whiteSpace: "nowrap" }}>Actions</span>,
              width: 210,
              render: (_, record) => (
                <Space style={{ whiteSpace: "nowrap" }}>
                  <Button
                    size="small"
                    icon={<EditOutlined />}
                    onClick={() => void openAgentDetail(record.id)}
                  >
                    Edit
                  </Button>
                  <Button
                    size="small"
                    icon={<PlayCircleOutlined />}
                    onClick={() => handleRun(false, record.id)}
                    loading={runningAgentId === record.id}
                    disabled={!config?.enabled || !record.enabled}
                  >
                    Run
                  </Button>
                  <Popconfirm
                    title="Delete Daily Agent?"
                    okText="Delete"
                    okButtonProps={{ danger: true }}
                    onConfirm={() => removeAgent(record.id)}
                  >
                    <Button size="small" danger icon={<DeleteOutlined />} />
                  </Popconfirm>
                </Space>
              ),
            },
          ]}
        />
        <Button
          style={{ marginTop: 12 }}
          icon={<PlusOutlined />}
          onClick={addAgent}
        >
          Add Agent
        </Button>
      </Card>
    </Space>
  );
}

export { DailyAgentRecordsTab } from "./DailyAgentRecordsTab";
