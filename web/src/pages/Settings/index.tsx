import { useEffect, useState, useCallback, useMemo, useRef } from "react";
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
  logoutSyncSession,
  openSyncLogin,
  runSyncNow,
  updateSyncConfig,
  type SyncProviderStatus,
  type SyncStatus,
} from "../../api/sync";
import { isConnectionIssueError } from "../../api/client";
import { copyToClipboard } from "../../utils/clipboard";
import { useSyncStore } from "../../stores/useSyncStore";
import {
  getCertInfo,
  getCertDownloadUrl,
  getCertQRCodeUrl,
  type CertInfo,
} from "../../api/cert";
import type { AppMetrics, HostMetrics } from "../../types";
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
const DEFAULT_TAB = "proxy";
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
];

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
  const [appMetricsLoading, setAppMetricsLoading] = useState(false);
  const [hostMetrics, setHostMetrics] = useState<HostMetrics[]>([]);
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
  const [syncRemoteBaseUrlDraft, setSyncRemoteBaseUrlDraft] = useState("");
  const syncRemoteBaseUrlDirtyRef = useRef(false);
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

  const applySyncStatus = useCallback(
    (status: SyncStatus, options?: { syncRemoteBaseUrlDraft?: boolean }) => {
      setSyncStatus(status);
      if (
        options?.syncRemoteBaseUrlDraft ||
        !syncRemoteBaseUrlDirtyRef.current
      ) {
        syncRemoteBaseUrlDirtyRef.current = false;
        setSyncRemoteBaseUrlDraft(status.remote_base_url);
      }
      useSyncStore.getState().setSyncStatus(status);
    },
    [],
  );

  const handleSyncRemoteBaseUrlDraftChange = useCallback((value: string) => {
    syncRemoteBaseUrlDirtyRef.current = true;
    setSyncRemoteBaseUrlDraft(value);
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
      const metrics = await getAppMetrics();
      setAppMetrics(metrics);
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
      const metrics = await getHostMetrics();
      setHostMetrics(metrics);
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
        enabled ? "System proxy enabled" : "System proxy disabled",
      );
    } else {
      const proxyError = useProxyStore.getState().error;
      message.error(proxyError || "Failed to toggle system proxy");
    }
  };

  const handleSystemProxyLaunchdToggle = async (enabled: boolean) => {
    const success = await toggleSystemProxyLaunchd(enabled);
    if (success) {
      message.success(
        enabled
          ? "System proxy cleanup fallback installed"
          : "System proxy cleanup fallback removed",
      );
    } else {
      const proxyError = useProxyStore.getState().error;
      message.error(proxyError || "Failed to update cleanup fallback");
    }
  };

  const handleTrayToggle = async (enabled: boolean) => {
    setTrayLoading(true);
    try {
      const tray = await updateTrayConfig({ enabled });
      setProxySettings((prev) => (prev ? { ...prev, tray } : prev));
      message.success(enabled ? "Tray icon enabled" : "Tray icon disabled");
    } catch {
      message.error("Failed to update tray setting");
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
        enabled ? "Tray system stats enabled" : "Tray system stats disabled",
      );
    } catch {
      message.error("Failed to update tray system stats setting");
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
      message.success("Tray system status item updated");
    } catch {
      message.error("Failed to update tray system status item");
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
        enabled ? "HTTPS interception enabled" : "HTTPS interception disabled",
      );
    } catch {
      message.error("Failed to update TLS config");
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
          ? "Certificate verification disabled"
          : "Certificate verification enabled",
      );
    } catch {
      message.error("Failed to update TLS config");
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
          ? "Auto-disconnect on config change enabled"
          : "Auto-disconnect on config change disabled",
      );
    } catch {
      message.error("Failed to update TLS config");
    } finally {
      setTlsLoading(false);
    }
  };

  const handleAddExcludePattern = async () => {
    if (!newExcludePattern.trim()) {
      message.warning("Please enter a pattern");
      return;
    }

    const pattern = newExcludePattern.trim();
    if (tlsConfig?.intercept_exclude.includes(pattern)) {
      message.warning("Pattern already exists");
      return;
    }

    setTlsLoading(true);
    try {
      const newList = [...(tlsConfig?.intercept_exclude || []), pattern];
      const result = await updateTlsConfig({ intercept_exclude: newList });
      setTlsConfig(result);
      setNewExcludePattern("");
      message.success(`Added ${pattern} to exclude list`);
    } catch {
      message.error("Failed to add pattern");
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
      message.success(`Removed ${pattern} from exclude list`);
    } catch {
      message.error("Failed to remove pattern");
    } finally {
      setTlsLoading(false);
    }
  };

  const handleAddIncludePattern = async () => {
    if (!newIncludePattern.trim()) {
      message.warning("Please enter a pattern");
      return;
    }

    const pattern = newIncludePattern.trim();
    if (tlsConfig?.intercept_include.includes(pattern)) {
      message.warning("Pattern already exists");
      return;
    }

    setTlsLoading(true);
    try {
      const newList = [...(tlsConfig?.intercept_include || []), pattern];
      const result = await updateTlsConfig({ intercept_include: newList });
      setTlsConfig(result);
      setNewIncludePattern("");
      showTlsWhitelistChangeSuccess(`Added ${pattern} to include list`);
    } catch {
      message.error("Failed to add pattern");
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
      showTlsWhitelistChangeSuccess(`Removed ${pattern} from include list`);
    } catch {
      message.error("Failed to remove pattern");
    } finally {
      setTlsLoading(false);
    }
  };

  const handleAddAppExcludePattern = async () => {
    if (!newAppExcludePattern.trim()) {
      message.warning("Please enter a pattern");
      return;
    }

    const pattern = newAppExcludePattern.trim();
    if (tlsConfig?.app_intercept_exclude.includes(pattern)) {
      message.warning("Pattern already exists");
      return;
    }

    setTlsLoading(true);
    try {
      const newList = [...(tlsConfig?.app_intercept_exclude || []), pattern];
      const result = await updateTlsConfig({ app_intercept_exclude: newList });
      setTlsConfig(result);
      setNewAppExcludePattern("");
      message.success(`Added ${pattern} to app exclude list`);
    } catch {
      message.error("Failed to add pattern");
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
      message.success(`Removed ${pattern} from app exclude list`);
    } catch {
      message.error("Failed to remove pattern");
    } finally {
      setTlsLoading(false);
    }
  };

  const handleAddAppIncludePattern = async () => {
    if (!newAppIncludePattern.trim()) {
      message.warning("Please enter a pattern");
      return;
    }

    const pattern = newAppIncludePattern.trim();
    if (tlsConfig?.app_intercept_include.includes(pattern)) {
      message.warning("Pattern already exists");
      return;
    }

    setTlsLoading(true);
    try {
      const newList = [...(tlsConfig?.app_intercept_include || []), pattern];
      const result = await updateTlsConfig({ app_intercept_include: newList });
      setTlsConfig(result);
      setNewAppIncludePattern("");
      showTlsWhitelistChangeSuccess(`Added ${pattern} to app include list`);
    } catch {
      message.error("Failed to add pattern");
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
      showTlsWhitelistChangeSuccess(`Removed ${pattern} from app include list`);
    } catch {
      message.error("Failed to remove pattern");
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
      `Max records updated to ${value}`,
      "Failed to update max records",
    );
  };

  const handleMaxDbSizeChange = (value: number) => {
    updatePerfDraft({ max_db_size_bytes: value });
    schedulePerformanceUpdate(
      "max_db_size_bytes",
      { max_db_size_bytes: value },
      "Max DB size updated",
      "Failed to update max DB size",
    );
  };

  const handleMaxBodyMemorySizeChange = (value: number) => {
    updatePerfDraft({ max_body_memory_size: value });
    schedulePerformanceUpdate(
      "max_body_memory_size",
      { max_body_memory_size: value },
      "Max body inline size updated",
      "Failed to update max body inline size",
    );
  };

  const handleMaxBodyBufferSizeChange = (value: number) => {
    updatePerfDraft({ max_body_buffer_size: value });
    schedulePerformanceUpdate(
      "max_body_buffer_size",
      { max_body_buffer_size: value },
      "Max body buffer size updated",
      "Failed to update max body buffer size",
    );
  };

  const handleMaxBodyProbeSizeChange = (value: number) => {
    updatePerfDraft({ max_body_probe_size: value });
    schedulePerformanceUpdate(
      "max_body_probe_size",
      { max_body_probe_size: value },
      "Max body probe size updated",
      "Failed to update max body probe size",
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
        ? "Binary file capture and decoding enabled"
        : "Binary traffic performance mode enabled",
      "Failed to update binary traffic capture setting",
    );
  };

  const handleInjectBifrostBadgeChange = (enabled: boolean) => {
    updatePerfDraft({ inject_bifrost_badge: enabled });
    schedulePerformanceUpdate(
      "inject_bifrost_badge",
      { inject_bifrost_badge: enabled },
      enabled ? "Bifrost badge enabled" : "Bifrost badge disabled",
      "Failed to update Bifrost badge setting",
    );
  };

  const handleFileRetentionDaysChange = (value: number) => {
    updatePerfDraft({ file_retention_days: value });
    schedulePerformanceUpdate(
      "file_retention_days",
      { file_retention_days: value },
      `File retention updated to ${value} days`,
      "Failed to update file retention days",
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
      `Breakpoint timeout updated to ${value}ms`,
      "Failed to update breakpoint timeout",
    );
  };

  const handleClearBodyCache = async () => {
    setPerfLoading(true);
    try {
      const result = await clearBodyCache();
      message.success(result.message);
      fetchPerformanceConfig();
    } catch {
      message.error("Failed to clear body cache");
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
    message.success("Proxy config copied to clipboard");
  };

  const handleSyncToggle = useCallback(async (enabled: boolean) => {
    setSyncLoading(true);
    try {
      const status = await updateSyncConfig({ enabled });
      applySyncStatus(status);
      message.success(enabled ? "Remote sync enabled" : "Remote sync disabled");
    } catch {
      message.error("Failed to update sync setting");
    } finally {
      setSyncLoading(false);
    }
  }, [applySyncStatus]);

  const handleSyncAutoToggle = useCallback(async (autoSync: boolean) => {
    setSyncLoading(true);
    try {
      const status = await updateSyncConfig({ auto_sync: autoSync });
      applySyncStatus(status);
      message.success(autoSync ? "Auto sync enabled" : "Auto sync disabled");
    } catch {
      message.error("Failed to update auto sync");
    } finally {
      setSyncLoading(false);
    }
  }, [applySyncStatus]);

  const handleSyncRemoteBaseUrlSave = useCallback(async () => {
    const remoteBaseUrl = syncRemoteBaseUrlDraft.trim();
    if (!remoteBaseUrl) {
      message.warning("Please enter the remote Bifrost URL");
      return;
    }
    setSyncLoading(true);
    try {
      const status = await updateSyncConfig({ remote_base_url: remoteBaseUrl });
      applySyncStatus(status, { syncRemoteBaseUrlDraft: true });
      message.success("Remote sync URL updated");
    } catch {
      message.error("Failed to update remote sync URL");
    } finally {
      setSyncLoading(false);
    }
  }, [applySyncStatus, syncRemoteBaseUrlDraft]);

  const handleSyncSignIn = useCallback(async () => {
    try {
      const status = await openSyncLogin();
      applySyncStatus(status);
      message.success("Remote sign-in window opened");
    } catch {
      message.error("Failed to open remote sign-in page");
    }
  }, [applySyncStatus]);

  const handleSyncProviderSignIn = useCallback(
    async (provider: SyncProviderStatus) => {
      if (provider.id === "github_gist") {
        message.info("GitHub Gist sync is not connected on this device");
        return;
      }
      setSyncLoading(true);
      try {
        if (provider.remote_base_url) {
          const status = await updateSyncConfig({
            enabled: true,
            remote_base_url: provider.remote_base_url,
          });
          applySyncStatus(status, { syncRemoteBaseUrlDraft: true });
        }
        const status = await openSyncLogin();
        applySyncStatus(status);
        message.success(`${provider.name} sign-in window opened`);
      } catch {
        message.error(`Failed to open ${provider.name} sign-in page`);
      } finally {
        setSyncLoading(false);
      }
    },
    [applySyncStatus],
  );

  const handleSyncSignOut = useCallback(async () => {
    setSyncLoading(true);
    try {
      const status = await logoutSyncSession();
      applySyncStatus(status);
      message.success("Sync session cleared");
    } catch {
      message.error("Failed to sign out from remote sync");
    } finally {
      setSyncLoading(false);
    }
  }, [applySyncStatus]);

  const handleSyncRunNow = useCallback(async () => {
    setSyncLoading(true);
    try {
      const status = await runSyncNow();
      applySyncStatus(status);
      message.success("Sync requested");
    } catch {
      message.error("Failed to trigger sync");
    } finally {
      setSyncLoading(false);
    }
  }, [applySyncStatus]);

  const handleDesktopProxyPortApply = useCallback(async () => {
    if (!isDesktopShell()) {
      return;
    }

    if (
      !Number.isInteger(desktopPortDraft) ||
      desktopPortDraft <= 0 ||
      desktopPortDraft > 65535
    ) {
      message.error("Port must be between 1 and 65535");
      return;
    }

    if (desktopPortDraft === desktopExpectedProxyPort) {
      message.info("Proxy port is unchanged");
      return;
    }

    setDesktopPortSaving(true);
    try {
      beginDesktopCoreRestart(desktopPortDraft);
      setDesktopCorePhase("restarting", "Rebinding the proxy listener to the requested port.");
      const runtime = await updateDesktopProxyPort(desktopPortDraft);
      setDesktopProxyPort(runtime.proxyPort);
      setDesktopExpectedProxyPort(runtime.expectedProxyPort);
      setDesktopActualProxyPort(runtime.proxyPort);
      setDesktopPortDraft(runtime.expectedProxyPort);
      await waitForDesktopBackendReady(runtime.proxyPort);
      setDesktopCorePhase("reconnecting", "Refreshing proxy state and reconnecting live data streams.");
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
          ? `Proxy listener moved to port ${runtime.proxyPort}`
          : `Preferred port ${runtime.expectedProxyPort} was busy, switched to ${runtime.proxyPort}`,
      );
      window.setTimeout(() => {
        hideDesktopCoreRestart();
      }, 600);
    } catch (error) {
      const text = getErrorMessage(error, "Failed to switch proxy port");
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
        message="Failed to load system info"
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

  const memoryPercent = overview
    ? (overview.metrics.memory_used / overview.metrics.memory_total) * 100
    : 0;

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
  const maxBodyProbeMarks: Record<number, string> = {
    0: "Off",
    [16 * 1024]: "16KB",
    [64 * 1024]: "64KB",
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
          <GlobalOutlined /> Proxy
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
          <SafetyCertificateOutlined /> Certificate
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
          <DashboardOutlined /> Metrics
        </span>
      ),
      children: (
        <MetricsTab
          overview={overview}
          history={history}
          memoryPercent={memoryPercent}
          appMetrics={appMetrics}
          appMetricsLoading={appMetricsLoading}
          hostMetrics={hostMetrics}
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
          <SafetyOutlined /> Access Control
        </span>
      ),
      children: <AccessControlTab />,
    },
    {
      key: "performance",
      label: (
        <span>
          <ThunderboltOutlined /> Performance
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
          <AppstoreOutlined /> Tray
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
          <CloudOutlined /> Sync
        </span>
      ),
      children: (
        <SyncTab
          syncStatus={syncStatus}
          syncLoading={syncLoading}
          remoteBaseUrlDraft={syncRemoteBaseUrlDraft}
          setRemoteBaseUrlDraft={handleSyncRemoteBaseUrlDraftChange}
          onToggleEnabled={handleSyncToggle}
          onToggleAutoSync={handleSyncAutoToggle}
          onSaveRemoteBaseUrl={handleSyncRemoteBaseUrlSave}
          onSignIn={handleSyncSignIn}
          onProviderSignIn={handleSyncProviderSignIn}
          onSignOut={handleSyncSignOut}
          onRunSync={handleSyncRunNow}
        />
      ),
    },
    {
      key: "remote",
      label: (
        <span>
          <KeyOutlined /> Remote Access
        </span>
      ),
      children: <RemoteAccessTab />,
    },
    {
      key: "remote-invoke",
      label: (
        <span>
          <ApiOutlined /> Remote Invoke
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
