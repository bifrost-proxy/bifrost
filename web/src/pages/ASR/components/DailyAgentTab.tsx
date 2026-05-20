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
  ThunderboltOutlined,
} from "@ant-design/icons";
import type {
  AsrDailyAgentConfig,
  AsrDailyAgentConfigResponse,
  AsrDailyAgentInstructionsResponse,
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
  triggerDailyAgentRun,
  updateDailyAgentConfig,
  updateDailyAgentInstructions,
} from "../../../api/asr";

const { Text } = Typography;
const { TextArea } = Input;

interface DailyAgentTabProps {
  taskId: string;
  onOpenReport?: (date: string) => void;
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
  const [instructionsText, setInstructionsText] = useState("");
  const [instructionsDirty, setInstructionsDirty] = useState(false);

  const fetchAll = useCallback(async () => {
    setLoading(true);
    try {
      const [config, instr, runners, providers, targets] = await Promise.all([
        getDailyAgentConfig(taskId),
        getDailyAgentInstructions(taskId),
        imGatewayApi.getExternalCliConfig(),
        imGatewayApi.listProviders(),
        imGatewayApi.listTargets(),
      ]);
      setConfigData(config);
      setInstructions(instr);
      setInstructionsText(instr.content);
      setInstructionsDirty(false);
      setRunnerConfig(runners);
      setImProviders(providers);
      setImTargets(targets);
    } catch (error: unknown) {
      message.error(`Failed to load Daily Agent config: ${errorMessage(error)}`);
    } finally {
      setLoading(false);
    }
  }, [taskId]);

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
      await updateDailyAgentInstructions(taskId, instructionsText);
      message.success("Instructions saved");
      setInstructionsDirty(false);
    } catch (error: unknown) {
      message.error(`Failed to save instructions: ${errorMessage(error)}`);
    } finally {
      setSaving(false);
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
      const result = await sendDailyAgentReport(taskId);
      if (result.ok) {
        message.success(`Sent ${result.sent_reports.length} report(s)`);
      }
    } catch (error: unknown) {
      message.error(`Send failed: ${errorMessage(error)}`);
    }
  };

  const config = configData?.config;

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
          </Descriptions>
        )}
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
        </Card>
      )}

      {/* Actions */}
      <Space>
        <Button
          type="primary"
          icon={<PlayCircleOutlined />}
          onClick={() => handleRun(false)}
          loading={running}
          disabled={!config?.enabled}
        >
          Run Now
        </Button>
        <Button
          icon={<ThunderboltOutlined />}
          onClick={() => handleRun(true)}
          loading={running}
          disabled={!config?.enabled}
        >
          Force Run
        </Button>
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
        title="Agent Instructions (AGENTS.md)"
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
      {runsData && runsData.processed_documents.length > 0 ? (
        <Table<AsrDailyAgentProcessedDocument>
          rowKey="date"
          size="small"
          dataSource={runsData.processed_documents}
          pagination={{ pageSize: 10, hideOnSinglePage: true }}
          columns={[
            { title: "Date", dataIndex: "date", width: 120 },
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
                    data-testid={`asr-daily-agent-report-link-${record.date}`}
                    style={{ padding: 0, height: "auto", fontSize: 11 }}
                    onClick={() => onOpenReport?.(record.date)}
                  >
                    {v.split("/").pop()}
                  </Button>
                ) : (
                  "-"
                ),
            },
          ]}
        />
      ) : (
        <Empty description="No Daily Agent records yet" />
      )}
    </Card>
  );
}
