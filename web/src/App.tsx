import { useEffect, useState, useCallback, type CSSProperties } from "react";
import {
  BrowserRouter,
  HashRouter,
  Routes,
  Route,
  Navigate,
  useLocation,
  useNavigate,
} from "react-router-dom";
import { ConfigProvider, Modal, Steps, App as AntApp, message, theme, Typography, Button, Space } from "antd";
import AppLayout from "./components/Layout";
import BifrostFileDropZone from "./components/BifrostFileDropZone";
import { importBifrostFileContent } from "./components/BifrostFileDropZone";
import PendingAuthModal from "./components/PendingAuthModal";
import PendingIpTlsModal from "./components/PendingIpTlsModal";
import PairingApprovalModal from "./components/PairingApprovalModal";
import Rules from "./pages/Rules";
import Traffic from "./pages/Traffic";
import TrafficDetailPage from "./pages/TrafficDetailPage";
import Replay from "./pages/Replay";
import Activity from "./pages/Activity";
import Settings from "./pages/Settings";
import SyncLogin from "./pages/SyncLogin";
import Login from "./pages/Login";
import Values from "./pages/Values";
import Scripts from "./pages/Scripts";
import AI from "./pages/AI";
import Groups from "./pages/Groups";
import GroupDetail from "./pages/Groups/GroupDetail";
import Notifications from "./pages/Notifications";
import DevTools from "./pages/DevTools";
import {
  DESKTOP_HANDOFF_COMPLETE_EVENT,
  DESKTOP_OPEN_REQUEST_EVENT,
  getPendingDesktopOpenRequests,
  getDesktopRuntime,
  listenDesktopEvent,
  openExternalUrl,
  startDesktopCore,
  type DesktopRuntimeInfo,
} from "./desktop/tauri";
import { resolveDesktopOpenTarget } from "./desktop/openTarget";
import { getCliInstallStatus, installCliFromDesktop, type CliInstallStatus } from "./api/system";
import type { DesktopOpenRequest } from "./desktop/tauri";
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
  setDesktopProxyPort,
} from "./runtime";

const CLI_DOCS_URL = "https://bifrost-proxy.github.io/getting-started/desktop";

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
    if (isDesktopShell()) {
      document.documentElement.setAttribute("data-desktop-platform", desktopPlatform);
    } else {
      document.documentElement.removeAttribute("data-desktop-platform");
    }
  }, [desktopPlatform]);

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
        {isDesktopShell() ? (
          <DesktopStartupGate resolvedTheme={resolvedTheme} />
        ) : null}
        {isDesktopShell() && desktopPlatform === "macos" ? (
          <DesktopTransitionMask resolvedTheme={resolvedTheme} />
        ) : null}
        {isDesktopShell() ? <DesktopExternalOpenBridge /> : null}
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
          width={Math.min(
            720,
            Math.max(560, Math.floor(window.innerWidth * 0.42)),
          )}
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
                  desktopCorePhase === "booting"
                    ? "Wait for Core"
                    : "Save Config",
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
              <GlobalRouteEffects />
              <DesktopOpenRequestBridge />
              <PendingAuthModal />
              <PendingIpTlsModal />
              <PairingApprovalModal />
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
                  <Route index element={<Navigate to="/activity" replace />} />
                  <Route path="activity" element={<Activity />} />
                  <Route path="traffic" element={<Traffic />} />
                  <Route path="replay" element={<Replay />} />
                  <Route path="rules" element={<Rules />} />
                  <Route path="values" element={<Values />} />
                  <Route path="scripts" element={<Scripts />} />
                  <Route path="ai" element={<AI />} />
                  <Route path="devtools" element={<DevTools />} />
                  <Route path="devtools/:pageId" element={<DevTools />} />
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
              <GlobalRouteEffects />
              <DesktopOpenRequestBridge />
              <PendingAuthModal />
              <PendingIpTlsModal />
              <PairingApprovalModal />
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
                  <Route index element={<Navigate to="/activity" replace />} />
                  <Route path="activity" element={<Activity />} />
                  <Route path="traffic" element={<Traffic />} />
                  <Route path="replay" element={<Replay />} />
                  <Route path="rules" element={<Rules />} />
                  <Route path="values" element={<Values />} />
                  <Route path="scripts" element={<Scripts />} />
                  <Route path="ai" element={<AI />} />
                  <Route path="devtools" element={<DevTools />} />
                  <Route path="devtools/:pageId" element={<DevTools />} />
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

