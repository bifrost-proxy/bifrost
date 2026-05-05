/**
 * Session Detail Page - Sub-page view for session details
 * Shows: session metadata, AGENTS.md, Skills, Messages/Events
 */
import { useCallback, useEffect, useRef, useState } from "react";
import {
  Button,
  Card,
  Col,
  Descriptions,
  Empty,
  Image,
  Modal,
  Row,
  Space,
  Spin,
  Tag,
  Tooltip,
  Typography,
  theme,
} from "antd";
import { ExpandOutlined } from "@ant-design/icons";
import {
  ArrowLeftOutlined,
  ClockCircleOutlined,
  CodeOutlined,
  FileTextOutlined,
  PlayCircleOutlined,
  ReadOutlined,
  ReloadOutlined,
  RobotOutlined,
  StopOutlined,
  CompressOutlined,
  CheckCircleOutlined,
  CloseCircleOutlined,
  CloudOutlined,
  ApiOutlined,
  ToolOutlined,
  UserOutlined,
} from "@ant-design/icons";
import { get } from "../../../../api/client";
import {
  BASE,
  type SessionDetail,
  type ConversationEvent,
  type SkillInfo,
} from "./types";

const { Text, Title } = Typography;

function extractImageUrlsFromParts(parts: unknown): string[] {
  if (!Array.isArray(parts)) return [];
  return parts
    .map((part) => {
      if (!part || typeof part !== "object") return null;
      const record = part as Record<string, unknown>;
      if (record.type !== "image_url") return null;
      const imageUrl = record.image_url;
      if (typeof imageUrl === "string") return imageUrl;
      if (imageUrl && typeof imageUrl === "object") {
        const url = (imageUrl as Record<string, unknown>).url;
        return typeof url === "string" ? url : null;
      }
      return null;
    })
    .filter((url): url is string => Boolean(url));
}

function extractPersistedImageUrls(content: Record<string, unknown> | null): string[] {
  const images = content?.images;
  if (!Array.isArray(images)) return [];
  return images
    .map((image) => {
      if (!image || typeof image !== "object") return null;
      const record = image as Record<string, unknown>;
      const data = record.data;
      if (typeof data !== "string" || data.length === 0) return null;
      if (data.startsWith("data:")) return data;
      const mimeType =
        typeof record.mime_type === "string" && record.mime_type
          ? record.mime_type
          : "image/png";
      return `data:${mimeType};base64,${data}`;
    })
    .filter((url): url is string => Boolean(url));
}

function MessageImages({ urls }: { urls: string[] }) {
  if (urls.length === 0) return null;
  return (
    <Image.PreviewGroup>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 8, marginTop: 6 }}>
        {urls.map((url, idx) => (
          <Image
            key={`${idx}-${url.slice(0, 32)}`}
            src={url}
            alt={`message attachment ${idx + 1}`}
            width={220}
            height={180}
            style={{
              objectFit: "contain",
              borderRadius: 6,
              border: "1px solid var(--ant-color-border, #d9d9d9)",
              background: "var(--ant-color-bg-container, #ffffff)",
              cursor: "zoom-in",
            }}
            preview={{
              mask: <ExpandOutlined />,
            }}
          />
        ))}
      </div>
    </Image.PreviewGroup>
  );
}

/** Clamp text content to N lines; show expand button + modal for full content */
function ClampedContent({
  children,
  maxLines = 4,
  title,
}: {
  children: string;
  maxLines?: number;
  title?: string;
}) {
  const contentRef = useRef<HTMLDivElement>(null);
  const [clamped, setClamped] = useState(false);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    const el = contentRef.current;
    if (el) {
      setClamped(el.scrollHeight > el.clientHeight + 1);
    }
  }, [children]);

  return (
    <>
      <div style={{ position: "relative" }}>
        <div
          ref={contentRef}
          style={{
            fontSize: 12,
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
            display: "-webkit-box",
            WebkitLineClamp: maxLines,
            WebkitBoxOrient: "vertical",
            overflow: "hidden",
          }}
        >
          {children}
        </div>
        {clamped && (
          <Button
            type="link"
            size="small"
            icon={<ExpandOutlined />}
            onClick={() => setOpen(true)}
            style={{ padding: 0, height: "auto", fontSize: 11, marginTop: 2 }}
          >
            Show more
          </Button>
        )}
      </div>
      <Modal
        title={title || "Full Content"}
        open={open}
        onCancel={() => setOpen(false)}
        footer={null}
        width={720}
      >
        <pre
          style={{
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
            fontSize: 12,
            maxHeight: "70vh",
            overflow: "auto",
            margin: 0,
          }}
        >
          {children}
        </pre>
      </Modal>
    </>
  );
}

