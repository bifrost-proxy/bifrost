import { Card, Col, Row, Space, Switch, Tooltip, Typography } from "antd";
import { AppstoreOutlined, DashboardOutlined } from "@ant-design/icons";
import type { CSSProperties } from "react";
import type { TrayConfig } from "../../../api/config";

const { Text } = Typography;

const settingRowStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: 12,
  width: "100%",
};

const settingTextStyle: CSSProperties = {
  minWidth: 0,
};

const settingControlStyle: CSSProperties = {
  flexShrink: 0,
};

interface TrayTabProps {
  trayConfig: TrayConfig | null;
  trayLoading: boolean;
  onToggleTray: (enabled: boolean) => void;
  onToggleSystemStats: (enabled: boolean) => void;
  onToggleSystemStatsItem: (
    item: keyof TrayConfig["system_stats_items"],
    enabled: boolean,
  ) => void;
}

export default function TrayTab({
  trayConfig,
  trayLoading,
  onToggleTray,
  onToggleSystemStats,
  onToggleSystemStatsItem,
}: TrayTabProps) {
  const systemStatsDisabled =
    !trayConfig || !trayConfig.supported || !trayConfig.show_system_stats;
  const statsItems: Array<{
    key: keyof TrayConfig["system_stats_items"];
    label: string;
    description: string;
  }> = [
    {
      key: "cpu",
      label: "CPU",
      description: "Processor utilization",
    },
    {
      key: "memory",
      label: "Memory",
      description: "RAM usage percentage",
    },
    {
      key: "disk",
      label: "Disk",
      description: "Current data disk usage",
    },
    {
      key: "upload",
      label: "Upload",
      description: "Default-route outbound throughput",
    },
    {
      key: "download",
      label: "Download",
      description: "Default-route inbound throughput",
    },
  ];

  return (
    <Row gutter={[16, 16]} data-testid="settings-tray-tab">
      <Col xs={24}>
        <Card
          title={
            <Space>
              <AppstoreOutlined />
              <span>Tray</span>
            </Space>
          }
          size="small"
        >
          <Space direction="vertical" style={{ width: "100%" }} size="middle">
            <div style={settingRowStyle}>
              <div style={settingTextStyle}>
                <Text>Tray Icon</Text>
                <br />
                <Text type="secondary" style={{ fontSize: 12 }}>
                  Native quick controller
                </Text>
              </div>
              <div style={settingControlStyle}>
                {trayConfig ? (
                  trayConfig.supported ? (
                    <Switch
                      checked={trayConfig.enabled}
                      loading={trayLoading}
                      onChange={onToggleTray}
                      data-testid="settings-tray-switch"
                    />
                  ) : (
                    <Tooltip title="Tray icon is not supported on this platform">
                      <Text type="secondary">Not Supported</Text>
                    </Tooltip>
                  )
                ) : (
                  <Text type="secondary">Loading...</Text>
                )}
              </div>
            </div>
          </Space>
        </Card>
      </Col>

      <Col xs={24}>
        <Card
          title={
            <Space>
              <DashboardOutlined />
              <span>System Status</span>
            </Space>
          }
          size="small"
        >
          <Space direction="vertical" style={{ width: "100%" }} size="middle">
            <div style={settingRowStyle}>
              <div style={settingTextStyle}>
                <Text>Show System Stats</Text>
                <br />
                <Text type="secondary" style={{ fontSize: 12 }}>
                  CPU, memory, disk, upload, and download speed
                </Text>
              </div>
              <div style={settingControlStyle}>
                {trayConfig ? (
                  trayConfig.supported ? (
                    <Switch
                      checked={trayConfig.show_system_stats}
                      loading={trayLoading}
                      onChange={onToggleSystemStats}
                      data-testid="settings-tray-system-stats-switch"
                    />
                  ) : (
                    <Text type="secondary">Not Supported</Text>
                  )
                ) : (
                  <Text type="secondary">Loading...</Text>
                )}
              </div>
            </div>

            {statsItems.map((item) => (
              <div key={item.key} style={settingRowStyle}>
                <div style={{ ...settingTextStyle, paddingLeft: 12 }}>
                  <Text>{item.label}</Text>
                  <br />
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {item.description}
                  </Text>
                </div>
                <div style={settingControlStyle}>
                  {trayConfig ? (
                    <Switch
                      checked={trayConfig.system_stats_items[item.key]}
                      disabled={systemStatsDisabled}
                      loading={trayLoading}
                      onChange={(enabled) =>
                        onToggleSystemStatsItem(item.key, enabled)
                      }
                      data-testid={`settings-tray-system-stats-${item.key}-switch`}
                    />
                  ) : (
                    <Text type="secondary">Loading...</Text>
                  )}
                </div>
              </div>
            ))}
          </Space>
        </Card>
      </Col>
    </Row>
  );
}
