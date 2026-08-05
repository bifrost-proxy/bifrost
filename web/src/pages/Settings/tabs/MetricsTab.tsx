import { Card, Col, Row, Space, Statistic, Progress, Typography, Button, Table, Tabs, theme } from "antd";
import type { ColumnsType } from "antd/es/table";
import {
  ApiOutlined,
  CloudDownloadOutlined,
  CloudUploadOutlined,
  DashboardOutlined,
  DatabaseOutlined,
  GlobalOutlined,
  LaptopOutlined,
  ReloadOutlined,
  SwapOutlined,
} from "@ant-design/icons";
import MetricsChart from "../../../components/MetricsChart";
import type {
  AppMetrics,
  HostMetrics,
  TrafficTypeMetrics,
  SystemOverview,
  MetricsSnapshot,
  MetricsAggregateSummary,
} from "../../../types";

const { Text } = Typography;

export interface MetricsTabProps {
  overview: SystemOverview | null;
  history: MetricsSnapshot[];
  memoryPercent: number;
  appMetrics: AppMetrics[];
  appMetricsSummary: MetricsAggregateSummary;
  appMetricsLoading: boolean;
  hostMetrics: HostMetrics[];
  hostMetricsSummary: MetricsAggregateSummary;
  hostMetricsLoading: boolean;
  formatBytes: (bytes: number) => string;
  formatBytesRate: (bytesPerSec: number) => string;
  onRefreshAppMetrics: () => void;
  onRefreshHostMetrics: () => void;
}

interface MetricsContentProps {
  activeConnections: number;
  totalRequests: number;
  qps: number;
  maxQps: number;
  recordedTraffic: number;
  bytesSentRate: number;
  bytesReceivedRate: number;
  maxBytesSentRate: number;
  maxBytesReceivedRate: number;
  bytesSent: number;
  bytesReceived: number;
  formatBytes: (bytes: number) => string;
  formatBytesRate: (bytesPerSec: number) => string;
}

function MetricsContent({
  activeConnections,
  totalRequests,
  qps,
  maxQps,
  recordedTraffic,
  bytesSentRate,
  bytesReceivedRate,
  maxBytesSentRate,
  maxBytesReceivedRate,
  bytesSent,
  bytesReceived,
  formatBytes,
  formatBytesRate,
}: MetricsContentProps) {
  const { token } = theme.useToken();

  return (
    <>
      <Row gutter={[16, 16]}>
        <Col xs={24} sm={12} lg={6}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Active Connections"
              value={activeConnections}
              prefix={<SwapOutlined />}
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Total Requests"
              value={totalRequests}
              prefix={<ApiOutlined />}
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Current QPS"
              value={qps.toFixed(2)}
              prefix={<DashboardOutlined />}
              suffix={
                <Text type="secondary" style={{ fontSize: 12 }}>
                  max: {maxQps.toFixed(2)}
                </Text>
              }
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Recorded Traffic"
              value={recordedTraffic}
              prefix={<DatabaseOutlined />}
            />
          </Card>
        </Col>
      </Row>

      <Row gutter={[16, 16]} style={{ marginTop: 16 }}>
        <Col xs={24} sm={12} lg={6}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Upload Rate"
              value={formatBytesRate(bytesSentRate)}
              prefix={<CloudUploadOutlined style={{ color: token.colorSuccess }} />}
            />
            <Text type="secondary" style={{ fontSize: 12 }}>
              Max: {formatBytesRate(maxBytesSentRate)}
            </Text>
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Download Rate"
              value={formatBytesRate(bytesReceivedRate)}
              prefix={<CloudDownloadOutlined style={{ color: token.colorInfo }} />}
            />
            <Text type="secondary" style={{ fontSize: 12 }}>
              Max: {formatBytesRate(maxBytesReceivedRate)}
            </Text>
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Total Upload"
              value={formatBytes(bytesSent)}
              prefix={<CloudUploadOutlined style={{ color: token.colorSuccess }} />}
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Total Download"
              value={formatBytes(bytesReceived)}
              prefix={<CloudDownloadOutlined style={{ color: token.colorInfo }} />}
            />
          </Card>
        </Col>
      </Row>
    </>
  );
}

