import {
  EMPTY_TELEMETRY,
  finishTool,
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

export function mergeDetailMessagesWithTimeline(
  detailMessages: ChatMessage[],
  timelineMessages: ChatMessage[],
): ChatMessage[] {
  if (detailMessages.length === 0) {
    return timelineMessages;
  }
  if (timelineMessages.length === 0) {
    return detailMessages;
  }
  const merged = detailMessages.map((message) => ({ ...message }));
  let searchStart = Math.max(0, merged.length - timelineMessages.length - 8);
  timelineMessages.forEach((timelineMessage) => {
    const matchIndex = findMergeMessageIndex(merged, timelineMessage, searchStart);
    if (matchIndex >= 0) {
      merged[matchIndex] = mergeChatMessage(merged[matchIndex], timelineMessage);
      searchStart = matchIndex + 1;
      return;
    }
    if (!hasEquivalentMessage(merged, timelineMessage)) {
      merged.push(timelineMessage);
      searchStart = merged.length;
    }
  });
  return sortMessagesByTimestamp(merged);
}

export function sliceRecentChatTurns(
  messages: ChatMessage[],
  visibleTurns: number,
): { messages: ChatMessage[]; hasOlder: boolean } {
  if (visibleTurns <= 0) {
    return { messages, hasOlder: false };
  }
  let seenUserTurns = 0;
  let startIndex = 0;
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    if (messages[index].role !== "user") {
      continue;
    }
    seenUserTurns += 1;
    if (seenUserTurns === visibleTurns) {
      startIndex = index;
    } else if (seenUserTurns > visibleTurns) {
      break;
    }
  }
  while (startIndex > 0 && messages[startIndex - 1].role === "system") {
    startIndex -= 1;
  }
  return {
    messages: messages.slice(startIndex),
    hasOlder: startIndex > 0,
  };
}

