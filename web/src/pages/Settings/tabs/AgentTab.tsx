/**
 * Agent Tab — Main configuration component
 * Sub-components are in ./agent/ directory
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Button,
  Card,
  Col,
  Divider,
  Empty,
  Input,
  Row,
  Select,
  Space,
  Spin,
  Typography,
  message,
} from "antd";
import { ReloadOutlined, ReadOutlined, RobotOutlined } from "@ant-design/icons";
import { get, patch } from "../../../api/client";
import { BASE, type AgentConfig } from "./agent/types";
import * as imGatewayApi from "../../../api/imGateway";
import type {
  ExternalCliGatewayConfig,
  ImProviderConfig,
} from "../../../api/imGateway";
import SkillsSection from "./agent/SkillsSection";
import LongTextModalField from "./agent/LongTextModalField";
import ExternalCliPanel from "./imGateway/ExternalCliPanel";

const { Text } = Typography;

// ─── Main Component ──────────────────────────────────────────────────────────

export default function AgentTab() {
  const [config, setConfig] = useState<AgentConfig | null>(null);
  const [loading, setLoading] = useState(false);
  const [runnerConfig, setRunnerConfig] =
    useState<ExternalCliGatewayConfig | null>(null);
  const [imProviders, setImProviders] = useState<ImProviderConfig[]>([]);
  const updateTimers = useRef<Record<string, ReturnType<typeof setTimeout>>>(
    {},
  );
  const pendingFieldValues = useRef<Record<string, unknown>>({});

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

  const patchField = useCallback(async (field: string, value: unknown) => {
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
  }, []);

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

  const selectedRunnerValue =
    config?.runner || runnerConfig?.defaultRunnerId || runnerOptions[0]?.value;

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

  if (loading && !config) {
    return <Spin style={{ display: "block", margin: "60px auto" }} />;
  }

  if (!config) {
    return <Empty description="Unable to load agent configuration" />;
  }

  return (
    <div
      data-testid="agent-settings-layout"
      style={{
        height: "100%",
        minHeight: 0,
        overflow: "hidden",
      }}
    >
      <div
        data-testid="agent-settings-section-content"
        style={{
          height: "100%",
          minHeight: 0,
          overflowY: "auto",
          overflowX: "hidden",
        }}
      >
        <Row gutter={[16, 16]}>
          {/* Runners */}
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

          {/* General Settings */}
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
                <Button
                  icon={<ReloadOutlined />}
                  onClick={fetchConfig}
                  loading={loading}
                  size="small"
                >
                  Refresh
                </Button>
              }
            >
              <Space direction="vertical" style={{ width: "100%" }}>
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
                  Selects the default runner for IM agent messages. Custom
                  runners are managed in this Agent Runners section.
                </Text>

                <Divider style={{ margin: "12px 0" }} />

                <Row justify="space-between" align="middle" gutter={16}>
                  <Col flex="none">
                    <Text>Working Directory</Text>
                  </Col>
                  <Col flex="auto" style={{ textAlign: "right" }}>
                    <Input
                      value={config.work_dir || ""}
                      onChange={(e) =>
                        handleStringChange("work_dir", e.target.value)
                      }
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
                  onChange={(value) =>
                    handleStringChange("base_instructions", value)
                  }
                  placeholder="Optional instructions for external runners"
                  description="Sent only with the first message of a new external-runner session. When empty, Bifrost adds nothing."
                  testId="settings-agent-base-instructions"
                />

                <Divider style={{ margin: "12px 0" }} />

                <LongTextModalField
                  label="Developer Instructions"
                  value={config.developer_instructions || ""}
                  onChange={(value) =>
                    handleStringChange("developer_instructions", value)
                  }
                  placeholder="Optional developer-level instructions"
                  description="Sent with every external-runner message after the optional base instructions. Empty values add nothing."
                  testId="settings-agent-developer-instructions"
                />

                <Divider style={{ margin: "12px 0" }} />

                <LongTextModalField
                  label="User Instructions"
                  value={config.user_instructions || ""}
                  onChange={(value) =>
                    handleStringChange("user_instructions", value)
                  }
                  placeholder="Optional user-level instructions"
                  description="Sent with every external-runner message after developer instructions. Empty values add nothing."
                  testId="settings-agent-user-instructions"
                />
              </Space>
            </Card>
          </Col>

          {/* Skills */}
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
        </Row>
      </div>
    </div>
  );
}