interface TrafficTypeContentProps {
  metrics?: TrafficTypeMetrics;
  formatBytes: (bytes: number) => string;
}

function TrafficTypeContent({ metrics, formatBytes }: TrafficTypeContentProps) {
  const { token } = theme.useToken();
  const data = metrics || {
    requests: 0,
    bytes_sent: 0,
    bytes_received: 0,
    active_connections: 0,
  };

  return (
    <Row gutter={[16, 16]}>
      <Col xs={24} sm={12} lg={6}>
        <Card
          size="small"
          bordered={false}
          style={{ background: token.colorBgLayout }}
        >
          <Statistic
            title="Active Connections"
            value={data.active_connections}
            prefix={<SwapOutlined />}
          />
        </Card>
      </Col>
      <Col xs={24} sm={12} lg={6}>
        <Card
          size="small"
          bordered={false}
          style={{ background: token.colorBgLayout }}
        >
          <Statistic
            title="Total Requests"
            value={data.requests}
            prefix={<ApiOutlined />}
          />
        </Card>
      </Col>
      <Col xs={24} sm={12} lg={6}>
        <Card
          size="small"
          bordered={false}
          style={{ background: token.colorBgLayout }}
        >
          <Statistic
            title="Total Upload"
            value={formatBytes(data.bytes_sent)}
            prefix={<CloudUploadOutlined style={{ color: token.colorSuccess }} />}
          />
        </Card>
      </Col>
      <Col xs={24} sm={12} lg={6}>
        <Card
          size="small"
          bordered={false}
          style={{ background: token.colorBgLayout }}
        >
          <Statistic
            title="Total Download"
            value={formatBytes(data.bytes_received)}
            prefix={<CloudDownloadOutlined style={{ color: token.colorInfo }} />}
          />
        </Card>
      </Col>
    </Row>
  );
}

interface AppMetricsContentProps {
  appMetrics: AppMetrics[];
  summary: MetricsAggregateSummary;
  loading: boolean;
  formatBytes: (bytes: number) => string;
  onRefresh: () => void;
}

