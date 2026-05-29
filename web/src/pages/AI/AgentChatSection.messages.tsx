import { useEffect, useState, type CSSProperties, type ReactNode } from "react";
import {
  CompressOutlined,
  DownOutlined,
  LoadingOutlined,
  RightOutlined,
} from "@ant-design/icons";
import { Space, Typography } from "antd";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  ProcessStepsBlock,
  formatMessageTime,
  resolveAgentMarkdownImageSrc,
  type ChatMessage,
  type ProcessStep,
} from "./AgentChatSection.helpers";
import { RunnerCallChip } from "./AgentChatSection.runnerCall";

const { Text, Paragraph } = Typography;
const PROCESS_ONLY_FALLBACK = "我先执行一步检查。";

type MessageItem = {
  message: ChatMessage;
  index: number;
};

type MessageGroup =
  | { type: "single"; item: MessageItem }
  | {
      type: "turn";
      key: string;
      user: MessageItem;
      assistants: MessageItem[];
      completed: boolean;
    };

function isVisibleMessage(message: ChatMessage, running: boolean) {
  if (message.runnerCall) {
    return true;
  }
  const hasImages = (message.contentParts || []).some((part) => part.type === "image_url");
  const hasProcessSteps = (message.processSteps?.length || 0) > 0;
  const isRunningPlaceholder =
    message.content === "Agent is running..." ||
    message.content === "Runner is running...";
  return (
    message.content.trim().length > 0 ||
    hasImages ||
    hasProcessSteps ||
    (running && isRunningPlaceholder)
  );
}

function isCompactionOnlyStatusMessage(message?: ChatMessage) {
  if (!message || message.role !== "assistant" || message.runnerCall) {
    return false;
  }
  const processSteps = message.processSteps || [];
  return (
    message.content.trim().length === 0 &&
    processSteps.length > 0 &&
    processSteps.every((step) => step.type === "compaction")
  );
}

