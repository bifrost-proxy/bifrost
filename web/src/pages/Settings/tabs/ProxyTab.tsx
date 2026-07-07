import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Alert,
  AutoComplete,
  Button,
  Card,
  Col,
  Descriptions,
  Divider,
  Image,
  Input,
  InputNumber,
  Row,
  Space,
  Switch,
  Tag,
  Tooltip,
  Typography,
  message,
  theme,
} from "antd";
import {
  ApiOutlined,
  CheckCircleOutlined,
  CopyOutlined,
  DownloadOutlined,
  ExclamationCircleOutlined,
  LockOutlined,
  PlusOutlined,
  QrcodeOutlined,
  SafetyCertificateOutlined,
  SwapOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import type { SystemOverview } from "../../../types";
import { getProxyQRCodeUrl } from "../../../api/proxy";
import type {
  CliProxyStatus,
  ProxyAddressInfo,
  SystemProxyLaunchdStatus,
  SystemProxyStatus,
} from "../../../api/proxy";
import {
  getTemporaryPortActiveSummary,
  getTemporaryPorts,
  type TemporaryPortActiveSummary,
  type TemporaryPortBinding,
  type TemporaryPortRuleSetRef,
} from "../../../api/ports";
import type { ProxySettings, TlsConfig } from "../../../api/config";
import { updateTlsConfig } from "../../../api/config";
import {
  getCliInstallStatus,
  installCliFromDesktop,
  type CliInstallStatus,
} from "../../../api/system";
import { useTlsConfigStore } from "../../../stores/useTlsConfigStore";
import SystemProxySection from "./SystemProxySection";

const { Text } = Typography;

export interface CliInstallActionState {
  showInstallCli: boolean;
  showInstallSkills: boolean;
  skillsButtonLabel: string;
}

export function getCliInstallActionState(
  status: CliInstallStatus | null,
): CliInstallActionState {
  const cliInstalled = status?.installed === true;
  return {
    showInstallCli: !cliInstalled,
    showInstallSkills: cliInstalled,
    skillsButtonLabel:
      status?.skills_installed === true ? "Reinstall AI Skills" : "Install AI Skills",
  };
}

function formatRuleRef(ref: TemporaryPortRuleSetRef): string {
  switch (ref.type) {
    case "local_rule":
      return ref.name;
    case "group_rule":
      return `${ref.group_id}/${ref.name}`;
    case "rule_file":
      return ref.path;
    case "inline_rule":
      return ref.content.split(/\r?\n/).find((line) => line.trim()) || "inline rule";
    default:
      return "unknown";
  }
}

function ruleRefColor(ref: TemporaryPortRuleSetRef): string {
  switch (ref.type) {
    case "local_rule":
      return "blue";
    case "group_rule":
      return "purple";
    case "rule_file":
      return "cyan";
    case "inline_rule":
      return "geekblue";
    default:
      return "default";
  }
}

interface TemporaryProxyPortsSectionProps {
  mainPort: number;
}

function TemporaryProxyPortsSection({ mainPort }: TemporaryProxyPortsSectionProps) {
  const { token } = theme.useToken();
  const [ports, setPorts] = useState<TemporaryPortBinding[]>([]);
  const [summaries, setSummaries] = useState<Record<number, TemporaryPortActiveSummary>>({});
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetchPorts = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const nextPorts = await getTemporaryPorts();
      const settled = await Promise.allSettled(
        nextPorts.map(
          async (port) =>
            [port.port, await getTemporaryPortActiveSummary(port.port)] as const,
        ),
      );
      const nextSummaries: Record<number, TemporaryPortActiveSummary> = {};
      for (const result of settled) {
        if (result.status === "fulfilled") {
          const [port, summary] = result.value;
          nextSummaries[port] = summary;
        }
      }
      setPorts(nextPorts);
      setSummaries(nextSummaries);
    } catch (err) {
      setPorts([]);
      setSummaries({});
      setError(err instanceof Error ? err.message : "Failed to load temporary ports");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void fetchPorts();
  }, [fetchPorts]);

  return (
    <Card
      title={
        <Space>
          <ApiOutlined />
          <span>Temporary Proxy Ports</span>
          <Tag color={ports.length > 0 ? "blue" : "default"}>{ports.length}</Tag>
        </Space>
      }
      size="small"
      extra={
        <Button
          icon={<ReloadOutlined />}
          size="small"
          loading={loading}
          onClick={fetchPorts}
          data-testid="settings-temporary-ports-refresh"
        >
          Refresh
        </Button>
      }
    >
      <Space direction="vertical" style={{ width: "100%" }}>
        {error ? (
          <Alert
            type="warning"
            showIcon
            message="Temporary proxy ports are unavailable"
            description={error}
            data-testid="settings-temporary-ports-error"
          />
        ) : null}

        {!error && !loading && ports.length === 0 ? (
          <Text type="secondary" data-testid="settings-temporary-ports-empty">
            Main proxy port {mainPort} is using the default active rules. Temporary
            ports created with bifrost port bind will appear here.
          </Text>
        ) : null}

        <div style={{ width: "100%" }} data-testid="settings-temporary-ports-list">
          {ports.map((port, index) => {
          const summary = summaries[port.port];
          return (
            <div key={port.port} data-testid={`settings-temporary-port-card-${port.port}`}>
              {index > 0 ? <Divider style={{ margin: "16px 0" }} /> : null}
              <Descriptions column={1} size="small">
                <Descriptions.Item label="Port">
                  <Space wrap>
                    <Text code>
                      {port.host}:{port.port}
                    </Text>
                    <Tag color={port.status === "running" ? "green" : "orange"}>
                      {port.status}
                    </Tag>
                  </Space>
                </Descriptions.Item>
                {port.name ? (
                  <Descriptions.Item label="Name">{port.name}</Descriptions.Item>
                ) : null}
                <Descriptions.Item label="Bound Rules">
                  {port.rule_refs.length > 0 ? (
                    <Space wrap>
                      {port.rule_refs.map((ref, refIndex) => (
                        <Tooltip key={`${ref.type}-${refIndex}`} title={formatRuleRef(ref)}>
                          <Tag color={ruleRefColor(ref)}>{formatRuleRef(ref)}</Tag>
                        </Tooltip>
                      ))}
                    </Space>
                  ) : (
                    <Text type="secondary">No bound rule sets</Text>
                  )}
                </Descriptions.Item>
                {port.missing_refs.length > 0 ? (
                  <Descriptions.Item label="Missing Rules">
                    <Space wrap>
                      {port.missing_refs.map((ref, refIndex) => (
                        <Tag key={`${ref.type}-${refIndex}`} color="orange">
                          {formatRuleRef(ref)}
                        </Tag>
                      ))}
                    </Space>
                  </Descriptions.Item>
                ) : null}
                <Descriptions.Item label="Active Rules">
                  {summary ? (
                    summary.rules.length > 0 ? (
                      <Space wrap>
                        {summary.rules.map((rule) => (
                          <Tag
                            key={`${rule.group_id || "local"}-${rule.name}`}
                            color={rule.group_id ? "purple" : "blue"}
                          >
                            {rule.group_name ? `${rule.group_name}/` : ""}
                            {rule.name} · {rule.rule_count}
                          </Tag>
                        ))}
                      </Space>
                    ) : (
                      <Text type="secondary">No active rules resolved</Text>
                    )
                  ) : (
                    <Text type="secondary">Loading active rules...</Text>
                  )}
                </Descriptions.Item>
              </Descriptions>
              <Divider style={{ margin: "12px 0" }} />
              <Text
                type="secondary"
                style={{
                  fontSize: 12,
                  display: "block",
                  marginBottom: 12,
                }}
              >
                Merged Rules for this temporary proxy port
              </Text>
              <pre
                style={{
                  margin: 0,
                  padding: 12,
                  maxHeight: 180,
                  overflow: "auto",
                  whiteSpace: "pre-wrap",
                  wordBreak: "break-word",
                  borderRadius: 6,
                  border: `1px solid ${token.colorBorderSecondary}`,
                  background: token.colorFillQuaternary,
                  color: token.colorText,
                  fontSize: 12,
                }}
                data-testid={`settings-temporary-port-merged-${port.port}`}
              >
                {summary?.merged_content || ""}
              </pre>
            </div>
          );
        })}
        </div>
      </Space>
    </Card>
  );
}

