import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Descriptions,
  Empty,
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
  AsrDailyAgentProcessedDocument,
  AsrDailyAgentRunsResponse,
} from "../../../api/asr";
import { hasRunningDailyAgent } from "../asrUtils";
import * as imGatewayApi from "../../../api/imGateway";
import type {
  ExternalCliGatewayConfig,
  ImProviderConfig,
  ImTarget,
} from "../../../api/imGateway";
import {
  getDailyAgentConfig,
  getDailyAgentRuns,
  getDailyAgentInstructions,
  sendDailyAgentReport,
  syncDailyAgentReports,
  triggerDailyAgentRun,
  updateDailyAgentConfig,
  updateDailyAgentInstructions,
} from "../../../api/asr";
import { useI18n } from "../../../i18n";

const { Text } = Typography;
const { TextArea } = Input;
const DAILY_AGENT_TOKEN_RE = /^[A-Za-z0-9_-]+$/;
const DAILY_AGENT_RECORDS_TABLE_SCROLL_X = 1080;

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
  return <TaskDailyAgentTab key={taskId} taskId={taskId} />;
}

function TaskDailyAgentTab({ taskId }: DailyAgentTabProps) {
  const { t } = useI18n();
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
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [runningAgentId, setRunningAgentId] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [instructionsText, setInstructionsText] = useState("");
  const [instructionsDirty, setInstructionsDirty] = useState(false);
  const [selectedAgentId, setSelectedAgentId] = useState<string>("daily_report");
  const [detailAgentId, setDetailAgentId] = useState<string | null>(null);
  const [reportSyncDirDraft, setReportSyncDirDraft] = useState({
    value: "",
    dirty: false,
  });
  const { value: reportSyncDir, dirty: reportSyncDirDirty } = reportSyncDirDraft;
  const latestFetchId = useRef(0);
  const [terminology, setTerminology] = useState("");
  const [terminologyDirty, setTerminologyDirty] = useState(false);
  const [contextProfileDraft, setContextProfileDraft] = useState<{
    id: string;
    runner: string;
    work_dir: string;
    instructions: string;
  } | null>(null);

  const fetchAll = useCallback(async () => {
    const fetchId = ++latestFetchId.current;
    setLoading(true);
    try {
      const [config, runners, providers, targets] = await Promise.all([
        getDailyAgentConfig(taskId),
        imGatewayApi.getExternalCliConfig(),
        imGatewayApi.listProviders(),
        imGatewayApi.listTargets(),
      ]);
      const agents = config.config.agents || [];
      const preferredAgentId = routeDetailAgentId || selectedAgentId;
      const nextSelectedAgentId = agents.some((agent) => agent.id === preferredAgentId)
        ? preferredAgentId
        : agents[0]?.id || "daily_report";
      const instr = await getDailyAgentInstructions(taskId, nextSelectedAgentId);
      if (fetchId !== latestFetchId.current) return;
      setConfigData(config);
      setInstructions(instr);
      setSelectedAgentId(nextSelectedAgentId);
      setInstructionsText(instr.content);
      setInstructionsDirty(false);
      // Refreshes can finish after the user starts editing, including when
      // returning from an agent's details. Read the current draft at commit time.
      setReportSyncDirDraft((draft) =>
        draft.dirty
          ? draft
          : { value: config.config.report_sync_dir || "", dirty: false },
      );
      setTerminology(config.config.terminology || "");
      setTerminologyDirty(false);
      setRunnerConfig(runners);
      setImProviders(providers);
      setImTargets(targets);
    } catch (error: unknown) {
      if (fetchId === latestFetchId.current) {
        message.error(t("asrTasks.dailyAgent.loadConfigFailed", { error: errorMessage(error) }));
      }
    } finally {
      if (fetchId === latestFetchId.current) setLoading(false);
    }
  }, [routeDetailAgentId, selectedAgentId, t, taskId]);

  useEffect(() => {
    fetchAll();
    return () => {
      latestFetchId.current += 1;
    };
  }, [fetchAll]);

  useEffect(() => {
    setDetailAgentId(routeDetailAgentId);
    if (routeDetailAgentId) {
      setSelectedAgentId(routeDetailAgentId);
    }
  }, [routeDetailAgentId]);

  const handleConfigUpdate = async (
    updates: Partial<AsrDailyAgentConfig>
  ) => {
    setSaving(true);
    try {
      const result = await updateDailyAgentConfig(taskId, updates);
      if (result.ok) {
        message.success(t("asrTasks.dailyAgent.configSaved"));
        fetchAll();
      }
    } catch (error: unknown) {
      message.error(t("asrTasks.dailyAgent.saveFailed", { error: errorMessage(error) }));
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
    [configData?.config.agents]
  );

  const selectedAgent = useMemo(
    () => agents.find((agent) => agent.id === selectedAgentId) || agents[0],
    [agents, selectedAgentId]
  );

  const selectedWorkspaceAgent = useMemo(
    () =>
      configData?.workspace?.agents?.find(
        (agent) => agent.agent_id === selectedAgent?.id
      ),
    [configData?.workspace?.agents, selectedAgent?.id]
  );

  const loadAgentInstructions = useCallback(
    async (agentId: string) => {
      const instr = await getDailyAgentInstructions(taskId, agentId);
      setInstructions(instr);
      setInstructionsText(instr.content);
      setInstructionsDirty(false);
    },
    [taskId]
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
      message.error(t("asrTasks.dailyAgent.loadInstructionsFailed", { error: errorMessage(error) }));
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
      agent.id === agentId ? { ...agent, ...patch } : agent
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
      runner: template.runner || runnerConfig?.defaultRunnerId || runnerOptions[0]?.value || "Codex",
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
      message.warning(t("asrTasks.dailyAgent.atLeastOneRequired"));
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
      imProviders.map((provider) => [provider.id, provider])
    );
    const ownerOptions = imProviders
      .filter((provider) => provider.owner_open_id?.trim())
      .map((provider) => ({
        label: t("asrTasks.dailyAgent.ownerChannel", {
          name: provider.display_name || provider.id,
        }),
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
  }, [imProviders, imTargets, t]);

  const handleSaveInstructions = async () => {
    setSaving(true);
    try {
      await updateDailyAgentInstructions(taskId, instructionsText, selectedAgent?.id);
      message.success(t("asrTasks.dailyAgent.instructionsSaved"));
      setInstructionsDirty(false);
    } catch (error: unknown) {
      message.error(t("asrTasks.dailyAgent.saveInstructionsFailed", { error: errorMessage(error) }));
    } finally {
      setSaving(false);
    }
  };

  const handleSaveReportSyncDir = async () => {
    setSaving(true);
    try {
      const result = await updateDailyAgentConfig(taskId, {
        report_sync_dir: reportSyncDir,
      });
      if (!result.ok) throw new Error(t("asrTasks.dailyAgent.configNotSaved"));
      message.success(t("asrTasks.dailyAgent.reportSyncDirSaved"));
      setReportSyncDirDraft((draft) =>
        draft.value === reportSyncDir
          ? { value: result.config.report_sync_dir || "", dirty: false }
          : draft,
      );
      fetchAll();
    } catch (error: unknown) {
      message.error(t("asrTasks.dailyAgent.saveSyncDirFailed", { error: errorMessage(error) }));
    } finally {
      setSaving(false);
    }
  };

  const handleSaveTerminology = async () => {
    setSaving(true);
    try {
      await updateDailyAgentConfig(taskId, { terminology });
      message.success(t("asrTasks.dailyAgent.terminologySaved"));
      setTerminologyDirty(false);
      fetchAll();
    } catch (error: unknown) {
      message.error(t("asrTasks.dailyAgent.saveTerminologyFailed", { error: errorMessage(error) }));
    } finally {
      setSaving(false);
    }
  };

  const handleSyncReports = async () => {
    setSyncing(true);
    try {
      const result = await syncDailyAgentReports(taskId);
      message.success(
        t("asrTasks.dailyAgent.syncedSummary", {
          copied: result.sync.copied_files,
          skipped: result.sync.skipped_files,
        }),
      );
      fetchAll();
    } catch (error: unknown) {
      message.error(t("asrTasks.dailyAgent.syncFailed", { error: errorMessage(error) }));
      fetchAll();
    } finally {
      setSyncing(false);
    }
  };

  const handleRun = async (force: boolean, agentId: string) => {
    setRunningAgentId(agentId);
    try {
      const result = await triggerDailyAgentRun(taskId, { force, agentId });
      message.success(result.message || t("asrTasks.dailyAgent.runQueued"));
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
      message.error(t("asrTasks.dailyAgent.runFailed", { error: errorMessage(error) }));
      setRunningAgentId(null);
    }
  };

  const handleSend = async () => {
    try {
      const result = await sendDailyAgentReport(taskId, selectedAgent?.id);
      if (result.ok) {
        message.success(t("asrTasks.dailyAgent.sentReports", { count: result.sent_reports.length }));
      }
    } catch (error: unknown) {
      message.error(t("asrTasks.dailyAgent.sendFailed", { error: errorMessage(error) }));
    }
  };

  const config = configData?.config;
  const reportIndex = configData?.report_index_status;
  const activeAgentId = selectedAgent?.id || selectedAgentId;
  const reportSync = selectedAgent?.last_report_sync || config?.last_report_sync;
  const formatTimestamp = (value?: number) =>
    value ? new Date(value).toLocaleString() : t("asrTasks.dailyAgent.never");

  if (loading && !configData) {
    return (
      <div style={{ padding: 24, textAlign: "center" }}>
        <Text type="secondary">{t("asrTasks.dailyAgent.loading")}</Text>
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
            <Button
              icon={<ArrowLeftOutlined />}
              onClick={closeAgentDetail}
            >
              {t("asrTasks.dailyAgent.dailyAgents")}
            </Button>
            <Text strong>{selectedAgent.name}</Text>
            <Text code>{selectedAgent.id}</Text>
          </Space>
          <Button icon={<ReloadOutlined />} onClick={fetchAll} loading={loading}>
            {t("common.refresh")}
          </Button>
        </Space>

        <Card size="small" title={t("asrTasks.dailyAgent.agentConfiguration")} loading={loading && !config}>
          <Descriptions column={2} size="small" bordered>
            <Descriptions.Item label={t("common.enabled")}>
              <Switch
                checked={selectedAgent.enabled}
                onChange={(enabled) => updateAgent(selectedAgent.id, { enabled })}
                loading={saving}
              />
            </Descriptions.Item>
            <Descriptions.Item label={t("asrTasks.field.runner")}>
              <Select
                data-testid="asr-daily-agent-runner-select"
                size="small"
                value={selectedAgent.runner || undefined}
                placeholder={t("asrTasks.dailyAgent.selectRunner")}
                onChange={(runner) => updateAgent(selectedAgent.id, { runner })}
                loading={loading}
                disabled={saving}
                style={{ width: 240 }}
                options={runnerOptions}
              />
            </Descriptions.Item>
            <Descriptions.Item label={t("asrTasks.dailyAgent.agentName")}>
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
            <Descriptions.Item label={t("asrTasks.field.outputDir")}>
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
            <Descriptions.Item label={t("asrTasks.dailyAgent.triggerPolicy")}>
              <Select
                size="small"
                value={selectedAgent.trigger_policy}
                onChange={(trigger_policy) =>
                  updateAgent(selectedAgent.id, { trigger_policy })
                }
                style={{ width: 160 }}
                options={[
                  { label: t("asrTasks.dailyAgent.trigger.afterAsrRun"), value: "after_asr_run" },
                  { label: t("asrTasks.dailyAgent.trigger.manualOnly"), value: "manual_only" },
                ]}
              />
            </Descriptions.Item>
            <Descriptions.Item label={t("asrTasks.dailyAgent.timeout")}>
              <InputNumber
                size="small"
                min={1}
                step={5}
                addonAfter={t("asrTasks.dailyAgent.minutesUnit")}
                value={Math.round(selectedAgent.timeout_ms / 60000)}
                onChange={(minutes) =>
                  updateAgent(selectedAgent.id, {
                    timeout_ms: Number(minutes || 1) * 60000,
                  })
                }
                style={{ width: 150 }}
              />
            </Descriptions.Item>
            <Descriptions.Item label={t("asrTasks.dailyAgent.sessionKey")}>
              <Input
                size="small"
                value={selectedAgent.session_key || ""}
                placeholder={t("asrTasks.dailyAgent.autoPlaceholder")}
                onChange={(event) =>
                  updateAgent(selectedAgent.id, {
                    session_key: event.target.value.trim() || undefined,
                  })
                }
              />
            </Descriptions.Item>
            <Descriptions.Item label={t("asrTasks.dailyAgent.dependencies")} span={2}>
              <Space direction="vertical" size={8} style={{ width: "100%" }}>
                <Select
                  data-testid="asr-daily-agent-dependencies-select"
                  mode="multiple"
                  allowClear
                  value={(selectedAgent.dependencies || []).map(
                    (dependency) => dependency.agent_id
                  )}
                  placeholder={t("asrTasks.dailyAgent.selectUpstreamAgents")}
                  options={agents
                    .filter((agent) => agent.id !== selectedAgent.id)
                    .map((agent) => ({ label: agent.name, value: agent.id }))}
                  onChange={(dependencyIds: string[]) => {
                    const existing = new Map(
                      (selectedAgent.dependencies || []).map((dependency) => [
                        dependency.agent_id,
                        dependency,
                      ])
                    );
                    updateAgent(selectedAgent.id, {
                      dependencies: dependencyIds.map(
                        (agent_id) =>
                          existing.get(agent_id) || {
                            agent_id,
                            include_output: true,
                          }
                      ),
                    });
                  }}
                  style={{ width: "100%" }}
                />
                {(selectedAgent.dependencies || []).map((dependency) => (
                  <Space key={dependency.agent_id}>
                    <Text code>{dependency.agent_id}</Text>
                    <Text type="secondary">{t("asrTasks.dailyAgent.includeOutput")}</Text>
                    <Switch
                      size="small"
                      checked={dependency.include_output !== false}
                      onChange={(include_output) =>
                        updateAgent(selectedAgent.id, {
                          dependencies: (selectedAgent.dependencies || []).map(
                            (item) =>
                              item.agent_id === dependency.agent_id
                                ? { ...item, include_output }
                                : item
                          ),
                        })
                      }
                    />
                  </Space>
                ))}
              </Space>
            </Descriptions.Item>
            <Descriptions.Item label={t("asrTasks.dailyAgent.dependencyFailure")}>
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
                  { label: t("asrTasks.dailyAgent.dependencyFailure.skip"), value: "skip" },
                  { label: t("asrTasks.dailyAgent.dependencyFailure.continue"), value: "continue" },
                ]}
              />
            </Descriptions.Item>
            <Descriptions.Item label={t("asrTasks.dailyAgent.independentResearch")}>
              <Switch
                data-testid="asr-daily-agent-research-fanout-switch"
                checked={Boolean(selectedAgent.research_fanout)}
                onChange={(enabled) =>
                  updateAgent(selectedAgent.id, {
                    research_fanout: enabled
                      ? {
                          max_questions: 8,
                          chatgpt_interface_mode: "chat",
                          chatgpt_model: "pro",
                          chatgpt_project_url: undefined,
                          allowed_runners: selectedAgent.runner
                            ? [selectedAgent.runner]
                            : [],
                          context_profiles: {},
                        }
                      : undefined,
                  })
                }
              />
            </Descriptions.Item>
            {selectedAgent.research_fanout ? (
              <>
                <Descriptions.Item label={t("asrTasks.dailyAgent.maxQuestions")}>
                  <InputNumber
                    data-testid="asr-daily-agent-research-max-questions"
                    size="small"
                    min={1}
                    max={50}
                    value={selectedAgent.research_fanout.max_questions}
                    onChange={(maxQuestions) =>
                      updateAgent(selectedAgent.id, {
                        research_fanout: {
                          ...selectedAgent.research_fanout!,
                          max_questions: Number(maxQuestions || 1),
                        },
                      })
                    }
                  />
                </Descriptions.Item>
                <Descriptions.Item label={t("asrTasks.dailyAgent.chatgptProjectUrl")} span={2}>
                  <Input
                    key={`${selectedAgent.id}-project-url-${
                      selectedAgent.research_fanout.chatgpt_project_url || ""
                    }`}
                    data-testid="asr-daily-agent-research-project-url"
                    size="small"
                    defaultValue={
                      selectedAgent.research_fanout.chatgpt_project_url || ""
                    }
                    placeholder="https://chatgpt.com/g/g-p-.../project"
                    onBlur={(event) => {
                      const chatgptProjectUrl = event.target.value.trim();
                      if (
                        chatgptProjectUrl ===
                        (selectedAgent.research_fanout!
                          .chatgpt_project_url || "")
                      ) {
                        return;
                      }
                      updateAgent(selectedAgent.id, {
                        research_fanout: {
                          ...selectedAgent.research_fanout!,
                          chatgpt_project_url:
                            chatgptProjectUrl || undefined,
                        },
                      });
                    }}
                    onPressEnter={(event) => event.currentTarget.blur()}
                  />
                </Descriptions.Item>
                <Descriptions.Item label={t("asrTasks.dailyAgent.allowedResearchRunners")} span={2}>
                  <Select
                    data-testid="asr-daily-agent-research-runners"
                    mode="multiple"
                    allowClear
                    value={selectedAgent.research_fanout.allowed_runners}
                    options={runnerOptions}
                    onChange={(allowedRunners: string[]) =>
                      updateAgent(selectedAgent.id, {
                        research_fanout: {
                          ...selectedAgent.research_fanout!,
                          allowed_runners: allowedRunners,
                        },
                      })
                    }
                    style={{ width: "100%" }}
                  />
                </Descriptions.Item>
                <Descriptions.Item label={t("asrTasks.dailyAgent.runtimeDataFallbacks")} span={2}>
                  <Space direction="vertical" size={8} style={{ width: "100%" }}>
                    {Object.entries(
                      selectedAgent.research_fanout.context_profiles || {},
                    ).map(([profileId, profile]) => (
                      <Space key={profileId} wrap style={{ width: "100%" }}>
                        <Text code>{profileId}</Text>
                        <Select
                          aria-label={t("asrTasks.dailyAgent.fallbackRunnerAria", { profile: profileId })}
                          size="small"
                          value={profile.runner || undefined}
                          placeholder={t("asrTasks.field.runner")}
                          options={runnerOptions}
                          onChange={(runner) =>
                            updateAgent(selectedAgent.id, {
                              research_fanout: {
                                ...selectedAgent.research_fanout!,
                                context_profiles: {
                                  ...selectedAgent.research_fanout!
                                    .context_profiles,
                                  [profileId]: { ...profile, runner },
                                },
                              },
                            })
                          }
                          style={{ width: 180 }}
                        />
                        <Input
                          key={`${profileId}-work-dir-${profile.work_dir}`}
                          aria-label={t("asrTasks.dailyAgent.fallbackWorkDirAria", { profile: profileId })}
                          size="small"
                          defaultValue={profile.work_dir}
                          placeholder={t("asrTasks.dailyAgent.localWorkingFolder")}
                          onBlur={(event) => {
                            const workDir = event.target.value.trim();
                            if (workDir === profile.work_dir) {
                              return;
                            }
                            updateAgent(selectedAgent.id, {
                              research_fanout: {
                                ...selectedAgent.research_fanout!,
                                context_profiles: {
                                  ...selectedAgent.research_fanout!
                                    .context_profiles,
                                  [profileId]: {
                                    ...profile,
                                    work_dir: workDir,
                                  },
                                },
                              },
                            });
                          }}
                          onPressEnter={(event) => event.currentTarget.blur()}
                          style={{ width: 260 }}
                        />
                        <Input
                          key={`${profileId}-instructions-${
                            profile.instructions || ""
                          }`}
                          aria-label={t("asrTasks.dailyAgent.fallbackInstructionsAria", { profile: profileId })}
                          size="small"
                          defaultValue={profile.instructions || ""}
                          placeholder={t("asrTasks.dailyAgent.factsPlaceholder")}
                          onBlur={(event) => {
                            const instructions = event.target.value.trim();
                            if (instructions === (profile.instructions || "")) {
                              return;
                            }
                            updateAgent(selectedAgent.id, {
                              research_fanout: {
                                ...selectedAgent.research_fanout!,
                                context_profiles: {
                                  ...selectedAgent.research_fanout!
                                    .context_profiles,
                                  [profileId]: {
                                    ...profile,
                                    instructions:
                                      instructions || undefined,
                                  },
                                },
                              },
                            });
                          }}
                          onPressEnter={(event) => event.currentTarget.blur()}
                          style={{ width: 260 }}
                        />
                        <Button
                          size="small"
                          danger
                          onClick={() => {
                            const nextProfiles = {
                              ...selectedAgent.research_fanout!
                                .context_profiles,
                            };
                            delete nextProfiles[profileId];
                            updateAgent(selectedAgent.id, {
                              research_fanout: {
                                ...selectedAgent.research_fanout!,
                                context_profiles: nextProfiles,
                              },
                            });
                          }}
                        >
                          {t("common.remove")}
                        </Button>
                      </Space>
                    ))}
                    {contextProfileDraft ? (
                      <Space wrap style={{ width: "100%" }}>
                        <Input
                          aria-label={t("asrTasks.dailyAgent.newFallbackNameAria")}
                          size="small"
                          value={contextProfileDraft.id}
                          placeholder={t("asrTasks.dailyAgent.fallbackName")}
                          onChange={(event) =>
                            setContextProfileDraft({
                              ...contextProfileDraft,
                              id: normalizeAgentToken(event.target.value),
                            })
                          }
                          style={{ width: 160 }}
                        />
                        <Select
                          aria-label={t("asrTasks.dailyAgent.newFallbackRunnerAria")}
                          size="small"
                          value={contextProfileDraft.runner || undefined}
                          placeholder={t("asrTasks.field.runner")}
                          options={runnerOptions}
                          onChange={(runner) =>
                            setContextProfileDraft({
                              ...contextProfileDraft,
                              runner,
                            })
                          }
                          style={{ width: 180 }}
                        />
                        <Input
                          aria-label={t("asrTasks.dailyAgent.newFallbackWorkDirAria")}
                          size="small"
                          value={contextProfileDraft.work_dir}
                          placeholder={t("asrTasks.dailyAgent.localWorkingFolder")}
                          onChange={(event) =>
                            setContextProfileDraft({
                              ...contextProfileDraft,
                              work_dir: event.target.value,
                            })
                          }
                          style={{ width: 260 }}
                        />
                        <Input
                          aria-label={t("asrTasks.dailyAgent.newFallbackInstructionsAria")}
                          size="small"
                          value={contextProfileDraft.instructions}
                          placeholder={t("asrTasks.dailyAgent.factsPlaceholder")}
                          onChange={(event) =>
                            setContextProfileDraft({
                              ...contextProfileDraft,
                              instructions: event.target.value,
                            })
                          }
                          style={{ width: 260 }}
                        />
                        <Button
                          size="small"
                          type="primary"
                          onClick={() => {
                            const profileId = contextProfileDraft.id.trim();
                            const runner = contextProfileDraft.runner.trim();
                            const workDir = contextProfileDraft.work_dir.trim();
                            if (
                              !DAILY_AGENT_TOKEN_RE.test(profileId) ||
                              !runner ||
                              !workDir
                            ) {
                              message.warning(
                                t("asrTasks.dailyAgent.fallbackRequired"),
                              );
                              return;
                            }
                            updateAgent(selectedAgent.id, {
                              research_fanout: {
                                ...selectedAgent.research_fanout!,
                                context_profiles: {
                                  ...selectedAgent.research_fanout!
                                    .context_profiles,
                                  [profileId]: {
                                    runner,
                                    work_dir: workDir,
                                    instructions:
                                      contextProfileDraft.instructions.trim() ||
                                      undefined,
                                  },
                                },
                              },
                            });
                            setContextProfileDraft(null);
                          }}
                        >
                          {t("asrTasks.dailyAgent.saveFallback")}
                        </Button>
                        <Button
                          size="small"
                          onClick={() => setContextProfileDraft(null)}
                        >
                          {t("common.cancel")}
                        </Button>
                      </Space>
                    ) : null}
                    <Button
                      data-testid="asr-daily-agent-add-context-profile"
                      size="small"
                      icon={<PlusOutlined />}
                      onClick={() => {
                        const profiles =
                          selectedAgent.research_fanout!.context_profiles || {};
                        let index = Object.keys(profiles).length + 1;
                        let profileId = `context_${index}`;
                        while (profiles[profileId]) {
                          index += 1;
                          profileId = `context_${index}`;
                        }
                        setContextProfileDraft({
                          id: profileId,
                          runner:
                            selectedAgent.research_fanout!.allowed_runners[0] ||
                            selectedAgent.runner,
                          work_dir: "",
                          instructions: "",
                        });
                      }}
                      disabled={Boolean(contextProfileDraft)}
                    >
                      {t("asrTasks.dailyAgent.addRuntimeFallback")}
                    </Button>
                  </Space>
                </Descriptions.Item>
              </>
            ) : null}
            <Descriptions.Item label={t("asrTasks.dailyAgent.instructionsSource")}>
              <Tag>{instructions?.source || selectedAgent.instructions_source}</Tag>
            </Descriptions.Item>
          </Descriptions>
          {selectedAgent.research_fanout ? (
            <Alert
              style={{ marginTop: 12 }}
              type="info"
              showIcon
              message={t("asrTasks.dailyAgent.researchAlertTitle")}
              description={t("asrTasks.dailyAgent.researchAlertDescription")}
            />
          ) : null}
        </Card>

        <Card size="small" title={t("asrTasks.dailyAgent.imDelivery")} loading={loading && !config}>
          <Descriptions column={2} size="small" bordered>
            <Descriptions.Item label={t("common.enabled")}>
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
            <Descriptions.Item label={t("asrTasks.dailyAgent.channel")}>
              <Select
                data-testid="asr-daily-agent-im-channel-select"
                size="small"
                allowClear
                value={selectedAgent.im_delivery.channel || undefined}
                placeholder={t("asrTasks.dailyAgent.selectChannel")}
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
            <Descriptions.Item label={t("asrTasks.field.mode")}>
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
                  { label: t("asrTasks.dailyAgent.deliveryMode.fullReport"), value: "full_report" },
                  { label: t("asrTasks.dailyAgent.deliveryMode.summary"), value: "summary" },
                ]}
              />
            </Descriptions.Item>
            <Descriptions.Item label={t("asrTasks.dailyAgent.sendPolicy")}>
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
                    label: t("asrTasks.dailyAgent.sendPolicy.onSuccessWithReport"),
                    value: "on_success_with_report",
                  },
                  { label: t("asrTasks.dailyAgent.sendPolicy.onSuccess"), value: "on_success" },
                  { label: t("asrTasks.dailyAgent.sendPolicy.always"), value: "always" },
                ]}
              />
            </Descriptions.Item>
            {selectedAgent.im_delivery.last_send_error && (
              <Descriptions.Item label={t("asrTasks.dailyAgent.lastSendError")} span={2}>
                <Alert
                  type="error"
                  showIcon
                  message={selectedAgent.im_delivery.last_send_error}
                />
              </Descriptions.Item>
            )}
          </Descriptions>
        </Card>

        <Card size="small" title={t("asrTasks.dailyAgent.lastRunStatus")}>
          <Descriptions column={3} size="small">
            <Descriptions.Item label={t("common.status")}>
              <Tag
                color={
                  selectedAgent.last_status === "success"
                    ? "green"
                    : selectedAgent.last_status === "failed"
                      ? "red"
                      : selectedAgent.last_status === "skipped_dependency_failed"
                        ? "orange"
                      : "default"
                }
              >
                {selectedAgent.last_status || t("asrTasks.dailyAgent.statusNever")}
              </Tag>
            </Descriptions.Item>
            <Descriptions.Item label={t("asrTasks.dailyAgent.runId")}>
              <Text code style={{ fontSize: 11 }}>
                {selectedAgent.last_run_id?.slice(0, 16) || "-"}
              </Text>
            </Descriptions.Item>
            <Descriptions.Item label={t("asrTasks.field.lastRun")}>
              <Text>{formatTimestamp(selectedAgent.last_run_at_ms)}</Text>
            </Descriptions.Item>
            <Descriptions.Item label={t("asrTasks.dailyAgent.reports")}>
              {selectedWorkspaceAgent?.report_count ?? 0}
            </Descriptions.Item>
            <Descriptions.Item label={t("asrTasks.field.outputDir")}>
              <Text code>{selectedWorkspaceAgent?.output_dir || selectedAgent.output_dir}</Text>
            </Descriptions.Item>
            <Descriptions.Item label={t("asrTasks.dailyAgent.instructionsFile")}>
              <Tag
                color={
                  selectedWorkspaceAgent?.instructions_exists ? "green" : "orange"
                }
              >
                {selectedWorkspaceAgent?.instructions_exists
                  ? t("asrTasks.field.ready")
                  : t("asrTasks.dailyAgent.missing")}
              </Tag>
            </Descriptions.Item>
            {selectedAgent.last_error && (
              <Descriptions.Item label={t("common.error")} span={3}>
                <Alert type="error" message={selectedAgent.last_error} showIcon />
              </Descriptions.Item>
            )}
          </Descriptions>
          {reportSync && (
            <>
              <Descriptions column={3} size="small" style={{ marginTop: 8 }}>
                <Descriptions.Item label={t("asrTasks.dailyAgent.reportSync")}>
                  <Tag color={reportSync.failed_files > 0 ? "red" : "green"}>
                    {t("asrTasks.dailyAgent.reportSyncCounts", {
                      copied: reportSync.copied_files,
                      total: reportSync.total_files,
                    })}
                  </Tag>
                </Descriptions.Item>
                <Descriptions.Item label={t("asrTasks.dailyAgent.skipped")}>
                  {reportSync.skipped_files}
                </Descriptions.Item>
                <Descriptions.Item label={t("asrTasks.dailyAgent.lastSync")}>
                  {formatTimestamp(reportSync.synced_at_ms)}
                </Descriptions.Item>
              </Descriptions>
              {reportSync.failed_files > 0 && (
                <Alert
                  style={{ marginTop: 8 }}
                  type="error"
                  showIcon
                  message={t("asrTasks.dailyAgent.reportSyncFailed", { count: reportSync.failed_files })}
                  description={(reportSync.errors || []).slice(0, 3).join("; ")}
                />
              )}
            </>
          )}
          {reportIndex && (
            <Descriptions column={3} size="small" style={{ marginTop: 8 }}>
              <Descriptions.Item label={t("asrTasks.dailyAgent.indexedReports")}>
                <Tag
                  color={reportIndex.unindexed_reports > 0 ? "orange" : "green"}
                >
                  {reportIndex.indexed_reports}/{reportIndex.report_files}
                </Tag>
              </Descriptions.Item>
              <Descriptions.Item label={t("asrTasks.dailyAgent.unindexedReports")}>
                <Tag
                  color={reportIndex.unindexed_reports > 0 ? "orange" : "green"}
                >
                  {reportIndex.unindexed_reports}
                </Tag>
              </Descriptions.Item>
              <Descriptions.Item label={t("asrTasks.dailyAgent.missingReports")}>
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
          <Tooltip title={t("asrTasks.dailyAgent.runNowTooltip")}>
            <Button
              type="primary"
              icon={<PlayCircleOutlined />}
              onClick={() => handleRun(false, activeAgentId)}
              loading={runningAgentId === activeAgentId}
              disabled={!config?.enabled || !selectedAgent.enabled}
            >
              {t("asrTasks.dailyAgent.runNow")}
            </Button>
          </Tooltip>
          <Tooltip title={t("asrTasks.dailyAgent.forceRunTooltip")}>
            <Button
              icon={<ThunderboltOutlined />}
              onClick={() => handleRun(true, activeAgentId)}
              loading={runningAgentId === activeAgentId}
              disabled={!config?.enabled || !selectedAgent.enabled}
            >
              {t("asrTasks.dailyAgent.forceRun")}
            </Button>
          </Tooltip>
          <Tooltip title={t("asrTasks.dailyAgent.sendReportTooltip")}>
            <Button
              icon={<SendOutlined />}
              onClick={handleSend}
              disabled={!selectedAgent.im_delivery.enabled}
            >
              {t("asrTasks.dailyAgent.sendReport")}
            </Button>
          </Tooltip>
        </Space>

        <Card
          size="small"
          title={t("asrTasks.dailyAgent.agentInstructionsTitle", {
            name: selectedAgent.name || activeAgentId,
          })}
          extra={
            <Button
              type="primary"
              size="small"
              icon={<SaveOutlined />}
              onClick={handleSaveInstructions}
              loading={saving}
              disabled={!instructionsDirty}
            >
              {t("common.save")}
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
            placeholder={t("asrTasks.dailyAgent.instructionsPlaceholder")}
          />
          {instructions?.source && (
            <Text
              type="secondary"
              style={{ fontSize: 11, marginTop: 4, display: "block" }}
            >
              {t("asrTasks.dailyAgent.sourceLabel", { source: instructions.source })}
            </Text>
          )}
        </Card>
      </Space>
    );
  }

  return (
    <Space direction="vertical" size={16} style={{ width: "100%" }}>
      <Card size="small" title={t("asrTasks.dailyAgent.dailyAgents")} loading={loading && !config}>
        <Space direction="vertical" size={8} style={{ width: "100%", marginBottom: 12 }}>
          <Space
            style={{
              justifyContent: "space-between",
              width: "100%",
            }}
          >
            <Space wrap>
              <Text type="secondary">{t("asrTasks.field.task")}</Text>
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
                  placeholder={t("asrTasks.dailyAgent.reportSyncDirPlaceholder")}
                  onChange={(event) => {
                    setReportSyncDirDraft({
                      value: event.target.value,
                      dirty: true,
                    });
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
                  {t("common.save")}
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
                {t("asrTasks.dailyAgent.syncReports")}
              </Button>
            </Space>
            <Button icon={<ReloadOutlined />} onClick={fetchAll} loading={loading}>
              {t("common.refresh")}
            </Button>
          </Space>
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
              {t("asrTasks.dailyAgent.terminology")}
            </Text>
            <TextArea
              data-testid="asr-daily-agent-terminology"
              value={terminology}
              placeholder={t("asrTasks.dailyAgent.terminologyPlaceholder")}
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
              {t("common.save")}
            </Button>
          </div>
        </Space>
        <Alert
          type="info"
          showIcon
          style={{ marginBottom: 12 }}
          message={t("asrTasks.dailyAgent.orderHint")}
        />
        <Table<AsrDailyAgentItem>
          data-testid="asr-daily-agents-table"
          size="small"
          rowKey="id"
          dataSource={agents}
          pagination={false}
          scroll={{ x: 1370 }}
          columns={[
            {
              title: <span style={{ whiteSpace: "nowrap" }}>{t("common.enabled")}</span>,
              dataIndex: "enabled",
              width: 100,
              render: (enabled, record) => (
                <Switch
                  checked={enabled}
                  onChange={(checked) => updateAgent(record.id, { enabled: checked })}
                />
              ),
            },
            {
              title: <span style={{ whiteSpace: "nowrap" }}>{t("asrTasks.field.agent")}</span>,
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
              title: <span style={{ whiteSpace: "nowrap" }}>{t("asrTasks.field.outputDir")}</span>,
              dataIndex: "output_dir",
              width: 140,
              render: (value) => (
                <Text code style={{ whiteSpace: "nowrap" }}>
                  {value}
                </Text>
              ),
            },
            {
              title: <span style={{ whiteSpace: "nowrap" }}>{t("asrTasks.field.runner")}</span>,
              dataIndex: "runner",
              width: 140,
              render: (value) => (
                <Tag style={{ whiteSpace: "nowrap" }}>
                  {value || t("asrTasks.dailyAgent.notConfigured")}
                </Tag>
              ),
            },
            {
              title: <span style={{ whiteSpace: "nowrap" }}>{t("asrTasks.dailyAgent.dependencies")}</span>,
              dataIndex: "dependencies",
              width: 190,
              render: (_, record) =>
                record.dependencies?.length ? (
                  <Space size={[4, 4]} wrap>
                    {record.dependencies.map((dependency) => (
                      <Tag key={dependency.agent_id}>
                        {dependency.agent_id}
                        {dependency.include_output === false
                          ? t("asrTasks.dailyAgent.dependencyOrderOnly")
                          : ""}
                      </Tag>
                    ))}
                  </Space>
                ) : (
                  <Text type="secondary">{t("common.none")}</Text>
                ),
            },
            {
              title: <span style={{ whiteSpace: "nowrap" }}>{t("asrTasks.dailyAgent.imDelivery")}</span>,
              dataIndex: ["im_delivery", "enabled"],
              width: 190,
              render: (_, record) => (
                <Space size={6} style={{ whiteSpace: "nowrap" }}>
                  <Tag color={record.im_delivery.enabled ? "green" : "default"}>
                    {record.im_delivery.enabled ? t("common.enabled") : t("common.off")}
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
              title: <span style={{ whiteSpace: "nowrap" }}>{t("asrTasks.field.lastRun")}</span>,
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
                  <Text type="secondary">{t("asrTasks.dailyAgent.never")}</Text>
                ),
            },
            {
              title: <span style={{ whiteSpace: "nowrap" }}>{t("common.actions")}</span>,
              width: 210,
              render: (_, record) => (
                <Space style={{ whiteSpace: "nowrap" }}>
                  <Button
                    size="small"
                    icon={<EditOutlined />}
                    onClick={() => void openAgentDetail(record.id)}
                  >
                    {t("common.edit")}
                  </Button>
                  <Button
                    size="small"
                    icon={<PlayCircleOutlined />}
                    onClick={() => handleRun(false, record.id)}
                    loading={runningAgentId === record.id}
                    disabled={!config?.enabled || !record.enabled}
                  >
                    {t("asrTasks.field.run")}
                  </Button>
                  <Popconfirm
                    title={t("asrTasks.dailyAgent.deleteConfirm")}
                    okText={t("common.delete")}
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
        <Button style={{ marginTop: 12 }} icon={<PlusOutlined />} onClick={addAgent}>
          {t("asrTasks.dailyAgent.addAgent")}
        </Button>
      </Card>
    </Space>
  );
}

export function DailyAgentRecordsTab({ taskId, onOpenReport }: DailyAgentTabProps) {
  const { t } = useI18n();
  const [runsData, setRunsData] = useState<AsrDailyAgentRunsResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [agentFilter, setAgentFilter] = useState<string | undefined>();
  const [dateFilter, setDateFilter] = useState<string | undefined>();
  const [runnerFilter, setRunnerFilter] = useState<string | undefined>();

  const fetchRuns = useCallback(async () => {
    setLoading(true);
    try {
      setRunsData(await getDailyAgentRuns(taskId));
    } catch (error: unknown) {
      message.error(t("asrTasks.records.loadFailed", { error: errorMessage(error) }));
    } finally {
      setLoading(false);
    }
  }, [t, taskId]);

  useEffect(() => {
    fetchRuns();
  }, [fetchRuns]);

  useEffect(() => {
    setAgentFilter(undefined);
    setDateFilter(undefined);
    setRunnerFilter(undefined);
  }, [taskId]);

  const processedDocuments = useMemo(
    () =>
      [...(runsData?.processed_documents ?? [])].sort((a, b) => {
        const dateOrder = b.date.localeCompare(a.date);
        if (dateOrder !== 0) {
          return dateOrder;
        }
        return (b.processed_at_ms ?? 0) - (a.processed_at_ms ?? 0);
      }),
    [runsData],
  );

  const filterOptions = useMemo(() => {
    const agents = new Map<string, string>();
    const dates = new Set<string>();
    const runners = new Set<string>();

    for (const record of processedDocuments) {
      const agentId = record.agent_id || "daily_report";
      agents.set(agentId, record.agent_name || agentId);
      if (record.date) {
        dates.add(record.date);
      }
      if (record.runner) {
        runners.add(record.runner);
      }
    }

    return {
      agents: [...agents.entries()]
        .sort((a, b) => a[1].localeCompare(b[1]))
        .map(([value, label]) => ({
          value,
          label: label === value ? label : `${label} (${value})`,
        })),
      dates: [...dates].sort((a, b) => b.localeCompare(a)).map((value) => ({
        value,
        label: value,
      })),
      runners: [...runners].sort().map((value) => ({
        value,
        label: value,
      })),
    };
  }, [processedDocuments]);

  const filteredDocuments = useMemo(
    () =>
      processedDocuments.filter((record) => {
        if (agentFilter && (record.agent_id || "daily_report") !== agentFilter) {
          return false;
        }
        if (dateFilter && record.date !== dateFilter) {
          return false;
        }
        if (runnerFilter && record.runner !== runnerFilter) {
          return false;
        }
        return true;
      }),
    [agentFilter, dateFilter, processedDocuments, runnerFilter],
  );

  return (
    <Card
      size="small"
      title={t("asrTasks.records.runResults")}
      style={{ width: "100%", minWidth: 0 }}
      loading={loading && !runsData}
      extra={
        <Button icon={<ReloadOutlined />} onClick={fetchRuns} loading={loading}>
          {t("common.refresh")}
        </Button>
      }
    >
      {runsData && processedDocuments.length > 0 ? (
        <Space direction="vertical" size={12} style={{ width: "100%", minWidth: 0 }}>
          <Space size={8} wrap>
            <Select
              allowClear
              showSearch
              placeholder={t("asrTasks.field.agent")}
              style={{ width: 220 }}
              value={agentFilter}
              options={filterOptions.agents}
              optionFilterProp="label"
              onChange={setAgentFilter}
              data-testid="asr-daily-agent-records-agent-filter"
            />
            <Select
              allowClear
              showSearch
              placeholder={t("asrTasks.field.date")}
              style={{ width: 160 }}
              value={dateFilter}
              options={filterOptions.dates}
              optionFilterProp="label"
              onChange={setDateFilter}
              data-testid="asr-daily-agent-records-date-filter"
            />
            <Select
              allowClear
              showSearch
              placeholder={t("asrTasks.field.runner")}
              style={{ width: 160 }}
              value={runnerFilter}
              options={filterOptions.runners}
              optionFilterProp="label"
              onChange={setRunnerFilter}
              data-testid="asr-daily-agent-records-runner-filter"
            />
          </Space>
          <div
            data-testid="asr-daily-agent-run-results-table"
            style={{ width: "100%", minWidth: 0, overflow: "hidden" }}
          >
            <Table<AsrDailyAgentProcessedDocument>
              rowKey={(record) =>
                [
                  record.agent_id || "daily_report",
                  record.date,
                  record.last_run_id || record.report_path || record.output_dir || "",
                ].join(":")
              }
              size="small"
              tableLayout="fixed"
              scroll={{ x: DAILY_AGENT_RECORDS_TABLE_SCROLL_X }}
              dataSource={filteredDocuments}
              pagination={{ pageSize: 10, hideOnSinglePage: true }}
              columns={[
                { title: t("asrTasks.field.date"), dataIndex: "date", width: 120 },
                {
                  title: t("asrTasks.field.agent"),
                  dataIndex: "agent_name",
                  width: 140,
                  render: (v, record) => <Tag>{v || record.agent_id}</Tag>,
                },
                {
                  title: t("asrTasks.records.output"),
                  dataIndex: "output_dir",
                  width: 120,
                  render: (v) => <Text code>{v || "report"}</Text>,
                },
                {
                  title: t("asrTasks.field.processedAt"),
                  dataIndex: "processed_at_ms",
                  width: 180,
                  render: (v) => (v ? new Date(v).toLocaleString() : "-"),
                },
                {
                  title: "SHA256",
                  dataIndex: "source_sha256",
                  width: 100,
                  render: (v) => (
                    <Text code style={{ fontSize: 10 }}>
                      {v?.slice(0, 8)}
                    </Text>
                  ),
                },
                {
                  title: t("asrTasks.field.size"),
                  dataIndex: "source_len_bytes",
                  width: 80,
                  render: (v: number) =>
                    v < 1024
                      ? `${v} B`
                      : v < 1024 * 1024
                        ? `${(v / 1024).toFixed(1)} KB`
                        : `${(v / 1024 / 1024).toFixed(1)} MB`,
                },
                {
                  title: t("asrTasks.field.runner"),
                  dataIndex: "runner",
                  width: 100,
                  render: (v) => <Tag>{v}</Tag>,
                },
                {
                  title: t("asrTasks.records.report"),
                  dataIndex: "report_path",
                  width: 220,
                  ellipsis: true,
                  render: (v, record) =>
                    v ? (
                      <Button
                        type="link"
                        size="small"
                        data-testid={`asr-daily-agent-report-link-${record.agent_id}-${record.date}`}
                        style={{ padding: 0, height: "auto", fontSize: 11 }}
                        onClick={() => onOpenReport?.(record.date, record.agent_id)}
                      >
                        {v.split("/").pop()}
                      </Button>
                    ) : (
                      "-"
                    ),
                },
              ]}
              locale={{ emptyText: t("asrTasks.records.noMatching") }}
            />
          </div>
        </Space>
      ) : (
        <Empty description={t("asrTasks.records.empty")} />
      )}
    </Card>
  );
}
