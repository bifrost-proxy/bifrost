import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Button, Card, Divider, Dropdown, Empty, Input, Modal, Popover, Space, Tag, Typography, theme } from "antd";
import {
  DatabaseOutlined,
  DeleteOutlined,
  ExclamationCircleOutlined,
  FolderOpenOutlined,
  HistoryOutlined,
  RightOutlined,
  PlayCircleOutlined,
  SyncOutlined,
  WarningOutlined,
} from "@ant-design/icons";
import {
  ACTIVITY_ITEMS,
  MetricRow,
  compactionTagColor,
  formatContextManagement,
  formatContextUsage,
  formatContextWindow,
  formatDuration,
  formatLabel,
  formatLoopProgress,
  formatModelRef,
  formatNumber,
  formatRelativeTime,
  formatReasoningRef,
  formatRunPhase,
  formatRunnerTag,
  formatStatusMetricCount,
  formatThreadRunnerMark,
  formatThreadSource,
  isSelectedThread,
  statusTagColor,
  type AgentThreadSummary,
  type RunTelemetry,
} from "./AgentChatSection.helpers";
import { isThreadActive } from "./AgentChatSection.timelinePolling";

const { Text, Paragraph } = Typography;

const INITIAL_THREAD_LOAD_COUNT = 20;
const THREAD_LOAD_INCREMENT = 20;
const THREAD_ROW_ESTIMATE_SIZE = 68;

type AgentChatSettingsModalProps = {
  open: boolean;
  onClose: () => void;
  styles: Record<string, CSSProperties>;
  workDir: string;
  defaultWorkDir: string;
  telemetry: RunTelemetry;
  historyPath?: string;
};

type AgentThreadListCardProps = {
  threads: AgentThreadSummary[];
  sessionKey: string;
  historyPath?: string;
  view?: string;
  nowSeconds: number;
  styles: Record<string, CSSProperties>;
  compact?: boolean;
  onOpenThread: (thread: AgentThreadSummary) => void;
  onDeleteThread: (thread: AgentThreadSummary) => void;
  onCollapse?: () => void;
};

function metadataValue(
  metadata: Record<string, string> | undefined,
  keys: string[],
) {
  for (const key of keys) {
    const value = metadata?.[key];
    if (value && value.trim()) {
      return value;
    }
  }
  return undefined;
}

function metadataNumber(
  metadata: Record<string, string> | undefined,
  keys: string[],
) {
  const value = metadataValue(metadata, keys);
  if (!value) {
    return undefined;
  }
  const number = Number(value);
  return Number.isFinite(number) ? number : undefined;
}

function formatMetadataNumber(
  metadata: Record<string, string> | undefined,
  keys: string[],
) {
  return formatNumber(metadataNumber(metadata, keys));
}

function formatMetadataCount(
  metadata: Record<string, string> | undefined,
  keys: string[],
) {
  return formatStatusMetricCount(metadataNumber(metadata, keys));
}

function formatMetadataDurationMs(
  metadata: Record<string, string> | undefined,
  keys: string[],
) {
  const value = metadataNumber(metadata, keys);
  if (typeof value !== "number") {
    return "-";
  }
  if (value < 1000) {
    return `${Math.max(0, Math.round(value))}ms`;
  }
  return `${(value / 1000).toFixed(value < 10_000 ? 1 : 0)}s`;
}

function formatRunnerCli(metadata: Record<string, string> | undefined) {
  const executable = metadataValue(metadata, ["cli.executable"]);
  const version = metadataValue(metadata, ["cli.version"]);
  if (executable && version) {
    return `${executable} · ${version}`;
  }
  return executable || version || "-";
}

