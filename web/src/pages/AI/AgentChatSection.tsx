import { useCallback, useEffect, useMemo, useRef, useState, type ClipboardEvent, type CSSProperties, type KeyboardEvent } from "react";
import { useSearchParams } from "react-router-dom";
import { Button, Card, Empty, Grid, Input, Modal, Segmented, Select, Space, Tag, Typography, message as antdMessage, theme } from "antd";
import { DeleteOutlined, DownOutlined, FolderOpenOutlined, BorderOutlined, LeftOutlined, RobotOutlined, SendOutlined, SettingOutlined } from "@ant-design/icons";
import { apiFetch } from "../../api/apiFetch";
import { buildApiUrl } from "../../runtime";
import { getClientId } from "../../services/clientId";
import {
  EMPTY_TELEMETRY,
  PROMPT_CHIPS,
  STARTER_MESSAGES,
  dedupeThreads,
  eventToProcessStep,
  formatCurrentStateTag,
  formatRunnerTag,
  formatThreadSource,
  isRecord,
  isRealChatMessage,
  isSelectedThread,
  numberFrom,
  reduceTelemetry,
  runAgentStream,
  sameChatMessages,
  selectedRunnerAdapter,
  sessionDetailToMessages,
  stringFrom,
  titleFromChatMessages,
  telemetryFromSessionDetail,
  telemetryFromThread,
  type AgentThreadSummary,
  type ChatMessage,
  type HistoryEvent,
  type PendingChatImage,
  type ProcessStep,
  type RunnerConfigPayload,
  type RunnerOption,
  type RunTelemetry,
  type SessionDetail,
} from "./AgentChatSection.helpers";
import {
  appendProcessStepToTimeline,
  historyEventsToMessages,
  mergeHistoryEventWindow,
  historyEventsToTelemetry,
} from "./AgentChatSection.timeline";
import {
  isRunStateActive,
  isRunStateIdle,
  isThreadActive,
} from "./AgentChatSection.timelinePolling";
import { AgentChatMessageList } from "./AgentChatSection.messages";
import { AgentChatPlan, AgentChatPromptChips } from "./AgentChatSection.composerExtras";
import {
  AgentChatImagePreviewStrip,
  MAX_PASTED_IMAGES,
  imageContentParts,
  imageCountLabel,
  imageFilesFromClipboard,
  pendingImageFromFile,
} from "./AgentChatSection.images";
import { AgentChatSettingsModal, AgentThreadListCard } from "./AgentChatSection.panels";
import {
  queueItemsFromEvent,
  queueItemsFromUnknown,
  type QueuedInput,
} from "./AgentChatSection.queue";
import { SelectedRunnerPill, SlashRunnerPanel, useRunnerCallHandler, useSlashRunnerSelection, type SlashCommandOption } from "./AgentChatSection.runnerCall";
import { createAgentChatStyles } from "./AgentChatSection.styles";
import { AgentChatTokenHud } from "./AgentChatSection.tokenHud";
import { buildRunnerOptions, selectDefaultRunner } from "./aiLayout";

const { Text } = Typography;
const { TextArea } = Input;
const { useBreakpoint } = Grid;
const THREAD_RAIL_COLLAPSED_STORAGE_KEY = "bifrost.agentChat.threadRailCollapsed";

type AgentSessionEventPayload = {
  eventType?: string;
  event_type?: string;
  sessionKey?: string;
  session_key?: string;
  historyPath?: string;
  history_path?: string;
  endIndex?: number;
  end_index?: number;
  reason?: string;
};

export type AgentChatSectionHandle = {
  openNewChat: () => void;
  startNewChat: (
    message: string,
    runnerId?: string,
    images?: PendingChatImage[],
  ) => Promise<void>;
};

export type AgentChatSidebarState = {
  threads: AgentThreadSummary[];
  sessionKey: string;
  historyPath?: string;
  view?: string;
  nowSeconds: number;
  styles: Record<string, CSSProperties>;
  onOpenThread: (thread: AgentThreadSummary) => void;
  onDeleteThread: (thread: AgentThreadSummary) => void;
};

type AgentChatSectionProps = {
  embeddedSidebar?: boolean;
  forceNewChat?: boolean;
  onNewChatStateChange?: (active: boolean) => void;
  onSidebarStateChange?: (state: AgentChatSidebarState) => void;
  onControlsReady?: (handle: AgentChatSectionHandle) => void;
};

function explicitRunnerIdentity(
  status: RunTelemetry["status"] | undefined,
  thread: AgentThreadSummary | undefined,
) {
  return [
    status?.source,
    status?.runner_id,
    status?.runner_type,
    status?.agent_type,
    thread?.source,
    thread?.runner_id,
    thread?.runner_type,
    thread?.agent_type,
  ]
    .filter((value): value is string => Boolean(value))
    .join(" ")
    .toLowerCase();
}

export function supportsRunningGuide({
  runnerId,
  runnerOptions,
  selectedThread,
  status,
}: {
  runnerId: string;
  runnerOptions: RunnerOption[];
  selectedThread?: AgentThreadSummary;
  status?: RunTelemetry["status"];
}) {
  const explicit = explicitRunnerIdentity(status, selectedThread);
  if (/\b(chatgpt|webgpt|chatgpt_web)\b/.test(explicit)) {
    return false;
  }
  return selectedRunnerAdapter(runnerOptions, runnerId) !== "chatgpt_web";
}

function proposedPlanMessageContent(content: string) {
  return `**Plan Mode result**\n\n${content.trim()}`;
}

function supportsRunnerModelSlashCommand(runnerAdapter: string) {
  return (
    runnerAdapter === "codex" ||
    runnerAdapter === "traex" ||
    runnerAdapter === "claude_code"
  );
}

function isRunnerModelSlashCommand(content: string, runnerAdapter: string) {
  if (!supportsRunnerModelSlashCommand(runnerAdapter)) {
    return false;
  }
  const trimmed = content.trim();
  return (
    trimmed === "/models" ||
    trimmed === "/model" ||
    trimmed.startsWith("/model ")
  );
}

function runnerModelSlashSystemDisplayContent(command: string, response: string) {
  const trimmed = command.trim();
  const lower = trimmed.toLowerCase();
  if (lower === "/model clear") {
    return "清除模型切换";
  }
  if (
    lower.startsWith("/model ") &&
    response.includes("已将") &&
    response.includes("session 模型设置为")
  ) {
    const model = trimmed.slice("/model ".length).trim();
    if (model) {
      return `切换模型为 ${model}`;
    }
  }
  return response;
}

function resolveRunningState({
  fallbackRunning = false,
  phase,
  state,
  thread,
}: {
  fallbackRunning?: boolean;
  phase?: RunTelemetry["phase"];
  state?: string;
  thread?: AgentThreadSummary;
}) {
  if (isRunStateIdle(state)) {
    return false;
  }
  if (isRunStateActive(state)) {
    return true;
  }
  if (phase === "finished" || phase === "failed") {
    return false;
  }
  return fallbackRunning || phase === "running" || isThreadActive(thread);
}

type HistoryPagePayload = {
  events?: HistoryEvent[];
  count?: number;
  total_count?: number;
  start_index?: number;
  end_index?: number;
  next_cursor?: number | null;
  has_more?: boolean;
};

function historyPageUrl(
  historyPath: string,
  params: { since?: number } = {},
) {
  const query = new URLSearchParams();
  if (params.since !== undefined) {
    query.set("since", String(params.since));
  }
  const suffix = query.toString();
  return `/api/im-gateway/agent/sessions/history/${encodeURIComponent(historyPath)}${
    suffix ? `?${suffix}` : ""
  }`;
}

async function fetchHistoryPage(
  historyPath: string,
  params: { since?: number } = {},
) {
  const response = await apiFetch(historyPageUrl(historyPath, params));
  if (!response.ok) {
    throw new Error(await response.text());
  }
  return response.json() as Promise<HistoryPagePayload>;
}