export function AgentChatMessageList({
  isCompact,
  messages,
  onOpenRunnerCallThread,
  running,
  styles,
  token,
}: {
  isCompact: boolean;
  messages: ChatMessage[];
  onOpenRunnerCallThread?: (message: ChatMessage) => void;
  running: boolean;
  styles: Record<string, CSSProperties>;
  token: {
    colorPrimaryBg: string;
    colorBorderSecondary: string;
    colorTextSecondary: string;
    colorTextTertiary: string;
    colorFillQuaternary: string;
  };
}) {
  const lastAssistantIndex = messages.reduce(
    (lastIndex, message, index) =>
      message.role === "assistant" && isVisibleMessage(message, running)
        ? index
        : lastIndex,
    -1,
  );
  const lastAssistantIsCompactionOnlyStatus = isCompactionOnlyStatusMessage(
    messages[lastAssistantIndex],
  );
  const groups = buildMessageGroups(messages, running);
  const renderMessage = (
    message: ChatMessage,
    index: number,
    options: { forceProcessCompleted?: boolean; hideProcessSteps?: boolean } = {},
  ) => {
    const isUser = message.role === "user";
    const isRunningPlaceholder =
      !isUser &&
      (message.content === "Agent is running..." ||
        message.content === "Runner is running...");
    const processSteps = options.hideProcessSteps ? [] : message.processSteps || [];
    const compactionSteps = processSteps.filter((step) => step.type === "compaction");
    const executionSteps = processSteps.filter((step) => step.type !== "compaction");
    const hasProcessSteps = processSteps.length > 0;
    const hasExecutionSteps = executionSteps.length > 0;
    const shouldShowGenerating = isRunningPlaceholder && running && !hasProcessSteps;
    const shouldShowContent =
      shouldShowGenerating ||
      (message.content.trim().length > 0 &&
        !(isRunningPlaceholder && hasProcessSteps));
    const imageParts = (message.contentParts || []).filter(
      (part): part is Extract<typeof part, { type: "image_url" }> =>
        part.type === "image_url" && Boolean(part.image_url?.url),
    );
    const shouldShowProcessFallback =
      !isUser && hasExecutionSteps && !shouldShowContent && !message.runnerCall;
    const isCompactionOnlyStatus =
      !isUser &&
      compactionSteps.length > 0 &&
      !hasExecutionSteps &&
      !shouldShowContent &&
      !message.runnerCall;
    const shouldShowThinkingTail =
      running &&
      !isUser &&
      index === lastAssistantIndex &&
      !shouldShowGenerating &&
      !isCompactionOnlyStatus;
    if (!isVisibleMessage({ ...message, processSteps }, running)) {
      return null;
    }
    return (
      <div
        key={message.id}
        data-testid={`agent-chat-message-${message.role}`}
        style={{
          display: "flex",
          width: "100%",
          minWidth: 0,
          justifyContent: isUser ? "flex-end" : "flex-start",
        }}
      >
        <div
          style={{
            display: "flex",
            flexDirection: "row",
            alignItems: "flex-start",
            width: isUser ? "auto" : "100%",
            minWidth: 0,
            maxWidth: isUser ? (isCompact ? "100%" : "78%") : "100%",
          }}
        >
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              flex: isUser ? "0 1 auto" : "1 1 auto",
              minWidth: 0,
              alignItems: isUser ? "flex-end" : "stretch",
            }}
          >
            <div
              data-testid={`agent-chat-message-bubble-${message.role}`}
              style={{
                width: isUser ? "auto" : "100%",
                minWidth: 0,
                border: "none",
                borderRadius: 12,
                padding: isUser ? "10px 12px" : "2px 0",
                background: isUser ? token.colorPrimaryBg : "transparent",
                overflowWrap: "anywhere",
              }}
            >
              {message.runnerCall ? (
                <RunnerCallChip
                  role={message.role}
                  runnerCall={message.runnerCall}
                  onOpenThread={
                    message.runnerCall.childSessionKey && onOpenRunnerCallThread
                      ? () => onOpenRunnerCallThread(message)
                      : undefined
                  }
                  style={styles.runnerCallChip}
                />
              ) : null}
              {shouldShowContent ? (
                <Paragraph style={{ margin: hasProcessSteps ? "0 0 4px" : 0 }}>
                  {shouldShowGenerating ? (
                    <Text type="secondary" italic>
                      <LoadingOutlined style={{ marginRight: 6 }} />
                      Generating...
                    </Text>
                  ) : (
                    <div className="agent-chat-markdown">
                      <Markdown
                        remarkPlugins={[remarkGfm]}
                        components={{
                          a: ({ href, children }) => (
                            <a href={href} target="_blank" rel="noreferrer">
                              {children}
                            </a>
                          ),
                          img: ({ src, alt }) => (
                            <img
                              src={resolveAgentMarkdownImageSrc(src)}
                              alt={alt || ""}
                            />
                          ),
                        }}
                      >
                        {message.content}
                      </Markdown>
                    </div>
                  )}
                </Paragraph>
              ) : null}
              {imageParts.length > 0 ? (
                <div style={styles.messageImageGrid} data-testid="agent-chat-message-images">
                  {imageParts.map((part, imageIndex) => (
                    <img
                      key={`${message.id}-image-${imageIndex}`}
                      src={resolveAgentMarkdownImageSrc(part.image_url?.url)}
                      alt={`Attached image ${imageIndex + 1}`}
                      style={styles.messageImageThumb}
                    />
                  ))}
                </div>
              ) : null}
              {shouldShowProcessFallback ? (
                <Paragraph style={{ margin: "0 0 4px" }}>
                  <Text>{PROCESS_ONLY_FALLBACK}</Text>
                </Paragraph>
              ) : null}
              {!isUser && hasExecutionSteps ? (
                <ProcessStepsBlock
                  steps={executionSteps}
                  running={
                    !options.forceProcessCompleted &&
                    running &&
                    !lastAssistantIsCompactionOnlyStatus &&
                    executionSteps.some((step) => step.status === "running")
                  }
                />
              ) : null}
              {!isUser && compactionSteps.length > 0
                ? compactionSteps.map((step, compactionIndex) => (
                    <CompactionDivider
                      key={`compaction-${compactionIndex}-${step.status || "done"}`}
                      step={step}
                      token={token}
                    />
                  ))
                : null}
              {shouldShowThinkingTail ? (
                <Text
                  type="secondary"
                  italic
                  data-testid="agent-chat-thinking-tail"
                  style={{ display: "inline-flex", alignItems: "center", marginTop: 2 }}
                >
                  <LoadingOutlined style={{ marginRight: 6 }} />
                  Thinking...
                </Text>
              ) : null}
            </div>
            {!message.hideTimestamp ? (
              <Text
                type="secondary"
                title={formatMessageTime(message.timestamp)}
                data-testid="agent-chat-message-time"
                style={{
                  display: "block",
                  marginTop: 4,
                  fontSize: 11,
                  alignSelf: isUser ? "flex-end" : "flex-start",
                }}
              >
                {formatMessageTime(message.timestamp)}
              </Text>
            ) : null}
          </div>
        </div>
      </div>
    );
  };
  return (
    <Space direction="vertical" size={12} style={{ width: "100%", minWidth: 0 }}>
      {groups.map((group) =>
        group.type === "single" ? (
          renderMessage(group.item.message, group.item.index)
        ) : group.completed ? (
          <CompletedTurnGroup
            key={group.key}
            group={group}
            renderMessage={renderMessage}
            token={token}
          />
        ) : (
          <RunningTurnGroup
            key={group.key}
            group={group}
            renderMessage={renderMessage}
            token={token}
          />
        ),
      )}
    </Space>
  );
}

