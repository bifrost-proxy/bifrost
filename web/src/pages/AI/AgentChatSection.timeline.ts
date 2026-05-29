import {
  EMPTY_TELEMETRY,
  finishTool,
  formatLabel,
  numberFrom,
  parsePlanSteps,
  stringFrom,
  telemetryFromThread,
  type AgentThreadSummary,
  type ChatMessage,
  type HistoryEvent,
  type ProcessStep,
  type RunTelemetry,
} from "./AgentChatSection.helpers";

export type HistoryMessagesOptions = {
  ensureRunningAssistant?: boolean;
  runningState?: string;
};

export function historyEventsToMessages(
  events: HistoryEvent[],
  options: HistoryMessagesOptions = {},
): ChatMessage[] {
  const messages: ChatMessage[] = [];
  let pendingSteps: ProcessStep[] = [];
  let latestRunState: string | undefined;
  let lastEventWasAssistantDelta = false;

  const flushPendingSteps = () => {
    if (pendingSteps.length === 0) {
      return;
    }
    const lastMessage = messages[messages.length - 1];
    if (lastMessage?.role === "assistant") {
      const index = messages.length - 1;
      messages[index] = {
        ...messages[index],
        processSteps: [...(messages[index].processSteps || []), ...pendingSteps],
      };
    } else {
      messages.push({
        id: `history-progress-${messages.length}`,
        role: "assistant",
        content: "Agent is running...",
        timestamp: events[events.length - 1]?.timestamp,
        meta: "Agent progress",
        processSteps: pendingSteps,
      });
    }
    pendingSteps = [];
  };

  events.forEach((event, index) => {
    if (event.event_type === "run_state_changed") {
      latestRunState = stringFrom(event.content.state) || latestRunState;
    }
    if (event.event_type === "user_message") {
      lastEventWasAssistantDelta = false;
      const content = event.content.message;
      if (typeof content === "string" && content.trim().length > 0) {
        messages.push({
          id: `history-${index}`,
          role: "user",
          content,
          timestamp: event.timestamp,
          meta: "History user",
        });
      }
      return;
    }

    if (event.event_type === "assistant_delta") {
      const content =
        stringFrom(event.content.message) || stringFrom(event.content.content) || "";
      if (content.trim().length > 0) {
        const lastMessage = messages[messages.length - 1];
        if (
          lastEventWasAssistantDelta &&
          lastMessage?.role === "assistant" &&
          !lastMessage.processSteps?.length
        ) {
          const index = messages.length - 1;
          messages[index] = {
            ...messages[index],
            content: `${messages[index].content}${content}`,
          };
        } else {
          messages.push({
            id: `history-delta-${index}`,
            role: "assistant",
            content,
            timestamp: event.timestamp,
            meta: "History assistant",
            processSteps: pendingSteps.length > 0 ? pendingSteps : undefined,
          });
          pendingSteps = [];
        }
        lastEventWasAssistantDelta = true;
      }
      return;
    }

    if (event.event_type === "assistant_message") {
      flushPendingSteps();
      lastEventWasAssistantDelta = false;
      const content = event.content.message;
      if (typeof content === "string" && content.trim().length > 0) {
        messages.push({
          id: `history-${index}`,
          role: "assistant",
          content,
          timestamp: event.timestamp,
          meta: "History assistant",
          processSteps: pendingSteps.length > 0 ? pendingSteps : undefined,
        });
        pendingSteps = [];
      }
      return;
    }

    const step = historyEventToProcessStep(event);
    if (!step) {
      return;
    }
    lastEventWasAssistantDelta = false;
    if (event.event_type === "tool_result") {
      const name = stringFrom(event.content.tool_name) || stringFrom(event.content.toolName);
      const lastMessage = messages[messages.length - 1];
      if (lastMessage?.role === "assistant") {
        const lastSteps = [...(lastMessage.processSteps || [])];
        const lastPendingIndex = findPendingToolStep(lastSteps, name);
        if (lastPendingIndex >= 0) {
          lastSteps[lastPendingIndex] = {
            ...lastSteps[lastPendingIndex],
            status: step.status,
            result: step.result,
          };
          messages[messages.length - 1] = {
            ...lastMessage,
            processSteps: lastSteps,
          };
          return;
        }
      }
      const pendingIndex = findPendingToolStep(pendingSteps, name);
      if (pendingIndex >= 0) {
        pendingSteps[pendingIndex] = {
          ...pendingSteps[pendingIndex],
          status: step.status,
          result: step.result,
        };
        return;
      }
    }
    const lastMessage = messages[messages.length - 1];
    if (lastMessage?.role === "assistant") {
      messages[messages.length - 1] = {
        ...lastMessage,
        processSteps: [...(lastMessage.processSteps || []), step],
      };
    } else {
      pendingSteps.push(step);
    }
  });

  const runningState = latestRunState || options.runningState || "running";
  const shouldEnsureRunningAssistant =
    !isTerminalRunState(latestRunState) &&
    (isActiveRunState(latestRunState) ||
      isActiveRunState(options.runningState) ||
      options.ensureRunningAssistant === true);
  if (shouldEnsureRunningAssistant && pendingSteps.length > 0 && !hasRunStateStep(pendingSteps)) {
    pendingSteps = [runningStatusStep(runningState), ...pendingSteps];
  }
  flushPendingSteps();
  if (shouldEnsureRunningAssistant) {
    ensureRunningStatusStepOnLatestProgressMessage(messages, runningState);
  }
  if (shouldEnsureRunningAssistant && messages[messages.length - 1]?.role === "user") {
    messages.push({
      id: `history-running-${messages.length}`,
      role: "assistant",
      content: "Agent is running...",
      timestamp: events[events.length - 1]?.timestamp,
      meta: "Agent progress",
      processSteps: [
        runningStatusStep(runningState),
      ],
    });
  }
  return hideIntermediateAssistantTimestamps(
    messages.filter((item) => item.content.trim().length > 0 || item.processSteps?.length),
  );
}

