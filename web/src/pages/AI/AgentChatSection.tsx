import { useCallback, useEffect, useMemo, useRef, useState, type ClipboardEvent, type KeyboardEvent } from "react";
import { useSearchParams } from "react-router-dom";
import { Button, Card, Col, Empty, Grid, Input, Modal, Row, Segmented, Select, Space, Tag, Typography, message as antdMessage, theme } from "antd";
import { BulbOutlined, DeleteOutlined, DownOutlined, FolderOpenOutlined, BorderOutlined, LeftOutlined, PlusOutlined, RobotOutlined, SendOutlined, SettingOutlined } from "@ant-design/icons";
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
  formatRunnerOptionLabel,
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
import { historyEventsToMessages, historyEventsToTelemetry } from "./AgentChatSection.timeline";
import { isRunStateActive, isThreadActive } from "./AgentChatSection.timelinePolling";
import { AgentChatMessageList } from "./AgentChatSection.messages";
import { AgentChatPlan, AgentChatPromptChips } from "./AgentChatSection.composerExtras";
import { AgentChatSettingsModal, AgentThreadListCard } from "./AgentChatSection.panels";
import {
  queueItemsFromEvent,
  queueItemsFromUnknown,
  type QueuedInput,
} from "./AgentChatSection.queue";
import { SelectedRunnerPill, SlashRunnerPanel, useRunnerCallHandler, useSlashRunnerSelection, type SlashCommandOption } from "./AgentChatSection.runnerCall";
import { createAgentChatStyles } from "./AgentChatSection.styles";
import { AgentChatTokenHud } from "./AgentChatSection.tokenHud";

const { Text } = Typography;
const { TextArea } = Input;
const { useBreakpoint } = Grid;
const MAX_PASTED_IMAGES = 6;
const HISTORY_EVENT_PAGE_SIZE = 300;
const THREAD_RAIL_COLLAPSED_STORAGE_KEY = "bifrost.agentChat.threadRailCollapsed";
type AgentCollaborationMode = "plan";

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

function parseAgentPlanSlash(content: string): {
  message: string;
  collaborationMode?: AgentCollaborationMode;
} {
  const trimmed = content.trimStart();
  if (!trimmed.startsWith("/plan")) {
    return { message: content };
  }
  const rest = trimmed.slice("/plan".length);
  if (rest.length > 0 && !/\s/.test(rest[0])) {
    return { message: content };
  }
  return {
    message: rest.trimStart(),
    collaborationMode: "plan",
  };
}

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

