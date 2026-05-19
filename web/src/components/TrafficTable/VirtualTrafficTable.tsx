"use no memo";

import {
  useRef,
  useEffect,
  useCallback,
  useState,
  memo,
  useMemo,
  type CSSProperties,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Tag, Tooltip, Badge, theme } from "antd";
import {
  ThunderboltOutlined,
  ArrowDownOutlined,
  ArrowUpOutlined,
} from "@ant-design/icons";
import type { TrafficSummary } from "../../types";
import {
  formatDurationCompact,
  formatDurationDetailed,
  getEffectiveDurationMs,
  isLiveStreamingTraffic,
} from "../../utils/duration";
import { formatTimestamp } from "../../utils/datetime";
import { useLiveNow } from "../../hooks/useLiveNow";
import AppIcon from "../AppIcon";
import TrafficContextMenu from "./TrafficContextMenu";

const ellipsisStyle: CSSProperties = {
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  display: "block",
  maxWidth: "100%",
};

type SetSelectedIds = (ids: string[] | ((prev: string[]) => string[])) => void;

interface VirtualTrafficTableProps {
  data: TrafficSummary[];
  onSelect?: (record: TrafficSummary) => void;
  onDoubleClick?: (record: TrafficSummary) => void;
  selectedId?: string;
  selectedIds?: string[];
  onSelectedIdsChange?: SetSelectedIds;
  onLoadMore?: () => void;
  hasMore?: boolean;
  autoScroll?: boolean;
  onScrollPositionChange?: (isAtBottom: boolean) => void;
  newRecordsCount?: number;
  onScrollToBottom?: () => void;
  onKeyboardNavigate?: (record: TrafficSummary) => void;
  initialScrollTop?: number;
  onScrollTopChange?: (scrollTop: number) => void;
}

const ROW_HEIGHT = 36;
const SCROLL_THRESHOLD = 50;
const TABLE_MIN_WIDTH = 1510;

const DEFAULT_STATUS_DOT_COLOR = "#d9d9d9";

const formatSequence = (seq: number): string => {
  const raw = seq.toString();
  const trimmed = raw.length > 5 ? raw.slice(-5) : raw;
  return trimmed.padStart(5, "0");
};

interface ColumnDef {
  key: string;
  title: string;
  width: number | string;
  minWidth?: number;
  align?: "left" | "center" | "right";
  render: (
    record: TrafficSummary,
    textSecondary: string,
    rowIndex: number,
  ) => React.ReactNode;
}

const getColumnStyle = (col: ColumnDef): CSSProperties => {
  const width = typeof col.width === "number" ? col.width : undefined;
  const minWidth = col.minWidth ?? width;
  const isAutoWidth = col.width === "auto";
  return {
    width: width,
    minWidth: minWidth,
    flex: isAutoWidth ? 1 : `0 0 ${width}px`,
    justifyContent:
      col.align === "center"
        ? "center"
        : col.align === "right"
          ? "flex-end"
          : "flex-start",
  };
};

const columnStyles: CSSProperties[] = [];