export function AgentThreadListCard({
  threads,
  sessionKey,
  historyPath,
  view,
  nowSeconds,
  styles,
  compact = false,
  onOpenThread,
  onDeleteThread,
  onCollapse,
}: AgentThreadListCardProps) {
  const { token } = theme.useToken();
  const threadScrollRef = useRef<HTMLDivElement>(null);
  const [contextThreadKey, setContextThreadKey] = useState<string | null>(null);
  const [confirmDeleteKey, setConfirmDeleteKey] = useState<string | null>(null);
  const [visibleLimit, setVisibleLimit] = useState(INITIAL_THREAD_LOAD_COUNT);
  const isRunningThread = isThreadActive;
  const threadDuration = (thread: AgentThreadSummary) => {
    if (isRunningThread(thread) && thread.start_time) {
      return Math.max(0, nowSeconds - thread.start_time);
    }
    return thread.duration_secs || (
      thread.start_time && thread.last_active_time
        ? thread.last_active_time - thread.start_time
        : undefined
    );
  };
  const threadMeta = (thread: AgentThreadSummary) => {
    const created = formatRelativeTime(thread.start_time || thread.last_active_time, nowSeconds);
    return `${formatThreadSource(thread)} · ${created} · duration ${formatDuration(threadDuration(thread))}`;
  };
  const threadDetails = (thread: AgentThreadSummary) => (
    <Space direction="vertical" size={6} style={{ maxWidth: 320 }}>
      <Text strong>{thread.title || thread.session_key}</Text>
      <Text type="secondary">Workspace: {thread.work_dir || "Not recorded"}</Text>
      <Text type="secondary">{formatRunnerTag(undefined, thread)}</Text>
      <Text type="secondary">Source: {formatThreadSource(thread)}</Text>
      <Text type="secondary">
        State: {isRunningThread(thread) ? "Running" : "Ready"}
      </Text>
      <Text type="secondary">
        Created: {formatRelativeTime(thread.start_time || thread.last_active_time, nowSeconds)}
      </Text>
      <Text type="secondary">Duration: {formatDuration(threadDuration(thread))}</Text>
    </Space>
  );
  const selectedThreadIndex = useMemo(
    () =>
      threads.findIndex((thread) =>
        isSelectedThread(thread, sessionKey, historyPath, view),
      ),
    [historyPath, sessionKey, threads, view],
  );
  const minimumVisibleLimit = Math.max(
    INITIAL_THREAD_LOAD_COUNT,
    selectedThreadIndex >= 0 ? selectedThreadIndex + 1 : 0,
  );

  useEffect(() => {
    setVisibleLimit((current) => {
      if (threads.length === 0) {
        return INITIAL_THREAD_LOAD_COUNT;
      }
      const required = Math.min(threads.length, minimumVisibleLimit);
      return Math.min(threads.length, Math.max(current, required));
    });
  }, [minimumVisibleLimit, threads.length]);

  const visibleThreads = useMemo(
    () => threads.slice(0, Math.min(visibleLimit, threads.length)),
    [threads, visibleLimit],
  );
  const hasMoreThreads = visibleThreads.length < threads.length;

  const threadVirtualizer = useVirtualizer({
    count: visibleThreads.length,
    getScrollElement: () => threadScrollRef.current,
    estimateSize: () => (compact ? 38 : THREAD_ROW_ESTIMATE_SIZE),
    overscan: 6,
    getItemKey: (index) => {
      const thread = visibleThreads[index];
      return thread?.history_path || thread?.session_key || index;
    },
  });

  const loadMoreThreads = () => {
    setVisibleLimit((current) =>
      Math.min(threads.length, Math.max(current, INITIAL_THREAD_LOAD_COUNT) + THREAD_LOAD_INCREMENT),
    );
  };

  return (
    <Card
      size="small"
      style={styles.threadCard}
      bodyStyle={styles.threadCardBody}
      title={onCollapse ? (
        <div style={styles.threadCardTitle}>
          <span style={styles.threadCardTitleMain}>
            <HistoryOutlined />
            <span>Threads</span>
          </span>
          {onCollapse ? (
            <Button
              size="small"
              type="text"
              icon={<RightOutlined />}
              aria-label="Collapse threads"
              title="Collapse threads"
              data-testid="agent-chat-threads-collapse"
              style={styles.threadCollapseButton}
              onClick={onCollapse}
            />
          ) : null}
        </div>
      ) : undefined}
    >
      <div
        ref={threadScrollRef}
        style={styles.threadList}
        data-testid="agent-chat-thread-list"
      >
        {threads.length === 0 ? (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="No previous sessions" />
        ) : (
          <>
            <div
              style={{
                ...styles.threadVirtualSpace,
                height: threadVirtualizer.getTotalSize(),
              }}
              data-testid="agent-chat-thread-virtual-space"
            >
              {threadVirtualizer.getVirtualItems().map((virtualItem) => {
                const thread = visibleThreads[virtualItem.index];
                if (!thread) {
                  return null;
                }
                const selected = isSelectedThread(thread, sessionKey, historyPath, view);
                const threadKey = thread.history_path || thread.session_key;
                const showConfirm = confirmDeleteKey === threadKey;
                return (
                  <div
                    key={virtualItem.key}
                    ref={threadVirtualizer.measureElement}
                    data-index={virtualItem.index}
                    data-testid="agent-chat-thread-virtual-row"
                    style={{
                      ...styles.threadVirtualRow,
                      transform: `translateY(${virtualItem.start}px)`,
                    }}
                  >
                    <Dropdown
                      trigger={["contextMenu"]}
                      open={contextThreadKey === threadKey}
                      onOpenChange={(open) => {
                        if (open) {
                          setContextThreadKey(threadKey);
                          return;
                        }
                        setContextThreadKey((current) => (current === threadKey ? null : current));
                        setConfirmDeleteKey((current) => (current === threadKey ? null : current));
                      }}
                      dropdownRender={() => (
                        <div style={styles.threadContextMenu}>
                          {showConfirm ? (
                            <Space direction="vertical" size={6} style={{ width: "100%" }}>
                              <Text type="secondary" style={{ fontSize: 12 }}>
                                Delete this conversation?
                              </Text>
                              <Space size={6}>
                                <button
                                  type="button"
                                  data-testid="agent-chat-thread-delete-confirm"
                                  style={{
                                    ...styles.threadContextButton,
                                    ...styles.threadContextDangerButton,
                                  }}
                                  onClick={() => {
                                    onDeleteThread(thread);
                                    setConfirmDeleteKey(null);
                                    setContextThreadKey(null);
                                  }}
                                >
                                  Confirm
                                </button>
                                <button
                                  type="button"
                                  data-testid="agent-chat-thread-delete-cancel"
                                  style={styles.threadContextButton}
                                  onClick={() => {
                                    setConfirmDeleteKey(null);
                                  }}
                                >
                                  Cancel
                                </button>
                              </Space>
                            </Space>
                          ) : (
                            <button
                              type="button"
                              data-testid="agent-chat-thread-delete"
                              style={{
                                ...styles.threadContextButton,
                                ...styles.threadContextDangerButton,
                              }}
                              onClick={() => setConfirmDeleteKey(threadKey)}
                            >
                              <DeleteOutlined /> Delete
                            </button>
                          )}
                        </div>
                      )}
                    >
                      <button
                        type="button"
                        onClick={() => onOpenThread(thread)}
                        onContextMenu={() => {
                          setContextThreadKey(threadKey);
                          setConfirmDeleteKey(null);
                        }}
                        data-testid="agent-chat-thread-item"
                        data-selected={selected ? "true" : "false"}
                        aria-current={selected ? "true" : undefined}
                        style={{
                          ...styles.threadItem,
                          ...(compact
                            ? {
                                gap: 6,
                                height: 36,
                                minHeight: 36,
                                padding: "0 8px",
                                borderRadius: 7,
                              }
                            : {}),
                          ...(selected ? styles.threadItemSelected : {}),
                          ...(compact && selected
                            ? {
                                background: token.colorFillSecondary,
                                color: token.colorText,
                              }
                            : {}),
                        }}
                      >
                        {compact ? null : (
                          <Popover
                            trigger="hover"
                            placement="left"
                            content={threadDetails(thread)}
                            mouseEnterDelay={0.5}
                          >
                            <span
                              data-testid="agent-chat-thread-runner-mark"
                              style={{
                                ...styles.threadRunnerMark,
                                ...(selected
                                  ? {
                                      background: token.colorPrimaryBgHover,
                                      color: token.colorPrimary,
                                    }
                                  : {}),
                              }}
                            >
                              {formatThreadRunnerMark(thread)}
                            </span>
                          </Popover>
                        )}
                        <span style={{ flex: "1 1 auto", minWidth: 0 }}>
                          <span
                            style={{
                              display: "block",
                              overflow: "hidden",
                              textOverflow: "ellipsis",
                              whiteSpace: "nowrap",
                              fontSize: compact ? 12 : undefined,
                              lineHeight: compact ? "18px" : undefined,
                              fontWeight: compact ? 500 : selected ? 600 : 500,
                            }}
                          >
                            {thread.title || thread.session_key}
                          </span>
                          {compact ? null : (
                            <span
                              style={{
                                display: "block",
                                overflow: "hidden",
                                textOverflow: "ellipsis",
                                whiteSpace: "nowrap",
                                color: selected ? token.colorPrimaryText : token.colorTextSecondary,
                                fontSize: 12,
                                marginTop: 2,
                              }}
                            >
                              {threadMeta(thread)}
                            </span>
                          )}
                        </span>
                        {isRunningThread(thread) ? (
                          <span aria-label="running" title="Running" style={styles.runningDot} />
                        ) : null}
                      </button>
                    </Dropdown>
                  </div>
                );
              })}
            </div>
            <div style={styles.threadLoadMoreBar}>
              <Text
                type="secondary"
                style={{ fontSize: 12 }}
                data-testid="agent-chat-thread-load-count"
              >
                Showing {visibleThreads.length} of {threads.length}
              </Text>
              {hasMoreThreads ? (
                <Button
                  size="small"
                  block
                  onClick={loadMoreThreads}
                  data-testid="agent-chat-thread-load-more"
                >
                  Load more
                </Button>
              ) : null}
            </div>
          </>
        )}
      </div>
    </Card>
  );
}