function AppMetricsContent({
  appMetrics,
  summary,
  loading,
  formatBytes,
  onRefresh,
}: AppMetricsContentProps) {
  const { token } = theme.useToken();

  const columns: ColumnsType<AppMetrics> = [
    {
      title: "Application",
      dataIndex: "app_name",
      key: "app_name",
      fixed: "left" as const,
      width: 200,
      render: (name: string) => (
        <Space>
          <LaptopOutlined style={{ color: token.colorPrimary }} />
          <Text strong>{name}</Text>
        </Space>
      ),
    },
    {
      title: "Requests",
      dataIndex: "requests",
      key: "requests",
      width: 100,
      sorter: (a, b) => a.requests - b.requests,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "Active",
      dataIndex: "active_connections",
      key: "active_connections",
      width: 100,
      sorter: (a, b) => a.active_connections - b.active_connections,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "Upload",
      dataIndex: "bytes_sent",
      key: "bytes_sent",
      width: 100,
      sorter: (a, b) => a.bytes_sent - b.bytes_sent,
      render: (val: number) => formatBytes(val),
    },
    {
      title: "Download",
      dataIndex: "bytes_received",
      key: "bytes_received",
      width: 100,
      sorter: (a, b) => a.bytes_received - b.bytes_received,
      render: (val: number) => formatBytes(val),
    },
    {
      title: "HTTP",
      dataIndex: "http_requests",
      key: "http_requests",
      width: 80,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "HTTPS",
      dataIndex: "https_requests",
      key: "https_requests",
      width: 80,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "Tunnel",
      dataIndex: "tunnel_requests",
      key: "tunnel_requests",
      width: 80,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "WS",
      dataIndex: "ws_requests",
      key: "ws_requests",
      width: 80,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "WSS",
      dataIndex: "wss_requests",
      key: "wss_requests",
      width: 80,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "H3",
      dataIndex: "h3_requests",
      key: "h3_requests",
      width: 80,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "SOCKS5",
      dataIndex: "socks5_requests",
      key: "socks5_requests",
      width: 90,
      render: (val: number) => val.toLocaleString(),
    },
  ];

  return (
    <div>
      <Row gutter={[16, 16]} style={{ marginBottom: 16 }}>
        <Col xs={24} sm={8}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Total Applications"
              value={summary.total}
              prefix={<LaptopOutlined />}
            />
          </Card>
        </Col>
        <Col xs={24} sm={8}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Total Requests"
              value={summary.requests.toLocaleString()}
              prefix={<ApiOutlined />}
            />
          </Card>
        </Col>
        <Col xs={24} sm={8}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Total Traffic"
              value={formatBytes(summary.total_traffic_bytes)}
              prefix={<SwapOutlined />}
            />
          </Card>
        </Col>
      </Row>

      <div
        style={{
          marginBottom: 16,
          display: "flex",
          justifyContent: "flex-end",
        }}
      >
        <Button icon={<ReloadOutlined />} onClick={onRefresh} loading={loading} size="small">
          Refresh
        </Button>
      </div>

      <Table
        columns={columns}
        dataSource={appMetrics}
        rowKey="app_name"
        loading={loading}
        size="small"
        scroll={{ x: 900 }}
        pagination={{
          pageSize: 10,
          showSizeChanger: false,
          showTotal: (total) => `Total ${total} applications`,
        }}
      />
    </div>
  );
}

interface HostMetricsContentProps {
  hostMetrics: HostMetrics[];
  summary: MetricsAggregateSummary;
  loading: boolean;
  formatBytes: (bytes: number) => string;
  onRefresh: () => void;
}

function HostMetricsContent({
  hostMetrics,
  summary,
  loading,
  formatBytes,
  onRefresh,
}: HostMetricsContentProps) {
  const { token } = theme.useToken();

  const columns: ColumnsType<HostMetrics> = [
    {
      title: "Host",
      dataIndex: "host",
      key: "host",
      fixed: "left" as const,
      width: 250,
      render: (host: string) => (
        <Space>
          <GlobalOutlined style={{ color: token.colorPrimary }} />
          <Text strong>{host}</Text>
        </Space>
      ),
    },
    {
      title: "Requests",
      dataIndex: "requests",
      key: "requests",
      width: 100,
      sorter: (a, b) => a.requests - b.requests,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "Active",
      dataIndex: "active_connections",
      key: "active_connections",
      width: 100,
      sorter: (a, b) => a.active_connections - b.active_connections,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "Upload",
      dataIndex: "bytes_sent",
      key: "bytes_sent",
      width: 100,
      sorter: (a, b) => a.bytes_sent - b.bytes_sent,
      render: (val: number) => formatBytes(val),
    },
    {
      title: "Download",
      dataIndex: "bytes_received",
      key: "bytes_received",
      width: 100,
      sorter: (a, b) => a.bytes_received - b.bytes_received,
      render: (val: number) => formatBytes(val),
    },
    {
      title: "HTTP",
      dataIndex: "http_requests",
      key: "http_requests",
      width: 80,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "HTTPS",
      dataIndex: "https_requests",
      key: "https_requests",
      width: 80,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "Tunnel",
      dataIndex: "tunnel_requests",
      key: "tunnel_requests",
      width: 80,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "WS",
      dataIndex: "ws_requests",
      key: "ws_requests",
      width: 80,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "WSS",
      dataIndex: "wss_requests",
      key: "wss_requests",
      width: 80,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "H3",
      dataIndex: "h3_requests",
      key: "h3_requests",
      width: 80,
      render: (val: number) => val.toLocaleString(),
    },
    {
      title: "SOCKS5",
      dataIndex: "socks5_requests",
      key: "socks5_requests",
      width: 90,
      render: (val: number) => val.toLocaleString(),
    },
  ];

  return (
    <div>
      <Row gutter={[16, 16]} style={{ marginBottom: 16 }}>
        <Col xs={24} sm={8}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Total Hosts"
              value={summary.total}
              prefix={<GlobalOutlined />}
            />
          </Card>
        </Col>
        <Col xs={24} sm={8}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Total Requests"
              value={summary.requests.toLocaleString()}
              prefix={<ApiOutlined />}
            />
          </Card>
        </Col>
        <Col xs={24} sm={8}>
          <Card
            size="small"
            bordered={false}
            style={{ background: token.colorBgLayout }}
          >
            <Statistic
              title="Total Traffic"
              value={formatBytes(summary.total_traffic_bytes)}
              prefix={<SwapOutlined />}
            />
          </Card>
        </Col>
      </Row>

      <div
        style={{
          marginBottom: 16,
          display: "flex",
          justifyContent: "flex-end",
        }}
      >
        <Button icon={<ReloadOutlined />} onClick={onRefresh} loading={loading} size="small">
          Refresh
        </Button>
      </div>

      <Table
        columns={columns}
        dataSource={hostMetrics}
        rowKey="host"
        loading={loading}
        size="small"
        scroll={{ x: 900 }}
        pagination={{
          pageSize: 10,
          showSizeChanger: false,
          showTotal: (total) => `Total ${total} hosts`,
        }}
      />
    </div>
  );
}

