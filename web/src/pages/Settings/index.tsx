import { useEffect, useState, useCallback, useMemo, useRef, type CSSProperties } from "react";
import { useSearchParams } from "react-router-dom";
import { useShallow } from "zustand/react/shallow";
import {
  Alert,
  Spin,
  Tabs,
  message,
} from "antd";
import {
  DashboardOutlined,
  CloudOutlined,
  GlobalOutlined,
  KeyOutlined,
  SafetyCertificateOutlined,
  ThunderboltOutlined,
  SafetyOutlined,
  ApiOutlined,
  AppstoreOutlined,
  TranslationOutlined,
} from "@ant-design/icons";
import { useMetricsStore } from "../../stores/useMetricsStore";
import { useProxyStore } from "../../stores/useProxyStore";
import { getAppMetrics, getHostMetrics } from "../../api/metrics";
import {
  getProxyAddressInfo,
  type ProxyAddressInfo,
} from "../../api/proxy";
import {
  updateTlsConfig,
  updateTrayConfig,
  getProxySettings,
  getTlsConfig,
  getPerformanceConfig,
  updatePerformanceConfig,
  clearBodyCache,
  type TlsConfig,
  type ProxySettings,
  type BreakpointPerformanceConfig,
  type PerformanceConfig,
  type TraySystemStatsItems,
  type TrafficConfig,
  type UpdateTrafficConfigRequest,
} from "../../api/config";
import {
  getSyncStatus,
  logoutSyncProvider,
  logoutSyncSession,
  openSyncLogin,
  updateSyncConfig,
  type SyncProviderStatus,
  type SyncStatus,
} from "../../api/sync";
import { isConnectionIssueError, normalizeApiErrorMessage } from "../../api/client";
import { copyToClipboard } from "../../utils/clipboard";
import { useSyncStore } from "../../stores/useSyncStore";
import { usePerformanceModeStore } from "../../stores/usePerformanceModeStore";
import {
  getCertInfo,
  getCertDownloadUrl,
  getCertQRCodeUrl,
  type CertInfo,
} from "../../api/cert";
import type { AppMetrics, HostMetrics, MetricsAggregateSummary } from "../../types";
import { useWhitelistStore } from "../../stores/useWhitelistStore";
import ProxyTab from "./tabs/ProxyTab";
import CertificateTab from "./tabs/CertificateTab";
import MetricsTab from "./tabs/MetricsTab";
import AccessControlTab from "./tabs/AccessControlTab";
import PerformanceTab from "./tabs/PerformanceTab";
import SyncTab from "./tabs/SyncTab";
import TrayTab from "./tabs/TrayTab";
import RemoteAccessTab from "./tabs/RemoteAccessTab";
import RemoteInvokeTab from "./tabs/RemoteInvokeTab";
import LanguageTab from "./tabs/LanguageTab";
import { useI18n } from "../../i18n";
import { updateDesktopProxyPort } from "../../desktop/tauri";
import {
  getDesktopPlatform,
  getExpectedDesktopProxyPort,
  isDesktopShell,
  setDesktopProxyPort,
  waitForDesktopBackendReady,
} from "../../runtime";
import { useDesktopCoreStore } from "../../stores/useDesktopCoreStore";
import pushService from "../../services/pushService";
import { showTlsWhitelistChangeSuccess } from "../../utils/tlsInterceptionNotice";

const TAB_PARAM = "tab";
const HIGHLIGHT_PARAM = "highlight";
const DEFAULT_TAB = "proxy";
const MAX_BODY_PROBE_NEAR_LEFT_LABEL_GAP_PX = 16;
const MAX_BODY_PROBE_SECOND_NEAR_LEFT_LABEL_GAP_PX = 28;
const VALID_TABS = [
  "proxy",
  "certificate",
  "metrics",
  "access",
  "performance",
  "tray",
  "sync",
  "remote",
  "remote-invoke",
  "language",
];

const maxBodyProbeLeftMarkStyle: CSSProperties = {
  transform: "translateX(0)",
  whiteSpace: "nowrap",
};

const maxBodyProbeNearLeftMarkStyle: CSSProperties = {
  ...maxBodyProbeLeftMarkStyle,
  marginLeft: MAX_BODY_PROBE_NEAR_LEFT_LABEL_GAP_PX,
};

const maxBodyProbeSecondNearLeftMarkStyle: CSSProperties = {
  ...maxBodyProbeLeftMarkStyle,
  marginLeft: MAX_BODY_PROBE_SECOND_NEAR_LEFT_LABEL_GAP_PX,
};

function getErrorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message) {
    return error.message;
  }

  if (typeof error === "string" && error.trim()) {
    return error;
  }

  if (
    error &&
    typeof error === "object" &&
    "message" in error &&
    typeof error.message === "string" &&
    error.message.trim()
  ) {
    return error.message;
  }

  return fallback;
}