export function historyEventsToMessages(
  events: HistoryEvent[],
  options: HistoryMessagesOptions = {},
): ChatMessage[] {
  const messages: ChatMessage[] = [];
  let pendingSteps: ProcessStep[] = [];
  let latestRunState: string | undefined;
  let lastEventWasAssistantDelta = false;
  let externalRunnerTimeline = false;
  const finalAssistantMessages = new Set(
    events
      .filter((event) => event.event_type === "assistant_message")
      .map((event) => normalizedAssistantText(event.content.message))
      .filter((content): content is string => Boolean(content)),
  );

  const appendProcessStep = (step: ProcessStep) => {
    const lastMessage = messages[messages.length - 1];
    if (lastMessage?.role === "assistant") {
      const index = messages.length - 1;
      messages[index] = {
        ...messages[index],
        processSteps: insertProcessStep(messages[index].processSteps || [], step),
      };
    } else {
      pendingSteps = insertProcessStep(pendingSteps, step);
    }
  };

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
    if (event.event_type === "session_start") {
      const adapter = stringFrom(event.content.adapter);
      externalRunnerTimeline =
        externalRunnerTimeline ||
        stringFrom(event.content.runtime) === "external_cli" ||
        isExternalCliAdapter(adapter);
    }
    if (event.event_type === "run_state_changed") {
      latestRunState = stringFrom(event.content.state) || latestRunState;
    }
    if (event.event_type === "user_message") {
      lastEventWasAssistantDelta = false;
      const rawContent = event.content.message;
      const content = typeof rawContent === "string" ? rawContent : "";
      const contentParts = contentPartsFromHistoryUserMessage(event.content);
      if (content.trim().length > 0 || hasImageContentParts(contentParts)) {
        messages.push({
          id: `history-${index}`,
          role: "user",
          content: content.trim().length > 0 ? content : "Attached image",
          contentParts,
          timestamp: event.timestamp,
          meta: "History user",
        });
      }
      return;
    }

    if (event.event_type === "assistant_delta") {
      const content =
        stringFrom(event.content.message) || stringFrom(event.content.content) || "";
      if (externalRunnerTimeline) {
        if (
          content.trim().length > 0 &&
          !finalAssistantMessages.has(normalizedAssistantText(content) || "")
        ) {
          appendProcessStep({
            type: "thinking",
            summary: content,
            status: "success",
            startedAt: event.timestamp,
          });
        }
        lastEventWasAssistantDelta = false;
        return;
      }
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
      lastEventWasAssistantDelta = false;
      const content = event.content.message;
      if (typeof content === "string" && content.trim().length > 0) {
        const processSteps = pendingSteps.length > 0 ? pendingSteps : undefined;
        messages.push({
          id: `history-${index}`,
          role: "assistant",
          content,
          timestamp: event.timestamp,
          meta: "History assistant",
          processSteps,
        });
        pendingSteps = [];
      } else {
        flushPendingSteps();
      }
      return;
    }

    if (event.event_type === "proposed_plan") {
      flushPendingSteps();
      lastEventWasAssistantDelta = false;
      const content = stringFrom(event.content.content);
      if (content) {
        messages.push({
          id: `history-proposed-plan-${index}`,
          role: "assistant",
          content: `**Plan Mode result**\n\n${content.trim()}`,
          timestamp: event.timestamp,
          meta: "Plan Mode",
        });
      }
      return;
    }

    const step = historyEventToProcessStep(event);
    if (!step) {
      return;
    }
    lastEventWasAssistantDelta = false;
    if (event.event_type === "run_state_changed" && hasLatestStatusStep(step.summary)) {
      return;
    }
    if (event.event_type === "tool_call") {
      const lastMessage = messages[messages.length - 1];
      if (lastMessage?.role === "assistant") {
        const lastSteps = [...(lastMessage.processSteps || [])];
        const existingIndex = findMatchingToolStep(
          lastSteps,
          step.callId,
          step.summary,
        );
        if (existingIndex >= 0) {
          lastSteps[existingIndex] = {
            ...lastSteps[existingIndex],
            ...step,
            result: lastSteps[existingIndex].result,
            completedAt: lastSteps[existingIndex].completedAt,
            durationMs: lastSteps[existingIndex].durationMs,
            status: lastSteps[existingIndex].status || step.status,
          };
          messages[messages.length - 1] = {
            ...lastMessage,
            processSteps: lastSteps,
          };
          return;
        }
      }
      const existingPendingIndex = findMatchingToolStep(
        pendingSteps,
        step.callId,
        step.summary,
      );
      if (existingPendingIndex >= 0) {
        pendingSteps[existingPendingIndex] = {
          ...pendingSteps[existingPendingIndex],
          ...step,
          result: pendingSteps[existingPendingIndex].result,
          completedAt: pendingSteps[existingPendingIndex].completedAt,
          durationMs: pendingSteps[existingPendingIndex].durationMs,
          status: pendingSteps[existingPendingIndex].status || step.status,
        };
        return;
      }
    }
    if (event.event_type === "tool_result") {
      const name = stringFrom(event.content.tool_name) || stringFrom(event.content.toolName);
      const callId = stringFrom(event.content.call_id) || stringFrom(event.content.callId);
      const lastMessage = messages[messages.length - 1];
      if (lastMessage?.role === "assistant") {
        const lastSteps = [...(lastMessage.processSteps || [])];
        const lastPendingIndex = findPendingToolStep(lastSteps, callId, name);
        if (lastPendingIndex >= 0) {
          lastSteps[lastPendingIndex] = {
            ...lastSteps[lastPendingIndex],
            status: step.status,
            result: step.result,
            completedAt: step.completedAt,
            durationMs: durationMsBetween(
              lastSteps[lastPendingIndex].startedAt,
              step.completedAt,
            ),
          };
          messages[messages.length - 1] = {
            ...lastMessage,
            processSteps: lastSteps,
          };
          return;
        }
      }
      const pendingIndex = findPendingToolStep(pendingSteps, callId, name);
      if (pendingIndex >= 0) {
        pendingSteps[pendingIndex] = {
          ...pendingSteps[pendingIndex],
          status: step.status,
          result: step.result,
          completedAt: step.completedAt,
          durationMs: durationMsBetween(
            pendingSteps[pendingIndex].startedAt,
            step.completedAt,
          ),
        };
        return;
      }
    }
    appendProcessStep(step);
  });

  const effectiveRunState = options.runningState ?? latestRunState;
  const shouldEnsureRunningAssistant =
    !isTerminalRunState(effectiveRunState) &&
    (isActiveRunState(effectiveRunState) || options.ensureRunningAssistant === true);
  flushPendingSteps();
  if (shouldEnsureRunningAssistant && messages[messages.length - 1]?.role === "user") {
    messages.push({
      id: `history-running-${messages.length}`,
      role: "assistant",
      content: "Agent is running...",
      timestamp: events[events.length - 1]?.timestamp,
      meta: "Agent progress",
      processSteps: [],
    });
  }
  return hideIntermediateAssistantTimestamps(
    messages.filter(
      (item) =>
        item.content.trim().length > 0 ||
        hasImageContentParts(item.contentParts) ||
        item.processSteps?.length,
    ),
  );

  function hasLatestStatusStep(summary: string) {
    const lastMessage = messages[messages.length - 1];
    const lastSteps = lastMessage?.role === "assistant" ? lastMessage.processSteps || [] : [];
    const latestStep =
      lastSteps.length > 0
        ? lastSteps[lastSteps.length - 1]
        : pendingSteps[pendingSteps.length - 1];
    return latestStep?.type === "status" && latestStep.summary === summary;
  }
}

