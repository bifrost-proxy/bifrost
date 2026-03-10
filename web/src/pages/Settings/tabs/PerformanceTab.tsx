import {
  Card,
  Col,
  Row,
  Space,
  Typography,
  Slider,
  Divider,
  Button,
  Popconfirm,
  theme,
  Switch,
} from "antd";
import {
  ThunderboltOutlined,
  FolderOutlined,
  DeleteOutlined,
  FileOutlined,
  DatabaseOutlined,
  SwapOutlined,
} from "@ant-design/icons";
import type { PerformanceConfig, TrafficConfig } from "../../../api/config";

const { Text } = Typography;

export interface PerformanceTabProps {
  perfLoading: boolean;
  performanceConfig: PerformanceConfig | null;
  trafficDraft?: TrafficConfig | null;
  maxRecordsMin: number;
  maxRecordsMax: number;
  maxRecordsStep: number;
  maxRecordsMarks: Record<number, string>;
  maxDbSizeMarks: Record<number, string>;
  maxBodyInlineMarks: Record<number, string>;
  maxBodyBufferMarks: Record<number, string>;
  maxBodyProbeMarks: Record<number, string>;
  fileRetentionMarks: Record<number, string>;
  handleMaxRecordsChange: (value: number | null) => void;
  handleMaxDbSizeChange: (value: number) => void;
  handleMaxBodyMemorySizeChange: (value: number) => void;
  handleMaxBodyBufferSizeChange: (value: number) => void;
  handleMaxBodyProbeSizeChange: (value: number) => void;
  handleEnableBodyIndexChange: (enabled: boolean) => void;
  handleFileRetentionDaysChange: (value: number) => void;
  handleClearBodyCache: () => void;
  formatBytes: (bytes: number) => string;
}

export default function PerformanceTab({
  perfLoading,
  performanceConfig,
  trafficDraft,
  maxRecordsMin,
  maxRecordsMax,
  maxRecordsStep,
  maxRecordsMarks,
  maxDbSizeMarks,
  maxBodyInlineMarks,
  maxBodyBufferMarks,
  maxBodyProbeMarks,
  fileRetentionMarks,
  handleMaxRecordsChange,
  handleMaxDbSizeChange,
  handleMaxBodyMemorySizeChange,
  handleMaxBodyBufferSizeChange,
  handleMaxBodyProbeSizeChange,
  handleEnableBodyIndexChange,
  handleFileRetentionDaysChange,
  handleClearBodyCache,
  formatBytes,
}: PerformanceTabProps) {
  const { token } = theme.useToken();

  return (
    <Row gutter={[16, 16]}>
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
              <Row justify="space-between" align="middle">
                <Col flex="1" style={{ marginRight: 16 }}>
                  <Space direction="vertical" size={0} style={{ width: "100%" }}>
                    <Text>Max Records</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      Keep only the newest records in memory; older ones are
                      evicted and the database prunes the oldest entries.
                    </Text>
                    <Slider
                      min={maxRecordsMin}
                      max={maxRecordsMax}
                      step={maxRecordsStep}
                      value={trafficDraft?.max_records}
                      onChange={handleMaxRecordsChange}
                      marks={maxRecordsMarks}
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
                    <Text>Max DB Size</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      Caps traffic.db + body_cache + frames + ws_payload on
                      disk; when exceeded, the oldest data is deleted and the
                      database is vacuumed.
                    </Text>
                    <Slider
                      min={256 * 1024 * 1024}
                      max={10 * 1024 * 1024 * 1024}
                      step={256 * 1024 * 1024}
                      value={trafficDraft?.max_db_size_bytes}
                      onChange={handleMaxDbSizeChange}
                      marks={maxDbSizeMarks}
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
                    <Text>Body Index (Search)</Text>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      Builds a lightweight index for request/response bodies to speed up search.
                      May increase CPU usage; recommended to keep off unless needed.
                    </Text>
                  </Space>
                </Col>
                <Col>
                  <Space>
                    <Switch
                      checked={trafficDraft?.enable_body_index ?? false}
                      onChange={handleEnableBodyIndexChange}
                      disabled={!trafficDraft}
                    />
                    <Text code>
                      {(trafficDraft?.enable_body_index ?? false) ? "On" : "Off"}
                    </Text>
                  </Space>
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
                performanceConfig?.traffic_store_stats ||
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
                          >
                            Clear Cache
                          </Button>
                        </Popconfirm>
                      </Col>
                    </Row>
                    <Row gutter={[16, 8]} style={{ marginTop: 12 }}>
                      <Col xs={6}>
                        <Space direction="vertical" size={0}>
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            Body Cache
                          </Text>
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
                        </Space>
                      </Col>
                      <Col xs={6}>
                        <Space direction="vertical" size={0}>
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            Traffic Records
                          </Text>
                          <Space>
                            <DatabaseOutlined />
                            <Text>
                              {performanceConfig.traffic_store_stats
                                ?.record_count ?? 0}{" "}
                              records
                            </Text>
                          </Space>
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            {formatBytes(
                              performanceConfig.traffic_store_stats?.file_size ??
                                0,
                            )}
                          </Text>
                        </Space>
                      </Col>
                      <Col xs={6}>
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
                      <Col xs={6}>
                        <Space direction="vertical" size={0}>
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            WebSocket Payloads
                          </Text>
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
                                (performanceConfig.traffic_store_stats
                                  ?.file_size ?? 0) +
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