type CliGatePhase = "checking" | "missing" | "installing" | "installed" | "dismissed" | "error";

function DesktopStartupGate({ resolvedTheme }: { resolvedTheme: "light" | "dark" }) {
  const [runtime, setRuntime] = useState<DesktopRuntimeInfo | null>(null);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const [startingCore, setStartingCore] = useState(false);
  const [cliStatus, setCliStatus] = useState<CliInstallStatus | null>(null);
  const [cliPhase, setCliPhase] = useState<CliGatePhase>("checking");
  const [cliError, setCliError] = useState<string | null>(null);

  const refreshRuntime = useCallback(async () => {
    try {
      const next = await getDesktopRuntime();
      setRuntime(next);
      setRuntimeError(null);
      if (next.startupReady) {
        setDesktopProxyPort(next.proxyPort);
      }
      return next;
    } catch (error) {
      const message = error instanceof Error ? error.message : "Failed to read desktop runtime";
      setRuntimeError(message);
      return null;
    }
  }, []);

  const refreshCliStatus = useCallback(async () => {
    try {
      const status = await getCliInstallStatus();
      setCliStatus(status);
      setCliError(null);
      setCliPhase(status.installed ? "dismissed" : "missing");
    } catch (error) {
      setCliError(error instanceof Error ? error.message : "Failed to check CLI install");
      setCliPhase("error");
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      const next = await refreshRuntime();
      if (cancelled) {
        return;
      }
      if (next?.startupReady && cliPhase === "checking") {
        await refreshCliStatus();
      }
    };
    void tick();
    const timer = window.setInterval(() => {
      void tick();
    }, 3000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [cliPhase, refreshCliStatus, refreshRuntime]);

  const handleStartCore = useCallback(async () => {
    setStartingCore(true);
    try {
      const next = await startDesktopCore();
      setRuntime(next);
      setRuntimeError(null);
      setDesktopProxyPort(next.proxyPort);
      if (next.startupReady) {
        message.success("Bifrost service started");
        if (cliPhase === "checking") {
          await refreshCliStatus();
        }
      }
    } catch (error) {
      setRuntimeError(error instanceof Error ? error.message : "Failed to start Bifrost service");
    } finally {
      setStartingCore(false);
    }
  }, [cliPhase, refreshCliStatus]);

  const handleInstallCli = useCallback(async () => {
    setCliPhase("installing");
    setCliError(null);
    try {
      const status = await installCliFromDesktop({ install_skills: true });
      setCliStatus(status);
      if (status.installed) {
        setCliPhase("installed");
        message.success("CLI installed");
      } else {
        setCliPhase("missing");
      }
    } catch (error) {
      setCliError(error instanceof Error ? error.message : "Failed to install CLI");
      setCliPhase("error");
    }
  }, []);

  const openCliDocs = useCallback(() => {
    window.open(CLI_DOCS_URL, "_blank", "noopener,noreferrer");
  }, []);

  const coreNeedsAttention = Boolean(runtime?.startupError) || Boolean(runtimeError);
  if (coreNeedsAttention) {
    return (
      <DesktopBlockingOverlay
        resolvedTheme={resolvedTheme}
        title="Start Bifrost Service"
        description="Bifrost Desktop needs the local core service before the interface can connect."
        detail={runtime?.startupError || runtimeError || "The service is not running."}
        actions={
          <Button type="primary" size="large" loading={startingCore} onClick={handleStartCore}>
            Start Bifrost Service
          </Button>
        }
      />
    );
  }

  if (cliPhase === "missing" || cliPhase === "installing" || cliPhase === "installed" || cliPhase === "error") {
    return (
      <DesktopBlockingOverlay
        resolvedTheme={resolvedTheme}
        title={cliPhase === "installed" ? "CLI Installed" : "Install Bifrost CLI"}
        description={
          cliPhase === "installed"
            ? "Bifrost CLI is ready for Terminal and AI coding tools."
            : "Install the CLI so Terminal, Codex, Claude Code, Trae, and Cursor can call Bifrost."
        }
        detail={
          cliPhase === "error"
            ? cliError || "CLI install failed"
            : cliPhase === "installed"
              ? cliStatus?.skills_message || "Installation completed successfully."
              : cliStatus?.path_hint || "The CLI is not installed in your user command path yet."
        }
        actions={
          cliPhase === "installed" ? (
            <Space>
              <Button size="large" onClick={openCliDocs}>
                Open Docs
              </Button>
              <Button type="primary" size="large" onClick={() => setCliPhase("dismissed")}>
                Done
              </Button>
            </Space>
          ) : (
            <Space>
              <Button size="large" onClick={() => setCliPhase("dismissed")}>
                Later
              </Button>
              <Button
                type="primary"
                size="large"
                loading={cliPhase === "installing"}
                onClick={handleInstallCli}
              >
                Install CLI
              </Button>
            </Space>
          )
        }
      />
    );
  }

  return null;
}

function DesktopBlockingOverlay({
  resolvedTheme,
  title,
  description,
  detail,
  actions,
}: {
  resolvedTheme: "light" | "dark";
  title: string;
  description: string;
  detail?: string | null;
  actions: React.ReactNode;
}) {
  const dark = resolvedTheme === "dark";
  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 1400,
        display: "grid",
        placeItems: "center",
        padding: 24,
        background: dark ? "rgba(5, 10, 18, 0.86)" : "rgba(244, 248, 252, 0.88)",
        backdropFilter: "blur(18px) saturate(1.08)",
      }}
    >
      <div
        style={{
          width: "min(560px, 100%)",
          padding: 28,
          borderRadius: 8,
          background: dark ? "rgba(15, 23, 42, 0.92)" : "rgba(255, 255, 255, 0.94)",
          border: dark ? "1px solid rgba(148, 163, 184, 0.18)" : "1px solid rgba(15, 23, 42, 0.08)",
          boxShadow: dark ? "0 32px 96px rgba(0, 0, 0, 0.52)" : "0 28px 88px rgba(15, 23, 42, 0.16)",
        }}
      >
        <Typography.Title level={3} style={{ marginTop: 0 }}>
          {title}
        </Typography.Title>
        <Typography.Paragraph style={{ fontSize: 15 }}>
          {description}
        </Typography.Paragraph>
        {detail ? (
          <Typography.Paragraph type="secondary" style={{ marginBottom: 24 }}>
            {detail}
          </Typography.Paragraph>
        ) : null}
        {actions}
      </div>
    </div>
  );
}

