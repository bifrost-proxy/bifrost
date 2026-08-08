"use no memo";

import { useRef, useCallback, useMemo, type CSSProperties } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Tag, theme } from "antd";
import { ThunderboltOutlined } from "@ant-design/icons";
import type { SearchResultItem } from "../../types";
import { TrafficFlags } from "../../types";
import AppIcon from "../AppIcon";

interface SearchResultsListProps {
  results: SearchResultItem[];
  keyword: string;
  selectedId?: string;
  breakpointPhases?: Map<string, "request" | "response">;
  onSelect: (item: SearchResultItem) => void;
  onDoubleClick: (item: SearchResultItem) => void;
  onLoadMore: () => void;
  hasMore: boolean;
  isLoadingMore: boolean;
}

const ROW_HEIGHT = 64;

const getStatusColor = (status: number): string => {
  if (status === 0) return "#ff4d4f";
  if (status >= 500) return "#ff4d4f";
  if (status >= 400) return "#faad14";
  if (status >= 300) return "#1890ff";
  if (status >= 200) return "#52c41a";
  if (status >= 100) return "#d9d9d9";
  return "#d9d9d9";
};

const getMethodColor = (method: string): string => {
  const colors: Record<string, string> = {
    GET: "#52c41a",
    POST: "#1890ff",
    PUT: "#faad14",
    DELETE: "#ff4d4f",
    PATCH: "#722ed1",
    OPTIONS: "#8c8c8c",
    HEAD: "#8c8c8c",
    CONNECT: "#eb2f96",
  };
  return colors[method.toUpperCase()] || "#8c8c8c";
};

const formatSize = (bytes: number): string => {
  if (bytes === 0) return "-";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
};

const highlightText = (
  text: string,
  keyword: string,
  highlightColor: string
): React.ReactNode => {
  if (!keyword.trim()) return text;

  const lowerText = text.toLowerCase();
  const lowerKeyword = keyword.toLowerCase();
  const index = lowerText.indexOf(lowerKeyword);

  if (index === -1) return text;

  const before = text.substring(0, index);
  const match = text.substring(index, index + keyword.length);
  const after = text.substring(index + keyword.length);

  return (
    <>
      {before}
      <span style={{ backgroundColor: highlightColor, padding: "0 2px" }}>
        {match}
      </span>
      {after}
    </>
  );
};

const MatchPreview = ({
  preview,
  field,
  keyword,
  highlightColor,
}: {
  preview: string;
  field: string;
  keyword: string;
  highlightColor: string;
}) => {
  const fieldLabels: Record<string, string> = {
    url: "URL",
    request_header: "Req Header",
    response_header: "Res Header",
    request_body: "Req Body",
    response_body: "Res Body",
  };

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        fontSize: 11,
        overflow: "hidden",
      }}
    >
      <Tag
        color="blue"
        style={{ margin: 0, fontSize: 10, lineHeight: "16px", padding: "0 4px" }}
      >
        {fieldLabels[field] || field}
      </Tag>
      <span
        style={{
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          color: "#8c8c8c",
        }}
      >
        {highlightText(preview, keyword, highlightColor)}
      </span>
    </div>
  );
};