function hideIntermediateAssistantTimestamps(messages: ChatMessage[]) {
  const next = messages.map((message) => ({ ...message }));
  let turnAssistantIndexes: number[] = [];

  const flushTurn = () => {
    if (turnAssistantIndexes.length === 0) {
      return;
    }
    const lastAssistantIndex = turnAssistantIndexes[turnAssistantIndexes.length - 1];
    for (const index of turnAssistantIndexes) {
      next[index] = {
        ...next[index],
        hideTimestamp: index !== lastAssistantIndex,
      };
    }
    turnAssistantIndexes = [];
  };

  for (let index = 0; index < next.length; index += 1) {
    const message = next[index];
    if (message.role === "user") {
      flushTurn();
      continue;
    }
    if (message.role === "assistant") {
      turnAssistantIndexes.push(index);
    }
  }
  flushTurn();

  return next;
}

function ensureRunningStatusStepOnLatestProgressMessage(
  messages: ChatMessage[],
  runningState: string,
) {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.role !== "assistant") {
      continue;
    }
    if (!message.processSteps?.length) {
      continue;
    }
    if (hasRunStateStep(message.processSteps)) {
      return;
    }
    messages[index] = {
      ...message,
      processSteps: [runningStatusStep(runningState), ...message.processSteps],
    };
    return;
  }
}

function runningStatusStep(state: string): ProcessStep {
  return {
    type: "status",
    summary: `Run state: ${formatLabel(state)}`,
    status: "running",
  };
}

function hasRunStateStep(steps: ProcessStep[]) {
  return steps.some((step) => step.type === "status" && step.summary.startsWith("Run state:"));
}

function isActiveRunState(state?: string) {
  return isRunStateActive(state);
}

function isTerminalRunState(state?: string) {
  return state === "completed" || state === "failed" || state === "cancelled";
}