interface SessionDetailPageProps {
  sessionKey: string;
  /** "active" for in-memory session, "history" for persisted JSONL file */
  view: "active" | "history";
  /** For history view, the file path to load */
  historyFilePath?: string;
  onBack: () => void;
}

export default function SessionDetailPage({
  sessionKey,
  view,
  historyFilePath,
  onBack,
}: SessionDetailPageProps) {
  const { token: themeToken } = theme.useToken();

  // Active session detail
  const [sessionDetail, setSessionDetail] = useState<SessionDetail | null>(null);
  const [sessionLoading, setSessionLoading] = useState(false);

  // History events
  const [events, setEvents] = useState<ConversationEvent[]>([]);
  const [eventsLoading, setEventsLoading] = useState(false);

  // AGENTS.md content
  const [agentsContent, setAgentsContent] = useState<string | null>(null);
  const [agentsLoading, setAgentsLoading] = useState(false);

  // Skills
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  const [skillsLoading, setSkillsLoading] = useState(false);

  const fetchActiveSession = useCallback(async () => {
    setSessionLoading(true);
    try {
      const data = await get<SessionDetail>(
        `${BASE}/agent/sessions/${encodeURIComponent(sessionKey)}`,
      );
      setSessionDetail(data);
    } catch {
      // silent
    } finally {
      setSessionLoading(false);
    }
  }, [sessionKey]);

  const fetchHistoryEvents = useCallback(async () => {
    if (!historyFilePath) return;
    setEventsLoading(true);
    try {
      const data = await get<{ events: ConversationEvent[]; count: number }>(
        `${BASE}/agent/sessions/history/${encodeURIComponent(historyFilePath)}`,
      );
      setEvents(data.events || []);
    } catch {
      // silent
    } finally {
      setEventsLoading(false);
    }
  }, [historyFilePath]);

  const fetchAgentsContent = useCallback(async () => {
    setAgentsLoading(true);
    try {
      const data = await get<{ content: string | null; work_dir: string }>(
        `${BASE}/agent/instructions`,
      );
      setAgentsContent(data.content);
    } catch {
      // silent
    } finally {
      setAgentsLoading(false);
    }
  }, []);

  const fetchSkills = useCallback(async () => {
    setSkillsLoading(true);
    try {
      const data = await get<{ skills: SkillInfo[]; work_dir: string; home_dir: string }>(
        `${BASE}/agent/skills`,
      );
      setSkills(data.skills || []);
    } catch {
      // silent
    } finally {
      setSkillsLoading(false);
    }
  }, []);

  useEffect(() => {
    if (view === "active") {
      fetchActiveSession();
    } else {
      fetchHistoryEvents();
    }
    fetchAgentsContent();
    fetchSkills();
  }, [view, fetchActiveSession, fetchHistoryEvents, fetchAgentsContent, fetchSkills]);

  const formatTs = (ts?: number) => {
    if (!ts) return "-";
    return new Date(ts * 1000).toLocaleString();
  };

  const formatDuration = (startSecs?: number, endSecs?: number) => {
    if (!startSecs || !endSecs || endSecs <= startSecs) return "-";
    const diff = endSecs - startSecs;
    if (diff < 60) return `${diff}s`;
    if (diff < 3600) return `${Math.floor(diff / 60)}m ${diff % 60}s`;
    return `${Math.floor(diff / 3600)}h ${Math.floor((diff % 3600) / 60)}m`;
  };

  const sourceTag = (source?: string) => {
    if (!source || source === "unknown") return <Tag>Unknown</Tag>;
    if (source === "feishu") return <Tag icon={<CloudOutlined />} color="blue">Feishu</Tag>;
    if (source === "api") return <Tag icon={<ApiOutlined />} color="green">API</Tag>;
    return <Tag>{source}</Tag>;
  };

  const isLoading = sessionLoading || eventsLoading;

  return (
    <div>
      {/* Header with back button */}
      <div style={{ marginBottom: 16, display: "flex", alignItems: "center", gap: 12 }}>
        <Button icon={<ArrowLeftOutlined />} onClick={onBack}>
          Back
        </Button>
        <Title level={5} style={{ margin: 0 }}>
          Session Detail
        </Title>
        <Text code style={{ fontSize: 12 }}>{sessionKey}</Text>
        {view === "active" && <Tag color="green">Active</Tag>}
        {view === "history" && <Tag color="default">History</Tag>}
      </div>

      {isLoading ? (
        <Spin style={{ display: "block", margin: "60px auto" }} />
      ) : (
        <Row gutter={[16, 16]}>
          {/* Session Metadata */}
          <Col xs={24}>
            <Card title={<Space><RobotOutlined /><span>Session Info</span></Space>} size="small">
              {view === "active" && sessionDetail ? (
                <Descriptions size="small" column={{ xs: 1, sm: 2, md: 3 }} bordered>
                  <Descriptions.Item label="Source">{sourceTag(sessionDetail.source)}</Descriptions.Item>
                  <Descriptions.Item label="Work Dir">
                    {sessionDetail.work_dir ? (
                      <Tooltip title={sessionDetail.work_dir}>
                        <Text code style={{ fontSize: 11 }}>{sessionDetail.work_dir}</Text>
                      </Tooltip>
                    ) : (
                      <Text type="secondary">default</Text>
                    )}
                  </Descriptions.Item>
                  <Descriptions.Item label="Messages">{sessionDetail.message_count}</Descriptions.Item>
                  <Descriptions.Item label="Total Tokens">
                    {sessionDetail.total_tokens_used != null ? sessionDetail.total_tokens_used.toLocaleString() : "-"}
                  </Descriptions.Item>
                  <Descriptions.Item label="Est. Tokens">
                    {sessionDetail.estimated_tokens?.toLocaleString() ?? "-"}
                  </Descriptions.Item>
                  <Descriptions.Item label="Compactions">{sessionDetail.compaction_count}</Descriptions.Item>
                  <Descriptions.Item label="Created">{formatTs(sessionDetail.created_at)}</Descriptions.Item>
                  <Descriptions.Item label="Last Active">{formatTs(sessionDetail.last_active_at)}</Descriptions.Item>
                  <Descriptions.Item label="Duration">
                    {formatDuration(sessionDetail.created_at, sessionDetail.last_active_at)}
                  </Descriptions.Item>
                </Descriptions>
              ) : view === "history" && events.length > 0 ? (
                (() => {
                  const startEvt = events.find(e => e.event_type === "session_start");
                  const endEvt = [...events].reverse().find(e => e.event_type === "session_end");
                  const content = startEvt?.content as Record<string, unknown> | null;
                  const endContent = endEvt?.content as Record<string, unknown> | null;
                  const userMsgs = events.filter(e => e.event_type === "user_message").length;
                  const assistantMsgs = events.filter(e => e.event_type === "assistant_message").length;
                  const toolCalls = events.filter(e => e.event_type === "tool_call").length;
                  const startTime = events[0]?.timestamp;
                  const endTime = events[events.length - 1]?.timestamp;

                  return (
                    <Descriptions size="small" column={{ xs: 1, sm: 2, md: 3 }} bordered>
                      <Descriptions.Item label="Source">
                        {sourceTag(content?.source as string | undefined)}
                      </Descriptions.Item>
                      <Descriptions.Item label="Work Dir">
                        {content?.work_dir ? (
                          <Text code style={{ fontSize: 11 }}>{content.work_dir as string}</Text>
                        ) : (
                          <Text type="secondary">default</Text>
                        )}
                      </Descriptions.Item>
                      <Descriptions.Item label="Turns">
                        <Space size={4}>
                          <Tag color="green">{userMsgs} user</Tag>
                          <Tag color="blue">{assistantMsgs} assistant</Tag>
                        </Space>
                      </Descriptions.Item>
                      <Descriptions.Item label="Tool Calls">{toolCalls}</Descriptions.Item>
                      <Descriptions.Item label="Total Tokens">
                        {(endContent?.total_tokens as number)?.toLocaleString() ?? "-"}
                      </Descriptions.Item>
                      <Descriptions.Item label="Events">{events.length}</Descriptions.Item>
                      <Descriptions.Item label="Start Time">{formatTs(startTime)}</Descriptions.Item>
                      <Descriptions.Item label="End Time">{formatTs(endTime)}</Descriptions.Item>
                      <Descriptions.Item label="Duration">
                        {formatDuration(startTime, endTime)}
                      </Descriptions.Item>
                    </Descriptions>
                  );
                })()
              ) : (
                <Empty description="No session data" />
              )}
            </Card>
          </Col>

          {/* AGENTS.md Instructions */}
          <Col xs={24}>
            <Card
              title={<Space><FileTextOutlined /><span>AGENTS.md Instructions</span></Space>}
              size="small"
              extra={
                <Button icon={<ReloadOutlined />} size="small" onClick={fetchAgentsContent} loading={agentsLoading}>
                  Refresh
                </Button>
              }
            >
              {agentsContent ? (
                <div
                  style={{
                    background: themeToken.colorFillQuaternary,
                    borderRadius: 6,
                    padding: 12,
                    maxHeight: 300,
                    overflowY: "auto",
                  }}
                >
                  <pre style={{ margin: 0, fontSize: 11, whiteSpace: "pre-wrap", color: themeToken.colorText }}>
                    {agentsContent}
                  </pre>
                </div>
              ) : (
                <Empty description="No AGENTS.md found" />
              )}
            </Card>
          </Col>

          {/* Skills */}
          <Col xs={24}>
            <Card
              title={
                <Space>
                  <ReadOutlined />
                  <span>Skills</span>
                  <Tag>{skills.length}</Tag>
                </Space>
              }
              size="small"
              extra={
                <Button icon={<ReloadOutlined />} size="small" onClick={fetchSkills} loading={skillsLoading}>
                  Refresh
                </Button>
              }
            >
              {skills.length > 0 ? (
                <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
                  {(["repo", "user", "system"] as const).map((scope) => {
                    const filtered = skills.filter((s) => s.scope === scope);
                    if (filtered.length === 0) return null;
                    const label = scope === "repo" ? "Workspace" : scope === "user" ? "User" : "System";
                    return (
                      <div key={scope}>
                        <Text type="secondary" style={{ fontSize: 11 }}>
                          {label} ({filtered.length})
                        </Text>
                        <div style={{ display: "flex", flexWrap: "wrap", gap: 4, marginTop: 4 }}>
                          {filtered.map((s) => (
                            <Tooltip key={s.name} title={`${s.short_description || s.description}\n${s.path}`}>
                              <Tag>{s.name}</Tag>
                            </Tooltip>
                          ))}
                        </div>
                      </div>
                    );
                  })}
                </div>
              ) : (
                <Empty description="No skills loaded" />
              )}
            </Card>
          </Col>

          {/* Messages / Events Timeline */}
          <Col xs={24}>
            <Card
              title={
                <Space>
                  <ClockCircleOutlined />
                  <span>{view === "active" ? "Messages" : "Event Timeline"}</span>
                  <Tag>
                    {view === "active"
                      ? `${sessionDetail?.messages?.length ?? 0} messages`
                      : `${events.length} events`}
                  </Tag>
                </Space>
              }
              size="small"
              extra={
                view === "active" ? (
                  <Button icon={<ReloadOutlined />} size="small" onClick={fetchActiveSession} loading={sessionLoading}>
                    Refresh
                  </Button>
                ) : null
              }
            >
              <div
                style={{
                  maxHeight: 600,
                  overflowY: "auto",
                  display: "flex",
                  flexDirection: "column",
                  gap: 8,
                }}
              >
                {view === "active" ? (
                  sessionDetail?.messages && sessionDetail.messages.length > 0 ? (
                    sessionDetail.messages.map((msg, idx) => {
                      const imageUrls = extractImageUrlsFromParts(msg.content_parts);
                      return (
                        <Card
                          key={idx}
                          size="small"
                          style={{
                            borderColor:
                              msg.role === "assistant"
                                ? themeToken.colorPrimaryBorder
                                : msg.role === "user"
                                  ? themeToken.colorSuccessBorder
                                  : themeToken.colorBorderSecondary,
                            background:
                              msg.role === "assistant"
                                ? themeToken.colorPrimaryBg
                                : msg.role === "user"
                                  ? themeToken.colorSuccessBg
                                  : undefined,
                          }}
                        >
                          <Space direction="vertical" size={2} style={{ width: "100%" }}>
                            <Tag
                              color={
                                msg.role === "assistant" ? "blue"
                                  : msg.role === "user" ? "green"
                                  : msg.role === "system" ? "purple"
                                  : "default"
                              }
                              style={{ fontSize: 11 }}
                            >
                              {msg.role}
                            </Tag>
                            {msg.content ? (
                              <ClampedContent title={`${msg.role} message`}>
                                {msg.content}
                              </ClampedContent>
                            ) : (
                              <Text style={{ fontSize: 12 }}>(empty)</Text>
                            )}
                            <MessageImages urls={imageUrls} />
                            {msg.tool_calls && msg.tool_calls.length > 0 && (
                              <div>
                                <Text type="secondary" style={{ fontSize: 11 }}>Tool calls:</Text>
                                {msg.tool_calls.map((tc, i) => (
                                  <Text key={i} code style={{ fontSize: 10, display: "block", marginTop: 2 }}>
                                    {tc}
                                  </Text>
                                ))}
                              </div>
                            )}
                          </Space>
                        </Card>
                      );
                    })
                  ) : (
                    <Empty description="No messages" />
                  )
                ) : (
                  events.length > 0 ? (
                    events.map((evt, idx) => (
                      <HistoryEventCard key={idx} event={evt} token={themeToken} />
                    ))
                  ) : (
                    <Empty description="No events" />
                  )
                )}
              </div>
            </Card>
          </Col>
        </Row>
      )}
    </div>
  );
}

