import { useState } from "react";
import { Col, Row, Typography, theme } from "antd";
import {
  CheckCircleOutlined,
  DownOutlined,
  ExclamationCircleOutlined,
  LoadingOutlined,
  RightOutlined,
} from "@ant-design/icons";
import { apiFetch } from "../../api/apiFetch";

const { Text } = Typography;

export type ProcessStep = {
  type: "tool" | "plan" | "status" | "compaction" | "thinking";
  summary: string;
  detail?: string;
  /** Tool arguments JSON string */
  args?: string;
  /** Tool result string */
  result?: string;
  status?: "running" | "success" | "failed";
};

export type ChatMessage = {
  id: string;
  role: "user" | "assistant";
  content: string;
  timestamp?: number;
  meta?: string;
  processSteps?: ProcessStep[];
  processCollapsed?: boolean;
  hideTimestamp?: boolean;
  runnerCall?: RunnerCallMessageMeta;
};

export type RunnerCallMessageMeta = {
  callId?: string;
  targetRunnerId: string;
  targetRunnerLabel: string;
  targetAdapter?: string;
  childSessionKey?: string;
  status: "running" | "success" | "failed";
};

export type HistoryEvent = {
  timestamp: number;
  event_type: string;
  session_key: string;
  content: Record<string, unknown>;
};

export type AgentThreadSummary = {
  session_key: string;
  status: "active" | "ended";
  running?: boolean;
  state?: string;
  source?: string;
  agent_type?: string;
  runner_type?: string;
  runner_id?: string;
  title?: string;
  history_path?: string;
  start_time?: number;
  last_active_time?: number;
  duration_secs?: number;
  work_dir?: string;
  turns?: number;
  tokens?: number;
  compaction_count?: number;
  estimated_tokens?: number;
  context_window_tokens?: number;
  context_usage_percent?: number;
  last_response_tokens?: number;
  has_timeline?: boolean;
  timeline_event_count?: number;
  run_state?: string;
};

export type RunnerConfigPayload = {
  defaultRunnerId?: string;
  default_runner_id?: string;
  runners?: Record<string, { adapter?: string; enabled?: boolean }>;
};

export type RunnerOption = {
  label: string;
  value: string;
  adapter?: string;
};

export type SessionDetailMessage = {
  role: string;
  content: string;
  timestamp?: number;
};

export type SessionDetail = {
  session_key: string;
  title?: string;
  source?: string;
  state?: string;
  work_dir?: string;
  message_count?: number;
  total_tokens_used?: number;
  estimated_tokens?: number;
  context_window_tokens?: number;
  context_usage_percent?: number;
  last_response_tokens?: number;
  compaction_count?: number;
  history_version?: number;
  agent_type?: string;
  runner_type?: string;
  runner_id?: string;
  history_path?: string;
  has_timeline?: boolean;
  timeline_event_count?: number;
  run_state?: string;
  active_status?: RunStatusSnapshot;
  messages?: SessionDetailMessage[];
};

export type PlanStepStatus = "pending" | "in_progress" | "completed" | string;

export type PlanStep = {
  step: string;
  status: PlanStepStatus;
};

export type ToolRun = {
  id: string;
  name: string;
  status: "running" | "success" | "failed";
  arguments?: string;
  result?: string;
  durationMs?: number;
};

export type RunStatusSnapshot = {
  state?: string;
  source?: string;
  current_loop_iteration?: number;
  completed_loop_iterations?: number;
  max_loop_iterations?: number;
  last_response_tokens?: number;
  total_tokens_used?: number;
  estimated_context_tokens?: number;
  context_window_tokens?: number;
  context_usage_percent?: number;
  compaction_count?: number;
  history_version?: number;
  work_dir?: string;
  message_count?: number;
  local_tool_count?: number;
  mcp_tool_count?: number;
  pending_guide_messages?: string[];
  agent_type?: string;
  runner_type?: string;
  runner_id?: string;
};

export type AgentContextSnapshot = {
  estimatedContextTokens?: number;
  contextWindowTokens?: number;
  contextUsagePercent?: number;
  compactionCount?: number;
  historyVersion?: number;
  messageCount?: number;
  userTurnCount?: number;
  lastResponseTokens?: number;
  totalTokensUsed?: number;
};

export type CompactionProgress = {
  trigger?: string;
  reason?: string;
  phase?: string;
  preTokens?: number;
  postTokens?: number;
  tokensSaved?: number;
  messagesRemoved?: number;
  durationMs?: number;
  compactionCount?: number;
  historyVersion?: number;
  context?: AgentContextSnapshot;
};

export type RunTelemetry = {
  phase: "idle" | "running" | "finished" | "failed";
  title?: string;
  status?: RunStatusSnapshot;
  context?: AgentContextSnapshot;
  compaction?: CompactionProgress;
  compactionPhase?: "started" | "finished" | "failed";
  plan: PlanStep[];
  tools: ToolRun[];
  errors: string[];
};

export const EMPTY_TELEMETRY: RunTelemetry = {
  phase: "idle",
  plan: [],
  tools: [],
  errors: [],
};

export const STARTER_MESSAGES: ChatMessage[] = [];

export const PROMPT_CHIPS: string[] = [];

export const ACTIVITY_ITEMS = [
  { label: "Context", value: "Workspace indexed" },
  { label: "Mode", value: "Plan first" },
  { label: "Tools", value: "Files, terminal, browser" },
];