function GlobalRouteEffects() {
  const location = useLocation();
  const trafficEnabled =
    location.pathname === "/activity" ||
    location.pathname === "/traffic" ||
    location.pathname === "/traffic/detail";

  useGlobalDataSync({ trafficEnabled });
  useEditorCompletion();

  return null;
}

function DesktopExternalOpenBridge() {
  useEffect(() => {
    if (!isDesktopShell()) {
      return;
    }

    const originalOpen = window.open;
    window.open = ((url?: string | URL, target?: string, features?: string) => {
      const rawUrl = typeof url === "string" ? url : url?.toString();
      const resolved = rawUrl ? resolveDesktopOpenTarget(rawUrl) : null;
      if (resolved) {
        void openExternalUrl(resolved).catch((error) => {
          console.error("[desktop-runtime] Failed to open external URL.", error);
        });
        return null;
      }
      return originalOpen.call(window, url, target, features);
    }) as typeof window.open;

    const handleClick = (event: MouseEvent) => {
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
        return;
      }

      const anchor = (event.target as Element | null)?.closest?.("a[href]") as HTMLAnchorElement | null;
      if (!anchor) {
        return;
      }

      const resolved = resolveDesktopOpenTarget(anchor.getAttribute("href") ?? "");
      if (!resolved) {
        return;
      }

      event.preventDefault();
      void openExternalUrl(resolved).catch((error) => {
        console.error("[desktop-runtime] Failed to open clicked URL.", error);
      });
    };

    document.addEventListener("click", handleClick, true);
    return () => {
      window.open = originalOpen;
      document.removeEventListener("click", handleClick, true);
    };
  }, []);

  return null;
}

