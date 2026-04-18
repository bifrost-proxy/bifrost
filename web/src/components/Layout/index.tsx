import { Outlet, useNavigate, useLocation } from "react-router-dom";
import { theme, Badge } from "antd";
import {
  GlobalOutlined,
  FileTextOutlined,
  SettingOutlined,
  DatabaseOutlined,
  CodeOutlined,
  ThunderboltOutlined,
  SunOutlined,
  MoonOutlined,
  UsergroupAddOutlined,
  BellOutlined,
} from "@ant-design/icons";
import type { CSSProperties } from "react";
import { useEffect, useMemo } from "react";
import { usePendingAuthStore } from "../../stores/usePendingAuthStore";
import { usePendingIpTlsStore } from "../../stores/usePendingIpTlsStore";
import { useNotificationStore } from "../../stores/useNotificationStore";
import StatusBar from "../StatusBar";
import { setNavigateCallback, type ReferenceLocation } from "../BifrostEditor";
import { getDesktopPlatform, isDesktopShell } from "../../runtime";
import { useThemeStore } from "../../stores/useThemeStore";
import { useSyncStore } from "../../stores/useSyncStore";

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
  const desktopEnabled = isDesktopShell();
  const desktopPlatform = getDesktopPlatform();
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
      { key: "/traffic", icon: <GlobalOutlined />, label: "Network" },
      { key: "/replay", icon: <ThunderboltOutlined />, label: "Replay" },
      { key: "/rules", icon: <FileTextOutlined />, label: "Rules" },
      { key: "/values", icon: <DatabaseOutlined />, label: "Values" },
      { key: "/scripts", icon: <CodeOutlined />, label: "Scripts" },
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
    return () => {
      stopSSE();
      stopIpTlsSSE();
    };
  }, [fetchPendingList, startSSE, stopSSE, requestNotificationPermission, fetchIpTlsPendingList, startIpTlsSSE, stopIpTlsSSE]);

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
    macSidebarWash: {
      position: "absolute",
      top: 0,
      left: 0,
      width: 112,
      bottom: 20,
      background:
        isDark
          ? "linear-gradient(180deg, rgba(12,18,27,0.88) 0%, rgba(12,18,27,0.78) 72px, rgba(12,18,27,0.7) 100%)"
          : "linear-gradient(180deg, rgba(246,248,251,0.98) 0%, rgba(246,248,251,0.92) 72px, rgba(246,248,251,0.82) 100%)",
      borderRight: isDark
        ? "1px solid rgba(148, 163, 184, 0.12)"
        : "1px solid rgba(15, 23, 42, 0.06)",
      pointerEvents: "none",
      zIndex: 1,
    },
    main: {
      display: "flex",
      flex: 1,
      overflow: "hidden",
      position: "relative",
      zIndex: 2,
    },
    sidebar: {
      width: 50,
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
      paddingTop: desktopEnabled && desktopPlatform === "macos" ? 10 : 8,
      flexShrink: 0,
      backdropFilter: desktopEnabled ? "blur(18px) saturate(1.08)" : undefined,
    },
    menuItem: {
      width: 50,
      height: 64,
      display: "flex",
      flexDirection: "column",
      alignItems: "center",
      justifyContent: "center",
      cursor: "pointer",
      fontSize: 18,
      color: token.colorTextSecondary,
      position: "relative",
      transition: "all 0.2s",
    },
    menuItemLabel: {
      marginTop: 4,
      fontSize: 9,
      lineHeight: "9px",
      whiteSpace: "nowrap",
      color: "inherit",
    },
    menuItemActive: {
      color: token.colorPrimary,
      backgroundColor: token.colorPrimaryBg,
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

  const isActive = (key: string) => {
    if (key === "/traffic" && location.pathname === "/") return true;
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
          <div style={styles.macSidebarWash} />
        </>
      ) : null}
      {desktopEnabled ? (
        <>
          <div style={styles.desktopAtmosphere} />
          <div style={styles.desktopNoise} />
        </>
      ) : null}
      <div style={styles.main}>
        <div style={styles.sidebar}>
          {menuItems.filter((item) => !item.hidden).map((item) => {
            const active = isActive(item.key);
            return (
              <div
                style={{
                  ...styles.menuItem,
                  ...(active ? styles.menuItemActive : {}),
                }}
                onClick={() => handleClick(item.key)}
              >
                {active && <div style={styles.activeBorder as CSSProperties} />}
                {renderMenuIcon(item)}
                <div style={styles.menuItemLabel}>{item.label}</div>
              </div>
            );
          })}
          <div
            style={{
              marginTop: "auto",
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
            {isDark ? <SunOutlined /> : <MoonOutlined />}
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
