import { useEffect, useState, useCallback, type CSSProperties } from "react";
import { BrowserRouter, HashRouter, Routes, Route, Navigate } from "react-router-dom";
import { ConfigProvider, Modal, Steps, App as AntApp, message, theme, Typography } from "antd";
import AppLayout from "./components/Layout";
import BifrostFileDropZone from "./components/BifrostFileDropZone";
import PendingAuthModal from "./components/PendingAuthModal";
import PendingIpTlsModal from "./components/PendingIpTlsModal";
import Rules from "./pages/Rules";
import Traffic from "./pages/Traffic";
import TrafficDetailPage from "./pages/TrafficDetailPage";
import Replay from "./pages/Replay";
import Settings from "./pages/Settings";
import SyncLogin from "./pages/SyncLogin";
import Login from "./pages/Login";
import Values from "./pages/Values";
import Scripts from "./pages/Scripts";
import Groups from "./pages/Groups";
import GroupDetail from "./pages/Groups/GroupDetail";
import Notifications from "./pages/Notifications";
import {
  DESKTOP_HANDOFF_COMPLETE_EVENT,
  listenDesktopEvent,
} from "./desktop/tauri";
import { useThemeStore, initThemeListener } from "./stores/useThemeStore";
import { useGlobalDataSync } from "./hooks/useGlobalDataSync";
import { useEditorCompletion } from "./hooks/useEditorCompletion";
import { useForceRefreshStore } from "./stores/useForceRefreshStore";
import { useDesktopCoreStore } from "./stores/useDesktopCoreStore";
import AdminAuthGate from "./components/AdminAuthGate";
import { initDesktopEditEventListener } from "./components/MonacoDesktopCommands";
import {
  getDesktopPlatform,
  getAdminPrefix,
  initializeDesktopRuntime,
  isDesktopShell,
} from "./runtime";

export default function App() {
  const [desktopPlatform, setDesktopPlatform] = useState(getDesktopPlatform());

  useEffect(() => {
    void initializeDesktopRuntime().finally(() => {
      setDesktopPlatform(getDesktopPlatform());
    });
    if (isDesktopShell()) {
      initDesktopEditEventListener();
    }
  }, []);

  return <AppShell desktopPlatform={desktopPlatform} />;
}

