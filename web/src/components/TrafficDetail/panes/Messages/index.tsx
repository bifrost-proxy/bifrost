import { useState, useEffect, useMemo, useCallback, useRef, type CSSProperties } from "react";
import {
  Typography,
  Tag,
  theme,
  Button,
  Space,
  Tooltip,
  Empty,
  Modal,
} from "antd";
import {
  ArrowUpOutlined,
  ArrowDownOutlined,
  ReloadOutlined,
  CopyOutlined,
  ExpandOutlined,
  FullscreenOutlined,
} from "@ant-design/icons";
import dayjs from "dayjs";
import hljs from "highlight.js/lib/core";
import json from "highlight.js/lib/languages/json";
import plaintext from "highlight.js/lib/languages/plaintext";
import { useVirtualizer } from "@tanstack/react-virtual";
import "../../../../styles/hljs-github-theme.css";
import type {
  WebSocketFrame,
  FrameDirection,
  FrameType,
  SSEEvent,
  SessionTargetSearchState,
} from "../../../../types";
import { apiFetch } from "../../../../api/apiFetch";
import { getResponseBody } from "../../../../api/traffic";
import { getClientId } from "../../../../services/clientId";
import { buildApiUrl } from "../../../../runtime";
import { SseMessageList } from "./SseMessageList";
import {
  FullscreenMessageViewer,
  normalizeWSFrame,
  type MessageItem,
} from "../../../VirtualMessageViewer";
import { useTrafficStore } from "../../../../stores/useTrafficStore";
import { copyToClipboard } from '../../../../utils/clipboard';

hljs.registerLanguage("json", json);
hljs.registerLanguage("plaintext", plaintext);

import { useI18n } from "../../../../i18n";

const { Text } = Typography;

const MAX_SSE_EVENTS = 20_000;
const SSE_PARSE_CHAR_BUDGET = 128 * 1024;
const SSE_PARSE_EVENT_BATCH_SIZE = 200;

interface MessagesProps {
  recordId: string;
  isWebSocket: boolean;
  frameCount: number;
  isConnectionOpen?: boolean;
  searchValue: SessionTargetSearchState;
  onSearch: (v: Partial<SessionTargetSearchState>) => void;
  onSseCountChange?: (count: number) => void;
  onSseEventsChange?: (events: SSEEvent[]) => void;
  responseBodyOverride?: string | null;
  onResponseBodyChange?: (body: string | null, recordId: string) => void;
}

const formatSize = (bytes: number) => {
  if (bytes === 0) return "-";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(2)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
};

const getFrameTypeColor = (type: FrameType): string => {
  switch (type) {
    case "text":
      return "blue";
    case "binary":
      return "purple";
    case "ping":
      return "cyan";
    case "pong":
      return "geekblue";
    case "close":
      return "red";
    case "sse":
      return "green";
    default:
      return "default";
  }
};

const DirectionIcon = ({ direction }: { direction: FrameDirection }) => {
  return direction === "send" ? (
    <ArrowUpOutlined style={{ color: "#52c41a" }} />
  ) : (
    <ArrowDownOutlined style={{ color: "#1890ff" }} />
  );
};

interface WsMessageListProps {
  frames: WebSocketFrame[];
  loading: boolean;
  onOpenDetail: (frame: WebSocketFrame) => void;
  onCopy: (frame: WebSocketFrame) => void;
}