interface TlsInterceptionPatternsCardProps {
  tlsConfig: TlsConfig | null;
  tlsLoading: boolean;
  newIncludePattern: string;
  newExcludePattern: string;
  newAppIncludePattern: string;
  newAppExcludePattern: string;
  setNewIncludePattern: (pattern: string) => void;
  setNewExcludePattern: (pattern: string) => void;
  setNewAppIncludePattern: (pattern: string) => void;
  setNewAppExcludePattern: (pattern: string) => void;
  handleAddIncludePattern: () => void;
  handleRemoveIncludePattern: (pattern: string) => void;
  handleAddExcludePattern: () => void;
  handleRemoveExcludePattern: (pattern: string) => void;
  handleAddAppIncludePattern: () => void;
  handleRemoveAppIncludePattern: (pattern: string) => void;
  handleAddAppExcludePattern: () => void;
  handleRemoveAppExcludePattern: (pattern: string) => void;
  appSuggestions: string[];
}

function TlsInterceptionPatternsCard({
  tlsConfig,
  tlsLoading,
  newIncludePattern,
  newExcludePattern,
  newAppIncludePattern,
  newAppExcludePattern,
  setNewIncludePattern,
  setNewExcludePattern,
  setNewAppIncludePattern,
  setNewAppExcludePattern,
  handleAddIncludePattern,
  handleRemoveIncludePattern,
  handleAddExcludePattern,
  handleRemoveExcludePattern,
  handleAddAppIncludePattern,
  handleRemoveAppIncludePattern,
  handleAddAppExcludePattern,
  handleRemoveAppExcludePattern,
  appSuggestions,
}: TlsInterceptionPatternsCardProps) {
  const { token } = theme.useToken();
  const config = useTlsConfigStore((s) => s.config);
  const [newIpInclude, setNewIpInclude] = useState("");
  const [newIpExclude, setNewIpExclude] = useState("");
  const [ipLoading, setIpLoading] = useState(false);

  const appOptions = useMemo(() => {
    return appSuggestions.map((app) => ({
      value: app,
      label: app,
    }));
  }, [appSuggestions]);

  const handleAddIpInclude = async () => {
    if (!newIpInclude.trim() || !config) return;
    setIpLoading(true);
    try {
      const updated = [...(config.ip_intercept_include || [])];
      if (!updated.includes(newIpInclude.trim())) {
        updated.push(newIpInclude.trim());
      }
      const excluded = (config.ip_intercept_exclude || []).filter(
        (e) => e !== newIpInclude.trim(),
      );
      await updateTlsConfig({
        ip_intercept_include: updated,
        ip_intercept_exclude: excluded,
      });
      useTlsConfigStore.getState().fetchConfig();
      setNewIpInclude("");
    } finally {
      setIpLoading(false);
    }
  };

  const handleRemoveIpInclude = async (ip: string) => {
    if (!config) return;
    const updated = (config.ip_intercept_include || []).filter((e) => e !== ip);
    await updateTlsConfig({ ip_intercept_include: updated });
    useTlsConfigStore.getState().fetchConfig();
  };

  const handleAddIpExclude = async () => {
    if (!newIpExclude.trim() || !config) return;
    setIpLoading(true);
    try {
      const updated = [...(config.ip_intercept_exclude || [])];
      if (!updated.includes(newIpExclude.trim())) {
        updated.push(newIpExclude.trim());
      }
      const included = (config.ip_intercept_include || []).filter(
        (e) => e !== newIpExclude.trim(),
      );
      await updateTlsConfig({
        ip_intercept_exclude: updated,
        ip_intercept_include: included,
      });
      useTlsConfigStore.getState().fetchConfig();
      setNewIpExclude("");
    } finally {
      setIpLoading(false);
    }
  };

  const handleRemoveIpExclude = async (ip: string) => {
    if (!config) return;
    const updated = (config.ip_intercept_exclude || []).filter((e) => e !== ip);
    await updateTlsConfig({ ip_intercept_exclude: updated });
    useTlsConfigStore.getState().fetchConfig();
  };

  return (
    <Card
      title={
        <Space>
          <SwapOutlined />
          <span>TLS Interception Patterns</span>
        </Space>
      }
      size="small"
    >
      <Text
        type="secondary"
        style={{ display: "block", marginBottom: 16, fontSize: 12 }}
      >
        Configure TLS interception behavior by domain, application, or IP.
        Priority: Rules &gt; App Include &gt; App Exclude &gt; Domain Include
        &gt; Domain Exclude &gt; IP Include &gt; IP Exclude &gt; Global.
      </Text>

      <Divider titlePlacement="left" style={{ margin: "0 0 16px 0" }}>
        <Text type="secondary" style={{ fontSize: 12 }}>
          Domain-based Filtering
        </Text>
      </Divider>
      <Row gutter={[16, 16]}>
        <Col xs={24} md={12}>
          <div
            style={{
              padding: 16,
              background: token.colorSuccessBg,
              borderRadius: 8,
              border: `1px solid ${token.colorSuccessBorder}`,
            }}
          >
            <Space
              style={{
                width: "100%",
                justifyContent: "space-between",
                marginBottom: 8,
              }}
            >
              <Space>
                <LockOutlined style={{ color: token.colorSuccess }} />
                <Text strong style={{ color: token.colorSuccessText }}>
                  Force Intercept
                </Text>
                <Tag color="green">
                  {tlsConfig?.intercept_include.length || 0}
                </Tag>
              </Space>
            </Space>
            <Text
              type="secondary"
              style={{
                display: "block",
                marginBottom: 12,
                fontSize: 12,
              }}
            >
              Always intercept these domains, even when global interception is
              OFF.
            </Text>
            <Space.Compact style={{ width: "100%", marginBottom: 12 }}>
              <Input
                placeholder="*.api.example.com"
                value={newIncludePattern}
                onChange={(e) => setNewIncludePattern(e.target.value)}
                onPressEnter={handleAddIncludePattern}
                size="small"
                data-testid="settings-tls-include-input"
              />
              <Button
                type="primary"
                icon={<PlusOutlined />}
                onClick={handleAddIncludePattern}
                size="small"
                loading={tlsLoading}
                data-testid="settings-tls-include-add-button"
                style={{
                  background: token.colorSuccess,
                  borderColor: token.colorSuccess,
                }}
              >
                Add
              </Button>
            </Space.Compact>
            <div>
              {tlsConfig?.intercept_include.length === 0 ? (
                <Text type="secondary">No patterns configured</Text>
              ) : (
                <Space wrap>
                  {tlsConfig?.intercept_include.map((pattern) => (
                    <Tag
                      key={pattern}
                      color="green"
                      closable
                      onClose={() => handleRemoveIncludePattern(pattern)}
                    >
                      {pattern}
                    </Tag>
                  ))}
                </Space>
              )}
            </div>
          </div>
        </Col>
        <Col xs={24} md={12}>
          <div
            style={{
              padding: 16,
              background: token.colorWarningBg,
              borderRadius: 8,
              border: `1px solid ${token.colorWarningBorder}`,
            }}
          >
            <Space
              style={{
                width: "100%",
                justifyContent: "space-between",
                marginBottom: 8,
              }}
            >
              <Space>
                <SafetyCertificateOutlined style={{ color: token.colorWarning }} />
                <Text strong style={{ color: token.colorWarningText }}>
                  Passthrough
                </Text>
                <Tag color="orange">
                  {tlsConfig?.intercept_exclude.length || 0}
                </Tag>
              </Space>
            </Space>
            <Text
              type="secondary"
              style={{
                display: "block",
                marginBottom: 12,
                fontSize: 12,
              }}
            >
              Never intercept these domains, even when global interception is
              ON.
            </Text>
            <Space.Compact style={{ width: "100%", marginBottom: 12 }}>
              <Input
                placeholder="*.apple.com"
                value={newExcludePattern}
                onChange={(e) => setNewExcludePattern(e.target.value)}
                onPressEnter={handleAddExcludePattern}
                size="small"
                data-testid="settings-tls-exclude-input"
              />
              <Button
                type="primary"
                icon={<PlusOutlined />}
                onClick={handleAddExcludePattern}
                size="small"
                loading={tlsLoading}
                data-testid="settings-tls-exclude-add-button"
                style={{
                  background: token.colorWarning,
                  borderColor: token.colorWarning,
                }}
              >
                Add
              </Button>
            </Space.Compact>
            <div>
              {tlsConfig?.intercept_exclude.length === 0 ? (
                <Text type="secondary">No patterns configured</Text>
              ) : (
                <Space wrap>
                  {tlsConfig?.intercept_exclude.map((pattern) => (
                    <Tag
                      key={pattern}
                      color="orange"
                      closable
                      onClose={() => handleRemoveExcludePattern(pattern)}
                    >
                      {pattern}
                    </Tag>
                  ))}
                </Space>
              )}
            </div>
          </div>
        </Col>
      </Row>

      <Divider titlePlacement="left" style={{ margin: "16px 0" }}>
        <Text type="secondary" style={{ fontSize: 12 }}>
          Application-based Filtering
        </Text>
      </Divider>
      <Row gutter={[16, 16]}>
        <Col xs={24} md={12}>
          <div
            style={{
              padding: 16,
              background: token.colorSuccessBg,
              borderRadius: 8,
              border: `1px solid ${token.colorSuccessBorder}`,
            }}
          >
            <Space
              style={{
                width: "100%",
                justifyContent: "space-between",
                marginBottom: 8,
              }}
            >
              <Space>
                <LockOutlined style={{ color: token.colorSuccess }} />
                <Text strong style={{ color: token.colorSuccessText }}>
                  Force Intercept
                </Text>
                <Tag color="green">
                  {tlsConfig?.app_intercept_include.length || 0}
                </Tag>
              </Space>
            </Space>
            <Text
              type="secondary"
              style={{
                display: "block",
                marginBottom: 12,
                fontSize: 12,
              }}
            >
              Always intercept traffic from these apps. Supports: exact match,
              prefix*, *suffix.
            </Text>
            <Space.Compact style={{ width: "100%", marginBottom: 12 }}>
              <AutoComplete
                placeholder="Chrome*, *Browser, Postman"
                value={newAppIncludePattern}
                options={appOptions}
                onChange={(value) => setNewAppIncludePattern(value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    handleAddAppIncludePattern();
                  }
                }}
                size="small"
                style={{ flex: 1 }}
                filterOption={(inputValue, option) =>
                  option?.value
                    .toLowerCase()
                    .includes(inputValue.toLowerCase()) ?? false
                }
                allowClear
              />
              <Button
                type="primary"
                icon={<PlusOutlined />}
                onClick={handleAddAppIncludePattern}
                size="small"
                loading={tlsLoading}
                style={{
                  background: token.colorSuccess,
                  borderColor: token.colorSuccess,
                }}
              >
                Add
              </Button>
            </Space.Compact>
            <div>
              {tlsConfig?.app_intercept_include.length === 0 ? (
                <Text type="secondary">No patterns configured</Text>
              ) : (
                <Space wrap>
                  {tlsConfig?.app_intercept_include.map((pattern) => (
                    <Tag
                      key={pattern}
                      color="green"
                      closable
                      onClose={() => handleRemoveAppIncludePattern(pattern)}
                    >
                      {pattern}
                    </Tag>
                  ))}
                </Space>
              )}
            </div>
          </div>
        </Col>
        <Col xs={24} md={12}>
          <div
            style={{
              padding: 16,
              background: token.colorWarningBg,
              borderRadius: 8,
              border: `1px solid ${token.colorWarningBorder}`,
            }}
          >
            <Space
              style={{
                width: "100%",
                justifyContent: "space-between",
                marginBottom: 8,
              }}
            >
              <Space>
                <SafetyCertificateOutlined style={{ color: token.colorWarning }} />
                <Text strong style={{ color: token.colorWarningText }}>
                  Passthrough
                </Text>
                <Tag color="orange">
                  {tlsConfig?.app_intercept_exclude.length || 0}
                </Tag>
              </Space>
            </Space>
            <Text
              type="secondary"
              style={{
                display: "block",
                marginBottom: 12,
                fontSize: 12,
              }}
            >
              Never intercept traffic from these apps. Supports: exact match,
              prefix*, *suffix.
            </Text>
            <Space.Compact style={{ width: "100%", marginBottom: 12 }}>
              <AutoComplete
                placeholder="System*, *Agent, curl"
                value={newAppExcludePattern}
                options={appOptions}
                onChange={(value) => setNewAppExcludePattern(value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    handleAddAppExcludePattern();
                  }
                }}
                size="small"
                style={{ flex: 1 }}
                filterOption={(inputValue, option) =>
                  option?.value
                    .toLowerCase()
                    .includes(inputValue.toLowerCase()) ?? false
                }
                allowClear
              />
              <Button
                type="primary"
                icon={<PlusOutlined />}
                onClick={handleAddAppExcludePattern}
                size="small"
                loading={tlsLoading}
                style={{
                  background: token.colorWarning,
                  borderColor: token.colorWarning,
                }}
              >
                Add
              </Button>
            </Space.Compact>
            <div>
              {tlsConfig?.app_intercept_exclude.length === 0 ? (
                <Text type="secondary">No patterns configured</Text>
              ) : (
                <Space wrap>
                  {tlsConfig?.app_intercept_exclude.map((pattern) => (
                    <Tag
                      key={pattern}
                      color="orange"
                      closable
                      onClose={() => handleRemoveAppExcludePattern(pattern)}
                    >
                      {pattern}
                    </Tag>
                  ))}
                </Space>
              )}
            </div>
          </div>
        </Col>
      </Row>

      <Divider titlePlacement="left" style={{ margin: "16px 0" }}>
        <Text type="secondary" style={{ fontSize: 12 }}>
          IP-based Filtering
        </Text>
      </Divider>
      <Row gutter={[16, 16]}>
        <Col xs={24} md={12}>
          <div
            style={{
              padding: 16,
              background: token.colorSuccessBg,
              borderRadius: 8,
              border: `1px solid ${token.colorSuccessBorder}`,
            }}
          >
            <Space style={{ width: "100%", justifyContent: "space-between", marginBottom: 8 }}>
              <Space>
                <LockOutlined style={{ color: token.colorSuccess }} />
                <Text strong style={{ color: token.colorSuccessText }}>
                  Force Intercept
                </Text>
                <Tag color="green">
                  {config?.ip_intercept_include?.length || 0}
                </Tag>
              </Space>
            </Space>
            <Text type="secondary" style={{ display: "block", marginBottom: 12, fontSize: 12 }}>
              Always intercept TLS traffic from these IPs.
            </Text>
            <Space.Compact style={{ width: "100%", marginBottom: 12 }}>
              <Input
                placeholder="192.168.1.100 or 10.0.0.0/8"
                value={newIpInclude}
                onChange={(e) => setNewIpInclude(e.target.value)}
                onPressEnter={handleAddIpInclude}
                size="small"
              />
              <Button
                type="primary"
                icon={<PlusOutlined />}
                onClick={handleAddIpInclude}
                size="small"
                loading={ipLoading}
                style={{
                  background: token.colorSuccess,
                  borderColor: token.colorSuccess,
                }}
              >
                Add
              </Button>
            </Space.Compact>
            <div>
              {(config?.ip_intercept_include?.length || 0) === 0 ? (
                <Text type="secondary">No IPs configured</Text>
              ) : (
                <Space wrap>
                  {config?.ip_intercept_include?.map((ip) => (
                    <Tag key={ip} color="green" closable onClose={() => handleRemoveIpInclude(ip)}>
                      {ip}
                    </Tag>
                  ))}
                </Space>
              )}
            </div>
          </div>
        </Col>
        <Col xs={24} md={12}>
          <div
            style={{
              padding: 16,
              background: token.colorWarningBg,
              borderRadius: 8,
              border: `1px solid ${token.colorWarningBorder}`,
            }}
          >
            <Space style={{ width: "100%", justifyContent: "space-between", marginBottom: 8 }}>
              <Space>
                <SafetyCertificateOutlined style={{ color: token.colorWarning }} />
                <Text strong style={{ color: token.colorWarningText }}>
                  Passthrough
                </Text>
                <Tag color="orange">
                  {config?.ip_intercept_exclude?.length || 0}
                </Tag>
              </Space>
            </Space>
            <Text type="secondary" style={{ display: "block", marginBottom: 12, fontSize: 12 }}>
              Never intercept TLS traffic from these IPs.
            </Text>
            <Space.Compact style={{ width: "100%", marginBottom: 12 }}>
              <Input
                placeholder="192.168.1.100 or 10.0.0.0/8"
                value={newIpExclude}
                onChange={(e) => setNewIpExclude(e.target.value)}
                onPressEnter={handleAddIpExclude}
                size="small"
              />
              <Button
                type="primary"
                icon={<PlusOutlined />}
                onClick={handleAddIpExclude}
                size="small"
                loading={ipLoading}
                style={{ background: token.colorWarning, borderColor: token.colorWarning }}
              >
                Add
              </Button>
            </Space.Compact>
            <div>
              {(config?.ip_intercept_exclude?.length || 0) === 0 ? (
                <Text type="secondary">No IPs configured</Text>
              ) : (
                <Space wrap>
                  {config?.ip_intercept_exclude?.map((ip) => (
                    <Tag key={ip} color="orange" closable onClose={() => handleRemoveIpExclude(ip)}>
                      {ip}
                    </Tag>
                  ))}
                </Space>
              )}
            </div>
          </div>
        </Col>
      </Row>
    </Card>
  );
}