export default function AgentChatSection({
  embeddedSidebar = false,
  forceNewChat = false,
  onNewChatStateChange,
  onSidebarStateChange,
  onControlsReady,
}: AgentChatSectionProps = {}) {
  const [searchParams, setSearchParams] = useSearchParams();
  const { token } = theme.useToken();
  const screens = useBreakpoint();
  const isCompact = !screens.lg;
  const isNarrow = !screens.md;
  const [draft, setDraft] = useState("");
  const [pendingImages, setPendingImages] = useState<PendingChatImage[]>([]);
  const [messages, setMessages] = useState<ChatMessage[]>(STARTER_MESSAGES);
  const [sessionKey, setSessionKey] = useState(() => `admin-chat-${Date.now()}`);
  const [historyPath, setHistoryPath] = useState<string | undefined>();
  const [historyLoading, setHistoryLoading] = useState(false);
  const [running, setRunning] = useState(false);
  const [supplementSubmitting, setSupplementSubmitting] = useState(false);
  const [runningInputMode, setRunningInputMode] = useState<"guide" | "queue">("guide");
  const [queuedInputs, setQueuedInputs] = useState<QueuedInput[]>([]);
  const [nowSeconds, setNowSeconds] = useState(() => Math.floor(Date.now() / 1000));
  const [threads, setThreads] = useState<AgentThreadSummary[]>([]);
  const [threadRailCollapsed, setThreadRailCollapsed] = useState(() => {
    if (typeof window === "undefined") {
      return false;
    }
    return window.localStorage.getItem(THREAD_RAIL_COLLAPSED_STORAGE_KEY) === "true";
  });
  const [telemetry, setTelemetry] = useState<RunTelemetry>(EMPTY_TELEMETRY);
  const [workDir, setWorkDir] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [newChatOpen, setNewChatOpen] = useState(false);
  const [newChatWorkDir, setNewChatWorkDir] = useState("");
  const [newChatRunnerId, setNewChatRunnerId] = useState("Codex");
  const [runnerId, setRunnerId] = useState("Codex");
  const [defaultRunnerId, setDefaultRunnerId] = useState("Codex");
  const [runnerOptions, setRunnerOptions] = useState<RunnerOption[]>([
    { label: "Codex Runner", value: "Codex", adapter: "codex" },
  ]);
  const [slashActiveIndex, setSlashActiveIndex] = useState(0);
  const slashActiveIndexRef = useRef(0);
  const [defaultWorkDir, setDefaultWorkDir] = useState("");
  const [showScrollToBottom, setShowScrollToBottom] = useState(false);
  const messagesScrollRef = useRef<HTMLDivElement>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const pendingInstantScrollRef = useRef(true);
  const userNearBottomRef = useRef(true);
  const loadedConversationKeyRef = useRef<string | undefined>(undefined);
  const selectedHistoryPathRef = useRef<string | undefined>(undefined);
  const selectedSessionKeyRef = useRef<string | undefined>(undefined);
  const selectedThreadRef = useRef<AgentThreadSummary | undefined>(undefined);
  const telemetryPhaseRef = useRef<RunTelemetry["phase"]>(EMPTY_TELEMETRY.phase);
  const threadsRef = useRef<AgentThreadSummary[]>([]);
  const historyEventsRef = useRef<HistoryEvent[]>([]);
  const historyEventStartIndexRef = useRef<number | undefined>(undefined);
  const historyEventEndIndexRef = useRef<number | undefined>(undefined);
  const initialThreadAutoSelectRef = useRef(false);
  const streamAbortRef = useRef<AbortController | null>(null);

  const scrollMessagesToBottom = useCallback((force = true) => {
    const element = messagesScrollRef.current;
    if (!element) {
      return;
    }
    if (!force && !pendingInstantScrollRef.current && !userNearBottomRef.current) {
      return;
    }
    element.scrollTop = element.scrollHeight;
    userNearBottomRef.current = true;
    setShowScrollToBottom(false);
  }, []);

  const scheduleMessagesBottomScroll = useCallback((force = false) => {
    const scroll = () => scrollMessagesToBottom(force);
    scroll();
    requestAnimationFrame(() => {
      scroll();
      requestAnimationFrame(scroll);
    });
    window.setTimeout(scroll, 80);
    window.setTimeout(scroll, 240);
  }, [scrollMessagesToBottom]);

  const updateMessagesScrollState = useCallback(() => {
    const element = messagesScrollRef.current;
    if (!element) {
      return;
    }
    const distanceFromBottom =
      element.scrollHeight - element.scrollTop - element.clientHeight;
    const isNearBottom = distanceFromBottom < 96;
    userNearBottomRef.current = isNearBottom;
    setShowScrollToBottom(!isNearBottom);
  }, []);

  useEffect(() => {
    if (!pendingInstantScrollRef.current && !userNearBottomRef.current) {
      return;
    }
    scheduleMessagesBottomScroll();
    pendingInstantScrollRef.current = false;
    userNearBottomRef.current = true;
  }, [historyLoading, messages, queuedInputs.length, scheduleMessagesBottomScroll]);

  useEffect(() => {
    const element = messagesScrollRef.current;
    if (!element) {
      return;
    }
    const shouldKeepBottom = () =>
      pendingInstantScrollRef.current || userNearBottomRef.current;
    const keepBottomIfNeeded = () => {
      if (shouldKeepBottom()) {
        scheduleMessagesBottomScroll();
      }
    };
    const mutationObserver = new MutationObserver(keepBottomIfNeeded);
    mutationObserver.observe(element, {
      childList: true,
      subtree: true,
      characterData: true,
    });
    const resizeObserver = new ResizeObserver(keepBottomIfNeeded);
    resizeObserver.observe(element);
    Array.from(element.children).forEach((child) => resizeObserver.observe(child));
    keepBottomIfNeeded();
    return () => {
      mutationObserver.disconnect();
      resizeObserver.disconnect();
    };
  }, [scheduleMessagesBottomScroll]);

  const handleMessagesScroll = useCallback(() => {
    updateMessagesScrollState();
  }, [updateMessagesScrollState]);

  useEffect(() => {
    const element = messagesScrollRef.current;
    if (!element) {
      return;
    }
    element.addEventListener("scroll", updateMessagesScrollState, { passive: true });
    updateMessagesScrollState();
    return () => element.removeEventListener("scroll", updateMessagesScrollState);
  }, [updateMessagesScrollState]);

  const currentSessionFallbackTitle = useMemo(
    () => titleFromChatMessages(messages),
    [messages],
  );

  const replaceLoadedMessages = useCallback((restored: ChatMessage[], shouldStickToBottom: boolean) => {
    const fallbackTimestamp = Date.now() / 1000;
    const withTimestamps = restored.map((message) => ({
      ...message,
      timestamp: message.timestamp || fallbackTimestamp,
    }));
    setMessages((prev) => {
      if (sameChatMessages(prev, withTimestamps)) {
        return prev;
      }
      if (shouldStickToBottom) {
        pendingInstantScrollRef.current = true;
        userNearBottomRef.current = true;
      }
      return withTimestamps;
    });
  }, []);

  const resetHistoryEventWindow = useCallback(() => {
    historyEventsRef.current = [];
    historyEventStartIndexRef.current = undefined;
    historyEventEndIndexRef.current = undefined;
  }, []);

  const applyHistoryEventWindow = useCallback(
    (
      events: HistoryEvent[],
      page: HistoryPagePayload,
      matchedThread: AgentThreadSummary | undefined,
      shouldStickToBottom: boolean,
    ) => {
      historyEventsRef.current = events;
      historyEventStartIndexRef.current = page.start_index ?? 0;
      historyEventEndIndexRef.current =
        page.end_index ?? (page.start_index ?? 0) + events.length;
      const nextTelemetry = historyEventsToTelemetry(
        events,
        matchedThread,
        telemetryFromThread(matchedThread),
      );
      const terminalTimeline =
        nextTelemetry.phase === "finished" || nextTelemetry.phase === "failed";
      const timelineRunning = resolveRunningState({
        phase: nextTelemetry.phase,
        state: matchedThread?.run_state || matchedThread?.state,
        thread: matchedThread,
      });
      const restored = historyEventsToMessages(events, {
        ensureRunningAssistant:
          timelineRunning || (!terminalTimeline && isThreadActive(matchedThread)),
        runningState: matchedThread?.run_state || matchedThread?.state,
      });
      replaceLoadedMessages(restored, shouldStickToBottom);
      setTelemetry(nextTelemetry);
      setRunning(timelineRunning || (!terminalTimeline && isThreadActive(matchedThread)));
      return { restored, nextTelemetry };
    },
    [replaceLoadedMessages],
  );

  const querySessionKey = searchParams.get("session") || undefined;
  const queryHistoryPath = searchParams.get("historyPath") || undefined;
  const queryView = searchParams.get("view") || undefined;

  const selectedThread = useMemo(
    () =>
      threads.find((thread) =>
        isSelectedThread(
          thread,
          sessionKey,
          historyPath,
          queryView,
        ),
      ),
    [historyPath, queryView, sessionKey, threads],
  );

  useEffect(() => {
    selectedThreadRef.current = selectedThread;
  }, [selectedThread]);

  useEffect(() => {
    telemetryPhaseRef.current = telemetry.phase;
  }, [telemetry.phase]);

  const conversationTitle =
    telemetry.title ||
    selectedThread?.title ||
    currentSessionFallbackTitle ||
    selectedThread?.session_key ||
    sessionKey;
  const hasRealMessages = messages.some((message) => isRealChatMessage(message.id));
  const isUninitializedDraftSession =
    !querySessionKey &&
    !historyPath &&
    !selectedThread &&
    draft.trim().length === 0 &&
    !hasRealMessages &&
    telemetry.phase === "idle" &&
    telemetry.plan.length === 0 &&
    telemetry.tools.length === 0 &&
    telemetry.errors.length === 0;
  const newChatActive =
    forceNewChat ||
    (!querySessionKey &&
      !queryHistoryPath &&
      !historyPath &&
      !selectedThread &&
      !hasRealMessages &&
      telemetry.phase === "idle" &&
      telemetry.plan.length === 0 &&
      telemetry.tools.length === 0 &&
      telemetry.errors.length === 0);
  const currentSourceTag = selectedThread
    ? formatThreadSource(selectedThread)
    : formatThreadSource({
        session_key: sessionKey,
        status: "active",
        source: telemetry.status?.source,
        runner_type:
          telemetry.status?.runner_type ||
          selectedRunnerAdapter(runnerOptions, runnerId),
        runner_id: telemetry.status?.runner_id || runnerId,
        agent_type: telemetry.status?.agent_type,
      });
  const currentRunnerTag = formatRunnerTag(telemetry.status, selectedThread, runnerId);
  const terminalTimeline = telemetry.phase === "finished" || telemetry.phase === "failed";
  const displayRunning = resolveRunningState({
    fallbackRunning: running || (!terminalTimeline && isThreadActive(selectedThread)),
    phase: telemetry.phase,
    state: selectedThread?.run_state || selectedThread?.state || telemetry.status?.state,
    thread: selectedThread,
  });
  const currentStateTag = formatCurrentStateTag(telemetry, selectedThread, displayRunning);
  const currentRunnerAdapter = selectedRunnerAdapter(runnerOptions, runnerId);
  const modelCommandsSupported = supportsRunnerModelSlashCommand(currentRunnerAdapter);
  const guideSupported = supportsRunningGuide({
    runnerId,
    runnerOptions,
    selectedThread,
    status: telemetry.status,
  });
  const {
    slashRunner,
    setSlashRunner,
    slashCommandOptions,
    slashRunnerOptions,
    showSlashRunnerPanel,
  } = useSlashRunnerSelection({
    enableModelCommands: modelCommandsSupported,
    draft,
    running,
    supplementSubmitting,
    runnerOptions,
  });

  const refreshThreads = useCallback(async () => {
    try {
      const response = await apiFetch("/api/im-gateway/agent/sessions/all?limit=80");
      if (!response.ok) {
        return;
      }
      const payload = (await response.json()) as { sessions?: AgentThreadSummary[] };
      const incoming = dedupeThreads(payload.sessions || []);
      const selectedQueueThread = incoming.find((thread) =>
        isSelectedThread(thread, sessionKey, historyPath, queryView),
      );
      const selectedQueueItems = selectedQueueThread
        ? queueItemsFromUnknown(
            selectedQueueThread.queueItems ?? selectedQueueThread.queue_items,
          )
        : null;
      if (selectedQueueItems) {
        setQueuedInputs(selectedQueueItems);
      }
      setThreads((prev) => {
        const selectedLocal = prev.find((thread) =>
          isSelectedThread(thread, sessionKey, historyPath, queryView),
        );
        if (
          selectedLocal &&
          !incoming.some((thread) =>
            isSelectedThread(thread, sessionKey, historyPath, queryView),
          )
        ) {
          return dedupeThreads([selectedLocal, ...incoming]);
        }
        return incoming;
      });
      return selectedQueueThread;
    } catch {
      // Keep chat usable even if the session index is temporarily unavailable.
      return undefined;
    }
  }, [historyPath, queryView, sessionKey]);

  const refreshSessionDetailTelemetry = useCallback(async () => {
    const response = await apiFetch(
      `/api/im-gateway/agent/sessions/${encodeURIComponent(sessionKey)}`,
    );
    if (!response.ok) {
      return;
    }
    const detail = (await response.json()) as SessionDetail;
    const matchedThread = threadsRef.current.find((thread) =>
      isSelectedThread(thread, sessionKey, undefined, "active"),
    );
    setTelemetry(telemetryFromSessionDetail(detail, matchedThread));
    setThreads((prev) =>
      prev.map((thread) =>
        thread.session_key === sessionKey
          ? {
              ...thread,
              source: detail.source || thread.source,
              work_dir: detail.work_dir || thread.work_dir,
              agent_type: detail.agent_type || thread.agent_type,
              runner_type: detail.runner_type || thread.runner_type,
              runner_id: detail.runner_id || thread.runner_id,
              has_timeline: detail.has_timeline || thread.has_timeline,
              timeline_event_count:
                detail.timeline_event_count ?? thread.timeline_event_count,
              run_state: detail.run_state || thread.run_state,
            }
          : thread,
      ),
    );
  }, [sessionKey]);

  const setSearchParamsForActiveSession = useCallback(() => {
    setSearchParams(
      (prev) => {
        prev.set("view", "chat");
        prev.set("session", sessionKey);
        prev.delete("mode");
        prev.delete("aiSection");
        prev.delete("settings");
        prev.delete("agentSection");
        prev.delete("imGatewaySection");
        prev.delete("historyPath");
        return prev;
      },
      { replace: true },
    );
  }, [sessionKey, setSearchParams]);

  useEffect(() => {
    refreshThreads();
    const refreshIfVisible = () => {
      if (document.visibilityState === "visible") {
        void refreshThreads();
      }
    };
    document.addEventListener("visibilitychange", refreshIfVisible);
    return () => {
      document.removeEventListener("visibilitychange", refreshIfVisible);
    };
  }, [refreshThreads]);

  useEffect(() => {
    threadsRef.current = threads;
  }, [threads]);

  useEffect(() => {
    selectedHistoryPathRef.current = historyPath || queryHistoryPath;
    selectedSessionKeyRef.current = querySessionKey || sessionKey;
  }, [historyPath, queryHistoryPath, querySessionKey, sessionKey]);

  useEffect(() => {
    window.localStorage.setItem(
      THREAD_RAIL_COLLAPSED_STORAGE_KEY,
      threadRailCollapsed ? "true" : "false",
    );
  }, [threadRailCollapsed]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      setNowSeconds(Math.floor(Date.now() / 1000));
    }, 30_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    let cancelled = false;
    apiFetch("/api/im-gateway/chat/config")
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
        setDefaultRunnerId(defaultRunner);
        setRunnerId((current) => options.some((option) => option.value === current) ? current : defaultRunner);
        setNewChatRunnerId((current) => options.some((option) => option.value === current) ? current : defaultRunner);
      })
      .catch(() => {
        // Keep the last known external runner selection when configuration refresh fails.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    apiFetch("/api/im-gateway/agent/instructions")
      .then(async (response) => {
        if (!response.ok) {
          return undefined;
        }
        return response.json() as Promise<{ work_dir?: string }>;
      })
      .then((payload) => {
        const workDirFromConfig = payload?.work_dir;
        if (cancelled || !workDirFromConfig) {
          return;
        }
        setDefaultWorkDir(workDirFromConfig);
        setWorkDir((current) => current || workDirFromConfig);
      })
      .catch(() => {
        // Keep the composer usable even when instruction metadata is unavailable.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const nextHistoryPath = queryHistoryPath;
    const nextSessionKey = querySessionKey;
    const conversationKey = nextHistoryPath
      ? `history:${nextHistoryPath}`
      : nextSessionKey
        ? `active:${nextSessionKey}`
        : "draft";
    const shouldStickToBottom = loadedConversationKeyRef.current !== conversationKey;
    if (shouldStickToBottom) {
      resetHistoryEventWindow();
    }
    loadedConversationKeyRef.current = conversationKey;
    setHistoryPath(nextHistoryPath);
    if (nextSessionKey) {
      setSessionKey(nextSessionKey);
    }
    if (!nextHistoryPath) {
      if (!nextSessionKey) {
        return;
      }
      let cancelled = false;
      setHistoryLoading(true);
      apiFetch(`/api/im-gateway/agent/sessions/${encodeURIComponent(nextSessionKey)}`)
        .then(async (response) => {
          if (response.status === 404) {
            return { missing: true } as SessionDetail & { missing: true };
          }
          if (!response.ok) {
            return undefined;
          }
          return response.json() as Promise<SessionDetail>;
        })
        .then(async (detail) => {
          if (cancelled || !detail) {
            return;
          }
          const matchedThread = threadsRef.current.find((thread) =>
            isSelectedThread(thread, nextSessionKey, undefined, "active"),
          );
          const fallbackHistoryThread =
            matchedThread?.history_path
              ? matchedThread
              : threadsRef.current.find(
                  (thread) => thread.session_key === nextSessionKey && thread.history_path,
                );
          if ("missing" in detail && fallbackHistoryThread?.history_path) {
            pendingInstantScrollRef.current = true;
            setHistoryPath(fallbackHistoryThread.history_path);
            setSearchParams(
              (prev) => {
                prev.set("session", nextSessionKey);
                prev.set("view", "chat");
                prev.set("historyPath", fallbackHistoryThread.history_path!);
                prev.delete("mode");
                prev.delete("aiSection");
                prev.delete("settings");
                prev.delete("agentSection");
                prev.delete("imGatewaySection");
                return prev;
              },
              { replace: true },
            );
            return;
          }
          if ("missing" in detail) {
            return;
          }
          const restored = sessionDetailToMessages(detail);
          const timelineHistoryPath =
            detail.history_path ||
            matchedThread?.history_path ||
            fallbackHistoryThread?.history_path;
          let timelineMessages: ChatMessage[] | undefined;
          let timelineEvents: HistoryEvent[] | undefined;
          if (timelineHistoryPath) {
            try {
              const payload = await fetchHistoryPage(timelineHistoryPath);
              if (
                selectedSessionKeyRef.current !== nextSessionKey ||
                (selectedHistoryPathRef.current &&
                  selectedHistoryPathRef.current !== timelineHistoryPath)
              ) {
                return;
              }
              timelineEvents = payload.events || [];
              const timelineThread = {
                ...(matchedThread || fallbackHistoryThread || {
                  session_key: nextSessionKey,
                  status: "active" as const,
                }),
                ...(detail.running !== undefined ? { running: detail.running } : {}),
                ...(detail.state ? { state: detail.state } : {}),
                ...(detail.run_state ? { run_state: detail.run_state } : {}),
              };
              historyEventsRef.current = timelineEvents;
              historyEventStartIndexRef.current = payload.start_index ?? 0;
              historyEventEndIndexRef.current =
                payload.end_index ??
                (payload.start_index ?? 0) + timelineEvents.length;
              const timelineRunning = resolveRunningState({
                fallbackRunning: detail.running === true,
                state:
                  detail.run_state ||
                  detail.state ||
                  timelineThread?.run_state ||
                  timelineThread?.state,
                thread: timelineThread,
              });
              timelineMessages = historyEventsToMessages(timelineEvents, {
                ensureRunningAssistant: timelineRunning,
                runningState:
                  detail.run_state ||
                  detail.state ||
                  timelineThread?.run_state ||
                  timelineThread?.state,
              });
            } catch {
              // Keep the active detail fallback usable while timeline is being written.
            }
          }
          const loadedMessages =
            timelineMessages && timelineMessages.length > 0
              ? timelineMessages
              : restored;
          if (loadedMessages.length > 0) {
            replaceLoadedMessages(loadedMessages, shouldStickToBottom);
          }
          const resolvedTitle =
            matchedThread?.title || detail.title || titleFromChatMessages(loadedMessages);
          setThreads((prev) => {
            let changed = false;
            const next = prev.map((thread) => {
              if (thread.session_key !== nextSessionKey) {
                return thread;
              }
              const updated = {
                ...thread,
                title: resolvedTitle || thread.title,
                source: detail.source || thread.source,
                work_dir: detail.work_dir || thread.work_dir,
                agent_type: detail.agent_type || thread.agent_type,
                runner_type: detail.runner_type || thread.runner_type,
                runner_id: detail.runner_id || thread.runner_id,
                history_path: timelineHistoryPath || thread.history_path,
                has_timeline: detail.has_timeline || thread.has_timeline || Boolean(timelineHistoryPath),
                timeline_event_count:
                  detail.timeline_event_count ?? thread.timeline_event_count,
                run_state: detail.run_state || thread.run_state,
                ...(detail.running !== undefined ? { running: detail.running } : {}),
              };
              if (
                updated.title !== thread.title ||
                updated.source !== thread.source ||
                updated.work_dir !== thread.work_dir ||
                updated.agent_type !== thread.agent_type ||
                updated.runner_type !== thread.runner_type ||
                updated.runner_id !== thread.runner_id ||
                updated.history_path !== thread.history_path ||
                updated.has_timeline !== thread.has_timeline ||
                updated.timeline_event_count !== thread.timeline_event_count ||
                updated.run_state !== thread.run_state ||
                updated.running !== thread.running
              ) {
                changed = true;
                return updated;
              }
              return thread;
            });
            return changed ? next : prev;
          });
          const detailTelemetry = telemetryFromSessionDetail(detail, matchedThread);
          const detailQueueItems = queueItemsFromUnknown(
            detail.queueItems ?? detail.queue_items,
          );
          if (detailQueueItems) {
            setQueuedInputs(detailQueueItems);
          }
          const nextTelemetry = timelineEvents
            ? historyEventsToTelemetry(
                timelineEvents,
                {
                  ...(matchedThread || {
                    session_key: nextSessionKey,
                    status: "active" as const,
                  }),
                  history_path: timelineHistoryPath,
                  ...(detail.running !== undefined ? { running: detail.running } : {}),
                  state: detail.state || matchedThread?.state,
                  run_state: detail.run_state || matchedThread?.run_state,
                },
                detailTelemetry,
              )
            : detailTelemetry;
          setTelemetry(nextTelemetry);
          setRunning(
            resolveRunningState({
              fallbackRunning: detail.running === true,
              phase: nextTelemetry.phase,
              state: detail.run_state || detail.state || nextTelemetry.status?.state,
              thread: matchedThread,
            }),
          );
          setWorkDir(detail.work_dir || matchedThread?.work_dir || defaultWorkDir);
          setRunnerId(detail.runner_id || matchedThread?.runner_id || defaultRunnerId);
        })
        .finally(() => {
          if (!cancelled) {
            setHistoryLoading(false);
          }
        });
      return () => {
        cancelled = true;
      };
    }

    let cancelled = false;
    setHistoryLoading(true);
    fetchHistoryPage(nextHistoryPath)
      .then((payload) => {
        if (cancelled || selectedHistoryPathRef.current !== nextHistoryPath) {
          return;
        }
        const matchedThread = threadsRef.current.find((thread) =>
          isSelectedThread(thread, nextSessionKey || "", nextHistoryPath, "history"),
        );
        const pageEvents = payload.events || [];
        const { restored } = applyHistoryEventWindow(
          pageEvents,
          payload,
          matchedThread,
          shouldStickToBottom,
        );
        if (restored.length > 0) {
          const resolvedTitle = matchedThread?.title || titleFromChatMessages(restored);
          if (resolvedTitle) {
            setThreads((prev) => {
              let changed = false;
              const next = prev.map((thread) => {
                if (
                  thread.session_key === nextSessionKey &&
                  thread.history_path === nextHistoryPath &&
                  thread.title !== resolvedTitle
                ) {
                  changed = true;
                  return { ...thread, title: resolvedTitle };
                }
                return thread;
              });
              return changed ? next : prev;
            });
          }
          setRunnerId(matchedThread?.runner_id || defaultRunnerId);
          const eventSessionKey =
            pageEvents.find((event) => event.session_key)?.session_key;
          if (nextSessionKey || eventSessionKey) {
            setSessionKey(nextSessionKey || eventSessionKey!);
          }
        }
      })
      .catch((error) => {
        if (!cancelled) {
          antdMessage.error(
            error instanceof Error
              ? error.message
              : "Failed to load Agent history",
          );
        }
      })
      .finally(() => {
        if (!cancelled) {
          setHistoryLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [
    defaultRunnerId,
    defaultWorkDir,
    applyHistoryEventWindow,
    queryHistoryPath,
    querySessionKey,
    replaceLoadedMessages,
    resetHistoryEventWindow,
    setSearchParams,
  ]);

  const mergeTimelineEvents = useCallback(
    (
      events: HistoryEvent[],
      page: Pick<HistoryPagePayload, "start_index" | "end_index">,
      shouldStickToBottom: boolean,
      authoritativeThread?: AgentThreadSummary,
    ) => {
      const merged = mergeHistoryEventWindow(
        historyEventsRef.current,
        historyEventStartIndexRef.current,
        historyEventEndIndexRef.current,
        events,
        page.start_index,
        page.end_index,
      );
      if (merged.gap || !merged.changed) {
        return undefined;
      }
      const { nextTelemetry } = applyHistoryEventWindow(
        merged.events,
        {
          ...page,
          start_index: merged.startIndex,
          end_index: merged.endIndex,
        },
        authoritativeThread,
        shouldStickToBottom,
      );
      return nextTelemetry;
    },
    [applyHistoryEventWindow],
  );

  const catchUpTimelineFromEvent = useCallback(
    async (
      eventPayload?: AgentSessionEventPayload,
      options: { forceFull?: boolean } = {},
    ) => {
      const currentSessionKey = querySessionKey || sessionKey;
      const eventSessionKey = eventPayload?.sessionKey || eventPayload?.session_key;
      const eventHistoryPath = eventPayload?.historyPath || eventPayload?.history_path;
      const currentThread = selectedThreadRef.current;
      const currentHistoryPath =
        historyPath ||
        queryHistoryPath ||
        currentThread?.history_path ||
        (eventSessionKey === currentSessionKey ? eventHistoryPath : undefined);
      if (!currentHistoryPath) {
        return false;
      }
      if (eventHistoryPath && eventHistoryPath !== currentHistoryPath) {
        return false;
      }
      if (!eventHistoryPath && eventSessionKey && eventSessionKey !== currentSessionKey) {
        return false;
      }
      const endIndex = eventPayload?.endIndex ?? eventPayload?.end_index;
      const currentEndIndex = historyEventEndIndexRef.current;
      if (
        !options.forceFull &&
        endIndex !== undefined &&
        currentEndIndex !== undefined &&
        endIndex <= currentEndIndex
      ) {
        return true;
      }
      const fetchFull = options.forceFull || currentEndIndex === undefined;
      let page = await fetchHistoryPage(
        currentHistoryPath,
        fetchFull
          ? {}
          : { since: currentEndIndex },
      );
      if (
        selectedSessionKeyRef.current !== currentSessionKey ||
        (selectedHistoryPathRef.current &&
          selectedHistoryPathRef.current !== currentHistoryPath)
      ) {
        return false;
      }
      const liveEndIndex = historyEventEndIndexRef.current;
      if (
        !fetchFull &&
        liveEndIndex !== undefined &&
        page.start_index !== undefined &&
        page.start_index > liveEndIndex
      ) {
        await refreshThreads();
        page = await fetchHistoryPage(currentHistoryPath);
        if (
          selectedSessionKeyRef.current !== currentSessionKey ||
          (selectedHistoryPathRef.current &&
            selectedHistoryPathRef.current !== currentHistoryPath)
        ) {
          return false;
        }
      }
      const nextTelemetry = mergeTimelineEvents(
        page.events || [],
        page,
        userNearBottomRef.current,
        currentThread,
      );
      if (nextTelemetry) {
        const stillRunning = resolveRunningState({
          phase: nextTelemetry.phase,
          state: currentThread?.run_state || currentThread?.state || nextTelemetry.status?.state,
          thread: currentThread,
        });
        setRunning(stillRunning);
        if (!stillRunning && nextTelemetry.phase === "running") {
          setTelemetry((prev) =>
            prev.phase === "running" ? { ...prev, phase: "finished" } : prev,
          );
        }
      }
      return true;
    },
    [
      historyPath,
      mergeTimelineEvents,
      queryHistoryPath,
      querySessionKey,
      refreshThreads,
      sessionKey,
    ],
  );

  useEffect(() => {
    const parseEvent = (event: MessageEvent<string>) => {
      try {
        return JSON.parse(event.data) as AgentSessionEventPayload;
      } catch {
        return undefined;
      }
    };
    const refreshFromSessionEvent = (event: MessageEvent<string>) => {
      const payload = parseEvent(event);
      if (document.visibilityState === "visible") {
        void refreshThreads();
      }
      if (payload?.reason === "lagged") {
        void catchUpTimelineFromEvent(undefined, { forceFull: true });
      }
    };
    const refreshTimeline = (event: MessageEvent<string>) => {
      const payload = parseEvent(event);
      void catchUpTimelineFromEvent(payload);
    };
    const catchUpOnReconnect = () => {
      if (
        telemetryPhaseRef.current === "running" ||
        isThreadActive(selectedThreadRef.current)
      ) {
        void catchUpTimelineFromEvent(undefined, { forceFull: true });
      }
    };
    const eventsUrl = `${buildApiUrl("/im-gateway/agent/sessions/events")}?x_client_id=${encodeURIComponent(getClientId())}`;
    const eventSource = new EventSource(eventsUrl);
    eventSource.addEventListener("connected", catchUpOnReconnect);
    eventSource.addEventListener("sessions_changed", refreshFromSessionEvent);
    eventSource.addEventListener("timeline_changed", refreshTimeline);
    return () => {
      eventSource.removeEventListener("connected", catchUpOnReconnect);
      eventSource.removeEventListener("sessions_changed", refreshFromSessionEvent);
      eventSource.removeEventListener("timeline_changed", refreshTimeline);
      eventSource.close();
    };
  }, [
    catchUpTimelineFromEvent,
    refreshThreads,
  ]);

  useEffect(() => {
    const requestedSessionKey = searchParams.get("session") || undefined;
    const requestedHistoryPath = searchParams.get("historyPath") || undefined;
    if (!requestedSessionKey || requestedHistoryPath || threads.length === 0) {
      return;
    }
    const historyThread = threads.find(
      (thread) =>
        thread.session_key === requestedSessionKey &&
        thread.status === "ended" &&
        thread.history_path,
    );
    if (!historyThread?.history_path) {
      return;
    }
    setHistoryPath(historyThread.history_path);
    setSearchParams(
      (prev) => {
        prev.set("session", requestedSessionKey);
        prev.set("view", "chat");
        prev.set("historyPath", historyThread.history_path!);
        prev.delete("mode");
        prev.delete("aiSection");
        prev.delete("settings");
        prev.delete("agentSection");
        prev.delete("imGatewaySection");
        return prev;
      },
      { replace: true },
    );
  }, [searchParams, setSearchParams, threads]);

  const handleCreateNewChat = useCallback(() => {
    const selectedWorkDir = newChatWorkDir.trim() || defaultWorkDir;
    initialThreadAutoSelectRef.current = true;
    if (isUninitializedDraftSession) {
      setWorkDir(selectedWorkDir);
      setRunnerId(newChatRunnerId);
      setNewChatOpen(false);
      return;
    }
    const nextSessionKey = `admin-chat-${Date.now()}`;
    setSessionKey(nextSessionKey);
    setHistoryPath(undefined);
    pendingInstantScrollRef.current = true;
    setMessages([]);
    setDraft("");
    setSlashRunner(undefined);
    setTelemetry(EMPTY_TELEMETRY);
    setQueuedInputs([]);
    setRunning(false);
    setWorkDir(selectedWorkDir);
    setRunnerId(newChatRunnerId);
    setNewChatOpen(false);
    refreshThreads();
    setSearchParams(
      (prev) => {
        prev.set("view", "chat");
        prev.set("mode", "new");
        prev.delete("session");
        prev.delete("historyPath");
        prev.delete("aiSection");
        prev.delete("settings");
        prev.delete("agentSection");
        prev.delete("imGatewaySection");
        return prev;
      },
      { replace: false },
    );
  }, [
    defaultWorkDir,
    isUninitializedDraftSession,
    newChatRunnerId,
    newChatWorkDir,
    refreshThreads,
    setSearchParams,
  ]);

  const resetToNewChat = useCallback(() => {
    const nextSessionKey = `admin-chat-${Date.now()}`;
    streamAbortRef.current?.abort();
    streamAbortRef.current = null;
    setSessionKey(nextSessionKey);
    setHistoryPath(undefined);
    resetHistoryEventWindow();
    loadedConversationKeyRef.current = "draft";
    selectedHistoryPathRef.current = undefined;
    selectedSessionKeyRef.current = nextSessionKey;
    pendingInstantScrollRef.current = true;
    setMessages(STARTER_MESSAGES);
    setDraft("");
    setPendingImages([]);
    setTelemetry(EMPTY_TELEMETRY);
    setQueuedInputs([]);
    setRunning(false);
    setSupplementSubmitting(false);
    setSlashRunner(undefined);
    setWorkDir(defaultWorkDir);
    setRunnerId(defaultRunnerId);
    setNewChatRunnerId(defaultRunnerId);
    setSearchParams(
      (prev) => {
        prev.set("view", "chat");
        prev.set("mode", "new");
        prev.delete("aiSection");
        prev.delete("settings");
        prev.delete("agentSection");
        prev.delete("imGatewaySection");
        prev.delete("session");
        prev.delete("historyPath");
        return prev;
      },
      { replace: false },
    );
  }, [defaultRunnerId, defaultWorkDir, resetHistoryEventWindow, setSearchParams]);

  const handleOpenThread = useCallback(
    (thread: AgentThreadSummary) => {
      if (
        queryView !== "settings" &&
        isSelectedThread(thread, sessionKey, historyPath, queryView)
      ) {
        return;
      }
      // Abort any in-flight stream from the previous session to prevent cross-contamination
      streamAbortRef.current?.abort();
      streamAbortRef.current = null;
      setSessionKey(thread.session_key);
      setHistoryPath(thread.history_path);
      setDraft("");
      setSupplementSubmitting(false);
      setTelemetry(telemetryFromThread(thread));
      setQueuedInputs(
        queueItemsFromUnknown(thread.queueItems ?? thread.queue_items) ?? [],
      );
      setWorkDir(thread.work_dir || defaultWorkDir);
      setRunnerId(thread.runner_id || defaultRunnerId);
      pendingInstantScrollRef.current = true;
      if (!thread.history_path) {
        setMessages(STARTER_MESSAGES);
      }
      setSearchParams(
        (prev) => {
          prev.set("view", "chat");
          prev.set("session", thread.session_key);
          prev.delete("mode");
          prev.delete("aiSection");
          prev.delete("settings");
          prev.delete("agentSection");
          prev.delete("imGatewaySection");
          if (thread.history_path) {
            prev.set("view", "chat");
            prev.set("historyPath", thread.history_path);
          } else {
            prev.set("view", "chat");
            prev.delete("historyPath");
          }
          return prev;
        },
        { replace: false },
      );
    },
    [defaultWorkDir, historyPath, queryView, sessionKey, setSearchParams],
  );

  useEffect(() => {
    if (
      embeddedSidebar ||
      initialThreadAutoSelectRef.current ||
      querySessionKey ||
      queryHistoryPath ||
      threads.length === 0
    ) {
      return;
    }
    initialThreadAutoSelectRef.current = true;
    handleOpenThread(threads[0]);
  }, [embeddedSidebar, handleOpenThread, queryHistoryPath, querySessionKey, threads]);

  const handleDeleteThread = useCallback(
    async (thread: AgentThreadSummary) => {
      try {
        const response = await apiFetch(
          `/api/im-gateway/agent/sessions/${encodeURIComponent(thread.session_key)}`,
          { method: "DELETE" },
        );
        if (!response.ok) {
          throw new Error(await response.text());
        }

        setThreads((prev) =>
          prev.filter((item) => item.session_key !== thread.session_key),
        );

        if (isSelectedThread(thread, sessionKey, historyPath, queryView)) {
          const nextSessionKey = `admin-chat-${Date.now()}`;
          setSessionKey(nextSessionKey);
          setHistoryPath(undefined);
          setMessages(STARTER_MESSAGES);
          setTelemetry(EMPTY_TELEMETRY);
          setRunning(false);
          setQueuedInputs([]);
          setDraft("");
          setSlashRunner(undefined);
          pendingInstantScrollRef.current = true;
          setSearchParams(
            (prev) => {
              prev.set("view", "chat");
              prev.set("mode", "new");
              prev.delete("session");
              prev.delete("historyPath");
              prev.delete("aiSection");
              prev.delete("settings");
              prev.delete("agentSection");
              prev.delete("imGatewaySection");
              return prev;
            },
            { replace: false },
          );
        }

        antdMessage.success("Conversation deleted");
        void refreshThreads();
      } catch (error) {
        antdMessage.error(
          error instanceof Error && error.message.trim()
            ? error.message
            : "Failed to delete conversation",
        );
      }
    },
    [historyPath, queryView, refreshThreads, sessionKey, setSearchParams],
  );

  const styles = useMemo(
    () => createAgentChatStyles(isCompact, isNarrow, threadRailCollapsed, token, embeddedSidebar),
    [embeddedSidebar, isCompact, isNarrow, threadRailCollapsed, token],
  );

  useEffect(() => {
    onNewChatStateChange?.(newChatActive);
  }, [newChatActive, onNewChatStateChange]);

  useEffect(() => {
    onSidebarStateChange?.({
      threads,
      sessionKey,
      historyPath,
      view: queryView,
      nowSeconds,
      styles,
      onOpenThread: handleOpenThread,
      onDeleteThread: handleDeleteThread,
    });
  }, [
    handleDeleteThread,
    handleOpenThread,
    historyPath,
    nowSeconds,
    onSidebarStateChange,
    queryView,
    sessionKey,
    styles,
    threads,
  ]);

  const applyQueueEvent = (event: Record<string, unknown>) => {
    const items = queueItemsFromEvent(event);
    if (items) {
      setQueuedInputs(items);
    }
  };

  const submitRunningInput = async (
    content: string,
    mode: "guide" | "queue" | "stop" | "remove",
  ) => {
    const effectiveMode = mode === "guide" && !guideSupported ? "queue" : mode;
    const queuesRunningInput = effectiveMode === "queue";
    const rendersMessage = effectiveMode === "guide" || effectiveMode === "stop";
    const message = queuesRunningInput && !content.startsWith("/q ")
      ? `/q ${content}`
      : effectiveMode === "guide" && !content.startsWith("/g ")
        ? `/g ${content}`
        : content;
    const userMessage: ChatMessage = {
      id: `user-${Date.now()}`,
      role: "user",
      content: effectiveMode === "stop" ? "/stop" : content,
      timestamp: Date.now() / 1000,
      meta:
        effectiveMode === "stop"
          ? "Control"
          : queuesRunningInput
            ? "Queued user"
            : "Guide user",
    };
    const assistantId = `assistant-${Date.now()}`;
    const assistantMessage: ChatMessage = {
      id: assistantId,
      role: "assistant",
      content:
        effectiveMode === "stop"
          ? "Stopping..."
          : queuesRunningInput
            ? "Queueing..."
            : "Injecting guide...",
      timestamp: Date.now() / 1000,
      meta: "Runner",
    };
    if (rendersMessage) {
      pendingInstantScrollRef.current = true;
      setMessages((prev) => [...prev, userMessage, assistantMessage]);
    }
    setDraft("");
    const submitSessionKey = sessionKey;
    setSupplementSubmitting(true);
    try {
      await runAgentStream({
        message,
        sessionKey,
        historyPath,
        workDir: workDir || undefined,
        runnerId,
        runnerAdapter: selectedRunnerAdapter(runnerOptions, runnerId),
        signal: streamAbortRef.current?.signal,
        onEvent: (event) => {
          if (selectedSessionKeyRef.current !== submitSessionKey) return;
          if (effectiveMode === "stop") {
            setTelemetry((prev) => reduceTelemetry(prev, event));
          }
          applyQueueEvent(event);
        },
        onDelta: () => {},
        onFinal: (response) => {
          if (selectedSessionKeyRef.current !== submitSessionKey) return;
          if (rendersMessage) {
            setMessages((prev) =>
              prev.map((message) =>
                message.id === assistantId
                  ? { ...message, content: response || message.content }
                  : message,
              ),
            );
          }
        },
      });
      if (
        selectedSessionKeyRef.current === submitSessionKey &&
        effectiveMode === "stop"
      ) {
        refreshThreads();
      }
    } catch (error) {
      if (error instanceof DOMException && error.name === "AbortError") return;
      if (selectedSessionKeyRef.current !== submitSessionKey) return;
      const text = error instanceof Error ? error.message : "Agent input failed";
      if (rendersMessage) {
        setMessages((prev) =>
          prev.map((message) =>
            message.id === assistantId ? { ...message, content: text } : message,
          ),
        );
      }
      antdMessage.error(text);
    } finally {
      if (selectedSessionKeyRef.current === submitSessionKey) {
        setSupplementSubmitting(false);
      }
    }
  };

  const handleRunningInput = async (content: string) => {
    await submitRunningInput(
      content,
      guideSupported ? runningInputMode : "queue",
    );
  };

  const handleStop = async () => {
    await submitRunningInput("/stop", "stop");
    setRunning(false);
    refreshThreads();
  };

  const handleRemoveQueued = async (seq: number) => {
    await submitRunningInput(`/rq ${seq}`, "remove");
  };

  const handleGuideQueued = async (item: QueuedInput) => {
    await handleRemoveQueued(item.seq);
    await submitRunningInput(item.message, "guide");
  };

  const addImageFiles = useCallback(
    (files: File[]) => {
      const imageFiles = files.filter((file) => file.type.startsWith("image/"));
      if (imageFiles.length === 0) {
        return;
      }
      const remaining = MAX_PASTED_IMAGES - pendingImages.length;
      if (remaining <= 0) {
        antdMessage.warning(`You can attach up to ${MAX_PASTED_IMAGES} images.`);
        return;
      }
      const accepted = imageFiles.slice(0, remaining);
      if (accepted.length < imageFiles.length) {
        antdMessage.warning(`Only the first ${MAX_PASTED_IMAGES} images are kept.`);
      }
      accepted.forEach((file) => {
        void pendingImageFromFile(file).then((image) => {
          if (!image) {
            return;
          }
          setPendingImages((prev) => {
            if (prev.length >= MAX_PASTED_IMAGES) {
              return prev;
            }
            return [...prev, image];
          });
        });
      });
    },
    [pendingImages.length],
  );

  const handlePasteImages = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = imageFilesFromClipboard(event);
    if (files.length === 0) {
      return;
    }
    event.preventDefault();
    addImageFiles(files);
  };

  const handleSend = async (options?: {
    contentOverride?: string;
    imagesOverride?: PendingChatImage[];
    runnerIdOverride?: string;
  }) => {
    const rawContent = (options?.contentOverride ?? draft).trim();
    const imagesForSend = options?.imagesOverride ?? (options?.contentOverride ? [] : pendingImages);
    if ((!rawContent && imagesForSend.length === 0) || supplementSubmitting) {
      return;
    }
    if (running) {
      await handleRunningInput(rawContent);
      return;
    }
    const content = rawContent;
    const telemetryRunnerId = telemetry.status?.runner_id;
    const activeRunnerId = options?.runnerIdOverride || telemetryRunnerId || runnerId;
    const activeRunnerAdapter = selectedRunnerAdapter(runnerOptions, activeRunnerId);
    const runnerModelCommand = isRunnerModelSlashCommand(rawContent, activeRunnerAdapter);
    const controlCommand = runnerModelCommand;
    if (!content && imagesForSend.length === 0) {
      return;
    }
    if (slashRunner && !running && !controlCommand) {
      setPendingImages([]);
      await handleRunnerCall(content, slashRunner, imagesForSend);
      return;
    }
    const userVisibleContent = content || imageCountLabel(imagesForSend.length);
    const userMessage: ChatMessage = {
      id: `user-${Date.now()}`,
      role: "user",
      content: userVisibleContent,
      contentParts: imageContentParts(content, imagesForSend),
      timestamp: Date.now() / 1000,
      meta: "You",
    };
    const assistantId = `assistant-${Date.now()}`;
    const assistantMessage: ChatMessage = {
      id: assistantId,
      role: runnerModelCommand ? "system" : "assistant",
      content: controlCommand ? "" : "Agent is running...",
      timestamp: Date.now() / 1000,
      meta: runnerModelCommand ? "System" : "Runner",
    };
    pendingInstantScrollRef.current = true;
    setMessages((prev) => {
      if (runnerModelCommand) {
        return prev;
      }
      return [...prev, userMessage, assistantMessage];
    });
    setDraft("");
    setPendingImages([]);
    setRunning(true);
    setTelemetry((prev) => ({
      ...prev,
      phase: "running",
      status: {
        ...(prev.status || {}),
        work_dir: workDir || undefined,
        runner_id: activeRunnerId,
        runner_type: selectedRunnerAdapter(runnerOptions, activeRunnerId),
      },
      plan: [],
      tools: [],
      errors: [],
    }));
    // Ensure the current session is visible in the threads list with first message as fallback title
    setThreads((prev) => {
      const fallbackTitle =
        userVisibleContent.length > 40 ? `${userVisibleContent.slice(0, 40)}…` : userVisibleContent;
      return dedupeThreads([
        {
          session_key: sessionKey,
          status: "active",
          title: fallbackTitle,
          source: "admin-api",
          start_time: Math.floor(Date.now() / 1000),
          last_active_time: Math.floor(Date.now() / 1000),
          duration_secs: 0,
          runner_id: activeRunnerId,
          runner_type: selectedRunnerAdapter(runnerOptions, activeRunnerId),
          work_dir: workDir || undefined,
        },
        ...prev.filter((thread) => thread.session_key !== sessionKey),
      ]);
    });
    const sendSessionKey = sessionKey;
    const abortController = new AbortController();
    streamAbortRef.current?.abort();
    streamAbortRef.current = abortController;
    let assistantSegmentId = assistantId;
    let assistantSegmentIndex = 0;
    let assistantSegmentHasText = false;
    let assistantSegmentHasSteps = false;
    let assistantSegmentHasProposedPlan = false;
    try {

      const appendAssistantSegment = (initialContent = "") => {
        const previousSegmentId = assistantSegmentId;
        assistantSegmentIndex += 1;
        assistantSegmentId = `assistant-${Date.now()}-${assistantSegmentIndex}`;
        assistantSegmentHasText = initialContent.trim().length > 0;
        assistantSegmentHasSteps = false;
        const message: ChatMessage = {
          id: assistantSegmentId,
          role: "assistant",
          content: initialContent,
          timestamp: Date.now() / 1000,
          meta: "Runner",
        };
        setMessages((prev) => [
          ...prev.map((item) =>
            item.id === previousSegmentId ? { ...item, hideTimestamp: true } : item,
          ),
          message,
        ]);
      };


      const appendProcessStep = (step: ProcessStep) => {
        const targetId = assistantSegmentId;
        const segmentHadText = assistantSegmentHasText;
        setMessages((prev) =>
          prev.map((message) =>
            message.id === targetId
              ? (() => {
                  let processSteps = [...(message.processSteps || [])];
                  if (step.type === "compaction") {
                    const runningCompactionIndex = processSteps.findIndex(
                      (item) => item.type === "compaction" && item.status === "running",
                    );
                    if (runningCompactionIndex >= 0) {
                      processSteps[runningCompactionIndex] = step;
                    } else {
                      processSteps.push(step);
                    }
                  } else {
                    processSteps = appendProcessStepToTimeline(processSteps, step);
                  }
                  return {
                    ...message,
                    content:
                      !segmentHadText &&
                      message.content === "Agent is running..."
                        ? ""
                        : message.content,
                    processSteps,
                  };
                })()
              : message,
          ),
        );
        assistantSegmentHasSteps = true;
      };

      const updateRunningToolStep = (
        toolName: string,
        success: boolean,
        toolResult?: string,
        durationMs?: number,
      ) => {
        const targetId = assistantSegmentId;
        setMessages((prev) => {
          const updateMessageAt = (messages: ChatMessage[], index: number) => {
            const steps = [...(messages[index].processSteps || [])];
            const stepIndex = steps
              .map((step, i) => ({ step, i }))
              .reverse()
              .find(
                ({ step }) =>
                  step.type === "tool" &&
                  step.status === "running" &&
                  step.summary.startsWith(toolName),
              )?.i;
            if (stepIndex === undefined) {
              return null;
            }
            steps[stepIndex] = {
              ...steps[stepIndex],
              status: success ? "success" : "failed",
              result: toolResult || undefined,
              completedAt: Date.now() / 1000,
              durationMs,
            };
            const next = [...messages];
            next[index] = { ...messages[index], processSteps: steps };
            return next;
          };

          const targetIndex = prev.findIndex((message) => message.id === targetId);
          if (targetIndex >= 0) {
            const next = updateMessageAt(prev, targetIndex);
            if (next) {
              return next;
            }
          }
          for (let index = prev.length - 1; index >= 0; index -= 1) {
            if (index === targetIndex || prev[index].role !== "assistant") {
              continue;
            }
            const next = updateMessageAt(prev, index);
            if (next) {
              return next;
            }
          }
          return prev;
        });
        assistantSegmentHasSteps = true;
      };

      const updateRunningToolPreview = (toolResult: string, durationMs?: number) => {
        if (!toolResult.trim()) {
          return;
        }
        const targetId = assistantSegmentId;
        setMessages((prev) => {
          const updateMessageAt = (messages: ChatMessage[], index: number) => {
            const steps = [...(messages[index].processSteps || [])];
            const stepIndex = steps
              .map((step, i) => ({ step, i }))
              .reverse()
              .find(
                ({ step }) => step.type === "tool" && step.status === "running",
              )?.i;
            if (stepIndex === undefined) {
              return null;
            }
            steps[stepIndex] = {
              ...steps[stepIndex],
              result: toolResult,
              durationMs: durationMs ?? steps[stepIndex].durationMs,
            };
            const next = [...messages];
            next[index] = { ...messages[index], processSteps: steps };
            return next;
          };

          const targetIndex = prev.findIndex((message) => message.id === targetId);
          if (targetIndex >= 0) {
            const next = updateMessageAt(prev, targetIndex);
            if (next) {
              return next;
            }
          }
          for (let index = prev.length - 1; index >= 0; index -= 1) {
            if (index === targetIndex || prev[index].role !== "assistant") {
              continue;
            }
            const next = updateMessageAt(prev, index);
            if (next) {
              return next;
            }
          }
          return prev;
        });
        assistantSegmentHasSteps = true;
      };

      const applyFinalResponse = (response: string) => {
        const trimmedResponse = response.trim();
        if (!trimmedResponse) {
          return;
        }
        let handled = false;
        const targetId = assistantSegmentId;
        const segmentHadText = assistantSegmentHasText;
        const segmentHadSteps = assistantSegmentHasSteps;
        setMessages((prev) => {
          const current = prev.find((message) => message.id === targetId);
          if (!current) {
            return prev;
          }
          if (current.content.trim() === trimmedResponse) {
            handled = true;
            return prev;
          }
          if (!segmentHadText && !segmentHadSteps) {
            handled = true;
            assistantSegmentHasText = true;
            return prev.map((message) =>
              message.id === targetId
                ? { ...message, content: response || message.content }
                : message,
            );
          }
          return prev;
        });
        if (!handled) {
          appendAssistantSegment(response);
        }
        setMessages((prev) => {
          let lastAssistantIndex = -1;
          for (let index = prev.length - 1; index >= 0; index -= 1) {
            if (prev[index].role === "assistant") {
              lastAssistantIndex = index;
              break;
            }
            if (prev[index].role === "user") {
              break;
            }
          }
          if (lastAssistantIndex < 0) {
            return prev;
          }
          let lastUserIndex = -1;
          for (let index = lastAssistantIndex; index >= 0; index -= 1) {
            if (prev[index].role === "user") {
              lastUserIndex = index;
              break;
            }
          }
          return prev.map((message, index) =>
            message.role === "assistant" && index > lastUserIndex
              ? { ...message, hideTimestamp: index !== lastAssistantIndex }
              : message,
          );
        });
      };

      const appendSystemDisplayMessage = (content: string) => {
        const trimmedContent = content.trim();
        if (!trimmedContent) {
          return;
        }
        const timestamp = Date.now() / 1000;
        setMessages((prev) => {
          const last = prev[prev.length - 1];
          if (
            last?.role === "system" &&
            last.content === trimmedContent &&
            Math.abs((last.timestamp || 0) - timestamp) < 3
          ) {
            return prev;
          }
          return [
            ...prev,
            {
              id: `system-${Date.now()}`,
              role: "system",
              content: trimmedContent,
              timestamp,
              meta: "System",
            },
          ];
        });
      };

      const appendProposedPlan = (planContent: string) => {
        const trimmedPlan = planContent.trim();
        if (!trimmedPlan || assistantSegmentHasProposedPlan) {
          return;
        }
        assistantSegmentHasProposedPlan = true;
        const renderedPlan = proposedPlanMessageContent(trimmedPlan);
        if (assistantSegmentHasText || assistantSegmentHasSteps) {
          appendAssistantSegment(renderedPlan);
          return;
        }
        assistantSegmentHasText = true;
        setMessages((prev) =>
          prev.map((message) =>
            message.id === assistantSegmentId
              ? { ...message, content: renderedPlan }
              : message,
          ),
        );
      };

      await runAgentStream({
        message: content,
        images: imagesForSend,
        sessionKey,
        historyPath,
        workDir: workDir || undefined,
        runnerId: activeRunnerId,
        runnerAdapter: activeRunnerAdapter,
        signal: abortController.signal,
        onEvent: (event) => {
          if (selectedSessionKeyRef.current !== sendSessionKey) return;
          setTelemetry((prev) => reduceTelemetry(prev, event));
          if (event.eventType === "proposed_plan" && typeof event.content === "string") {
            appendProposedPlan(event.content);
            return;
          }
          if (event.eventType === "run_finished" && typeof event.proposedPlan === "string") {
            appendProposedPlan(event.proposedPlan);
          }
          // Dynamically update thread title in the sidebar when set_title fires
          if (event.eventType === "title_updated" && typeof event.title === "string") {
            const newTitle = event.title as string;
            setThreads((prev) => {
              let changed = false;
              const next = prev.map((t) => {
                if (t.session_key === sessionKey && t.title !== newTitle) {
                  changed = true;
                  return { ...t, title: newTitle };
                }
                return t;
              });
              return changed ? next : prev;
            });
          }
          if (event.eventType === "tool_started") {
            const toolStep = eventToProcessStep(event);
            if (toolStep) {
              appendProcessStep(toolStep);
            }
            return;
          }
          if (event.eventType === "tool_finished" && isRecord(event.log)) {
            const log = event.log as Record<string, unknown>;
            const toolName = stringFrom(log.tool_name) || stringFrom(log.toolName) || "tool";
            const success = log.success !== false;
            const toolResult = stringFrom(log.result);
            updateRunningToolStep(
              toolName,
              success,
              toolResult || undefined,
              typeof event.durationMs === "number" ? event.durationMs : undefined,
            );
            return;
          }
          if (event.eventType === "long_task_status") {
            const preview = stringFrom(event.lastOutputPreview);
            if (preview) {
              updateRunningToolPreview(preview, numberFrom(event.elapsedMs));
            }
            return;
          }
          const step = eventToProcessStep(event);
          if (step) {
            appendProcessStep(step);
          }
        },
        onDelta: () => {
          // Deltas are rendered as assistant message segments from onEvent.
        },
        onFinal: (response) => {
          if (selectedSessionKeyRef.current !== sendSessionKey) return;
          if (runnerModelCommand) {
            appendSystemDisplayMessage(
              runnerModelSlashSystemDisplayContent(rawContent, response),
            );
            return;
          }
          applyFinalResponse(response);
        },
      });
      if (selectedSessionKeyRef.current === sendSessionKey) {
        setHistoryPath(undefined);
        setQueuedInputs([]);
        setSearchParamsForActiveSession();
      }
      refreshThreads();
    } catch (error) {
      if (error instanceof DOMException && error.name === "AbortError") return;
      if (selectedSessionKeyRef.current !== sendSessionKey) return;
      const text = error instanceof Error ? error.message : "Agent run failed";
      setTelemetry((prev) => ({
        ...prev,
        phase: "failed",
        errors: prev.errors.includes(text) ? prev.errors : [...prev.errors, text],
      }));
      setMessages((prev) =>
        prev.map((message) =>
          message.id === assistantSegmentId
            ? {
                  ...message,
                  content: text,
                  processSteps: [
                    ...(message.processSteps || []),
                    {
                      type: "status",
                      summary: "Agent run failed",
                      status: "failed",
                      result: text,
                    },
                  ],
                }
            : message,
        ),
      );
      setThreads((prev) =>
        prev.map((thread) =>
          thread.session_key === sendSessionKey
            ? {
                ...thread,
                status: "ended",
                running: false,
                state: "failed",
                run_state: "failed",
                last_active_time: Math.floor(Date.now() / 1000),
              }
            : thread,
        ),
      );
      antdMessage.error(text);
    } finally {
      if (selectedSessionKeyRef.current === sendSessionKey) {
        setRunning(false);
      }
      if (streamAbortRef.current === abortController) {
        streamAbortRef.current = null;
      }
    }
  };

  const handleSendRef = useRef(handleSend);

  useEffect(() => {
    handleSendRef.current = handleSend;
  });

  const startNewChat = useCallback(
    async (message: string, nextRunnerId?: string, images: PendingChatImage[] = []) => {
      const trimmedMessage = message.trim();
      if (!trimmedMessage && images.length === 0) {
        return;
      }
      if (nextRunnerId) {
        setRunnerId(nextRunnerId);
        setNewChatRunnerId(nextRunnerId);
      }
      setDraft(trimmedMessage);
      await handleSendRef.current({
        contentOverride: trimmedMessage,
        imagesOverride: images,
        runnerIdOverride: nextRunnerId,
      });
    },
    [],
  );

  const resetToNewChatRef = useRef(resetToNewChat);
  const startNewChatRef = useRef(startNewChat);

  useEffect(() => {
    resetToNewChatRef.current = resetToNewChat;
    startNewChatRef.current = startNewChat;
  }, [resetToNewChat, startNewChat]);

  useEffect(() => {
    onControlsReady?.({
      openNewChat: () => resetToNewChatRef.current(),
      startNewChat: (message, runnerId, images) =>
        startNewChatRef.current(message, runnerId, images),
    });
  }, [onControlsReady]);


  const handleSlashCommand = (option: SlashCommandOption) => {
    setSlashRunner(undefined);
    const focusComposerAtEnd = (value?: string) => {
      setTimeout(() => {
        const input = document.querySelector<HTMLTextAreaElement>(
          '[data-testid="agent-chat-input"]',
        );
        input?.focus();
        const cursor = (value ?? input?.value ?? "").length;
        input?.setSelectionRange(cursor, cursor);
      }, 0);
    };
    if (option.action === "insert") {
      const inserted = option.insertText || `${option.command} `;
      setDraft(inserted);
      focusComposerAtEnd(inserted);
      return;
    }
    void handleSend({ contentOverride: option.command });
  };

  const updateSlashActiveIndex = useCallback((nextIndex: number | ((index: number) => number)) => {
    const resolvedIndex =
      typeof nextIndex === "function" ? nextIndex(slashActiveIndexRef.current) : nextIndex;
    slashActiveIndexRef.current = resolvedIndex;
    setSlashActiveIndex(resolvedIndex);
  }, []);

  const slashOptionCount = slashCommandOptions.length + slashRunnerOptions.length;
  const slashOptionKey = useMemo(
    () =>
      [
        ...slashCommandOptions.map((option) => `command:${option.value}`),
        ...slashRunnerOptions.map((option) => `runner:${option.value}`),
      ].join("|"),
    [slashCommandOptions, slashRunnerOptions],
  );

  useEffect(() => {
    if (!showSlashRunnerPanel || slashOptionCount <= 0) {
      updateSlashActiveIndex(0);
      return;
    }
    updateSlashActiveIndex((index) => Math.min(index, slashOptionCount - 1));
  }, [showSlashRunnerPanel, slashOptionCount, slashOptionKey, updateSlashActiveIndex]);

  const selectActiveSlashOption = useCallback(() => {
    if (!showSlashRunnerPanel || slashOptionCount <= 0) {
      return false;
    }
    const activeIndex = Math.min(slashActiveIndexRef.current, slashOptionCount - 1);
    if (activeIndex < slashCommandOptions.length) {
      const option = slashCommandOptions[activeIndex];
      if (!option) {
        return false;
      }
      handleSlashCommand(option);
      return true;
    }
    const runner = slashRunnerOptions[activeIndex - slashCommandOptions.length];
    if (!runner) {
      return false;
    }
    setSlashRunner(runner);
    setDraft("");
    return true;
  }, [
    showSlashRunnerPanel,
    slashCommandOptions,
    slashOptionCount,
    slashRunnerOptions,
    handleSlashCommand,
    setSlashRunner,
  ]);

  const handleComposerKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (showSlashRunnerPanel && slashOptionCount > 0) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        updateSlashActiveIndex((index) => (index + 1) % slashOptionCount);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        updateSlashActiveIndex((index) => (index - 1 + slashOptionCount) % slashOptionCount);
        return;
      }
      if (event.key === "Enter" && !event.shiftKey) {
        event.preventDefault();
        selectActiveSlashOption();
        return;
      }
      if (event.key === "Tab") {
        event.preventDefault();
        selectActiveSlashOption();
        return;
      }
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void handleSend();
    }
  };

  const handleRunnerCall = useRunnerCallHandler({
    historyPath,
    messages,
    pendingInstantScrollRef,
    refreshSessionDetailTelemetry,
    refreshThreads,
    runnerId,
    runnerOptions,
    sessionKey,
    setDraft,
    setHistoryPath,
    setMessages,
    setRunning,
    setSearchParamsForActiveSession,
    setSlashRunner,
    setTelemetry,
    setThreads,
    workDir,
  });

  const handleOpenRunnerCallThread = useCallback(
    (message: ChatMessage) => {
      const runnerCall = message.runnerCall;
      if (!runnerCall?.childSessionKey) {
        return;
      }
      handleOpenThread({
        session_key: runnerCall.childSessionKey,
        status: "active",
        running: runnerCall.status === "running",
        state: runnerCall.status === "running" ? "running" : "idle",
        title: `Run with ${runnerCall.targetRunnerLabel}`,
        source: "runner_call",
        start_time: Math.floor(message.timestamp || Date.now() / 1000),
        last_active_time: Math.floor(Date.now() / 1000),
        duration_secs: 0,
        runner_id: runnerCall.targetRunnerId,
        runner_type: runnerCall.targetAdapter || runnerCall.targetRunnerId,
        work_dir: workDir || undefined,
      });
    },
    [handleOpenThread, workDir],
  );

  return (
    <div data-testid="agent-chat-section" style={styles.shell}>
      <style>
        {`
          @keyframes agent-chat-running-dot {
            0%, 100% { opacity: 0.35; transform: scale(0.8); }
            50% { opacity: 1; transform: scale(1); }
          }
        `}
      </style>
      <div style={styles.body}>
        <Card
          size="small"
          title={
            <div style={styles.titleContent}>
              <div style={styles.titleMainLine}>
                <RobotOutlined style={{ flexShrink: 0 }} />
                <span
                  data-testid="agent-chat-title"
                  style={styles.titleText}
                  title={conversationTitle}
                >
                  {conversationTitle}
                </span>
              </div>
              <Space size={6} wrap>
                <Tag color="blue" data-testid="agent-chat-source-tag">
                  {currentSourceTag}
                </Tag>
                <Tag data-testid="agent-chat-runner-tag">{currentRunnerTag}</Tag>
                <Tag
                  color={
                    running
                      ? "processing"
                      : currentStateTag === "New"
                        ? "default"
                        : currentStateTag === "Error"
                          ? "error"
                          : "success"
                  }
                  data-testid="agent-chat-state-tag"
                >
                  {currentStateTag}
                </Tag>
              </Space>
            </div>
          }
          extra={
            <Space size={8} style={styles.titleActions}>
              <Button
                size="small"
                icon={<SettingOutlined />}
                data-testid="agent-chat-settings-open"
                onClick={() => setSettingsOpen(true)}
              >
                Status
              </Button>
            </Space>
          }
          style={styles.conversationCard}
          bodyStyle={{
            flex: 1,
            minHeight: 0,
            display: "flex",
            flexDirection: "column",
            padding: 0,
          }}
        >
          <div
            data-testid="agent-chat-messages"
            aria-busy={historyLoading}
            ref={messagesScrollRef}
            onScroll={handleMessagesScroll}
            style={styles.conversation}
          >
            <div style={styles.conversationTrack} data-testid="agent-chat-message-track">
              {messages.length === 0 ? (
                <div
                  data-testid="agent-chat-empty-state"
                  style={styles.emptyMessages}
                >
                  <Empty
                    image={Empty.PRESENTED_IMAGE_SIMPLE}
                    description="Start a conversation by typing a message below."
                  />
                </div>
              ) : (
                <AgentChatMessageList
                  isCompact={isCompact}
                  messages={messages}
                  onOpenRunnerCallThread={handleOpenRunnerCallThread}
                  running={displayRunning}
                  styles={styles}
                  token={token}
                />
              )}
            </div>
            <div ref={messagesEndRef} />

            <div style={styles.composer}>
              <div
                data-testid="agent-chat-scroll-bottom-layer"
                style={{
                  ...styles.scrollToBottomLayer,
                  opacity: showScrollToBottom ? 1 : 0,
                  transform: showScrollToBottom
                    ? "translateY(0) scale(1)"
                    : "translateY(10px) scale(0.96)",
                  pointerEvents: showScrollToBottom ? "auto" : "none",
                }}
              >
                <Button
                  shape="circle"
                  icon={<DownOutlined />}
                  aria-label="Scroll to bottom"
                  data-testid="agent-chat-scroll-bottom"
                  style={styles.scrollToBottomButton}
                  onClick={() => scheduleMessagesBottomScroll(true)}
                />
              </div>
              <div style={styles.composerTrack} data-testid="agent-chat-composer-track">
                <AgentChatTokenHud telemetry={telemetry} styles={styles} />
                <AgentChatPlan
                  plan={telemetry.plan}
                  styles={styles}
                  successColor={token.colorSuccess}
                  primaryColor={token.colorPrimary}
                />
                <Space direction="vertical" size={6} style={{ width: "100%" }}>
                  <AgentChatPromptChips prompts={PROMPT_CHIPS} onSelect={setDraft} />
                  {queuedInputs.length > 0 ? (
                    <div style={styles.queuePanel} data-testid="agent-chat-queue-panel">
                      <Space direction="vertical" size={6} style={{ width: "100%" }}>
                        <Text type="secondary" style={{ fontSize: 12 }}>
                          Queued
                        </Text>
                        <div style={styles.queueList} data-testid="agent-chat-queue-list">
                          {queuedInputs.map((item) => (
                            <div
                              key={item.seq}
                              style={styles.queueItem}
                              data-testid="agent-chat-queue-item"
                            >
                              <Text
                                ellipsis
                                style={styles.queueItemText}
                                title={`#${item.seq} ${item.message}`}
                              >
                                #{item.seq} {item.message}
                              </Text>
                              <span style={styles.queueItemActions}>
                                {guideSupported ? (
                                  <Button
                                    size="small"
                                    icon={<SendOutlined />}
                                    onClick={() => handleGuideQueued(item)}
                                  >
                                    Guide
                                  </Button>
                                ) : null}
                                <Button
                                  size="small"
                                  icon={<DeleteOutlined />}
                                  aria-label={`Remove queued message ${item.seq}`}
                                  onClick={() => handleRemoveQueued(item.seq)}
                                />
                              </span>
                            </div>
                          ))}
                        </div>
                      </Space>
                    </div>
                  ) : null}
                  {running && draft.trim() ? (
                    <div style={styles.runningInputToolbar}>
                      <Text type="secondary" style={{ fontSize: 12 }}>
                        {guideSupported
                          ? "Running input"
                          : "This runner queues follow-up messages"}
                      </Text>
                      {guideSupported ? (
                        <Segmented
                          size="small"
                          value={runningInputMode}
                          onChange={(value) =>
                            setRunningInputMode(value as "guide" | "queue")
                          }
                          options={[
                            { label: "Guide", value: "guide" },
                            { label: "Queue", value: "queue" },
                          ]}
                        />
                      ) : (
                        <Tag>Queue</Tag>
                      )}
                    </div>
                  ) : null}
              {showSlashRunnerPanel ? (
                <SlashRunnerPanel
                  commands={slashCommandOptions}
                  options={slashRunnerOptions}
                  activeIndex={slashActiveIndex}
                  styles={styles}
                  onActiveIndexChange={updateSlashActiveIndex}
                  onSelectCommand={(option) => {
                    handleSlashCommand(option);
                  }}
                  onSelect={(option) => {
                    setSlashRunner(option);
                    setDraft("");
                  }}
                />
              ) : null}
              <div style={styles.inputWrap}>
                {slashRunner ? (
                  <SelectedRunnerPill
                    runner={slashRunner}
                    styles={styles}
                    onClear={() => setSlashRunner(undefined)}
                  />
                ) : null}
                <AgentChatImagePreviewStrip
                  images={pendingImages}
                  styles={styles}
                  onRemove={(imageId) =>
                    setPendingImages((prev) => prev.filter((item) => item.id !== imageId))
                  }
                />
                <TextArea
                  data-testid="agent-chat-input"
                  data-session-key={sessionKey}
                  value={draft}
                  onChange={(event) => {
                    const nextDraft = event.target.value;
                    if (nextDraft.trimStart().startsWith("/") && slashRunner) {
                      setSlashRunner(undefined);
                    }
                    setDraft(nextDraft);
                  }}
                  onPaste={handlePasteImages}
                  onKeyDown={handleComposerKeyDown}
                  placeholder="Describe a task for the Agent..."
                  autoSize={{ minRows: 2, maxRows: 7 }}
                  style={{
                    padding: slashRunner
                      ? "42px 56px 30px 14px"
                      : "8px 56px 30px 14px",
                    border: "none",
                    boxShadow: "none",
                    outline: "none",
                    background: "transparent",
                    resize: "none",
                  }}
                />
                <Text style={styles.inputHint} data-testid="agent-chat-input-hint">
                  Shift + Enter for a new line
                </Text>
                <Button
                  shape="circle"
                  type="primary"
                  icon={running && !draft.trim() ? <BorderOutlined /> : <SendOutlined />}
                  aria-label={running && !draft.trim() ? "Stop" : "Send"}
                  title={running && !draft.trim() ? "Stop current turn" : "Send"}
                  onClick={running && !draft.trim() ? handleStop : () => void handleSend()}
                  disabled={
                    supplementSubmitting || (!draft.trim() && pendingImages.length === 0 && !running)
                  }
                  data-testid="agent-chat-send"
                  style={styles.sendInInput}
                />
              </div>
                </Space>
              </div>
            </div>
          </div>
        </Card>

        {!embeddedSidebar && threadRailCollapsed ? (
          <Button
            shape="circle"
            icon={<LeftOutlined />}
            aria-label="Expand threads"
            title="Expand threads"
            data-testid="agent-chat-threads-expand"
            style={styles.threadRailExpandButton}
            onClick={() => setThreadRailCollapsed(false)}
          />
        ) : !embeddedSidebar ? (
          <div style={styles.sideRail}>
            <AgentThreadListCard
              threads={threads}
              sessionKey={sessionKey}
              historyPath={historyPath}
              view={queryView}
              nowSeconds={nowSeconds}
              styles={styles}
              onOpenThread={handleOpenThread}
              onDeleteThread={handleDeleteThread}
              onCollapse={() => setThreadRailCollapsed(true)}
            />
          </div>
        ) : null}

        <AgentChatSettingsModal
          open={settingsOpen}
          onClose={() => setSettingsOpen(false)}
          styles={styles}
          workDir={workDir}
          defaultWorkDir={defaultWorkDir}
          telemetry={telemetry}
          historyPath={historyPath}
        />

        <Modal
          title="New Chat"
          open={newChatOpen}
          onOk={handleCreateNewChat}
          onCancel={() => setNewChatOpen(false)}
          okText="Create"
          destroyOnHidden
        >
          <Space
            direction="vertical"
            size={8}
            style={{ width: "100%" }}
            data-testid="agent-chat-new-modal"
          >
            <Input
              value={newChatWorkDir}
              onChange={(event) => setNewChatWorkDir(event.target.value)}
              placeholder="Working directory (leave empty for default)"
              prefix={<FolderOpenOutlined />}
              data-testid="agent-chat-new-workspace"
            />
            <Select
              value={newChatRunnerId}
              onChange={setNewChatRunnerId}
              options={runnerOptions}
              style={{ width: "100%" }}
              data-testid="agent-chat-new-runner"
            />
          </Space>
        </Modal>
      </div>
    </div>
  );
}