export function sameChatMessages(left: ChatMessage[], right: ChatMessage[]) {
  return (
    left.length === right.length &&
    left.every((message, index) => {
      const other = right[index];
      return (
        message.role === other.role &&
        message.content === other.content &&
        message.meta === other.meta &&
        JSON.stringify(message.processSteps || []) ===
          JSON.stringify(other.processSteps || [])
      );
    })
  );
}

export function titleFromChatMessages(items: ChatMessage[]) {
  const firstUserMsg = items.find((message) => message.role === "user");
  const text = firstUserMsg?.content.trim();
  if (!text) return undefined;
  return text.length > 40 ? `${text.slice(0, 40)}…` : text;
}

export function sessionDetailToMessages(detail: SessionDetail): ChatMessage[] {
  return (detail.messages || [])
    .filter((message) => message.role === "user" || message.role === "assistant")
    .map((message, index, sourceMessages) => {
      const role = message.role === "user" ? ("user" as const) : ("assistant" as const);
      const runnerCall = inferPersistedRunnerCall(
        detail.session_key,
        role,
        message.content || "",
        sourceMessages[index + 1]?.content || "",
      );
      return {
        id: `session-${detail.session_key}-${index}`,
        role,
        content: runnerCall?.content ?? message.content ?? "",
        timestamp: message.timestamp,
        meta: runnerCall ? "Runner call" : role === "user" ? "You" : "Bifrost Agent",
        runnerCall: runnerCall?.meta,
      };
    })
    .filter((message) => message.content.trim().length > 0);
}

function inferPersistedRunnerCall(
  sessionKey: string,
  role: "user" | "assistant",
  content: string,
  nextContent: string,
): { content: string; meta: RunnerCallMessageMeta } | undefined {
  if (isRunnerCallThread({ session_key: sessionKey })) {
    return undefined;
  }
  if (role === "user") {
    const match = content.match(/^Run with ([^:]+):\s*([\s\S]*)$/);
    if (!match) {
      return undefined;
    }
    const targetRunnerId = match[1].trim();
    if (!targetRunnerId) {
      return undefined;
    }
    return {
      content: match[2] || content,
      meta: {
        targetRunnerId,
        targetRunnerLabel: targetRunnerId,
        childSessionKey: `runner-call:${sessionKey}:${targetRunnerId}`,
        status: inferPersistedRunnerCallStatus(targetRunnerId, nextContent),
      },
    };
  }
  const running = content.match(/^Runner `([^`]+)` is running\.\.\.$/);
  if (running) {
    const targetRunnerId = running[1].trim();
    return {
      content: "Runner is running...",
      meta: {
        targetRunnerId,
        targetRunnerLabel: targetRunnerId,
        childSessionKey: `runner-call:${sessionKey}:${targetRunnerId}`,
        status: "running",
      },
    };
  }
  const finished = content.match(/^Runner `([^`]+)` completed this call\.\n\n([\s\S]*)$/);
  if (finished) {
    const targetRunnerId = finished[1].trim();
    return {
      content: finished[2] || content,
      meta: {
        targetRunnerId,
        targetRunnerLabel: targetRunnerId,
        childSessionKey: `runner-call:${sessionKey}:${targetRunnerId}`,
        status: "success",
      },
    };
  }
  return undefined;
}

function inferPersistedRunnerCallStatus(
  targetRunnerId: string,
  assistantContent: string,
): RunnerCallMessageMeta["status"] {
  if (assistantContent === `Runner \`${targetRunnerId}\` is running...`) {
    return "running";
  }
  if (assistantContent.startsWith(`Runner \`${targetRunnerId}\` completed this call.\n\n`)) {
    return "success";
  }
  return "success";
}

export function isSelectedThread(
  thread: AgentThreadSummary,
  sessionKey: string,
  historyPath?: string,
  view?: string,
) {
  if (historyPath) {
    return thread.history_path === historyPath;
  }
  if (view === "history") {
    return thread.session_key === sessionKey && Boolean(thread.history_path);
  }
  return thread.session_key === sessionKey;
}

export function dedupeThreads(threads: AgentThreadSummary[]) {
  const bySession = new Map<string, AgentThreadSummary>();
  for (const thread of threads.filter((item) => !isRunnerCallThread(item))) {
    const existing = bySession.get(thread.session_key);
    if (!existing || threadRank(thread) > threadRank(existing)) {
      bySession.set(thread.session_key, thread);
    }
  }
  return Array.from(bySession.values()).sort(
    (a, b) => (b.last_active_time || 0) - (a.last_active_time || 0),
  );
}

export function isRunnerCallThread(thread: Pick<AgentThreadSummary, "session_key">) {
  return thread.session_key.startsWith("runner-call:");
}

export function threadRank(thread: AgentThreadSummary) {
  const activeScore = thread.status === "active" ? 10_000_000_000 : 0;
  return activeScore + (thread.last_active_time || 0);
}

export function isRealChatMessage(id: string) {
  return !["assistant-welcome", "user-example", "assistant-example"].includes(id);
}

export function formatThreadSource(thread: AgentThreadSummary) {
  const source = (thread.source || "").toLowerCase();
  const agent = (thread.agent_type || "").toLowerCase();
  const values = [source, agent]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
  if (values.includes("asr")) {
    return "ASR Task";
  }
  if (values.includes("schedule") || values.includes("cron") || values.includes("timer")) {
    return "Scheduled";
  }
  if (values.includes("weixin") || values.includes("wechat")) {
    return "WeChat";
  }
  if (values.includes("feishu") || values.includes("lark")) {
    return "Feishu";
  }
  if (values.includes("admin") || values.includes("api") || values.includes("web")) {
    return "Web";
  }
  if (values.includes("im")) {
    return "IM";
  }
  return source ? formatLabel(source) : "Web";
}

