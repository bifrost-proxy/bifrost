import { Alert, Badge, Button, Card, Col, Divider, InputNumber, Popconfirm, Row, Slider, Space, Switch, Typography, theme } from "antd";
import type { ComponentProps } from "react";
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
import { useI18n, type TFunction } from "../../../i18n";

const { Text } = Typography;
type SliderMarks = NonNullable<ComponentProps<typeof Slider>["marks"]>;

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

function alertTitle(level: ResourceAlertLevel, t: TFunction): string {
  switch (level) {
    case "critical":
      return t("settings.perf.alertCritical");
    case "warn":
      return t("settings.perf.alertWarn");
    case "ok":
      return t("settings.perf.alertOk");
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
  maxBodyProbeMarks: SliderMarks;
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
  const { t } = useI18n();
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
              <span>{t("settings.tabPerformance")}</span>
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
                  message={alertTitle(overallAlertLevel, t)}
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
                  message={t("settings.perf.superMode")}
                  description={t("settings.perf.superModeDescription")}
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
                    <Text>{t("settings.perf.maxRecords")}</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {t("settings.perf.maxRecordsHint")}
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
                    <Text>{t("settings.perf.breakpointTimeout")}</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {t("settings.perf.breakpointTimeoutHint")}
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
                        {t("settings.perf.min", {
                          value: breakpointDraft
                            ? `${formatTimeoutLabel(breakpointDraft.timeout_min_ms)} (${breakpointDraft.timeout_min_ms}ms)`
                            : t("settings.perf.loading"),
                        })}
                      </Text>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        {t("settings.perf.max", {
                          value: breakpointDraft
                            ? `${formatTimeoutLabel(breakpointDraft.timeout_max_ms)} (${breakpointDraft.timeout_max_ms}ms)`
                            : t("settings.perf.loading"),
                        })}
                      </Text>
                    </div>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {breakpointDraft
                        ? t("settings.perf.safetyRange", {
                            min: formatTimeoutLabel(breakpointDraft.timeout_min_ms),
                            max: formatTimeoutLabel(breakpointDraft.timeout_max_ms),
                          })
                        : t("settings.perf.safetyRangeLoading")}
                    </Text>
                  </Space>
                </Col>
                <Col>
                  <Space direction="vertical" size={4} align="end">
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {t("settings.perf.current")}
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
                        : t("settings.perf.loading")}
                    </Text>
                  </Space>
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col flex="1" style={{ marginRight: 16 }}>
                  <Space direction="vertical" size={0} style={{ width: "100%" }}>
                    <Text>{t("settings.perf.binaryCapture")}</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {t("settings.perf.binaryCaptureHint")}
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
                    <Text>{t("settings.perf.maxDbSize")}</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {t("settings.perf.maxDbSizeHint")}
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
                    <Text>{t("settings.perf.maxBodyInline")}</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {t("settings.perf.maxBodyInlineHint")}
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
                    <Text>{t("settings.perf.maxBodyBuffer")}</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {t("settings.perf.maxBodyBufferHint")}
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
                  <Space
                    data-testid="settings-performance-max-body-probe-slider"
                    direction="vertical"
                    size={0}
                    style={{ width: "100%" }}
                  >
                    <Text>{t("settings.perf.maxBodyProbe")}</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {t("settings.perf.maxBodyProbeHint")}
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
                              ? t("common.off")
                              : formatBytes(value)
                            : "",
                      }}
                    />
                  </Space>
                </Col>
                <Col>
                  <Text code>
                    {trafficDraft?.max_body_probe_size === 0
                      ? t("common.off")
                      : formatBytes(trafficDraft?.max_body_probe_size || 0)}
                  </Text>
                </Col>
              </Row>

              <Divider style={{ margin: "12px 0" }} />

              <Row justify="space-between" align="middle">
                <Col flex="1" style={{ marginRight: 16 }}>
                  <Space direction="vertical" size={0} style={{ width: "100%" }}>
                    <Text>{t("settings.perf.fileRetention")}</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {t("settings.perf.fileRetentionHint")}
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
                  <Text code>
                    {t("settings.perf.days", { count: trafficDraft?.file_retention_days || 0 })}
                  </Text>
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
                          <Text strong>{t("settings.perf.storageStats")}</Text>
                        </Space>
                      </Col>
                      <Col>
                        <Popconfirm
                          title={t("settings.perf.clearCacheTitle")}
                          description={t("settings.perf.clearCacheDescription")}
                          onConfirm={handleClearBodyCache}
                          okText={t("common.clear")}
                          cancelText={t("common.cancel")}
                          okButtonProps={{ danger: true }}
                        >
                          <Button
                            size="small"
                            danger
                            icon={<DeleteOutlined />}
                            loading={perfLoading}
                            data-testid="settings-performance-clear-cache"
                          >
                            {t("settings.perf.clearCache")}
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
                              {t("settings.perf.bodyCache")}
                            </Text>
                          </Space>
                          <Space>
                            <FileOutlined />
                            <Text>
                              {t("settings.perf.files", {
                                count: performanceConfig.body_store_stats?.file_count ?? 0,
                              })}
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
                            {t("settings.perf.activeStreams", {
                              active: performanceConfig.body_store_stats?.active_stream_writers ?? 0,
                              max: performanceConfig.body_store_stats?.max_open_stream_writers ?? 0,
                            })}
                          </Text>
                        </Space>
                      </Col>
                      <Col xs={24} md={8}>
                        <Space direction="vertical" size={0}>
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            {t("settings.perf.wsFrames")}
                          </Text>
                          <Space>
                            <SwapOutlined />
                            <Text>
                              {t("settings.perf.connections", {
                                count: performanceConfig.frame_store_stats?.connection_count ?? 0,
                              })}
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
                              {t("settings.perf.wsPayloads")}
                            </Text>
                          </Space>
                          <Space>
                            <SwapOutlined />
                            <Text>
                              {t("settings.perf.files", {
                                count: performanceConfig.ws_payload_store_stats?.file_count ?? 0,
                              })}
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
                            {t("settings.perf.openWriters", {
                              active: performanceConfig.ws_payload_store_stats?.active_writers ?? 0,
                              max: performanceConfig.ws_payload_store_stats?.max_open_files ?? 0,
                            })}
                          </Text>
                        </Space>
                      </Col>
                    </Row>
                    <Divider style={{ margin: "8px 0" }} />
                    <Row>
                      <Col>
                        <Space>
                          <Text type="secondary">{t("settings.perf.totalStorage")}</Text>
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
