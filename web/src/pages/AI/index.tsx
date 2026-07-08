import { useCallback, useEffect, useMemo, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";
import { useSearchParams } from "react-router-dom";
import { Button, Empty, Grid, Input, Select, Space, Tabs, theme, Typography } from "antd";
import {
  AudioOutlined,
  CloudOutlined,
  HistoryOutlined,
  PlusOutlined,
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
import {
  type AgentSectionId,
  type ImGatewaySectionId,
} from "../Settings/tabs/aiSections";

const { Text } = Typography;
const { TextArea } = Input;
const { useBreakpoint } = Grid;

const AI_SETTINGS_AGENT_SECTIONS: AgentSectionId[] = [
  "general",
  "model",
  "runtime",
  "history",
  "memories",
  "skills",
  "memory-records",
  "mcp-servers",
  "sessions",
];
const AI_SETTINGS_RUNNER_SECTIONS: AgentSectionId[] = ["runners"];
const AI_SETTINGS_IM_SECTIONS: ImGatewaySectionId[] = [
  "targets",
  "routes",
  "schedules",
  "history",
];
const AI_CONTENT_MAX_WIDTH = 1120;
const AI_WORKBENCH_CONTENT_MAX_WIDTH = 920;

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
          prev.delete("settings");
          prev.delete("agentSection");
          if (view === "chat") {
            prev.set("mode", "new");
            prev.delete("session");
            prev.delete("historyPath");
          }
          if (view !== "im") {
            prev.delete("imGatewaySection");
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
          prev.delete("mode");
          prev.delete("session");
          prev.delete("historyPath");
          if (target === "agent" && (!prev.get("agentSection") || prev.get("agentSection") === "chat")) {
            prev.set("agentSection", "general");
            prev.delete("imGatewaySection");
          }
          if (target === "im" && (!prev.get("imGatewaySection") || prev.get("imGatewaySection") === "connections")) {
            prev.set("imGatewaySection", "targets");
            prev.delete("agentSection");
          }
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
      threadVirtualRow: {
        ...sidebarState.styles.threadVirtualRow,
        paddingBottom: 2,
      },
    };
  }, [sidebarState]);

  const settingsOpen = routeState.view === "settings";
  const settingsActiveKey =
    routeState.settings === "im"
      ? "im"
      : routeState.agentSection === "runners"
      ? "runner"
      : "agent";
  const showNewChatLanding =
    routeState.view === "chat" && routeState.chatMode === "new" && newChatActive;
  const contentPadding = screens.md ? 24 : 16;
  const contentTrackStyle: CSSProperties = {
    height: "100%",
    minHeight: 0,
    overflow: "auto",
    padding: `${contentPadding}px ${contentPadding}px ${screens.md ? 24 : 16}px`,
    boxSizing: "border-box",
  };
  const contentTrackInnerStyle: CSSProperties = {
    width: "100%",
    maxWidth: AI_CONTENT_MAX_WIDTH,
    margin: "0 auto",
  };
  const workbenchTrackInnerStyle: CSSProperties = {
    ...contentTrackInnerStyle,
    maxWidth: AI_WORKBENCH_CONTENT_MAX_WIDTH,
  };

  useEffect(() => {
    if (routeState.view !== "settings") {
      return;
    }
    const hasConversationState =
      searchParams.has("mode") ||
      searchParams.has("session") ||
      searchParams.has("historyPath") ||
      searchParams.get("agentSection") === "chat" ||
      searchParams.get("settings") === "chat" ||
      searchParams.has("aiSection");
    const hasDeprecatedSettingsImConnections =
      searchParams.get("settings") === "im" && searchParams.get("imGatewaySection") === "connections";
    if (!hasConversationState && !hasDeprecatedSettingsImConnections) {
      return;
    }
    setSearchParams(
      (prev) => {
        prev.set("view", "settings");
        prev.delete("mode");
        prev.delete("session");
        prev.delete("historyPath");
        prev.delete("aiSection");
        if (prev.get("settings") === "chat") {
          prev.set("settings", "agent");
        }
        if (!prev.get("settings")) {
          prev.set("settings", routeState.settings === "im" ? "im" : "agent");
        }
        if (prev.get("settings") === "agent" && (!prev.get("agentSection") || prev.get("agentSection") === "chat")) {
          prev.set("agentSection", "general");
        }
        if (
          prev.get("settings") === "im" &&
          (!prev.get("imGatewaySection") || prev.get("imGatewaySection") === "connections")
        ) {
          prev.set("imGatewaySection", "targets");
        }
        return prev;
      },
      { replace: true },
    );
  }, [routeState.settings, routeState.view, searchParams, setSearchParams]);

  const handleSettingsTabChange = useCallback(
    (key: string) => {
      setSearchParams(
        (prev) => {
          prev.set("view", "settings");
          prev.delete("aiSection");
          prev.delete("mode");
          prev.delete("session");
          prev.delete("historyPath");
          if (key === "im") {
            prev.set("settings", "im");
            prev.set(
              "imGatewaySection",
              !prev.get("imGatewaySection") || prev.get("imGatewaySection") === "connections"
                ? "targets"
                : prev.get("imGatewaySection")!,
            );
            prev.delete("agentSection");
          } else if (key === "runner") {
            prev.set("settings", "agent");
            prev.set("agentSection", "runners");
            prev.delete("imGatewaySection");
          } else {
            prev.set("settings", "agent");
            if (!prev.get("agentSection") || prev.get("agentSection") === "chat" || prev.get("agentSection") === "runners") {
              prev.set("agentSection", "general");
            }
            prev.delete("imGatewaySection");
          }
          return prev;
        },
        { replace: false },
      );
    },
    [setSearchParams],
  );

  const settingsTabItems = useMemo(
    () => [
      { key: "agent", label: "Agent" },
      { key: "runner", label: "Runner" },
      { key: "im", label: "IM" },
    ],
    [],
  );

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
          gridTemplateColumns: isCompactNav ? "1fr" : "216px minmax(0, 1fr)",
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
            padding: isCompactNav ? "8px" : "12px 10px",
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
                    minHeight: 118,
                    display: "flex",
                    flexDirection: "column",
                    gap: 8,
                    padding: "12px 12px 10px",
                    borderRadius: 18,
                    border: `1px solid ${token.colorBorderSecondary}`,
                    background: token.colorBgElevated,
                    boxShadow: "0 18px 48px rgba(17, 24, 22, 0.10)",
                    boxSizing: "border-box",
                  }}
                >
                  <TextArea
                    data-testid="agent-chat-input"
                    value={newChatDraft}
                    onChange={(event) => setNewChatDraft(event.target.value)}
                    onKeyDown={handleNewChatKeyDown}
                    placeholder="Describe a task for the Agent..."
                    autoSize={{ minRows: 2, maxRows: 5 }}
                    style={{
                      width: "100%",
                      minHeight: 52,
                      padding: "2px 4px",
                      border: "none",
                      boxShadow: "none",
                      outline: "none",
                      background: "transparent",
                      resize: "none",
                      lineHeight: "22px",
                    }}
                  />
                  <div
                    data-testid="agent-chat-new-toolbar"
                    style={{
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "space-between",
                      gap: 8,
                      minHeight: 32,
                    }}
                  >
                    <Space size={6} align="center" style={{ minWidth: 0 }}>
                      <Button
                        type="text"
                        icon={<PlusOutlined />}
                        aria-label="Attach context"
                        title="Attach context"
                        style={{
                          width: 30,
                          height: 30,
                          minWidth: 30,
                          padding: 0,
                          borderRadius: "50%",
                          color: token.colorTextSecondary,
                        }}
                      />
                      <div
                        data-testid="agent-chat-new-runner-row"
                        style={{
                          height: 30,
                          display: "inline-flex",
                          alignItems: "center",
                          borderRadius: 999,
                          background: token.colorFillSecondary,
                          padding: "0 6px 0 10px",
                          maxWidth: screens.md ? 240 : 176,
                        }}
                      >
                        <Text
                          type="secondary"
                          style={{
                            flex: "0 0 auto",
                            fontSize: 12,
                            lineHeight: "18px",
                            marginRight: 2,
                          }}
                        >
                          Runner
                        </Text>
                        <Select
                          size="small"
                          value={selectedRunnerId}
                          onChange={setSelectedRunnerId}
                          options={runnerOptions}
                          variant="borderless"
                          style={{ minWidth: 0, maxWidth: screens.md ? 160 : 104 }}
                          popupMatchSelectWidth={false}
                          data-testid="agent-chat-inline-runner"
                        />
                      </div>
                    </Space>
                    <Space size={6} align="center">
                      <Button
                        type="text"
                        shape="circle"
                        icon={<AudioOutlined />}
                        aria-label="Voice input"
                        title="Voice input"
                        style={{
                          width: 30,
                          height: 30,
                          minWidth: 30,
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
                        style={{
                          width: 30,
                          height: 30,
                          minWidth: 30,
                        }}
                      />
                    </Space>
                  </div>
                </div>
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
          {routeState.view === "asr" ? (
            <div data-testid="ai-asr-content" style={contentTrackStyle}>
              <div data-testid="ai-asr-track" style={workbenchTrackInnerStyle}>
                <ASR />
              </div>
            </div>
          ) : null}
          {routeState.view === "im" ? (
            <div data-testid="ai-im-content" style={contentTrackStyle}>
              <div data-testid="ai-im-track" style={workbenchTrackInnerStyle}>
                <ImGatewayTab hideSectionNav cardGrid />
              </div>
            </div>
          ) : null}
          {routeState.view === "videos" ? (
            <div data-testid="ai-videos-content" style={contentTrackStyle}>
              <div data-testid="ai-videos-track" style={workbenchTrackInnerStyle}>
                <VideosTool embedded />
              </div>
            </div>
          ) : null}
          {routeState.view === "settings" ? (
            <div
              data-testid="ai-settings-content"
              style={contentTrackStyle}
            >
              <div
                data-testid="ai-settings-track"
                style={contentTrackInnerStyle}
              >
                <Tabs
                  activeKey={settingsActiveKey}
                  onChange={handleSettingsTabChange}
                  items={settingsTabItems}
                  tabBarGutter={10}
                />
                <div data-testid="ai-settings-active-panel">
                  {routeState.settings === "im" ? (
                    <ImGatewayTab hideSectionNav visibleSections={AI_SETTINGS_IM_SECTIONS} cardGrid />
                  ) : settingsActiveKey === "runner" ? (
                    <AgentTab hideSectionNav visibleSections={AI_SETTINGS_RUNNER_SECTIONS} />
                  ) : (
                    <AgentTab hideSectionNav visibleSections={AI_SETTINGS_AGENT_SECTIONS} />
                  )}
                </div>
              </div>
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
