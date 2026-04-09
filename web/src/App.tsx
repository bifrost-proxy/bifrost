import React, { useEffect, useState, useCallback, type CSSProperties } from "react";
import { BrowserRouter, HashRouter, Routes, Route, Navigate } from "react-router-dom";
import {
  ConfigProvider,
  Modal,
  Steps,
  App as AntApp,
  message,
  theme,
  Typography,
  Progress,
} from "antd";
import AppLayout from "./components/Layout";
import BifrostFileDropZone from "./components/BifrostFileDropZone";
import PendingAuthModal from "./components/PendingAuthModal";
import Rules from "./pages/Rules";
import Traffic from "./pages/Traffic";
import TrafficDetailPage from "./pages/TrafficDetailPage";
import Replay from "./pages/Replay";
import Settings from "./pages/Settings";
import SyncLogin from "./pages/SyncLogin";
import Values from "./pages/Values";
import Scripts from "./pages/Scripts";
import Groups from "./pages/Groups";
import GroupDetail from "./pages/Groups/GroupDetail";
import {
  checkAndInstallUpdate,
  DESKTOP_HANDOFF_COMPLETE_EVENT,
  DESKTOP_UPDATE_STATUS_EVENT,
  listenDesktopEvent,
  type DesktopUpdateStatusPayload,
} from "./desktop/tauri";
import { useThemeStore, initThemeListener } from "./stores/useThemeStore";
import { useGlobalDataSync } from "./hooks/useGlobalDataSync";
import { useEditorCompletion } from "./hooks/useEditorCompletion";
import { useForceRefreshStore } from "./stores/useForceRefreshStore";
import { useDesktopCoreStore } from "./stores/useDesktopCoreStore";
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

  const [desktopUpdateVisible, setDesktopUpdateVisible] = useState(false);
  const [desktopUpdateStatus, setDesktopUpdateStatus] =
    useState<DesktopUpdateStatusPayload | null>(null);

  useEffect(() => {
    const cleanup = initThemeListener();
    return cleanup;
  }, []);

  useEffect(() => {
    if (!isDesktopShell()) {
      return;
    }

    let cancelled = false;
    let detach: (() => void | Promise<void>) | null = null;
    let closeTimer = 0;

    const scheduleClose = (delayMs: number) => {
      if (closeTimer) {
        window.clearTimeout(closeTimer);
      }
      closeTimer = window.setTimeout(() => {
        setDesktopUpdateVisible(false);
      }, delayMs);
    };

    void listenDesktopEvent(DESKTOP_UPDATE_STATUS_EVENT, (event) => {
      if (cancelled) {
        return;
      }

      const payload = event.payload as DesktopUpdateStatusPayload | null;
      if (!payload || typeof payload !== "object") {
        return;
      }

      setDesktopUpdateStatus(payload);
      setDesktopUpdateVisible(true);

      if (payload.phase === "up-to-date" || payload.phase === "done") {
        scheduleClose(1400);
      }
    })
      .then((unlisten) => {
        if (cancelled) {
          void unlisten();
          return;
        }
        detach = unlisten;
      })
      .catch((error) => {
        console.error("[desktop-update] Failed to subscribe update status event.", error);
      });

    void checkAndInstallUpdate().catch((error) => {
      console.error("[desktop-update] Failed to start update check.", error);
    });

    return () => {
      cancelled = true;
      if (closeTimer) {
        window.clearTimeout(closeTimer);
      }
      if (detach) {
        void detach();
      }
    };
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
        open={desktopUpdateVisible}
        title="应用更新"
        closable={
          desktopUpdateStatus?.phase === "error" ||
          desktopUpdateStatus?.phase === "up-to-date" ||
          desktopUpdateStatus?.phase === "done"
        }
        maskClosable={
          desktopUpdateStatus?.phase === "error" ||
          desktopUpdateStatus?.phase === "up-to-date" ||
          desktopUpdateStatus?.phase === "done"
        }
        keyboard={
          desktopUpdateStatus?.phase === "error" ||
          desktopUpdateStatus?.phase === "up-to-date" ||
          desktopUpdateStatus?.phase === "done"
        }
        okText={desktopUpdateStatus?.phase === "error" ? "关闭" : undefined}
        cancelButtonProps={{ style: { display: "none" } }}
        onOk={() => setDesktopUpdateVisible(false)}
        onCancel={() => setDesktopUpdateVisible(false)}
        footer={
          desktopUpdateStatus?.phase === "error" ||
          desktopUpdateStatus?.phase === "up-to-date" ||
          desktopUpdateStatus?.phase === "done"
            ? undefined
            : null
        }
        centered
        width={Math.min(680, Math.max(520, Math.floor(window.innerWidth * 0.44)))}
        zIndex={1000}
        styles={overlayStyles}
      >
        <Typography.Paragraph>
          {desktopUpdateStatus?.message || "正在检查更新"}
        </Typography.Paragraph>
        <Steps
          size="small"
          current={
            desktopUpdateStatus?.phase === "installing" || desktopUpdateStatus?.phase === "done"
              ? 2
              : desktopUpdateStatus?.phase === "update-available" ||
                  desktopUpdateStatus?.phase === "downloading" ||
                  desktopUpdateStatus?.phase === "downloaded"
                ? 1
                : 0
          }
          status={
            desktopUpdateStatus?.phase === "error"
              ? "error"
              : desktopUpdateStatus?.phase === "up-to-date" ||
                  desktopUpdateStatus?.phase === "done"
                ? "finish"
                : "process"
          }
          items={[
            { title: "检查更新" },
            { title: "下载" },
            { title: "安装" },
          ]}
        />
        {desktopUpdateStatus?.progress ? (
          <div style={{ marginTop: 16 }}>
            <Progress
              percent={desktopUpdateStatus.progress.percent ?? undefined}
              showInfo
            />
          </div>
        ) : null}
        {desktopUpdateStatus?.downloadUrl ? (
          <Typography.Paragraph type="secondary" style={{ marginTop: 12, marginBottom: 0 }}>
            <Typography.Text code>{desktopUpdateStatus.downloadUrl}</Typography.Text>
          </Typography.Paragraph>
        ) : null}
      </Modal>
      <Modal
        open={forceRefreshVisible}
        title="页面已被断开"
        closable={false}
        maskClosable={false}
        keyboard={false}
        okText="刷新页面"
        cancelButtonProps={{ style: { display: "none" } }}
        onOk={() => {
          window.location.reload();
        }}
      >
        <Typography.Paragraph>
          由于打开页面过多，当前页面的连接已被服务端关闭。
        </Typography.Paragraph>
        {forceRefreshReason ? (
          <Typography.Paragraph type="secondary">
            原因：{forceRefreshReason}
          </Typography.Paragraph>
        ) : null}
        <Typography.Paragraph type="secondary">
          请刷新页面后继续使用。
        </Typography.Paragraph>
      </Modal>
      {isDesktopShell() ? (
        <HashRouter>
          <BifrostFileDropZone>
            <PendingAuthModal />
            <Routes>
              <Route path="/sync-login" element={<SyncLogin />} />
              <Route path="/traffic/detail" element={<TrafficDetailPage />} />
              <Route path="/" element={<AppLayout />}>
                <Route index element={<Navigate to="/traffic" replace />} />
                <Route path="traffic" element={<Traffic />} />
                <Route path="replay" element={<Replay />} />
                <Route path="rules" element={<Rules />} />
                <Route path="values" element={<Values />} />
                <Route path="scripts" element={<Scripts />} />
                <Route path="groups" element={<Groups />} />
                <Route path="groups/:id" element={<GroupDetail />} />
                <Route path="settings" element={<Settings />} />
              </Route>
            </Routes>
          </BifrostFileDropZone>
        </HashRouter>
      ) : (
        <BrowserRouter basename={getAdminPrefix()}>
          <BifrostFileDropZone>
            <PendingAuthModal />
            <Routes>
              <Route path="/sync-login" element={<SyncLogin />} />
              <Route path="/traffic/detail" element={<TrafficDetailPage />} />
              <Route path="/" element={<AppLayout />}>
                <Route index element={<Navigate to="/traffic" replace />} />
                <Route path="traffic" element={<Traffic />} />
                <Route path="replay" element={<Replay />} />
                <Route path="rules" element={<Rules />} />
                <Route path="values" element={<Values />} />
                <Route path="scripts" element={<Scripts />} />
                <Route path="groups" element={<Groups />} />
                <Route path="groups/:id" element={<GroupDetail />} />
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