const columns: ColumnDef[] = [
  {
    key: "sequence",
    title: "#",
    width: 50,
    align: "right",
    render: (record, textSecondary, rowIndex) => {
      const displaySequence = record.sequence ?? rowIndex + 1;
      return (
        <span
          style={{ fontSize: 11, fontFamily: "monospace", color: textSecondary }}
        >
          {formatSequence(displaySequence)}
        </span>
      );
    },
  },
  {
    key: "status_dot",
    title: "",
    width: 24,
    align: "center",
    render: (record) => (
      <div
        title={record.status === 0 ? "Pending" : `Status: ${record.status}`}
        style={{
          width: 8,
          height: 8,
          borderRadius: "50%",
          backgroundColor: record._statusDotColor || DEFAULT_STATUS_DOT_COLOR,
        }}
      />
    ),
  },
  {
    key: "protocol",
    title: "Protocol",
    width: 70,
    render: (record) => (
      <Tag
        color={record.is_h3 ? "purple" : "default"}
        style={{ margin: 0, fontSize: 11 }}
      >
        {record._displayProtocol || record.protocol || "-"}
      </Tag>
    ),
  },
  {
    key: "method",
    title: "Method",
    width: 70,
    render: (record) => (
      <Tag
        color={record._methodColor || "default"}
        style={{ margin: 0, fontSize: 11 }}
      >
        {record.method}
      </Tag>
    ),
  },
  {
    key: "status",
    title: "Status",
    width: 55,
    align: "center",
    render: (record, textSecondary) =>
      record.status > 0 ? (
        <Tag
          color={record._statusColor || "default"}
          style={{ margin: 0, fontSize: 11 }}
        >
          {record.status}
        </Tag>
      ) : (
        <span style={{ color: textSecondary }}>-</span>
      ),
  },
  {
    key: "client",
    title: "Client",
    width: 140,
    render: (record, textSecondary) => (
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 4,
          overflow: "hidden",
        }}
        title={record._clientTooltip || record.client_ip || "-"}
      >
        {record.client_app && (
          <AppIcon
            appName={record.client_app}
            size={16}
            style={{ flexShrink: 0 }}
          />
        )}
        <span
          style={{
            ...ellipsisStyle,
            fontSize: 11,
            lineHeight: "16px",
            color: textSecondary,
          }}
        >
          {record._clientDisplay ||
            record.client_app ||
            record.client_ip ||
            "-"}
        </span>
      </div>
    ),
  },
  {
    key: "listener_port",
    title: "Port",
    width: 70,
    align: "center",
    render: (record, textSecondary) =>
      record.listener_port ? (
        <Tag style={{ margin: 0, fontSize: 11 }}>{record.listener_port}</Tag>
      ) : (
        <span style={{ color: textSecondary }}>-</span>
      ),
  },
  {
    key: "rules",
    title: "Rules",
    width: 60,
    align: "center",
    render: (record, textSecondary) =>
      record.has_rule_hit ? (
        <Tooltip
          title={
            <div>
              <div>{record.matched_rule_count} rule(s) matched</div>
              {record.matched_protocols.length > 0 && (
                <div style={{ marginTop: 4 }}>
                  {record.matched_protocols.join(", ")}
                </div>
              )}
            </div>
          }
        >
          <Badge count={record.matched_rule_count} size="small" color="blue">
            <ThunderboltOutlined style={{ fontSize: 14, color: "#1890ff" }} />
          </Badge>
        </Tooltip>
      ) : (
        <span style={{ color: textSecondary }}>-</span>
      ),
  },
  {
    key: "host",
    title: "Host",
    width: 160,
    render: (record, textSecondary) => (
      <span
        style={{ ...ellipsisStyle, fontSize: 12, color: textSecondary }}
        title={record.host}
      >
        {record.host}
      </span>
    ),
  },
  {
    key: "path",
    title: "Path",
    width: "auto",
    minWidth: 250,
    render: (record, textSecondary) => (
      <span
        style={{ ...ellipsisStyle, fontSize: 12, color: textSecondary }}
        title={record.path}
      >
        {record.path}
      </span>
    ),
  },
  {
    key: "content_type",
    title: "Type",
    width: 80,
    render: (record, textSecondary) => (
      <span style={{ fontSize: 11, color: textSecondary }}>
        {record._contentTypeShort || "-"}
      </span>
    ),
  },
  {
    key: "response_size",
    title: "Size",
    width: 65,
    align: "right",
    render: (record, textSecondary) => (
      <span style={{ fontSize: 11, color: textSecondary }}>
        {record._displaySize || "-"}
      </span>
    ),
  },
  {
    key: "duration_ms",
    title: "Time",
    width: 55,
    align: "right",
    render: (record, textSecondary) => (
      <Tooltip title={formatDurationDetailed(record.duration_ms)}>
        <span
          style={{
            fontSize: 11,
            color: record.duration_ms > 1000 ? "#faad14" : textSecondary,
          }}
        >
          {formatDurationCompact(record.duration_ms)}
        </span>
      </Tooltip>
    ),
  },
  {
    key: "start_time",
    title: "Start Time",
    width: 160,
    render: (record, textSecondary) => {
      const time = formatTimestamp(record.timestamp);
      return (
        <span
          style={{ fontSize: 11, fontFamily: "monospace", color: textSecondary }}
          title={time}
        >
          {time}
        </span>
      );
    },
  },
  {
    key: "end_time",
    title: "End Time",
    width: 160,
    render: (record, textSecondary) => {
      const time =
        record.duration_ms > 0
          ? formatTimestamp(record.timestamp + record.duration_ms)
          : "-";
      return (
        <span
          style={{ fontSize: 11, fontFamily: "monospace", color: textSecondary }}
          title={time}
        >
          {time}
        </span>
      );
    },
  },
];