const WsMessageList = ({
  frames,
  loading,
  onOpenDetail,
  onCopy,
}: WsMessageListProps) => {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const parentRef = useRef<HTMLDivElement>(null);
  const [isAtTop, setIsAtTop] = useState(true);
  const [isAtBottom, setIsAtBottom] = useState(true);
  const scrollButtonStyles = useMemo(
    () => ({
      scrollButton: {
        position: "absolute" as const,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        width: 36,
        height: 36,
        backgroundColor: token.colorBgElevated,
        color: token.colorTextSecondary,
        borderRadius: "50%",
        cursor: "pointer",
        boxShadow: "0 2px 8px rgba(0, 0, 0, 0.08)",
        zIndex: 10,
        border: `1px solid ${token.colorBorderSecondary}`,
        transition:
          "opacity 0.3s ease, transform 0.3s ease, background-color 0.2s",
      },
      scrollToTopButton: {
        top: 16,
        left: "50%",
        transform: "translateX(-50%)",
      },
      scrollToBottomButton: {
        bottom: 16,
        left: "50%",
        transform: "translateX(-50%)",
      },
    }),
    [token],
  );
  const rowVirtualizer = useVirtualizer({
    count: frames.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 36,
    overscan: 6,
    getItemKey: (index) => String(frames[index]?.frame_id ?? index),
  });

  const headerStyle: CSSProperties = {
    display: "grid",
    gridTemplateColumns: "60px 40px 80px 80px 110px 1fr 70px",
    gap: 8,
    padding: "6px 8px",
    backgroundColor: token.colorFillQuaternary,
    borderBottom: `1px solid ${token.colorBorderSecondary}`,
    color: token.colorTextSecondary,
    fontSize: 12,
    fontWeight: 500,
  };

  const rowBaseStyle: CSSProperties = {
    display: "grid",
    gridTemplateColumns: "60px 40px 80px 80px 110px 1fr 70px",
    gap: 8,
    alignItems: "center",
    padding: "4px 8px",
    backgroundColor: token.colorBgContainer,
    borderBottom: `1px solid ${token.colorBorderSecondary}`,
  };

  const handleScrollToTop = useCallback(() => {
    if (frames.length === 0) return;
    rowVirtualizer.scrollToIndex(0, { align: "start" });
  }, [frames.length, rowVirtualizer]);

  const handleScrollToBottom = useCallback(() => {
    if (frames.length === 0) return;
    rowVirtualizer.scrollToIndex(frames.length - 1, { align: "end" });
  }, [frames.length, rowVirtualizer]);

  useEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    const onScroll = () => {
      const threshold = 8;
      const distanceToBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
      setIsAtTop(el.scrollTop <= threshold);
      setIsAtBottom(distanceToBottom <= threshold);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    onScroll();
    return () => el.removeEventListener("scroll", onScroll);
  }, [frames.length]);

  if (frames.length === 0) {
    return (
      <div
        style={{
          flex: 1,
          minHeight: 0,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          color: token.colorTextSecondary,
        }}
      >
        {loading ? t("common.loading") : t("trafficDetail.messages.noFrames")}
      </div>
    );
  }

  return (
    <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
      <div style={headerStyle}>
        <span>#</span>
        <span />
        <span>{t("common.type")}</span>
        <span>{t("trafficDetail.messages.colSize")}</span>
        <span>{t("trafficDetail.messages.colTime")}</span>
        <span>{t("trafficDetail.messages.colPreview")}</span>
        <span />
      </div>
      <div style={{ flex: 1, minHeight: 0, position: "relative" }}>
        <div
          ref={parentRef}
          style={{ height: "100%", overflow: "auto" }}
          data-testid="ws-frames-table"
        >
        <div
          style={{
            height: rowVirtualizer.getTotalSize(),
            position: "relative",
          }}
        >
          {rowVirtualizer.getVirtualItems().map((virtualRow) => {
            const frame = frames[virtualRow.index];
            if (!frame) return null;
            const canOpen = frame.payload_size > 0;
            const rowStyle: CSSProperties = {
              ...rowBaseStyle,
              position: "absolute",
              top: 0,
              left: 0,
              width: "100%",
              transform: `translateY(${virtualRow.start}px)`,
              borderLeft:
                frame.direction === "send" ? "3px solid #52c41a" : "3px solid #1890ff",
              cursor: canOpen ? "pointer" : "default",
            };

            return (
              <div
                key={virtualRow.key}
                style={rowStyle}
                data-testid="ws-frame-row"
                data-frame-id={frame.frame_id}
                data-payload-size={frame.payload_size}
                onClick={() => {
                  if (!canOpen) return;
                  onOpenDetail(frame);
                }}
              >
                <Text type="secondary">{frame.frame_id}</Text>
                <DirectionIcon direction={frame.direction} />
                <Tag color={getFrameTypeColor(frame.frame_type)}>
                  {frame.frame_type.toUpperCase()}
                </Tag>
                <Text>{formatSize(frame.payload_size)}</Text>
                <Text>{dayjs(frame.timestamp).format("HH:mm:ss.SSS")}</Text>
                <div style={{ overflow: "hidden" }}>
                  {frame.payload_preview ? (
                    <Text
                      style={{ fontFamily: "monospace", fontSize: 12 }}
                      ellipsis={{ tooltip: frame.payload_preview }}
                    >
                      {frame.payload_preview}
                    </Text>
                  ) : (
                    <Text type="secondary">-</Text>
                  )}
                </div>
                <Space size={4}>
                  <Tooltip title={t("common.copy")}>
                    <Button
                      type="text"
                      size="small"
                      icon={<CopyOutlined />}
                      onClick={(e) => {
                        e.stopPropagation();
                        void onCopy(frame);
                      }}
                      disabled={!canOpen}
                    />
                  </Tooltip>
                  <Tooltip title={t("trafficDetail.messages.expand")}>
                    <Button
                      type="text"
                      size="small"
                      icon={<ExpandOutlined />}
                      onClick={(e) => {
                        e.stopPropagation();
                        if (!canOpen) return;
                        onOpenDetail(frame);
                      }}
                      disabled={!canOpen}
                    />
                  </Tooltip>
                </Space>
              </div>
            );
          })}
        </div>
      </div>
        {!isAtTop && (
          <div
            style={{
              ...scrollButtonStyles.scrollButton,
              ...scrollButtonStyles.scrollToTopButton,
            }}
            onClick={handleScrollToTop}
            data-testid="ws-scroll-top"
          >
            <ArrowUpOutlined style={{ fontSize: 14 }} />
          </div>
        )}
        {!isAtBottom && (
          <div
            style={{
              ...scrollButtonStyles.scrollButton,
              ...scrollButtonStyles.scrollToBottomButton,
            }}
            onClick={handleScrollToBottom}
            data-testid="ws-scroll-bottom"
          >
            <ArrowDownOutlined style={{ fontSize: 14 }} />
          </div>
        )}
      </div>
    </div>
  );
};

const formatJson = (text: string): { formatted: string; isJson: boolean } => {
  try {
    const parsed = JSON.parse(text);
    return { formatted: JSON.stringify(parsed, null, 2), isJson: true };
  } catch {
    return { formatted: text, isJson: false };
  }
};

const highlightContent = (text: string): string => {
  const { formatted, isJson } = formatJson(text);
  try {
    const result = hljs.highlight(formatted, {
      language: isJson ? "json" : "plaintext",
    });
    return result.value;
  } catch {
    return formatted;
  }
};