export default function MetricsTab({
  overview,
  history,
  memoryPercent,
  appMetrics,
  appMetricsSummary,
  appMetricsLoading,
  hostMetrics,
  hostMetricsSummary,
  hostMetricsLoading,
  formatBytes,
  formatBytesRate,
  onRefreshAppMetrics,
  onRefreshHostMetrics,
}: MetricsTabProps) {
  const { token } = theme.useToken();

  return (
    <div>
      <Card title="Performance Metrics" size="small">
        <Tabs
          defaultActiveKey="overview"
          size="small"
          items={[
            {
              key: "overview",
              label: "Overview",
              children: (
                <MetricsContent
                  activeConnections={overview?.metrics.active_connections || 0}
                  totalRequests={overview?.metrics.total_requests || 0}
                  qps={overview?.metrics.qps || 0}
                  maxQps={overview?.metrics.max_qps || 0}
                  recordedTraffic={overview?.traffic.recorded || 0}
                  bytesSentRate={overview?.metrics.bytes_sent_rate || 0}
                  bytesReceivedRate={overview?.metrics.bytes_received_rate || 0}
                  maxBytesSentRate={overview?.metrics.max_bytes_sent_rate || 0}
                  maxBytesReceivedRate={overview?.metrics.max_bytes_received_rate || 0}
                  bytesSent={overview?.metrics.bytes_sent || 0}
                  bytesReceived={overview?.metrics.bytes_received || 0}
                  formatBytes={formatBytes}
                  formatBytesRate={formatBytesRate}
                />
              ),
            },
            {
              key: "http",
              label: "HTTP",
              children: (
                <TrafficTypeContent
                  metrics={overview?.metrics.http}
                  formatBytes={formatBytes}
                />
              ),
            },
            {
              key: "https",
              label: "HTTPS",
              children: (
                <TrafficTypeContent
                  metrics={overview?.metrics.https}
                  formatBytes={formatBytes}
                />
              ),
            },
            {
              key: "tunnel",
              label: "Tunnel",
              children: (
                <TrafficTypeContent
                  metrics={overview?.metrics.tunnel}
                  formatBytes={formatBytes}
                />
              ),
            },
            {
              key: "ws",
              label: "WS",
              children: (
                <TrafficTypeContent
                  metrics={overview?.metrics.ws}
                  formatBytes={formatBytes}
                />
              ),
            },
            {
              key: "wss",
              label: "WSS",
              children: (
                <TrafficTypeContent
                  metrics={overview?.metrics.wss}
                  formatBytes={formatBytes}
                />
              ),
            },
            {
              key: "h3",
              label: "H3",
              children: (
                <TrafficTypeContent
                  metrics={overview?.metrics.h3}
                  formatBytes={formatBytes}
                />
              ),
            },
            {
              key: "socks5",
              label: "SOCKS5",
              children: (
                <TrafficTypeContent
                  metrics={overview?.metrics.socks5}
                  formatBytes={formatBytes}
                />
              ),
            },
            {
              key: "apps",
              label: "Applications",
              children: (
                <AppMetricsContent
                  appMetrics={appMetrics}
                  summary={appMetricsSummary}
                  loading={appMetricsLoading}
                  formatBytes={formatBytes}
                  onRefresh={onRefreshAppMetrics}
                />
              ),
            },
            {
              key: "hosts",
              label: "Hosts",
              children: (
                <HostMetricsContent
                  hostMetrics={hostMetrics}
                  summary={hostMetricsSummary}
                  loading={hostMetricsLoading}
                  formatBytes={formatBytes}
                  onRefresh={onRefreshHostMetrics}
                />
              ),
            },
          ]}
        />

        <Row gutter={[16, 16]} style={{ marginTop: 16 }}>
          <Col xs={24} sm={12}>
            <Card
              size="small"
              bordered={false}
              style={{ background: token.colorBgLayout }}
            >
              <Text strong>CPU Usage</Text>
              <Progress
                percent={Number((overview?.metrics.cpu_usage || 0).toFixed(1))}
                status={
                  (overview?.metrics.cpu_usage || 0) > 80
                    ? "exception"
                    : "normal"
                }
                strokeColor={{
                  "0%": "#108ee9",
                  "100%":
                    (overview?.metrics.cpu_usage || 0) > 80
                      ? "#ff4d4f"
                      : "#87d068",
                }}
              />
              {history.length > 0 && (
                <div style={{ marginTop: 12 }}>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    Last Hour
                  </Text>
                  <MetricsChart data={history} type="cpu" height={120} />
                </div>
              )}
            </Card>
          </Col>
          <Col xs={24} sm={12}>
            <Card
              size="small"
              bordered={false}
              style={{ background: token.colorBgLayout }}
            >
              <Text strong>Memory Usage</Text>
              <Progress
                percent={Number(memoryPercent.toFixed(1))}
                status={memoryPercent > 80 ? "exception" : "normal"}
                strokeColor={{
                  "0%": "#108ee9",
                  "100%": memoryPercent > 80 ? "#ff4d4f" : "#87d068",
                }}
                format={() =>
                  `${formatBytes(overview?.metrics.memory_used || 0)} / ${formatBytes(overview?.metrics.memory_total || 0)}`
                }
              />
              {history.length > 0 && (
                <div style={{ marginTop: 12 }}>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    Last Hour
                  </Text>
                  <MetricsChart data={history} type="memory" height={120} />
                </div>
              )}
            </Card>
          </Col>
        </Row>
      </Card>

      <Row gutter={[16, 16]} style={{ marginTop: 16 }}>
        <Col xs={24} sm={12} lg={6}>
          <Card size="small">
            <Statistic title="Total Rules" value={overview?.rules.total || 0} />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card size="small">
            <Statistic title="Enabled Rules" value={overview?.rules.enabled || 0} />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card size="small">
            <Statistic
              title="Recorded Traffic"
              value={overview?.traffic.recorded || 0}
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card size="small">
            <Statistic
              title="Total Requests"
              value={overview?.metrics.total_requests || 0}
            />
          </Card>
        </Col>
      </Row>
    </div>
  );
}
