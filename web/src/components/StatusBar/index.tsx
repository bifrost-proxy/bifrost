import {
  useEffect,
  useMemo,
  memo,
  useCallback,
  useRef,
  type CSSProperties,
  type KeyboardEvent,
} from "react";
import { useNavigate } from "react-router-dom";
import { theme, Tooltip, Popover, Switch, notification } from "antd";
import { ArrowUpOutlined, ArrowDownOutlined } from "@ant-design/icons";
import { useShallow } from "zustand/react/shallow";
import type { TlsConfig } from "../../api/config";
import { useMetricsStore } from "../../stores/useMetricsStore";
import {
  isSystemProxyConfiguredEnabled,
  isSystemProxyLiveEnabledByBifrost,
  useProxyStore,
} from "../../stores/useProxyStore";
import { useTlsConfigStore } from "../../stores/useTlsConfigStore";
import {
  DESKTOP_CHECK_INTERVAL_MS,
  useVersionStore,
} from "../../stores/useVersionStore";
import { useSyncStore } from "../../stores/useSyncStore";
import type { SyncStatus } from "../../api/sync";
import pushService, { type SettingsScope } from "../../services/pushService";
import VersionModal from "../VersionModal";
import AiSkillAssistant from "../AiSkillAssistant";
import { getTlsInterceptionIndicator } from "./statusIndicators";
import { isDesktopShell } from "../../runtime";
import { useI18n, type TFunction } from "../../i18n";
import "./index.css";