function findMergeMessageIndex(
  messages: ChatMessage[],
  target: ChatMessage,
  startIndex: number,
) {
  for (let index = Math.max(0, startIndex); index < messages.length; index += 1) {
    if (isSameConversationMessage(messages[index], target)) {
      return index;
    }
  }
  for (let index = Math.max(0, startIndex) - 1; index >= 0; index -= 1) {
    if (isSameConversationMessage(messages[index], target)) {
      return index;
    }
  }
  return -1;
}

function hasEquivalentMessage(messages: ChatMessage[], target: ChatMessage) {
  return messages.some((message) => isSameConversationMessage(message, target));
}

function sortMessagesByTimestamp(messages: ChatMessage[]) {
  if (!messages.every((message) => typeof message.timestamp === "number")) {
    return messages;
  }
  return messages
    .map((message, index) => ({ message, index }))
    .sort((left, right) => {
      const leftTimestamp = left.message.timestamp as number;
      const rightTimestamp = right.message.timestamp as number;
      if (leftTimestamp === rightTimestamp) {
        return left.index - right.index;
      }
      return leftTimestamp - rightTimestamp;
    })
    .map(({ message }) => message);
}

function isSameConversationMessage(left: ChatMessage, right: ChatMessage) {
  return (
    left.role === right.role &&
    normalizedAssistantText(left.content) === normalizedAssistantText(right.content)
  );
}

function mergeChatMessage(detailMessage: ChatMessage, timelineMessage: ChatMessage) {
  return {
    ...timelineMessage,
    id: detailMessage.id,
    role: detailMessage.role,
    content: detailMessage.content || timelineMessage.content,
    contentParts: detailMessage.contentParts || timelineMessage.contentParts,
    meta: detailMessage.meta || timelineMessage.meta,
    timestamp: timelineMessage.timestamp || detailMessage.timestamp,
    processSteps: timelineMessage.processSteps || detailMessage.processSteps,
  };
}

function contentPartsFromHistoryUserMessage(
  content: Record<string, unknown>,
): ChatMessage["contentParts"] | undefined {
  const text = stringFrom(content.message);
  const imageParts = imagePartsFromHistoryImages(content.images);
  if (imageParts.length === 0) {
    return undefined;
  }
  const parts: NonNullable<ChatMessage["contentParts"]> = [];
  if (text?.trim()) {
    parts.push({ type: "text", text });
  }
  parts.push(...imageParts);
  return parts;
}

function imagePartsFromHistoryImages(images: unknown) {
  if (!Array.isArray(images)) {
    return [];
  }
  return images.flatMap((image) => {
    if (!image || typeof image !== "object") {
      return [];
    }
    const record = image as Record<string, unknown>;
    const data = stringFrom(record.data);
    if (!data) {
      return [];
    }
    const mimeType =
      stringFrom(record.mime_type) || stringFrom(record.mimeType) || "image/png";
    const url = data.startsWith("data:") ? data : `data:${mimeType};base64,${data}`;
    return [{ type: "image_url" as const, image_url: { url, detail: "auto" } }];
  });
}