function historyEventToProcessStep(event: HistoryEvent): ProcessStep | null {
  if (event.event_type === "run_state_changed") {
    const state = stringFrom(event.content.state) || "running";
    return {
      type: "status",
      summary: `Run state: ${formatLabel(state)}`,
      detail: stringFrom(event.content.source_channel),
      status: isActiveRunState(state)
        ? "running"
        : state === "failed" || state === "cancelled"
          ? "failed"
          : "success",
    };
  }
  if (event.event_type === "plan_updated" && Array.isArray(event.content.plan)) {
    return {
      type: "plan",
      summary: `Plan updated (${event.content.plan.length} steps)`,
      status: "success",
    };
  }
  if (event.event_type === "compaction") {
    return { type: "compaction", summary: "上下文已自动压缩", status: "success" };
  }
  if (event.event_type === "tool_call") {
    const name = stringFrom(event.content.tool_name) || "tool";
    return {
      type: "tool",
      summary: name,
      detail: stringFrom(event.content.arguments),
      args: stringFrom(event.content.arguments),
      status: "running",
    };
  }
  if (event.event_type === "tool_result") {
    const name = stringFrom(event.content.tool_name) || "tool";
    return {
      type: "tool",
      summary: name,
      result: stringFrom(event.content.result),
      status: event.content.success === false ? "failed" : "success",
    };
  }
  return null;
}

function findPendingToolStep(steps: ProcessStep[], name?: string) {
  for (let index = steps.length - 1; index >= 0; index -= 1) {
    const step = steps[index];
    if (step.type === "tool" && step.status === "running" && (!name || step.summary === name)) {
      return index;
    }
  }
  return -1;
}

export function historyEventsToTelemetry(
  events: HistoryEvent[],
  thread?: AgentThreadSummary,
  fallback: RunTelemetry = EMPTY_TELEMETRY,
): RunTelemetry {
  const threadTelemetry = telemetryFromThread(thread);
  let telemetry: RunTelemetry = {
    ...threadTelemetry,
    phase: fallback.phase,
    status: mergeDefinedStatus(fallback.status, threadTelemetry.status),
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
          source: stringFrom(event.content.source) || telemetry.status?.source,
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
    if (event.event_type === "run_state_changed") {
      const state = stringFrom(event.content.state) || telemetry.status?.state || "idle";
      telemetry = {
        ...telemetry,
        phase:
          state === "running" || state === "queued" || state === "waiting_for_tool"
            ? "running"
            : state === "failed" || state === "cancelled"
              ? "failed"
              : state === "completed"
                ? "finished"
                : telemetry.phase,
        status: {
          ...telemetry.status,
          state,
          source: stringFrom(event.content.source_channel) || telemetry.status?.source,
          agent_type: stringFrom(event.content.agent_kind) || telemetry.status?.agent_type,
        },
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
    if (event.event_type === "assistant_message") {
      telemetry = {
        ...telemetry,
        status: {
          ...telemetry.status,
          total_tokens_used:
            numberFrom(event.content.total_tokens) ??
            numberFrom(event.content.totalTokens) ??
            telemetry.status?.total_tokens_used,
          estimated_context_tokens:
            numberFrom(event.content.context_tokens) ??
            numberFrom(event.content.contextTokens) ??
            telemetry.status?.estimated_context_tokens,
        },
      };
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
  const liveStatus = isThreadActiveForTelemetry(thread)
    ? threadTelemetry.status
    : isRunStatusActive(fallback.status)
      ? fallback.status
      : undefined;
  return {
    ...telemetry,
    status: mergeDefinedStatus(telemetry.status, liveStatus),
    tools: telemetry.tools.map((tool) =>
      tool.status === "running" ? { ...tool, status: "success" } : tool,
    ),
  };
}

function mergeDefinedStatus(
  base?: RunTelemetry["status"],
  overlay?: RunTelemetry["status"],
): RunTelemetry["status"] {
  if (!overlay) {
    return base;
  }
  const next = { ...(base || {}) };
  for (const [key, value] of Object.entries(overlay)) {
    if (value !== undefined && value !== null) {
      (next as Record<string, unknown>)[key] = value;
    }
  }
  return next;
}

function isThreadActiveForTelemetry(thread?: AgentThreadSummary) {
  return thread?.running === true || isRunStateActive(thread?.run_state || thread?.state);
}

function isRunStatusActive(status?: RunTelemetry["status"]) {
  return isRunStateActive(status?.state);
}

function isRunStateActive(state?: string) {
  return (
    state === "running" ||
    state === "queued" ||
    state === "waiting_for_tool" ||
    state === "waiting_on_session" ||
    state === "model_response" ||
    state === "tool_running" ||
    state === "compacting" ||
    state === "stopping"
  );
}
