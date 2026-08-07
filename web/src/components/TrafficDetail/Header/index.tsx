import { useCallback, useState, useRef, useEffect, useMemo, memo } from "react";
import { Typography, Tooltip, Dropdown, message, AutoComplete, Input } from "antd";
import type { MenuProps } from "antd";
import {
  CopyOutlined,
  ExportOutlined,
  ExpandAltOutlined,
  ShrinkOutlined,
  SearchOutlined,
} from "@ant-design/icons";
import type { TrafficRecord, TrafficSummary } from "../../../types";
import { generateCurl } from "../../../utils/curl";
import { copyToClipboard } from "../../../utils/clipboard";
import { useTrafficStore } from "../../../stores/useTrafficStore";

const { Text } = Typography;

interface HeaderProps {
  record: TrafficRecord;
  requestBody: string | null;
  onOpenInNewWindow?: ((record: TrafficRecord) => void) | undefined;
  onSelectById?: (id: string) => void;
}

const STATUS_CODES: Record<number, string> = {
  100: "Continue",
  101: "Switching Protocols",
  200: "OK",
  201: "Created",
  202: "Accepted",
  204: "No Content",
  301: "Moved Permanently",
  302: "Found",
  304: "Not Modified",
  400: "Bad Request",
  401: "Unauthorized",
  403: "Forbidden",
  404: "Not Found",
  500: "Internal Server Error",
  502: "Bad Gateway",
  503: "Service Unavailable",
  504: "Gateway Timeout",
};

const getStatusColor = (status: number): string => {
  if (status >= 500) return "#f5222d";
  if (status >= 400) return "#fa8c16";
  if (status >= 300) return "#faad14";
  if (status >= 200) return "#52c41a";
  return "#1890ff";
};

const MAX_SUGGESTIONS = 20;

const formatSuggestion = (r: TrafficSummary): string => {
  const parts = [r.method, String(r.status), r.host];
  if (r.path && r.path !== "/") parts.push(r.path.length > 40 ? `${r.path.slice(0, 40)}…` : r.path);
  return parts.join("  ");
};

const SequenceSearch = memo(function SequenceSearch({
  currentSequence,
  onSelect,
}: {
  currentSequence?: number;
  onSelect: (id: string) => void;
}) {
  const records = useTrafficStore((state) => state.records);
  const [searching, setSearching] = useState(false);
  const [searchValue, setSearchValue] = useState("");
  const [hovered, setHovered] = useState(false);
  const inputRef = useRef<{ focus: () => void } | null>(null);

  const handleCopySequence = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      if (currentSequence != null) {
        copyToClipboard(String(currentSequence));
        message.success("Sequence copied");
      }
    },
    [currentSequence],
  );

  const options = useMemo(() => {
    const keyword = searchValue.trim();
    if (!keyword) return [];
    const matched: TrafficSummary[] = [];
    for (const r of records) {
      if (String(r.sequence).includes(keyword)) {
        matched.push(r);
        if (matched.length >= MAX_SUGGESTIONS) break;
      }
    }
    return matched.map((r) => ({
      value: r.id,
      label: (
        <div style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12, fontFamily: "monospace" }}>
          <span style={{ fontWeight: 600, minWidth: 48, textAlign: "right" }}>#{r.sequence}</span>
          <span style={{ color: "#666", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {formatSuggestion(r)}
          </span>
        </div>
      ),
    }));
  }, [records, searchValue]);

  const handleSelect = useCallback(
    (id: string) => {
      onSelect(id);
      setSearching(false);
      setSearchValue("");
    },
    [onSelect],
  );

  const handleBlur = useCallback(() => {
    setTimeout(() => {
      setSearching(false);
      setSearchValue("");
    }, 200);
  }, []);

  const handleStartSearch = useCallback(() => {
    setSearching(true);
    setTimeout(() => inputRef.current?.focus(), 50);
  }, []);

  if (searching) {
    return (
      <AutoComplete
        options={options}
        onSelect={handleSelect}
        onSearch={setSearchValue}
        value={searchValue}
        style={{ width: 200 }}
        popupMatchSelectWidth={360}
      >
        <Input
          ref={(el) => { inputRef.current = el; }}
          size="small"
          placeholder="Search by #seq..."
          prefix={<SearchOutlined style={{ color: "#bbb" }} />}
          onBlur={handleBlur}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              setSearching(false);
              setSearchValue("");
            }
          }}
          style={{ fontSize: 12 }}
        />
      </AutoComplete>
    );
  }

  return (
    <div
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 2,
        flexShrink: 0,
      }}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      <Tooltip title="Click to search by sequence number">
        <Text
          strong
          onClick={handleStartSearch}
          style={{
            whiteSpace: "nowrap",
            userSelect: "none",
            flexShrink: 0,
            cursor: "pointer",
            color: "#999",
            fontSize: 12,
            fontFamily: "monospace",
          }}
        >
          #{currentSequence}
        </Text>
      </Tooltip>
      {hovered && (
        <Tooltip title="Copy sequence number">
          <CopyOutlined
            onClick={handleCopySequence}
            style={{
              fontSize: 12,
              cursor: "pointer",
              color: "#999",
              padding: 2,
            }}
          />
        </Tooltip>
      )}
    </div>
  );
});