for (const col of columns) {
  columnStyles.push(getColumnStyle(col));
}

const baseRowStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  height: ROW_HEIGHT,
  maxHeight: ROW_HEIGHT,
  minHeight: ROW_HEIGHT,
  minWidth: TABLE_MIN_WIDTH,
  boxSizing: "border-box",
  cursor: "pointer",
  position: "absolute",
  top: 0,
  left: 0,
  width: "100%",
  willChange: "transform",
  contain: "layout style",
};

const baseCellStyle: CSSProperties = {
  padding: "0 8px",
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  display: "flex",
  alignItems: "center",
  height: "100%",
  maxHeight: ROW_HEIGHT,
  lineHeight: `${ROW_HEIGHT - 2}px`,
  contain: "layout style paint",
};

interface TableRowProps {
  record: TrafficSummary;
  liveNow: number;
  isSelected: boolean;
  isMultiSelected: boolean;
  isImported: boolean;
  translateY: number;
  rowIndex: number;
  borderColor: string;
  selectedBg: string;
  multiSelectedBg: string;
  importedBg: string;
  evenBg: string;
  oddBg: string;
  textSecondary: string;
  onRowClick: (e: React.MouseEvent) => void;
  onRowDoubleClick: () => void;
  onRowContextMenu: (e: React.MouseEvent) => void;
}

export const areTrafficRowPropsEqual = (
  prev: TableRowProps,
  next: TableRowProps,
): boolean => {
  if (prev.isSelected !== next.isSelected) return false;
  if (prev.isMultiSelected !== next.isMultiSelected) return false;
  if (prev.isImported !== next.isImported) return false;
  if (prev.translateY !== next.translateY) return false;
  if (prev.rowIndex !== next.rowIndex) return false;
  if (prev.liveNow !== next.liveNow) return false;

  const prevRecord = prev.record;
  const nextRecord = next.record;
  if (prevRecord.id !== nextRecord.id) return false;
  if (prevRecord.sequence !== nextRecord.sequence) return false;
  if (prevRecord.method !== nextRecord.method) return false;
  if (prevRecord.protocol !== nextRecord.protocol) return false;
  if (prevRecord.status !== nextRecord.status) return false;
  if (prevRecord.client_ip !== nextRecord.client_ip) return false;
  if (prevRecord.client_app !== nextRecord.client_app) return false;
  if (prevRecord.host !== nextRecord.host) return false;
  if (prevRecord.path !== nextRecord.path) return false;
  if (prevRecord.request_size !== nextRecord.request_size) return false;
  if (prevRecord.duration_ms !== nextRecord.duration_ms) return false;
  if (prevRecord.listener_port !== nextRecord.listener_port) return false;
  if (prevRecord.response_size !== nextRecord.response_size) return false;
  if (prevRecord.frame_count !== nextRecord.frame_count) return false;
  if (prevRecord.content_type !== nextRecord.content_type) return false;
  if (prevRecord.has_rule_hit !== nextRecord.has_rule_hit) return false;
  if (prevRecord.matched_rule_count !== nextRecord.matched_rule_count) return false;
  if (prevRecord.matched_protocols.join(",") !== nextRecord.matched_protocols.join(",")) return false;
  if (prevRecord.end_time !== nextRecord.end_time) return false;
  if (
    prevRecord.socket_status?.send_bytes !==
    nextRecord.socket_status?.send_bytes
  )
    return false;
  if (
    prevRecord.socket_status?.receive_bytes !==
    nextRecord.socket_status?.receive_bytes
  )
    return false;

  return true;
};

