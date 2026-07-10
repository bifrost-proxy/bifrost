import { Alert, Badge, Button, Card, Col, Divider, InputNumber, Popconfirm, Row, Slider, Space, Switch, Typography, theme } from "antd";
import {
  ThunderboltOutlined,
  FolderOutlined,
  DeleteOutlined,
  FileOutlined,
  SwapOutlined,
} from "@ant-design/icons";
import type {
  BreakpointPerformanceConfig,
  PerformanceConfig,
  ResourceAlertLevel,
  ResourceAlertStatus,
  TrafficConfig,
} from "../../../api/config";

const { Text } = Typography;

const resourceAlertBadgeStatus: Record<ResourceAlertLevel, "success" | "warning" | "error"> = {
  ok: "success",
  warn: "warning",
  critical: "error",
};

const resourceAlertType: Record<ResourceAlertLevel, "success" | "warning" | "error"> = {
  ok: "success",
  warn: "warning",
  critical: "error",
};

function alertTitle(level: ResourceAlertLevel): string {
  switch (level) {
    case "critical":
      return "Open-file pressure is critically high";
    case "warn":
      return "Open-file usage is approaching the limit";
    case "ok":
      return "Open-file usage is healthy";
  }
}

function resourceTextColor(
  level: ResourceAlertLevel,
  warningColor: string,
  errorColor: string,
): string | undefined {
  if (level === "critical") {
    return errorColor;
  }
  if (level === "warn") {
    return warningColor;
  }
  return undefined;
}

function statusMessage(status: ResourceAlertStatus | null | undefined): string | null {
  return status?.level && status.level !== "ok" ? status.message : null;
}

function formatTimeoutLabel(ms: number): string {
  return `${Math.round(ms / 1000)}s`;
}

export interface PerformanceTabProps {
  perfLoading: boolean;
  performanceConfig: PerformanceConfig | null;
  trafficDraft?: TrafficConfig | null;
  breakpointDraft?: BreakpointPerformanceConfig | null;
  maxRecordsMin: number;
  maxRecordsMax: number;
  maxRecordsStep: number;
  maxRecordsMarks: Record<number, string>;
  maxDbSizeMarks: Record<number, string>;
  maxBodyInlineMarks: Record<number, string>;
  maxBodyBufferMarks: Record<number, string>;
  maxBodyProbeMarks: Record<number, string>;
  fileRetentionMarks: Record<number, string>;
  breakpointTimeoutMarks: Record<number, string>;
  highlightSuperPerformanceMode?: boolean;
  handleSuperPerformanceModeChange: (checked: boolean) => void;
  handleMaxRecordsChange: (value: number | null) => void;
  handleMaxDbSizeChange: (value: number) => void;
  handleMaxBodyMemorySizeChange: (value: number) => void;
  handleMaxBodyBufferSizeChange: (value: number) => void;
  handleMaxBodyProbeSizeChange: (value: number) => void;
  handleEnableBinaryTrafficCaptureChange: (checked: boolean) => void;
  handleFileRetentionDaysChange: (value: number) => void;
  handleBreakpointTimeoutChange: (value: number) => void;
  handleClearBodyCache: () => void;
  formatBytes: (bytes: number) => string;
}