function supportsBuiltInAgentCommands({
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
  if (/\b(codex|chatgpt|webgpt|external|external_runner|chatgpt_web)\b/.test(explicit)) {
    return false;
  }
  if (/\b(bifrost_agent|builtin)\b/.test(explicit)) {
    return true;
  }
  if (selectedThread?.runner_id && selectedThread.runner_id !== "bifrost_agent") {
    return false;
  }
  if (status?.runner_id && status.runner_id !== "bifrost_agent") {
    return false;
  }
  return runnerId === "bifrost_agent" || selectedRunnerAdapter(runnerOptions, runnerId) === "bifrost_agent";
}

function proposedPlanMessageContent(content: string) {
  return `**Plan Mode result**\n\n${content.trim()}`;
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
  params: { tail?: boolean; limit?: number; cursor?: number; since?: number } = {},
) {
  const query = new URLSearchParams();
  if (params.tail) {
    query.set("tail", "true");
  }
  if (params.limit !== undefined) {
    query.set("limit", String(params.limit));
  }
  if (params.cursor !== undefined) {
    query.set("cursor", String(params.cursor));
  }
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
  params: { tail?: boolean; limit?: number; cursor?: number; since?: number } = {},
) {
  const response = await apiFetch(historyPageUrl(historyPath, params));
  if (!response.ok) {
    throw new Error(await response.text());
  }
  return response.json() as Promise<HistoryPagePayload>;
}

function imageCountLabel(count: number) {
  return count === 1 ? "Attached 1 image" : `Attached ${count} images`;
}

function imageContentParts(content: string, images: PendingChatImage[]) {
  if (images.length === 0) {
    return undefined;
  }
  return [
    ...(content.trim() ? [{ type: "text" as const, text: content }] : []),
    ...images.map((image) => ({
      type: "image_url" as const,
      image_url: { url: image.previewUrl, detail: "auto" },
    })),
  ];
}

function imageSizeLabel(size: number) {
  if (size >= 1024 * 1024) {
    return `${(size / (1024 * 1024)).toFixed(1)} MB`;
  }
  if (size >= 1024) {
    return `${Math.ceil(size / 1024)} KB`;
  }
  return `${size} B`;
}

function imageFilesFromClipboard(event: ClipboardEvent<HTMLTextAreaElement>) {
  const files = Array.from(event.clipboardData.files);
  const itemFiles = Array.from(event.clipboardData.items)
    .filter((item) => item.kind === "file")
    .map((item) => item.getAsFile())
    .filter((file): file is File => Boolean(file));
  return [...files, ...itemFiles].filter((file, index, allFiles) =>
    file.type.startsWith("image/") &&
    allFiles.findIndex(
      (candidate) =>
        candidate.name === file.name &&
        candidate.size === file.size &&
        candidate.type === file.type,
    ) === index,
  );
}

export default function AgentChatSection() {
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
  const [historyLoadingOlder, setHistoryLoadingOlder] = useState(false);
  const [historyHasOlder, setHistoryHasOlder] = useState(false);
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
  const [newChatRunnerId, setNewChatRunnerId] = useState("bifrost_agent");
  const [runnerId, setRunnerId] = useState("bifrost_agent");
  const [defaultRunnerId, setDefaultRunnerId] = useState("bifrost_agent");
  const [runnerOptions, setRunnerOptions] = useState<RunnerOption[]>([
    { label: "Bifrost Agent", value: "bifrost_agent", adapter: "bifrost_agent" },
  ]);
  const [composerMode, setComposerMode] = useState<AgentCollaborationMode | undefined>();
  const [activeCollaborationMode, setActiveCollaborationMode] = useState<AgentCollaborationMode | undefined>();
  const [slashActiveIndex, setSlashActiveIndex] = useState(0);
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
  const historyOlderCursorRef = useRef<number | undefined>(undefined);
  const historyLoadingOlderRef = useRef(false);
  const loadOlderHistoryPageRef = useRef<() => void>(() => {});
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
    const element = messagesScrollRef.current;
    if (element && element.scrollTop < 96) {
      loadOlderHistoryPageRef.current();
    }
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
    historyOlderCursorRef.current = undefined;
    setHistoryHasOlder(false);
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
      historyOlderCursorRef.current =
        typeof page.next_cursor === "number"
          ? page.next_cursor
          : historyEventStartIndexRef.current;
      setHistoryHasOlder(Boolean(page.has_more));
      const nextTelemetry = historyEventsToTelemetry(
        events,
        matchedThread,
        telemetryFromThread(matchedThread),
      );
      const terminalTimeline =
        nextTelemetry.phase === "finished" || nextTelemetry.phase === "failed";
      const timelineRunning =
        nextTelemetry.phase === "running" && isThreadActive(matchedThread);
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

  const loadOlderHistoryPage = useCallback(async () => {
    const timelineHistoryPath = historyPath || selectedThread?.history_path;
    const cursor = historyOlderCursorRef.current;
    if (
      !timelineHistoryPath ||
      cursor === undefined ||
      cursor <= 0 ||
      historyLoadingOlderRef.current ||
      historyLoadingOlder ||
      !historyHasOlder
    ) {
      return;
    }
    const element = messagesScrollRef.current;
    const previousScrollHeight = element?.scrollHeight ?? 0;
    const previousScrollTop = element?.scrollTop ?? 0;
    const previousEndIndex = historyEventEndIndexRef.current;
    historyLoadingOlderRef.current = true;
    setHistoryLoadingOlder(true);
    try {
      const page = await fetchHistoryPage(timelineHistoryPath, {
        cursor,
        limit: HISTORY_EVENT_PAGE_SIZE,
      });
      const olderEvents = page.events || [];
      if (olderEvents.length === 0) {
        setHistoryHasOlder(false);
        historyOlderCursorRef.current = undefined;
        return;
      }
      const mergedEvents = [...olderEvents, ...historyEventsRef.current];
      applyHistoryEventWindow(mergedEvents, page, selectedThread, false);
      historyEventEndIndexRef.current =
        previousEndIndex ?? historyEventEndIndexRef.current ?? mergedEvents.length;
      requestAnimationFrame(() => {
        const nextElement = messagesScrollRef.current;
        if (!nextElement) {
          return;
        }
        const addedHeight = nextElement.scrollHeight - previousScrollHeight;
        nextElement.scrollTop = previousScrollTop + addedHeight;
      });
    } catch (error) {
      antdMessage.error(
        error instanceof Error ? error.message : "Failed to load older Agent history",
      );
    } finally {
      historyLoadingOlderRef.current = false;
      setHistoryLoadingOlder(false);
    }
  }, [
    applyHistoryEventWindow,
    historyHasOlder,
    historyLoadingOlder,
    historyPath,
    selectedThread,
  ]);

  useEffect(() => {
    loadOlderHistoryPageRef.current = () => {
      void loadOlderHistoryPage();
    };
  }, [loadOlderHistoryPage]);

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
  const currentSourceTag = selectedThread
    ? formatThreadSource(selectedThread)
    : formatThreadSource({
        session_key: sessionKey,
        status: "active",
        source: telemetry.status?.source,
        runner_type:
          telemetry.status?.runner_type ||
          (runnerId === "bifrost_agent" ? "bifrost_agent" : selectedRunnerAdapter(runnerOptions, runnerId)),
        runner_id: telemetry.status?.runner_id || (runnerId === "bifrost_agent" ? undefined : runnerId),
        agent_type: telemetry.status?.agent_type,
      });
  const currentRunnerTag = formatRunnerTag(telemetry.status, selectedThread, runnerId);
  const terminalTimeline = telemetry.phase === "finished" || telemetry.phase === "failed";
  const displayRunning = running || (!terminalTimeline && isThreadActive(selectedThread));
  const currentStateTag = formatCurrentStateTag(telemetry, selectedThread, displayRunning);
  const builtInAgentCommandsSupported = supportsBuiltInAgentCommands({
    runnerId,
    runnerOptions,
    selectedThread,
    status: telemetry.status,
  });
  const guideSupported = builtInAgentCommandsSupported;
  const {
    slashRunner,
    setSlashRunner,
    slashCommandOptions,
    slashRunnerOptions,
    showSlashRunnerPanel,
  } = useSlashRunnerSelection({
    enableCommands: builtInAgentCommandsSupported,
    draft,
    running,
    supplementSubmitting,
    runnerOptions,
  });

  useEffect(() => {
    if (builtInAgentCommandsSupported) {
      return;
    }
    setComposerMode(undefined);
    setActiveCollaborationMode(undefined);
  }, [builtInAgentCommandsSupported]);

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

  const setSearchParamsForActiveSession = useCallback(() => {
    setSearchParams(
      (prev) => {
        prev.set("aiSection", "agent-chat");
        prev.set("agentSection", "chat");
        prev.set("session", sessionKey);
        prev.set("view", "active");
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
        const custom = Object.entries(payload.runners || {}).map(([id, settings]) => ({
          label: formatRunnerOptionLabel(id, settings.adapter),
          value: id,
          adapter: settings.adapter,
        }));
        setRunnerOptions([
          { label: "Bifrost Agent", value: "bifrost_agent", adapter: "bifrost_agent" },
          ...custom.sort((a, b) => a.label.localeCompare(b.label)),
        ]);
        const defaultRunner = payload.defaultRunnerId || payload.default_runner_id;
        if (defaultRunner) {
          setDefaultRunnerId(defaultRunner);
          setRunnerId((current) =>
            current === "bifrost_agent" ? defaultRunner : current,
          );
          setNewChatRunnerId((current) =>
            current === "bifrost_agent" ? defaultRunner : current,
          );
        }
      })
      .catch(() => {
        // Keep runner selection usable with the built-in runner fallback.
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
                prev.set("aiSection", "agent-chat");
                prev.set("agentSection", "chat");
                prev.set("session", nextSessionKey);
                prev.set("view", "history");
                prev.set("historyPath", fallbackHistoryThread.history_path!);
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
              const payload = await fetchHistoryPage(timelineHistoryPath, {
                tail: true,
                limit: HISTORY_EVENT_PAGE_SIZE,
              });
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
              historyOlderCursorRef.current =
                typeof payload.next_cursor === "number"
                  ? payload.next_cursor
                  : historyEventStartIndexRef.current;
              setHistoryHasOlder(Boolean(payload.has_more));
              timelineMessages = historyEventsToMessages(timelineEvents, {
                ensureRunningAssistant:
                  isThreadActive(timelineThread) ||
                  isRunStateActive(detail.run_state || detail.state),
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
            timelineMessages && timelineMessages.length > 0 ? timelineMessages : restored;
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
                updated.run_state !== thread.run_state
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
            detail.running === false
              ? false
              : nextTelemetry.phase === "running" ||
                  isRunStateActive(detail.run_state || detail.state),
          );
          setWorkDir(detail.work_dir || matchedThread?.work_dir || defaultWorkDir);
          setRunnerId(detail.runner_id || matchedThread?.runner_id || "bifrost_agent");
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
    fetchHistoryPage(nextHistoryPath, {
      tail: true,
      limit: HISTORY_EVENT_PAGE_SIZE,
    })
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
          setRunnerId(matchedThread?.runner_id || "bifrost_agent");
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
    defaultWorkDir,
    applyHistoryEventWindow,
    queryHistoryPath,
    querySessionKey,
    resetHistoryEventWindow,
    setSearchParams,
  ]);

  const mergeTimelineEvents = useCallback(
    (
      events: HistoryEvent[],
      page: Pick<HistoryPagePayload, "start_index" | "end_index" | "next_cursor" | "has_more">,
      shouldStickToBottom: boolean,
      authoritativeThread?: AgentThreadSummary,
    ) => {
      if (events.length === 0) {
        historyEventEndIndexRef.current =
          page.end_index ?? historyEventEndIndexRef.current;
        return undefined;
      }
      const startIndex = page.start_index;
      const currentEnd = historyEventEndIndexRef.current;
      const mergedEvents =
        startIndex !== undefined &&
        currentEnd !== undefined &&
        startIndex >= currentEnd
          ? [...historyEventsRef.current, ...events]
          : events;
      const previousStartIndex = historyEventStartIndexRef.current;
      const previousOlderCursor = historyOlderCursorRef.current;
      const wasAppending =
        startIndex !== undefined && currentEnd !== undefined && startIndex >= currentEnd;
      const { nextTelemetry } = applyHistoryEventWindow(
        mergedEvents,
        page,
        authoritativeThread,
        shouldStickToBottom,
      );
      if (wasAppending) {
        historyEventStartIndexRef.current = previousStartIndex ?? startIndex;
        historyOlderCursorRef.current =
          previousOlderCursor ?? historyEventStartIndexRef.current;
        setHistoryHasOlder(
          historyOlderCursorRef.current !== undefined && historyOlderCursorRef.current > 0,
        );
      }
      return nextTelemetry;
    },
    [applyHistoryEventWindow],
  );

  const catchUpTimelineFromEvent = useCallback(
    async (
      eventPayload?: AgentSessionEventPayload,
      options: { forceTail?: boolean } = {},
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
        !options.forceTail &&
        endIndex !== undefined &&
        currentEndIndex !== undefined &&
        endIndex <= currentEndIndex
      ) {
        return true;
      }
      const fetchTail = options.forceTail || currentEndIndex === undefined;
      let page = await fetchHistoryPage(
        currentHistoryPath,
        fetchTail
          ? { tail: true, limit: HISTORY_EVENT_PAGE_SIZE }
          : { since: currentEndIndex },
      );
      if (
        selectedSessionKeyRef.current !== currentSessionKey ||
        (selectedHistoryPathRef.current &&
          selectedHistoryPathRef.current !== currentHistoryPath)
      ) {
        return false;
      }
      if (
        !fetchTail &&
        currentEndIndex !== undefined &&
        page.start_index !== undefined &&
        page.start_index !== currentEndIndex
      ) {
        await refreshThreads();
        page = await fetchHistoryPage(currentHistoryPath, {
          tail: true,
          limit: HISTORY_EVENT_PAGE_SIZE,
        });
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
        const terminal =
          nextTelemetry.phase === "finished" || nextTelemetry.phase === "failed";
        const stillRunning =
          !terminal && (nextTelemetry.phase === "running" || isThreadActive(currentThread));
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
        void catchUpTimelineFromEvent(undefined, { forceTail: true });
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
        void catchUpTimelineFromEvent(undefined, { forceTail: true });
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
    const requestedView = searchParams.get("view") || undefined;
    if (
      !requestedSessionKey ||
      requestedHistoryPath ||
      requestedView === "active" ||
      threads.length === 0
    ) {
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
        prev.set("aiSection", "agent-chat");
        prev.set("agentSection", "chat");
        prev.set("session", requestedSessionKey);
        prev.set("view", "history");
        prev.set("historyPath", historyThread.history_path!);
        return prev;
      },
      { replace: true },
    );
  }, [searchParams, setSearchParams, threads]);

  const handleOpenNewChat = useCallback(() => {
    setNewChatWorkDir(workDir || defaultWorkDir);
    setNewChatRunnerId(isUninitializedDraftSession ? defaultRunnerId : runnerId);
    setNewChatOpen(true);
  }, [defaultRunnerId, defaultWorkDir, isUninitializedDraftSession, runnerId, workDir]);

  const handleCreateNewChat = useCallback(() => {
    const selectedWorkDir = newChatWorkDir.trim() || defaultWorkDir;
    initialThreadAutoSelectRef.current = true;
    if (isUninitializedDraftSession) {
      setWorkDir(selectedWorkDir);
      setRunnerId(newChatRunnerId);
      setComposerMode(undefined);
      setActiveCollaborationMode(undefined);
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
    setComposerMode(undefined);
    setActiveCollaborationMode(undefined);
    setTelemetry(EMPTY_TELEMETRY);
    setQueuedInputs([]);
    setRunning(false);
    setWorkDir(selectedWorkDir);
    setRunnerId(newChatRunnerId);
    setNewChatOpen(false);
    refreshThreads();
    setSearchParams(
      (prev) => {
        prev.set("aiSection", "agent-chat");
        prev.set("agentSection", "chat");
        prev.delete("session");
        prev.delete("view");
        prev.delete("historyPath");
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

  const handleOpenThread = useCallback(
    (thread: AgentThreadSummary) => {
      if (isSelectedThread(thread, sessionKey, historyPath, queryView)) {
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
      setComposerMode(undefined);
      setActiveCollaborationMode(undefined);
      setQueuedInputs(
        queueItemsFromUnknown(thread.queueItems ?? thread.queue_items) ?? [],
      );
      setWorkDir(thread.work_dir || defaultWorkDir);
      setRunnerId(thread.runner_id || "bifrost_agent");
      pendingInstantScrollRef.current = true;
      if (!thread.history_path) {
        setMessages(STARTER_MESSAGES);
      }
      setSearchParams(
        (prev) => {
          prev.set("aiSection", "agent-chat");
          prev.set("agentSection", "chat");
          prev.set("session", thread.session_key);
          if (thread.history_path) {
            prev.set("view", "history");
            prev.set("historyPath", thread.history_path);
          } else {
            prev.set("view", "active");
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
      initialThreadAutoSelectRef.current ||
      querySessionKey ||
      queryHistoryPath ||
      threads.length === 0
    ) {
      return;
    }
    initialThreadAutoSelectRef.current = true;
    handleOpenThread(threads[0]);
  }, [handleOpenThread, queryHistoryPath, querySessionKey, threads]);

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
          setComposerMode(undefined);
          setActiveCollaborationMode(undefined);
          pendingInstantScrollRef.current = true;
          setSearchParams(
            (prev) => {
              prev.set("aiSection", "agent-chat");
              prev.set("agentSection", "chat");
              prev.delete("session");
              prev.delete("view");
              prev.delete("historyPath");
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
    () => createAgentChatStyles(isCompact, isNarrow, threadRailCollapsed, token),
    [isCompact, isNarrow, threadRailCollapsed, token],
  );

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
    const message =
      queuesRunningInput && !content.startsWith("/q ")
        ? `/q ${content}`
        : effectiveMode === "remove"
          ? content
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
      meta: "Bifrost Agent",
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
          setTelemetry((prev) => reduceTelemetry(prev, event));
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
      if (selectedSessionKeyRef.current === submitSessionKey) {
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
        const reader = new FileReader();
        reader.onload = () => {
          const previewUrl = String(reader.result || "");
          if (!previewUrl.startsWith("data:")) {
            return;
          }
          const data = previewUrl.split(",", 2)[1] || "";
          setPendingImages((prev) => {
            if (prev.length >= MAX_PASTED_IMAGES) {
              return prev;
            }
            return [
              ...prev,
              {
                id: `image-${Date.now()}-${Math.random().toString(36).slice(2)}`,
                mimeType: file.type || "image/png",
                data,
                previewUrl,
                name: file.name || undefined,
                size: file.size,
              },
            ];
          });
        };
        reader.readAsDataURL(file);
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

  const handleSend = async (options?: { contentOverride?: string; silentCommand?: boolean }) => {
    const rawContent = (options?.contentOverride ?? draft).trim();
    const imagesForSend = options?.contentOverride ? [] : pendingImages;
    if ((!rawContent && imagesForSend.length === 0) || supplementSubmitting) {
      return;
    }
    if (running) {
      await handleRunningInput(rawContent);
      return;
    }
    const parsedPlanSlash = builtInAgentCommandsSupported
      ? parseAgentPlanSlash(rawContent)
      : { message: rawContent };
    const collaborationMode =
      builtInAgentCommandsSupported
        ? parsedPlanSlash.collaborationMode || composerMode
        : undefined;
    const content = parsedPlanSlash.message.trim();
    const silentCommand =
      builtInAgentCommandsSupported &&
      (options?.silentCommand === true || rawContent === "/compact");
    if (!content && imagesForSend.length === 0) {
      if (collaborationMode === "plan" && imagesForSend.length === 0) {
        antdMessage.warning("Type a task to start Plan Mode.");
        setComposerMode("plan");
      }
      return;
    }
    if (slashRunner && !running && !silentCommand) {
      if (imagesForSend.length > 0) {
        antdMessage.warning("Slash runner calls do not support image attachments yet. Select the runner and send from chat instead.");
        return;
      }
      await handleRunnerCall(content, slashRunner);
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
      role: "assistant",
      content: silentCommand ? "" : collaborationMode === "plan" ? "Planning..." : "Agent is running...",
      timestamp: Date.now() / 1000,
      meta: "Bifrost Agent",
      processSteps: silentCommand
        ? [
            {
              type: "compaction",
              summary: "上下文正在自动压缩",
              status: "running",
            },
          ]
        : collaborationMode === "plan"
          ? [
              {
                type: "status",
                summary: "Plan Mode: drafting an implementation plan",
                status: "running",
              },
            ]
          : undefined,
    };
    pendingInstantScrollRef.current = true;
    setMessages((prev) =>
      silentCommand ? [...prev, assistantMessage] : [...prev, userMessage, assistantMessage],
    );
    setDraft("");
    setComposerMode(undefined);
    setActiveCollaborationMode(collaborationMode);
    setPendingImages([]);
    setRunning(true);
    setTelemetry({
      phase: "running",
      status: {
        work_dir: workDir || undefined,
        runner_id: runnerId === "bifrost_agent" ? undefined : runnerId,
        runner_type: runnerId === "bifrost_agent" ? "bifrost_agent" : selectedRunnerAdapter(runnerOptions, runnerId),
      },
      plan: [],
      tools: [],
      errors: [],
    });
    // Ensure the current session is visible in the threads list with first message as fallback title
    setThreads((prev) => {
      const fallbackTitle = silentCommand
        ? currentSessionFallbackTitle || "Context compaction"
        : userVisibleContent.length > 40 ? `${userVisibleContent.slice(0, 40)}…` : userVisibleContent;
      return dedupeThreads([
        {
          session_key: sessionKey,
          status: "active",
          title: fallbackTitle,
          source: "admin-api",
          start_time: Math.floor(Date.now() / 1000),
          last_active_time: Math.floor(Date.now() / 1000),
          duration_secs: 0,
          runner_id: runnerId === "bifrost_agent" ? undefined : runnerId,
          runner_type:
            runnerId === "bifrost_agent"
              ? "bifrost_agent"
              : selectedRunnerAdapter(runnerOptions, runnerId),
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
    let assistantSegmentHasSteps = silentCommand;
    let assistantSegmentHasProposedPlan = false;
    let nextAssistantDeltaStartsSegment = false;
    try {

      const appendAssistantSegment = (initialContent = "") => {
        const previousSegmentId = assistantSegmentId;
        assistantSegmentIndex += 1;
        assistantSegmentId = `assistant-${Date.now()}-${assistantSegmentIndex}`;
        assistantSegmentHasText = initialContent.trim().length > 0;
        assistantSegmentHasSteps = false;
        nextAssistantDeltaStartsSegment = false;
        const message: ChatMessage = {
          id: assistantSegmentId,
          role: "assistant",
          content: initialContent,
          timestamp: Date.now() / 1000,
          meta: "Bifrost Agent",
        };
        setMessages((prev) => [
          ...prev.map((item) =>
            item.id === previousSegmentId ? { ...item, hideTimestamp: true } : item,
          ),
          message,
        ]);
      };

      const appendAssistantDelta = (delta: string) => {
        if (!delta) {
          return;
        }
        if (
          nextAssistantDeltaStartsSegment &&
          (assistantSegmentHasText || assistantSegmentHasSteps)
        ) {
          appendAssistantSegment();
        }
        const targetId = assistantSegmentId;
        const segmentHadText = assistantSegmentHasText;
        setMessages((prev) =>
          prev.map((message) => {
            if (message.id !== targetId) {
              return message;
            }
            const nextContent =
              !segmentHadText &&
              (message.content === "Agent is running..." || message.content === "Planning...")
                ? delta
                : `${message.content}${delta}`;
            return {
              ...message,
              content: nextContent,
            };
          }),
        );
        assistantSegmentHasText =
          assistantSegmentHasText || delta.trim().length > 0;
        nextAssistantDeltaStartsSegment = false;
      };

      const appendProcessStep = (step: ProcessStep) => {
        const targetId = assistantSegmentId;
        const segmentHadText = assistantSegmentHasText;
        setMessages((prev) =>
          prev.map((message) =>
            message.id === targetId
              ? (() => {
                  const processSteps = [...(message.processSteps || [])];
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
                    processSteps.push(step);
                  }
                  return {
                    ...message,
                    content:
                      !segmentHadText &&
                      (message.content === "Agent is running..." || message.content === "Planning...")
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

      const finishSilentCommandCompaction = (
        status: "success" | "failed",
        summary: string,
      ) => {
        setMessages((prev) =>
          prev.map((message) => {
            if (message.id !== assistantId) {
              return message;
            }
            const processSteps = [...(message.processSteps || [])];
            const runningCompactionIndex = processSteps.findIndex(
              (item) => item.type === "compaction" && item.status === "running",
            );
            if (runningCompactionIndex < 0) {
              return message;
            }
            processSteps[runningCompactionIndex] = {
              ...processSteps[runningCompactionIndex],
              status,
              summary,
            };
            return { ...message, content: "", processSteps };
          }),
        );
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
        nextAssistantDeltaStartsSegment = true;
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
        runnerId,
        runnerAdapter: selectedRunnerAdapter(runnerOptions, runnerId),
        collaborationMode,
        signal: abortController.signal,
        onEvent: (event) => {
          if (selectedSessionKeyRef.current !== sendSessionKey) return;
          setTelemetry((prev) => reduceTelemetry(prev, event));
          if (silentCommand) {
            const step = eventToProcessStep(event);
            if (step?.type === "compaction") {
              appendProcessStep(step);
            }
            return;
          }
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
          if (
            event.eventType === "assistant_delta" &&
            typeof event.content === "string" &&
            runnerId === "bifrost_agent"
          ) {
            appendAssistantDelta(event.content);
            return;
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
          if (silentCommand) {
            finishSilentCommandCompaction("success", "上下文已自动压缩");
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
            ? silentCommand
              ? {
                  ...message,
                  content: "",
                  processSteps: [
                    ...(message.processSteps || []).filter(
                      (step) =>
                        !(step.type === "compaction" && step.status === "running"),
                    ),
                    {
                      type: "compaction",
                      summary: "上下文压缩失败",
                      status: "failed",
                      result: text,
                    },
                  ],
                }
              : {
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
        setActiveCollaborationMode(undefined);
      }
      if (streamAbortRef.current === abortController) {
        streamAbortRef.current = null;
      }
    }
  };

  const handleSlashCommand = (option: SlashCommandOption) => {
    setSlashRunner(undefined);
    if (option.value === "plan") {
      setComposerMode("plan");
      setDraft("");
      setTimeout(() => {
        const input = document.querySelector<HTMLTextAreaElement>(
          '[data-testid="agent-chat-input"]',
        );
        input?.focus();
      }, 0);
      return;
    }
    if (option.action === "insert") {
      setDraft(option.insertText || `${option.command} `);
      setTimeout(() => {
        const input = document.querySelector<HTMLTextAreaElement>(
          '[data-testid="agent-chat-input"]',
        );
        input?.focus();
      }, 0);
      return;
    }
    void handleSend({ contentOverride: option.command, silentCommand: true });
  };

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
      setSlashActiveIndex(0);
      return;
    }
    setSlashActiveIndex((index) => Math.min(index, slashOptionCount - 1));
  }, [showSlashRunnerPanel, slashOptionCount, slashOptionKey]);

  const selectActiveSlashOption = useCallback(() => {
    if (!showSlashRunnerPanel || slashOptionCount <= 0) {
      return false;
    }
    if (slashActiveIndex < slashCommandOptions.length) {
      const option = slashCommandOptions[slashActiveIndex];
      if (!option) {
        return false;
      }
      handleSlashCommand(option);
      return true;
    }
    const runner = slashRunnerOptions[slashActiveIndex - slashCommandOptions.length];
    if (!runner) {
      return false;
    }
    setSlashRunner(runner);
    setComposerMode(undefined);
    setDraft("");
    return true;
  }, [
    showSlashRunnerPanel,
    slashActiveIndex,
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
        setSlashActiveIndex((index) => (index + 1) % slashOptionCount);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        setSlashActiveIndex((index) => (index - 1 + slashOptionCount) % slashOptionCount);
        return;
      }
      if (event.key === "Enter" && !event.shiftKey) {
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
        runner_id:
          runnerCall.targetRunnerId === "bifrost_agent"
            ? undefined
            : runnerCall.targetRunnerId,
        runner_type:
          runnerCall.targetRunnerId === "bifrost_agent"
            ? "bifrost_agent"
            : runnerCall.targetAdapter || runnerCall.targetRunnerId,
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
                {activeCollaborationMode === "plan" ? (
                  <Tag color="gold" data-testid="agent-chat-active-plan-mode">
                    Plan Mode
                  </Tag>
                ) : null}
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
              <Button
                size="small"
                type="primary"
                icon={<PlusOutlined />}
                data-testid="agent-chat-new"
                onClick={handleOpenNewChat}
              >
                New Chat
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
              {historyHasOlder ? (
                <div style={{ display: "flex", justifyContent: "center", padding: "4px 0 8px" }}>
                  <Button
                    size="small"
                    loading={historyLoadingOlder}
                    onClick={loadOlderHistoryPage}
                    data-testid="agent-chat-load-older"
                  >
                    Load older
                  </Button>
                </div>
              ) : null}
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
                <Space direction="vertical" size={6} style={{ width: "100%" }}>
                  <AgentChatPlan
                    plan={telemetry.plan}
                    styles={styles}
                    successColor={token.colorSuccess}
                    primaryColor={token.colorPrimary}
                  />
                  <AgentChatPromptChips prompts={PROMPT_CHIPS} onSelect={setDraft} />
                  {queuedInputs.length > 0 ? (
                    <div style={styles.queuePanel} data-testid="agent-chat-queue-panel">
                      <Space direction="vertical" size={6} style={{ width: "100%" }}>
                        <Text type="secondary" style={{ fontSize: 12 }}>
                          Queued
                        </Text>
                        {queuedInputs.map((item) => (
                          <Row
                            key={item.seq}
                            align="middle"
                            justify="space-between"
                            gutter={8}
                          >
                            <Col flex="auto">
                              <Text ellipsis>
                                #{item.seq} {item.message}
                              </Text>
                            </Col>
                            <Col>
                              <Space size={4}>
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
                              </Space>
                            </Col>
                          </Row>
                        ))}
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
                  onActiveIndexChange={setSlashActiveIndex}
                  onSelectCommand={(option) => {
                    handleSlashCommand(option);
                  }}
                  onSelect={(option) => {
                    setSlashRunner(option);
                    setComposerMode(undefined);
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
                {pendingImages.length > 0 ? (
                  <div style={styles.imagePreviewStrip} data-testid="agent-chat-image-preview-strip">
                    {pendingImages.map((image, index) => (
                      <div key={image.id} style={styles.imagePreviewItem} data-testid="agent-chat-image-preview">
                        <img
                          src={image.previewUrl}
                          alt={image.name || `Pasted image ${index + 1}`}
                          style={styles.imagePreviewThumb}
                        />
                        <Button
                          size="small"
                          type="text"
                          icon={<DeleteOutlined />}
                          aria-label={`Remove pasted image ${index + 1}`}
                          style={styles.imagePreviewRemove}
                          onClick={() =>
                            setPendingImages((prev) => prev.filter((item) => item.id !== image.id))
                          }
                        />
                        <span style={styles.imagePreviewMeta}>{imageSizeLabel(image.size)}</span>
                      </div>
                    ))}
                  </div>
                ) : null}
                <TextArea
                  data-testid="agent-chat-input"
                  data-session-key={sessionKey}
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                  onPaste={handlePasteImages}
                  onKeyDown={handleComposerKeyDown}
                  placeholder={
                    composerMode === "plan"
                      ? "Describe what should be planned..."
                      : "Describe a task for the Agent..."
                  }
                  autoSize={{ minRows: 2, maxRows: 7 }}
                  style={{
                    padding: slashRunner || composerMode === "plan"
                      ? "42px 56px 30px 14px"
                      : "8px 56px 30px 14px",
                    border: "none",
                    boxShadow: "none",
                    outline: "none",
                    background: "transparent",
                    resize: "none",
                  }}
                />
                {composerMode === "plan" ? (
                  <Tag
                    color="gold"
                    data-testid="agent-chat-plan-mode-pill"
                    style={styles.planModePill}
                    closable
                    onClose={(event) => {
                      event.preventDefault();
                      setComposerMode(undefined);
                    }}
                  >
                    <BulbOutlined style={{ marginRight: 4 }} />
                    Plan Mode
                  </Tag>
                ) : null}
                <Text style={styles.inputHint} data-testid="agent-chat-input-hint">
                  {composerMode === "plan"
                    ? "Planning only. Shift + Enter for a new line"
                    : "Shift + Enter for a new line"}
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

        {threadRailCollapsed ? (
          <Button
            shape="circle"
            icon={<LeftOutlined />}
            aria-label="Expand threads"
            title="Expand threads"
            data-testid="agent-chat-threads-expand"
            style={styles.threadRailExpandButton}
            onClick={() => setThreadRailCollapsed(false)}
          />
        ) : (
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
        )}

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