const TableRow = memo(function TableRow({
  record,
  liveNow,
  isSelected,
  isMultiSelected,
  isImported,
  translateY,
  rowIndex,
  borderColor,
  selectedBg,
  multiSelectedBg,
  importedBg,
  evenBg,
  oddBg,
  textSecondary,
  onRowClick,
  onRowDoubleClick,
  onRowContextMenu,
}: TableRowProps) {
  const durationMs = getEffectiveDurationMs(record, liveNow);
  const rowStyle: CSSProperties = {
    ...baseRowStyle,
    borderBottom: `1px solid ${borderColor}`,
    transform: `translateY(${translateY}px)`,
    backgroundColor: isMultiSelected
      ? multiSelectedBg
      : isSelected
        ? selectedBg
        : isImported
          ? importedBg
          : rowIndex % 2 === 0
            ? evenBg
            : oddBg,
  };

  return (
    <div
      data-index={rowIndex}
      data-testid="traffic-row"
      data-record-id={record.id}
      data-request-size={record.request_size}
      data-response-size={record.response_size}
      data-frame-count={record.frame_count}
      style={rowStyle}
      onClick={onRowClick}
      onDoubleClick={onRowDoubleClick}
      onContextMenu={onRowContextMenu}
    >
      {columns.map((col, colIndex) => (
        <div
          key={col.key}
          style={{ ...baseCellStyle, ...columnStyles[colIndex] }}
        >
          {col.key === "duration_ms" ? (
            <Tooltip title={formatDurationDetailed(durationMs)}>
              <span
                style={{
                  fontSize: 11,
                  color: durationMs > 1000 ? "#faad14" : textSecondary,
                }}
              >
                {formatDurationCompact(durationMs)}
              </span>
            </Tooltip>
          ) : (
            col.render(record, textSecondary, rowIndex)
          )}
        </div>
      ))}
    </div>
  );
}, areTrafficRowPropsEqual);

const keyframesStyle = `
  @keyframes slideUp {
    from {
      opacity: 0;
      transform: translateX(-50%) translateY(20px);
    }
    to {
      opacity: 1;
      transform: translateX(-50%) translateY(0);
    }
  }
  @keyframes pulse {
    0%, 100% {
      transform: translateX(-50%) scale(1);
    }
    50% {
      transform: translateX(-50%) scale(1.05);
    }
  }
  @keyframes fadeSlideIn {
    from {
      opacity: 0;
      transform: translateY(10px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
  @keyframes fadeSlideInCenter {
    from {
      opacity: 0;
      transform: translateX(-50%) translateY(10px);
    }
    to {
      opacity: 1;
      transform: translateX(-50%) translateY(0);
    }
  }
  @keyframes fadeSlideDownCenter {
    from {
      opacity: 0;
      transform: translateX(-50%) translateY(-10px);
    }
    to {
      opacity: 1;
      transform: translateX(-50%) translateY(0);
    }
  }
`;