function buildMessageGroups(messages: ChatMessage[], running: boolean): MessageGroup[] {
  const groups: MessageGroup[] = [];
  let openTurn: Extract<MessageGroup, { type: "turn" }> | undefined;

  const flushTurn = (completed: boolean) => {
    if (!openTurn) {
      return;
    }
    if (openTurn.assistants.length === 0) {
      groups.push({ type: "single", item: openTurn.user });
    } else {
      groups.push({ ...openTurn, completed });
    }
    openTurn = undefined;
  };

  messages.forEach((message, index) => {
    if (message.role === "user") {
      flushTurn(true);
      openTurn = {
        type: "turn",
        key: `turn-${message.id}`,
        user: { message, index },
        assistants: [],
        completed: true,
      };
      return;
    }
    if (openTurn) {
      openTurn.assistants.push({ message, index });
      return;
    }
    groups.push({ type: "single", item: { message, index } });
  });
  flushTurn(!running);

  if (running) {
    const lastTurnOffset = [...groups]
      .reverse()
      .findIndex((group) => group.type === "turn");
    if (lastTurnOffset >= 0) {
      const index = groups.length - 1 - lastTurnOffset;
      const group = groups[index];
      if (group.type === "turn") {
        groups[index] = { ...group, completed: false };
      }
    }
  }
  return groups;
}

function RunningTurnGroup({
  group,
  renderMessage,
  token,
}: {
  group: Extract<MessageGroup, { type: "turn" }>;
  renderMessage: (
    message: ChatMessage,
    index: number,
    options?: { forceProcessCompleted?: boolean; hideProcessSteps?: boolean },
  ) => ReactNode;
  token: {
    colorBorderSecondary: string;
    colorTextSecondary: string;
  };
}) {
  const [mountedAtSeconds] = useState(() => Date.now() / 1000);
  const [nowSeconds, setNowSeconds] = useState(() => Date.now() / 1000);

  useEffect(() => {
    const timer = window.setInterval(() => {
      setNowSeconds(Date.now() / 1000);
    }, 1000);
    return () => window.clearInterval(timer);
  }, []);

  const durationLabel = formatTurnDuration(
    runningDurationSeconds(
      group.user.message.timestamp,
      group.assistants,
      nowSeconds,
      mountedAtSeconds,
    ),
  );
  return (
    <div
      data-testid="agent-chat-turn-running-group"
      style={{ display: "grid", gap: 12, minWidth: 0, width: "100%" }}
    >
      {renderMessage(group.user.message, group.user.index)}
      <div
        data-testid="agent-chat-turn-running-summary"
        aria-live="polite"
        style={{
          margin: "8px 0 0",
          padding: "0 0 12px",
          borderBottom: `1px solid ${token.colorBorderSecondary}`,
          color: token.colorTextSecondary,
          fontWeight: 600,
        }}
      >
        已处理 {durationLabel}
      </div>
      {group.assistants.map((item) => renderMessage(item.message, item.index))}
    </div>
  );
}

