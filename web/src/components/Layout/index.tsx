import { Outlet, useNavigate, useLocation } from "react-router-dom";
import { theme, Badge } from "antd";
import {
  GlobalOutlined,
  DashboardOutlined,
  FileTextOutlined,
  SettingOutlined,
  DatabaseOutlined,
  CodeOutlined,
  ThunderboltOutlined,
  SunOutlined,
  MoonOutlined,
  RobotOutlined,
  UsergroupAddOutlined,
  BellOutlined,
  BugOutlined,
  BorderOutlined,
  CloseOutlined,
  MinusOutlined,
} from "@ant-design/icons";
import type { CSSProperties } from "react";
import { useCallback, useEffect, useMemo } from "react";
import { usePendingAuthStore } from "../../stores/usePendingAuthStore";
import { usePendingIpTlsStore } from "../../stores/usePendingIpTlsStore";
import { usePairingRequestStore } from "../../stores/usePairingRequestStore";
import { useNotificationStore } from "../../stores/useNotificationStore";
import StatusBar from "../StatusBar";
import MobileDeviceTrustPrompt from "../MobileDeviceTrustPrompt";
import AvailabilityCheckNotificationCenter from "../AvailabilityCheckNotificationCenter";
import { setNavigateCallback, type ReferenceLocation } from "../BifrostEditor";
import { getDesktopPlatform, isDesktopShell } from "../../runtime";
import { useThemeStore } from "../../stores/useThemeStore";
import { useSyncStore } from "../../stores/useSyncStore";
import RulesDynamicIsland from "../../pages/Rules/RulesDynamicIsland";
import { getCurrentDesktopWindow } from "../../desktop/tauri";
import {
  DESKTOP_TOP_DRAG_HEIGHT,
  getDesktopDragRegionAttributes,
  getDesktopTopDragRightInset,
} from "./desktopChrome";
import {
  APP_SIDEBAR_WIDTH,
  SIDEBAR_MENU_ITEM_STYLE,
  SIDEBAR_MENU_SCROLL_STYLE,
} from "./sidebarLayout";

interface MenuItem {
  key: string;
  icon: React.ReactNode;
  label: string;
  hidden?: boolean;
}

