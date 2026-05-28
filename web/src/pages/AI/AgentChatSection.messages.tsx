import type { CSSProperties } from "react";
import { CompressOutlined, LoadingOutlined } from "@ant-design/icons";
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

function isVisibleMessage(message: ChatMessage, running: boolean) {
  if (message.runnerCall) {
    return true;
  }
  const hasProcessSteps = (message.processSteps?.length || 0) > 0;
  const isRunningPlaceholder =
    message.content === "Agent is running..." ||
    message.content === "Runner is running...";
  return message.content.trim().length > 0 || hasProcessSteps || (running && isRunningPlaceholder);
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
  return (
    <Space direction="vertical" size={12} style={{ width: "100%" }}>
      {messages.map((message, index) => {
        const isUser = message.role === "user";
        const isRunningPlaceholder =
          !isUser &&
          (message.content === "Agent is running..." ||
            message.content === "Runner is running...");
        const processSteps = message.processSteps || [];
        const compactionSteps = processSteps.filter((step) => step.type === "compaction");
        const executionSteps = processSteps.filter((step) => step.type !== "compaction");
        const hasProcessSteps = processSteps.length > 0;
        const hasExecutionSteps = executionSteps.length > 0;
        const shouldShowGenerating = isRunningPlaceholder && running && !hasProcessSteps;
        const shouldShowContent =
          shouldShowGenerating ||
          (message.content.trim().length > 0 &&
            !(isRunningPlaceholder && hasProcessSteps));
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
        if (!isVisibleMessage(message, running)) {
          return null;
        }
        return (
          <div
            key={message.id}
            data-testid={`agent-chat-message-${message.role}`}
            style={{
              display: "flex",
              justifyContent: isUser ? "flex-end" : "flex-start",
            }}
          >
            <div
              style={{
                display: "flex",
                flexDirection: "row",
                alignItems: "flex-start",
                width: isUser ? "auto" : "100%",
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
                    border: "none",
                    borderRadius: 12,
                    padding: isUser ? "10px 12px" : "2px 0",
                    background: isUser ? token.colorPrimaryBg : "transparent",
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
                  {shouldShowProcessFallback ? (
                    <Paragraph style={{ margin: "0 0 4px" }}>
                      <Text>{PROCESS_ONLY_FALLBACK}</Text>
                    </Paragraph>
                  ) : null}
                  {!isUser && hasExecutionSteps ? (
                    <ProcessStepsBlock
                      steps={executionSteps}
                      running={
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
      })}
    </Space>
  );
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