const formatUptime = (secs: number): string => {
  const days = Math.floor(secs / 86400);
  const hours = Math.floor((secs % 86400) / 3600);
  const mins = Math.floor((secs % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h ${mins}m`;
  if (hours > 0) return `${hours}h ${mins}m`;
  return `${mins}m ${secs % 60}s`;
};

export interface ProxyTabProps {
  desktopMode: boolean;
  desktopPlatform: string;
  proxySettings: ProxySettings | null;
  desktopExpectedProxyPort: number | null;
  desktopProxyPort: number | null;
  desktopPortDraft: number;
  desktopPortSaving: boolean;
  setDesktopPortDraft: (value: number) => void;
  onApplyDesktopProxyPort: () => void;
  systemProxy: SystemProxyStatus | null;
  systemProxyLaunchd: SystemProxyLaunchdStatus | null;
  cliProxy: CliProxyStatus | null;
  systemProxyLoading: boolean;
  systemProxyLaunchdLoading: boolean;
  onToggleSystemProxy: (enabled: boolean) => void;
  onToggleSystemProxyLaunchd: (enabled: boolean) => void;
  copyProxyConfig: () => void;
  overview: SystemOverview | null;
  proxyAddressInfo: ProxyAddressInfo | null;
  tlsConfig: TlsConfig | null;
  tlsLoading: boolean;
  onToggleTlsInterception: (enabled: boolean) => void;
  onToggleUnsafeSsl: (enabled: boolean) => void;
  onToggleDisconnectOnConfigChange: (enabled: boolean) => void;
  injectBifrostBadge: boolean | null;
  injectBifrostBadgeLoading: boolean;
  onToggleInjectBifrostBadge: (enabled: boolean) => void;
  newIncludePattern: string;
  newExcludePattern: string;
  newAppIncludePattern: string;
  newAppExcludePattern: string;
  setNewIncludePattern: (pattern: string) => void;
  setNewExcludePattern: (pattern: string) => void;
  setNewAppIncludePattern: (pattern: string) => void;
  setNewAppExcludePattern: (pattern: string) => void;
  handleAddIncludePattern: () => void;
  handleRemoveIncludePattern: (pattern: string) => void;
  handleAddExcludePattern: () => void;
  handleRemoveExcludePattern: (pattern: string) => void;
  handleAddAppIncludePattern: () => void;
  handleRemoveAppIncludePattern: (pattern: string) => void;
  handleAddAppExcludePattern: () => void;
  handleRemoveAppExcludePattern: (pattern: string) => void;
  appSuggestions: string[];
}

export default function ProxyTab({
  desktopMode,
  desktopPlatform,
  proxySettings,
  desktopExpectedProxyPort,
  desktopProxyPort,
  desktopPortDraft,
  desktopPortSaving,
  setDesktopPortDraft,
  onApplyDesktopProxyPort,
  systemProxy,
  systemProxyLaunchd,
  cliProxy,
  systemProxyLoading,
  systemProxyLaunchdLoading,
  onToggleSystemProxy,
  onToggleSystemProxyLaunchd,
  copyProxyConfig,
  overview,
  proxyAddressInfo,
  tlsConfig,
  tlsLoading,
  onToggleTlsInterception,
  onToggleUnsafeSsl,
  onToggleDisconnectOnConfigChange,
  injectBifrostBadge,
  injectBifrostBadgeLoading,
  onToggleInjectBifrostBadge,
  newIncludePattern,
  newExcludePattern,
  newAppIncludePattern,
  newAppExcludePattern,
  setNewIncludePattern,
  setNewExcludePattern,
  setNewAppIncludePattern,
  setNewAppExcludePattern,
  handleAddIncludePattern,
  handleRemoveIncludePattern,
  handleAddExcludePattern,
  handleRemoveExcludePattern,
  handleAddAppIncludePattern,
  handleRemoveAppIncludePattern,
  handleAddAppExcludePattern,
  handleRemoveAppExcludePattern,
  appSuggestions,
}: ProxyTabProps) {
  const [cliInstallStatus, setCliInstallStatus] = useState<CliInstallStatus | null>(null);
  const [cliInstallLoading, setCliInstallLoading] = useState(false);

  const refreshCliInstallStatus = useCallback(async () => {
    if (!desktopMode) {
      return;
    }
    try {
      setCliInstallStatus(await getCliInstallStatus());
    } catch {
      setCliInstallStatus(null);
    }
  }, [desktopMode]);

  useEffect(() => {
    void refreshCliInstallStatus();
  }, [refreshCliInstallStatus]);

  const cliInstallActions = getCliInstallActionState(cliInstallStatus);

  const handleInstallCli = useCallback(async () => {
    setCliInstallLoading(true);
    try {
      const status = await installCliFromDesktop({ install_skills: false });
      setCliInstallStatus(status);
      message.success("CLI installed");
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to install CLI");
    } finally {
      setCliInstallLoading(false);
    }
  }, []);

  const handleInstallSkills = useCallback(async () => {
    setCliInstallLoading(true);
    try {
      const status = await installCliFromDesktop({ install_skills: true });
      setCliInstallStatus(status);
      if (status.skills_installed === false) {
        message.warning(status.skills_message || "AI skill setup needs a retry");
      } else {
        message.success("AI skills installed");
      }
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to install AI skills");
    } finally {
      setCliInstallLoading(false);
    }
  }, []);

  return (
    <div>
      <Row gutter={[16, 16]}>
        {desktopMode ? (
          <Col xs={24}>
            <Card
              title={
                <Space>
                  <ApiOutlined />
                  <span>Desktop Proxy Core</span>
                </Space>
              }
              size="small"
            >
              <Space direction="vertical" style={{ width: "100%" }} size="middle">
                <Alert
                  type="info"
                  showIcon
                  message="Changing the port rebinds the embedded bifrost core listener"
                  description={
                    desktopPlatform === "macos"
                      ? "The bundled UI stays in place while the local proxy listener switches ports and reconnects."
                      : "The desktop shell updates the local proxy listener in place and then restores the live desktop connection."
                  }
                />
                <Row gutter={16} align="bottom" data-testid="settings-desktop-port-row">
                  <Col flex="220px">
                    <Space direction="vertical" style={{ width: "100%" }} size={4}>
                      <Text>Proxy Port</Text>
                      <InputNumber
                        data-testid="settings-desktop-port-input"
                        min={1}
                        max={65535}
                        precision={0}
                        style={{ width: "100%" }}
                        value={desktopPortDraft}
                        onChange={(value) =>
                          setDesktopPortDraft(
                            Number(
                              value ??
                                desktopExpectedProxyPort ??
                                proxySettings?.port ??
                                9900,
                            ),
                          )
                        }
                        status={
                          desktopExpectedProxyPort !== null &&
                          desktopPortDraft !== desktopExpectedProxyPort
                            ? "warning"
                            : undefined
                        }
                      />
                    </Space>
                  </Col>
                  <Col flex="none">
                    <Button
                      type="primary"
                      loading={desktopPortSaving}
                      disabled={
                        desktopExpectedProxyPort !== null &&
                        desktopPortDraft === desktopExpectedProxyPort
                      }
                      onClick={onApplyDesktopProxyPort}
                      data-testid="settings-desktop-port-apply"
                    >
                      Apply & Restart
                    </Button>
                  </Col>
                </Row>
                <Text type="secondary" style={{ fontSize: 12 }}>
                  Platform: {desktopPlatform} · Expected port:{" "}
                  {desktopExpectedProxyPort ?? proxySettings?.port ?? 9900} · Actual
                  port: {desktopProxyPort ?? proxySettings?.port ?? 9900}
                </Text>
                {desktopExpectedProxyPort !== null &&
                desktopPortDraft !== desktopExpectedProxyPort ? (
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    Pending change: {desktopExpectedProxyPort} → {desktopPortDraft}
                  </Text>
                ) : null}
                {desktopExpectedProxyPort !== null &&
                desktopProxyPort !== null &&
                desktopExpectedProxyPort !== desktopProxyPort ? (
                  <Alert
                    type="warning"
                    showIcon
                    message={`Expected ${desktopExpectedProxyPort}, running on ${desktopProxyPort}`}
                    description="The preferred startup port was unavailable, so the embedded core automatically moved to the next available port."
                  />
                ) : null}
                <Divider style={{ margin: "4px 0" }} />
                <Row gutter={[16, 12]} align="middle">
                  <Col flex="auto">
                    <Space direction="vertical" size={4} style={{ width: "100%" }}>
                      <Space wrap>
                        <Text strong>Command Line & AI Tools</Text>
                        {cliInstallStatus?.installed ? (
                          <Tag color="green" icon={<CheckCircleOutlined />}>
                            CLI installed
                          </Tag>
                        ) : (
                          <Tag>CLI not installed</Tag>
                        )}
                        {cliInstallStatus?.skills_installed ? (
                          <Tag color="blue">AI skills installed</Tag>
                        ) : null}
                      </Space>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        {cliInstallStatus?.install_path
                          ? `CLI path: ${cliInstallStatus.install_path}`
                          : "Install the bundled CLI so terminals and AI coding tools can call bifrost directly."}
                      </Text>
                      {cliInstallStatus?.path_hint ? (
                        <Text type="secondary" style={{ fontSize: 12 }}>
                          {cliInstallStatus.path_hint}
                        </Text>
                      ) : null}
                      {cliInstallStatus?.skills_message ? (
                        <Text type="secondary" style={{ fontSize: 12 }}>
                          {cliInstallStatus.skills_message}
                        </Text>
                      ) : null}
                    </Space>
                  </Col>
                  <Col flex="none">
                    <Space>
                      <Button
                        icon={<ReloadOutlined />}
                        loading={cliInstallLoading}
                        onClick={refreshCliInstallStatus}
                      >
                        Refresh
                      </Button>
                      {cliInstallActions.showInstallCli ? (
                        <Button
                          type="primary"
                          icon={<DownloadOutlined />}
                          loading={cliInstallLoading}
                          onClick={handleInstallCli}
                          data-testid="settings-install-cli"
                        >
                          Install CLI
                        </Button>
                      ) : null}
                      {cliInstallActions.showInstallSkills ? (
                        <Button
                          type="primary"
                          icon={<DownloadOutlined />}
                          loading={cliInstallLoading}
                          onClick={handleInstallSkills}
                          data-testid="settings-install-skills"
                        >
                          {cliInstallActions.skillsButtonLabel}
                        </Button>
                      ) : null}
                    </Space>
                  </Col>
                </Row>
              </Space>
            </Card>
          </Col>
        ) : null}

        <SystemProxySection
          systemProxy={systemProxy}
          systemProxyLaunchd={systemProxyLaunchd}
          cliProxy={cliProxy}
          systemProxyLoading={systemProxyLoading}
          systemProxyLaunchdLoading={systemProxyLaunchdLoading}
          injectBifrostBadge={injectBifrostBadge}
          injectBifrostBadgeLoading={injectBifrostBadgeLoading}
          onToggleSystemProxy={onToggleSystemProxy}
          onToggleSystemProxyLaunchd={onToggleSystemProxyLaunchd}
          onToggleInjectBifrostBadge={onToggleInjectBifrostBadge}
        />

        <Col xs={24}>
          <TemporaryProxyPortsSection mainPort={overview?.server.port || 9900} />
        </Col>

        <Col xs={24}>
          <Card
            title={
              <Space>
                <ApiOutlined />
                <span>Proxy Address</span>
              </Space>
            }
            size="small"
            extra={
              <Button icon={<CopyOutlined />} size="small" onClick={copyProxyConfig}>
                Copy
              </Button>
            }
          >
            <Descriptions column={1} size="small">
              <Descriptions.Item label="Port">
                <Text code>{overview?.server.port || 9900}</Text>
              </Descriptions.Item>
              <Descriptions.Item label="Admin URL">
                <a
                  href={overview?.server.admin_url}
                  target="_blank"
                  rel="noreferrer"
                >
                  {overview?.server.admin_url}
                </a>
              </Descriptions.Item>
            </Descriptions>
            {proxyAddressInfo && proxyAddressInfo.addresses.length > 0 && (
              <>
                <Divider style={{ margin: "12px 0" }} />
                <Text
                  type="secondary"
                  style={{
                    fontSize: 12,
                    display: "block",
                    marginBottom: 12,
                  }}
                >
                  Available Network Addresses — scan QR code with your device
                </Text>
                <Row gutter={[16, 16]} justify="start">
                  {proxyAddressInfo.addresses.map((addr, index) => (
                    <Col key={addr.ip}>
                      <div style={{ textAlign: "center" }}>
                        <Image
                          src={getProxyQRCodeUrl(addr.ip)}
                          alt={`QR ${addr.address}`}
                          width={120}
                          height={120}
                          preview={{
                            mask: <QrcodeOutlined style={{ fontSize: 20 }} />,
                          }}
                          fallback="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mN8/+F9PQAJpAN4pokyXwAAAABJRU5ErkJggg=="
                          data-testid={
                            index === 0
                              ? "settings-proxy-qrcode"
                              : `settings-proxy-qrcode-${addr.ip}`
                          }
                        />
                        <div style={{ marginTop: 4 }}>
                          <Text code style={{ fontSize: 12 }}>
                            {addr.address}
                          </Text>
                        </div>
                        {addr.is_preferred && (
                          <Tag color="green" style={{ marginTop: 4, fontSize: 11 }}>
                            Recommended
                          </Tag>
                        )}
                      </div>
                    </Col>
                  ))}
                </Row>
              </>
            )}
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            title={
              <Space>
                <LockOutlined />
                <span>TLS/HTTPS Settings</span>
              </Space>
            }
            size="small"
            loading={tlsLoading && !tlsConfig}
          >
            <Space
              direction="vertical"
              style={{ width: "100%" }}
              size="middle"
            >
              <Row justify="space-between" align="middle">
                <Col>
                  <Text>Enable HTTPS Interception</Text>
                </Col>
                <Col>
                  <Switch
                    checked={tlsConfig?.enable_tls_interception}
                    loading={tlsLoading}
                    onChange={onToggleTlsInterception}
                    data-testid="settings-tls-enable-switch"
                  />
                </Col>
              </Row>
              <Text type="secondary" style={{ fontSize: 12 }}>
                Intercept and inspect HTTPS traffic. Requires CA certificate
                installed.
              </Text>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Space>
                    <Text>Skip Certificate Verification</Text>
                    <Tooltip title="Warning: This makes connections insecure">
                      <ExclamationCircleOutlined style={{ color: "#faad14" }} />
                    </Tooltip>
                  </Space>
                </Col>
                <Col>
                  <Switch
                    checked={tlsConfig?.unsafe_ssl}
                    loading={tlsLoading}
                    onChange={onToggleUnsafeSsl}
                    data-testid="settings-tls-unsafe-switch"
                  />
                </Col>
              </Row>
              {tlsConfig?.unsafe_ssl && (
                <Alert
                  type="warning"
                  message="Certificate verification is disabled"
                  description="Only use this in development environments"
                  showIcon
                  style={{ marginTop: 8 }}
                />
              )}

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col>
                  <Tooltip title="Automatically disconnect affected connections when TLS config changes">
                    <Text>Auto-disconnect on Config Change</Text>
                  </Tooltip>
                </Col>
                <Col>
                  <Switch
                    checked={tlsConfig?.disconnect_on_config_change}
                    loading={tlsLoading}
                    onChange={onToggleDisconnectOnConfigChange}
                    data-testid="settings-tls-disconnect-switch"
                  />
                </Col>
              </Row>
              <Text type="secondary" style={{ fontSize: 12 }}>
                When enabled, existing connections will be closed when TLS
                settings change.
              </Text>
            </Space>
          </Card>
        </Col>

        <Col xs={24}>
          <TlsInterceptionPatternsCard
            tlsConfig={tlsConfig}
            tlsLoading={tlsLoading}
            newIncludePattern={newIncludePattern}
            newExcludePattern={newExcludePattern}
            newAppIncludePattern={newAppIncludePattern}
            newAppExcludePattern={newAppExcludePattern}
            setNewIncludePattern={setNewIncludePattern}
            setNewExcludePattern={setNewExcludePattern}
            setNewAppIncludePattern={setNewAppIncludePattern}
            setNewAppExcludePattern={setNewAppExcludePattern}
            handleAddIncludePattern={handleAddIncludePattern}
            handleRemoveIncludePattern={handleRemoveIncludePattern}
            handleAddExcludePattern={handleAddExcludePattern}
            handleRemoveExcludePattern={handleRemoveExcludePattern}
            handleAddAppIncludePattern={handleAddAppIncludePattern}
            handleRemoveAppIncludePattern={handleRemoveAppIncludePattern}
            handleAddAppExcludePattern={handleAddAppExcludePattern}
            handleRemoveAppExcludePattern={handleRemoveAppExcludePattern}
            appSuggestions={appSuggestions}
          />
        </Col>

        

        <Col xs={24}>
          <Card title="System Information" size="small">
            <Descriptions column={1} size="small">
              <Descriptions.Item label="Version">
                <Text code>v{overview?.system.version}</Text>
              </Descriptions.Item>
              <Descriptions.Item label="OS">
                {overview?.system.os} ({overview?.system.arch})
              </Descriptions.Item>
              <Descriptions.Item label="PID">
                {overview?.system.pid}
              </Descriptions.Item>
              <Descriptions.Item label="Uptime">
                {overview ? formatUptime(overview.system.uptime_secs) : "-"}
              </Descriptions.Item>
            </Descriptions>
          </Card>
        </Col>
      </Row>
    </div>
  );
}
