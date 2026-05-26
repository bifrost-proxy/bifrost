import { useState } from "react";
import { Col, Row, Typography } from "antd";
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
  compaction_count?: number;
  history_version?: number;
  agent_type?: string;
  runner_type?: string;
  runner_id?: string;
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

export function historyEventsToMessages(events: HistoryEvent[]): ChatMessage[] {
  return events
    .filter(
      (event) =>
        event.event_type === "user_message" ||
        event.event_type === "assistant_message",
    )
    .map((event, index) => {
      const role =
        event.event_type === "user_message" ? ("user" as const) : ("assistant" as const);
      const content = event.content.message;
      return {
        id: `history-${index}`,
        role,
        content: typeof content === "string" ? content : "",
        timestamp: event.timestamp,
        meta: role === "user" ? "History user" : "History assistant",
      };
    })
    .filter((item) => item.content.trim().length > 0);
}

export function sessionDetailToMessages(detail: SessionDetail): ChatMessage[] {
  return (detail.messages || [])
    .filter((message) => message.role === "user" || message.role === "assistant")
    .map((message, index) => {
      const role = message.role === "user" ? ("user" as const) : ("assistant" as const);
      return {
        id: `session-${detail.session_key}-${index}`,
        role,
        content: message.content || "",
        timestamp: message.timestamp,
        meta: role === "user" ? "You" : "Bifrost Agent",
      };
    })
    .filter((message) => message.content.trim().length > 0);
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
  return thread.session_key === sessionKey && !thread.history_path;
}

export function dedupeThreads(threads: AgentThreadSummary[]) {
  const bySession = new Map<string, AgentThreadSummary>();
  for (const thread of threads) {
    const existing = bySession.get(thread.session_key);
    if (!existing || threadRank(thread) > threadRank(existing)) {
      bySession.set(thread.session_key, thread);
    }
  }
  return Array.from(bySession.values()).sort(
    (a, b) => (b.last_active_time || 0) - (a.last_active_time || 0),
  );
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
      state: detail.state || thread?.state || thread?.status || "idle",
      source: detail.source || thread?.source,
      work_dir: detail.work_dir || thread?.work_dir,
      message_count: detail.message_count ?? thread?.turns,
      total_tokens_used: detail.total_tokens_used ?? thread?.tokens,
      estimated_context_tokens: detail.estimated_tokens ?? thread?.estimated_tokens,
      compaction_count: detail.compaction_count ?? thread?.compaction_count,
      history_version: detail.history_version,
      runner_type: detail.runner_type || thread?.runner_type,
      runner_id: detail.runner_id || thread?.runner_id,
      agent_type: detail.agent_type || thread?.agent_type,
    },
  };
}

