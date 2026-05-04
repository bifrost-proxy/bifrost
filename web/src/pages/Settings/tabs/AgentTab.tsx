/**
 * Agent Tab — Main configuration component
 * Sub-components are in ./agent/ directory
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { useSearchParams } from "react-router-dom";
import {
  Button,
  Card,
  Col,
  Divider,
  Empty,
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
} from "antd";
import {
  ReloadOutlined,
  ReadOutlined,
  RobotOutlined,
  SettingOutlined,
  ThunderboltOutlined,
  LinkOutlined,
  KeyOutlined,
} from "@ant-design/icons";
import { get, patch } from "../../../api/client";
import {
  BASE,
  DEFAULTS,
  getEffectiveCompactLimit,
  type AgentConfig,
  type McpServerConfig,
  type ProviderInfo,
} from "./agent/types";
import McpServersSection from "./agent/McpServersSection";
import SkillsSection from "./agent/SkillsSection";
import UnifiedSessionsSection from "./agent/UnifiedSessionsSection";
import SessionDetailPage from "./agent/SessionDetailPage";
import MemoriesSection from "./agent/MemoriesSection";

const { Text } = Typography;
const { TextArea } = Input;

// ─── Main Component ──────────────────────────────────────────────────────────

export default function AgentTab() {
  const [config, setConfig] = useState<AgentConfig | null>(null);
  const [loading, setLoading] = useState(false);
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const updateTimers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});

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

  const fetchProviders = useCallback(async () => {
    try {
      const data = await get<ProviderInfo[]>(`${BASE}/agent/providers`);
      setProviders(data);
    } catch {
      // silent — fallback to empty list
    }
  }, []);

  useEffect(() => {
    fetchConfig();
    fetchProviders();
  }, [fetchConfig, fetchProviders]);

  useEffect(() => {
    const timers = updateTimers.current;
    return () => {
      Object.values(timers).forEach(clearTimeout);
    };
  }, []);

  const patchField = useCallback(
    async (field: string, value: unknown) => {
      try {
        const updated = await patch<AgentConfig>(`${BASE}/agent`, {
          [field]: value,
        });
        setConfig(updated);
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
      updateTimers.current[field] = setTimeout(() => {
        patchField(field, value);
      }, delay);
    },
    [patchField],
  );

  const handleNumberChange = (field: string, value: number | null) => {
    if (value === null) {
      // Clear the field — send null to backend to fall back to default
      setConfig((prev) => (prev ? { ...prev, [field]: undefined } : prev));
      debouncedPatch(field, null as unknown as number);
      return;
    }
    setConfig((prev) => (prev ? { ...prev, [field]: value } : prev));
    debouncedPatch(field, value);
  };

  const handleStringChange = (field: string, value: string) => {
    setConfig((prev) => (prev ? { ...prev, [field]: value } : prev));
    debouncedPatch(field, value, 800);
  };

  const handleSelectChange = (field: string, value: string) => {
    setConfig((prev) => (prev ? { ...prev, [field]: value } : prev));
    patchField(field, value);
  };

  const handleSwitchChange = (field: string, value: boolean) => {
    setConfig((prev) => (prev ? { ...prev, [field]: value } : prev));
    patchField(field, value);
  };

  const handleMcpServersUpdate = (servers: Record<string, McpServerConfig>) => {
    setConfig((prev) => (prev ? { ...prev, mcp_servers: servers } : prev));
    patchField("mcp_servers", servers);
  };

  const getProviderField = useCallback(
    (field: string): unknown => {
      if (!config) return undefined;
      const providerId = config.model_provider || "aidp_crawl";
      const provider = config.model_providers?.[providerId];
      return provider?.[field];
    },
    [config],
  );

  // Session detail navigation via URL params
  const [searchParams, setSearchParams] = useSearchParams();
  const selectedSession = searchParams.get("session");
  const sessionView = (searchParams.get("view") || "active") as "active" | "history";
  const historyFilePath = searchParams.get("historyPath") || undefined;

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

  if (selectedSession) {
    return (
      <SessionDetailPage
        sessionKey={selectedSession}
        view={sessionView}
        historyFilePath={historyFilePath}
        onBack={handleBackFromSession}
      />
    );
  }

  return (
    <div>
      <Row gutter={[16, 16]}>
        {/* General Settings */}
        <Col xs={24}>
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
                Enable or disable the agent
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
                Agent working directory for file operations
              </Text>

              <Divider style={{ margin: "12px 0" }} />

              <Text>Instructions / System Prompt</Text>
              <TextArea
                value={config.instructions || ""}
                onChange={(e) =>
                  handleStringChange("instructions", e.target.value)
                }
                placeholder="System prompt instructions for the agent..."
                rows={5}
                style={{ fontFamily: "monospace", fontSize: 12 }}
              />
              <Text type="secondary" style={{ fontSize: 12 }}>
                Custom system prompt to guide agent behavior
              </Text>
            </Space>
          </Card>
        </Col>

        {/* Model Configuration */}
        <Col xs={24}>
          <Card
            title={
              <Space>
                <SettingOutlined />
                <span>Model Configuration</span>
              </Space>
            }
            size="small"
          >
            <Space direction="vertical" style={{ width: "100%" }}>
              <Row justify="space-between" align="middle" gutter={16}>
                <Col flex="none">
                  <Text>Model</Text>
                </Col>
                <Col flex="auto" style={{ textAlign: "right" }}>
                  <Input
                    value={config.model ?? DEFAULTS.model}
                    onChange={(e) => handleStringChange("model", e.target.value)}
                    placeholder={DEFAULTS.model}
                    style={{ maxWidth: 480 }}
                    size="small"
                  />
                </Col>
              </Row>
              <Text type="secondary" style={{ fontSize: 12 }}>
                Model name identifier
              </Text>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle" gutter={16}>
                <Col flex="none">
                  <Text>Model Provider</Text>
                </Col>
                <Col flex="auto" style={{ textAlign: "right" }}>
                  <Select
                    value={config.model_provider || undefined}
                    onChange={(val) => handleSelectChange("model_provider", val)}
                    placeholder="Select a provider"
                    style={{ width: 280 }}
                    size="small"
                    showSearch
                    optionFilterProp="label"
                    options={[
                      ...providers.map((p) => ({
                        label: p.name,
                        value: p.id,
                      })),
                    ]}
                  />
                </Col>
              </Row>
              {(() => {
                const selected = providers.find(
                  (p) => p.id === config.model_provider,
                );
                if (!selected) return null;
                return (
                  <Space
                    direction="vertical"
                    size={2}
                    style={{ width: "100%", marginTop: 4 }}
                  >
                    {selected.base_url && (
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        <LinkOutlined style={{ marginRight: 4 }} />
                        API URL: <Text code style={{ fontSize: 11 }}>{selected.base_url}</Text>
                      </Text>
                    )}
                    {selected.env_key && (
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        <KeyOutlined style={{ marginRight: 4 }} />
                        Default Env: <Text code style={{ fontSize: 11 }}>{selected.env_key}</Text>
                      </Text>
                    )}
                    {!selected.env_key && !selected.base_url && (
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        Local provider — no API key required
                      </Text>
                    )}
                  </Space>
                );
              })()}

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle" gutter={16}>
                <Col flex="none">
                  <Tooltip title="API Key for the model provider. Use $ENV_VAR to read from an environment variable (e.g. $OPENAI_API_KEY).">
                    <Text>API Key</Text>
                  </Tooltip>
                </Col>
                <Col flex="auto" style={{ textAlign: "right" }}>
                  <Input.Password
                    value={
                      (getProviderField("api_key") as string | undefined) ?? ""
                    }
                    onChange={(e) => {
                      const val = e.target.value;
                      setConfig((prev) => {
                        if (!prev) return prev;
                        const providerId = prev.model_provider || "aidp_crawl";
                        const existingProvider = prev.model_providers?.[providerId] || {};
                        return {
                          ...prev,
                          model_providers: {
                            ...prev.model_providers,
                            [providerId]: {
                              ...existingProvider,
                              api_key: val || undefined,
                            },
                          },
                        };
                      });
                      debouncedPatch("api_key", val, 800);
                    }}
                    placeholder={(() => {
                      const selected = providers.find(
                        (p) => p.id === config.model_provider,
                      );
                      return selected?.env_key
                        ? `$${selected.env_key}`
                        : "sk-xxx... or $ENV_VAR_NAME";
                    })()}
                    style={{ maxWidth: 480 }}
                    size="small"
                  />
                </Col>
              </Row>
              <Text type="secondary" style={{ fontSize: 12 }}>
                Direct key or <Text code style={{ fontSize: 11 }}>$VAR_NAME</Text> to resolve from environment variable. Empty = <Text code style={{ fontSize: 11 }}>{(() => {
                  const selected = providers.find(
                    (p) => p.id === config.model_provider,
                  );
                  return selected?.env_key ? `$${selected.env_key}` : "provider default";
                })()}</Text>
              </Text>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Text>Max Completion Tokens</Text>
                </Col>
                <Col>
                  <InputNumber
                    value={config.max_completion_tokens}
                    onChange={(val) =>
                      handleNumberChange("max_completion_tokens", val)
                    }
                    min={1}
                    step={1000}
                    style={{ width: 140 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Text>Reasoning Effort</Text>
                </Col>
                <Col>
                  <Select
                    value={config.model_reasoning_effort ?? DEFAULTS.model_reasoning_effort}
                    onChange={(val) =>
                      handleSelectChange("model_reasoning_effort", val)
                    }
                    placeholder="Select"
                    style={{ width: 140 }}
                    size="small"
                    options={[
                      { label: "None (disabled)", value: "none" },
                      { label: "Low", value: "low" },
                      { label: "Medium", value: "medium" },
                      { label: "High", value: "high" },
                    ]}
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Text>Reasoning Summary</Text>
                </Col>
                <Col>
                  <Select
                    value={config.model_reasoning_summary ?? DEFAULTS.model_reasoning_summary}
                    onChange={(val) =>
                      handleSelectChange("model_reasoning_summary", val)
                    }
                    placeholder="Select"
                    style={{ width: 140 }}
                    size="small"
                    options={[
                      { label: "None (disabled)", value: "none" },
                      { label: "Auto", value: "auto" },
                      { label: "Concise", value: "concise" },
                      { label: "Detailed", value: "detailed" },
                    ]}
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Max context window tokens">
                    <Text>Context Window</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <InputNumber
                    value={config.model_context_window ?? DEFAULTS.model_context_window}
                    onChange={(val) =>
                      handleNumberChange("model_context_window", val)
                    }
                    min={1}
                    step={1000}
                    style={{ width: 140 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Token threshold for auto-compacting context. When empty, defaults to context_window × 90%.">
                    <Text>Auto Compact Token Limit</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <InputNumber
                    value={config.model_auto_compact_token_limit ?? undefined}
                    placeholder={`${getEffectiveCompactLimit(config)} (90%)`}
                    onChange={(val) =>
                      handleNumberChange("model_auto_compact_token_limit", val)
                    }
                    min={1}
                    step={1000}
                    style={{ width: 180 }}
                    size="small"
                  />
                </Col>
              </Row>
              <Divider style={{ margin: "12px 0" }} />

              <Text strong style={{ fontSize: 13, marginBottom: 8, display: "block" }}>
                Provider Connection
              </Text>

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Max retries for failed API requests">
                    <Text>Request Max Retries</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <InputNumber
                    value={getProviderField("request_max_retries") as number | undefined}
                    onChange={(val) => handleNumberChange("request_max_retries", val)}
                    min={0}
                    max={20}
                    step={1}
                    style={{ width: 140 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Stream idle timeout in milliseconds (0 = no timeout)">
                    <Text>Stream Idle Timeout (ms)</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <InputNumber
                    value={getProviderField("stream_idle_timeout_ms") as number | undefined}
                    onChange={(val) => handleNumberChange("stream_idle_timeout_ms", val)}
                    min={0}
                    step={10000}
                    style={{ width: 140 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Max retries for stream reconnection">
                    <Text>Stream Max Retries</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <InputNumber
                    value={getProviderField("stream_max_retries") as number | undefined}
                    onChange={(val) => handleNumberChange("stream_max_retries", val)}
                    min={0}
                    max={20}
                    step={1}
                    style={{ width: 140 }}
                    size="small"
                  />
                </Col>
              </Row>
            </Space>
          </Card>
        </Col>

        {/* Runtime Settings */}
        <Col xs={24}>
          <Card
            title={
              <Space>
                <ThunderboltOutlined />
                <span>Runtime Settings</span>
              </Space>
            }
            size="small"
          >
            <Space direction="vertical" style={{ width: "100%" }}>
              <Row justify="space-between" align="middle">
                <Col>
                  <Text>Shell Timeout (secs)</Text>
                </Col>
                <Col>
                  <InputNumber
                    value={config.shell_timeout_secs}
                    onChange={(val) => handleNumberChange("shell_timeout_secs", val)}
                    min={1}
                    step={10}
                    style={{ width: 120 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Text>Max Turn Iterations</Text>
                </Col>
                <Col>
                  <InputNumber
                    value={config.max_turn_iterations ?? DEFAULTS.max_turn_iterations}
                    onChange={(val) => handleNumberChange("max_turn_iterations", val)}
                    min={1}
                    step={1}
                    style={{ width: 120 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Text>Max History Messages</Text>
                </Col>
                <Col>
                  <InputNumber
                    value={config.max_history_messages}
                    onChange={(val) =>
                      handleNumberChange("max_history_messages", val)
                    }
                    min={1}
                    step={10}
                    style={{ width: 120 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Text>Session TTL (secs)</Text>
                </Col>
                <Col>
                  <InputNumber
                    value={config.session_ttl_secs ?? DEFAULTS.session_ttl_secs}
                    onChange={(val) => handleNumberChange("session_ttl_secs", val)}
                    min={60}
                    step={60}
                    style={{ width: 120 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Text>Request Timeout (secs)</Text>
                </Col>
                <Col>
                  <InputNumber
                    value={config.request_timeout_secs ?? DEFAULTS.request_timeout_secs}
                    onChange={(val) =>
                      handleNumberChange("request_timeout_secs", val)
                    }
                    min={1}
                    step={10}
                    style={{ width: 120 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Text>Tool Output Token Limit</Text>
                </Col>
                <Col>
                  <InputNumber
                    value={config.tool_output_token_limit}
                    onChange={(val) =>
                      handleNumberChange("tool_output_token_limit", val)
                    }
                    min={100}
                    step={1000}
                    placeholder="10000"
                    style={{ width: 120 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Text>Project Doc Max Bytes</Text>
                </Col>
                <Col>
                  <InputNumber
                    value={config.project_doc_max_bytes ?? DEFAULTS.project_doc_max_bytes}
                    onChange={(val) =>
                      handleNumberChange("project_doc_max_bytes", val)
                    }
                    min={0}
                    step={1024}
                    style={{ width: 120 }}
                    size="small"
                  />
                </Col>
              </Row>
              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Maximum poll window for background terminal output (ms)">
                    <Text>Background Terminal Timeout (ms)</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <InputNumber
                    value={config.background_terminal_max_timeout ?? DEFAULTS.background_terminal_max_timeout}
                    onChange={(val) =>
                      handleNumberChange("background_terminal_max_timeout", val)
                    }
                    min={10000}
                    step={60000}
                    style={{ width: 140 }}
                    size="small"
                  />
                </Col>
              </Row>
            </Space>
          </Card>
        </Col>

        {/* History & Session */}
        <Col xs={24}>
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

        {/* Memories */}
        <Col xs={24}>
          <Card
            title={
              <Space>
                <SettingOutlined />
                <span>Memories</span>
              </Space>
            }
            size="small"
          >
            <Space direction="vertical" style={{ width: "100%" }}>
              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Generate memories from conversation threads">
                    <Text>Generate Memories</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <Switch
                    checked={config.memories?.generate_memories ?? true}
                    onChange={(checked) => {
                      const newMemories = {
                        ...(config.memories ?? {}),
                        generate_memories: checked,
                      };
                      setConfig((prev) =>
                        prev ? { ...prev, memories: newMemories } : prev,
                      );
                      patchField("memories", newMemories);
                    }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Inject memory usage instructions into developer prompts">
                    <Text>Use Memories</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <Switch
                    checked={config.memories?.use_memories ?? true}
                    onChange={(checked) => {
                      const newMemories = {
                        ...(config.memories ?? {}),
                        use_memories: checked,
                      };
                      setConfig((prev) =>
                        prev ? { ...prev, memories: newMemories } : prev,
                      );
                      patchField("memories", newMemories);
                    }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Disable memories when MCP or web search provide external context">
                    <Text>Disable on External Context</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <Switch
                    checked={config.memories?.disable_on_external_context ?? false}
                    onChange={(checked) => {
                      const newMemories = {
                        ...(config.memories ?? {}),
                        disable_on_external_context: checked,
                      };
                      setConfig((prev) =>
                        prev ? { ...prev, memories: newMemories } : prev,
                      );
                      patchField("memories", newMemories);
                    }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Maximum recent raw memories retained for consolidation">
                    <Text>Max Raw Memories</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <InputNumber
                    value={config.memories?.max_raw_memories_for_consolidation}
                    onChange={(val) => {
                      const newMemories = {
                        ...(config.memories ?? {}),
                        max_raw_memories_for_consolidation: val ?? undefined,
                      };
                      setConfig((prev) =>
                        prev ? { ...prev, memories: newMemories } : prev,
                      );
                      patchField("memories", newMemories);
                    }}
                    min={1}
                    max={4096}
                    step={10}
                    style={{ width: 140 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Max days since last use before memory becomes ineligible">
                    <Text>Max Unused Days</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <InputNumber
                    value={config.memories?.max_unused_days}
                    onChange={(val) => {
                      const newMemories = {
                        ...(config.memories ?? {}),
                        max_unused_days: val ?? undefined,
                      };
                      setConfig((prev) =>
                        prev ? { ...prev, memories: newMemories } : prev,
                      );
                      patchField("memories", newMemories);
                    }}
                    min={1}
                    step={1}
                    style={{ width: 140 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Maximum age of threads used for memories (days)">
                    <Text>Max Rollout Age (days)</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <InputNumber
                    value={config.memories?.max_rollout_age_days}
                    onChange={(val) => {
                      const newMemories = {
                        ...(config.memories ?? {}),
                        max_rollout_age_days: val ?? undefined,
                      };
                      setConfig((prev) =>
                        prev ? { ...prev, memories: newMemories } : prev,
                      );
                      patchField("memories", newMemories);
                    }}
                    min={1}
                    step={1}
                    style={{ width: 140 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Model used for thread summarisation">
                    <Text>Extract Model</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <Input
                    value={config.memories?.extract_model ?? ""}
                    onChange={(e) => {
                      const newMemories = {
                        ...(config.memories ?? {}),
                        extract_model: e.target.value || undefined,
                      };
                      setConfig((prev) =>
                        prev ? { ...prev, memories: newMemories } : prev,
                      );
                      patchField("memories", newMemories);
                    }}
                    placeholder="Default"
                    style={{ width: 200 }}
                    size="small"
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Model used for memory consolidation">
                    <Text>Consolidation Model</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <Input
                    value={config.memories?.consolidation_model ?? ""}
                    onChange={(e) => {
                      const newMemories = {
                        ...(config.memories ?? {}),
                        consolidation_model: e.target.value || undefined,
                      };
                      setConfig((prev) =>
                        prev ? { ...prev, memories: newMemories } : prev,
                      );
                      patchField("memories", newMemories);
                    }}
                    placeholder="Default"
                    style={{ width: 200 }}
                    size="small"
                  />
                </Col>
              </Row>
            </Space>
          </Card>
        </Col>

        {/* Skills */}
        <Col xs={24}>
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

        {/* MCP Servers */}
        <Col xs={24}>
          <Card
            title={
              <Space>
                <SettingOutlined />
                <span>Memory Records</span>
              </Space>
            }
            size="small"
          >
            <MemoriesSection />
          </Card>
        </Col>

        {/* MCP Servers */}
        <Col xs={24}>
          <Card
            title={
              <Space>
                <ThunderboltOutlined />
                <span>MCP Servers</span>
                <Tag>{Object.keys(config.mcp_servers || {}).length}</Tag>
              </Space>
            }
            size="small"
          >
            <McpServersSection
              servers={config.mcp_servers || {}}
              onUpdate={handleMcpServersUpdate}
            />
          </Card>
        </Col>

        {/* Sessions (Unified — Active + History) */}
        <Col xs={24}>
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
      </Row>
    </div>
  );
}