const parseSseChunkToEvent = (
  chunk: string,
  index: number,
  timestamp: number,
): SSEEvent | null => {
  const dataLines: string[] = [];
  let eventId: string | undefined;
  let eventType: string | undefined;
  let lineStart = 0;
  const readLine = (raw: string) => raw.endsWith("\r") ? raw.slice(0, -1) : raw;

  while (lineStart <= chunk.length) {
    const lineEnd = chunk.indexOf("\n", lineStart);
    const isLastLine = lineEnd === -1;
    const rawLine = isLastLine
      ? chunk.slice(lineStart)
      : chunk.slice(lineStart, lineEnd);
    const line = readLine(rawLine);

    if (line) {
      if (line.startsWith("data:")) {
        dataLines.push(line.slice(5).replace(/^ /, ""));
      } else if (line.startsWith("event:")) {
        const v = line.slice(6).replace(/^ /, "");
        if (v) eventType = v;
      } else if (line.startsWith("id:")) {
        const v = line.slice(3).replace(/^ /, "");
        if (v) eventId = v;
      }
    }

    if (isLastLine) {
      break;
    }
    lineStart = lineEnd + 1;
  }

  const data = dataLines.length > 0 ? dataLines.join("\n") : chunk;
  if (!data && !eventId && !eventType) return null;
  const normalizedEventType =
    !eventType && data.trim() === "[DONE]" ? "finish" : eventType;
  return {
    id: eventId ?? String(index + 1),
    event: normalizedEventType ?? "message",
    data,
    timestamp,
  };
};

const findNextSseEventBoundary = (text: string, start: number): number => {
  for (let i = start; i < text.length - 1; i += 1) {
    const current = text.charCodeAt(i);
    const next = text.charCodeAt(i + 1);

    if (current === 10 && next === 10) {
      return i;
    }
    if (current === 13 && next === 10) {
      const after = i + 2;
      if (after < text.length && text.charCodeAt(after) === 13) {
        if (after + 1 < text.length && text.charCodeAt(after + 1) === 10) {
          return i;
        }
      } else if (after < text.length && text.charCodeAt(after) === 10) {
        return i;
      }
    }
  }
  return -1;
};

const getBoundaryAdvance = (text: string, boundaryIndex: number): number => {
  if (
    text.charCodeAt(boundaryIndex) === 13 &&
    boundaryIndex + 3 < text.length &&
    text.charCodeAt(boundaryIndex + 1) === 10 &&
    text.charCodeAt(boundaryIndex + 2) === 13 &&
    text.charCodeAt(boundaryIndex + 3) === 10
  ) {
    return 4;
  }

  if (
    text.charCodeAt(boundaryIndex) === 13 &&
    boundaryIndex + 2 < text.length &&
    text.charCodeAt(boundaryIndex + 1) === 10 &&
    text.charCodeAt(boundaryIndex + 2) === 10
  ) {
    return 3;
  }

  return 2;
};