function DesktopOpenRequestBridge() {
  const navigate = useNavigate();

  useEffect(() => {
    if (!isDesktopShell()) {
      return;
    }

    let cancelled = false;

    const handleRequest = async (request: DesktopOpenRequest) => {
      if (cancelled) {
        return;
      }

      if (request.kind === "route") {
        navigate(request.route);
        return;
      }

      if (request.kind === "bifrostFile") {
        await importBifrostFileContent(request.content, request.filename, navigate);
      }
    };

    const drainPending = async (fallback?: unknown) => {
      try {
        const pending = await getPendingDesktopOpenRequests();
        const requests =
          pending.length > 0 ? pending : normalizeDesktopOpenPayload(fallback);
        for (const request of requests) {
          await handleRequest(request);
        }
      } catch (error) {
        console.error("[desktop-runtime] Failed to handle desktop open request.", error);
      }
    };

    void drainPending();
    let detach: (() => void | Promise<void>) | null = null;
    void listenDesktopEvent(DESKTOP_OPEN_REQUEST_EVENT, (event) => {
      void drainPending(event.payload);
    })
      .then((unlisten) => {
        if (cancelled) {
          void unlisten();
          return;
        }
        detach = unlisten;
      })
      .catch((error) => {
        console.error("[desktop-runtime] Failed to subscribe to open requests.", error);
      });

    return () => {
      cancelled = true;
      if (detach) {
        void detach();
      }
    };
  }, [navigate]);

  return null;
}

function normalizeDesktopOpenPayload(payload: unknown): DesktopOpenRequest[] {
  if (!payload || typeof payload !== "object") {
    return [];
  }
  const candidate = payload as Partial<DesktopOpenRequest>;
  if (candidate.kind === "route" && typeof candidate.route === "string") {
    return [candidate as DesktopOpenRequest];
  }
  if (
    candidate.kind === "bifrostFile" &&
    typeof candidate.filename === "string" &&
    typeof candidate.content === "string"
  ) {
    return [candidate as DesktopOpenRequest];
  }
  return [];
}

function DesktopTransitionMask({ resolvedTheme }: { resolvedTheme: "light" | "dark" }) {
  const [transitionMaskPhase, setTransitionMaskPhase] = useState<
    "visible" | "exiting" | "hidden"
  >("visible");

  useEffect(() => {
    let cancelled = false;
    let exitTimer = 0;
    let fallbackTimer = 0;
    let detach: (() => void | Promise<void>) | null = null;

    const hideMask = () => {
      if (cancelled) {
        return;
      }

      setTransitionMaskPhase((phase) => {
        if (phase === "hidden" || phase === "exiting") {
          return phase;
        }
        return "exiting";
      });
      if (exitTimer) {
        window.clearTimeout(exitTimer);
      }
      exitTimer = window.setTimeout(() => {
        if (!cancelled) {
          setTransitionMaskPhase("hidden");
        }
      }, 220);
    };

    void getDesktopRuntime()
      .then((runtime) => {
        if (runtime.handoffCompleted) {
          hideMask();
        }
      })
      .catch((error) => {
        console.error("[desktop-runtime] Failed to read handoff snapshot.", error);
      });

    void listenDesktopEvent(DESKTOP_HANDOFF_COMPLETE_EVENT, () => {
      hideMask();
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

    fallbackTimer = window.setTimeout(() => {
      hideMask();
    }, 1500);

    return () => {
      cancelled = true;
      if (exitTimer) {
        window.clearTimeout(exitTimer);
      }
      if (fallbackTimer) {
        window.clearTimeout(fallbackTimer);
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