export default function PerformanceTab({
  perfLoading,
  performanceConfig,
  trafficDraft,
  breakpointDraft,
  maxRecordsMin,
  maxRecordsMax,
  maxRecordsStep,
  maxRecordsMarks,
  maxDbSizeMarks,
  maxBodyInlineMarks,
  maxBodyBufferMarks,
  maxBodyProbeMarks,
  fileRetentionMarks,
  breakpointTimeoutMarks,
  highlightSuperPerformanceMode = false,
  handleSuperPerformanceModeChange,
  handleMaxRecordsChange,
  handleMaxDbSizeChange,
  handleMaxBodyMemorySizeChange,
  handleMaxBodyBufferSizeChange,
  handleMaxBodyProbeSizeChange,
  handleEnableBinaryTrafficCaptureChange,
  handleFileRetentionDaysChange,
  handleBreakpointTimeoutChange,
  handleClearBodyCache,
  formatBytes,
}: PerformanceTabProps) {
  const { token } = theme.useToken();
  const resourceAlerts = performanceConfig?.resource_alerts;
  const overallAlertLevel = resourceAlerts?.overall_level ?? "ok";
  const bodyAlert = resourceAlerts?.body_stream_writers;
  const wsAlert = resourceAlerts?.ws_payload_writers;
  const alertMessages = [statusMessage(bodyAlert), statusMessage(wsAlert)].filter(
    (message): message is string => Boolean(message),
  );

  return (
    <Row gutter={[16, 16]} data-testid="settings-performance-tab">
      <Col xs={24}>
        <Card
          title={
            <Space>
              <ThunderboltOutlined />
              <span>Performance</span>
            </Space>
          }
          size="small"
          loading={perfLoading && !performanceConfig}
        >
          <div style={{ paddingLeft: 24,paddingRight:12 }}>
            <Space direction="vertical" style={{ width: "100%" }} size="middle">
              {overallAlertLevel !== "ok" && (
                <Alert
                  type={resourceAlertType[overallAlertLevel]}
                  showIcon
                  message={alertTitle(overallAlertLevel)}
                  description={
                    <Space direction="vertical" size={4}>
                      {alertMessages.map((message) => (
                        <Text key={message}>{message}</Text>
                      ))}
                    </Space>
                  }
                />
              )}

              <div
                data-testid="settings-performance-super-mode-highlight"
                style={{
                  borderRadius: token.borderRadiusLG,
                  boxShadow: highlightSuperPerformanceMode
                    ? `0 0 0 3px ${token.colorWarningBorder}, 0 0 0 8px ${token.colorWarningBg}`
                    : undefined,
                  transition: "box-shadow 180ms ease",
                }}
              >
                <Alert
                  type={trafficDraft?.super_performance_mode ? "warning" : "info"}
                  showIcon
                  message="Super Performance Mode"
                  description="When enabled, Bifrost still processes proxy rules but records no traffic entries, request or response bodies, WebSocket frames, or traffic database updates. Network will show no traffic history while this mode is on."
                  action={
                    <Switch
                      checked={trafficDraft?.super_performance_mode ?? false}
                      onChange={handleSuperPerformanceModeChange}
                      data-testid="settings-performance-super-mode"
                    />
                  }
                />
              </div>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col flex="1" style={{ marginRight: 16 }}>
                  <Space direction="vertical" size={0} style={{ width: "100%" }}>
                    <Text>Max Records</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      Traffic retention is controlled by your configured record
                      limit. Older entries are pruned first, and the allowed
                      upper bound is 100,000.
                    </Text>
                    <Slider
                      min={maxRecordsMin}
                      max={maxRecordsMax}
                      step={maxRecordsStep}
                      value={trafficDraft?.max_records}
                      onChange={handleMaxRecordsChange}
                      marks={maxRecordsMarks}
                      data-testid="settings-performance-max-records"
                      tooltip={{
                        formatter: (value) =>
                          value !== null && value !== undefined
                            ? value.toLocaleString()
                            : "",
                      }}
                    />
                  </Space>
                </Col>
                <Col>
                  <Text code>
                    {(trafficDraft?.max_records || 0).toLocaleString()}
                  </Text>
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col flex="1" style={{ marginRight: 16 }}>
                  <Space direction="vertical" size={0} style={{ width: "100%" }}>
                    <Text>Breakpoint Auto-Resume Timeout</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      Applies when a matched breakpoint rule pauses traffic
                      and nobody clicks Resume. The request or response is
                      automatically released after this timeout to avoid
                      long network stalls and connection buildup. Lower
                      values reduce latency risk; higher values give more
                      time for manual debugging.
                    </Text>
                    <div data-testid="settings-performance-breakpoint-timeout">
                      <Slider
                        min={breakpointDraft?.timeout_min_ms ?? 0}
                        max={breakpointDraft?.timeout_max_ms ?? 1}
                        step={1000}
                        value={breakpointDraft?.timeout_ms ?? 0}
                        disabled={!breakpointDraft}
                        onChange={handleBreakpointTimeoutChange}
                        marks={breakpointDraft ? breakpointTimeoutMarks : {}}
                        tooltip={{
                          formatter: (value) =>
                            value !== null && value !== undefined
                              ? formatTimeoutLabel(value)
                              : "",
                        }}
                      />
                    </div>
                    <div
                      data-testid="settings-performance-breakpoint-timeout-bounds"
                      style={{
                        display: "flex",
                        justifyContent: "space-between",
                        marginTop: -6,
                      }}
                    >
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        Min:{" "}
                        {breakpointDraft
                          ? `${formatTimeoutLabel(breakpointDraft.timeout_min_ms)} (${breakpointDraft.timeout_min_ms}ms)`
                          : "Loading"}
                      </Text>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        Max:{" "}
                        {breakpointDraft
                          ? `${formatTimeoutLabel(breakpointDraft.timeout_max_ms)} (${breakpointDraft.timeout_max_ms}ms)`
                          : "Loading"}
                      </Text>
                    </div>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {breakpointDraft ? (
                        <>
                          Fixed safety range:{" "}
                          {formatTimeoutLabel(breakpointDraft.timeout_min_ms)}
                          {" - "}
                          {formatTimeoutLabel(breakpointDraft.timeout_max_ms)}.
                          Values outside this range are rejected to avoid long
                          paused connections.
                        </>
                      ) : (
                        "Loading fixed safety range..."
                      )}
                    </Text>
                  </Space>
                </Col>
                <Col>
                  <Space direction="vertical" size={4} align="end">
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      Current
                    </Text>
                    <div data-testid="settings-performance-breakpoint-timeout-input">
                      <InputNumber
                        min={breakpointDraft?.timeout_min_ms}
                        max={breakpointDraft?.timeout_max_ms}
                        step={1000}
                        value={breakpointDraft?.timeout_ms}
                        disabled={!breakpointDraft}
                        onChange={(value) => {
                          if (typeof value === "number") {
                            handleBreakpointTimeoutChange(value);
                          }
                        }}
                        addonAfter="ms"
                        style={{ width: 160 }}
                      />
                    </div>
                    <Text code>
                      {breakpointDraft
                        ? formatTimeoutLabel(breakpointDraft.timeout_ms)
                        : "Loading"}
                    </Text>
                  </Space>
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col flex="1" style={{ marginRight: 16 }}>
                  <Space direction="vertical" size={0} style={{ width: "100%" }}>
                    <Text>Enable Binary File Capture and Decoding</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      Record and decode binary downloads and media streams in traffic details.
                      This setting has higher priority than DB size, inline size, and body buffer
                      limits, and can noticeably reduce proxy throughput while increasing CPU,
                      memory, and disk usage. It is disabled by default, which keeps binary traffic
                      in performance mode.
                    </Text>
                  </Space>
                </Col>
                <Col>
                  <Switch
                    checked={!trafficDraft?.binary_traffic_performance_mode}
                    onChange={handleEnableBinaryTrafficCaptureChange}
                  />
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col flex="1" style={{ marginRight: 16 }}>
                  <Space direction="vertical" size={0} style={{ width: "100%" }}>
                    <Text>Max DB Size</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      Caps traffic.db + body_cache + frames + ws_payload on
                      disk; when exceeded, the oldest data is deleted first.
                    </Text>
                    <Slider
                      min={256 * 1024 * 1024}
                      max={10 * 1024 * 1024 * 1024}
                      step={256 * 1024 * 1024}
                      value={trafficDraft?.max_db_size_bytes}
                      onChange={handleMaxDbSizeChange}
                      marks={maxDbSizeMarks}
                      data-testid="settings-performance-max-db-size"
                      tooltip={{
                        formatter: (value) => (value ? formatBytes(value) : ""),
                      }}
                    />
                  </Space>
                </Col>
                <Col>
                  <Text code>
                    {formatBytes(trafficDraft?.max_db_size_bytes || 0)}
                  </Text>
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col flex="1" style={{ marginRight: 16 }}>
                  <Space direction="vertical" size={0} style={{ width: "100%" }}>
                    <Text>Max Body Inline Size (DB)</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      Bodies up to this size are stored inline in SQLite; larger
                      bodies are stored as files in body_cache.
                    </Text>
                    <Slider
                      min={64 * 1024}
                      max={10 * 1024 * 1024}
                      step={64 * 1024}
                      value={trafficDraft?.max_body_memory_size}
                      onChange={handleMaxBodyMemorySizeChange}
                      marks={maxBodyInlineMarks}
                      tooltip={{
                        formatter: (value) => (value ? formatBytes(value) : ""),
                      }}
                    />
                  </Space>
                </Col>
                <Col>
                  <Text code>
                    {formatBytes(trafficDraft?.max_body_memory_size || 0)}
                  </Text>
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col flex="1" style={{ marginRight: 16 }}>
                  <Space direction="vertical" size={0} style={{ width: "100%" }}>
                    <Text>Max Body Buffer Size</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      Maximum body size to capture; larger bodies are truncated
                      and forwarded as streams (rules may skip).
                    </Text>
                    <Slider
                      min={1 * 1024 * 1024}
                      max={64 * 1024 * 1024}
                      step={1 * 1024 * 1024}
                      value={trafficDraft?.max_body_buffer_size}
                      onChange={handleMaxBodyBufferSizeChange}
                      marks={maxBodyBufferMarks}
                      tooltip={{
                        formatter: (value) => (value ? formatBytes(value) : ""),
                      }}
                    />
                  </Space>
                </Col>
                <Col>
                  <Text code>
                    {formatBytes(trafficDraft?.max_body_buffer_size || 0)}
                  </Text>
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col flex="1" style={{ marginRight: 16 }}>
                  <Space direction="vertical" size={0} style={{ width: "100%" }}>
                    <Text>Max Body Probe Size</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      Default 64 KB. For non-text or suspected large bodies,
                      only pre-read up to this size; if exceeded, body
                      processing (rules/scripts) is skipped and the body is
                      forwarded as a stream. Lower values reduce memory/CPU but
                      may skip more body-based processing; higher values improve
                      processing coverage at higher cost.
                    </Text>
                    <Slider
                      min={0}
                      max={1 * 1024 * 1024}
                      step={16 * 1024}
                      value={trafficDraft?.max_body_probe_size}
                      onChange={handleMaxBodyProbeSizeChange}
                      marks={maxBodyProbeMarks}
                      tooltip={{
                        formatter: (value) =>
                          value !== null && value !== undefined
                            ? value === 0
                              ? "Off"
                              : formatBytes(value)
                            : "",
                      }}
                    />
                  </Space>
                </Col>
                <Col>
                  <Text code>
                    {trafficDraft?.max_body_probe_size === 0
                      ? "Off"
                      : formatBytes(trafficDraft?.max_body_probe_size || 0)}
                  </Text>
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col flex="1" style={{ marginRight: 16 }}>
                  <Space direction="vertical" size={0} style={{ width: "100%" }}>
                    <Text>File Retention Days</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      Files older than this are deleted (body and WebSocket
                      payload cache).
                    </Text>
                    <Slider
                      min={1}
                      max={7}
                      step={1}
                      value={trafficDraft?.file_retention_days}
                      onChange={handleFileRetentionDaysChange}
                      marks={fileRetentionMarks}
                    />
                  </Space>
                </Col>
                <Col>
                  <Text code>{trafficDraft?.file_retention_days || 0} days</Text>
                </Col>
              </Row>

              {(performanceConfig?.body_store_stats ||
                performanceConfig?.frame_store_stats ||
                performanceConfig?.ws_payload_store_stats) && (
                <>
                  <Divider style={{ margin: "12px 0" }} />
                  <Card
                    size="small"
                    bordered={false}
                    style={{ background: token.colorBgLayout }}
                  >
                    <Row gutter={[16, 8]} align="middle">
                      <Col flex="auto">
                        <Space>
                          <FolderOutlined />
                          <Text strong>File Storage Statistics</Text>
                        </Space>
                      </Col>
                      <Col>
                        <Popconfirm
                          title="Clear all cache files?"
                          description="This will delete all cached data including body files, traffic records, WebSocket frames, and WebSocket payloads."
                          onConfirm={handleClearBodyCache}
                          okText="Clear"
                          cancelText="Cancel"
                          okButtonProps={{ danger: true }}
                        >
                          <Button
                            size="small"
                            danger
                            icon={<DeleteOutlined />}
                            loading={perfLoading}
                            data-testid="settings-performance-clear-cache"
                          >
                            Clear Cache
                          </Button>
                        </Popconfirm>
                      </Col>
                    </Row>
                    <Row gutter={[16, 8]} style={{ marginTop: 12 }}>
                      <Col xs={24} md={8}>
                        <Space direction="vertical" size={0}>
                          <Space size={6}>
                            <Badge status={resourceAlertBadgeStatus[bodyAlert?.level ?? "ok"]} />
                            <Text type="secondary" style={{ fontSize: 12 }}>
                              Body Cache
                            </Text>
                          </Space>
                          <Space>
                            <FileOutlined />
                            <Text>
                              {performanceConfig.body_store_stats?.file_count ??
                                0}{" "}
                              files
                            </Text>
                          </Space>
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            {formatBytes(
                              performanceConfig.body_store_stats?.total_size ??
                                0,
                            )}
                          </Text>
                          <Text
                            style={{
                              fontSize: 12,
                              color: resourceTextColor(
                                bodyAlert?.level ?? "ok",
                                token.colorWarning,
                                token.colorError,
                              ),
                            }}
                          >
                            Active streams{" "}
                            {performanceConfig.body_store_stats
                              ?.active_stream_writers ?? 0}
                            /
                            {performanceConfig.body_store_stats
                              ?.max_open_stream_writers ?? 0}
                          </Text>
                        </Space>
                      </Col>
                      <Col xs={24} md={8}>
                        <Space direction="vertical" size={0}>
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            WebSocket Frames
                          </Text>
                          <Space>
                            <SwapOutlined />
                            <Text>
                              {performanceConfig.frame_store_stats
                                ?.connection_count ?? 0}{" "}
                              connections
                            </Text>
                          </Space>
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            {formatBytes(
                              performanceConfig.frame_store_stats?.total_size ??
                                0,
                            )}
                          </Text>
                        </Space>
                      </Col>
                      <Col xs={24} md={8}>
                        <Space direction="vertical" size={0}>
                          <Space size={6}>
                            <Badge status={resourceAlertBadgeStatus[wsAlert?.level ?? "ok"]} />
                            <Text type="secondary" style={{ fontSize: 12 }}>
                              WebSocket Payloads
                            </Text>
                          </Space>
                          <Space>
                            <SwapOutlined />
                            <Text>
                              {performanceConfig.ws_payload_store_stats
                                ?.file_count ?? 0}{" "}
                              files
                            </Text>
                          </Space>
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            {formatBytes(
                              performanceConfig.ws_payload_store_stats
                                ?.total_size ?? 0,
                            )}
                          </Text>
                          <Text
                            style={{
                              fontSize: 12,
                              color: resourceTextColor(
                                wsAlert?.level ?? "ok",
                                token.colorWarning,
                                token.colorError,
                              ),
                            }}
                          >
                            Open writers{" "}
                            {performanceConfig.ws_payload_store_stats
                              ?.active_writers ?? 0}
                            /
                            {performanceConfig.ws_payload_store_stats
                              ?.max_open_files ?? 0}
                          </Text>
                        </Space>
                      </Col>
                    </Row>
                    <Divider style={{ margin: "8px 0" }} />
                    <Row>
                      <Col>
                        <Space>
                          <Text type="secondary">Total Storage:</Text>
                          <Text strong>
                            {formatBytes(
                              (performanceConfig.body_store_stats?.total_size ??
                                0) +
                                (performanceConfig.frame_store_stats
                                  ?.total_size ?? 0) +
                                (performanceConfig.ws_payload_store_stats
                                  ?.total_size ?? 0),
                            )}
                          </Text>
                        </Space>
                      </Col>
                    </Row>
                  </Card>
                </>
              )}
            </Space>
          </div>
        </Card>
      </Col>
    </Row>
  );
}