export function formatRunnerTag(
  status?: RunStatusSnapshot,
  thread?: AgentThreadSummary,
  selectedRunnerId?: string,
) {
  const runner =
    status?.runner_id ||
    thread?.runner_id ||
    selectedRunnerId ||
    status?.runner_type ||
    thread?.runner_type ||
    status?.agent_type ||
    thread?.agent_type;
  return runner ? `Runner: ${runner}` : "Runner: Default";
}

export function formatThreadRunnerMark(thread: AgentThreadSummary) {
  const explicitRunner = [thread.runner_id, thread.runner_type, thread.agent_type]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
  if (explicitRunner.includes("codex")) {
    return "Cx";
  }
  if (explicitRunner.includes("chatgpt") || explicitRunner.includes("webgpt")) {
    return "GPT";
  }
  if (
    explicitRunner.includes("bifrost_agent") ||
    explicitRunner.includes("builtin") ||
    explicitRunner.includes("bifrost") ||
    explicitRunner.includes("agent")
  ) {
    return "Bf";
  }
  const title = (thread.title || "").toLowerCase();
  if (title.includes("chatgpt web") || title.includes("webgpt")) {
    return "GPT";
  }
  return "Bf";
}

export function formatRelativeTime(timestamp?: number, nowSeconds = Date.now() / 1000) {
  if (!timestamp) {
    return "unknown time";
  }
  const seconds = timestamp > 1_000_000_000_000 ? timestamp / 1000 : timestamp;
  const diff = Math.max(0, Math.floor(nowSeconds - seconds));
  if (diff < 60) {
    return "just now";
  }
  const minutes = Math.floor(diff / 60);
  if (minutes < 60) {
    return `${minutes}m ago`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours}h ago`;
  }
  const days = Math.floor(hours / 24);
  if (days < 30) {
    return `${days}d ago`;
  }
  const date = new Date(seconds * 1000);
  return `${date.getMonth() + 1}/${date.getDate()}`;
}

export function formatDuration(seconds?: number) {
  if (!seconds || seconds < 1) {
    return "<1m";
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 1) {
    return "<1m";
  }
  if (minutes < 60) {
    return `${minutes}m`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    const restMinutes = minutes % 60;
    return restMinutes ? `${hours}h ${restMinutes}m` : `${hours}h`;
  }
  const days = Math.floor(hours / 24);
  const restHours = hours % 24;
  return restHours ? `${days}d ${restHours}h` : `${days}d`;
}

export function formatMessageTime(timestamp?: number) {
  const resolvedTimestamp = timestamp || Date.now() / 1000;
  const milliseconds =
    resolvedTimestamp > 1_000_000_000_000
      ? resolvedTimestamp
      : resolvedTimestamp * 1000;
  return new Intl.DateTimeFormat(undefined, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(milliseconds));
}

export function resolveAgentMarkdownImageSrc(src?: string | null) {
  if (!src) {
    return src || "";
  }
  const normalized = src.startsWith("file://") ? src.slice("file://".length) : src;
  const marker = "/im_gateway/attachments/";
  const markerIndex = normalized.indexOf(marker);
  if (markerIndex < 0 || !normalized.startsWith("/")) {
    return src;
  }
  const relative = normalized.slice(markerIndex + marker.length);
  if (!relative || relative.split("/").some((part) => part === "..")) {
    return src;
  }
  return `/_bifrost/api/im-gateway/attachments/${relative
    .split("/")
    .map((part) => encodeURIComponent(part))
    .join("/")}`;
}

export function selectedRunnerAdapter(options: RunnerOption[], runnerId: string) {
  return options.find((option) => option.value === runnerId)?.adapter || runnerId;
}

export function formatRunnerOptionLabel(id: string, adapter?: string) {
  if (!adapter || adapter === id) {
    return id;
  }
  if (adapter === "chatgpt_web") {
    return `${id} (ChatGPT Web)`;
  }
  if (adapter === "codex") {
    return `${id} (Codex)`;
  }
  return `${id} (${adapter})`;
}

export function formatCurrentStateTag(
  telemetry: RunTelemetry,
  thread: AgentThreadSummary | undefined,
  running: boolean,
) {
  if (running || thread?.running === true) {
    return "Running";
  }
  if (telemetry.phase === "failed") {
    return "Error";
  }
  if (thread || telemetry.phase === "finished" || telemetry.status?.state) {
    return "Ready";
  }
  return "New";
}

export function telemetryFromThread(thread?: AgentThreadSummary): RunTelemetry {
  if (!thread) {
    return EMPTY_TELEMETRY;
  }
  return {
    phase: "idle",
    title: thread.title,
    status: {
      state: thread.state || thread.status,
      source: thread.source,
      work_dir: thread.work_dir,
      message_count: thread.turns,
      total_tokens_used: thread.tokens,
      estimated_context_tokens: thread.estimated_tokens,
      context_window_tokens: thread.context_window_tokens,
      context_usage_percent: thread.context_usage_percent,
      last_response_tokens: thread.last_response_tokens,
      compaction_count: thread.compaction_count,
      runner_type: thread.runner_type,
      runner_id: thread.runner_id,
      agent_type: thread.agent_type,
    },
    plan: [],
    tools: [],
    errors: [],
  };
}

export function telemetryFromSessionDetail(
  detail: SessionDetail,
  thread?: AgentThreadSummary,
): RunTelemetry {
  return {
    ...telemetryFromThread(thread),
    phase: "idle",
    title: detail.title || thread?.title,
    status: {
      ...detail.active_status,
      state:
        detail.active_status?.state ||
        detail.run_state ||
        detail.state ||
        thread?.run_state ||
        thread?.state ||
        thread?.status ||
        "idle",
      source: detail.source || thread?.source,
      work_dir: detail.active_status?.work_dir || detail.work_dir || thread?.work_dir,
      message_count:
        detail.active_status?.message_count ?? detail.message_count ?? thread?.turns,
      total_tokens_used:
        detail.active_status?.total_tokens_used ??
        detail.total_tokens_used ??
        thread?.tokens,
      estimated_context_tokens:
        detail.active_status?.estimated_context_tokens ??
        detail.estimated_tokens ??
        thread?.estimated_tokens,
      context_window_tokens:
        detail.active_status?.context_window_tokens ??
        detail.context_window_tokens ??
        thread?.context_window_tokens,
      context_usage_percent:
        detail.active_status?.context_usage_percent ??
        detail.context_usage_percent ??
        thread?.context_usage_percent,
      last_response_tokens:
        detail.active_status?.last_response_tokens ??
        detail.last_response_tokens ??
        thread?.last_response_tokens,
      compaction_count:
        detail.active_status?.compaction_count ??
        detail.compaction_count ??
        thread?.compaction_count,
      history_version: detail.active_status?.history_version ?? detail.history_version,
      runner_type: detail.active_status?.runner_type || detail.runner_type || thread?.runner_type,
      runner_id: detail.active_status?.runner_id || detail.runner_id || thread?.runner_id,
      agent_type: detail.active_status?.agent_type || detail.agent_type || thread?.agent_type,
    },
  };
}

export function MetricRow({ label, value }: { label: string; value: string }) {
  return (
    <Row justify="space-between" align="middle" gutter={8}>
      <Col>
        <Text type="secondary" style={{ fontSize: 12 }}>
          {label}
        </Text>
      </Col>
      <Col>
        <Text>{value}</Text>
      </Col>
    </Row>
  );
}

export async function runAgentStream(params: {
  message: string;
  sessionKey: string;
  historyPath?: string;
  workDir?: string;
  runnerId?: string;
  runnerAdapter?: string;
  onEvent: (event: Record<string, unknown>) => void;
  onDelta: (content: string) => void;
  onFinal: (content: string) => void;
}) {
  const isExternalRunner =
    params.runnerId && params.runnerId !== "bifrost_agent";
  const response = await apiFetch(
    isExternalRunner ? "/api/im-gateway/chat/stream" : "/api/agent/chat/stream",
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(
        isExternalRunner
          ? {
              message: params.message,
              sessionKey: params.sessionKey,
              runnerId: params.runnerId,
              adapter: params.runnerAdapter,
              params: params.historyPath ? { historyPath: params.historyPath } : undefined,
              workDir: params.workDir,
            }
          : {
              message: params.message,
              session_key: params.sessionKey,
              history_path: params.historyPath,
              work_dir: params.workDir,
            },
      ),
    },
  );
  if (!response.ok || !response.body) {
    throw new Error(await response.text());
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let finalResponse = "";
  const handleEvent = (event: Record<string, unknown>) => {
    params.onEvent(event);
    if (event.eventType === "assistant_delta" && typeof event.content === "string") {
      params.onDelta(event.content);
    } else if (
      event.eventType === "assistant_final" &&
      typeof event.content === "string"
    ) {
      finalResponse = event.content;
    } else if (
      (event.eventType === "run_finished" || event.eventType === "turn_finished") &&
      typeof (event.response || event.content) === "string"
    ) {
      finalResponse = String(event.response || event.content);
    } else if (event.eventType === "run_busy") {
      throw new Error(String(event.message || "Agent session is busy"));
    } else if (event.eventType === "run_failed" || event.eventType === "turn_failed") {
      throw new Error(String(event.error || "Agent run failed"));
    }
  };
  while (true) {
    const { value, done } = await reader.read();
    if (done) {
      break;
    }
    buffer += decoder.decode(value, { stream: true });
    if (isExternalRunner) {
      const lines = buffer.split("\n");
      buffer = lines.pop() || "";
      for (const line of lines) {
        const event = parseNdjsonEvent(line);
        if (event) {
          handleEvent(event);
        }
      }
      continue;
    }
    const frames = buffer.split("\n\n");
    buffer = frames.pop() || "";
    for (const frame of frames) {
      const event = parseSseFrame(frame);
      if (!event) {
        continue;
      }
      handleEvent(event);
    }
  }
  const tailEvent = isExternalRunner ? parseNdjsonEvent(buffer) : parseSseFrame(buffer);
  if (tailEvent) {
    handleEvent(tailEvent);
  }
  params.onFinal(finalResponse);
}

export async function runRunnerCallStream(params: {
  message: string;
  sessionKey: string;
  historyPath?: string;
  workDir?: string;
  callerRunnerId: string;
  callerRunnerAdapter?: string;
  targetRunnerId: string;
  callerMessages: Array<{ role: "user" | "assistant"; content: string }>;
  onEvent: (event: Record<string, unknown>) => void;
  onFinal: (content: string) => void;
}) {
  const response = await apiFetch("/api/im-gateway/chat/runner-calls/stream", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      callerSessionKey: params.sessionKey,
      callerRunnerId: params.callerRunnerId,
      callerRunnerAdapter: params.callerRunnerAdapter,
      targetRunnerId: params.targetRunnerId,
      message: params.message,
      workDir: params.workDir,
      historyPath: params.historyPath,
      callerMessages: params.callerMessages,
    }),
  });
  if (!response.ok || !response.body) {
    throw new Error(await response.text());
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let finalResponse = "";
  const handleEvent = (event: Record<string, unknown>) => {
    params.onEvent(event);
    if (
      (event.eventType === "runner_call_finished" ||
        event.eventType === "run_finished") &&
      typeof event.response === "string"
    ) {
      finalResponse = event.response;
    }
    if (event.eventType === "runner_call_failed") {
      throw new Error(String(event.error || "Runner call failed"));
    }
  };
  while (true) {
    const { value, done } = await reader.read();
    if (done) {
      break;
    }
    buffer += decoder.decode(value, { stream: true });
    const lines = buffer.split("\n");
    buffer = lines.pop() || "";
    for (const line of lines) {
      const event = parseNdjsonEvent(line);
      if (event) {
        handleEvent(event);
      }
    }
  }
  const tailEvent = parseNdjsonEvent(buffer);
  if (tailEvent) {
    handleEvent(tailEvent);
  }
  params.onFinal(finalResponse);
}

export function reduceTelemetry(
  telemetry: RunTelemetry,
  event: Record<string, unknown>,
): RunTelemetry {
  const eventType = typeof event.eventType === "string" ? event.eventType : "";
  if (eventType === "run_started") {
    return { ...telemetry, phase: "running" };
  }
  if (eventType === "status" && isRecord(event.status)) {
    return { ...telemetry, status: event.status as RunStatusSnapshot };
  }
  if (eventType === "title_updated" && typeof event.title === "string") {
    return { ...telemetry, title: event.title };
  }
  if (eventType === "plan_updated" && Array.isArray(event.steps)) {
    return {
      ...telemetry,
      plan: parsePlanSteps(event.steps),
    };
  }
  if (eventType === "tool_started") {
    const name = stringFrom(event.toolName) || "tool";
    return {
      ...telemetry,
      tools: [
        ...telemetry.tools,
        {
          id: `${Date.now()}-${telemetry.tools.length}-${name}`,
          name,
          status: "running",
          arguments: stringFrom(event.arguments),
        },
      ],
    };
  }
  if (eventType === "tool_finished" && isRecord(event.log)) {
    return finishTool(telemetry, event.log, numberFrom(event.durationMs));
  }
  if (eventType === "run_finished" || eventType === "turn_finished") {
    const finalPlan =
      Array.isArray(event.planSteps) && event.planSteps.length > 0
        ? parsePlanSteps(event.planSteps)
        : telemetry.plan;
    const finalTools =
      Array.isArray(event.toolCalls) && event.toolCalls.length > 0
        ? parseToolCalls(event.toolCalls)
        : telemetry.tools;
    return {
      ...telemetry,
      phase: "finished",
      plan: finalPlan,
      tools: finalTools,
    };
  }
  if (eventType === "assistant_final" && typeof event.content === "string") {
    return telemetry;
  }
  if (eventType === "context_updated" && isRecord(event.context)) {
    return {
      ...telemetry,
      context: event.context as AgentContextSnapshot,
    };
  }
  if (
    (eventType === "compaction_started" ||
      eventType === "compaction_finished" ||
      eventType === "compaction_failed") &&
    isRecord(event.compaction)
  ) {
    const compaction = event.compaction as CompactionProgress;
    return {
      ...telemetry,
      context: compaction.context || telemetry.context,
      compaction,
      compactionPhase:
        eventType === "compaction_started"
          ? "started"
          : eventType === "compaction_failed"
            ? "failed"
            : "finished",
      errors:
        eventType === "compaction_failed"
          ? appendError(
              telemetry.errors,
              stringFrom(event.error) || "Context compaction failed",
            )
          : telemetry.errors,
    };
  }
  if (eventType === "run_busy") {
    return {
      ...telemetry,
      phase: "failed",
      errors: appendError(
        telemetry.errors,
        stringFrom(event.message) || "Agent session is busy",
      ),
    };
  }
  if (eventType === "run_failed" || eventType === "turn_failed") {
    return {
      ...telemetry,
      phase: "failed",
      errors: appendError(telemetry.errors, stringFrom(event.error) || "Agent run failed"),
    };
  }
  return telemetry;
}

export function appendError(errors: string[], error: string) {
  return errors.includes(error) ? errors : [...errors, error];
}

export function finishTool(
  telemetry: RunTelemetry,
  log: Record<string, unknown>,
  durationMs?: number,
): RunTelemetry {
  const name = stringFrom(log.tool_name) || stringFrom(log.toolName) || "tool";
  const finished: ToolRun = {
    id: `${Date.now()}-${telemetry.tools.length}-${name}-finished`,
    name,
    status: log.success === false ? "failed" : "success",
    arguments: stringFrom(log.arguments),
    result: stringFrom(log.result),
    durationMs,
  };
  let pendingIndex = -1;
  for (let index = telemetry.tools.length - 1; index >= 0; index -= 1) {
    const tool = telemetry.tools[index];
    if (tool.name === name && tool.status === "running") {
      pendingIndex = index;
      break;
    }
  }
  if (pendingIndex === -1) {
    return { ...telemetry, tools: [...telemetry.tools, finished] };
  }
  return {
    ...telemetry,
    tools: telemetry.tools.map((tool, index) =>
      index === pendingIndex ? { ...tool, ...finished, id: tool.id } : tool,
    ),
  };
}

export function parsePlanSteps(value: unknown[]): PlanStep[] {
  return value
    .filter(isRecord)
    .map((item) => ({
      step: stringFrom(item.step) || "",
      status: stringFrom(item.status) || "pending",
    }))
    .filter((step) => step.step.length > 0);
}

export function parseToolCalls(value: unknown[]): ToolRun[] {
  return value.filter(isRecord).map((item, index) => ({
    id: `final-tool-${index}`,
    name: stringFrom(item.tool_name) || stringFrom(item.toolName) || "tool",
    status: item.success === false ? "failed" : "success",
    arguments: stringFrom(item.arguments),
    result: stringFrom(item.result),
  }));
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function stringFrom(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

export function numberFrom(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

export function formatRunPhase(phase: RunTelemetry["phase"]) {
  return phase === "idle" ? "Idle" : formatLabel(phase);
}

export function formatLabel(value: string) {
  return value
    .replace(/_/g, " ")
    .replace(/\b\w/g, (char) => char.toUpperCase());
}

export function formatNumber(value?: number) {
  return typeof value === "number" ? value.toLocaleString() : "-";
}

export function formatStatusMetricCount(value?: number) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return "-";
  }
  const wholeValue = Math.max(0, Math.round(value));
  const units: Array<[number, string]> = [
    [1_000, "K"],
    [1_000_000, "M"],
    [1_000_000_000, "B"],
  ];
  if (wholeValue < units[0][0]) {
    return String(wholeValue);
  }

  let unitIndex = units.length - 1;
  for (let index = 0; index < units.length; index += 1) {
    if (wholeValue < units[index][0]) {
      unitIndex = Math.max(index - 1, 0);
      break;
    }
  }
  while (
    unitIndex + 1 < units.length &&
    roundedMetricTenths(wholeValue, units[unitIndex][0]) >= 10_000
  ) {
    unitIndex += 1;
  }

  const [unit, suffix] = units[unitIndex];
  const scaledTenths = roundedMetricTenths(wholeValue, unit);
  const whole = Math.floor(scaledTenths / 10);
  const decimal = scaledTenths % 10;
  return decimal === 0 ? `${whole}${suffix}` : `${whole}.${decimal}${suffix}`;
}

function roundedMetricTenths(value: number, unit: number) {
  return Math.floor((value * 10 + Math.floor(unit / 2)) / unit);
}

export function formatLoopProgress(status?: RunStatusSnapshot) {
  if (!status) {
    return "-";
  }
  const current = status.current_loop_iteration ?? status.completed_loop_iterations ?? 0;
  const max = status.max_loop_iterations;
  return typeof max === "number" && max > 0 ? `${current}/${max}` : String(current);
}

export function formatContextUsage(
  status?: RunStatusSnapshot,
  context?: AgentContextSnapshot,
) {
  if (typeof status?.context_usage_percent === "number") {
    return `${status.context_usage_percent}%`;
  }
  if (typeof context?.contextUsagePercent === "number") {
    return `${context.contextUsagePercent}%`;
  }
  if (
    typeof status?.estimated_context_tokens === "number" &&
    typeof status?.context_window_tokens === "number"
  ) {
    return `${status.estimated_context_tokens}/${status.context_window_tokens}`;
  }
  if (
    typeof context?.estimatedContextTokens === "number" &&
    typeof context.contextWindowTokens === "number"
  ) {
    return `${context.estimatedContextTokens}/${context.contextWindowTokens}`;
  }
  return "-";
}

export function formatContextWindow(
  status?: RunStatusSnapshot,
  context?: AgentContextSnapshot,
) {
  const used = status?.estimated_context_tokens ?? context?.estimatedContextTokens;
  const window = status?.context_window_tokens ?? context?.contextWindowTokens;
  return typeof used === "number" && typeof window === "number"
    ? `${formatStatusMetricCount(used)} / ${formatStatusMetricCount(window)}`
    : "-";
}

export function formatContextManagement(
  status?: RunStatusSnapshot,
  context?: AgentContextSnapshot,
) {
  const total = status?.message_count ?? context?.messageCount;
  return total === undefined
    ? "Token budget + compaction"
    : `Full history (${total}) · token budget + compaction`;
}

export function statusTagColor(phase: RunTelemetry["phase"]) {
  if (phase === "running") {
    return "processing";
  }
  if (phase === "finished") {
    return "success";
  }
  if (phase === "failed") {
    return "error";
  }
  return "default";
}

export function planStatusColor(status: PlanStepStatus) {
  if (status === "completed") {
    return "success";
  }
  if (status === "in_progress") {
    return "processing";
  }
  return "default";
}

export function toolStatusColor(status: ToolRun["status"]) {
  if (status === "success") {
    return "success";
  }
  if (status === "failed") {
    return "error";
  }
  return "processing";
}

export function compactionTagColor(phase?: RunTelemetry["compactionPhase"]) {
  if (phase === "failed") {
    return "error";
  }
  if (phase === "finished") {
    return "success";
  }
  if (phase === "started") {
    return "processing";
  }
  return "default";
}

export function parseSseFrame(frame: string): Record<string, unknown> | null {
  const data = frame
    .split("\n")
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice("data:".length).trimStart())
    .join("\n");
  if (!data) {
    return null;
  }
  try {
    return JSON.parse(data) as Record<string, unknown>;
  } catch {
    return null;
  }
}

export function parseNdjsonEvent(line: string): Record<string, unknown> | null {
  const text = line.trim();
  if (!text) {
    return null;
  }
  try {
    const event = JSON.parse(text) as Record<string, unknown>;
    return normalizeExternalRunnerEvent(event);
  } catch {
    return null;
  }
}

export function normalizeExternalRunnerEvent(event: Record<string, unknown>) {
  const eventType = typeof event.eventType === "string" ? event.eventType : "";
  const content = stringFrom(event.content);
  if (eventType === "tool_started") {
    return {
      ...event,
      toolName: stringFrom(event.title) || content || "runner",
      arguments: content,
    };
  }
  if (eventType === "tool_finished") {
    return {
      ...event,
      log: {
        tool_name: stringFrom(event.title) || "runner",
        result: content,
        success: true,
      },
    };
  }
  if (eventType === "status") {
    return {
      ...event,
      status: {
        state: content || "running",
      },
    };
  }
  return event;
}

/**
 * Convert a SSE event into a short process step for the thinking block.
 * Returns null for events that should not appear as process steps.
 */
export function eventToProcessStep(event: Record<string, unknown>): ProcessStep | null {
  const eventType = typeof event.eventType === "string" ? event.eventType : "";
  if (eventType === "tool_started") {
    const name = stringFrom(event.toolName) || "tool";
    const args = stringFrom(event.arguments);
    // Extract a short target from arguments (e.g. file path)
    let target = "";
    if (args) {
      try {
        const parsed = JSON.parse(args);
        target = parsed.path || parsed.command || parsed.query || "";
        if (typeof target === "string" && target.length > 60) {
          target = `…${target.slice(-55)}`;
        }
      } catch {
        target = args.length > 40 ? `${args.slice(0, 37)}…` : args;
      }
    }
    return {
      type: "tool",
      summary: target ? `${name} → ${target}` : name,
      detail: args,
      args: args || undefined,
      status: "running",
    };
  }
  if (eventType === "plan_updated" && Array.isArray(event.steps)) {
    const count = event.steps.length;
    const title = stringFrom(event.title);
    return {
      type: "plan",
      summary: title
        ? `Plan: ${title} (${count} steps)`
        : `Plan updated (${count} steps)`,
      status: "success",
    };
  }
  if (eventType === "compaction_started") {
    return { type: "compaction", summary: "上下文正在自动压缩", status: "running" };
  }
  if (eventType === "compaction_finished") {
    const comp = isRecord(event.compaction) ? event.compaction : {};
    const saved = numberFrom(comp.tokensSaved);
    const removed = numberFrom(comp.messagesRemoved);
    const parts: string[] = ["上下文已自动压缩"];
    if (typeof saved === "number") parts.push(`saved ${saved} tokens`);
    if (typeof removed === "number") parts.push(`removed ${removed} msgs`);
    return { type: "compaction", summary: parts.join(", "), status: "success" };
  }
  if (eventType === "compaction_failed") {
    return {
      type: "compaction",
      summary: "Compaction failed",
      detail: stringFrom(event.error),
      status: "failed",
    };
  }
  return null;
}

/**
 * Truncates text to max 3 lines and 1000 characters.
 * Returns { display, isTruncated } for expand/collapse UI.
 */
export function truncateText(text: string): { display: string; isTruncated: boolean } {
  const MAX_CHARS = 1000;
  const MAX_LINES = 3;
  const lines = text.split("\n");
  let result = text;
  let truncated = false;
  if (lines.length > MAX_LINES) {
    result = lines.slice(0, MAX_LINES).join("\n");
    truncated = true;
  }
  if (result.length > MAX_CHARS) {
    result = result.slice(0, MAX_CHARS);
    truncated = true;
  }
  return { display: truncated ? `${result}…` : result, isTruncated: truncated };
}

/**
 * Expandable text block for tool args/result or thinking content.
 */
export function ExpandableText({ text, label }: { text: string; label: string }) {
  const { token } = theme.useToken();
  const [expanded, setExpanded] = useState(false);
  const { display, isTruncated } = truncateText(text);

  return (
    <div style={{ marginTop: 3, marginLeft: 16 }}>
      <div
        onClick={() => isTruncated && setExpanded(!expanded)}
        style={{ cursor: isTruncated ? "pointer" : "default" }}
      >
        <Text type="secondary" style={{ fontSize: 10, fontWeight: 500 }}>
          {label}:
        </Text>
        <pre
          style={{
            margin: "2px 0 0",
            padding: "4px 8px",
            fontSize: 11,
            lineHeight: "16px",
            background: token.colorFillQuaternary,
            color: token.colorTextTertiary,
            borderRadius: 4,
            whiteSpace: "pre-wrap",
            wordBreak: "break-all",
            maxHeight: expanded ? "none" : "52px",
            overflow: "hidden",
          }}
        >
          {expanded ? text : display}
        </pre>
        {isTruncated && (
          <Text
            type="secondary"
            style={{ fontSize: 10, color: token.colorTextTertiary, cursor: "pointer" }}
            onClick={(e) => { e.stopPropagation(); setExpanded(!expanded); }}
          >
            {expanded ? "收起" : "展开更多"}
          </Text>
        )}
      </div>
    </div>
  );
}

/**
 * Collapsible "thinking process" block inside a message bubble.
 * Shows thinking steps interleaved with tool calls.
 */
export function ProcessStepsBlock({
  steps,
  running: isRunning,
}: {
  steps: ProcessStep[];
  running: boolean;
}) {
  const { token } = theme.useToken();
  const [expanded, setExpanded] = useState(false);
  const [expandedTools, setExpandedTools] = useState<Set<number>>(new Set());
  const visibleSteps = isRunning
    ? steps
    : steps.filter(
        (step) =>
          step.type !== "compaction" &&
          !(step.type === "status" && step.summary.startsWith("Run state:")),
      );
  if (visibleSteps.length === 0) return null;
  const mutedColor = token.colorTextQuaternary;
  const textColor = token.colorTextTertiary;

  const runningCount = isRunning
    ? visibleSteps.filter((s) => s.status === "running").length
    : 0;
  const commandCount = visibleSteps.filter((step) => step.type === "tool").length;
  const summaryCount = commandCount || visibleSteps.length;
  const summaryText = `${isRunning ? "Running" : "Ran"} ${
    summaryCount
  } command${summaryCount > 1 ? "s" : ""}${
    runningCount > 0 ? ` (${runningCount} active)` : ""
  }`;

  const toggleToolExpand = (index: number) => {
    setExpandedTools((prev) => {
      const next = new Set(prev);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });
  };

  return (
    <div
      data-testid="agent-chat-process-block"
      style={{
        margin: "0 0 2px",
        padding: 0,
        fontSize: 12,
        color: textColor,
      }}
    >
      <div
        onClick={() => setExpanded(!expanded)}
        style={{
          cursor: "pointer",
          display: "flex",
          alignItems: "center",
          gap: 6,
          userSelect: "none",
          width: "fit-content",
        }}
      >
        {isRunning ? (
          <LoadingOutlined style={{ fontSize: 12, color: mutedColor }} />
        ) : expanded ? (
          <DownOutlined style={{ fontSize: 10, color: mutedColor }} />
        ) : (
          <RightOutlined style={{ fontSize: 10, color: mutedColor }} />
        )}
        <Text type="secondary" style={{ fontSize: 12, color: textColor }}>
          {summaryText}
        </Text>
      </div>
      {expanded && (
        <div style={{ marginTop: 6, paddingLeft: 18 }}>
          {visibleSteps.map((step, index) => {
            const displayStatus =
              !isRunning && step.status === "running" ? "success" : step.status;
            if (step.type === "thinking") {
              // Thinking step: show as italic/gray text block
              const { display, isTruncated } = truncateText(step.summary);
              return (
                <ThinkingStepItem key={`thinking-${index}`} text={step.summary} display={display} isTruncated={isTruncated} />
              );
            }
            // Tool or other step
            const isToolExpanded = expandedTools.has(index);
            return (
              <div key={`${index}-${step.summary}`} style={{ marginBottom: 5 }}>
                <div
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 6,
                    lineHeight: "18px",
                    cursor: step.type === "tool" ? "pointer" : "default",
                    width: "fit-content",
                  }}
                  onClick={() => step.type === "tool" && toggleToolExpand(index)}
                >
                  {displayStatus === "running" ? (
                    <LoadingOutlined style={{ fontSize: 11, color: mutedColor }} />
                  ) : displayStatus === "failed" ? (
                    <ExclamationCircleOutlined style={{ fontSize: 11, color: textColor }} />
                  ) : (
                    <CheckCircleOutlined style={{ fontSize: 11, color: mutedColor }} />
                  )}
                  {step.type === "tool" && (
                    isToolExpanded
                      ? <DownOutlined style={{ fontSize: 8, color: mutedColor }} />
                      : <RightOutlined style={{ fontSize: 8, color: mutedColor }} />
                  )}
                  <Text
                    type="secondary"
                    style={{ fontSize: 11, color: textColor }}
                    ellipsis={{ tooltip: step.summary }}
                  >
                    {step.summary}
                  </Text>
                </div>
                {step.type === "tool" && isToolExpanded && (
                  <div style={{ marginLeft: 30, marginTop: 4, opacity: 0.78 }}>
                    {step.args && <ExpandableText text={step.args} label="Input" />}
                    {step.result && <ExpandableText text={step.result} label="Output" />}
                    {!step.args && !step.result && (
                      <Text type="secondary" style={{ fontSize: 10, marginLeft: 16 }}>
                        No details available
                      </Text>
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

/**
 * A thinking step displayed as an italic gray text block.
 */
export function ThinkingStepItem({ text, display, isTruncated }: { text: string; display: string; isTruncated: boolean }) {
  const { token } = theme.useToken();
  const [showFull, setShowFull] = useState(false);
  return (
    <div
      style={{
        margin: "4px 0",
        padding: "4px 8px",
        borderLeft: `2px solid ${token.colorBorderSecondary}`,
        background: token.colorFillQuaternary,
        borderRadius: "0 4px 4px 0",
      }}
    >
      <pre
        style={{
          margin: 0,
          fontSize: 11,
          lineHeight: "16px",
          fontStyle: "italic",
          color: token.colorTextTertiary,
          whiteSpace: "pre-wrap",
          wordBreak: "break-all",
          maxHeight: showFull ? "none" : "52px",
          overflow: "hidden",
        }}
      >
        {showFull ? text : display}
      </pre>
      {isTruncated && (
        <Text
          type="secondary"
          style={{ fontSize: 10, color: token.colorTextTertiary, cursor: "pointer" }}
          onClick={() => setShowFull(!showFull)}
        >
          {showFull ? "收起" : "展开更多"}
        </Text>
      )}
    </div>
  );
}