export default function Settings() {
  const { t } = useI18n();
  const { overview, history, loading, error, fetchOverview, fetchHistory } =
    useMetricsStore(
      useShallow((state) => ({
        overview: state.overview,
        history: state.history,
        loading: state.loading,
        error: state.error,
        fetchOverview: state.fetchOverview,
        fetchHistory: state.fetchHistory,
      })),
    );
  const [searchParams, setSearchParams] = useSearchParams();

  const tabFromUrl = searchParams.get(TAB_PARAM);
  const highlightFromUrl = searchParams.get(HIGHLIGHT_PARAM);
  const activeTab =
    tabFromUrl && VALID_TABS.includes(tabFromUrl) ? tabFromUrl : DEFAULT_TAB;

  const handleTabChange = useCallback(
    (key: string) => {
      setSearchParams(
        (prev) => {
          prev.set(TAB_PARAM, key);
          return prev;
        },
        { replace: true },
      );
    },
    [setSearchParams],
  );

  const {
    systemProxy,
    systemProxyLaunchd,
    cliProxy,
    loading: systemProxyLoading,
    launchdLoading: systemProxyLaunchdLoading,
    toggleSystemProxy,
    toggleSystemProxyLaunchd,
    fetchSystemProxy,
    fetchSystemProxyLaunchd,
    fetchCliProxy,
  } = useProxyStore();
  const [tlsConfig, setTlsConfig] = useState<TlsConfig | null>(null);
  const [tlsLoading, setTlsLoading] = useState(false);
  const [certInfo, setCertInfo] = useState<CertInfo | null>(null);
  const [newExcludePattern, setNewExcludePattern] = useState("");
  const [newIncludePattern, setNewIncludePattern] = useState("");
  const [newAppExcludePattern, setNewAppExcludePattern] = useState("");
  const [newAppIncludePattern, setNewAppIncludePattern] = useState("");
  const [performanceConfig, setPerformanceConfig] =
    useState<PerformanceConfig | null>(null);
  const [perfLoading, setPerfLoading] = useState(false);
  const [perfDraft, setPerfDraft] = useState<TrafficConfig | null>(null);
  const [breakpointPerfDraft, setBreakpointPerfDraft] =
    useState<BreakpointPerformanceConfig | null>(null);
  const perfUpdateTimers = useRef<Record<string, number>>({});
  const [appMetrics, setAppMetrics] = useState<AppMetrics[]>([]);
  const [appMetricsSummary, setAppMetricsSummary] = useState<MetricsAggregateSummary>({
    total: 0,
    requests: 0,
    bytes_sent: 0,
    bytes_received: 0,
    total_traffic_bytes: 0,
  });
  const [appMetricsLoading, setAppMetricsLoading] = useState(false);
  const [hostMetrics, setHostMetrics] = useState<HostMetrics[]>([]);
  const [hostMetricsSummary, setHostMetricsSummary] = useState<MetricsAggregateSummary>({
    total: 0,
    requests: 0,
    bytes_sent: 0,
    bytes_received: 0,
    total_traffic_bytes: 0,
  });
  const [hostMetricsLoading, setHostMetricsLoading] = useState(false);
  const [proxyAddressInfo, setProxyAddressInfo] =
    useState<ProxyAddressInfo | null>(null);
  const [proxySettings, setProxySettings] = useState<ProxySettings | null>(null);
  const [trayLoading, setTrayLoading] = useState(false);
  const [desktopExpectedProxyPort, setDesktopExpectedProxyPort] = useState<number | null>(
    isDesktopShell() ? getExpectedDesktopProxyPort() : null,
  );
  const [desktopActualProxyPort, setDesktopActualProxyPort] = useState<number | null>(
    null,
  );
  const [desktopPortDraft, setDesktopPortDraft] = useState(9900);
  const [desktopPortSaving, setDesktopPortSaving] = useState(false);
  const [syncStatus, setSyncStatus] = useState<SyncStatus | null>(null);
  const [syncLoading, setSyncLoading] = useState(false);
  const beginDesktopCoreRestart = useDesktopCoreStore(
    (state) => state.beginRestart,
  );
  const setDesktopCorePhase = useDesktopCoreStore((state) => state.setPhase);
  const failDesktopCoreRestart = useDesktopCoreStore(
    (state) => state.failRestart,
  );
  const hideDesktopCoreRestart = useDesktopCoreStore((state) => state.hide);
  const desktopCoreVisible = useDesktopCoreStore((state) => state.visible);
  const desktopCorePhase = useDesktopCoreStore((state) => state.phase);
  const fetchWhitelistStatus = useWhitelistStore((state) => state.fetchStatus);
  const suppressRestartErrors =
    isDesktopShell() &&
    desktopCoreVisible &&
    desktopCorePhase !== "idle" &&
    desktopCorePhase !== "error";

  const applySyncStatus = useCallback((status: SyncStatus) => {
    setSyncStatus(status);
    useSyncStore.getState().setSyncStatus(status);
  }, []);

  const fetchProxySettings = useCallback(async () => {
    try {
      const settings = await getProxySettings();
      setProxySettings(settings);
    } catch (error) {
      if (!suppressRestartErrors && !isConnectionIssueError(error)) {
        console.error("Failed to fetch proxy settings");
      }
    }
  }, [suppressRestartErrors]);

  const fetchTlsConfigData = useCallback(async () => {
    setTlsLoading(true);
    try {
      const config = await getTlsConfig();
      setTlsConfig(config);
    } catch (error) {
      if (!suppressRestartErrors && !isConnectionIssueError(error)) {
        console.error("Failed to fetch TLS config");
      }
    } finally {
      setTlsLoading(false);
    }
  }, [suppressRestartErrors]);

  const fetchDesktopRuntime = useCallback(async () => {
    if (!isDesktopShell()) {
      return;
    }
    try {
      const { getDesktopRuntime } = await import("../../desktop/tauri");
      const runtime = await getDesktopRuntime();
      setDesktopExpectedProxyPort(runtime.expectedProxyPort);
      setDesktopActualProxyPort(runtime.proxyPort);
      setDesktopPortDraft(runtime.expectedProxyPort);
      setDesktopProxyPort(runtime.proxyPort);
    } catch (error) {
      if (!suppressRestartErrors && !isConnectionIssueError(error)) {
        console.error("Failed to fetch desktop runtime");
      }
    }
  }, [suppressRestartErrors]);

  const fetchPerformanceConfig = useCallback(async () => {
    setPerfLoading(true);
    try {
      const config = await getPerformanceConfig();
      setPerformanceConfig(config);
      setPerfDraft(config.traffic);
      setBreakpointPerfDraft(config.breakpoint);
    } catch (error) {
      if (!suppressRestartErrors && !isConnectionIssueError(error)) {
        console.error("Failed to fetch performance config");
      }
    } finally {
      setPerfLoading(false);
    }
  }, [suppressRestartErrors]);

  const fetchSyncStatusData = useCallback(async () => {
    setSyncLoading(true);
    try {
      const status = await getSyncStatus();
      applySyncStatus(status);
    } catch (error) {
      if (!suppressRestartErrors && !isConnectionIssueError(error)) {
        console.error("Failed to fetch sync status");
      }
    } finally {
      setSyncLoading(false);
    }
  }, [applySyncStatus, suppressRestartErrors]);

  const fetchCertInfoData = useCallback(async () => {
    try {
      const info = await getCertInfo();
      setCertInfo(info);
    } catch (error) {
      if (!suppressRestartErrors && !isConnectionIssueError(error)) {
        console.error("Failed to fetch certificate info");
      }
    }
  }, [suppressRestartErrors]);

  const fetchAppMetricsData = useCallback(async () => {
    setAppMetricsLoading(true);
    try {
      const response = await getAppMetrics();
      setAppMetrics(response.items);
      setAppMetricsSummary(response.summary);
    } catch (error) {
      if (!suppressRestartErrors && !isConnectionIssueError(error)) {
        console.error("Failed to fetch app metrics");
      }
    } finally {
      setAppMetricsLoading(false);
    }
  }, [suppressRestartErrors]);

  const fetchHostMetricsData = useCallback(async () => {
    setHostMetricsLoading(true);
    try {
      const response = await getHostMetrics();
      setHostMetrics(response.items);
      setHostMetricsSummary(response.summary);
    } catch (error) {
      if (!suppressRestartErrors && !isConnectionIssueError(error)) {
        console.error("Failed to fetch host metrics");
      }
    } finally {
      setHostMetricsLoading(false);
    }
  }, [suppressRestartErrors]);

  const fetchProxyAddressInfo = useCallback(async () => {
    try {
      const info = await getProxyAddressInfo();
      setProxyAddressInfo(info);
    } catch (error) {
      if (!suppressRestartErrors && !isConnectionIssueError(error)) {
        console.error("Failed to fetch proxy address info");
      }
    }
  }, [suppressRestartErrors]);

  const handleSystemProxyToggle = async (enabled: boolean) => {
    const success = await toggleSystemProxy(enabled);
    if (success) {
      message.success(
        enabled ? t("settings.msg.systemProxyEnabled") : t("settings.msg.systemProxyDisabled"),
      );
    } else {
      const proxyError = useProxyStore.getState().error;
      message.error(proxyError || t("settings.msg.systemProxyToggleFailed"));
    }
  };

  const handleSystemProxyLaunchdToggle = async (enabled: boolean) => {
    const success = await toggleSystemProxyLaunchd(enabled);
    if (success) {
      message.success(
        enabled
          ? t("settings.msg.cleanupFallbackInstalled")
          : t("settings.msg.cleanupFallbackRemoved"),
      );
    } else {
      const proxyError = useProxyStore.getState().error;
      message.error(proxyError || t("settings.msg.cleanupFallbackFailed"));
    }
  };

  const handleTrayToggle = async (enabled: boolean) => {
    setTrayLoading(true);
    try {
      const tray = await updateTrayConfig({ enabled });
      setProxySettings((prev) => (prev ? { ...prev, tray } : prev));
      message.success(enabled ? t("settings.msg.trayIconEnabled") : t("settings.msg.trayIconDisabled"));
    } catch {
      message.error(t("settings.msg.trayUpdateFailed"));
      void fetchProxySettings();
    } finally {
      setTrayLoading(false);
    }
  };

  const handleTraySystemStatsToggle = async (enabled: boolean) => {
    setTrayLoading(true);
    try {
      const tray = await updateTrayConfig({ show_system_stats: enabled });
      setProxySettings((prev) => (prev ? { ...prev, tray } : prev));
      message.success(
        enabled ? t("settings.msg.traySystemStatsEnabled") : t("settings.msg.traySystemStatsDisabled"),
      );
    } catch {
      message.error(t("settings.msg.traySystemStatsFailed"));
      void fetchProxySettings();
    } finally {
      setTrayLoading(false);
    }
  };

  const handleTraySystemStatsItemToggle = async (
    item: keyof TraySystemStatsItems,
    enabled: boolean,
  ) => {
    setTrayLoading(true);
    try {
      const tray = await updateTrayConfig({
        system_stats_items: { [item]: enabled },
      });
      setProxySettings((prev) => (prev ? { ...prev, tray } : prev));
      message.success(t("settings.msg.traySystemStatsItemUpdated"));
    } catch {
      message.error(t("settings.msg.traySystemStatsItemFailed"));
      void fetchProxySettings();
    } finally {
      setTrayLoading(false);
    }
  };

  const handleTlsInterceptionToggle = async (enabled: boolean) => {
    setTlsLoading(true);
    try {
      const result = await updateTlsConfig({
        enable_tls_interception: enabled,
      });
      setTlsConfig(result);
      message.success(
        enabled ? t("settings.msg.httpsInterceptionEnabled") : t("settings.msg.httpsInterceptionDisabled"),
      );
    } catch {
      message.error(t("settings.msg.tlsConfigFailed"));
    } finally {
      setTlsLoading(false);
    }
  };

  const handleUnsafeSslToggle = async (enabled: boolean) => {
    setTlsLoading(true);
    try {
      const result = await updateTlsConfig({ unsafe_ssl: enabled });
      setTlsConfig(result);
      message.success(
        enabled
          ? t("settings.msg.certVerificationDisabled")
          : t("settings.msg.certVerificationEnabled"),
      );
    } catch {
      message.error(t("settings.msg.tlsConfigFailed"));
    } finally {
      setTlsLoading(false);
    }
  };

  const handleDisconnectOnConfigChangeToggle = async (enabled: boolean) => {
    setTlsLoading(true);
    try {
      const result = await updateTlsConfig({
        disconnect_on_config_change: enabled,
      });
      setTlsConfig(result);
      message.success(
        enabled
          ? t("settings.msg.autoDisconnectEnabled")
          : t("settings.msg.autoDisconnectDisabled"),
      );
    } catch {
      message.error(t("settings.msg.tlsConfigFailed"));
    } finally {
      setTlsLoading(false);
    }
  };

  const handleAddExcludePattern = async () => {
    if (!newExcludePattern.trim()) {
      message.warning(t("settings.msg.enterPattern"));
      return;
    }

    const pattern = newExcludePattern.trim();
    if (tlsConfig?.intercept_exclude.includes(pattern)) {
      message.warning(t("settings.msg.patternExists"));
      return;
    }

    setTlsLoading(true);
    try {
      const newList = [...(tlsConfig?.intercept_exclude || []), pattern];
      const result = await updateTlsConfig({ intercept_exclude: newList });
      setTlsConfig(result);
      setNewExcludePattern("");
      message.success(t("settings.msg.addedToExclude", { pattern }));
    } catch {
      message.error(t("settings.msg.addPatternFailed"));
    } finally {
      setTlsLoading(false);
    }
  };

  const handleRemoveExcludePattern = async (pattern: string) => {
    setTlsLoading(true);
    try {
      const newList = (tlsConfig?.intercept_exclude || []).filter(
        (p) => p !== pattern,
      );
      const result = await updateTlsConfig({ intercept_exclude: newList });
      setTlsConfig(result);
      message.success(t("settings.msg.removedFromExclude", { pattern }));
    } catch {
      message.error(t("settings.msg.removePatternFailed"));
    } finally {
      setTlsLoading(false);
    }
  };

  const handleAddIncludePattern = async () => {
    if (!newIncludePattern.trim()) {
      message.warning(t("settings.msg.enterPattern"));
      return;
    }

    const pattern = newIncludePattern.trim();
    if (tlsConfig?.intercept_include.includes(pattern)) {
      message.warning(t("settings.msg.patternExists"));
      return;
    }

    setTlsLoading(true);
    try {
      const newList = [...(tlsConfig?.intercept_include || []), pattern];
      const result = await updateTlsConfig({ intercept_include: newList });
      setTlsConfig(result);
      setNewIncludePattern("");
      showTlsWhitelistChangeSuccess(t("settings.msg.addedToInclude", { pattern }));
    } catch {
      message.error(t("settings.msg.addPatternFailed"));
    } finally {
      setTlsLoading(false);
    }
  };

  const handleRemoveIncludePattern = async (pattern: string) => {
    setTlsLoading(true);
    try {
      const newList = (tlsConfig?.intercept_include || []).filter(
        (p) => p !== pattern,
      );
      const result = await updateTlsConfig({ intercept_include: newList });
      setTlsConfig(result);
      showTlsWhitelistChangeSuccess(t("settings.msg.removedFromInclude", { pattern }));
    } catch {
      message.error(t("settings.msg.removePatternFailed"));
    } finally {
      setTlsLoading(false);
    }
  };

  const handleAddAppExcludePattern = async () => {
    if (!newAppExcludePattern.trim()) {
      message.warning(t("settings.msg.enterPattern"));
      return;
    }

    const pattern = newAppExcludePattern.trim();
    if (tlsConfig?.app_intercept_exclude.includes(pattern)) {
      message.warning(t("settings.msg.patternExists"));
      return;
    }

    setTlsLoading(true);
    try {
      const newList = [...(tlsConfig?.app_intercept_exclude || []), pattern];
      const result = await updateTlsConfig({ app_intercept_exclude: newList });
      setTlsConfig(result);
      setNewAppExcludePattern("");
      message.success(t("settings.msg.addedToAppExclude", { pattern }));
    } catch {
      message.error(t("settings.msg.addPatternFailed"));
    } finally {
      setTlsLoading(false);
    }
  };

  const handleRemoveAppExcludePattern = async (pattern: string) => {
    setTlsLoading(true);
    try {
      const newList = (tlsConfig?.app_intercept_exclude || []).filter(
        (p) => p !== pattern,
      );
      const result = await updateTlsConfig({ app_intercept_exclude: newList });
      setTlsConfig(result);
      message.success(t("settings.msg.removedFromAppExclude", { pattern }));
    } catch {
      message.error(t("settings.msg.removePatternFailed"));
    } finally {
      setTlsLoading(false);
    }
  };

  const handleAddAppIncludePattern = async () => {
    if (!newAppIncludePattern.trim()) {
      message.warning(t("settings.msg.enterPattern"));
      return;
    }

    const pattern = newAppIncludePattern.trim();
    if (tlsConfig?.app_intercept_include.includes(pattern)) {
      message.warning(t("settings.msg.patternExists"));
      return;
    }

    setTlsLoading(true);
    try {
      const newList = [...(tlsConfig?.app_intercept_include || []), pattern];
      const result = await updateTlsConfig({ app_intercept_include: newList });
      setTlsConfig(result);
      setNewAppIncludePattern("");
      showTlsWhitelistChangeSuccess(t("settings.msg.addedToAppInclude", { pattern }));
    } catch {
      message.error(t("settings.msg.addPatternFailed"));
    } finally {
      setTlsLoading(false);
    }
  };

  const handleRemoveAppIncludePattern = async (pattern: string) => {
    setTlsLoading(true);
    try {
      const newList = (tlsConfig?.app_intercept_include || []).filter(
        (p) => p !== pattern,
      );
      const result = await updateTlsConfig({ app_intercept_include: newList });
      setTlsConfig(result);
      showTlsWhitelistChangeSuccess(t("settings.msg.removedFromAppInclude", { pattern }));
    } catch {
      message.error(t("settings.msg.removePatternFailed"));
    } finally {
      setTlsLoading(false);
    }
  };

  const updatePerfDraft = (update: Partial<TrafficConfig>) => {
    setPerfDraft((prev) => {
      const base = prev ?? performanceConfig?.traffic;
      if (!base) return prev;
      return { ...base, ...update };
    });
  };

  const schedulePerformanceUpdate = (
    key: keyof UpdateTrafficConfigRequest,
    payload: UpdateTrafficConfigRequest,
    successMessage: string,
    errorMessage: string,
  ) => {
    const existing = perfUpdateTimers.current[key];
    if (existing) {
      window.clearTimeout(existing);
    }
    perfUpdateTimers.current[key] = window.setTimeout(async () => {
      setPerfLoading(true);
      try {
        const result = await updatePerformanceConfig(payload);
        setPerformanceConfig(result);
        setPerfDraft(result.traffic);
        setBreakpointPerfDraft(result.breakpoint);
        if (typeof result.traffic.super_performance_mode === "boolean") {
          usePerformanceModeStore
            .getState()
            .setSuperPerformanceMode(result.traffic.super_performance_mode);
        }
        message.success(successMessage);
      } catch {
        message.error(errorMessage);
        if (performanceConfig) {
          setPerfDraft(performanceConfig.traffic);
          setBreakpointPerfDraft(performanceConfig.breakpoint);
        }
      } finally {
        setPerfLoading(false);
      }
    }, 600);
  };

  const handleMaxRecordsChange = (value: number | null) => {
    if (value === null) return;
    updatePerfDraft({ max_records: value });
    schedulePerformanceUpdate(
      "max_records",
      { max_records: value },
      t("settings.msg.maxRecordsUpdated", { value }),
      t("settings.msg.maxRecordsFailed"),
    );
  };

  const handleMaxDbSizeChange = (value: number) => {
    updatePerfDraft({ max_db_size_bytes: value });
    schedulePerformanceUpdate(
      "max_db_size_bytes",
      { max_db_size_bytes: value },
      t("settings.msg.maxDbSizeUpdated"),
      t("settings.msg.maxDbSizeFailed"),
    );
  };

  const handleMaxBodyMemorySizeChange = (value: number) => {
    updatePerfDraft({ max_body_memory_size: value });
    schedulePerformanceUpdate(
      "max_body_memory_size",
      { max_body_memory_size: value },
      t("settings.msg.maxBodyInlineUpdated"),
      t("settings.msg.maxBodyInlineFailed"),
    );
  };

  const handleMaxBodyBufferSizeChange = (value: number) => {
    updatePerfDraft({ max_body_buffer_size: value });
    schedulePerformanceUpdate(
      "max_body_buffer_size",
      { max_body_buffer_size: value },
      t("settings.msg.maxBodyBufferUpdated"),
      t("settings.msg.maxBodyBufferFailed"),
    );
  };

  const handleMaxBodyProbeSizeChange = (value: number) => {
    updatePerfDraft({ max_body_probe_size: value });
    schedulePerformanceUpdate(
      "max_body_probe_size",
      { max_body_probe_size: value },
      t("settings.msg.maxBodyProbeUpdated"),
      t("settings.msg.maxBodyProbeFailed"),
    );
  };

  const handleSuperPerformanceModeChange = (enabled: boolean) => {
    updatePerfDraft({ super_performance_mode: enabled });
    schedulePerformanceUpdate(
      "super_performance_mode",
      { super_performance_mode: enabled },
      enabled
        ? t("settings.msg.superPerfEnabled")
        : t("settings.msg.superPerfDisabled"),
      t("settings.msg.superPerfFailed"),
    );
  };

  const handleEnableBinaryTrafficCaptureChange = (checked: boolean) => {
    const performanceModeEnabled = !checked;
    updatePerfDraft({
      binary_traffic_performance_mode: performanceModeEnabled,
    });
    schedulePerformanceUpdate(
      "binary_traffic_performance_mode",
      { binary_traffic_performance_mode: performanceModeEnabled },
      checked
        ? t("settings.msg.binaryCaptureEnabled")
        : t("settings.msg.binaryPerfModeEnabled"),
      t("settings.msg.binaryCaptureFailed"),
    );
  };

  const handleInjectBifrostBadgeChange = (enabled: boolean) => {
    updatePerfDraft({ inject_bifrost_badge: enabled });
    schedulePerformanceUpdate(
      "inject_bifrost_badge",
      { inject_bifrost_badge: enabled },
      enabled ? t("settings.msg.badgeEnabled") : t("settings.msg.badgeDisabled"),
      t("settings.msg.badgeFailed"),
    );
  };

  const handleFileRetentionDaysChange = (value: number) => {
    updatePerfDraft({ file_retention_days: value });
    schedulePerformanceUpdate(
      "file_retention_days",
      { file_retention_days: value },
      t("settings.msg.fileRetentionUpdated", { value }),
      t("settings.msg.fileRetentionFailed"),
    );
  };

  const handleBreakpointTimeoutChange = (value: number) => {
    setBreakpointPerfDraft((prev) => {
      const base = prev ?? performanceConfig?.breakpoint;
      if (!base) return prev;
      return { ...base, timeout_ms: value };
    });
    schedulePerformanceUpdate(
      "breakpoint_timeout_ms",
      { breakpoint_timeout_ms: value },
      t("settings.msg.breakpointTimeoutUpdated", { value }),
      t("settings.msg.breakpointTimeoutFailed"),
    );
  };

  const handleClearBodyCache = async () => {
    setPerfLoading(true);
    try {
      const result = await clearBodyCache();
      message.success(result.message);
      fetchPerformanceConfig();
    } catch {
      message.error(t("settings.msg.clearBodyCacheFailed"));
    } finally {
      setPerfLoading(false);
    }
  };

  useEffect(() => {
    switch (activeTab) {
      case "proxy":
        void Promise.all([
          fetchDesktopRuntime(),
          fetchProxySettings(),
          fetchTlsConfigData(),
          fetchProxyAddressInfo(),
          fetchSystemProxy(),
          fetchSystemProxyLaunchd(),
          fetchCliProxy(),
          fetchPerformanceConfig(),
        ]);
        break;
      case "certificate":
        void Promise.all([fetchCertInfoData(), fetchProxyAddressInfo()]);
        break;
      case "metrics":
        void Promise.all([
          fetchHistory(3600),
          fetchAppMetricsData(),
          fetchHostMetricsData(),
        ]);
        break;
      case "access":
        void fetchWhitelistStatus();
        break;
      case "performance":
        void fetchPerformanceConfig();
        break;
      case "tray":
        void fetchProxySettings();
        break;
      case "sync":
      case "remote-invoke":
        void fetchSyncStatusData();
        break;
      default:
        break;
    }
  }, [
    activeTab,
    fetchAppMetricsData,
    fetchCertInfoData,
    fetchCliProxy,
    fetchDesktopRuntime,
    fetchHistory,
    fetchHostMetricsData,
    fetchPerformanceConfig,
    fetchProxyAddressInfo,
    fetchProxySettings,
    fetchSyncStatusData,
    fetchSystemProxy,
    fetchSystemProxyLaunchd,
    fetchTlsConfigData,
    fetchWhitelistStatus,
  ]);

  useEffect(() => {
    if (activeTab !== "sync" && activeTab !== "remote-invoke") {
      return;
    }

    let stopped = false;
    const refresh = async () => {
      try {
        const status = await getSyncStatus();
        if (stopped) return;
        applySyncStatus(status);
      } catch (error) {
        if (!suppressRestartErrors && !isConnectionIssueError(error)) {
          console.error("Failed to refresh sync status");
        }
      }
    };

    const timer = window.setInterval(() => {
      void refresh();
    }, 2_000);
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [activeTab, applySyncStatus, suppressRestartErrors]);

  useEffect(() => {
    const timers = perfUpdateTimers.current;
    return () => {
      Object.values(timers).forEach((timer) => {
        window.clearTimeout(timer);
      });
    };
  }, []);

  const copyProxyConfig = () => {
    const config = `HTTP Proxy: 127.0.0.1:${overview?.server.port || 9900}
HTTPS Proxy: 127.0.0.1:${overview?.server.port || 9900}`;
    copyToClipboard(config);
    message.success(t("settings.msg.proxyConfigCopied"));
  };

  const normalizeBifrostCloudUrl = useCallback((value?: string | null) => {
    const trimmed = value?.trim() ?? "";
    if (!trimmed) {
      return {
        ok: false as const,
        message: t("settings.msg.syncUrlRequired"),
      };
    }
    let parsed: URL;
    try {
      parsed = new URL(trimmed);
    } catch {
      return {
        ok: false as const,
        message: t("settings.msg.syncUrlInvalid"),
      };
    }
    if (parsed.protocol !== "https:") {
      return {
        ok: false as const,
        message: t("settings.msg.syncUrlHttps"),
      };
    }
    if (!parsed.hostname) {
      return {
        ok: false as const,
        message: t("settings.msg.syncUrlHostname"),
      };
    }
    parsed.hash = "";
    parsed.search = "";
    return {
      ok: true as const,
      value: parsed.toString().replace(/\/$/, ""),
    };
  }, [t]);

  const handleSyncProviderRemoteBaseUrlSave = useCallback(async (
    provider: SyncProviderStatus,
    remoteBaseUrlValue: string,
  ) => {
    const normalized = normalizeBifrostCloudUrl(remoteBaseUrlValue);
    if (!normalized.ok) {
      message.warning(normalized.message);
      return;
    }
    setSyncLoading(true);
    try {
      const status = await updateSyncConfig({ remote_base_url: normalized.value });
      applySyncStatus(status);
      message.success(t("settings.msg.providerUrlUpdated", { name: provider.name }));
    } catch {
      message.error(t("settings.msg.providerUrlFailed", { name: provider.name }));
    } finally {
      setSyncLoading(false);
    }
  }, [applySyncStatus, normalizeBifrostCloudUrl, t]);

  const handleSyncSignIn = useCallback(async () => {
    try {
      const status = await openSyncLogin();
      applySyncStatus(status);
      message.success(t("settings.msg.remoteSignInOpened"));
    } catch (error) {
      message.error(
        normalizeApiErrorMessage(error, t("settings.msg.remoteSignInFailed")),
      );
    }
  }, [applySyncStatus, t]);

  const handleSyncProviderSignIn = useCallback(
    async (provider: SyncProviderStatus, options?: { token?: string }) => {
      if (provider.id === "github_gist") {
        if (!options?.token?.trim()) {
          message.warning(t("settings.msg.githubTokenRequired"));
          return;
        }
        setSyncLoading(true);
        try {
          const status = await openSyncLogin({
            provider_id: "github_gist",
            token: options.token.trim(),
          });
          applySyncStatus(status);
          message.success(t("settings.msg.githubGistConnected"));
        } catch (error) {
          message.error(
            normalizeApiErrorMessage(error, t("settings.msg.githubGistFailed")),
          );
        } finally {
          setSyncLoading(false);
        }
        return;
      }
      setSyncLoading(true);
      try {
        if (provider.id === "bifrost_cloud") {
          const normalized = normalizeBifrostCloudUrl(provider.remote_base_url);
          if (!normalized.ok) {
            message.warning(normalized.message);
            return;
          }
          const status = await updateSyncConfig({
            enabled: true,
            remote_base_url: normalized.value,
          });
          applySyncStatus(status);
        } else if (provider.remote_base_url) {
          const status = await updateSyncConfig({
            enabled: true,
            remote_base_url: provider.remote_base_url,
          });
          applySyncStatus(status);
        }
        const status = await openSyncLogin();
        applySyncStatus(status);
        message.success(t("settings.msg.providerSignInOpened", { name: provider.name }));
      } catch (error) {
        message.error(
          normalizeApiErrorMessage(
            error,
            t("settings.msg.providerSignInFailed", { name: provider.name }),
          ),
        );
      } finally {
        setSyncLoading(false);
      }
    },
    [applySyncStatus, normalizeBifrostCloudUrl, t],
  );

  const handleSyncSignOut = useCallback(async () => {
    setSyncLoading(true);
    try {
      const status = await logoutSyncSession();
      applySyncStatus(status);
      message.success(t("settings.msg.syncSessionCleared"));
    } catch {
      message.error(t("settings.msg.syncSignOutFailed"));
    } finally {
      setSyncLoading(false);
    }
  }, [applySyncStatus, t]);

  const handleSyncProviderSignOut = useCallback(
    async (provider: SyncProviderStatus) => {
      setSyncLoading(true);
      try {
        const status = await logoutSyncProvider(provider.id);
        applySyncStatus(status);
        message.success(t("settings.msg.providerSignedOut", { name: provider.name }));
      } catch {
        message.error(t("settings.msg.providerSignOutFailed", { name: provider.name }));
      } finally {
        setSyncLoading(false);
      }
    },
    [applySyncStatus, t],
  );

  const handleDesktopProxyPortApply = useCallback(async () => {
    if (!isDesktopShell()) {
      return;
    }

    if (
      !Number.isInteger(desktopPortDraft) ||
      desktopPortDraft <= 0 ||
      desktopPortDraft > 65535
    ) {
      message.error(t("settings.msg.portRange"));
      return;
    }

    if (desktopPortDraft === desktopExpectedProxyPort) {
      message.info(t("settings.msg.portUnchanged"));
      return;
    }

    setDesktopPortSaving(true);
    try {
      beginDesktopCoreRestart(desktopPortDraft);
      setDesktopCorePhase("restarting", t("settings.msg.portRebinding"));
      const runtime = await updateDesktopProxyPort(desktopPortDraft);
      setDesktopProxyPort(runtime.proxyPort);
      setDesktopExpectedProxyPort(runtime.expectedProxyPort);
      setDesktopActualProxyPort(runtime.proxyPort);
      setDesktopPortDraft(runtime.expectedProxyPort);
      await waitForDesktopBackendReady(runtime.proxyPort);
      setDesktopCorePhase("reconnecting", t("settings.msg.portReconnecting"));
      const subscription = pushService.getSubscription();
      pushService.disconnect();
      pushService.connect(subscription);
      await Promise.all([
        fetchProxySettings(),
        fetchProxyAddressInfo(),
        fetchOverview(),
        useProxyStore.getState().fetchSystemProxy(),
        useProxyStore.getState().fetchCliProxy(),
      ]);
      message.success(
        runtime.expectedProxyPort === runtime.proxyPort
          ? t("settings.msg.portMoved", { port: runtime.proxyPort })
          : t("settings.msg.portBusySwitched", { expected: runtime.expectedProxyPort, port: runtime.proxyPort }),
      );
      window.setTimeout(() => {
        hideDesktopCoreRestart();
      }, 600);
    } catch (error) {
      const text = getErrorMessage(error, t("settings.msg.portSwitchFailed"));
      failDesktopCoreRestart(text);
      message.error(text);
      await Promise.all([fetchProxySettings(), fetchDesktopRuntime()]);
    } finally {
      setDesktopPortSaving(false);
    }
  }, [
    beginDesktopCoreRestart,
    desktopExpectedProxyPort,
    desktopPortDraft,
    failDesktopCoreRestart,
    fetchDesktopRuntime,
    fetchOverview,
    fetchProxyAddressInfo,
    fetchProxySettings,
    hideDesktopCoreRestart,
    setDesktopCorePhase,
    t,
  ]);

  const appSuggestions = useMemo(
    () => appMetrics.map((m) => m.app_name).filter((n) => n !== "Unknown"),
    [appMetrics],
  );

  const shouldDeferToDesktopOverlay = !overview;

  if (loading && !overview && !suppressRestartErrors) {
    return (
      <Spin size="large" style={{ display: "block", margin: "100px auto" }} />
    );
  }

  if (error && !suppressRestartErrors && !shouldDeferToDesktopOverlay) {
    return (
      <Alert
        type="error"
        message={t("settings.loadSystemInfoFailed")}
        description={error}
      />
    );
  }

  const formatBytes = (bytes: number) => {
    if (bytes === 0) return "0 B";
    const k = 1024;
    const sizes = ["B", "KB", "MB", "GB", "TB"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return `${(bytes / Math.pow(k, i)).toFixed(1)} ${sizes[i]}`;
  };

  const formatBytesRate = (bytesPerSec: number) => {
    return `${formatBytes(bytesPerSec)}/s`;
  };

  const buildSliderMarks = (
    min: number,
    max: number,
    step: number,
    formatter: (value: number) => string,
  ) => {
    const segments = 6;
    const values = new Set<number>();
    for (let i = 0; i <= segments; i += 1) {
      const raw = min + ((max - min) / segments) * i;
      const snapped = step > 0 ? Math.round(raw / step) * step : raw;
      const value = Math.min(max, Math.max(min, snapped));
      values.add(value);
    }
    values.add(min);
    values.add(max);
    const marks: Record<number, string> = {};
    Array.from(values)
      .sort((a, b) => a - b)
      .forEach((value) => {
        marks[value] = formatter(value);
      });
    return marks;
  };

  const memoryPercent = overview?.metrics.memory_usage_percent ?? 0;

  const trafficDraft = perfDraft ?? performanceConfig?.traffic;
  const breakpointDraft = breakpointPerfDraft ?? performanceConfig?.breakpoint;

  const maxRecordsMin = 1000;
  const maxRecordsMax = 100000;
  const maxRecordsStep = 100;
  const maxRecordsMarks = buildSliderMarks(
    maxRecordsMin,
    maxRecordsMax,
    maxRecordsStep,
    (value) => value.toLocaleString(),
  );
  const maxDbSizeMarks = buildSliderMarks(
    256 * 1024 * 1024,
    10 * 1024 * 1024 * 1024,
    256 * 1024 * 1024,
    formatBytes,
  );
  const maxBodyInlineMarks = buildSliderMarks(
    64 * 1024,
    10 * 1024 * 1024,
    64 * 1024,
    formatBytes,
  );
  const maxBodyBufferMarks = buildSliderMarks(
    1 * 1024 * 1024,
    64 * 1024 * 1024,
    1 * 1024 * 1024,
    formatBytes,
  );
  const maxBodyProbeMarks = {
    0: {
      label: t("common.off"),
      style: maxBodyProbeLeftMarkStyle,
    },
    [16 * 1024]: {
      label: "16KB",
      style: maxBodyProbeNearLeftMarkStyle,
    },
    [64 * 1024]: {
      label: "64KB",
      style: maxBodyProbeSecondNearLeftMarkStyle,
    },
    [256 * 1024]: "256KB",
    [1 * 1024 * 1024]: "1MB",
  };
  const fileRetentionMarks = buildSliderMarks(
    1,
    7,
    1,
    (value) => `${value}d`,
  );
  const breakpointTimeoutMarks = breakpointDraft
    ? buildSliderMarks(
        breakpointDraft.timeout_min_ms,
        breakpointDraft.timeout_max_ms,
        1000,
        (value) => `${Math.round(value / 1000)}s`,
      )
    : {};

  const tabItems = [
    {
      key: "proxy",
      label: (
        <span>
          <GlobalOutlined /> {t("settings.tabProxy")}
        </span>
      ),
      children: (
              <ProxyTab
                desktopMode={isDesktopShell()}
                desktopPlatform={getDesktopPlatform()}
                proxySettings={proxySettings}
                desktopPortDraft={desktopPortDraft}
                desktopPortSaving={desktopPortSaving}
                desktopExpectedProxyPort={desktopExpectedProxyPort}
                desktopProxyPort={desktopActualProxyPort}
                setDesktopPortDraft={setDesktopPortDraft}
                onApplyDesktopProxyPort={handleDesktopProxyPortApply}
                systemProxy={systemProxy}
                systemProxyLaunchd={systemProxyLaunchd}
                cliProxy={cliProxy}
                systemProxyLoading={systemProxyLoading}
                systemProxyLaunchdLoading={systemProxyLaunchdLoading}
                onToggleSystemProxy={handleSystemProxyToggle}
                onToggleSystemProxyLaunchd={handleSystemProxyLaunchdToggle}
                copyProxyConfig={copyProxyConfig}
          overview={overview}
          proxyAddressInfo={proxyAddressInfo}
          tlsConfig={tlsConfig}
          tlsLoading={tlsLoading}
          onToggleTlsInterception={handleTlsInterceptionToggle}
          onToggleUnsafeSsl={handleUnsafeSslToggle}
          onToggleDisconnectOnConfigChange={handleDisconnectOnConfigChangeToggle}
          injectBifrostBadge={trafficDraft?.inject_bifrost_badge ?? null}
          injectBifrostBadgeLoading={perfLoading}
          onToggleInjectBifrostBadge={handleInjectBifrostBadgeChange}
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
      ),
    },
    {
      key: "certificate",
      label: (
        <span>
          <SafetyCertificateOutlined /> {t("settings.tabCertificate")}
        </span>
      ),
      children: (
        <CertificateTab
          certInfo={certInfo}
          getCertDownloadUrl={getCertDownloadUrl}
          getCertQRCodeUrl={getCertQRCodeUrl}
          onCertInfoChange={setCertInfo}
        />
      ),
    },
    {
      key: "metrics",
      label: (
        <span>
          <DashboardOutlined /> {t("settings.tabMetrics")}
        </span>
      ),
      children: (
        <MetricsTab
          overview={overview}
          history={history}
          memoryPercent={memoryPercent}
          appMetrics={appMetrics}
          appMetricsSummary={appMetricsSummary}
          appMetricsLoading={appMetricsLoading}
          hostMetrics={hostMetrics}
          hostMetricsSummary={hostMetricsSummary}
          hostMetricsLoading={hostMetricsLoading}
          formatBytes={formatBytes}
          formatBytesRate={formatBytesRate}
          onRefreshAppMetrics={fetchAppMetricsData}
          onRefreshHostMetrics={fetchHostMetricsData}
        />
      ),
    },
    {
      key: "access",
      label: (
        <span>
          <SafetyOutlined /> {t("settings.tabAccessControl")}
        </span>
      ),
      children: <AccessControlTab />,
    },
    {
      key: "performance",
      label: (
        <span>
          <ThunderboltOutlined /> {t("settings.tabPerformance")}
        </span>
      ),
      children: (
        <PerformanceTab
          perfLoading={perfLoading}
          performanceConfig={performanceConfig}
          trafficDraft={trafficDraft}
          breakpointDraft={breakpointDraft}
          maxRecordsMin={maxRecordsMin}
          maxRecordsMax={maxRecordsMax}
          maxRecordsStep={maxRecordsStep}
          maxRecordsMarks={maxRecordsMarks}
          maxDbSizeMarks={maxDbSizeMarks}
          maxBodyInlineMarks={maxBodyInlineMarks}
          maxBodyBufferMarks={maxBodyBufferMarks}
          maxBodyProbeMarks={maxBodyProbeMarks}
          fileRetentionMarks={fileRetentionMarks}
          breakpointTimeoutMarks={breakpointTimeoutMarks}
          highlightSuperPerformanceMode={
            activeTab === "performance" &&
            highlightFromUrl === "super-performance-mode"
          }
          handleSuperPerformanceModeChange={handleSuperPerformanceModeChange}
          handleMaxRecordsChange={handleMaxRecordsChange}
          handleMaxDbSizeChange={handleMaxDbSizeChange}
          handleMaxBodyMemorySizeChange={handleMaxBodyMemorySizeChange}
              handleMaxBodyBufferSizeChange={handleMaxBodyBufferSizeChange}
              handleMaxBodyProbeSizeChange={handleMaxBodyProbeSizeChange}
              handleEnableBinaryTrafficCaptureChange={handleEnableBinaryTrafficCaptureChange}
              handleFileRetentionDaysChange={handleFileRetentionDaysChange}
              handleBreakpointTimeoutChange={handleBreakpointTimeoutChange}
              handleClearBodyCache={handleClearBodyCache}
              formatBytes={formatBytes}
        />
      ),
    },
    {
      key: "tray",
      label: (
        <span>
          <AppstoreOutlined /> {t("settings.tabTray")}
        </span>
      ),
      children: (
        <TrayTab
          trayConfig={proxySettings?.tray ?? null}
          trayLoading={trayLoading}
          onToggleTray={handleTrayToggle}
          onToggleSystemStats={handleTraySystemStatsToggle}
          onToggleSystemStatsItem={handleTraySystemStatsItemToggle}
        />
      ),
    },
    {
      key: "sync",
      label: (
        <span>
          <CloudOutlined /> {t("settings.tabSync")}
        </span>
      ),
      children: (
        <SyncTab
          syncStatus={syncStatus}
          syncLoading={syncLoading}
          onSignIn={handleSyncSignIn}
          onProviderSignIn={handleSyncProviderSignIn}
          onProviderRemoteBaseUrlSave={handleSyncProviderRemoteBaseUrlSave}
          onProviderSignOut={handleSyncProviderSignOut}
          onSignOut={handleSyncSignOut}
        />
      ),
    },
    {
      key: "remote",
      label: (
        <span>
          <KeyOutlined /> {t("settings.tabRemoteAccess")}
        </span>
      ),
      children: <RemoteAccessTab />,
    },
    {
      key: "remote-invoke",
      label: (
        <span>
          <ApiOutlined /> {t("settings.tabRemoteInvoke")}
        </span>
      ),
      children: (
        <RemoteInvokeTab
          syncStatus={syncStatus}
          onGoToSyncTab={() => handleTabChange("sync")}
          onSyncSignIn={handleSyncSignIn}
        />
      ),
    },
    {
      key: "language",
      label: (
        <span>
          <TranslationOutlined /> {t("settings.tabLanguage")}
        </span>
      ),
      children: <LanguageTab />,
    },
  ];

  return (
    <div
      style={{
        padding: "0 16px 0px",
        height: "100%",
        minHeight: 0,
        display: "flex",
        flexDirection: "column",
      }}
    >
      <Tabs
        className="settings-tabs"
        style={{ flex: 1, minHeight: 0 }}
        activeKey={activeTab}
        onChange={handleTabChange}
        items={tabItems}
      />
    </div>
  );
}
