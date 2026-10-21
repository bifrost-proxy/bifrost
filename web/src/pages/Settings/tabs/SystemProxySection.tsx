import { useMemo } from "react";
import {
  Alert,
  Card,
  Col,
  Divider,
  Row,
  Space,
  Switch,
  Tag,
  Tooltip,
  Typography,
} from "antd";
import { ExclamationCircleOutlined, GlobalOutlined } from "@ant-design/icons";
import type {
  CliProxyStatus,
  SystemProxyLaunchdStatus,
  SystemProxyStatus,
} from "../../../api/proxy";
import {
  isSystemProxyConfiguredEnabled,
  isSystemProxyLiveEnabledByBifrost,
} from "../../../stores/useProxyStore";
import { useI18n } from "../../../i18n";

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
  const { t } = useI18n();
  const cliProxyDisplay = useMemo(() => {
    if (!cliProxy) {
      return { tag: null, detail: t("settings.systemProxy.cliLoading") };
    }
    const tag = {
      color: cliProxy.enabled ? "green" : "default",
      text: cliProxy.enabled ? t("common.enabled") : t("common.disabled"),
    };
    const shortFiles = (cliProxy.config_files || [])
      .map((file) => file.replace(`${window.location.origin}/`, ""))
      .slice(0, 2);
    const filesText =
      shortFiles.length > 0
        ? shortFiles.join(", ") +
          ((cliProxy.config_files || []).length > 2 ? " ..." : "")
        : "-";
    return {
      tag,
      detail: t("settings.systemProxy.cliDetail", {
        shell: cliProxy.shell || "-",
        files: filesText,
      }),
    };
  }, [cliProxy, t]);

  const launchdEnabled =
    !!systemProxyLaunchd?.installed &&
    !!systemProxyLaunchd?.loaded &&
    !systemProxyLaunchd?.needs_upgrade;
  const showLaunchdControls = systemProxyLaunchd?.supported === true;
  const systemProxyConfiguredEnabled = systemProxy
    ? isSystemProxyConfiguredEnabled(systemProxy)
    : false;
  const systemProxyEnabledByBifrost = systemProxy
    ? isSystemProxyLiveEnabledByBifrost(systemProxy)
    : false;
  const systemProxyOwnedByOther =
    !!systemProxy?.enabled && systemProxy.managed_by_bifrost === false;
  const systemProxyConfiguredButInactive =
    systemProxyConfiguredEnabled &&
    !systemProxyEnabledByBifrost &&
    !systemProxyOwnedByOther;
  const systemProxyDisabledButActive =
    !systemProxyConfiguredEnabled && systemProxyEnabledByBifrost;

  return (
    <Col xs={24}>
      <Card
        title={
          <Space>
            <GlobalOutlined />
            <span>{t("settings.systemProxy.title")}</span>
          </Space>
        }
        size="small"
      >
        <Space direction="vertical" style={{ width: "100%" }}>
          <Row justify="space-between" align="middle">
            <Col>
              <Text>{t("settings.systemProxy.enable")}</Text>
            </Col>
            <Col>
              {systemProxy ? (
                systemProxy.supported ? (
                  <Switch
                    checked={systemProxyConfiguredEnabled}
                    loading={systemProxyLoading}
                    onChange={onToggleSystemProxy}
                    data-testid="settings-system-proxy-switch"
                  />
                ) : (
                  <Tooltip
                    title={t("settings.systemProxy.notSupportedTooltip")}
                  >
                    <Text type="secondary">
                      {t("settings.systemProxy.notSupported")}
                    </Text>
                  </Tooltip>
                )
              ) : (
                <Text type="secondary">{t("common.loading")}</Text>
              )}
            </Col>
          </Row>
          {systemProxyOwnedByOther ? (
            <Alert
              type="warning"
              showIcon
              message={t("settings.systemProxy.occupied")}
              description={`${t("settings.systemProxy.occupiedPointsTo", {
                address: `${systemProxy.host}:${systemProxy.port}`,
              })} ${
                systemProxyConfiguredEnabled
                  ? t("settings.systemProxy.occupiedConfigured")
                  : t("settings.systemProxy.occupiedTakeOver")
              }`}
            />
          ) : systemProxyConfiguredButInactive ? (
            <Alert
              type="warning"
              showIcon
              message={t("settings.systemProxy.configuredInactive")}
              description={t("settings.systemProxy.configuredInactiveHint")}
            />
          ) : systemProxyDisabledButActive ? (
            <Alert
              type="warning"
              showIcon
              message={t("settings.systemProxy.stillActive")}
              description={t("settings.systemProxy.stillActiveHint")}
            />
          ) : (
            <Text type="secondary" style={{ fontSize: 12 }}>
              {t("settings.systemProxy.hint")}
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
                  <Text>{t("settings.systemProxy.cleanup")}</Text>
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
                  message={t("settings.systemProxy.cleanupNeedsUpgrade")}
                  description={
                    systemProxyLaunchd.needs_upgrade_reason ||
                    systemProxyLaunchd.message ||
                    t("settings.systemProxy.cleanupOutdated", {
                      version: systemProxyLaunchd.installed_version || "-",
                    })
                  }
                />
              ) : systemProxyLaunchd.message ? (
                <Text type="secondary" style={{ fontSize: 12 }}>
                  {systemProxyLaunchd.message}
                </Text>
              ) : (
                <Text type="secondary" style={{ fontSize: 12 }}>
                  {t("settings.systemProxy.cleanupHint", {
                    version:
                      systemProxyLaunchd.installed_version ||
                      systemProxyLaunchd.current_version ||
                      "-",
                  })}
                </Text>
              )}
            </Space>
          )}

          <Divider style={{ margin: "12px 0" }} />

          <Row justify="space-between" align="middle">
            <Col>
              <Text>{t("settings.systemProxy.injectBadge")}</Text>
            </Col>
            <Col>
              {injectBifrostBadge === null ? (
                <Text type="secondary">{t("common.loading")}</Text>
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
            {t("settings.systemProxy.injectBadgeHint")}
          </Text>

          <Divider style={{ margin: "12px 0" }} />

          <Row justify="space-between" align="middle">
            <Col>
              <Space>
                <Text>{t("settings.systemProxy.cliProxy")}</Text>
                <Tooltip title={t("settings.systemProxy.cliProxyTooltip")}>
                  <ExclamationCircleOutlined style={{ color: "#8c8c8c" }} />
                </Tooltip>
              </Space>
            </Col>
            <Col>
              {cliProxyDisplay.tag ? (
                <Tag
                  color={cliProxyDisplay.tag.color}
                  data-testid="settings-cli-proxy-tag"
                >
                  {cliProxyDisplay.tag.text}
                </Tag>
              ) : (
                <Text type="secondary">{t("common.loading")}</Text>
              )}
            </Col>
          </Row>
          <Tooltip
            title={
              cliProxy ? (
                <pre style={{ margin: 0, whiteSpace: "pre-wrap" }}>
                  {`${t("settings.systemProxy.cliProxyUrl", { url: cliProxy.proxy_url })}\n${t(
                    "settings.systemProxy.cliConfigFiles",
                  )}\n${(cliProxy.config_files || []).join("\n") || "-"}`}
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