export default function AppLayout() {
  const navigate = useNavigate();
  const location = useLocation();
  const { token } = theme.useToken();
  const {
    pendingCount,
    startSSE,
    stopSSE,
    fetchPendingList,
    requestNotificationPermission,
  } = usePendingAuthStore();
  const {
    startSSE: startIpTlsSSE,
    stopSSE: stopIpTlsSSE,
    fetchPendingList: fetchIpTlsPendingList,
  } = usePendingIpTlsStore();
  const {
    startPolling: startPairingPolling,
    stopPolling: stopPairingPolling,
  } = usePairingRequestStore();
  const desktopEnabled = isDesktopShell();
  const desktopPlatform = getDesktopPlatform();
  const desktopCustomChrome =
    desktopEnabled && (desktopPlatform === "macos" || desktopPlatform === "windows");
  const showWindowsControls = desktopEnabled && desktopPlatform === "windows";
  const resolvedTheme = useThemeStore((state) => state.resolvedTheme);
  const setThemeMode = useThemeStore((state) => state.setMode);
  const isDark = resolvedTheme === "dark";
  const syncStatus = useSyncStore((state) => state.syncStatus);
  const startSyncPolling = useSyncStore((state) => state.startPolling);
  const stopSyncPolling = useSyncStore((state) => state.stopPolling);

  useEffect(() => {
    startSyncPolling();
    return () => {
      stopSyncPolling();
    };
  }, [startSyncPolling, stopSyncPolling]);

  const showGroups = syncStatus?.enabled ?? false;

  const { unreadCount, fetchUnreadCount } = useNotificationStore();

  useEffect(() => {
    fetchUnreadCount();
    const timer = setInterval(fetchUnreadCount, 5000);
    return () => clearInterval(timer);
  }, [fetchUnreadCount]);

  const menuItems: MenuItem[] = useMemo(
    () => [
      { key: "/activity", icon: <DashboardOutlined />, label: "Activity" },
      { key: "/traffic", icon: <GlobalOutlined />, label: "Network" },
      { key: "/replay", icon: <ThunderboltOutlined />, label: "Replay" },
      { key: "/rules", icon: <FileTextOutlined />, label: "Rules" },
      { key: "/values", icon: <DatabaseOutlined />, label: "Values" },
      { key: "/scripts", icon: <CodeOutlined />, label: "Scripts" },
      { key: "/ai", icon: <RobotOutlined />, label: "AI" },
      { key: "/devtools", icon: <BugOutlined />, label: "DevTools" },
      { key: "/groups", icon: <UsergroupAddOutlined />, label: "Groups", hidden: !showGroups },
      { key: "/notifications", icon: <BellOutlined />, label: "Notify" },
      { key: "/settings", icon: <SettingOutlined />, label: "Settings" },
    ],
    [showGroups],
  );

  useEffect(() => {
    fetchPendingList();
    startSSE();
    requestNotificationPermission();
    fetchIpTlsPendingList();
    startIpTlsSSE();
    startPairingPolling();
    return () => {
      stopSSE();
      stopIpTlsSSE();
      stopPairingPolling();
    };
  }, [fetchPendingList, startSSE, stopSSE, requestNotificationPermission, fetchIpTlsPendingList, startIpTlsSSE, stopIpTlsSSE, startPairingPolling, stopPairingPolling]);

  useEffect(() => {
    const handleNavigate = (location: ReferenceLocation) => {
      if (location.uri) {
        navigate(location.uri);
      }
    };
    setNavigateCallback(handleNavigate);
    return () => {
      setNavigateCallback(null);
    };
  }, [navigate]);

  const handleThemeToggle = () => {
    setThemeMode(isDark ? "light" : "dark");
  };

  const handleOpenApiClick = () => {
    window.open("/_bifrost/swagger", "_blank", "noopener,noreferrer");
  };

  const handleWindowsWindowAction = (action: "minimize" | "maximize" | "close") => {
    const desktopWindow = getCurrentDesktopWindow();
    if (!desktopWindow) {
      return;
    }

    const task =
      action === "minimize"
        ? desktopWindow.minimize()
        : action === "maximize"
          ? desktopWindow.toggleMaximize()
          : desktopWindow.close();
    void task.catch((error: unknown) => {
      console.warn(`[desktop-window] failed to ${action} window`, error);
    });
  };

  const styles: Record<string, CSSProperties> = {
    layout: {
      display: "flex",
      flexDirection: "column",
      height: "100vh",
      width: "100vw",
      overflow: "hidden",
      position: "relative",
      background: desktopEnabled
        ? isDark
          ? "radial-gradient(circle at top left, rgba(56, 189, 248, 0.18) 0%, rgba(56, 189, 248, 0) 28%), radial-gradient(circle at 82% 12%, rgba(59, 130, 246, 0.16) 0%, rgba(59, 130, 246, 0) 24%), linear-gradient(180deg, rgba(8,12,18,0.6) 0%, rgba(11,16,24,0.5) 100%)"
          : "radial-gradient(circle at 14% 0%, rgba(125, 211, 252, 0.28) 0%, rgba(125, 211, 252, 0) 24%), radial-gradient(circle at 86% 10%, rgba(59, 130, 246, 0.16) 0%, rgba(59, 130, 246, 0) 20%), linear-gradient(180deg, rgba(247,249,252,0.64) 0%, rgba(241,245,249,0.5) 100%)"
        : token.colorBgLayout,
    },
    desktopAtmosphere: {
      position: "absolute",
      inset: 0,
      background: isDark
        ? "radial-gradient(circle at 18% 14%, rgba(71, 85, 105, 0.26) 0%, rgba(71, 85, 105, 0) 24%), radial-gradient(circle at 78% 82%, rgba(14, 165, 233, 0.14) 0%, rgba(14, 165, 233, 0) 28%), linear-gradient(180deg, rgba(255,255,255,0.02) 0%, rgba(255,255,255,0) 22%)"
        : "radial-gradient(circle at 16% 18%, rgba(255, 255, 255, 0.56) 0%, rgba(255, 255, 255, 0) 24%), radial-gradient(circle at 84% 78%, rgba(125, 211, 252, 0.22) 0%, rgba(125, 211, 252, 0) 26%), linear-gradient(180deg, rgba(255,255,255,0.28) 0%, rgba(255,255,255,0) 24%)",
      pointerEvents: "none",
      zIndex: 0,
    },
    desktopNoise: {
      position: "absolute",
      inset: 0,
      opacity: isDark ? 0.08 : 0.05,
      backgroundImage:
        "url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='140' height='140' viewBox='0 0 140 140'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='1.15' numOctaves='2' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='140' height='140' filter='url(%23n)' opacity='0.8'/%3E%3C/svg%3E\")",
      mixBlendMode: isDark ? "soft-light" : "multiply",
      pointerEvents: "none",
      zIndex: 0,
    },
    windowsWindowControls: {
      position: "absolute",
      top: 8,
      right: 8,
      height: 28,
      display: "flex",
      alignItems: "center",
      gap: 2,
      padding: 2,
      borderRadius: 6,
      background: isDark ? "rgba(8, 13, 20, 0.36)" : "rgba(255, 255, 255, 0.42)",
      border: isDark
        ? "1px solid rgba(148, 163, 184, 0.14)"
        : "1px solid rgba(255, 255, 255, 0.34)",
      backdropFilter: "blur(12px) saturate(1.08)",
      zIndex: 20,
    },
    windowsWindowControlButton: {
      width: 32,
      height: 24,
      border: 0,
      borderRadius: 4,
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      padding: 0,
      margin: 0,
      color: isDark ? "rgba(226, 232, 240, 0.88)" : "rgba(30, 41, 59, 0.82)",
      background: "transparent",
      cursor: "pointer",
      fontSize: 12,
      lineHeight: 1,
    },
    macTopWash: {
      position: "absolute",
      top: 0,
      left: 0,
      right: 0,
      height: 124,
      background:
        isDark
          ? "linear-gradient(180deg, rgba(14,19,29,0.84) 0%, rgba(14,19,29,0.44) 52%, rgba(14,19,29,0) 100%)"
          : "linear-gradient(180deg, rgba(248,250,253,0.96) 0%, rgba(248,250,253,0.72) 52%, rgba(248,250,253,0) 100%)",
      pointerEvents: "none",
      zIndex: 1,
    },
    desktopTopDragRegion: {
      position: "absolute",
      top: 0,
      left: APP_SIDEBAR_WIDTH,
      right: 0,
      height: DESKTOP_TOP_DRAG_HEIGHT,
      zIndex: 4,
      cursor: "default",
      userSelect: "none",
      WebkitUserSelect: "none",
    },
    main: {
      display: "flex",
      flex: 1,
      overflow: "hidden",
      position: "relative",
      zIndex: 2,
    },
    sidebar: {
      width: APP_SIDEBAR_WIDTH,
      height: "100%",
      background:
        desktopEnabled
          ? desktopPlatform === "macos"
            ? isDark
              ? "linear-gradient(180deg, rgba(16,22,33,0.76) 0%, rgba(16,22,33,0.68) 72px, rgba(12,18,27,0.72) 100%)"
              : "linear-gradient(180deg, rgba(249,250,252,0.92) 0%, rgba(249,250,252,0.84) 72px, rgba(255,255,255,0.88) 100%)"
            : isDark
              ? "linear-gradient(180deg, rgba(12,18,27,0.66) 0%, rgba(12,18,27,0.56) 100%)"
              : "linear-gradient(180deg, rgba(255,255,255,0.58) 0%, rgba(248,250,252,0.5) 100%)"
          : token.colorBgContainer,
      borderRight: desktopEnabled
        ? isDark
          ? "1px solid rgba(148, 163, 184, 0.12)"
          : "1px solid rgba(255, 255, 255, 0.28)"
        : `1px solid ${token.colorBorderSecondary}`,
      display: "flex",
      flexDirection: "column",
      alignItems: "center",
      paddingTop: desktopCustomChrome ? 0 : 8,
      flexShrink: 0,
      backdropFilter: desktopEnabled ? "blur(18px) saturate(1.08)" : undefined,
      cursor: desktopCustomChrome ? "default" : undefined,
      userSelect: desktopCustomChrome ? "none" : undefined,
    },
    macWindowControlSpacer: {
      width: "100%",
      height: 38,
      minHeight: 38,
      flexShrink: 0,
      cursor: "default",
      userSelect: "none",
    },
    menuScroll: {
      ...SIDEBAR_MENU_SCROLL_STYLE,
    },
    menuItem: {
      ...SIDEBAR_MENU_ITEM_STYLE,
      cursor: "pointer",
      fontSize: 18,
      color: token.colorTextSecondary,
      transition: "all 0.2s",
    },
    menuItemLabel: {
      marginTop: 4,
      fontSize: 9,
      lineHeight: "9px",
      whiteSpace: "nowrap",
      color: "inherit",
      pointerEvents: desktopCustomChrome ? "none" : undefined,
    },
    menuItemIcon: {
      pointerEvents: desktopCustomChrome ? "none" : undefined,
    },
    menuItemActive: {
      color: token.colorPrimary,
      backgroundColor: token.colorPrimaryBg,
    },
    openApiLink: {
      flexShrink: 0,
      marginBottom: 6,
      padding: "2px 0",
      width: 44,
      borderRadius: 6,
      color: token.colorTextSecondary,
      cursor: "pointer",
      fontSize: 9,
      lineHeight: "12px",
      textAlign: "center",
      userSelect: "none",
      transition: "all 0.2s",
    },
    activeBorder: {
      position: "absolute",
      left: 0,
      top: 8,
      bottom: 8,
      width: 3,
      backgroundColor: token.colorPrimary,
      borderRadius: "0 2px 2px 0",
    },
    content: {
      flex: 1,
      display: "flex",
      flexDirection: "column",
      overflow: "auto",
      boxSizing: "border-box",
      paddingTop: desktopCustomChrome ? DESKTOP_TOP_DRAG_HEIGHT : undefined,
      background:
        desktopEnabled
          ? desktopPlatform === "macos"
            ? isDark
              ? "linear-gradient(180deg, rgba(14,20,30,0.58) 0%, rgba(14,20,30,0.18) 88px, transparent 160px), linear-gradient(90deg, rgba(12,18,27,0.28) 0%, rgba(12,18,27,0) 120px), rgba(9,13,20,0.34)"
              : "linear-gradient(180deg, rgba(249,251,253,0.84) 0%, rgba(249,251,253,0.32) 88px, transparent 160px), linear-gradient(90deg, rgba(246,248,251,0.42) 0%, rgba(246,248,251,0) 120px), rgba(247,249,252,0.34)"
            : isDark
              ? "linear-gradient(180deg, rgba(10,15,23,0.34) 0%, rgba(10,15,23,0.2) 100%)"
              : "linear-gradient(180deg, rgba(255,255,255,0.28) 0%, rgba(248,250,252,0.18) 100%)"
          : token.colorBgLayout,
    },
  };

  const handleClick = (key: string) => {
    navigate(key);
  };

  const handleNavigateRuleFromIsland = useCallback(
    (name: string, groupId: string | null) => {
      const params = new URLSearchParams();
      if (groupId) {
        params.set("group", groupId);
      }
      params.set("rule", name);
      navigate({ pathname: "/rules", search: `?${params.toString()}` });
    },
    [navigate],
  );
  const desktopDragRegionAttributes =
    getDesktopDragRegionAttributes(desktopCustomChrome);

  const isActive = (key: string) => {
    if (key === "/activity" && location.pathname === "/") return true;
    return location.pathname === key || location.pathname.startsWith(key + "/");
  };

  const renderMenuIcon = (item: MenuItem) => {
    if (item.key === "/settings" && pendingCount > 0) {
      return (
        <Badge count={pendingCount} size="small" offset={[4, -4]}>
          {item.icon}
        </Badge>
      );
    }
    if (item.key === "/notifications" && unreadCount > 0) {
      return (
        <Badge count={unreadCount} size="small" offset={[4, -4]}>
          {item.icon}
        </Badge>
      );
    }
    return item.icon;
  };

  return (
    <div style={styles.layout}>
      {desktopEnabled && desktopPlatform === "macos" ? (
        <>
          <div style={styles.macTopWash} />
        </>
      ) : null}
      {desktopEnabled ? (
        <>
          <div style={styles.desktopAtmosphere} />
          <div style={styles.desktopNoise} />
        </>
      ) : null}
      {desktopCustomChrome ? (
        <div
          data-testid="desktop-top-drag-region"
          {...desktopDragRegionAttributes}
          style={{
            ...styles.desktopTopDragRegion,
            right: getDesktopTopDragRightInset(desktopPlatform),
          }}
        />
      ) : null}
      {showWindowsControls ? (
        <div
          data-testid="desktop-windows-window-controls"
          style={styles.windowsWindowControls}
        >
          <button
            type="button"
            data-testid="desktop-window-minimize"
            aria-label="Minimize window"
            title="Minimize"
            style={styles.windowsWindowControlButton}
            onClick={() => handleWindowsWindowAction("minimize")}
          >
            <MinusOutlined />
          </button>
          <button
            type="button"
            data-testid="desktop-window-maximize"
            aria-label="Maximize window"
            title="Maximize"
            style={styles.windowsWindowControlButton}
            onClick={() => handleWindowsWindowAction("maximize")}
          >
            <BorderOutlined />
          </button>
          <button
            type="button"
            data-testid="desktop-window-close"
            aria-label="Close window"
            title="Close"
            style={{
              ...styles.windowsWindowControlButton,
              color: isDark ? "rgba(248, 113, 113, 0.92)" : "rgba(185, 28, 28, 0.88)",
            }}
            onClick={() => handleWindowsWindowAction("close")}
          >
            <CloseOutlined />
          </button>
        </div>
      ) : null}
      <div style={styles.main}>
        <MobileDeviceTrustPrompt />
        <AvailabilityCheckNotificationCenter />
        <RulesDynamicIsland
          onNavigateRule={handleNavigateRuleFromIsland}
          defaultTop={desktopCustomChrome ? DESKTOP_TOP_DRAG_HEIGHT + 10 : undefined}
        />
        <div
          style={styles.sidebar}
          data-testid="desktop-sidebar-window-drag-region"
        >
          {desktopCustomChrome ? (
            <div
              data-testid="desktop-window-control-spacer"
              {...desktopDragRegionAttributes}
              style={styles.macWindowControlSpacer}
            />
          ) : null}
          <div
            className="app-sidebar-nav-scroll"
            style={styles.menuScroll}
            data-testid="app-sidebar-nav-scroll"
          >
            {menuItems.filter((item) => !item.hidden).map((item) => {
              const active = isActive(item.key);
              return (
                <div
                  key={item.key}
                  data-testid="app-sidebar-nav-item"
                  data-nav-label={item.label}
                  data-nav-key={item.key}
                  style={{
                    ...styles.menuItem,
                    ...(active ? styles.menuItemActive : {}),
                  }}
                  onClick={() => handleClick(item.key)}
                >
                  {active && <div style={styles.activeBorder as CSSProperties} />}
                  <div data-testid="app-sidebar-nav-icon" style={styles.menuItemIcon}>
                    {renderMenuIcon(item)}
                  </div>
                  <div data-testid="app-sidebar-nav-label" style={styles.menuItemLabel}>
                    {item.label}
                  </div>
                </div>
              );
            })}
          </div>
          <div
            data-testid="app-sidebar-openapi"
            style={styles.openApiLink}
            onClick={handleOpenApiClick}
            title="Open OpenAPI documentation"
          >
            OpenAPI
          </div>
          <div
            data-testid="theme-toggle"
            style={{
              flexShrink: 0,
              marginBottom: 8,
              width: 36,
              height: 36,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              cursor: "pointer",
              fontSize: 16,
              borderRadius: "50%",
              color: isDark ? "#facc15" : "#64748b",
              background: isDark
                ? "rgba(250, 204, 21, 0.12)"
                : "rgba(100, 116, 139, 0.1)",
              transition: "all 0.3s",
            }}
            onClick={handleThemeToggle}
          >
            <span style={styles.menuItemIcon}>
              {isDark ? <SunOutlined /> : <MoonOutlined />}
            </span>
          </div>
        </div>
        <div style={styles.content}>
          <Outlet />
        </div>
      </div>
      <StatusBar />
    </div>
  );
}