function CompletedTurnGroup({
  group,
  renderMessage,
  token,
}: {
  group: Extract<MessageGroup, { type: "turn" }>;
  renderMessage: (
    message: ChatMessage,
    index: number,
    options?: { forceProcessCompleted?: boolean; hideProcessSteps?: boolean },
  ) => ReactNode;
  token: {
    colorTextSecondary: string;
    colorTextTertiary: string;
  };
}) {
  const [expanded, setExpanded] = useState(false);
  const finalAssistantItem = findFinalAssistantItem(group.assistants);
  const durationLabel = formatTurnDuration(
    durationSeconds(group.user.message.timestamp, finalAssistantItem?.message.timestamp),
  );
  return (
    <div data-testid="agent-chat-turn-group" style={{ width: "100%", minWidth: 0 }}>
      {renderMessage(group.user.message, group.user.index)}
      <button
        type="button"
        data-testid="agent-chat-turn-collapse-toggle"
        aria-expanded={expanded}
        onClick={() => setExpanded((value) => !value)}
        style={{
          display: "inline-flex",
          alignItems: "center",
          gap: 8,
          margin: "8px 0 8px",
          padding: 0,
          border: 0,
          background: "transparent",
          color: token.colorTextTertiary,
          cursor: "pointer",
          font: "inherit",
        }}
      >
        <span style={{ fontWeight: 600, color: token.colorTextSecondary }}>
          已处理 {durationLabel}
        </span>
        {expanded ? (
          <DownOutlined style={{ fontSize: 12 }} />
        ) : (
          <RightOutlined style={{ fontSize: 12 }} />
        )}
      </button>
      {expanded
        ? group.assistants.map((item) =>
            renderMessage(item.message, item.index, { forceProcessCompleted: true }),
          )
        : finalAssistantItem
          ? renderMessage(finalAssistantItem.message, finalAssistantItem.index, {
              forceProcessCompleted: true,
              hideProcessSteps: true,
            })
          : null}
    </div>
  );
}

function findFinalAssistantItem(items: MessageItem[]) {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const message = items[index].message;
    const hasImages = (message.contentParts || []).some((part) => part.type === "image_url");
    if (message.content.trim().length > 0 || hasImages || message.runnerCall) {
      return items[index];
    }
  }
  return items[items.length - 1];
}

function durationSeconds(start?: number, end?: number) {
  if (!start || !end) {
    return undefined;
  }
  return Math.max(0, normalizeTimestampSeconds(end) - normalizeTimestampSeconds(start));
}

function runningDurationSeconds(
  start: number | undefined,
  assistants: MessageItem[],
  now: number,
  mountedAt: number,
) {
  if (!start) {
    return undefined;
  }
  const normalizedStart = normalizeTimestampSeconds(start);
  if (normalizedStart < 1_000_000_000) {
    const lastAssistantTimestamp = assistants
      .map((item) => item.message.timestamp)
      .filter((value): value is number => typeof value === "number")
      .map(normalizeTimestampSeconds)
      .reduce<number | undefined>(
        (latest, value) => (latest === undefined ? value : Math.max(latest, value)),
        undefined,
      );
    const timelineSeconds =
      lastAssistantTimestamp === undefined
        ? 0
        : Math.max(0, lastAssistantTimestamp - normalizedStart);
    return timelineSeconds + Math.max(0, normalizeTimestampSeconds(now) - mountedAt);
  }
  return Math.max(0, normalizeTimestampSeconds(now) - normalizedStart);
}

function normalizeTimestampSeconds(timestamp: number) {
  return timestamp > 1_000_000_000_000 ? timestamp / 1000 : timestamp;
}

function formatTurnDuration(seconds?: number) {
  if (seconds === undefined || !Number.isFinite(seconds)) {
    return "<1s";
  }
  const rounded = Math.max(0, Math.round(seconds));
  if (rounded < 1) {
    return "<1s";
  }
  const hours = Math.floor(rounded / 3600);
  const minutes = Math.floor((rounded % 3600) / 60);
  const secs = rounded % 60;
  if (hours > 0) {
    return secs > 0 ? `${hours}h ${minutes}m ${secs}s` : `${hours}h ${minutes}m`;
  }
  if (minutes > 0) {
    return secs > 0 ? `${minutes}m ${secs}s` : `${minutes}m`;
  }
  return `${secs}s`;
}

function CompactionDivider({
  step,
  token,
}: {
  step: ProcessStep;
  token: {
    colorBorderSecondary: string;
    colorTextTertiary: string;
    colorFillQuaternary: string;
  };
}) {
  const label =
    step.status === "running"
      ? "上下文正在自动压缩"
      : step.status === "failed"
        ? "上下文压缩失败"
        : "上下文已自动压缩";
  return (
    <div
      data-testid="agent-chat-compaction-divider"
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        margin: "14px 0 12px",
        color: token.colorTextTertiary,
      }}
      title={step.detail || step.summary}
    >
      <span style={{ height: 1, flex: 1, background: token.colorBorderSecondary }} />
      <span
        style={{
          display: "inline-flex",
          alignItems: "center",
          gap: 6,
          padding: "2px 8px",
          borderRadius: 6,
          background: token.colorFillQuaternary,
          fontSize: 12,
          lineHeight: "18px",
          whiteSpace: "nowrap",
        }}
      >
        <CompressOutlined style={{ fontSize: 12 }} />
        {label}
      </span>
      <span style={{ height: 1, flex: 1, background: token.colorBorderSecondary }} />
    </div>
  );
}