export function historyEventsToTelemetry(
  events: HistoryEvent[],
  thread?: AgentThreadSummary,
  fallback: RunTelemetry = EMPTY_TELEMETRY,
): RunTelemetry {
  let telemetry = {
    ...telemetryFromThread(thread),
    phase: fallback.phase,
    plan: fallback.plan,
    tools: fallback.tools,
    errors: fallback.errors,
  };
  for (const event of events) {
    if (event.event_type === "session_start") {
      telemetry = {
        ...telemetry,
        status: {
          ...telemetry.status,
          state: telemetry.status?.state || "started",
          source:
            stringFrom(event.content.source) ||
            telemetry.status?.source,
          work_dir:
            stringFrom(event.content.work_dir) ||
            stringFrom(event.content.workDir) ||
            telemetry.status?.work_dir,
          agent_type:
            stringFrom(event.content.agent_type) ||
            stringFrom(event.content.agentType) ||
            telemetry.status?.agent_type,
          runner_type:
            stringFrom(event.content.runner_type) ||
            stringFrom(event.content.runnerType) ||
            telemetry.status?.runner_type,
          runner_id:
            stringFrom(event.content.runner_id) ||
            stringFrom(event.content.runnerId) ||
            telemetry.status?.runner_id,
        },
      };
      continue;
    }
    if (event.event_type === "title_updated") {
      telemetry = {
        ...telemetry,
        title: stringFrom(event.content.title) || telemetry.title,
      };
      continue;
    }
    if (event.event_type === "plan_updated" && Array.isArray(event.content.plan)) {
      telemetry = {
        ...telemetry,
        plan: parsePlanSteps(event.content.plan),
      };
      continue;
    }
    if (event.event_type === "tool_call") {
      const name = stringFrom(event.content.tool_name) || "tool";
      telemetry = {
        ...telemetry,
        tools: [
          ...telemetry.tools,
          {
            id: `history-tool-${event.timestamp}-${telemetry.tools.length}`,
            name,
            status: "running",
            arguments: stringFrom(event.content.arguments),
          },
        ],
      };
      continue;
    }
    if (event.event_type === "tool_result") {
      telemetry = finishTool(telemetry, event.content);
      continue;
    }
    if (event.event_type === "compaction") {
      telemetry = {
        ...telemetry,
        compactionPhase: "finished",
        compaction: {
          trigger: stringFrom(event.content.trigger),
          reason: stringFrom(event.content.reason),
          phase: stringFrom(event.content.phase),
          preTokens: numberFrom(event.content.pre_tokens),
          postTokens:
            numberFrom(event.content.post_tokens) ??
            numberFrom(event.content.postTokens),
          tokensSaved:
            numberFrom(event.content.tokens_saved) ??
            numberFrom(event.content.tokensSaved),
          messagesRemoved:
            numberFrom(event.content.messages_removed) ??
            numberFrom(event.content.messagesRemoved),
          compactionCount:
            numberFrom(event.content.compaction_count) ??
            numberFrom(event.content.compactionCount),
        },
        status: {
          ...telemetry.status,
          total_tokens_used:
            numberFrom(event.content.total_tokens) ??
            numberFrom(event.content.totalTokens) ??
            telemetry.status?.total_tokens_used,
          estimated_context_tokens:
            numberFrom(event.content.post_tokens) ??
            numberFrom(event.content.postTokens) ??
            telemetry.status?.estimated_context_tokens,
          compaction_count:
            numberFrom(event.content.compaction_count) ??
            numberFrom(event.content.compactionCount) ??
            telemetry.status?.compaction_count,
        },
      };
      continue;
    }
    if (event.event_type === "session_end") {
      telemetry = {
        ...telemetry,
        phase: "finished",
        status: {
          ...telemetry.status,
          total_tokens_used:
            numberFrom(event.content.total_tokens) ??
            numberFrom(event.content.totalTokens) ??
            telemetry.status?.total_tokens_used,
          message_count:
            numberFrom(event.content.message_count) ??
            numberFrom(event.content.messageCount) ??
            telemetry.status?.message_count,
          compaction_count:
            numberFrom(event.content.compaction_count) ??
            numberFrom(event.content.compactionCount) ??
            telemetry.status?.compaction_count,
        },
      };
    }
  }
  return {
    ...telemetry,
    tools: telemetry.tools.map((tool) =>
      tool.status === "running" ? { ...tool, status: "success" } : tool,
    ),
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
    ? `${used.toLocaleString()} / ${window.toLocaleString()}`
    : "-";
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
    return { type: "compaction", summary: "Context compaction…", status: "running" };
  }
  if (eventType === "compaction_finished") {
    const comp = isRecord(event.compaction) ? event.compaction : {};
    const saved = numberFrom(comp.tokensSaved);
    const removed = numberFrom(comp.messagesRemoved);
    const parts: string[] = ["Compacted"];
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
            background: "rgba(0,0,0,0.03)",
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
            style={{ fontSize: 10, color: "#1677ff", cursor: "pointer" }}
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
  const [expanded, setExpanded] = useState(false);
  const [expandedTools, setExpandedTools] = useState<Set<number>>(new Set());
  if (steps.length === 0) return null;

  const runningCount = steps.filter((s) => s.status === "running").length;
  const summaryText = isRunning
    ? `Thinking… ${steps.length} step${steps.length > 1 ? "s" : ""}${runningCount > 0 ? ` (${runningCount} running)` : ""}`
    : `${steps.length} step${steps.length > 1 ? "s" : ""} completed`;

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
        marginBottom: 8,
        padding: "6px 10px",
        borderRadius: 8,
        background: "rgba(0,0,0,0.02)",
        border: "1px solid rgba(0,0,0,0.06)",
        fontSize: 12,
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
        }}
      >
        {isRunning ? (
          <LoadingOutlined style={{ fontSize: 11, color: "#1677ff" }} />
        ) : expanded ? (
          <DownOutlined style={{ fontSize: 9 }} />
        ) : (
          <RightOutlined style={{ fontSize: 9 }} />
        )}
        <Text type="secondary" style={{ fontSize: 12 }}>
          {summaryText}
        </Text>
      </div>
      {expanded && (
        <div style={{ marginTop: 6, paddingLeft: 4 }}>
          {steps.map((step, index) => {
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
              <div key={`${index}-${step.summary}`} style={{ marginBottom: 4 }}>
                <div
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 6,
                    lineHeight: "18px",
                    cursor: step.type === "tool" ? "pointer" : "default",
                  }}
                  onClick={() => step.type === "tool" && toggleToolExpand(index)}
                >
                  {step.status === "running" ? (
                    <LoadingOutlined style={{ fontSize: 10, color: "#1677ff" }} />
                  ) : step.status === "failed" ? (
                    <ExclamationCircleOutlined style={{ fontSize: 10, color: "#ff4d4f" }} />
                  ) : (
                    <CheckCircleOutlined style={{ fontSize: 10, color: "#52c41a" }} />
                  )}
                  {step.type === "tool" && (
                    isToolExpanded
                      ? <DownOutlined style={{ fontSize: 8 }} />
                      : <RightOutlined style={{ fontSize: 8 }} />
                  )}
                  <Text
                    type="secondary"
                    style={{ fontSize: 11 }}
                    ellipsis={{ tooltip: step.summary }}
                  >
                    {step.summary}
                  </Text>
                </div>
                {step.type === "tool" && isToolExpanded && (
                  <div style={{ marginLeft: 16 }}>
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
  const [showFull, setShowFull] = useState(false);
  return (
    <div
      style={{
        margin: "4px 0",
        padding: "4px 8px",
        borderLeft: "2px solid rgba(0,0,0,0.1)",
        background: "rgba(0,0,0,0.015)",
        borderRadius: "0 4px 4px 0",
      }}
    >
      <pre
        style={{
          margin: 0,
          fontSize: 11,
          lineHeight: "16px",
          fontStyle: "italic",
          color: "rgba(0,0,0,0.5)",
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
          style={{ fontSize: 10, color: "#1677ff", cursor: "pointer" }}
          onClick={() => setShowFull(!showFull)}
        >
          {showFull ? "收起" : "展开更多"}
        </Text>
      )}
    </div>
  );
}