export const Messages = ({
  recordId,
  isWebSocket,
  frameCount,
  isConnectionOpen = false,
  searchValue,
  onSearch,
  onSseCountChange,
  onSseEventsChange,
  responseBodyOverride,
  onResponseBodyChange,
}: MessagesProps) => {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const [frames, setFrames] = useState<WebSocketFrame[]>([]);
  const [loading, setLoading] = useState(false);
  const [lastFetchedFrameId, setLastFetchedFrameId] = useState(0);
  const [lastSeenFrameId, setLastSeenFrameId] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const tableRef = useRef<HTMLDivElement>(null);
  const [selectedFrame, setSelectedFrame] = useState<WebSocketFrame | null>(
    null,
  );
  const [selectedSseEvent, setSelectedSseEvent] = useState<SSEEvent | null>(null);
  const [detailModalOpen, setDetailModalOpen] = useState(false);
  const [sseDetailModalOpen, setSseDetailModalOpen] = useState(false);
  const [wsDetailLoading, setWsDetailLoading] = useState(false);
  const eventSourceRef = useRef<EventSource | null>(null);
  const sseEventSourceRef = useRef<EventSource | null>(null);
  const [sseEvents, setSseEvents] = useState<SSEEvent[]>([]);
  const lastSseSeqRef = useRef<number>(0);
  const [sseReloadToken, setSseReloadToken] = useState(0);
  const [sseConnectionState, setSseConnectionState] = useState<
    "idle" | "connecting" | "open" | "closed" | "error"
  >("idle");
  const [sseLoading, setSseLoading] = useState(false);
  const sseParseTokenRef = useRef(0);
  const ssePendingRef = useRef<SSEEvent[]>([]);
  const sseFlushRef = useRef<number | null>(null);
  const sseClosedByUsRef = useRef(false);
  const sseSessionKeyRef = useRef<string | null>(null);
  const [sseForceClosed, setSseForceClosed] = useState(false);
  const [wsPayloadById, setWsPayloadById] = useState<Record<number, string>>(
    {},
  );
  const inflightWsPayloadIdsRef = useRef<Set<number>>(new Set());
  const responseBodyFromStore = useTrafficStore((state) => state.responseBody);
  const setResponseBody = useTrafficStore((state) => state.setResponseBody);
  const responseBody =
    responseBodyOverride !== undefined ? responseBodyOverride : responseBodyFromStore;

  const commitResponseBody = useCallback(
    (body: string | null) => {
      setResponseBody(recordId, body);
      onResponseBodyChange?.(body, recordId);
    },
    [onResponseBodyChange, recordId, setResponseBody],
  );

  const mergeWsFrames = useCallback((base: WebSocketFrame[], incoming: WebSocketFrame[]) => {
    if (incoming.length === 0) return base;
    const byId = new Map<number, WebSocketFrame>();
    for (const f of base) {
      byId.set(f.frame_id, f);
    }
    for (const f of incoming) {
      const existing = byId.get(f.frame_id);
      if (!existing) {
        byId.set(f.frame_id, f);
        continue;
      }
      const shouldReplace = !existing.payload_preview && !!f.payload_preview;
      if (shouldReplace) {
        byId.set(f.frame_id, { ...existing, ...f });
      }
    }
    return Array.from(byId.values()).sort((a, b) => a.frame_id - b.frame_id);
  }, []);

  const fetchFrames = useCallback(
    async (after?: number) => {
      setLoading(true);
      try {
        const params = new URLSearchParams();
        if (after !== undefined) {
          params.set("after", String(after));
        }
        params.set("limit", "100");

        const response = await apiFetch(
          `/_bifrost/api/traffic/${recordId}/frames?${params.toString()}`,
        );
        if (!response.ok) {
          throw new Error("Failed to fetch frames");
        }
        const data = await response.json();
        const nextFrames = (data.frames || []) as WebSocketFrame[];

        if (after !== undefined) {
          setFrames((prev) => mergeWsFrames(prev, nextFrames));
        } else {
          setFrames(mergeWsFrames([], nextFrames));
        }
        if (nextFrames.length > 0) {
          const lastId = nextFrames[nextFrames.length - 1].frame_id;
          setLastFetchedFrameId(lastId);
          setLastSeenFrameId((prev) => Math.max(prev, lastId));
        }
        setHasMore(data.has_more);
      } catch (error) {
        console.error("Failed to fetch frames:", error);
      } finally {
        setLoading(false);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [recordId],
  );

  const fetchFramePayload = useCallback(
    async (frameId: number) => {
      const response = await apiFetch(
        `/_bifrost/api/traffic/${recordId}/frames/${frameId}`,
      );
      if (!response.ok) {
        return "";
      }
      const data = await response.json();
      return data.full_payload || "";
    },
    [recordId],
  );

  useEffect(() => {
    if (isWebSocket && frameCount > 0) {
      fetchFrames();
    }
  }, [fetchFrames, frameCount, isWebSocket]);

  useEffect(() => {
    sseClosedByUsRef.current = true;
    sseEventSourceRef.current?.close();
    sseEventSourceRef.current = null;
    sseSessionKeyRef.current = null;
    setFrames([]);
    setLastFetchedFrameId(0);
    setLastSeenFrameId(0);
    setHasMore(false);
    setWsPayloadById({});
    inflightWsPayloadIdsRef.current.clear();
    setSseEvents([]);
    lastSseSeqRef.current = 0;
    sseParseTokenRef.current += 1;
    if (sseFlushRef.current !== null) {
      cancelAnimationFrame(sseFlushRef.current);
      sseFlushRef.current = null;
    }
    ssePendingRef.current = [];
    setSseConnectionState("idle");
    setSseLoading(false);
    setSseForceClosed(false);
  }, [recordId]);

  useEffect(() => {
    return () => {
      sseClosedByUsRef.current = true;
      sseEventSourceRef.current?.close();
      sseEventSourceRef.current = null;
      sseSessionKeyRef.current = null;
      if (sseFlushRef.current !== null) {
        cancelAnimationFrame(sseFlushRef.current);
        sseFlushRef.current = null;
      }
      sseParseTokenRef.current += 1;
    };
  }, []);

  /* eslint-disable react-hooks/exhaustive-deps */
  useEffect(() => {
    if (!isWebSocket || !isConnectionOpen) {
      return;
    }

    const eventSource = new EventSource(
      `${buildApiUrl(`/traffic/${recordId}/frames/stream`)}?x_client_id=${encodeURIComponent(getClientId())}`,
    );
    eventSourceRef.current = eventSource;

    eventSource.onmessage = (event) => {
      try {
        const frame = JSON.parse(event.data) as WebSocketFrame;
        setFrames((prev) => mergeWsFrames(prev, [frame]));
        setLastSeenFrameId((prev) => Math.max(prev, frame.frame_id));
      } catch (error) {
        console.error("Failed to parse frame event:", error);
      }
    };

    eventSource.onerror = () => {
      eventSource.close();
      eventSourceRef.current = null;
    };

    return () => {
      eventSource.close();
      eventSourceRef.current = null;
      apiFetch(`/_bifrost/api/traffic/${recordId}/frames/unsubscribe`, {
        method: "DELETE",
      }).catch(() => {});
    };
  }, [isConnectionOpen, isWebSocket, recordId]);
  /* eslint-enable react-hooks/exhaustive-deps */

  useEffect(() => {
    const sessionKey = `${recordId}:${sseReloadToken}`;

    if (isWebSocket || sseForceClosed) {
      if (sseEventSourceRef.current) {
        sseClosedByUsRef.current = true;
        sseEventSourceRef.current.close();
        sseEventSourceRef.current = null;
        sseSessionKeyRef.current = null;
        setSseConnectionState("closed");
        setSseLoading(false);
      }
      return;
    }

    if (
      sseEventSourceRef.current &&
      sseSessionKeyRef.current === sessionKey
    ) {
      // Once the live SSE detail subscription is established, don't close the EventSource
      // just because the summary received 'closed' first; otherwise tail events would be
      // truncated by the frontend at the close boundary.
      return;
    }

    if (sseEventSourceRef.current) {
      sseClosedByUsRef.current = true;
      sseEventSourceRef.current.close();
      sseEventSourceRef.current = null;
      sseSessionKeyRef.current = null;
    }

    if (!isConnectionOpen) {
      return;
    }

    const eventSource = new EventSource(
      `${buildApiUrl(`/traffic/${recordId}/sse/stream`)}?from=begin&batch=1&x_client_id=${encodeURIComponent(getClientId())}`,
    );
    sseEventSourceRef.current = eventSource;
    sseSessionKeyRef.current = sessionKey;
    sseClosedByUsRef.current = false;
    setSseConnectionState("connecting");
    setSseLoading(true);
    lastSseSeqRef.current = 0;
    setSseEvents([]);

    const flushPending = () => {
      const pending = ssePendingRef.current;
      ssePendingRef.current = [];
      sseFlushRef.current = null;
      if (pending.length > 0) {
        setSseEvents((prev) => {
          const next = prev.concat(pending);
          if (next.length <= MAX_SSE_EVENTS) return next;
          return next.slice(next.length - MAX_SSE_EVENTS);
        });
      }
    };

    const flushPendingNow = () => {
      if (sseFlushRef.current !== null) {
        cancelAnimationFrame(sseFlushRef.current);
        sseFlushRef.current = null;
      }
      flushPending();
    };

    const enqueueEvent = (ev: SSEEvent) => {
      ssePendingRef.current.push(ev);
      if (sseFlushRef.current === null) {
        sseFlushRef.current = requestAnimationFrame(flushPending);
      }
    };

    const enqueueEvents = (events: SSEEvent[]) => {
      if (events.length === 0) return;
      const pending = ssePendingRef.current;
      for (let i = 0; i < events.length; i++) {
        pending.push(events[i]);
      }
      if (sseFlushRef.current === null) {
        sseFlushRef.current = requestAnimationFrame(flushPending);
      }
    };

    eventSource.onopen = () => {
      setSseConnectionState("open");
      setSseLoading(false);
    };

    eventSource.onmessage = (event) => {
      try {
        const payload = JSON.parse(event.data) as unknown;
        if (!payload || typeof payload !== "object") return;
        const payloadObj = payload as Record<string, unknown>;

        if (payloadObj.batch && Array.isArray(payloadObj.events)) {
          const batch = payloadObj.events as Array<Record<string, unknown>>;
          const parsed: SSEEvent[] = [];
          let maxSeq = lastSseSeqRef.current;
          for (let i = 0; i < batch.length; i++) {
            const e = batch[i];
            const seq = typeof e.seq === "number" ? e.seq : 0;
            if (seq > 0 && seq <= lastSseSeqRef.current) {
              continue;
            }
            if (seq > 0) {
              maxSeq = Math.max(maxSeq, seq);
            }
            const ts = typeof e.ts === "number" ? e.ts : Date.now();
            parsed.push({
              id: typeof e.id === "string" ? e.id : String(seq || ts),
              event: typeof e.event === "string" ? e.event : "message",
              data: typeof e.data === "string" ? e.data : "",
              timestamp: ts,
            });
          }
          if (maxSeq > lastSseSeqRef.current) {
            lastSseSeqRef.current = maxSeq;
          }
          enqueueEvents(parsed);
          return;
        }

        const seq = typeof payloadObj.seq === "number" ? payloadObj.seq : 0;
        if (seq > 0 && seq <= lastSseSeqRef.current) {
          return;
        }
        if (seq > 0) {
          lastSseSeqRef.current = seq;
        }
        const ts = typeof payloadObj.ts === "number" ? payloadObj.ts : Date.now();
        enqueueEvent({
          id: typeof payloadObj.id === "string" ? payloadObj.id : String(seq || ts),
          event: typeof payloadObj.event === "string" ? payloadObj.event : "message",
          data: typeof payloadObj.data === "string" ? payloadObj.data : "",
          timestamp: ts,
        });
      } catch (e) {
        console.error("Failed to parse SSE event:", e);
      }
    };

    eventSource.onerror = () => {
      if (sseClosedByUsRef.current) return;
      flushPendingNow();
      eventSource.close();
      sseEventSourceRef.current = null;
      sseSessionKeyRef.current = null;
      setSseConnectionState("closed");
      setSseLoading(false);
      setSseForceClosed(true);
      getResponseBody(recordId)
        .then((body) => commitResponseBody(body))
        .catch(() => {});
    };
  }, [
    isConnectionOpen,
    isWebSocket,
    recordId,
    commitResponseBody,
    sseReloadToken,
    sseForceClosed,
  ]);

  useEffect(() => {
    if (isWebSocket || (isConnectionOpen && !sseForceClosed)) {
      return;
    }
    if (responseBody === null) {
      return;
    }
    const token = ++sseParseTokenRef.current;
    setSseConnectionState("closed");
    setSseLoading(true);
    setSseEvents([]);
    let index = 0;
    let eventIndex = 0;
    let rafId: number | null = null;

    const run = () => {
      if (sseParseTokenRef.current !== token) return;
      const batch: SSEEvent[] = [];
      let processedChars = 0;

      while (
        batch.length < SSE_PARSE_EVENT_BATCH_SIZE &&
        processedChars < SSE_PARSE_CHAR_BUDGET &&
        index < responseBody.length
      ) {
        const next = findNextSseEventBoundary(responseBody, index);
        if (next === -1) {
          const tailChunk = responseBody.slice(index).replace(/[\r\n]+$/, "");
          if (tailChunk.trim().length > 0) {
            const ev = parseSseChunkToEvent(tailChunk, eventIndex, Date.now());
            if (ev) {
              batch.push(ev);
              eventIndex += 1;
            }
          }
          index = responseBody.length;
          break;
        }

        const chunk = responseBody.slice(index, next).replace(/[\r\n]+$/, "");
        processedChars += next - index;
        index = next + getBoundaryAdvance(responseBody, next);
        if (chunk.trim().length > 0) {
          const ev = parseSseChunkToEvent(chunk, eventIndex, Date.now());
          if (ev) {
            batch.push(ev);
            eventIndex += 1;
          }
        }
      }

      if (batch.length > 0) {
        setSseEvents((prev) => {
          const next = prev.concat(batch);
          if (next.length <= MAX_SSE_EVENTS) return next;
          return next.slice(next.length - MAX_SSE_EVENTS);
        });
      }

      if (index < responseBody.length) {
        rafId = requestAnimationFrame(run);
      } else {
        setSseLoading(false);
      }
    };

    rafId = requestAnimationFrame(run);
    return () => {
      if (rafId !== null) {
        cancelAnimationFrame(rafId);
      }
      if (sseParseTokenRef.current === token) {
        sseParseTokenRef.current += 1;
      }
    };
  }, [isConnectionOpen, isWebSocket, responseBody, sseForceClosed]);

  useEffect(() => {
    onSseCountChange?.(sseEvents.length);
  }, [onSseCountChange, sseEvents.length]);

  useEffect(() => {
    onSseEventsChange?.(sseEvents);
  }, [onSseEventsChange, sseEvents]);

  const [sseSearchQuery, setSseSearchQuery] = useState("");
  const [sseSearchMode, setSseSearchMode] = useState<"highlight" | "filter">(
    "highlight",
  );
  const [wsFullscreenOpen, setWsFullscreenOpen] = useState(false);
  const [sseFullscreenOpen, setSseFullscreenOpen] = useState(false);

  const framesForWsDisplay = useMemo<WebSocketFrame[]>(() => {
    if (!isWebSocket) {
      return frames;
    }
    return frames.map((f) => {
      if (f.payload_preview) return f;
      const payload = wsPayloadById[f.frame_id];
      if (!payload) return f;
      return { ...f, payload_preview: payload };
    });
  }, [frames, isWebSocket, wsPayloadById]);

  const filteredWsFrames = useMemo(() => {
    if (!isWebSocket) return [];
    if (!searchValue.value) return framesForWsDisplay;
    const searchLower = searchValue.value.toLowerCase();
    return framesForWsDisplay.filter(
      (frame) =>
        frame.payload_preview?.toLowerCase().includes(searchLower) ||
        frame.frame_type.toLowerCase().includes(searchLower),
    );
  }, [framesForWsDisplay, isWebSocket, searchValue.value]);

  const handleCopyWsFrame = useCallback(
    async (record: WebSocketFrame) => {
      const payload =
        wsPayloadById[record.frame_id] || record.payload_preview || "";
      if (payload) {
        await copyToClipboard(payload);
        return;
      }
      if (record.payload_size === 0) return;
      if (inflightWsPayloadIdsRef.current.has(record.frame_id)) return;
      inflightWsPayloadIdsRef.current.add(record.frame_id);
      const full = await fetchFramePayload(record.frame_id);
      inflightWsPayloadIdsRef.current.delete(record.frame_id);
      if (!full) return;
      setWsPayloadById((prev) =>
        prev[record.frame_id] ? prev : { ...prev, [record.frame_id]: full },
      );
      await copyToClipboard(full);
    },
    [fetchFramePayload, wsPayloadById],
  );

  const normalizedWsMessages = useMemo<MessageItem[]>(() => {
    return framesForWsDisplay.map(normalizeWSFrame);
  }, [framesForWsDisplay]);

  useEffect(() => {
    if (!isWebSocket) {
      setSseSearchQuery(searchValue.value ?? "");
    }
  }, [isWebSocket, searchValue.value]);

  const openWsFrameDetail = useCallback(
    async (frame: WebSocketFrame) => {
      setSelectedFrame(frame);
      setDetailModalOpen(true);
      if (frame.payload_size === 0) {
        return;
      }
      if (
        frame.payload_preview &&
        (frame.frame_type === "text" ||
          frame.frame_type === "close" ||
          frame.frame_type === "sse") &&
        frame.payload_preview.length >= frame.payload_size
      ) {
        return;
      }
      if (wsPayloadById[frame.frame_id]) {
        return;
      }
      if (inflightWsPayloadIdsRef.current.has(frame.frame_id)) {
        return;
      }
      inflightWsPayloadIdsRef.current.add(frame.frame_id);
      setWsDetailLoading(true);
      try {
        const payload = await fetchFramePayload(frame.frame_id);
        if (payload) {
          setWsPayloadById((prev) =>
            prev[frame.frame_id]
              ? prev
              : { ...prev, [frame.frame_id]: payload },
          );
        }
      } finally {
        inflightWsPayloadIdsRef.current.delete(frame.frame_id);
        setWsDetailLoading(false);
      }
    },
    [fetchFramePayload, wsPayloadById],
  );

  useEffect(() => {
    if (!isWebSocket) {
      return;
    }
    const previewLimitGuess = 256;
    const missingIds = frames
      .filter(
        (f) =>
          f.payload_size > 0 &&
          !wsPayloadById[f.frame_id] &&
          (!f.payload_preview || f.payload_size > previewLimitGuess),
      )
      .map((f) => f.frame_id)
      .filter((id) => !inflightWsPayloadIdsRef.current.has(id));
    if (missingIds.length === 0) {
      return;
    }
    let cancelled = false;
    const run = async () => {
      const concurrency = 6;
      let idx = 0;
      const worker = async () => {
        while (!cancelled) {
          const current = missingIds[idx++];
          if (current === undefined) return;
          inflightWsPayloadIdsRef.current.add(current);
          const payload = await fetchFramePayload(current);
          inflightWsPayloadIdsRef.current.delete(current);
          if (cancelled || !payload) continue;
          setWsPayloadById((prev) =>
            prev[current] ? prev : { ...prev, [current]: payload },
          );
        }
      };
      await Promise.all(
        Array.from({ length: Math.min(concurrency, missingIds.length) }, () =>
          worker(),
        ),
      );
    };
    void run();
    return () => {
      cancelled = true;
    };
  }, [fetchFramePayload, frames, isWebSocket, wsPayloadById]);

  const selectedPayload = useMemo(() => {
    if (!selectedFrame) return "";
    return (
      wsPayloadById[selectedFrame.frame_id] ||
      selectedFrame.payload_preview ||
      ""
    );
  }, [selectedFrame, wsPayloadById]);

  const selectedSsePayload = useMemo(() => {
    if (!selectedSseEvent) return "";
    return formatJson(selectedSseEvent.data || "").formatted;
  }, [selectedSseEvent]);

  const openSseEventDetail = useCallback((event: SSEEvent) => {
    setSelectedSseEvent(event);
    setSseDetailModalOpen(true);
  }, []);


  if (
    frameCount === 0 &&
    frames.length === 0 &&
    sseEvents.length === 0 &&
    !isConnectionOpen
  ) {
    return (
      <div
        style={{
          padding: 24,
          display: "flex",
          justifyContent: "center",
          alignItems: "center",
          height: 200,
        }}
      >
        <Empty
          description={t("trafficDetail.messages.noMessagesYet", {
            type: isWebSocket ? "WebSocket" : "SSE",
          })}
        />
      </div>
    );
  }

  if (!isWebSocket) {
    return (
      <div
        data-testid="sse-message-list"
        style={{ height: "100%", overflow: "hidden" }}
      >
        <SseMessageList
          events={sseEvents}
          loading={sseLoading}
          hasMore={false}
          searchQuery={searchValue.value ?? sseSearchQuery}
          searchMode={sseSearchMode}
          onSearchChange={(value) => {
            setSseSearchQuery(value);
            onSearch({ value, next: 1 });
          }}
          onSearchModeChange={setSseSearchMode}
          onLoadMore={() => {}}
          onRefresh={() => {
            setSseForceClosed(false);
            setSseReloadToken((n) => n + 1);
          }}
          onFullscreenOpen={() => setSseFullscreenOpen(true)}
          connectionState={sseConnectionState}
          externalNext={searchValue.next}
          onMatchCountChange={(total) => {
            if (searchValue.total !== total) {
              onSearch({ total });
            }
          }}
          onMatchNavigate={(next) => {
            if (searchValue.next !== next) {
              onSearch({ next });
            }
          }}
          onOpenDetail={openSseEventDetail}
        />
        <Modal
          open={sseFullscreenOpen}
          onCancel={() => setSseFullscreenOpen(false)}
          footer={null}
          width="90vw"
          styles={{
            body: {
              height: "80vh",
              overflow: "hidden",
            },
          }}
        >
          <SseMessageList
            events={sseEvents}
            loading={sseLoading}
            hasMore={false}
            searchQuery={searchValue.value ?? sseSearchQuery}
            searchMode={sseSearchMode}
            onSearchChange={(value) => {
              setSseSearchQuery(value);
              onSearch({ value, next: 1 });
            }}
            onSearchModeChange={setSseSearchMode}
            onLoadMore={() => {}}
            onRefresh={() => {
              setSseForceClosed(false);
              setSseReloadToken((n) => n + 1);
            }}
            connectionState={sseConnectionState}
            externalNext={searchValue.next}
            onMatchCountChange={(total) => {
              if (searchValue.total !== total) {
                onSearch({ total });
              }
            }}
            onMatchNavigate={(next) => {
              if (searchValue.next !== next) {
                onSearch({ next });
              }
            }}
            onOpenDetail={openSseEventDetail}
          />
        </Modal>
        <Modal
          title={
            <Space>
              <Tag color="green">{selectedSseEvent?.event || "message"}</Tag>
              {selectedSseEvent?.id && (
                <Text type="secondary">id: {selectedSseEvent.id}</Text>
              )}
              {selectedSseEvent?.timestamp ? (
                <Text type="secondary">
                  {dayjs(selectedSseEvent.timestamp).format(
                    "YYYY-MM-DD HH:mm:ss.SSS",
                  )}
                </Text>
              ) : null}
            </Space>
          }
          open={sseDetailModalOpen}
          zIndex={3000}
          onCancel={() => {
            setSseDetailModalOpen(false);
            setSelectedSseEvent(null);
          }}
          footer={
            <Space>
              <Button
                icon={<CopyOutlined />}
                onClick={() => {
                  if (selectedSsePayload) {
                    copyToClipboard(selectedSsePayload);
                  }
                }}
                disabled={!selectedSsePayload}
              >
                {t("common.copy")}
              </Button>
              <Button
                onClick={() => {
                  setSseDetailModalOpen(false);
                  setSelectedSseEvent(null);
                }}
              >
                {t("common.close")}
              </Button>
            </Space>
          }
          width={700}
          styles={{
            body: {
              maxHeight: "60vh",
              overflow: "auto",
            },
          }}
        >
          {!!selectedSsePayload && (
            <pre
              data-testid="sse-event-detail-content"
              style={{
                margin: 0,
                padding: 12,
                fontSize: 12,
                fontFamily: "monospace",
                backgroundColor: token.colorBgLayout,
                borderRadius: 4,
                whiteSpace: "pre-wrap",
                wordBreak: "break-all",
                lineHeight: 1.5,
              }}
            >
              <code
                className="hljs"
                dangerouslySetInnerHTML={{
                  __html: highlightContent(selectedSsePayload),
                }}
              />
            </pre>
          )}
        </Modal>
      </div>
    );
  }

  return (
    <div
      ref={tableRef}
      data-testid="ws-frames-pane"
      style={{
        height: "100%",
        display: "flex",
        flexDirection: "column",
        minHeight: 0,
      }}
    >
      <div
        style={{
          marginBottom: 4,
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
        }}
      >
        <Text type="secondary" data-testid="ws-frames-summary">
          {t("trafficDetail.messages.frameCount", { shown: filteredWsFrames.length, total: frames.length })}
          {hasMore && t("trafficDetail.messages.moreAvailable")}
        </Text>
        <Space>
          <Tooltip title={t("trafficDetail.messages.fullscreen")}>
            <Button
              size="small"
              icon={<FullscreenOutlined />}
              onClick={() => setWsFullscreenOpen(true)}
              disabled={frames.length === 0}
            />
          </Tooltip>
          {(hasMore || lastSeenFrameId > lastFetchedFrameId) && (
            <Button
              size="small"
              onClick={() => fetchFrames(lastFetchedFrameId)}
              loading={loading}
            >
              {t("trafficDetail.messages.loadMore")}
            </Button>
          )}
          <Tooltip title={t("common.refresh")}>
            <Button
              size="small"
              icon={<ReloadOutlined />}
              onClick={() => fetchFrames()}
              loading={loading}
            />
          </Tooltip>
        </Space>
      </div>

      <WsMessageList
        frames={filteredWsFrames}
        loading={loading}
        onOpenDetail={openWsFrameDetail}
        onCopy={handleCopyWsFrame}
      />

      <Modal
        title={
          <Space>
            <DirectionIcon direction={selectedFrame?.direction ?? "receive"} />
            <Tag color={getFrameTypeColor(selectedFrame?.frame_type ?? "text")}>
              {selectedFrame?.frame_type?.toUpperCase()}
            </Tag>
            <Text type="secondary">
              #{selectedFrame?.frame_id} -{" "}
              {dayjs(selectedFrame?.timestamp).format(
                "YYYY-MM-DD HH:mm:ss.SSS",
              )}
            </Text>
          </Space>
        }
        open={detailModalOpen}
        onCancel={() => {
          setDetailModalOpen(false);
          setSelectedFrame(null);
          setWsDetailLoading(false);
        }}
        footer={
          <Space>
            <Button
              icon={<CopyOutlined />}
              onClick={() => {
                if (selectedPayload) {
                  const { formatted } = formatJson(selectedPayload);
                  copyToClipboard(formatted);
                }
              }}
              disabled={!selectedPayload || wsDetailLoading}
            >
              {t("common.copy")}
            </Button>
            <Button onClick={() => setDetailModalOpen(false)}>{t("common.close")}</Button>
          </Space>
        }
        width={700}
        styles={{
          body: {
            maxHeight: "60vh",
            overflow: "auto",
          },
        }}
      >
        {wsDetailLoading && <Text type="secondary">{t("common.loading")}</Text>}
        {!!selectedPayload && (
          <pre
            style={{
              margin: 0,
              padding: 12,
              fontSize: 12,
              fontFamily: "monospace",
              backgroundColor: token.colorBgLayout,
              borderRadius: 4,
              whiteSpace: "pre-wrap",
              wordBreak: "break-all",
              lineHeight: 1.5,
            }}
          >
            <code
              className="hljs"
              dangerouslySetInnerHTML={{
                __html: highlightContent(selectedPayload),
              }}
            />
          </pre>
        )}
      </Modal>

      <FullscreenMessageViewer
        open={wsFullscreenOpen}
        onClose={() => setWsFullscreenOpen(false)}
        items={normalizedWsMessages}
        title={
          <Space>
            <Tag color="purple">WebSocket</Tag>
            <Text type="secondary">{t("trafficDetail.messages.framesTotal", { count: frames.length })}</Text>
          </Space>
        }
        initialQuery={searchValue.value}
        initialMatchMode="highlight"
      />
    </div>
  );
};