function formatSyncAction(
  t: TFunction,
  action?: SyncStatus["last_sync_action"],
): string | null {
  switch (action) {
    case "local_pushed":
      return t("app.statusBar.syncActionLocalPushed");
    case "remote_pulled":
      return t("app.statusBar.syncActionRemotePulled");
    case "bidirectional":
      return t("app.statusBar.syncActionBidirectional");
    case "no_change":
      return t("app.statusBar.syncActionNoChange");
    default:
      return null;
  }
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(1)} ${sizes[i]}`;
}

function formatBytesRate(bytesPerSecond: number): string {
  return `${formatBytes(bytesPerSecond)}/s`;
}

function formatUptime(t: TFunction, seconds: number): string {
  if (seconds < 60) return t("app.statusBar.uptimeSeconds", { s: seconds });
  if (seconds < 3600) return t("app.statusBar.uptimeMinutes", { m: Math.floor(seconds / 60) });
  if (seconds < 86400) {
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    return m > 0
      ? t("app.statusBar.uptimeHoursMinutes", { h, m })
      : t("app.statusBar.uptimeHours", { h });
  }
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  return h > 0
    ? t("app.statusBar.uptimeDaysHours", { d, h })
    : t("app.statusBar.uptimeDays", { d });
}

function withSettingsScope(scope: SettingsScope): SettingsScope[] {
  return Array.from(
    new Set([...(pushService.getSubscription().settings_scopes ?? []), scope]),
  );
}

function tlsConfigFromPushData(data: unknown): TlsConfig | null {
  if (!data || typeof data !== "object") {
    return null;
  }
  const candidate = data as Partial<TlsConfig>;
  return typeof candidate.enable_tls_interception === "boolean"
    ? (candidate as TlsConfig)
    : null;
}

const StatusBar = memo(function StatusBar() {
  const navigate = useNavigate();
  const { token } = theme.useToken();
  const { t } = useI18n();
  const { overview, current, enablePush, disablePush } = useMetricsStore(
    useShallow((state) => ({
      overview: state.overview,
      current: state.current,
      enablePush: state.enablePush,
      disablePush: state.disablePush,
    })),
  );
  const systemProxy = useProxyStore((state) => state.systemProxy);
  const fetchSystemProxy = useProxyStore((state) => state.fetchSystemProxy);
  const toggleSystemProxy = useProxyStore((state) => state.toggleSystemProxy);
  const proxyLoading = useProxyStore((state) => state.loading);
  const tlsConfig = useTlsConfigStore((state) => state.config);
  const fetchTlsConfig = useTlsConfigStore((state) => state.fetchConfig);
  const applyTlsConfigSnapshot = useTlsConfigStore(
    (state) => state.applyConfigSnapshot,
  );
  const syncStatus = useSyncStore((state) => state.syncStatus);
  const startPolling = useSyncStore((state) => state.startPolling);
  const stopPolling = useSyncStore((state) => state.stopPolling);

  const hasUpdate = useVersionStore((state) => state.hasUpdate);
  const latestVersion = useVersionStore((state) => state.latestVersion);
  const setModalVisible = useVersionStore((state) => state.setModalVisible);
  const checkVersion = useVersionStore((state) => state.checkVersion);
  const lastDesktopNotificationVersion = useRef<string | null>(null);

  useEffect(() => {
    fetchSystemProxy();
    fetchTlsConfig();
    pushService.connect({
      ...pushService.getSubscription(),
      settings_scopes: withSettingsScope("tls_config"),
    });
    const unsubscribeTlsSettings = pushService.onSettingsUpdate((update) => {
      if (update.scope !== "tls_config") {
        return;
      }
      const nextConfig = tlsConfigFromPushData(update.data);
      if (nextConfig) {
        applyTlsConfigSnapshot(nextConfig);
      }
    });
    enablePush({ needOverview: true, needMetrics: true });
    startPolling();
    return () => {
      unsubscribeTlsSettings();
      disablePush();
      stopPolling();
      pushService.disconnectIfIdle();
    };
  }, [
    fetchSystemProxy,
    fetchTlsConfig,
    applyTlsConfigSnapshot,
    enablePush,
    disablePush,
    startPolling,
    stopPolling,
  ]);

  useEffect(() => {
    if (!isDesktopShell()) {
      return;
    }

    const runDesktopVersionCheck = async () => {
      await checkVersion({ forceRefresh: true });
      const versionState = useVersionStore.getState();
      if (!versionState.shouldShowAutoModal() || !versionState.latestVersion) {
        return;
      }
      if (lastDesktopNotificationVersion.current !== versionState.latestVersion) {
        notification.info({
          message: t("app.statusBar.desktopUpdateAvailable", {
            version: versionState.latestVersion,
          }),
          description: t("app.statusBar.desktopUpdateDescription"),
          placement: "bottomRight",
          duration: 8,
          onClick: () => setModalVisible(true),
        });
        lastDesktopNotificationVersion.current = versionState.latestVersion;
      }
      setModalVisible(true);
    };

    void runDesktopVersionCheck();
    const timer = window.setInterval(
      () => void runDesktopVersionCheck(),
      DESKTOP_CHECK_INTERVAL_MS,
    );
    return () => window.clearInterval(timer);
  }, [checkVersion, setModalVisible, t]);

  const metrics = current || overview?.metrics;

  const totalTraffic = useMemo(() => {
    if (!metrics) return "0 B";
    return formatBytes(metrics.total_traffic_bytes);
  }, [metrics]);

  const uploadRate = useMemo(() => {
    if (!metrics) return "0 B/s";
    return formatBytesRate(metrics.bytes_sent_rate);
  }, [metrics]);

  const downloadRate = useMemo(() => {
    if (!metrics) return "0 B/s";
    return formatBytesRate(metrics.bytes_received_rate);
  }, [metrics]);

  const memoryUsage = useMemo(() => {
    if (!metrics) return "-";
    return formatBytes(metrics.memory_used);
  }, [metrics]);

  const cpuUsage = useMemo(() => {
    if (!metrics) return "-";
    return `${metrics.cpu_usage.toFixed(1)}%`;
  }, [metrics]);

  const uptime = useMemo(() => {
    if (!overview?.system) return "-";
    return formatUptime(t, overview.system.uptime_secs);
  }, [overview, t]);

  const proxyStatus = useMemo(() => {
    if (!systemProxy) return { text: t("common.unknown"), running: false };
    const configured = isSystemProxyConfiguredEnabled(systemProxy);
    const managedByCurrentBifrost = isSystemProxyLiveEnabledByBifrost(systemProxy);
    return {
      text: managedByCurrentBifrost
        ? t("app.statusBar.proxyRunning")
        : configured
          ? t("app.statusBar.proxyNotApplied")
          : t("app.statusBar.proxyStopped"),
      running: managedByCurrentBifrost,
    };
  }, [systemProxy, t]);

  const tlsIndicator = useMemo(
    () => getTlsInterceptionIndicator(tlsConfig, t),
    [tlsConfig, t],
  );

  const handleToggleSystemProxy = useCallback(
    (checked: boolean) => {
      toggleSystemProxy(checked);
    },
    [toggleSystemProxy],
  );

  const proxyPopoverContent = useMemo(() => {
    if (!systemProxy) return null;
    const configured = isSystemProxyConfiguredEnabled(systemProxy);
    const managedByCurrentBifrost = isSystemProxyLiveEnabledByBifrost(systemProxy);
    const ownedByOther =
      systemProxy.enabled && systemProxy.managed_by_bifrost === false;
    if (!systemProxy.supported) {
      return (
        <div style={{ fontSize: 12, color: token.colorTextSecondary }}>
          {t("app.statusBar.systemProxyUnsupported")}
        </div>
      );
    }
    return (
      <div style={{ minWidth: 180 }}>
        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
          }}
        >
          <span style={{ fontSize: 12 }}>{t("app.statusBar.systemProxy")}</span>
          <Switch
            size="small"
            checked={managedByCurrentBifrost}
            loading={proxyLoading}
            onChange={handleToggleSystemProxy}
          />
        </div>
        {ownedByOther ? (
          <div
            style={{
              fontSize: 11,
              color: token.colorWarningText,
              background: token.colorWarningBg,
              border: `1px solid ${token.colorWarningBorder}`,
              borderRadius: 6,
              padding: "6px 8px",
              marginTop: 6,
            }}
          >
            {t("app.statusBar.managedByOtherProxy", {
              address: `${systemProxy.host}:${systemProxy.port}`,
            })}
          </div>
        ) : managedByCurrentBifrost ? (
          <div
            style={{
              fontSize: 11,
              color: token.colorTextTertiary,
              marginTop: 6,
              fontFamily: "monospace",
            }}
          >
            {systemProxy.host}:{systemProxy.port}
          </div>
        ) : configured ? (
          <div
            style={{
              fontSize: 11,
              color: token.colorWarningText,
              background: token.colorWarningBg,
              border: `1px solid ${token.colorWarningBorder}`,
              borderRadius: 6,
              padding: "6px 8px",
              marginTop: 6,
            }}
          >
            {t("app.statusBar.configuredNotApplied")}
          </div>
        ) : null}
      </div>
    );
  }, [systemProxy, proxyLoading, handleToggleSystemProxy, token, t]);

  const syncIndicator = useMemo(() => {
    if (!syncStatus || !syncStatus.enabled) {
      return {
        text: t("common.off"),
        detail: t("app.statusBar.syncDisabled"),
        color: token.colorTextQuaternary,
        pulse: false,
        state: "disabled",
      };
    }

    const readyProvider = syncStatus.providers?.find(
      (provider) => provider.connected && provider.reachable && provider.authorized,
    );
    const effectiveLastSyncAt =
      readyProvider?.last_sync_at ?? syncStatus.last_sync_at ?? null;
    const effectiveLastSyncAction =
      readyProvider?.last_sync_action ?? syncStatus.last_sync_action ?? null;

    if (syncStatus.syncing) {
      return {
        text: t("app.statusBar.syncing"),
        detail: t("app.statusBar.syncingDetail"),
        color: token.colorWarning,
        pulse: true,
        state: "syncing",
      };
    }

    if (!readyProvider && !syncStatus.reachable) {
      return {
        text: t("app.statusBar.syncLocal"),
        detail: t("app.statusBar.syncLocalDetail"),
        color: token.colorWarning,
        pulse: false,
        state: "unreachable",
      };
    }

    if (!readyProvider && !syncStatus.authorized) {
      return {
        text: t("app.statusBar.syncSignIn"),
        detail: t("app.statusBar.syncSignInDetail"),
        color: token.colorInfo,
        pulse: false,
        state: "unauthorized",
      };
    }

    return {
      text: effectiveLastSyncAt ? t("app.statusBar.synced") : t("app.statusBar.connected"),
      detail: effectiveLastSyncAt
        ? t("app.statusBar.syncActionAt", {
            action:
              formatSyncAction(t, effectiveLastSyncAction) ??
              t("app.statusBar.syncActionCompleted"),
            time: new Date(effectiveLastSyncAt).toLocaleString(),
          })
        : readyProvider
          ? t("app.statusBar.connectedTo", { name: readyProvider.name })
          : t("app.statusBar.connectedToRemote"),
      color: token.colorSuccess,
      pulse: false,
      state: effectiveLastSyncAt ? "ready" : "connected",
    };
  }, [syncStatus, token.colorInfo, token.colorSuccess, token.colorTextQuaternary, token.colorWarning, t]);

  const handleVersionClick = useCallback(() => {
    checkVersion({ forceRefresh: true });
    setModalVisible(true);
  }, [checkVersion, setModalVisible]);

  const handleSyncClick = useCallback(() => {
    navigate("/settings?tab=sync");
  }, [navigate]);

  const handleSyncKeyDown = useCallback(
    (event: KeyboardEvent<HTMLDivElement>) => {
      if (event.key !== "Enter" && event.key !== " ") {
        return;
      }
      event.preventDefault();
      handleSyncClick();
    },
    [handleSyncClick],
  );

  const styles: Record<string, CSSProperties> = {
    container: {
      height: 20,
      backgroundColor: token.colorBgContainer,
      borderTop: `1px solid ${token.colorBorderSecondary}`,
      display: "flex",
      alignItems: "center",
      padding: "0 12px",
      fontSize: 10,
      color: token.colorTextTertiary,
      gap: 16,
      flexShrink: 0,
      overflow: "hidden",
    },
    item: {
      display: "flex",
      alignItems: "center",
      gap: 4,
      whiteSpace: "nowrap",
    },
    label: {
      opacity: 0.7,
    },
    value: {
      fontFamily: "monospace",
    },
    valueRate: {
      fontFamily: "monospace",
      minWidth: 70,
      textAlign: "right" as const,
    },
    valueTraffic: {
      fontFamily: "monospace",
      minWidth: 58,
      textAlign: "right" as const,
    },
    valueNumber: {
      fontFamily: "monospace",
      minWidth: 40,
      textAlign: "right" as const,
    },
    valueMem: {
      fontFamily: "monospace",
      minWidth: 52,
      textAlign: "right" as const,
    },
    valueCpu: {
      fontFamily: "monospace",
      minWidth: 38,
      textAlign: "right" as const,
    },
    valueUptime: {
      fontFamily: "monospace",
      minWidth: 48,
      textAlign: "right" as const,
    },
    valueStatus: {
      fontFamily: "monospace",
      minWidth: 52,
    },
    syncButton: {
      cursor: "pointer",
      borderRadius: 3,
      padding: "1px 4px",
      margin: "0 -4px",
      transition: "background-color 0.2s",
    },
    statusDot: {
      width: 6,
      height: 6,
      borderRadius: "50%",
      backgroundColor: proxyStatus.running
        ? token.colorSuccess
        : token.colorTextQuaternary,
    },
    tlsDot: {
      width: 7,
      height: 7,
      borderRadius: "50%",
      color: tlsIndicator.active ? token.colorError : token.colorTextQuaternary,
      backgroundColor: tlsIndicator.active
        ? token.colorError
        : token.colorTextQuaternary,
      boxShadow: tlsIndicator.active ? `0 0 8px ${token.colorError}` : "none",
      flexShrink: 0,
    },
    tlsButton: {
      cursor: "pointer",
      borderRadius: 3,
      padding: "1px 4px",
      margin: "0 -4px",
      transition: "background-color 0.2s",
    },
    tlsValue: {
      fontFamily: "monospace",
      minWidth: 48,
      color: tlsIndicator.active ? token.colorError : token.colorTextTertiary,
      fontWeight: tlsIndicator.active ? 700 : 400,
      display: "inline-block",
    },
    syncDot: {
      width: 6,
      height: 6,
      borderRadius: "50%",
      backgroundColor: syncIndicator.color,
      boxShadow: syncIndicator.pulse ? `0 0 0 3px ${token.colorWarningBg}` : "none",
    },
    rateUp: {
      color: token.colorTextTertiary,
    },
    rateDown: {
      color: token.colorTextTertiary,
    },
    separator: {
      width: 1,
      height: 10,
      backgroundColor: token.colorBorderSecondary,
    },
    versionButton: {
      display: "flex",
      alignItems: "center",
      gap: 4,
      cursor: "pointer",
      padding: "2px 6px",
      borderRadius: 3,
      transition: "background-color 0.2s",
    },
    versionButtonHover: {
      backgroundColor: token.colorFillSecondary,
    },
    updateDot: {
      width: 6,
      height: 6,
      borderRadius: "50%",
      backgroundColor: token.colorError,
    },
    updateArrow: {
      fontSize: 10,
      color: token.colorSuccess,
    },
    rightCluster: {
      display: "flex",
      alignItems: "center",
      gap: 6,
    },
  };

  const versionTooltip = hasUpdate
    ? t("app.statusBar.newVersionAvailable", { version: latestVersion ?? "" })
    : t("app.statusBar.clickToViewVersion");

  return (
    <>
      <div style={styles.container} data-testid="status-bar">
        <Popover
          content={proxyPopoverContent}
          trigger="hover"
          placement="top"
          arrow={false}
        >
          <div style={{ ...styles.item, cursor: "pointer" }}>
            <div style={styles.statusDot} />
            <span style={styles.label}>{t("app.statusBar.proxyLabel")}</span>
            <span style={styles.valueStatus}>{proxyStatus.text}</span>
          </div>
        </Popover>

        <Tooltip title={tlsIndicator.detail}>
          <div
            style={{ ...styles.item, ...styles.tlsButton }}
            role="button"
            tabIndex={0}
            aria-label={t("app.statusBar.openTlsSettings")}
            data-testid="statusbar-tls-interception"
            data-tls-state={tlsIndicator.state}
            onClick={() => navigate("/settings?tab=tls")}
            onKeyDown={(event) => {
              if (event.key !== "Enter" && event.key !== " ") {
                return;
              }
              event.preventDefault();
              navigate("/settings?tab=tls");
            }}
            onMouseEnter={(e) => {
              e.currentTarget.style.backgroundColor = token.colorFillSecondary;
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.backgroundColor = "transparent";
            }}
          >
            <div
              className={`statusbar-tls-dot ${
                tlsIndicator.active ? "statusbar-tls-dot--active" : ""
              }`}
              style={styles.tlsDot}
            />
            <span style={styles.label}>{t("app.statusBar.tlsLabel")}</span>
            <span
              className={
                tlsIndicator.active ? "statusbar-tls-value--active" : undefined
              }
              style={styles.tlsValue}
            >
              {tlsIndicator.text}
            </span>
          </div>
        </Tooltip>

        <Tooltip title={syncIndicator.detail}>
          <div
            style={{ ...styles.item, ...styles.syncButton }}
            role="button"
            tabIndex={0}
            aria-label={t("app.statusBar.openSyncSettings")}
            data-testid="statusbar-sync"
            data-sync-state={syncIndicator.state}
            data-sync-action={syncStatus?.last_sync_action ?? "unknown"}
            onClick={handleSyncClick}
            onKeyDown={handleSyncKeyDown}
            onMouseEnter={(e) => {
              e.currentTarget.style.backgroundColor = token.colorFillSecondary;
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.backgroundColor = "transparent";
            }}
          >
            <div style={styles.syncDot} />
            <span style={styles.label}>{t("app.statusBar.syncLabel")}</span>
            <span style={styles.valueStatus}>{syncIndicator.text}</span>
          </div>
        </Tooltip>

        <div style={styles.separator} />

        <div style={styles.item}>
          <ArrowUpOutlined style={styles.rateUp} />
          <span style={styles.valueRate}>{uploadRate}</span>
        </div>

        <div style={styles.item}>
          <ArrowDownOutlined style={styles.rateDown} />
          <span style={styles.valueRate}>{downloadRate}</span>
        </div>

        <div style={styles.separator} />

        <div style={styles.item}>
          <span style={styles.label}>{t("app.statusBar.totalLabel")}</span>
          <span style={styles.valueTraffic}>{totalTraffic}</span>
        </div>

        <div style={styles.separator} />

        <div style={styles.item}>
          <span style={styles.label}>{t("app.statusBar.connLabel")}</span>
          <span style={styles.valueNumber}>
            {metrics?.active_connections ?? 0}
          </span>
        </div>

        <div style={styles.item}>
          <span style={styles.label}>{t("app.statusBar.reqLabel")}</span>
          <span style={styles.valueNumber}>{metrics?.total_requests ?? 0}</span>
        </div>

        <div style={styles.separator} />

        <div style={styles.item}>
          <span style={styles.label}>{t("app.statusBar.memLabel")}</span>
          <span style={styles.valueMem}>{memoryUsage}</span>
        </div>

        <div style={styles.item}>
          <span style={styles.label}>{t("app.statusBar.cpuLabel")}</span>
          <span style={styles.valueCpu}>{cpuUsage}</span>
        </div>

        <div style={styles.separator} />

        <div style={styles.item}>
          <span style={styles.label}>{t("app.statusBar.uptimeLabel")}</span>
          <span style={styles.valueUptime}>{uptime}</span>
        </div>

        {overview?.system?.version && (
          <>
            <div style={{ flex: 1 }} />
            <div style={styles.rightCluster}>
              <Tooltip title={versionTooltip}>
                <div
                  style={styles.versionButton}
                  onClick={handleVersionClick}
                  data-testid="statusbar-version-button"
                  onMouseEnter={(e) => {
                    e.currentTarget.style.backgroundColor = token.colorFillSecondary;
                  }}
                  onMouseLeave={(e) => {
                    e.currentTarget.style.backgroundColor = "transparent";
                  }}
                >
                  {hasUpdate && <div style={styles.updateDot} />}
                  <span style={styles.value}>v{overview.system.version}</span>
                  {hasUpdate && (
                    <ArrowUpOutlined style={styles.updateArrow} />
                  )}
                </div>
              </Tooltip>
              <div style={styles.separator} />
              <AiSkillAssistant />
            </div>
          </>
        )}
      </div>
      <VersionModal />
    </>
  );
});

export default StatusBar;