export function AgentChatSettingsModal({
  open,
  onClose,
  styles,
  workDir,
  defaultWorkDir,
  telemetry,
  historyPath,
}: AgentChatSettingsModalProps) {
  const { token } = theme.useToken();

  return (
    <Modal
      title="Agent Chat Status"
      open={open}
      onCancel={onClose}
      footer={null}
      width={720}
      destroyOnHidden
    >
      <Space
        direction="vertical"
        size={16}
        style={styles.settingsModalContent}
        data-testid="agent-chat-settings-modal"
      >
        <Card
          size="small"
          title={
            <Space>
              <FolderOpenOutlined />
              <span>Workspace</span>
            </Space>
          }
        >
          <Space direction="vertical" size={8} style={{ width: "100%" }}>
            <Input
              size="small"
              placeholder="Working directory (leave empty for default)"
              value={workDir || defaultWorkDir}
              data-testid="agent-chat-workspace-display"
              prefix={<FolderOpenOutlined style={{ color: "rgba(0,0,0,0.25)" }} />}
              disabled
            />
            {telemetry.status?.work_dir ? (
              <Text type="secondary" style={{ fontSize: 12 }}>
                Active: {telemetry.status.work_dir}
              </Text>
            ) : null}
          </Space>
        </Card>

        <Card size="small" data-testid="agent-chat-info">
          <Space direction="vertical" size={6} style={{ width: "100%" }}>
            <Space wrap size={4}>
              <Tag color="blue">Agent Chat</Tag>
              <Tag>{historyPath ? "History Continue" : "Live API"}</Tag>
            </Space>
            <Text strong>Streaming Agent workspace</Text>
            <Paragraph type="secondary" style={{ margin: 0, fontSize: 12 }}>
              Prompt shortcuts, execution context, and safe JSONL history
              continue are available in this compact chat panel.
            </Paragraph>
          </Space>
        </Card>

        <Card
          size="small"
          title={
            <Space>
              <SyncOutlined spin={telemetry.phase === "running"} />
              <span>Status</span>
            </Space>
          }
          data-testid="agent-chat-status"
        >
          <Space direction="vertical" size={8} style={{ width: "100%" }}>
            <Space wrap size={4}>
              <Tag color={statusTagColor(telemetry.phase)}>
                {formatRunPhase(telemetry.phase)}
              </Tag>
              {telemetry.status?.state ? (
                <Tag>{formatLabel(telemetry.status.state)}</Tag>
              ) : null}
            </Space>
            <MetricRow label="Model" value={formatModelRef(telemetry.status)} />
            <MetricRow label="Reasoning" value={formatReasoningRef(telemetry.status)} />
            <MetricRow label="Loop" value={formatLoopProgress(telemetry.status)} />
            <MetricRow
              label="Messages"
              value={formatNumber(telemetry.status?.message_count)}
            />
            <MetricRow
              label="Explicit compactions"
              value={formatNumber(telemetry.status?.compaction_count)}
            />
            <MetricRow
              label="Context management"
              value={formatContextManagement(telemetry.status, telemetry.context)}
            />
          </Space>
        </Card>

        <Card
          size="small"
          title={
            <Space>
              <DatabaseOutlined />
              <span>Context</span>
            </Space>
          }
          data-testid="agent-chat-context"
        >
          <Space direction="vertical" size={8} style={{ width: "100%" }}>
            <MetricRow
              label="Usage"
              value={formatContextUsage(telemetry.status, telemetry.context)}
            />
            <MetricRow
              label="Tokens"
              value={formatStatusMetricCount(
                telemetry.status?.total_tokens_used ??
                  telemetry.context?.totalTokensUsed,
              )}
            />
            <MetricRow
              label="Window"
              value={formatContextWindow(telemetry.status, telemetry.context)}
            />
            <MetricRow
              label="Explicit compactions"
              value={formatNumber(
                telemetry.status?.compaction_count ??
                  telemetry.context?.compactionCount,
              )}
            />
            <MetricRow
              label="Context management"
              value={formatContextManagement(telemetry.status, telemetry.context)}
            />
            <MetricRow
              label="Local tools"
              value={formatNumber(telemetry.status?.local_tool_count)}
            />
            <MetricRow
              label="MCP tools"
              value={formatNumber(telemetry.status?.mcp_tool_count)}
            />
            {telemetry.status?.runner_type || telemetry.status?.runner_id ? (
              <>
                <Divider style={{ margin: "4px 0" }} />
                <Text type="secondary">
                  Runner: {[telemetry.status.runner_type, telemetry.status.runner_id]
                    .filter(Boolean)
                    .join(" / ")}
                </Text>
                <MetricRow label="Model" value={formatModelRef(telemetry.status)} />
                <MetricRow label="Reasoning" value={formatReasoningRef(telemetry.status)} />
              </>
            ) : null}
            {telemetry.status?.metadata ? (
              <>
                <Divider style={{ margin: "4px 0" }} />
                <Text type="secondary">Runner diagnostics</Text>
                <MetricRow
                  label="CLI"
                  value={formatRunnerCli(telemetry.status.metadata)}
                />
                <MetricRow
                  label="Prompt"
                  value={`${formatMetadataCount(telemetry.status.metadata, [
                    "prompt.estimatedTokens",
                  ])} tok · ${formatMetadataCount(telemetry.status.metadata, [
                    "prompt.bytes",
                  ])} bytes`}
                />
                <MetricRow
                  label="Attachments"
                  value={`${formatMetadataNumber(telemetry.status.metadata, [
                    "attachments.count",
                  ])} · ${formatMetadataCount(telemetry.status.metadata, [
                    "attachments.totalBytes",
                  ])} bytes`}
                />
                <MetricRow
                  label="I/O"
                  value={`${formatMetadataCount(telemetry.status.metadata, [
                    "io.stdoutBytes",
                  ])} out · ${formatMetadataCount(telemetry.status.metadata, [
                    "io.stderrBytes",
                  ])} err`}
                />
                <MetricRow
                  label="Tools"
                  value={`${formatMetadataNumber(telemetry.status.metadata, [
                    "tools.count",
                  ])} calls · ${formatMetadataNumber(telemetry.status.metadata, [
                    "tools.failedCount",
                  ])} failed`}
                />
                <MetricRow
                  label="Tool time"
                  value={formatMetadataDurationMs(telemetry.status.metadata, [
                    "tools.totalDurationMs",
                  ])}
                />
                <MetricRow
                  label="Run time"
                  value={formatMetadataDurationMs(telemetry.status.metadata, [
                    "timing.totalDurationMs",
                  ])}
                />
                <MetricRow
                  label="First event"
                  value={formatMetadataDurationMs(telemetry.status.metadata, [
                    "timing.firstEventLatencyMs",
                  ])}
                />
                <MetricRow
                  label="Resume"
                  value={metadataValue(telemetry.status.metadata, [
                    "resume.requested",
                  ]) || "-"}
                />
              </>
            ) : null}
            {telemetry.compaction ? (
              <>
                <Divider style={{ margin: "4px 0" }} />
                <Space direction="vertical" size={4} style={{ width: "100%" }}>
                  <Space wrap size={4}>
                    <Tag color={compactionTagColor(telemetry.compactionPhase)}>
                      {formatLabel(telemetry.compactionPhase || "finished")}
                    </Tag>
                    {telemetry.compaction.phase ? (
                      <Text>{formatLabel(telemetry.compaction.phase)}</Text>
                    ) : null}
                  </Space>
                  <MetricRow
                    label="Saved"
                    value={formatNumber(telemetry.compaction.tokensSaved)}
                  />
                  <MetricRow
                    label="Removed"
                    value={formatNumber(telemetry.compaction.messagesRemoved)}
                  />
                </Space>
              </>
            ) : null}
          </Space>
        </Card>

        <Card
          size="small"
          title={
            <Space>
              <WarningOutlined />
              <span>Errors</span>
            </Space>
          }
          data-testid="agent-chat-errors"
        >
          {telemetry.errors.length === 0 ? (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="No errors" />
          ) : (
            <Space direction="vertical" size={8} style={{ width: "100%" }}>
              {telemetry.errors.map((error, index) => (
                <div key={`${index}-${error}`} style={styles.telemetrySection}>
                  <Space align="start">
                    <ExclamationCircleOutlined style={{ color: token.colorError }} />
                    <Text type="danger">{error}</Text>
                  </Space>
                </div>
              ))}
            </Space>
          )}
        </Card>

        <Card
          size="small"
          title={
            <Space>
              <PlayCircleOutlined />
              <span>Run Settings</span>
            </Space>
          }
        >
          <Space direction="vertical" size={8}>
            {ACTIVITY_ITEMS.map((item) => (
              <div key={item.label}>
                <Text type="secondary" style={{ fontSize: 12 }}>
                  {item.label}
                </Text>
                <div>
                  <Text>{item.value}</Text>
                </div>
              </div>
            ))}
          </Space>
        </Card>
      </Space>
    </Modal>
  );
}