function AppShell({ desktopPlatform }: { desktopPlatform: ReturnType<typeof getDesktopPlatform> }) {
  const resolvedTheme = useThemeStore((state) => state.resolvedTheme);
  const forceRefreshVisible = useForceRefreshStore((s) => s.visible);
  const forceRefreshReason = useForceRefreshStore((s) => s.reason);
  const desktopCoreVisible = useDesktopCoreStore((state) => state.visible);
  const desktopCorePhase = useDesktopCoreStore((state) => state.phase);
  const desktopCoreTargetPort = useDesktopCoreStore((state) => state.targetPort);
  const desktopCoreDetail = useDesktopCoreStore((state) => state.detail);
  const hideDesktopCore = useDesktopCoreStore((state) => state.hide);

  useGlobalDataSync();
  useEditorCompletion();

  useEffect(() => {
    const cleanup = initThemeListener();
    return cleanup;
  }, []);

  const holderRender = useCallback(
    (children: React.ReactNode) => (
      <ConfigProvider
        theme={{
          algorithm:
            resolvedTheme === "dark"
              ? theme.darkAlgorithm
              : theme.defaultAlgorithm,
          token: {
            colorPrimary: "#1677ff",
            borderRadius: 6,
          },
        }}
      >
        <AntApp>{children}</AntApp>
      </ConfigProvider>
    ),
    [resolvedTheme],
  );

  useEffect(() => {
    ConfigProvider.config({ holderRender });
  }, [holderRender]);

  useEffect(() => {
    message.config({ maxCount: 1, top: 24 });
  }, []);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", resolvedTheme);
  }, [resolvedTheme]);

  useEffect(() => {
    document.documentElement.setAttribute(
      "data-platform",
      isDesktopShell() ? "desktop" : "web",
    );
  }, []);

  const overlayStyles =
    resolvedTheme === "dark"
      ? {
          mask: {
            background: "rgba(4, 8, 14, 0.52)",
            backdropFilter: "blur(20px) saturate(1.08)",
          },
          container: {
            background: "rgba(14, 19, 31, 0.76)",
            backdropFilter: "blur(24px) saturate(1.06)",
            border: "1px solid rgba(148, 163, 184, 0.14)",
            boxShadow: "0 30px 96px rgba(0, 0, 0, 0.5)",
          },
          header: {
            background: "transparent",
            borderBottom: "1px solid rgba(148, 163, 184, 0.08)",
          },
          body: {
            paddingTop: 8,
          },
        }
      : {
          mask: {
            background: "rgba(242, 246, 252, 0.26)",
            backdropFilter: "blur(18px) saturate(1.1)",
          },
          container: {
            background: "rgba(255, 255, 255, 0.74)",
            backdropFilter: "blur(22px) saturate(1.12)",
            border: "1px solid rgba(255, 255, 255, 0.28)",
            boxShadow: "0 24px 80px rgba(15, 23, 42, 0.12)",
          },
          header: {
            background: "transparent",
            borderBottom: "none",
          },
          body: {
            paddingTop: 4,
          },
        };

  return (
    <ConfigProvider
      theme={{
        algorithm:
          resolvedTheme === "dark"
            ? theme.darkAlgorithm
            : theme.defaultAlgorithm,
        token: {
          colorPrimary: "#1677ff",
          borderRadius: 6,
        },
      }}
    >
      <AntApp>
      {isDesktopShell() && desktopPlatform === "macos" ? (
        <DesktopTransitionMask resolvedTheme={resolvedTheme} />
      ) : null}
      <Modal
        open={desktopCoreVisible}
        title={
          desktopCorePhase === "error"
            ? "Bifrost Core Error"
            : desktopCorePhase === "booting"
              ? "Connecting to Bifrost Core"
              : "Switching Bifrost Port"
        }
        closable={desktopCorePhase === "error"}
        maskClosable={desktopCorePhase === "error"}
        keyboard={desktopCorePhase === "error"}
        okText={desktopCorePhase === "error" ? "Close" : undefined}
        cancelButtonProps={{ style: { display: "none" } }}
        onOk={hideDesktopCore}
        onCancel={hideDesktopCore}
        footer={desktopCorePhase === "error" ? undefined : null}
        centered
        width={Math.min(720, Math.max(560, Math.floor(window.innerWidth * 0.42)))}
        zIndex={1000}
        styles={overlayStyles}
      >
        <Typography.Paragraph>
          {desktopCorePhase === "booting"
            ? "The interface is waiting for the Bifrost core to become available."
            : desktopCoreTargetPort
              ? `Bifrost is switching the local core to port ${desktopCoreTargetPort}.`
              : "Bifrost is updating the local core listener and reconnecting the interface."}
        </Typography.Paragraph>
        <Steps
          size="small"
          style={
            resolvedTheme === "dark"
              ? ({
                  ["--ant-color-text" as string]: "rgba(241, 245, 249, 0.92)",
                  ["--ant-color-text-description" as string]:
                    "rgba(148, 163, 184, 0.92)",
                  ["--ant-color-primary" as string]: "#7dd3fc",
                  ["--ant-color-split" as string]:
                    "rgba(148, 163, 184, 0.16)",
                } as CSSProperties)
              : undefined
          }
          current={
            desktopCorePhase === "booting"
              ? 0
              : desktopCorePhase === "saving"
              ? 0
              : desktopCorePhase === "restarting"
                ? 1
                : desktopCorePhase === "reconnecting"
                  ? 2
                  : 1
          }
          status={desktopCorePhase === "error" ? "error" : "process"}
          items={[
            {
              title:
                desktopCorePhase === "booting" ? "Wait for Core" : "Save Config",
            },
            { title: "Rebind Port" },
            { title: "Reconnect UI" },
          ]}
        />
        <Typography.Paragraph
          type={desktopCorePhase === "error" ? "danger" : "secondary"}
          style={{ marginTop: 16, marginBottom: 0 }}
        >
          {desktopCoreDetail}
        </Typography.Paragraph>
      </Modal>
      <Modal
        open={forceRefreshVisible}
        title="Page Disconnected"
        closable={false}
        maskClosable={false}
        keyboard={false}
        okText="Refresh Page"
        cancelButtonProps={{ style: { display: "none" } }}
        onOk={() => {
          window.location.reload();
        }}
      >
        <Typography.Paragraph>
          The connection has been closed by the server due to too many open pages.
        </Typography.Paragraph>
        {forceRefreshReason ? (
          <Typography.Paragraph type="secondary">
            Reason: {forceRefreshReason}
          </Typography.Paragraph>
        ) : null}
        <Typography.Paragraph type="secondary">
          Please refresh the page to continue.
        </Typography.Paragraph>
      </Modal>
      {isDesktopShell() ? (
        <HashRouter>
          <BifrostFileDropZone>
            <PendingAuthModal />
            <PendingIpTlsModal />
            <Routes>
              <Route path="/login" element={<Login />} />
              <Route path="/sync-login" element={<SyncLogin />} />
              <Route path="/traffic/detail" element={<TrafficDetailPage />} />
              <Route
                path="/"
                element={
                  <AdminAuthGate>
                    <AppLayout />
                  </AdminAuthGate>
                }
              >
                <Route index element={<Navigate to="/traffic" replace />} />
                <Route path="traffic" element={<Traffic />} />
                <Route path="replay" element={<Replay />} />
                <Route path="rules" element={<Rules />} />
                <Route path="values" element={<Values />} />
                <Route path="scripts" element={<Scripts />} />
                <Route path="groups" element={<Groups />} />
                <Route path="groups/:id" element={<GroupDetail />} />
                <Route path="notifications" element={<Notifications />} />
                <Route path="settings" element={<Settings />} />
              </Route>
            </Routes>
          </BifrostFileDropZone>
        </HashRouter>
      ) : (
        <BrowserRouter basename={getAdminPrefix()}>
          <BifrostFileDropZone>
            <PendingAuthModal />
            <PendingIpTlsModal />
            <Routes>
              <Route path="/login" element={<Login />} />
              <Route path="/sync-login" element={<SyncLogin />} />
              <Route path="/traffic/detail" element={<TrafficDetailPage />} />
              <Route
                path="/"
                element={
                  <AdminAuthGate>
                    <AppLayout />
                  </AdminAuthGate>
                }
              >
                <Route index element={<Navigate to="/traffic" replace />} />
                <Route path="traffic" element={<Traffic />} />
                <Route path="replay" element={<Replay />} />
                <Route path="rules" element={<Rules />} />
                <Route path="values" element={<Values />} />
                <Route path="scripts" element={<Scripts />} />
                <Route path="groups" element={<Groups />} />
                <Route path="groups/:id" element={<GroupDetail />} />
                <Route path="notifications" element={<Notifications />} />
                <Route path="settings" element={<Settings />} />
              </Route>
            </Routes>
          </BifrostFileDropZone>
        </BrowserRouter>
      )}
      </AntApp>
    </ConfigProvider>
  );
}