function hasImageContentParts(contentParts: ChatMessage["contentParts"] | undefined) {
  return (contentParts || []).some(
    (part) => part.type === "image_url" && Boolean(part.image_url?.url),
  );
}

function insertProcessStep(steps: ProcessStep[], step: ProcessStep) {
  if (step.type !== "thinking") {
    return [...steps, step];
  }
  let insertAt = steps.length;
  while (
    insertAt > 0 &&
    steps[insertAt - 1]?.type === "tool" &&
    steps[insertAt - 1]?.status === "running"
  ) {
    insertAt -= 1;
  }
  return [...steps.slice(0, insertAt), step, ...steps.slice(insertAt)];
}

function normalizedAssistantText(value: unknown) {
  if (typeof value !== "string") {
    return undefined;
  }
  const normalized = value.replace(/\s+/g, " ").trim();
  return normalized.length > 0 ? normalized : undefined;
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

function isActiveRunState(state?: string) {
  return isRunStateActive(state);
}

function isTerminalRunState(state?: string) {
  return state === "completed" || state === "failed" || state === "cancelled";
}

function historyEventToProcessStep(event: HistoryEvent): ProcessStep | null {
  if (event.event_type === "run_state_changed") {
    return null;
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
      callId: stringFrom(event.content.call_id) || stringFrom(event.content.callId),
      status: "running",
      startedAt: event.timestamp,
    };
  }
  if (event.event_type === "tool_result") {
    const name = stringFrom(event.content.tool_name) || "tool";
    return {
      type: "tool",
      summary: name,
      result: stringFrom(event.content.result),
      callId: stringFrom(event.content.call_id) || stringFrom(event.content.callId),
      status: event.content.success === false ? "failed" : "success",
      completedAt: event.timestamp,
    };
  }
  return null;
}

function durationMsBetween(start?: number, end?: number) {
  if (typeof start !== "number" || typeof end !== "number") {
    return undefined;
  }
  return Math.max(0, (normalizeTimestampSeconds(end) - normalizeTimestampSeconds(start)) * 1000);
}

function normalizeTimestampSeconds(timestamp: number) {
  return timestamp > 1_000_000_000_000 ? timestamp / 1000 : timestamp;
}

function isExternalCliAdapter(adapter?: string) {
  return adapter === "codex" || adapter === "traex" || adapter === "mock" || adapter === "custom";
}

function findMatchingToolStep(steps: ProcessStep[], callId?: string, name?: string) {
  for (let index = steps.length - 1; index >= 0; index -= 1) {
    const step = steps[index];
    if (step.type !== "tool") {
      continue;
    }
    if (callId && step.callId === callId) {
      return index;
    }
    if (!callId && name && step.summary === name) {
      return index;
    }
  }
  return -1;
}

function findPendingToolStep(steps: ProcessStep[], callId?: string, name?: string) {
  for (let index = steps.length - 1; index >= 0; index -= 1) {
    const step = steps[index];
    if (step.type !== "tool" || step.status !== "running") {
      continue;
    }
    if (callId && step.callId === callId) {
      return index;
    }
    if (!callId && name && step.summary === name) {
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
    if (event.event_type === "plan_cleared") {
      telemetry = {
        ...telemetry,
        plan: [],
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
  const threadState = thread?.run_state || thread?.state;
  const explicitIdleThread =
    thread?.running === false ||
    (thread?.running !== true && Boolean(threadState) && !isRunStateActive(threadState));
  const liveStatus = explicitIdleThread
    ? threadTelemetry.status
    : isThreadActiveForTelemetry(thread)
    ? threadTelemetry.status
    : isRunStatusActive(fallback.status)
      ? fallback.status
      : undefined;
  return {
    ...telemetry,
    phase:
      explicitIdleThread && telemetry.phase === "running" ? "idle" : telemetry.phase,
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
  if (thread?.running === false) {
    return false;
  }
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