export default function SearchResultsList({
  results,
  keyword,
  selectedId,
  breakpointPhases,
  onSelect,
  onDoubleClick,
  onLoadMore,
  hasMore,
  isLoadingMore,
}: SearchResultsListProps) {
  const { token } = theme.useToken();
  const parentRef = useRef<HTMLDivElement>(null);

  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: results.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 5,
  });

  const handleScroll = useCallback(() => {
    if (!parentRef.current || isLoadingMore || !hasMore) return;

    const { scrollTop, scrollHeight, clientHeight } = parentRef.current;
    if (scrollHeight - scrollTop - clientHeight < 100) {
      onLoadMore();
    }
  }, [hasMore, isLoadingMore, onLoadMore]);

  const styles = useMemo<Record<string, CSSProperties>>(
    () => ({
      container: {
        height: "100%",
        overflow: "auto",
      },
      list: {
        height: `${virtualizer.getTotalSize()}px`,
        width: "100%",
        position: "relative",
      },
      row: {
        position: "absolute",
        top: 0,
        left: 0,
        width: "100%",
        height: ROW_HEIGHT,
        display: "flex",
        flexDirection: "column",
        padding: "6px 16px",
        borderBottom: `1px solid ${token.colorBorderSecondary}`,
        cursor: "pointer",
        overflow: "hidden",
      },
      mainRow: {
        display: "flex",
        alignItems: "center",
        gap: 8,
        height: 24,
      },
      matchRow: {
        display: "flex",
        alignItems: "center",
        gap: 4,
        height: 22,
        overflow: "hidden",
      },
    }),
    [token, virtualizer]
  );

  return (
    <div ref={parentRef} style={styles.container} onScroll={handleScroll}>
      <div style={styles.list}>
        {virtualizer.getVirtualItems().map((virtualRow) => {
          const item = results[virtualRow.index];
          const record = item.record;
          const isSelected = record.id === selectedId;
          const breakpointPhase = breakpointPhases?.get(record.id);
          const hasRuleHit = (record.flags & TrafficFlags.HAS_RULE_HIT) !== 0;

          return (
            <div
              key={record.id}
              data-testid="search-result-row"
              data-record-id={record.id}
              data-breakpoint-phase={breakpointPhase}
              style={{
                ...styles.row,
                transform: `translateY(${virtualRow.start}px)`,
                backgroundColor: breakpointPhase
                  ? token.colorWarningBg
                  : isSelected
                    ? token.colorPrimaryBg
                    : "transparent",
                boxShadow:
                  breakpointPhase && isSelected
                    ? `inset 3px 0 ${token.colorPrimary}`
                    : undefined,
              }}
              onClick={() => onSelect(item)}
              onDoubleClick={() => onDoubleClick(item)}
            >
              <div style={styles.mainRow}>
                <span
                  style={{
                    width: 8,
                    height: 8,
                    borderRadius: "50%",
                    backgroundColor: getStatusColor(record.s),
                    flexShrink: 0,
                  }}
                />
                <Tag
                  color={getMethodColor(record.m)}
                  style={{ margin: 0, fontSize: 11, lineHeight: "18px" }}
                >
                  {record.m}
                </Tag>
                <span
                  style={{
                    fontSize: 12,
                    color: token.colorTextSecondary,
                  }}
                >
                  {record.s || "-"}
                </span>
                <span
                  style={{
                    flex: 1,
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                    fontSize: 12,
                  }}
                >
                  <span style={{ color: token.colorText }}>{record.h}</span>
                  <span style={{ color: token.colorTextSecondary }}>
                    {record.p}
                  </span>
                </span>
                {hasRuleHit && (
                  <ThunderboltOutlined
                    style={{ color: token.colorWarning, fontSize: 12 }}
                  />
                )}
                {breakpointPhase && (
                  <Tag
                    color="warning"
                    data-testid={`search-breakpoint-${breakpointPhase}-indicator`}
                    style={{ margin: 0, fontSize: 10, lineHeight: "16px" }}
                  >
                    Breakpoint · {breakpointPhase === "request" ? "Request" : "Response"}
                  </Tag>
                )}
                <span
                  style={{
                    fontSize: 11,
                    color: token.colorTextSecondary,
                    minWidth: 50,
                    textAlign: "right",
                  }}
                >
                  {formatSize(record.res_sz)}
                </span>
                <span
                  style={{
                    fontSize: 11,
                    color: token.colorTextSecondary,
                    fontFamily: "monospace",
                    minWidth: 70,
                    textAlign: "right",
                  }}
                  title={record.st}
                >
                  {record.st || "-"}
                </span>
                {record.capp && (
                  <AppIcon appName={record.capp} size={14} />
                )}
              </div>
              <div style={styles.matchRow}>
                {item.matches.slice(0, 2).map((match, idx) => (
                  <MatchPreview
                    key={idx}
                    preview={match.preview}
                    field={match.field}
                    keyword={keyword}
                    highlightColor={token.colorWarningBg}
                  />
                ))}
                {item.matches.length > 2 && (
                  <Tag
                    style={{
                      margin: 0,
                      fontSize: 10,
                      lineHeight: "16px",
                      padding: "0 4px",
                    }}
                  >
                    +{item.matches.length - 2} more
                  </Tag>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
