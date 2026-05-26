import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useSearchParams } from "react-router-dom";
import {
  Button,
  Card,
  Col,
  Empty,
  Grid,
  Input,
  Modal,
  Row,
  Segmented,
  Select,
  Space,
  Tag,
  Typography,
  message as antdMessage,
  theme,
} from "antd";
import {
  CheckCircleOutlined,
  DeleteOutlined,
  DownOutlined,
  FolderOpenOutlined,
  BorderOutlined,
  LoadingOutlined,
  PlusOutlined,
  RightOutlined,
  RobotOutlined,
  SendOutlined,
  SettingOutlined,
} from "@ant-design/icons";
import { apiFetch } from "../../api/apiFetch";
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
  historyEventsToMessages,
  historyEventsToTelemetry,
  isRecord,
  isRealChatMessage,
  isSelectedThread,
  reduceTelemetry,
  runAgentStream,
  selectedRunnerAdapter,
  sessionDetailToMessages,
  stringFrom,
  telemetryFromSessionDetail,
  telemetryFromThread,
  type AgentThreadSummary,
  type ChatMessage,
  type HistoryEvent,
  type ProcessStep,
  type RunnerConfigPayload,
  type RunnerOption,
  type RunTelemetry,
  type SessionDetail,
} from "./AgentChatSection.helpers";
import { AgentChatMessageList } from "./AgentChatSection.messages";
import { AgentChatSettingsModal, AgentThreadListCard } from "./AgentChatSection.panels";
import { queueItemsFromEvent, type QueuedInput } from "./AgentChatSection.queue";
import {
  SelectedRunnerPill,
  SlashRunnerPanel,
  useRunnerCallHandler,
  useSlashRunnerSelection,
} from "./AgentChatSection.runnerCall";
import { createAgentChatStyles } from "./AgentChatSection.styles";

const { Text } = Typography;
const { TextArea } = Input;
const { useBreakpoint } = Grid;

