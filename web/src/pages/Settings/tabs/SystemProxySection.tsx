import { useMemo } from "react";
import { Alert, Card, Col, Divider, Row, Space, Switch, Tag, Tooltip, Typography } from "antd";
import { ExclamationCircleOutlined, GlobalOutlined } from "@ant-design/icons";
import type {
  CliProxyStatus,
  SystemProxyLaunchdStatus,
  SystemProxyStatus,
} from "../../../api/proxy";

const { Text } = Typography;

interface SystemProxySectionProps {
  systemProxy: SystemProxyStatus | null;
  systemProxyLaunchd: SystemProxyLaunchdStatus | null;
  cliProxy: CliProxyStatus | null;
  systemProxyLoading: boolean;
  systemProxyLaunchdLoading: boolean;
  injectBifrostBadge: boolean | null;
  injectBifrostBadgeLoading: boolean;
  onToggleSystemProxy: (enabled: boolean) => void;
  onToggleSystemProxyLaunchd: (enabled: boolean) => void;
  onToggleInjectBifrostBadge: (enabled: boolean) => void;
}

export default function SystemProxySection({
  systemProxy,
  systemProxyLaunchd,
  cliProxy,
  systemProxyLoading,
  systemProxyLaunchdLoading,
  injectBifrostBadge,
  injectBifrostBadgeLoading,
  onToggleSystemProxy,
  onToggleSystemProxyLaunchd,
  onToggleInjectBifrostBadge,
}: SystemProxySectionProps) {
  const cliProxyDisplay = useMemo(() => {
    if (!cliProxy) {
      return { tag: null, detail: "Loading CLI proxy status..." };
    }
    const tag = {
      color: cliProxy.enabled ? "green" : "default",
      text: cliProxy.enabled ? "Enabled" : "Disabled",
    };
    const shortFiles = (cliProxy.config_files || [])
      .map((file) => file.replace(`${window.location.origin}/`, ""))
      .slice(0, 2);
    const filesText =
      shortFiles.length > 0
        ? shortFiles.join(", ") + ((cliProxy.config_files || []).length > 2 ? " ..." : "")
        : "-";
    return {
      tag,
      detail: `Shell: ${cliProxy.shell || "-"} · Files: ${filesText}`,
    };
  }, [cliProxy]);

  const launchdEnabled =
    !!systemProxyLaunchd?.installed &&
    !!systemProxyLaunchd?.loaded &&
    !systemProxyLaunchd?.needs_upgrade;
  const showLaunchdControls = systemProxyLaunchd?.supported === true;
  const systemProxyEnabledByBifrost =
    !!systemProxy?.enabled && systemProxy.managed_by_bifrost !== false;
  const systemProxyOwnedByOther =
    !!systemProxy?.enabled && systemProxy.managed_by_bifrost === false;

  return (
    <Col xs={24}>
      <Card
        title={
          <Space>
            <GlobalOutlined />
            <span>System Proxy</span>
          </Space>
        }
        size="small"
      >
        <Space direction="vertical" style={{ width: "100%" }}>
          <Row justify="space-between" align="middle">
            <Col>
              <Text>Enable System Proxy</Text>
            </Col>
            <Col>
              {systemProxy ? (
                systemProxy.supported ? (
                  <Switch
                    checked={systemProxyEnabledByBifrost}
                    loading={systemProxyLoading}
                    onChange={onToggleSystemProxy}
                    data-testid="settings-system-proxy-switch"
                  />
                ) : (
                  <Tooltip title="System proxy is not supported on this platform">
                    <Text type="secondary">Not Supported</Text>
                  </Tooltip>
                )
              ) : (
                <Text type="secondary">Loading...</Text>
              )}
            </Col>
          </Row>
          {systemProxyOwnedByOther ? (
            <Alert
              type="warning"
              showIcon
              message="System proxy is occupied by another proxy"
              description={`Current system proxy points to ${systemProxy.host}:${systemProxy.port}. Turn this on to let Bifrost take over and restore it when turned off.`}
            />
          ) : (
            <Text type="secondary" style={{ fontSize: 12 }}>
              Route all system traffic through this proxy
            </Text>
          )}

          {showLaunchdControls && (
            <Space
              direction="vertical"
              size={4}
              style={{ width: "100%", paddingLeft: 16 }}
            >
              <Row justify="space-between" align="middle">
                <Col>
                  <Text>Boot/Shutdown Cleanup</Text>
                </Col>
                <Col>
                  <Switch
                    checked={launchdEnabled}
                    loading={systemProxyLaunchdLoading}
                    onChange={onToggleSystemProxyLaunchd}
                    data-testid="settings-system-proxy-launchd-switch"
                  />
                </Col>
              </Row>
              {systemProxyLaunchd.needs_upgrade ? (
                <Alert
                  type="warning"
                  showIcon
                  message="Cleanup fallback needs upgrade"
                  description={
                    systemProxyLaunchd.needs_upgrade_reason ||
                    systemProxyLaunchd.message ||
                    `Installed helper mode or paths are outdated. Version: ${
                      systemProxyLaunchd.installed_version || "-"
                    }`
                  }
                />
              ) : systemProxyLaunchd.message ? (
                <Text type="secondary" style={{ fontSize: 12 }}>
                  {systemProxyLaunchd.message}
                </Text>
              ) : (
                <Text type="secondary" style={{ fontSize: 12 }}>
                  Restores Bifrost-managed system proxy settings during crash
                  recovery and after restart without keeping an idle daemon
                  process alive. Version:{" "}
                  {systemProxyLaunchd.installed_version ||
                    systemProxyLaunchd.current_version ||
                    "-"}
                </Text>
              )}
            </Space>
          )}

          <Divider style={{ margin: "12px 0" }} />

          <Row justify="space-between" align="middle">
            <Col>
              <Text>Inject Bifrost Badge</Text>
            </Col>
            <Col>
              {injectBifrostBadge === null ? (
                <Text type="secondary">Loading...</Text>
              ) : (
                <Switch
                  checked={injectBifrostBadge}
                  loading={injectBifrostBadgeLoading}
                  onChange={onToggleInjectBifrostBadge}
                  data-testid="settings-badge-injection-switch"
                />
              )}
            </Col>
          </Row>
          <Text type="secondary" style={{ fontSize: 12 }}>
            Only applies to HTML pages. Indicates that traffic is flowing through Bifrost proxy.
          </Text>

          <Divider style={{ margin: "12px 0" }} />

          <Row justify="space-between" align="middle">
            <Col>
              <Space>
                <Text>CLI Proxy (ENV)</Text>
                <Tooltip title="Persist proxy environment variables in your shell config so new terminals inherit the proxy">
                  <ExclamationCircleOutlined style={{ color: "#8c8c8c" }} />
                </Tooltip>
              </Space>
            </Col>
            <Col>
              {cliProxyDisplay.tag ? (
                <Tag color={cliProxyDisplay.tag.color} data-testid="settings-cli-proxy-tag">
                  {cliProxyDisplay.tag.text}
                </Tag>
              ) : (
                <Text type="secondary">Loading...</Text>
              )}
            </Col>
          </Row>
          <Tooltip
            title={
              cliProxy ? (
                <pre style={{ margin: 0, whiteSpace: "pre-wrap" }}>
                  {`Proxy URL: ${cliProxy.proxy_url}\nConfig files:\n${
                    (cliProxy.config_files || []).join("\n") || "-"
                  }`}
                </pre>
              ) : undefined
            }
          >
            <Text
              type="secondary"
              style={{ fontSize: 12 }}
              data-testid="settings-cli-proxy-detail"
            >
              {cliProxyDisplay.detail}
            </Text>
          </Tooltip>
        </Space>
      </Card>
    </Col>
  );
}