export default function VirtualTrafficTable({
  data,
  onSelect,
  onDoubleClick,
  selectedId,
  selectedIds = [],
  onSelectedIdsChange,
  onLoadMore,
  hasMore,
  autoScroll = true,
  onScrollPositionChange,
  newRecordsCount = 0,
  onScrollToBottom,
  onKeyboardNavigate,
  initialScrollTop = 0,
  onScrollTopChange,
}: VirtualTrafficTableProps) {
  const { token } = theme.useToken();
  const hasLiveDuration = useMemo(
    () => data.some((record) => isLiveStreamingTraffic(record)),
    [data],
  );
  const liveNow = useLiveNow(hasLiveDuration);
  const parentRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const prevDataLengthRef = useRef(data.length);
  const isAtBottomRef = useRef(true);
  const [showNewIndicator, setShowNewIndicator] = useState(false);
  const initialScrollRestoredRef = useRef(false);
  const [isAtTop, setIsAtTop] = useState(true);
  const [isAtBottom, setIsAtBottom] = useState(true);
  const lastSelectedIndexRef = useRef<number | null>(null);

  const [contextMenu, setContextMenu] = useState<{
    visible: boolean;
    record: TrafficSummary | null;
    position: { x: number; y: number };
  }>({
    visible: false,
    record: null,
    position: { x: 0, y: 0 },
  });

  const handleContextMenu = useCallback(
    (e: React.MouseEvent, record: TrafficSummary) => {
      e.preventDefault();
      e.stopPropagation();
      if (selectedIds.length > 0 && !selectedIds.includes(record.id)) {
        onSelectedIdsChange?.([]);
      }
      setContextMenu({
        visible: true,
        record,
        position: { x: e.clientX, y: e.clientY },
      });
    },
    [selectedIds, onSelectedIdsChange],
  );

  const handleCloseContextMenu = useCallback(() => {
    setContextMenu((prev) => ({ ...prev, visible: false }));
  }, []);

  const handleRowClick = useCallback(
    (e: React.MouseEvent, record: TrafficSummary, index: number) => {
      const isMeta = e.metaKey || e.ctrlKey;
      const isShift = e.shiftKey;

      if (isMeta) {
        onSelectedIdsChange?.((prev) => {
          let currentSelectedIds = [...prev];
          if (currentSelectedIds.length === 0 && selectedId) {
            currentSelectedIds = [selectedId];
          }
          return currentSelectedIds.includes(record.id)
            ? currentSelectedIds.filter((id) => id !== record.id)
            : [...currentSelectedIds, record.id];
        });
        lastSelectedIndexRef.current = index;
      } else if (isShift && lastSelectedIndexRef.current !== null) {
        const start = Math.min(lastSelectedIndexRef.current, index);
        const end = Math.max(lastSelectedIndexRef.current, index);
        const rangeIds = data.slice(start, end + 1).map((r) => r.id);
        onSelectedIdsChange?.((prev) => {
          let currentSelectedIds = [...prev];
          if (currentSelectedIds.length === 0 && selectedId) {
            currentSelectedIds = [selectedId];
          }
          return Array.from(new Set([...currentSelectedIds, ...rangeIds]));
        });
      } else {
        onSelectedIdsChange?.([]);
        lastSelectedIndexRef.current = index;
        onSelect?.(record);
      }
    },
    [data, selectedId, onSelectedIdsChange, onSelect],
  );

  const selectedRecords =
    selectedIds.length > 0
      ? data.filter((r) => selectedIds.includes(r.id))
      : contextMenu.record
        ? [contextMenu.record]
        : [];

  // eslint-disable-next-line react-hooks/incompatible-library
  const rowVirtualizer = useVirtualizer({
    count: data.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 5,
    getItemKey: (index) => data[index]?.id ?? index,
  });

  const checkIsAtBottom = useCallback(() => {
    if (!parentRef.current) return true;
    const { scrollTop, scrollHeight, clientHeight } = parentRef.current;
    return scrollHeight - scrollTop - clientHeight < SCROLL_THRESHOLD;
  }, []);

  const handleScroll = useCallback(() => {
    if (!parentRef.current) return;

    const { scrollTop, scrollHeight, clientHeight } = parentRef.current;
    onScrollTopChange?.(scrollTop);

    const atTopNow = scrollTop < SCROLL_THRESHOLD;
    const atBottomNow =
      scrollHeight - scrollTop - clientHeight < SCROLL_THRESHOLD;

    setIsAtTop(atTopNow);
    setIsAtBottom(atBottomNow);

    if (isAtBottomRef.current !== atBottomNow) {
      isAtBottomRef.current = atBottomNow;
      onScrollPositionChange?.(atBottomNow);

      if (atBottomNow) {
        setShowNewIndicator(false);
      }
    }

    if (onLoadMore && hasMore) {
      if (scrollHeight - scrollTop - clientHeight < 200) {
        onLoadMore();
      }
    }
  }, [onScrollPositionChange, onLoadMore, hasMore, onScrollTopChange]);

  useEffect(() => {
    if (
      !initialScrollRestoredRef.current &&
      data.length > 0 &&
      parentRef.current
    ) {
      initialScrollRestoredRef.current = true;
      if (initialScrollTop > 0) {
        parentRef.current.scrollTop = initialScrollTop;
        isAtBottomRef.current = checkIsAtBottom();
      }
      prevDataLengthRef.current = data.length;
      return;
    }

    const prevLength = prevDataLengthRef.current;
    const currLength = data.length;

    if (currLength > prevLength && prevLength > 0) {
      if (autoScroll && isAtBottomRef.current) {
        rowVirtualizer.scrollToIndex(currLength - 1, {
          align: "end",
          behavior: "auto",
        });
      } else if (!isAtBottomRef.current) {
        setShowNewIndicator(true);
      }
    }

    prevDataLengthRef.current = currLength;
  }, [
    data.length,
    autoScroll,
    rowVirtualizer,
    initialScrollTop,
    checkIsAtBottom,
  ]);

  useEffect(() => {
    if (data.length === 0) {
      prevDataLengthRef.current = 0;
      isAtBottomRef.current = true;
      setShowNewIndicator(false);
    }
  }, [data.length]);

  useEffect(() => {
    if (newRecordsCount > 0 && !isAtBottomRef.current) {
      setShowNewIndicator(true);
    } else if (newRecordsCount === 0 || isAtBottomRef.current) {
      setShowNewIndicator(false);
    }
  }, [newRecordsCount]);

  const handleScrollToBottomClick = useCallback(() => {
    rowVirtualizer.scrollToIndex(data.length - 1, {
      align: "end",
      behavior: "smooth",
    });
    setShowNewIndicator(false);
    onScrollToBottom?.();
  }, [rowVirtualizer, data.length, onScrollToBottom]);

  const handleScrollToTopClick = useCallback(() => {
    rowVirtualizer.scrollToIndex(0, {
      align: "start",
      behavior: "smooth",
    });
  }, [rowVirtualizer]);

  const scrollToRow = useCallback(
    (index: number, smooth: boolean = true) => {
      if (!parentRef.current) return;

      const scrollTop = parentRef.current.scrollTop;
      const clientHeight = parentRef.current.clientHeight;
      const headerHeight = ROW_HEIGHT;
      const rowTop = index * ROW_HEIGHT + headerHeight;
      const rowBottom = rowTop + ROW_HEIGHT;
      const visibleTop = scrollTop + headerHeight;
      const visibleBottom = scrollTop + clientHeight;

      const behavior = smooth ? "smooth" : "auto";
      if (rowTop < visibleTop) {
        rowVirtualizer.scrollToIndex(index, { align: "start", behavior });
      } else if (rowBottom > visibleBottom) {
        rowVirtualizer.scrollToIndex(index, { align: "end", behavior });
      }
    },
    [rowVirtualizer],
  );

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (data.length === 0) return;
      if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;

      e.preventDefault();

      const currentIndex = selectedId
        ? data.findIndex((r) => r.id === selectedId)
        : -1;

      let nextIndex: number;

      if (e.key === "ArrowDown") {
        if (currentIndex === -1 || currentIndex >= data.length - 1) {
          nextIndex = 0;
        } else {
          nextIndex = currentIndex + 1;
        }
      } else {
        if (currentIndex === -1 || currentIndex <= 0) {
          nextIndex = data.length - 1;
        } else {
          nextIndex = currentIndex - 1;
        }
      }

      const nextRecord = data[nextIndex];
      if (nextRecord) {
        onSelect?.(nextRecord);
        onKeyboardNavigate?.(nextRecord);
        scrollToRow(nextIndex);
      }
    },
    [data, selectedId, onSelect, onKeyboardNavigate, scrollToRow],
  );

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    container.addEventListener("keydown", handleKeyDown);
    return () => {
      container.removeEventListener("keydown", handleKeyDown);
    };
  }, [handleKeyDown]);

  const styles = useMemo(
    () => ({
      container: {
        display: "flex",
        flexDirection: "column" as const,
        height: "100%",
        width: "100%",
        overflow: "hidden",
        position: "relative" as const,
        backgroundColor: token.colorBgContainer,
        outline: "none",
      },
      scrollContainer: {
        flex: 1,
        overflow: "auto",
        position: "relative" as const,
        minWidth: 0,
        backgroundColor: token.colorBgContainer,
      },
      tableInner: {
        minWidth: TABLE_MIN_WIDTH,
        display: "flex",
        flexDirection: "column" as const,
        height: "fit-content",
        minHeight: "100%",
        backgroundColor: token.colorBgContainer,
      },
      header: {
        display: "flex",
        alignItems: "center",
        height: ROW_HEIGHT,
        minWidth: TABLE_MIN_WIDTH,
        borderBottom: `1px solid ${token.colorBorderSecondary}`,
        backgroundColor: token.colorBgContainer,
        fontSize: 12,
        fontWeight: 500,
        color: token.colorTextSecondary,
        position: "sticky" as const,
        top: 0,
        zIndex: 2,
        flexShrink: 0,
      },
      headerCell: {
        padding: "0 8px",
        overflow: "hidden",
        textOverflow: "ellipsis",
        whiteSpace: "nowrap" as const,
      },
      sequenceHeader: {
        display: "flex",
        alignItems: "baseline",
        justifyContent: "flex-end",
        gap: 4,
        width: "100%",
        fontFamily: "monospace",
      },
      sequenceHeaderCount: {
        fontSize: 10,
        fontWeight: 400,
        color: token.colorTextTertiary,
      },
      virtualList: {
        width: "100%",
        minWidth: TABLE_MIN_WIDTH,
        position: "relative" as const,
        willChange: "transform",
        contain: "strict",
        backgroundColor: token.colorBgContainer,
      },
      emptyState: {
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        height: "100%",
        color: token.colorTextSecondary,
      },
      newRecordsIndicator: {
        position: "absolute" as const,
        bottom: 16,
        left: "50%",
        transform: "translateX(-50%)",
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "8px 16px",
        backgroundColor: token.colorPrimary,
        color: "#fff",
        borderRadius: 20,
        cursor: "pointer",
        boxShadow: "0 2px 8px rgba(0, 0, 0, 0.15)",
        zIndex: 100,
        animation: "slideUp 0.3s ease-out",
        transition: "transform 0.2s, box-shadow 0.2s",
      },
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
        zIndex: 100,
        border: `1px solid ${token.colorBorderSecondary}`,
        transition:
          "opacity 0.3s ease, transform 0.3s ease, background-color 0.2s",
      },
      scrollToTopButton: {
        top: 52,
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

  const headerCells = useMemo(
    () =>
      columns.map((col, index) => (
        <div
          key={col.key}
          style={{ ...styles.headerCell, ...columnStyles[index] }}
        >
          {col.key === "sequence" ? (
            <div style={styles.sequenceHeader}>
              <span>{col.title}</span>
              <span style={styles.sequenceHeaderCount}>
                {data.length.toLocaleString()}
              </span>
            </div>
          ) : (
            col.title
          )}
        </div>
      )),
    [
      data.length,
      styles.headerCell,
      styles.sequenceHeader,
      styles.sequenceHeaderCount,
    ],
  );

  const virtualItems = rowVirtualizer.getVirtualItems();

  return (
    <div
      ref={containerRef}
      style={styles.container}
      tabIndex={0}
      data-testid="traffic-table"
    >
      <style>{keyframesStyle}</style>
      <div
        ref={parentRef}
        style={styles.scrollContainer}
        onScroll={handleScroll}
        data-testid="traffic-table-scroll"
      >
        <div style={styles.tableInner}>
          <div style={styles.header}>{headerCells}</div>

          {data.length === 0 ? (
            <div style={styles.emptyState} data-testid="traffic-empty">
              No traffic data
            </div>
          ) : (
            <div
              style={{
                ...styles.virtualList,
                height: `${rowVirtualizer.getTotalSize()}px`,
              }}
            >
              {virtualItems.map((virtualRow) => {
                const record = data[virtualRow.index];
                if (!record) return null;
                return (
                  <TableRow
                    key={virtualRow.key}
                    record={record}
                    liveNow={liveNow}
                    isSelected={record.id === selectedId}
                    isMultiSelected={selectedIds.includes(record.id)}
                    isImported={record.id.startsWith("OUT-") || record.client_app === "Bifrost Import"}
                    translateY={virtualRow.start}
                    rowIndex={virtualRow.index}
                    borderColor={token.colorBorderSecondary}
                    selectedBg={token.colorPrimaryBg}
                    multiSelectedBg={token.colorInfoBg}
                    importedBg={token.colorWarningBg}
                    evenBg={token.colorBgContainer}
                    oddBg={token.colorFillQuaternary}
                    textSecondary={token.colorTextSecondary}
                    onRowClick={(e) => handleRowClick(e, record, virtualRow.index)}
                    onRowDoubleClick={() => onDoubleClick?.(record)}
                    onRowContextMenu={(e) => handleContextMenu(e, record)}
                  />
                );
              })}
            </div>
          )}
        </div>
      </div>

      {!isAtTop && data.length > 0 && (
        <div
          style={{
            ...styles.scrollButton,
            ...styles.scrollToTopButton,
            animation: "fadeSlideDownCenter 0.3s ease-out",
          }}
          data-testid="traffic-scroll-top"
          onClick={handleScrollToTopClick}
        >
          <ArrowUpOutlined style={{ fontSize: 14 }} />
        </div>
      )}

      {showNewIndicator && newRecordsCount > 0 ? (
        <div
          style={{
            ...styles.newRecordsIndicator,
            animation: "slideUp 0.3s ease-out, pulse 2s ease-in-out infinite",
          }}
          data-testid="traffic-new-indicator"
          onClick={handleScrollToBottomClick}
        >
          <Badge
            count={newRecordsCount}
            size="small"
            style={{ backgroundColor: "#fff", color: token.colorPrimary }}
          >
            <span style={{ color: "#fff", fontSize: 13 }}>New Traffic</span>
          </Badge>
          <ArrowDownOutlined style={{ fontSize: 14 }} />
        </div>
      ) : !isAtBottom && data.length > 0 ? (
        <div
          style={{
            ...styles.scrollButton,
            ...styles.scrollToBottomButton,
            animation: "fadeSlideInCenter 0.3s ease-out",
          }}
          data-testid="traffic-scroll-bottom"
          onClick={handleScrollToBottomClick}
        >
          <ArrowDownOutlined style={{ fontSize: 14 }} />
        </div>
      ) : null}

      <TrafficContextMenu
        record={contextMenu.record}
        visible={contextMenu.visible}
        position={contextMenu.position}
        onClose={handleCloseContextMenu}
        selectedRecords={selectedRecords}
      />
    </div>
  );
}