export default function AgentChatSection() {
  const [searchParams, setSearchParams] = useSearchParams();
  const { token } = theme.useToken();
  const screens = useBreakpoint();
  const isCompact = !screens.lg;
  const [draft, setDraft] = useState("");
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
  const {
    slashRunner,
    setSlashRunner,
    slashRunnerOptions,
    showSlashRunnerPanel,
  } = useSlashRunnerSelection({
    draft,
    running,
    supplementSubmitting,
    runnerId,
    runnerOptions,
  });
  const [planCollapsed, setPlanCollapsed] = useState(false);
  const [defaultWorkDir, setDefaultWorkDir] = useState("");
  const messagesScrollRef = useRef<HTMLDivElement>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const pendingInstantScrollRef = useRef(true);
  const userNearBottomRef = useRef(true);
  const loadedConversationKeyRef = useRef<string | undefined>(undefined);
  const threadsRef = useRef<AgentThreadSummary[]>([]);
  const initialThreadAutoSelectRef = useRef(false);

  const scrollMessagesToBottom = useCallback(() => {
    const element = messagesScrollRef.current;
    if (!element) {
      return;
    }
    element.scrollTop = element.scrollHeight;
  }, []);

  const scheduleMessagesBottomScroll = useCallback(() => {
    scrollMessagesToBottom();
    requestAnimationFrame(() => {
      scrollMessagesToBottom();
      requestAnimationFrame(scrollMessagesToBottom);
    });
    window.setTimeout(scrollMessagesToBottom, 80);
    window.setTimeout(scrollMessagesToBottom, 240);
  }, [scrollMessagesToBottom]);

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
    const element = messagesScrollRef.current;
    if (!element) {
      return;
    }
    const distanceFromBottom =
      element.scrollHeight - element.scrollTop - element.clientHeight;
    userNearBottomRef.current = distanceFromBottom < 96;
  }, []);

  // Compute fallback title for the current session from first user message
  const currentSessionFallbackTitle = useMemo(() => {
    const firstUserMsg = messages.find((m) => m.role === "user");
    if (!firstUserMsg) return undefined;
    const text = firstUserMsg.content.trim();
    return text.length > 40 ? `${text.slice(0, 40)}…` : text;
  }, [messages]);

  const titleFromMessages = useCallback((items: ChatMessage[]) => {
    const firstUserMsg = items.find((message) => message.role === "user");
    const text = firstUserMsg?.content.trim();
    if (!text) return undefined;
    return text.length > 40 ? `${text.slice(0, 40)}…` : text;
  }, []);

  const sameMessages = useCallback((left: ChatMessage[], right: ChatMessage[]) =>
    left.length === right.length &&
    left.every((message, index) => {
      const other = right[index];
      return (
        message.role === other.role &&
        message.content === other.content &&
        message.meta === other.meta
      );
    }), []);

  const replaceLoadedMessages = useCallback((restored: ChatMessage[], shouldStickToBottom: boolean) => {
    const fallbackTimestamp = Date.now() / 1000;
    const withTimestamps = restored.map((message) => ({
      ...message,
      timestamp: message.timestamp || fallbackTimestamp,
    }));
    setMessages((prev) => {
      if (sameMessages(prev, withTimestamps)) {
        return prev;
      }
      if (shouldStickToBottom) {
        pendingInstantScrollRef.current = true;
        userNearBottomRef.current = true;
      }
      return withTimestamps;
    });
  }, [sameMessages]);

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
  const currentStateTag = formatCurrentStateTag(telemetry, selectedThread, running);
  const guideSupported = runnerId === "bifrost_agent";

  const refreshThreads = useCallback(async () => {
    try {
      const response = await apiFetch("/api/im-gateway/agent/sessions/all");
      if (!response.ok) {
        return;
      }
      const payload = (await response.json()) as { sessions?: AgentThreadSummary[] };
      const incoming = dedupeThreads(payload.sessions || []);
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
    } catch {
      // Keep chat usable even if the session index is temporarily unavailable.
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
  }, [refreshThreads]);

  useEffect(() => {
    threadsRef.current = threads;
  }, [threads]);

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
        .then((detail) => {
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
          if (restored.length > 0) {
            replaceLoadedMessages(restored, shouldStickToBottom);
          }
          const resolvedTitle = matchedThread?.title || detail.title || titleFromMessages(restored);
          setThreads((prev) => {
            let changed = false;
            const next = prev.map((thread) => {
              if (thread.session_key !== nextSessionKey || thread.history_path) {
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
              };
              if (
                updated.title !== thread.title ||
                updated.source !== thread.source ||
                updated.work_dir !== thread.work_dir ||
                updated.agent_type !== thread.agent_type ||
                updated.runner_type !== thread.runner_type ||
                updated.runner_id !== thread.runner_id
              ) {
                changed = true;
                return updated;
              }
              return thread;
            });
            return changed ? next : prev;
          });
          setTelemetry(telemetryFromSessionDetail(detail, matchedThread));
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
    apiFetch(
      `/api/im-gateway/agent/sessions/history/${encodeURIComponent(
        nextHistoryPath,
      )}`,
    )
      .then(async (response) => {
        if (!response.ok) {
          throw new Error(await response.text());
        }
        return response.json() as Promise<{ events?: HistoryEvent[] }>;
      })
      .then((payload) => {
        if (cancelled) {
          return;
        }
        const matchedThread = threadsRef.current.find((thread) =>
          isSelectedThread(thread, nextSessionKey || "", nextHistoryPath, "history"),
        );
        const restored = historyEventsToMessages(payload.events || []);
        if (restored.length > 0) {
          replaceLoadedMessages(restored, shouldStickToBottom);
          const resolvedTitle = matchedThread?.title || titleFromMessages(restored);
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
          setTelemetry((prev) =>
            historyEventsToTelemetry(payload.events || [], matchedThread, prev),
          );
          setRunnerId(matchedThread?.runner_id || "bifrost_agent");
          const eventSessionKey =
            payload.events?.find((event) => event.session_key)?.session_key;
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
    queryHistoryPath,
    querySessionKey,
    replaceLoadedMessages,
    setSearchParams,
    titleFromMessages,
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
      setSessionKey(thread.session_key);
      setHistoryPath(thread.history_path);
      setDraft("");
      setTelemetry(telemetryFromThread(thread));
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
    () => createAgentChatStyles(isCompact, token),
    [isCompact, token],
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
    const isExternalRunner = runnerId !== "bifrost_agent";
    const rendersMessage = mode === "guide" || mode === "stop";
    const message =
      mode === "queue" && !content.startsWith("/q ")
        ? `/q ${content}`
        : mode === "remove"
          ? content
          : content;
    const userMessage: ChatMessage = {
      id: `user-${Date.now()}`,
      role: "user",
      content: mode === "stop" ? "/stop" : content,
      timestamp: Date.now() / 1000,
      meta:
        mode === "stop"
          ? "Control"
          : mode === "queue" || isExternalRunner
            ? "Queued user"
            : "Guide user",
    };
    const assistantId = `assistant-${Date.now()}`;
    const assistantMessage: ChatMessage = {
      id: assistantId,
      role: "assistant",
      content:
        mode === "stop"
          ? "Stopping..."
          : mode === "queue" || isExternalRunner
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
    setSupplementSubmitting(true);
    try {
      await runAgentStream({
        message,
        sessionKey,
        historyPath,
        workDir: workDir || undefined,
        runnerId,
        runnerAdapter: selectedRunnerAdapter(runnerOptions, runnerId),
        onEvent: (event) => {
          setTelemetry((prev) => reduceTelemetry(prev, event));
          applyQueueEvent(event);
        },
        onDelta: () => {},
        onFinal: (response) => {
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
      refreshThreads();
    } catch (error) {
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
      setSupplementSubmitting(false);
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

  const handleSend = async () => {
    const content = draft.trim();
    if (!content || supplementSubmitting) {
      return;
    }
    if (slashRunner && !running) {
      await handleRunnerCall(content, slashRunner);
      return;
    }
    if (running) {
      await handleRunningInput(content);
      return;
    }
    const userMessage: ChatMessage = {
      id: `user-${Date.now()}`,
      role: "user",
      content,
      timestamp: Date.now() / 1000,
      meta: "You",
    };
    const assistantId = `assistant-${Date.now()}`;
    const assistantMessage: ChatMessage = {
      id: assistantId,
      role: "assistant",
      content: "Agent is running...",
      timestamp: Date.now() / 1000,
      meta: "Bifrost Agent",
    };
    pendingInstantScrollRef.current = true;
    setMessages((prev) => [...prev, userMessage, assistantMessage]);
    setDraft("");
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
      const fallbackTitle = content.length > 40 ? `${content.slice(0, 40)}…` : content;
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
    try {
      // Buffer for intermediate thinking text between tool calls
      let thinkingBuffer = "";
      await runAgentStream({
        message: content,
        sessionKey,
        historyPath,
        workDir: workDir || undefined,
        runnerId,
        runnerAdapter: selectedRunnerAdapter(runnerOptions, runnerId),
        onEvent: (event) => {
          setTelemetry((prev) => reduceTelemetry(prev, event));
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
          // Accumulate assistant_delta into thinking buffer (not message.content)
          if (event.eventType === "assistant_delta" && typeof event.content === "string") {
            thinkingBuffer += event.content;
            return;
          }
          // When tool_started arrives, flush thinking buffer as a thinking step, then add tool step
          if (event.eventType === "tool_started") {
            const newSteps: ProcessStep[] = [];
            if (thinkingBuffer.trim()) {
              newSteps.push({
                type: "thinking",
                summary: thinkingBuffer.trim(),
                status: "success",
              });
              thinkingBuffer = "";
            }
            const toolStep = eventToProcessStep(event);
            if (toolStep) newSteps.push(toolStep);
            if (newSteps.length > 0) {
              setMessages((prev) =>
                prev.map((message) =>
                  message.id === assistantId
                    ? {
                        ...message,
                        processSteps: [...(message.processSteps || []), ...newSteps],
                      }
                    : message,
                ),
              );
            }
            return;
          }
          // When tool_finished arrives, update the last matching running tool step with result and status
          if (event.eventType === "tool_finished" && isRecord(event.log)) {
            const log = event.log as Record<string, unknown>;
            const toolName = stringFrom(log.tool_name) || stringFrom(log.toolName) || "tool";
            const success = log.success !== false;
            const toolResult = stringFrom(log.result);
            setMessages((prev) =>
              prev.map((message) => {
                if (message.id !== assistantId) return message;
                // Find the last running tool step matching this tool name and update it
                const steps = [...(message.processSteps || [])];
                for (let i = steps.length - 1; i >= 0; i--) {
                  if (steps[i].type === "tool" && steps[i].status === "running" && steps[i].summary.startsWith(toolName)) {
                    steps[i] = {
                      ...steps[i],
                      status: success ? "success" : "failed",
                      result: toolResult || undefined,
                    };
                    break;
                  }
                }
                return { ...message, processSteps: steps };
              }),
            );
            return;
          }
          // Build process steps for other event types (plan_updated, compaction, etc.)
          const step = eventToProcessStep(event);
          if (step) {
            setMessages((prev) =>
              prev.map((message) =>
                message.id === assistantId
                  ? {
                      ...message,
                      processSteps: [...(message.processSteps || []), step],
                    }
                  : message,
              ),
            );
          }
        },
        onDelta: () => {
          // No-op: deltas are accumulated as thinking steps in onEvent
        },
        onFinal: (response) => {
          // Flush any remaining thinking buffer (from the final loop iteration before tools)
          // The final response from assistant_final/turn_finished is the real output
          setMessages((prev) =>
            prev.map((message) =>
              message.id === assistantId
                ? { ...message, content: response || message.content }
                : message,
            ),
          );
        },
      });
      setHistoryPath(undefined);
      setQueuedInputs([]);
      setSearchParamsForActiveSession();
      refreshThreads();
    } catch (error) {
      const text = error instanceof Error ? error.message : "Agent run failed";
      setTelemetry((prev) => ({
        ...prev,
        phase: "failed",
        errors: prev.errors.includes(text) ? prev.errors : [...prev.errors, text],
      }));
      setMessages((prev) =>
        prev.map((message) =>
          message.id === assistantId ? { ...message, content: text } : message,
        ),
      );
      antdMessage.error(text);
    } finally {
      setRunning(false);
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
                disabled={running}
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
                  running={running}
                  styles={styles}
                  token={token}
                />
              )}
            </div>
            <div ref={messagesEndRef} />

            <div style={styles.composer}>
              <div style={styles.composerTrack} data-testid="agent-chat-composer-track">
                <Space direction="vertical" size={10} style={{ width: "100%" }}>
              {telemetry.plan.length > 0 ? (
                <div data-testid="agent-chat-plan" style={styles.planPanel}>
                  <button
                    type="button"
                    onClick={() => setPlanCollapsed((value) => !value)}
                    data-testid="agent-chat-plan-toggle"
                    style={{
                      ...styles.planHeader,
                      border: 0,
                      background: "transparent",
                      cursor: "pointer",
                      color: token.colorText,
                    }}
                  >
                    <span style={styles.planHeaderLabel}>
                      <CheckCircleOutlined />
                      <Text strong style={{ fontSize: 13, lineHeight: "18px" }}>
                        Plan
                      </Text>
                      <Tag style={styles.planCountTag}>{telemetry.plan.length}</Tag>
                    </span>
                    {planCollapsed ? <RightOutlined /> : <DownOutlined />}
                  </button>
                  {!planCollapsed ? (
                    <div style={styles.planBody}>
                      <div data-testid="agent-chat-plan-list" style={styles.planList}>
                        {telemetry.plan.map((step, index) => (
                          <div
                            key={`${index}-${step.step}`}
                            data-testid="agent-chat-plan-item"
                            style={styles.planItem}
                          >
                            {step.status === "completed" ? (
                              <CheckCircleOutlined
                                aria-label="completed"
                                data-testid="agent-chat-plan-status-completed"
                                style={{
                                  ...styles.planStatusIcon,
                                  color: token.colorSuccess,
                                }}
                              />
                            ) : step.status === "in_progress" ? (
                              <LoadingOutlined
                                spin
                                aria-label="in progress"
                                data-testid="agent-chat-plan-status-in-progress"
                                style={{
                                  ...styles.planStatusIcon,
                                  color: token.colorPrimary,
                                }}
                              />
                            ) : (
                              <span
                                aria-label="pending"
                                data-testid="agent-chat-plan-status-pending"
                                style={styles.planPendingIcon}
                              />
                            )}
                            <Text title={step.step} style={styles.planStepText}>
                              {step.step}
                            </Text>
                          </div>
                        ))}
                      </div>
                    </div>
                  ) : null}
                </div>
              ) : null}
              {PROMPT_CHIPS.length > 0 ? (
                <Space wrap data-testid="agent-chat-prompt-chips">
                  {PROMPT_CHIPS.map((prompt) => (
                    <Button
                      key={prompt}
                      size="small"
                      onClick={() => setDraft(prompt)}
                    >
                      {prompt}
                    </Button>
                    ))}
                </Space>
              ) : null}
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
                  options={slashRunnerOptions}
                  styles={styles}
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
                <TextArea
                  data-testid="agent-chat-input"
                  data-session-key={sessionKey}
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                  onPressEnter={(event) => {
                    if (!event.shiftKey) {
                      event.preventDefault();
                      handleSend();
                    }
                  }}
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
                  onClick={running && !draft.trim() ? handleStop : handleSend}
                  disabled={
                    supplementSubmitting || (!draft.trim() && !running)
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
          />
        </div>

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