function DesktopTransitionMask({ resolvedTheme }: { resolvedTheme: "light" | "dark" }) {
  const [transitionMaskPhase, setTransitionMaskPhase] = useState<
    "visible" | "exiting" | "hidden"
  >("visible");

  useEffect(() => {
    let cancelled = false;
    let exitTimer = 0;
    let detach: (() => void | Promise<void>) | null = null;

    void listenDesktopEvent(DESKTOP_HANDOFF_COMPLETE_EVENT, () => {
      if (cancelled) {
        return;
      }

      setTransitionMaskPhase("exiting");
      if (exitTimer) {
        window.clearTimeout(exitTimer);
      }
      exitTimer = window.setTimeout(() => {
        setTransitionMaskPhase("hidden");
      }, 220);
    })
      .then((unlisten) => {
        if (cancelled) {
          void unlisten();
          return;
        }
        detach = unlisten;
      })
      .catch((error) => {
        console.error("[desktop-runtime] Failed to subscribe to handoff completion.", error);
        setTransitionMaskPhase("hidden");
      });

    return () => {
      cancelled = true;
      if (exitTimer) {
        window.clearTimeout(exitTimer);
      }
      if (detach) {
        void detach();
      }
    };
  }, []);

  if (transitionMaskPhase === "hidden") {
    return null;
  }

  return (
    <div
      aria-hidden="true"
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 1001,
        pointerEvents: "none",
        opacity: transitionMaskPhase === "exiting" ? 0 : 1,
        transition: "opacity 220ms ease",
        background:
          resolvedTheme === "dark"
            ? "linear-gradient(180deg, rgba(8, 14, 22, 0.90), rgba(8, 14, 22, 0.82))"
            : "linear-gradient(180deg, rgba(239, 244, 250, 0.94), rgba(239, 244, 250, 0.88))",
        backdropFilter: "blur(20px) saturate(1.04)",
      }}
    />
  );
}