// Inline event card renderer for history view
function HistoryEventCard({
  event,
  token,
}: {
  event: ConversationEvent;
  token: ReturnType<typeof theme.useToken>["token"];
}) {
  const content = event.content as Record<string, unknown> | null;
  const ts = new Date(event.timestamp * 1000).toLocaleTimeString();

  // Map event types to icons and colors
  const configs: Record<string, { icon: React.ReactNode; color: string; borderColor: string; bg: string }> = {
    session_start: {
      icon: <PlayCircleOutlined style={{ color: token.colorTextSecondary }} />,
      color: "default",
      borderColor: token.colorBorderSecondary,
      bg: token.colorFillQuaternary,
    },
    user_message: {
      icon: <UserOutlined style={{ color: token.colorSuccess }} />,
      color: "green",
      borderColor: token.colorSuccessBorder,
      bg: token.colorSuccessBg,
    },
    assistant_message: {
      icon: <RobotOutlined style={{ color: token.colorPrimary }} />,
      color: "blue",
      borderColor: token.colorPrimaryBorder,
      bg: token.colorPrimaryBg,
    },
    tool_call: {
      icon: <ToolOutlined style={{ color: token.colorWarning }} />,
      color: "orange",
      borderColor: token.colorWarningBorder,
      bg: token.colorWarningBg,
    },
    tool_result: {
      icon: content?.success
        ? <CheckCircleOutlined style={{ color: token.colorSuccess }} />
        : <CloseCircleOutlined style={{ color: token.colorError }} />,
      color: content?.success ? "green" : "red",
      borderColor: content?.success ? token.colorSuccessBorder : token.colorErrorBorder,
      bg: content?.success ? token.colorSuccessBg : token.colorErrorBg,
    },
    compaction: {
      icon: <CompressOutlined style={{ color: token.colorInfo }} />,
      color: "purple",
      borderColor: token.colorInfoBorder,
      bg: token.colorInfoBg,
    },
    session_end: {
      icon: <StopOutlined style={{ color: token.colorTextSecondary }} />,
      color: "default",
      borderColor: token.colorBorderSecondary,
      bg: token.colorFillQuaternary,
    },
  };

  const cfg = configs[event.event_type] || {
    icon: <CodeOutlined style={{ color: token.colorTextSecondary }} />,
    color: "default",
    borderColor: token.colorBorderSecondary,
    bg: token.colorFillQuaternary,
  };

  // Extract displayable content based on event type
  let bodyContent: React.ReactNode = null;
  if (event.event_type === "user_message" || event.event_type === "assistant_message") {
    const msg = (content?.message as string) || "";
    const imageUrls = extractPersistedImageUrls(content);
    if (msg) {
      bodyContent = (
        <>
          <ClampedContent title={`${event.event_type} content`}>
            {msg}
          </ClampedContent>
          <MessageImages urls={imageUrls} />
        </>
      );
    } else if (imageUrls.length > 0) {
      bodyContent = <MessageImages urls={imageUrls} />;
    }
  } else if (event.event_type === "tool_call") {
    const toolName = (content?.tool_name as string) || "unknown";
    const args = (content?.arguments as string) || "";
    bodyContent = (
      <>
        <Tag style={{ fontSize: 11 }}>{toolName}</Tag>
        {args && (
          <ClampedContent title={`Tool Call: ${toolName}`}>
            {args}
          </ClampedContent>
        )}
      </>
    );
  } else if (event.event_type === "tool_result") {
    const toolName = (content?.tool_name as string) || "";
    const result = (content?.result as string) || "";
    bodyContent = (
      <>
        {toolName && <Tag style={{ fontSize: 11 }}>{toolName}</Tag>}
        {result && (
          <ClampedContent title={`Tool Result: ${toolName}`}>
            {result}
          </ClampedContent>
        )}
      </>
    );
  } else if (event.event_type === "compaction") {
    const preTokens = content?.pre_tokens as number | undefined;
    const postTokens = content?.post_tokens as number | undefined;
    if (preTokens != null && postTokens != null) {
      bodyContent = (
        <Text style={{ fontSize: 12 }}>
          Tokens: {preTokens.toLocaleString()} → {postTokens.toLocaleString()}
          {" "}(saved {(preTokens - postTokens).toLocaleString()})
        </Text>
      );
    }
  } else if (event.event_type === "session_start") {
    const model = content?.model as string | undefined;
    const provider = content?.provider as string | undefined;
    bodyContent = (
      <Space size={4}>
        {model && <Tag>{model}</Tag>}
        {provider && <Tag color="cyan">{provider}</Tag>}
      </Space>
    );
  } else if (event.event_type === "session_end") {
    const totalTokens = content?.total_tokens as number | undefined;
    if (totalTokens != null) {
      bodyContent = <Text style={{ fontSize: 12 }}>Total tokens: {totalTokens.toLocaleString()}</Text>;
    }
  }

  return (
    <Card size="small" style={{ borderColor: cfg.borderColor, background: cfg.bg }}>
      <Space direction="vertical" size={2} style={{ width: "100%" }}>
        <Space size="small">
          {cfg.icon}
          <Tag color={cfg.color} style={{ fontSize: 11 }}>{event.event_type}</Tag>
          <Text type="secondary" style={{ fontSize: 11 }}>{ts}</Text>
        </Space>
        {bodyContent}
      </Space>
    </Card>
  );
}
