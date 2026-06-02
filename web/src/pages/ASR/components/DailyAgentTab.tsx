import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Descriptions,
  Empty,
  Input,
  message,
  Select,
  Space,
  Switch,
  Table,
  Tag,
  Tooltip,
  Typography,
} from "antd";
import {
  PlayCircleOutlined,
  ReloadOutlined,
  SaveOutlined,
  SendOutlined,
  SyncOutlined,
  ThunderboltOutlined,
  PlusOutlined,
  DeleteOutlined,
} from "@ant-design/icons";
import type {
  AsrDailyAgentConfig,
  AsrDailyAgentConfigResponse,
  AsrDailyAgentInstructionsResponse,
  AsrDailyAgentItem,
  AsrDailyAgentProcessedDocument,
  AsrDailyAgentRunsResponse,
} from "../../../api/asr";
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
  const [running, setRunning] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [instructionsText, setInstructionsText] = useState("");
  const [instructionsDirty, setInstructionsDirty] = useState(false);
  const [selectedAgentId, setSelectedAgentId] = useState<string>("daily_report");
  const [reportSyncDir, setReportSyncDir] = useState("");
  const [reportSyncDirDirty, setReportSyncDirDirty] = useState(false);

  const fetchAll = useCallback(async () => {
    setLoading(true);
    try {
      const [config, runners, providers, targets] = await Promise.all([
        getDailyAgentConfig(taskId),
        imGatewayApi.getExternalCliConfig(),
        imGatewayApi.listProviders(),
        imGatewayApi.listTargets(),
      ]);
      const agents = config.config.agents || [];
      const nextSelectedAgentId = agents.some((agent) => agent.id === selectedAgentId)
        ? selectedAgentId
        : agents[0]?.id || "daily_report";
      const instr = await getDailyAgentInstructions(taskId, nextSelectedAgentId);
      setConfigData(config);
      setInstructions(instr);
      setSelectedAgentId(nextSelectedAgentId);
      setInstructionsText(instr.content);
      setInstructionsDirty(false);
      setReportSyncDir(config.config.report_sync_dir || "");
      setReportSyncDirDirty(false);
      setRunnerConfig(runners);
      setImProviders(providers);
      setImTargets(targets);
    } catch (error: unknown) {
      message.error(`Failed to load Daily Agent config: ${errorMessage(error)}`);
    } finally {
      setLoading(false);
    }
  }, [selectedAgentId, taskId]);

  useEffect(() => {
    fetchAll();
  }, [fetchAll]);

  const handleConfigUpdate = async (
    updates: Partial<AsrDailyAgentConfig>
  ) => {
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
    return [
      { label: "Bifrost Agent", value: "bifrost_agent" },
      ...runnerIds.map((id) => ({ label: id, value: id })),
    ];
  }, [runnerConfig?.runners]);

  const agents = useMemo(
    () => configData?.config.agents || [],
    [configData?.config.agents]
  );

  const selectedAgent = useMemo(
    () => agents.find((agent) => agent.id === selectedAgentId) || agents[0],
    [agents, selectedAgentId]
  );

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
      runner: template.runner || "bifrost_agent",
      timeout_ms: template.timeout_ms || 7_200_000,
      trigger_policy: "after_asr_run",
      instructions_source: "default",
      im_delivery: {
        enabled: false,
        mode: "full_report",
        send_policy: "on_success_with_report",
      },
      output_dir: id,
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
    void saveAgents(nextAgents);
  };

  const handleRunnerChange = (value: string) => {
    void handleConfigUpdate({ runner: value });
  };

  const imChannelOptions = useMemo<ImChannelOption[]>(() => {
    const providerById = new Map(
      imProviders.map((provider) => [provider.id, provider])
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

  const resolveImChannelValue = (
    imDelivery: AsrDailyAgentConfig["im_delivery"]
  ) => {
    return imDelivery.channel || undefined;
  };

  const handleImChannelChange = (
    value: string | undefined,
    config: AsrDailyAgentConfig
  ) => {
    void handleConfigUpdate({
      im_delivery: {
        ...config.im_delivery,
        channel: value || "",
      },
    });
  };

  const handleSaveInstructions = async () => {
    setSaving(true);
    try {
      await updateDailyAgentInstructions(taskId, instructionsText, selectedAgent?.id);
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

  const handleSyncReports = async () => {
    setSyncing(true);
    try {
      const result = await syncDailyAgentReports(taskId);
      message.success(
        `Synced ${result.sync.copied_files} copied, ${result.sync.skipped_files} skipped`
      );
      fetchAll();
    } catch (error: unknown) {
      message.error(`Sync failed: ${errorMessage(error)}`);
      fetchAll();
    } finally {
      setSyncing(false);
    }
  };

  const handleRun = async (force: boolean) => {
    setRunning(true);
    try {
      const result = await triggerDailyAgentRun(taskId, { force });
      message.success(result.message || "Run queued");
      // Poll status until no longer running
      const pollInterval = setInterval(async () => {
        try {
          const config = await getDailyAgentConfig(taskId);
          setConfigData(config);
          if (config.last_run?.status !== "running") {
            clearInterval(pollInterval);
            setRunning(false);
            fetchAll();
          }
        } catch {
          clearInterval(pollInterval);
          setRunning(false);
        }
      }, 3000);
      // Safety timeout: stop polling after 10 minutes
      setTimeout(() => {
        clearInterval(pollInterval);
        setRunning(false);
        fetchAll();
      }, 600_000);
    } catch (error: unknown) {
      message.error(`Run failed: ${errorMessage(error)}`);
      setRunning(false);
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
  const reportSync = config?.last_report_sync;

  if (loading && !configData) {
    return (
      <div style={{ padding: 24, textAlign: "center" }}>
        <Text type="secondary">Loading Daily Agent...</Text>
      </div>
    );
  }

  return (
    <Space direction="vertical" size={16} style={{ width: "100%" }}>
      {/* Configuration */}
      <Card size="small" title="Configuration" loading={loading && !config}>
        {config && (
          <Descriptions column={2} size="small" bordered>
            <Descriptions.Item label="Enabled">
              <Switch
                checked={config.enabled}
                onChange={(checked) =>
                  handleConfigUpdate({ enabled: checked })
                }
                loading={saving}
              />
            </Descriptions.Item>
            <Descriptions.Item label="Runner">
              <Select
                data-testid="asr-daily-agent-runner-select"
                size="small"
                value={config.runner || undefined}
                placeholder="Select runner..."
                onChange={handleRunnerChange}
                loading={loading}
                disabled={saving}
                style={{ width: 240 }}
                options={runnerOptions}
              />
            </Descriptions.Item>
            <Descriptions.Item label="Trigger Policy">
              <Select
                size="small"
                value={config.trigger_policy}
                onChange={(value) =>
                  handleConfigUpdate({ trigger_policy: value })
                }
                style={{ width: 160 }}
                options={[
                  { label: "After ASR Run", value: "after_asr_run" },
                  { label: "Manual Only", value: "manual_only" },
                ]}
              />
            </Descriptions.Item>
            <Descriptions.Item label="Timeout">
              <Text>{Math.round(config.timeout_ms / 60000)} min</Text>
            </Descriptions.Item>
            <Descriptions.Item label="Session Key">
              <Text code>{config.session_key || "(auto)"}</Text>
            </Descriptions.Item>
            <Descriptions.Item label="Report Sync Dir" span={2}>
              <Space.Compact style={{ width: "100%" }}>
                <Input
                  data-testid="asr-daily-agent-report-sync-dir"
                  size="small"
                  value={reportSyncDir}
                  placeholder="Optional directory for iCloud or external sync"
                  onChange={(event) => {
                    setReportSyncDir(event.target.value);
                    setReportSyncDirDirty(true);
                  }}
                  onPressEnter={handleSaveReportSyncDir}
                  disabled={saving}
                />
                <Button
                  size="small"
                  icon={<SaveOutlined />}
                  onClick={handleSaveReportSyncDir}
                  disabled={!reportSyncDirDirty}
                  loading={saving}
                >
                  Save
                </Button>
              </Space.Compact>
            </Descriptions.Item>
          </Descriptions>
        )}
        {config && (
          <Space style={{ marginTop: 8 }}>
            <Button
              data-testid="asr-daily-agent-sync-reports-button"
              icon={<SyncOutlined />}
              onClick={handleSyncReports}
              loading={syncing}
              disabled={!config.report_sync_dir?.trim()}
            >
              Sync Reports
            </Button>
            {config.report_sync_dir?.trim() ? (
              <Text type="secondary" style={{ fontSize: 12 }}>
                Copies reports to the configured directory.
              </Text>
            ) : (
              <Text type="secondary" style={{ fontSize: 12 }}>
                Set a directory to enable manual report sync.
              </Text>
            )}
          </Space>
        )}
      </Card>

      <Card size="small" title="Daily Agents" loading={loading && !config}>
        <Alert
          type="info"
          showIcon
          style={{ marginBottom: 12 }}
          message="Each ASR daily markdown is processed by enabled agents in order. Agent id/name/output directory must use English letters, numbers, '_' or '-'."
        />
        <Table<AsrDailyAgentItem>
          data-testid="asr-daily-agents-table"
          size="small"
          rowKey="id"
          dataSource={agents}
          pagination={false}
          columns={[
            {
              title: "Enabled",
              dataIndex: "enabled",
              width: 90,
              render: (enabled, record) => (
                <Switch
                  checked={enabled}
                  onChange={(checked) => updateAgent(record.id, { enabled: checked })}
                />
              ),
            },
            {
              title: "Agent",
              dataIndex: "name",
              width: 180,
              render: (value, record) => (
                <Input
                  size="small"
                  value={value}
                  status={DAILY_AGENT_TOKEN_RE.test(value) ? undefined : "error"}
                  onChange={(event) => {
                    const name = normalizeAgentToken(event.target.value);
                    updateAgent(record.id, { name, output_dir: record.output_dir || name });
                  }}
                />
              ),
            },
            {
              title: "Output Dir",
              dataIndex: "output_dir",
              width: 170,
              render: (value, record) => (
                <Input
                  size="small"
                  value={value}
                  status={DAILY_AGENT_TOKEN_RE.test(value) ? undefined : "error"}
                  onChange={(event) =>
                    updateAgent(record.id, { output_dir: normalizeAgentToken(event.target.value) })
                  }
                />
              ),
            },
            {
              title: "Runner",
              dataIndex: "runner",
              width: 220,
              render: (value, record) => (
                <Select
                  size="small"
                  value={value || undefined}
                  onChange={(runner) => updateAgent(record.id, { runner })}
                  style={{ width: 190 }}
                  options={runnerOptions}
                />
              ),
            },
            {
              title: "IM",
              dataIndex: ["im_delivery", "enabled"],
              width: 90,
              render: (_, record) => (
                <Switch
                  checked={record.im_delivery.enabled}
                  onChange={(enabled) => updateAgent(record.id, { im_delivery: { ...record.im_delivery, enabled } })}
                />
              ),
            },
            {
              title: "Channel",
              width: 240,
              render: (_, record) => (
                <Select
                  size="small"
                  allowClear
                  value={record.im_delivery.channel || undefined}
                  onChange={(channel) => updateAgent(record.id, { im_delivery: { ...record.im_delivery, channel: channel || "" } })}
                  style={{ width: 220 }}
                  options={imChannelOptions}
                />
              ),
            },
            {
              title: "Actions",
              width: 150,
              render: (_, record) => (
                <Space>
                  <Button size="small" onClick={() => setSelectedAgentId(record.id)}>Edit MD</Button>
                  <Button size="small" onClick={() => void triggerDailyAgentRun(taskId, { force: false, agentId: record.id })}>Run</Button>
                  <Button size="small" danger icon={<DeleteOutlined />} onClick={() => removeAgent(record.id)} />
                </Space>
              ),
            },
          ]}
        />
        <Button style={{ marginTop: 12 }} icon={<PlusOutlined />} onClick={addAgent}>
          Add Agent
        </Button>
      </Card>

      {/* IM Delivery */}
      <Card size="small" title="IM Delivery" loading={loading && !config}>
        {config && (
          <Descriptions column={2} size="small" bordered>
            <Descriptions.Item label="Enabled">
              <Switch
                checked={config.im_delivery.enabled}
                onChange={(checked) =>
                  handleConfigUpdate({
                    im_delivery: { ...config.im_delivery, enabled: checked },
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
                value={resolveImChannelValue(config.im_delivery)}
                placeholder="Select channel..."
                onChange={(value) => handleImChannelChange(value, config)}
                loading={loading}
                disabled={saving}
                style={{ width: 260 }}
                options={imChannelOptions}
              />
            </Descriptions.Item>
            <Descriptions.Item label="Mode">
              <Select
                size="small"
                value={config.im_delivery.mode}
                onChange={(value) =>
                  handleConfigUpdate({
                    im_delivery: { ...config.im_delivery, mode: value },
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
                value={config.im_delivery.send_policy}
                onChange={(value) =>
                  handleConfigUpdate({
                    im_delivery: {
                      ...config.im_delivery,
                      send_policy: value,
                    },
                  })
                }
                style={{ width: 200 }}
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
          </Descriptions>
        )}
      </Card>

      {/* Status */}
      {configData?.last_run && (
        <Card size="small" title="Last Run Status">
          <Descriptions column={2} size="small">
            <Descriptions.Item label="Status">
              <Tag
                color={
                  configData.last_run.status === "success"
                    ? "green"
                    : configData.last_run.status === "failed"
                      ? "red"
                      : "default"
                }
              >
                {configData.last_run.status || "never"}
              </Tag>
            </Descriptions.Item>
            <Descriptions.Item label="Run ID">
              <Text code style={{ fontSize: 11 }}>
                {configData.last_run.run_id?.slice(0, 16) || "-"}
              </Text>
            </Descriptions.Item>
            <Descriptions.Item label="Last Run">
              <Text>
                {configData.last_run.last_run_at_ms
                  ? new Date(
                      configData.last_run.last_run_at_ms
                    ).toLocaleString()
                  : "Never"}
              </Text>
            </Descriptions.Item>
            {configData.last_run.error && (
              <Descriptions.Item label="Error" span={2}>
                <Alert
                  type="error"
                  message={configData.last_run.error}
                  showIcon
                  banner
                />
              </Descriptions.Item>
            )}
          </Descriptions>
          {configData.workspace && (
            <Descriptions column={2} size="small" style={{ marginTop: 8 }}>
              <Descriptions.Item label="Reports">
                {configData.workspace.report_count}
              </Descriptions.Item>
              <Descriptions.Item label="Git">
                <Tag
                  color={
                    configData.workspace.git_initialized ? "green" : "orange"
                  }
                >
                  {configData.workspace.git_initialized
                    ? "Initialized"
                    : "Not initialized"}
                </Tag>
              </Descriptions.Item>
            </Descriptions>
          )}
          {reportSync && (
            <>
              <Descriptions column={3} size="small" style={{ marginTop: 8 }}>
                <Descriptions.Item label="Report Sync">
                  <Tag color={reportSync.failed_files > 0 ? "red" : "green"}>
                    {reportSync.copied_files} copied / {reportSync.total_files} total
                  </Tag>
                </Descriptions.Item>
                <Descriptions.Item label="Skipped">
                  {reportSync.skipped_files}
                </Descriptions.Item>
                <Descriptions.Item label="Last Sync">
                  {reportSync.synced_at_ms
                    ? new Date(reportSync.synced_at_ms).toLocaleString()
                    : "-"}
                </Descriptions.Item>
                <Descriptions.Item label="Sync Dir" span={3}>
                  <Text code style={{ fontSize: 11 }}>
                    {reportSync.target_dir}
                  </Text>
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
            <>
              <Descriptions column={3} size="small" style={{ marginTop: 8 }}>
                <Descriptions.Item label="Indexed Reports">
                  <Tag color={reportIndex.unindexed_reports > 0 ? "orange" : "green"}>
                    {reportIndex.indexed_reports}/{reportIndex.report_files}
                  </Tag>
                </Descriptions.Item>
                <Descriptions.Item label="Unindexed Reports">
                  <Tag color={reportIndex.unindexed_reports > 0 ? "orange" : "green"}>
                    {reportIndex.unindexed_reports}
                  </Tag>
                </Descriptions.Item>
                <Descriptions.Item label="Missing Reports">
                  <Tag color={reportIndex.processed_missing_report > 0 ? "red" : "green"}>
                    {reportIndex.processed_missing_report}
                  </Tag>
                </Descriptions.Item>
              </Descriptions>
              {reportIndex.unindexed_reports > 0 && (
                <Alert
                  style={{ marginTop: 8 }}
                  type="warning"
                  showIcon
                  message={`Unindexed report dates: ${reportIndex.unindexed_dates
                    .slice(0, 5)
                    .join(", ")}${
                    reportIndex.unindexed_dates.length > 5 ? "..." : ""
                  }`}
                />
              )}
            </>
          )}
        </Card>
      )}

      {/* Actions */}
      <Space>
        <Tooltip title="Incremental run: only Daily Agent state changes are processed.">
          <Button
            type="primary"
            icon={<PlayCircleOutlined />}
            onClick={() => handleRun(false)}
            loading={running}
            disabled={!config?.enabled}
          >
            Run Now
          </Button>
        </Tooltip>
        <Tooltip title="Force run: rebuild all matching daily reports.">
          <Button
            icon={<ThunderboltOutlined />}
            onClick={() => handleRun(true)}
            loading={running}
            disabled={!config?.enabled}
          >
            Force Run
          </Button>
        </Tooltip>
        <Tooltip title="Send latest report via IM">
          <Button
            icon={<SendOutlined />}
            onClick={handleSend}
            disabled={!config?.im_delivery.enabled}
          >
            Send Report
          </Button>
        </Tooltip>
        <Button icon={<ReloadOutlined />} onClick={fetchAll} loading={loading}>
          Refresh
        </Button>
      </Space>

      {/* Instructions Editor */}
      <Card
        size="small"
        title={`Agent Instructions (${selectedAgent?.name || selectedAgentId})`}
        extra={
          <Space>
          <Select
            size="small"
            value={selectedAgent?.id || selectedAgentId}
            style={{ width: 180 }}
            options={agents.map((agent) => ({ label: agent.name, value: agent.id }))}
            onChange={async (agentId) => {
              setSelectedAgentId(agentId);
              const instr = await getDailyAgentInstructions(taskId, agentId);
              setInstructions(instr);
              setInstructionsText(instr.content);
              setInstructionsDirty(false);
            }}
          />
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
          </Space>
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
          placeholder="Agent instructions..."
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

export function DailyAgentRecordsTab({ taskId, onOpenReport }: DailyAgentTabProps) {
  const [runsData, setRunsData] = useState<AsrDailyAgentRunsResponse | null>(null);
  const [loading, setLoading] = useState(false);

  const fetchRuns = useCallback(async () => {
    setLoading(true);
    try {
      setRunsData(await getDailyAgentRuns(taskId));
    } catch (error: unknown) {
      message.error(`Failed to load Daily Agent records: ${errorMessage(error)}`);
    } finally {
      setLoading(false);
    }
  }, [taskId]);

  useEffect(() => {
    fetchRuns();
  }, [fetchRuns]);

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

  return (
    <Card
      size="small"
      title="Run Results"
      loading={loading && !runsData}
      extra={
        <Button icon={<ReloadOutlined />} onClick={fetchRuns} loading={loading}>
          Refresh
        </Button>
      }
    >
      {runsData && processedDocuments.length > 0 ? (
        <div data-testid="asr-daily-agent-run-results-table">
          <Table<AsrDailyAgentProcessedDocument>
            rowKey={(record) => `${record.agent_id || "daily_report"}:${record.date}`}
            size="small"
            dataSource={processedDocuments}
            pagination={{ pageSize: 10, hideOnSinglePage: true }}
            columns={[
              { title: "Date", dataIndex: "date", width: 120 },
              {
                title: "Agent",
                dataIndex: "agent_name",
                width: 140,
                render: (v, record) => <Tag>{v || record.agent_id}</Tag>,
              },
              {
                title: "Output",
                dataIndex: "output_dir",
                width: 120,
                render: (v) => <Text code>{v || "report"}</Text>,
              },
              {
                title: "Processed At",
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
                title: "Size",
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
                title: "Runner",
                dataIndex: "runner",
                width: 100,
                render: (v) => <Tag>{v}</Tag>,
              },
              {
                title: "Report",
                dataIndex: "report_path",
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
          />
        </div>
      ) : (
        <Empty description="No Daily Agent records yet" />
      )}
    </Card>
  );
}
