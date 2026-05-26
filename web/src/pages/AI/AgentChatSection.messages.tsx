import type { CSSProperties } from "react";
import { LoadingOutlined } from "@ant-design/icons";
import { Space, Typography } from "antd";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  ProcessStepsBlock,
  formatMessageTime,
  resolveAgentMarkdownImageSrc,
  type ChatMessage,
} from "./AgentChatSection.helpers";
import { RunnerCallChip } from "./AgentChatSection.runnerCall";

const { Text, Paragraph } = Typography;

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
  };
}) {
  return (
    <Space direction="vertical" size={12} style={{ width: "100%" }}>
      {messages.map((message) => {
        const isUser = message.role === "user";
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
                    padding: "10px 12px",
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
                  {!isUser && message.processSteps && message.processSteps.length > 0 && (
                    <ProcessStepsBlock
                      steps={message.processSteps}
                      running={
                        running &&
                        (message.content === "Agent is running..." ||
                          message.content === "Runner is running...")
                      }
                    />
                  )}
                  <Paragraph style={{ margin: 0 }}>
                    {(message.content === "Agent is running..." ||
                      message.content === "Runner is running...") &&
                    running ? (
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
                </div>
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
              </div>
            </div>
          </div>
        );
      })}
    </Space>
  );
}
