import { useCallback, useEffect, useMemo, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";
import { useSearchParams } from "react-router-dom";
import { Button, Empty, Grid, Input, Select, Space, Tabs, theme, Typography } from "antd";
import {
  AudioOutlined,
  CloudOutlined,
  HistoryOutlined,
  PlusOutlined,
  RobotOutlined,
  SendOutlined,
  SettingOutlined,
  SoundOutlined,
  VideoCameraOutlined,
} from "@ant-design/icons";
import AgentTab from "../Settings/tabs/AgentTab";
import ImGatewayTab from "../Settings/tabs/ImGatewayTab";
import ASR from "../ASR";
import VideosTool from "./VideosTool";
import { getAsrCapabilities } from "../../api/asr";
import AgentChatSection, {
  type AgentChatSectionHandle,
  type AgentChatSidebarState,
} from "./AgentChatSection";
import { AgentThreadListCard } from "./AgentChatSection.panels";
import {
  buildRunnerOptions,
  resolveAiRouteState,
  selectDefaultRunner,
  type AiMainView,
  type AiSettingsTarget,
} from "./aiLayout";
import { apiFetch } from "../../api/apiFetch";
import type { RunnerConfigPayload, RunnerOption } from "./AgentChatSection.helpers";

const { Text } = Typography;
const { TextArea } = Input;
const { useBreakpoint } = Grid;

export default function AI() {
  const [searchParams, setSearchParams] = useSearchParams();
  const { token } = theme.useToken();
  const screens = useBreakpoint();
  const isCompactNav = !screens.md;
  const [asrEntryEnabled, setAsrEntryEnabled] = useState(false);
  const [sidebarState, setSidebarState] = useState<AgentChatSidebarState | null>(null);
  const [chatControls, setChatControls] = useState<AgentChatSectionHandle | null>(null);
  const [newChatActive, setNewChatActive] = useState(true);
  const [newChatDraft, setNewChatDraft] = useState("");
  const [newChatSubmitting, setNewChatSubmitting] = useState(false);
  const [runnerOptions, setRunnerOptions] = useState<RunnerOption[]>([
    { label: "Bifrost Agent", value: "bifrost_agent", adapter: "bifrost_agent" },
  ]);
  const [selectedRunnerId, setSelectedRunnerId] = useState("bifrost_agent");
  const routeState = useMemo(() => resolveAiRouteState(searchParams), [searchParams]);

  useEffect(() => {
    let alive = true;
    void getAsrCapabilities()
      .then((capabilities) => {
        if (alive) {
          setAsrEntryEnabled(capabilities.qwen3_asr.enabled && !capabilities.qwen3_asr.hidden);
        }
      })
      .catch(() => {
        if (alive) {
          setAsrEntryEnabled(false);
        }
      });
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    void apiFetch("/api/im-gateway/chat/config")
      .then(async (response) => {
        if (!response.ok) {
          return undefined;
        }
        return response.json() as Promise<RunnerConfigPayload>;
      })
      .then((payload) => {
        if (cancelled || !payload) {
          return;
        }
        const options = buildRunnerOptions(payload);
        const defaultRunner = selectDefaultRunner(options).value;
        setRunnerOptions(options);
        setSelectedRunnerId(defaultRunner);
      })
      .catch(() => {
        // Keep the default built-in runner when config is temporarily unavailable.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const setMainView = useCallback(
    (view: AiMainView) => {
      setSearchParams(
        (prev) => {
          prev.set("view", view);
          prev.delete("aiSection");
          if (view === "chat") {
            prev.set("mode", "new");
            prev.delete("session");
            prev.delete("historyPath");
            prev.delete("agentSection");
          }
          if (view !== "im" && view !== "settings") {
            prev.delete("imGatewaySection");
          }
          if (view !== "settings") {
            prev.delete("settings");
          }
          return prev;
        },
        { replace: false },
      );
    },
    [setSearchParams],
  );

  const openNewChat = useCallback(() => {
    chatControls?.openNewChat();
    setMainView("chat");
  }, [chatControls, setMainView]);

  const openSettings = useCallback(
    (target: Exclude<AiSettingsTarget, null> = "agent") => {
      setSearchParams(
        (prev) => {
          prev.set("view", "settings");
          prev.set("settings", target);
          prev.delete("aiSection");
          return prev;
        },
        { replace: false },
      );
    },
    [setSearchParams],
  );

  const submitNewChat = useCallback(async () => {
    const message = newChatDraft.trim();
    if (!message || !chatControls) {
      return;
    }
    setNewChatSubmitting(true);
    try {
      await chatControls.startNewChat(message, selectedRunnerId);
      setNewChatDraft("");
    } finally {
      setNewChatSubmitting(false);
    }
  }, [chatControls, newChatDraft, selectedRunnerId]);

  const handleNewChatKeyDown = useCallback(
    (event: KeyboardEvent<HTMLTextAreaElement>) => {
      if (event.key === "Enter" && !event.shiftKey) {
        event.preventDefault();
        void submitNewChat();
      }
    },
    [submitNewChat],
  );

  const navButton = (
    key: string,
    label: string,
    icon: ReactNode,
    active: boolean,
    onClick: () => void,
    testId: string,
  ) => (
    <button
      key={key}
      type="button"
      data-testid={testId}
      aria-current={active ? "true" : undefined}
      onClick={onClick}
      style={{
        width: isCompactNav ? "auto" : "100%",
        minWidth: isCompactNav ? 92 : undefined,
        minHeight: 32,
        display: "inline-flex",
        alignItems: "center",
        gap: 8,
        border: 0,
        borderRadius: 7,
        background: active ? token.colorFillSecondary : "transparent",
        color: token.colorText,
        cursor: "pointer",
        font: "inherit",
        fontSize: 12,
        fontWeight: active ? 600 : 500,
        lineHeight: "16px",
        padding: "7px 8px",
        textAlign: "left",
        whiteSpace: "nowrap",
      }}
    >
      {icon}
      <span>{label}</span>
    </button>
  );

  const threadStyles = useMemo<Record<string, CSSProperties> | undefined>(() => {
    if (!sidebarState?.styles) return undefined;
    return {
      ...sidebarState.styles,
      threadCard: {
        ...sidebarState.styles.threadCard,
        border: 0,
        boxShadow: "none",
        background: "transparent",
      },
      threadCardBody: {
        ...sidebarState.styles.threadCardBody,
        padding: 0,
      },
      threadLoadMoreBar: {
        ...sidebarState.styles.threadLoadMoreBar,
        padding: "6px 2px 2px",
      },
    };
  }, [sidebarState]);

  const settingsOpen = routeState.view === "settings";
  const settingsActiveKey = routeState.settings || "agent";
  const showNewChatLanding =
    routeState.view === "chat" && routeState.chatMode === "new" && newChatActive;

  return (
    <div
      data-testid="ai-page-layout"
      style={{
        padding: 0,
        height: "100%",
        minHeight: 0,
        overflow: "hidden",
        background: token.colorBgLayout,
      }}
    >
      <div
        style={{
          display: "grid",
          gridTemplateColumns: isCompactNav ? "1fr" : "176px minmax(0, 1fr)",
          gridTemplateRows: isCompactNav ? "auto minmax(0, 1fr)" : undefined,
          gap: 0,
          height: "100%",
          minHeight: 0,
          overflow: "hidden",
        }}
      >
        <nav
          aria-label="AI workspace"
          data-testid="ai-section-nav"
          style={{
            height: isCompactNav ? undefined : "100%",
            zIndex: 1,
            display: "flex",
            flexDirection: isCompactNav ? "row" : "column",
            gap: isCompactNav ? 8 : 10,
            alignItems: isCompactNav ? "center" : "stretch",
            overflowX: isCompactNav ? "auto" : undefined,
            overflowY: isCompactNav ? "hidden" : "auto",
            padding: isCompactNav ? "8px" : "12px 8px",
            minHeight: 0,
            maxHeight: "100%",
            background: token.colorFillQuaternary,
            borderRight: isCompactNav ? undefined : `1px solid ${token.colorBorderSecondary}`,
          }}
        >
          {!isCompactNav ? (
            <Text
              strong
              style={{
                fontSize: 13,
                lineHeight: "18px",
                padding: "0 4px 4px",
                color: token.colorText,
              }}
            >
              Bifrost AI
            </Text>
          ) : null}
          <div
            style={{
              display: "flex",
              flexDirection: isCompactNav ? "row" : "column",
              alignItems: isCompactNav ? "center" : "stretch",
              flex: isCompactNav ? "0 0 auto" : undefined,
              gap: 2,
            }}
          >
            {navButton(
              "new-chat",
              "New Chat",
              <PlusOutlined />,
              routeState.view === "chat" && routeState.chatMode === "new" && newChatActive,
              openNewChat,
              "ai-nav-new-chat",
            )}
            {asrEntryEnabled
              ? navButton(
                  "asr",
                  "ASR",
                  <SoundOutlined />,
                  routeState.view === "asr",
                  () => setMainView("asr"),
                  "ai-nav-tools-asr",
                )
              : null}
            {navButton(
              "im",
              "IM",
              <CloudOutlined />,
              routeState.view === "im",
              () => setMainView("im"),
              "ai-nav-im",
            )}
            {navButton(
              "videos",
              "Videos",
              <VideoCameraOutlined />,
              routeState.view === "videos",
              () => setMainView("videos"),
              "ai-nav-tools-videos",
            )}
          </div>
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              flex: isCompactNav ? "0 0 auto" : "1 1 auto",
              minHeight: 0,
              minWidth: isCompactNav ? 180 : 0,
              gap: 4,
              overflow: "hidden",
            }}
          >
            <Text
              type="secondary"
              style={{
                display: "flex",
                alignItems: "center",
                gap: 6,
                fontSize: 11,
                fontWeight: 600,
                lineHeight: "18px",
                padding: isCompactNav ? "0 2px" : "8px 6px 2px",
                whiteSpace: "nowrap",
              }}
            >
              <HistoryOutlined />
              Threads
            </Text>
            <div style={{ flex: 1, minHeight: 0, overflow: "hidden" }}>
              {sidebarState && threadStyles ? (
                <AgentThreadListCard
                  threads={sidebarState.threads}
                  sessionKey={sidebarState.sessionKey}
                  historyPath={sidebarState.historyPath}
                  view={sidebarState.view}
                  nowSeconds={sidebarState.nowSeconds}
                  styles={threadStyles}
                  onOpenThread={sidebarState.onOpenThread}
                  onDeleteThread={sidebarState.onDeleteThread}
                  compact
                />
              ) : (
                <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="No previous sessions" />
              )}
            </div>
          </div>
          <div style={{ marginTop: isCompactNav ? 0 : "auto" }}>
            {navButton(
              "settings",
              "Settings",
              <SettingOutlined />,
              settingsOpen,
              () => openSettings("agent"),
              "ai-nav-settings",
            )}
          </div>
        </nav>

        <div
          data-testid="ai-section-content"
          style={{
            height: "100%",
            minHeight: 0,
            overflow: "hidden",
            background: token.colorBgLayout,
          }}
        >
          {showNewChatLanding ? (
            <div
              data-testid="ai-new-chat-landing"
              style={{
                height: "100%",
                minHeight: 0,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                padding: screens.md ? 40 : 16,
                boxSizing: "border-box",
              }}
            >
              <div
                style={{
                  width: "min(620px, 100%)",
                  display: "flex",
                  flexDirection: "column",
                  alignItems: "center",
                }}
              >
                <Text
                  strong
                  style={{
                    marginBottom: 20,
                    color: token.colorText,
                    fontSize: screens.md ? 20 : 18,
                    lineHeight: "28px",
                    textAlign: "center",
                  }}
                  data-testid="agent-chat-new-inline-header"
                >
                  How can Bifrost help?
                </Text>
                <div
                  data-testid="agent-chat-new-input-pill"
                  style={{
                    width: "100%",
                    minHeight: 48,
                    display: "flex",
                    alignItems: "center",
                    gap: 6,
                    padding: "0 10px",
                    borderRadius: 999,
                    border: `1px solid ${token.colorBorderSecondary}`,
                    background: token.colorBgElevated,
                    boxShadow: "0 18px 48px rgba(17, 24, 22, 0.10)",
                    boxSizing: "border-box",
                  }}
                >
                  <Button
                    type="text"
                    icon={<PlusOutlined />}
                    aria-label="Attach context"
                    title="Attach context"
                    style={{
                      width: 28,
                      height: 28,
                      minWidth: 28,
                      padding: 0,
                      borderRadius: "50%",
                      color: token.colorTextSecondary,
                    }}
                  />
                  <TextArea
                    data-testid="agent-chat-input"
                    value={newChatDraft}
                    onChange={(event) => setNewChatDraft(event.target.value)}
                    onKeyDown={handleNewChatKeyDown}
                    placeholder="Describe a task for the Agent..."
                    autoSize={{ minRows: 1, maxRows: 4 }}
                    style={{
                      flex: 1,
                      minHeight: 34,
                      padding: "7px 4px",
                      border: "none",
                      boxShadow: "none",
                      outline: "none",
                      background: "transparent",
                      resize: "none",
                      lineHeight: "20px",
                    }}
                  />
                  <Button
                    type="text"
                    shape="circle"
                    icon={<AudioOutlined />}
                    aria-label="Voice input"
                    title="Voice input"
                    style={{
                      width: 28,
                      height: 28,
                      minWidth: 28,
                      color: token.colorTextSecondary,
                    }}
                  />
                  <Button
                    shape="circle"
                    type="primary"
                    icon={<SendOutlined />}
                    aria-label="Send"
                    title="Send"
                    loading={newChatSubmitting}
                    disabled={!newChatDraft.trim() || !chatControls}
                    data-testid="agent-chat-send"
                    onClick={() => void submitNewChat()}
                  />
                </div>
                <Space
                  align="center"
                  size={8}
                  wrap
                  style={{ justifyContent: "center", width: "100%", marginTop: 10 }}
                  data-testid="agent-chat-new-runner-row"
                >
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    Runner
                  </Text>
                  <Select
                    size="small"
                    value={selectedRunnerId}
                    onChange={setSelectedRunnerId}
                    options={runnerOptions}
                    variant="borderless"
                    style={{ minWidth: 160 }}
                    data-testid="agent-chat-inline-runner"
                  />
                </Space>
              </div>
            </div>
          ) : null}
          <div
            style={{
              display: routeState.view === "chat" && !showNewChatLanding ? "block" : "none",
              height: "100%",
            }}
            aria-hidden={showNewChatLanding ? "true" : undefined}
          >
            <AgentChatSection
              embeddedSidebar
              onNewChatStateChange={setNewChatActive}
              onSidebarStateChange={setSidebarState}
              onControlsReady={setChatControls}
            />
          </div>
          {routeState.view === "asr" ? <ASR /> : null}
          {routeState.view === "im" ? <ImGatewayTab hideSectionNav /> : null}
          {routeState.view === "videos" ? <VideosTool /> : null}
          {routeState.view === "settings" ? (
            <div
              data-testid="ai-settings-content"
              style={{
                height: "100%",
                minHeight: 0,
                overflow: "auto",
                padding: screens.md ? 16 : 8,
                boxSizing: "border-box",
              }}
            >
              <Tabs
                activeKey={settingsActiveKey}
                onChange={(key) => openSettings(key as Exclude<AiSettingsTarget, null>)}
                items={[
                  {
                    key: "agent",
                    label: (
                      <Space size={6}>
                        <RobotOutlined />
                        <span>Agent</span>
                      </Space>
                    ),
                    children: <AgentTab hideSectionNav />,
                  },
                  {
                    key: "im",
                    label: (
                      <Space size={6}>
                        <CloudOutlined />
                        <span>IM Gateway</span>
                      </Space>
                    ),
                    children: <ImGatewayTab hideSectionNav />,
                  },
                ]}
              />
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