const HeaderContent = memo(function HeaderContent({
  record,
  requestBody,
  onOpenInNewWindow,
  onSelectById,
}: {
  record: TrafficRecord;
  requestBody: string | null;
  onOpenInNewWindow?: ((record: TrafficRecord) => void) | undefined;
  onSelectById?: (id: string) => void;
}) {
  const { method, status, url } = record;
  const statusText = STATUS_CODES[status] || "";
  const statusLabel = statusText ? `${status} ${statusText}` : String(status);
  const statusColor = getStatusColor(status);

  const [expanded, setExpanded] = useState(false);
  const [isOverflow, setIsOverflow] = useState(false);
  const urlRef = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    const el = urlRef.current;
    if (el) {
      setIsOverflow(el.scrollWidth > el.clientWidth);
    }
  }, [url]);

  const handleCopyUrl = useCallback(() => {
    copyToClipboard(url);
    message.success("URL copied");
  }, [url]);

  const handleCopyCurl = useCallback(() => {
    const curl = generateCurl({
      ...record,
      request_body: requestBody,
    });
    copyToClipboard(curl);
    message.success("cURL copied");
  }, [record, requestBody]);

  const copyMenuItems: MenuProps["items"] = [
    { key: "url", label: "Copy URL", onClick: handleCopyUrl },
    { key: "curl", label: "Copy as cURL", onClick: handleCopyCurl },
  ];

  const toggleExpand = useCallback(() => {
    setExpanded((prev) => !prev);
  }, []);

  const handleOpenInNewWindow = useCallback(() => {
    onOpenInNewWindow?.(record);
  }, [onOpenInNewWindow, record]);

  return (
    <div
      style={{
        display: "flex",
        alignItems: "flex-start",
        padding: "4px 8px",
        borderBottom: "1px solid #f0f0f0",
        gap: 8,
      }}
      data-testid="traffic-detail-header"
      data-url={url}
    >
      <div
        style={{
          display: "flex",
          alignItems: expanded ? "flex-start" : "center",
          flex: 1,
          overflow: "hidden",
          gap: 8,
        }}
      >
        {onSelectById && (
          <SequenceSearch
            currentSequence={record.sequence}
            onSelect={onSelectById}
          />
        )}
        <Text
          strong
          style={{
            whiteSpace: "nowrap",
            userSelect: "none",
            flexShrink: 0,
          }}
        >
          {method}
        </Text>
        <Text
          style={{
            whiteSpace: "nowrap",
            userSelect: "none",
            color: statusColor,
            fontWeight: 500,
            flexShrink: 0,
          }}
        >
          {statusLabel}
        </Text>
        <Text
          ref={urlRef}
          style={{
            overflow: "hidden",
            textOverflow: expanded ? "unset" : "ellipsis",
            whiteSpace: expanded ? "normal" : "nowrap",
            wordBreak: expanded ? "break-all" : "normal",
            color: "#666",
            flex: 1,
          }}
        >
          {url}
        </Text>
      </div>

      <div
        style={{ display: "flex", alignItems: "center", gap: 4, flexShrink: 0 }}
      >
        {isOverflow && (
          <Tooltip title={expanded ? "Collapse" : "Expand"}>
            {expanded ? (
              <ShrinkOutlined
                onClick={toggleExpand}
                style={{
                  fontSize: 14,
                  padding: 4,
                  cursor: "pointer",
                  color: "#666",
                }}
              />
            ) : (
              <ExpandAltOutlined
                onClick={toggleExpand}
                style={{
                  fontSize: 14,
                  padding: 4,
                  cursor: "pointer",
                  color: "#666",
                }}
              />
            )}
          </Tooltip>
        )}
        {onOpenInNewWindow ? (
          <Tooltip title="Open in new window">
            <ExportOutlined
              onClick={handleOpenInNewWindow}
              data-testid="traffic-detail-open-window"
              style={{
                fontSize: 14,
                padding: 4,
                cursor: "pointer",
                color: "#666",
              }}
            />
          </Tooltip>
        ) : null}
        <Dropdown menu={{ items: copyMenuItems }} trigger={["click"]}>
          <Tooltip title="Copy">
            <CopyOutlined
              data-testid="traffic-detail-copy-trigger"
              style={{
                fontSize: 14,
                padding: 4,
                cursor: "pointer",
                color: "#666",
              }}
            />
          </Tooltip>
        </Dropdown>
      </div>
    </div>
  );
});

export const Header = ({
  record,
  requestBody,
  onOpenInNewWindow,
  onSelectById,
}: HeaderProps) => {
  return (
    <HeaderContent
      key={record.id}
      record={record}
      requestBody={requestBody}
      onOpenInNewWindow={onOpenInNewWindow}
      onSelectById={onSelectById}
    />
  );
};
