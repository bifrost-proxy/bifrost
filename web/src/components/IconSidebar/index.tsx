import { useNavigate, useLocation } from "react-router-dom";
import { Tooltip, theme } from "antd";
import {
  GlobalOutlined,
  FileTextOutlined,
  TeamOutlined,
  SettingOutlined,
  CodeOutlined,
  ThunderboltOutlined,
  SunOutlined,
  MoonOutlined,
  RobotOutlined,
  UsergroupAddOutlined,
  BellOutlined,
  BugOutlined,
} from "@ant-design/icons";
import { useEffect, type CSSProperties } from "react";
import { useThemeStore } from "../../stores/useThemeStore";
import { useSyncStore } from "../../stores/useSyncStore";

interface MenuItem {
  key: string;
  icon: React.ReactNode;
  label: string;
  hidden?: boolean;
}

export default function IconSidebar() {
  const navigate = useNavigate();
  const location = useLocation();
  const { token } = theme.useToken();
  const resolvedTheme = useThemeStore((state) => state.resolvedTheme);
  const setThemeMode = useThemeStore((state) => state.setMode);
  const isDark = resolvedTheme === "dark";
  const syncStatus = useSyncStore((state) => state.syncStatus);
  const startSyncPolling = useSyncStore((state) => state.startPolling);
  const stopSyncPolling = useSyncStore((state) => state.stopPolling);
  const showGroups = syncStatus?.enabled ?? false;

  useEffect(() => {
    startSyncPolling();
    return () => {
      stopSyncPolling();
    };
  }, [startSyncPolling, stopSyncPolling]);

  const menuItems: MenuItem[] = [
    { key: "/traffic", icon: <GlobalOutlined />, label: "Network" },
    { key: "/replay", icon: <ThunderboltOutlined />, label: "Replay" },
    { key: "/rules", icon: <FileTextOutlined />, label: "Rules" },
    { key: "/scripts", icon: <CodeOutlined />, label: "Scripts" },
    { key: "/ai", icon: <RobotOutlined />, label: "AI" },
    { key: "/devtools", icon: <BugOutlined />, label: "DevTools" },
    { key: "/values", icon: <TeamOutlined />, label: "Values" },
    { key: "/groups", icon: <UsergroupAddOutlined />, label: "Groups", hidden: !showGroups },
    { key: "/notifications", icon: <BellOutlined />, label: "Notifications" },
    { key: "/settings", icon: <SettingOutlined />, label: "Settings" },
  ];

  const handleThemeToggle = () => {
    setThemeMode(isDark ? "light" : "dark");
  };

  const styles: Record<string, CSSProperties> = {
    sidebar: {
      width: 48,
      height: "100vh",
      backgroundColor: token.colorBgContainer,
      borderRight: `1px solid ${token.colorBorderSecondary}`,
      display: "flex",
      flexDirection: "column",
      alignItems: "center",
      paddingTop: 8,
    },
    menuItem: {
      width: 48,
      height: 48,
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      cursor: "pointer",
      fontSize: 18,
      color: token.colorTextSecondary,
      position: "relative",
      transition: "all 0.2s",
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
  };

  const handleClick = (key: string) => {
    navigate(key);
  };

  return (
    <div style={styles.sidebar}>
      {menuItems.filter((item) => !item.hidden).map((item) => {
        const isActive = location.pathname === item.key || location.pathname.startsWith(item.key + "/");
        return (
          <Tooltip key={item.key} title={item.label} placement="right">
            <div
              style={{
                ...styles.menuItem,
                ...(isActive ? styles.menuItemActive : {}),
              }}
              onClick={() => handleClick(item.key)}
            >
              {isActive && <div style={styles.activeBorder as CSSProperties} />}
              {item.icon}
            </div>
          </Tooltip>
        );
      })}
      <Tooltip title={isDark ? "Switch to Light" : "Switch to Dark"} placement="right">
        <div
          data-testid="theme-toggle"
          style={{
            marginTop: "auto",
            marginBottom: 8,
            width: 34,
            height: 34,
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
      </Tooltip>
    </div>
  );
}
