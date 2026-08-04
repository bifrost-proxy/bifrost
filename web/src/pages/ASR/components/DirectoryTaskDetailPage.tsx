import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Descriptions,
  Dropdown,
  Empty,
  message,
  Popconfirm,
  Progress,
  Segmented,
  Space,
  Table,
  Tabs,
  Tag,
  Tooltip,
  Typography,
  theme,
} from "antd";
import {
  ArrowLeftOutlined,
  DeleteOutlined,
  FileZipOutlined,
  LoadingOutlined,
  PauseCircleOutlined,
  PlayCircleOutlined,
  ReloadOutlined,
  StopOutlined,
  WarningOutlined,
} from "@ant-design/icons";
import type {
  AsrDirectoryTaskDetail,
  AsrDailyAgentReportDetail,
  AsrPauseMode,
  AsrTaskDailyDocument,
  AsrTaskDailyDocumentDetail,
  AsrTaskFileRecord,
  AsrTranscriptTimeline,
} from "../../../api/asr";
import {
  cleanupAsrSourceAudio,
  cancelAsrSourceAudioCompression,
  retryAllFailedChunks,
  retryFailedChunks,
  startAsrSourceAudioCompression,
  triggerDailyAgentRun,
} from "../../../api/asr";
import {
  fileStatusColor,
  fileStatusLabel,
  formatDuration,
  formatSchedule,
  formatTime,
} from "../asrUtils";
import TaskFileTranscriptPage from "./TaskFileTranscriptPage";
import DailyAgentTab, { DailyAgentRecordsTab } from "./DailyAgentTab";
import MarkdownContent from "./MarkdownContent";
import {
  isDirectoryTaskDetailTabKey,
  type DirectoryTaskDetailTabKey,
} from "./taskDetailRoute";

const { Text } = Typography;
const DEFAULT_TASK_FILE_PAGE_SIZE = 8;
const TASK_FILE_PAGE_SIZE_OPTIONS = ["8", "10", "20", "50", "100"];
const TASK_FILE_TABLE_SCROLL_X = 1750;
const DAILY_DOCUMENT_PAGE_SIZE = 8;
const DAILY_DOCUMENT_TABLE_SCROLL_X = 900;

function pauseStatusLabel(task: AsrDirectoryTaskDetail): string {
  if (!task.paused) {
    return task.summary.running ? "Running" : task.last_error ? "Error" : "Ready";
  }
  if (task.summary.running) {
    return "Pausing";
  }
  return task.next_run_at_ms ? "Paused until schedule" : "Paused";
}

type FileStatusFilter = "processing" | "pending" | "completed" | "failed" | "all";

function recordedSortValue(record: AsrTaskFileRecord): number {
  return record.source_created_at_ms ?? record.source_modified_ms ?? 0;
}

function compareTaskFilesByRecordedDesc(a: AsrTaskFileRecord, b: AsrTaskFileRecord): number {
  const timeOrder = recordedSortValue(b) - recordedSortValue(a);
  if (timeOrder !== 0) {
    return timeOrder;
  }
  return a.key.localeCompare(b.key);
}

function fileElapsedMs(record: AsrTaskFileRecord, nowMs: number): number | undefined {
  if (!record.started_at_ms) {
    return undefined;
  }
  if (record.finished_at_ms) {
    return record.finished_at_ms - record.started_at_ms;
  }
  if (record.status === "processing") {
    return nowMs - record.started_at_ms;
  }
  return undefined;
}

function formatBytes(value?: number): string {
  if (value == null) {
    return "-";
  }
  if (value < 1024) {
    return `${value} B`;
  }
  const units = ["KB", "MB", "GB"];
  let size = value / 1024;
  let unit = units[0];
  for (let index = 1; index < units.length && size >= 1024; index += 1) {
    size /= 1024;
    unit = units[index];
  }
  return `${size.toFixed(size >= 10 ? 1 : 2)} ${unit}`;
}

function matchesFileStatusFilter(
  record: AsrTaskFileRecord,
  filter: FileStatusFilter,
): boolean {
  if (filter === "all") {
    return true;
  }
  if (filter === "failed") {
    return record.status === "failed" || record.status === "partial_success";
  }
  if (filter === "completed") {
    return record.status === "success";
  }
  return record.status === filter;
}

interface DirectoryTaskDetailPageProps {
  token: ReturnType<typeof theme.useToken>["token"];
  taskDetail: AsrDirectoryTaskDetail | null;
  taskDetailLoading: boolean;
  selectedFileKey: string | null;
  selectedDailyDate: string | null;
  selectedDailyAgentReportDate: string | null;
  selectedTaskTab: DirectoryTaskDetailTabKey;
  taskTimeline: AsrTranscriptTimeline | null;
  taskTimelineLoading: boolean;
  taskDailyDocument: AsrTaskDailyDocumentDetail | null;
  taskDailyDocumentLoading: boolean;
  taskDailyAgentReport: AsrDailyAgentReportDetail | null;
  taskDailyAgentReportLoading: boolean;
  onBackToTasks: () => void;
  onBackToTaskFiles: () => void;
  onBackToDailyDocuments: () => void;
  onBackToDailyAgentReports: () => void;
  onRefreshTask: (id: string) => void;
  onRunTask: (id: string) => void;
  onPauseTask: (id: string, force?: boolean, mode?: AsrPauseMode) => void;
  onResumeTask: (id: string) => void;
  onOpenFile: (file: AsrTaskFileRecord) => void;
  onOpenDailyDocument: (date: string) => void;
  onOpenDailyAgentReport: (date: string, agentId?: string) => void;
  onChangeTaskTab: (tab: DirectoryTaskDetailTabKey) => void;
}

export default function DirectoryTaskDetailPage({
  token,
  taskDetail,
  taskDetailLoading,
  selectedFileKey,
  selectedDailyDate,
  selectedDailyAgentReportDate,
  selectedTaskTab,
  taskTimeline,
  taskTimelineLoading,
  taskDailyDocument,
  taskDailyDocumentLoading,
  taskDailyAgentReport,
  taskDailyAgentReportLoading,
  onBackToTasks,
  onBackToTaskFiles,
  onBackToDailyDocuments,
  onBackToDailyAgentReports,
  onRefreshTask,
  onRunTask,
  onPauseTask,
  onResumeTask,
  onOpenFile,
  onOpenDailyDocument,
  onOpenDailyAgentReport,
  onChangeTaskTab,
}: DirectoryTaskDetailPageProps) {
  const [retryingFileKey, setRetryingFileKey] = useState<string | null>(null);
  const [startingBulkRetry, setStartingBulkRetry] = useState(false);
  const [cleaningSourceAudio, setCleaningSourceAudio] = useState(false);
  const [changingSourceCompression, setChangingSourceCompression] = useState(false);
  const [dailyAgentRunDate, setDailyAgentRunDate] = useState<string | null>(null);
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [fileStatusFilter, setFileStatusFilter] = useState<FileStatusFilter>("all");
  const [fileTablePagination, setFileTablePagination] = useState({
    current: 1,
    pageSize: DEFAULT_TASK_FILE_PAGE_SIZE,
  });

  useEffect(() => {
    setFileTablePagination({
      current: 1,
      pageSize: DEFAULT_TASK_FILE_PAGE_SIZE,
    });
  }, [taskDetail?.id]);

  useEffect(() => {
    setFileTablePagination((previous) => ({
      ...previous,
      current: 1,
    }));
  }, [fileStatusFilter]);

  useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  const selectedFile = selectedFileKey
    ? taskDetail?.files.find((file) => file.key === selectedFileKey) ?? null
    : null;
  const dailyDocuments = taskDetail?.daily_documents ?? [];
  const dailyDocumentsUnavailable =
    Boolean(taskDetail) && !Object.prototype.hasOwnProperty.call(taskDetail, "daily_documents");
  const fileStatusCounts = useMemo(() => {
    const files = taskDetail?.files ?? [];
    return files.reduce(
      (counts, file) => {
        counts.all += 1;
        if (file.status === "processing") {
          counts.processing += 1;
        }
        if (file.status === "pending") {
          counts.pending += 1;
        }
        if (file.status === "success") {
          counts.completed += 1;
        }
        if (file.status === "failed" || file.status === "partial_success") {
          counts.failed += 1;
        }
        return counts;
      },
      { processing: 0, pending: 0, completed: 0, failed: 0, all: 0 },
    );
  }, [taskDetail?.files]);
  const filteredTaskFiles = useMemo(
    () =>
      (taskDetail?.files ?? [])
        .filter((file) => matchesFileStatusFilter(file, fileStatusFilter))
        .sort(compareTaskFilesByRecordedDesc),
    [fileStatusFilter, taskDetail?.files],
  );

  const handleRetryChunks = useCallback(
    async (fileKey: string) => {
      if (!taskDetail) return;
      setRetryingFileKey(fileKey);
      try {
        const result = await retryFailedChunks(taskDetail.id, fileKey);
        if (result.still_failed > 0) {
          message.warning(
            `Recovered ${result.recovered} chunks, ${result.still_failed} still failed`,
          );
        } else {
          message.success(`All ${result.recovered} failed chunks recovered successfully`);
        }
        onRefreshTask(taskDetail.id);
      } catch (error) {
        message.error(error instanceof Error ? error.message : "Failed to retry chunks");
      } finally {
        setRetryingFileKey(null);
      }
    },
    [taskDetail, onRefreshTask],
  );

  const handleRetryAllFailedChunks = useCallback(async () => {
    if (!taskDetail) return;
    setStartingBulkRetry(true);
    try {
      const result = await retryAllFailedChunks(taskDetail.id);
      message.info(result.message);
      onRefreshTask(taskDetail.id);
    } catch (error) {
      message.error(
        error instanceof Error ? error.message : "Failed to retry failed chunks",
      );
    } finally {
      setStartingBulkRetry(false);
    }
  }, [taskDetail, onRefreshTask]);

  const handleCleanupSourceAudio = useCallback(async () => {
    if (!taskDetail) return;
    setCleaningSourceAudio(true);
    try {
      const result = await cleanupAsrSourceAudio(taskDetail.id);
      if (result.failed_files.length > 0) {
        message.warning(
          `Deleted ${result.deleted_files} source file(s), ${result.failed_files.length} failed`,
        );
      } else {
        message.success(
          `Deleted ${result.deleted_files} source file(s), freed ${formatBytes(
            result.deleted_bytes,
          )}`,
        );
      }
      onRefreshTask(taskDetail.id);
    } catch (error) {
      message.error(
        error instanceof Error ? error.message : "Failed to clean source audio",
      );
    } finally {
      setCleaningSourceAudio(false);
    }
  }, [taskDetail, onRefreshTask]);

  const handleStartSourceCompression = useCallback(async () => {
    if (!taskDetail) return;
    setChangingSourceCompression(true);
    try {
      const result = await startAsrSourceAudioCompression(taskDetail.id);
      message.info(result.compression?.message || "Source-audio compression started");
      onRefreshTask(taskDetail.id);
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to start compression");
    } finally {
      setChangingSourceCompression(false);
    }
  }, [taskDetail, onRefreshTask]);

  const handleCancelSourceCompression = useCallback(async () => {
    if (!taskDetail) return;
    setChangingSourceCompression(true);
    try {
      const result = await cancelAsrSourceAudioCompression(taskDetail.id);
      message.info(result.compression?.message || "Compression cancellation requested");
      onRefreshTask(taskDetail.id);
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to cancel compression");
    } finally {
      setChangingSourceCompression(false);
    }
  }, [taskDetail, onRefreshTask]);

  const dailyAgentRunMenuItems = useMemo(() => {
    const agents = taskDetail?.daily_agent?.agents || [];
    return [
      {
        key: "__all__",
        label: "Run All Agents",
      },
      ...agents.map((agent) => ({
        key: agent.id,
        label: `Run ${agent.name || agent.id}`,
        disabled: !agent.enabled,
      })),
    ];
  }, [taskDetail?.daily_agent?.agents]);

  const handleRunDailyAgentForDocument = useCallback(
    async (date: string, agentId?: string) => {
      if (!taskDetail || dailyAgentRunDate) return;
      setDailyAgentRunDate(date);
      try {
        const result = await triggerDailyAgentRun(taskDetail.id, {
          force: true,
          date,
          agentId,
        });
        if (result.status === "already_running") {
          message.warning(result.message || "A Daily Agent run is already in progress");
          return;
        }
        message.success(
          result.message ||
            (agentId
              ? `Daily Agent ${agentId} run queued for ${date}`
              : `All Daily Agents queued for ${date}`),
        );
      } catch (error) {
        message.error(
          `Daily Agent run failed: ${error instanceof Error ? error.message : String(error)}`,
        );
      } finally {
        setDailyAgentRunDate(null);
      }
    },
    [dailyAgentRunDate, taskDetail],
  );

  if (selectedFileKey) {
    return (
      <div
        className="asr-task-detail-page"
        data-testid="asr-task-detail-page"
      >
        <div className="asr-task-detail-standalone-scroll">
          <TaskFileTranscriptPage
            key={selectedFileKey}
            token={token}
            file={selectedFile}
            timeline={taskTimeline}
            loading={taskTimelineLoading}
            onBack={onBackToTaskFiles}
          />
        </div>
      </div>
    );
  }

  if (selectedDailyDate) {
    return (
      <div
        className="asr-task-detail-page"
        data-testid="asr-task-daily-document-page"
      >
        <Card
          className="asr-task-detail-card"
          title={
            <Space>
              <Button
                icon={<ArrowLeftOutlined />}
                onClick={onBackToDailyDocuments}
                aria-label="Back to daily docs"
              />
              <span>{selectedDailyDate}</span>
            </Space>
          }
          loading={taskDailyDocumentLoading}
        >
          {taskDailyDocument ? (
            <Space direction="vertical" size={12} style={{ width: "100%", minWidth: 0 }}>
              <div
                data-testid="asr-daily-document-meta"
                style={{ width: "100%", minWidth: 0, overflowX: "auto", overflowY: "hidden" }}
              >
                <Descriptions size="small" bordered column={2} style={{ minWidth: 720 }}>
                  <Descriptions.Item label="Task">
                    {taskDailyDocument.task_name}
                  </Descriptions.Item>
                  <Descriptions.Item label="Date">{taskDailyDocument.date}</Descriptions.Item>
                  <Descriptions.Item label="Path" span={2}>
                    <Text
                      style={{ display: "inline-block", fontSize: 12, whiteSpace: "nowrap" }}
                      ellipsis={{ tooltip: taskDailyDocument.path }}
                    >
                      {taskDailyDocument.path}
                    </Text>
                  </Descriptions.Item>
                  <Descriptions.Item label="Size">
                    {formatBytes(taskDailyDocument.size)}
                  </Descriptions.Item>
                  <Descriptions.Item label="Modified">
                    {formatTime(taskDailyDocument.modified_ms)}
                  </Descriptions.Item>
                </Descriptions>
              </div>
              <pre
                data-testid="asr-daily-document-content"
                style={{
                  margin: 0,
                  minHeight: 360,
                  padding: 16,
                  border: `1px solid ${token.colorBorderSecondary}`,
                  borderRadius: 6,
                  background: token.colorFillQuaternary,
                  color: token.colorText,
                  fontSize: 13,
                  lineHeight: 1.7,
                  whiteSpace: "pre-wrap",
                  wordBreak: "break-word",
                }}
              >
                {taskDailyDocument.content}
              </pre>
            </Space>
          ) : (
            <Text type="secondary">
              {taskDailyDocumentLoading
                ? "Loading daily document..."
                : "Daily document not found."}
            </Text>
          )}
        </Card>
      </div>
    );
  }

  if (selectedDailyAgentReportDate) {
    return (
      <div
        className="asr-task-detail-page"
        data-testid="asr-daily-agent-report-page"
      >
        <Card
          className="asr-task-detail-card"
          title={
            <Space>
              <Button
                icon={<ArrowLeftOutlined />}
                onClick={onBackToDailyAgentReports}
                aria-label="Back to Daily Agent reports"
              />
              <span>{selectedDailyAgentReportDate}-report.md</span>
            </Space>
          }
          loading={taskDailyAgentReportLoading}
        >
          {taskDailyAgentReport ? (
            <Space direction="vertical" size={12} style={{ width: "100%", minWidth: 0 }}>
              <div
                data-testid="asr-daily-agent-report-meta"
                style={{ width: "100%", minWidth: 0, overflowX: "auto", overflowY: "hidden" }}
              >
                <Descriptions size="small" bordered column={2} style={{ minWidth: 900 }}>
                  <Descriptions.Item label="Task">
                    {taskDailyAgentReport.task_name}
                  </Descriptions.Item>
                  <Descriptions.Item label="Date">
                    {taskDailyAgentReport.date}
                  </Descriptions.Item>
                  <Descriptions.Item label="Agent">
                    <Tag>
                      {taskDailyAgentReport.agent_name ||
                        taskDailyAgentReport.agent_id ||
                        "daily_report"}
                    </Tag>
                  </Descriptions.Item>
                  <Descriptions.Item label="Output Dir">
                    <Text code>{taskDailyAgentReport.output_dir || "report"}</Text>
                  </Descriptions.Item>
                  <Descriptions.Item label="Path" span={2}>
                    <Text
                      style={{ display: "inline-block", fontSize: 12, whiteSpace: "nowrap" }}
                      ellipsis={{ tooltip: taskDailyAgentReport.path }}
                    >
                      {taskDailyAgentReport.path}
                    </Text>
                  </Descriptions.Item>
                  <Descriptions.Item label="Size">
                    {formatBytes(taskDailyAgentReport.size)}
                  </Descriptions.Item>
                  <Descriptions.Item label="Modified">
                    {formatTime(taskDailyAgentReport.modified_ms)}
                  </Descriptions.Item>
                  <Descriptions.Item label="Processed At">
                    {formatTime(taskDailyAgentReport.processed_at_ms)}
                  </Descriptions.Item>
                  <Descriptions.Item label="Runner">
                    {taskDailyAgentReport.runner ? (
                      <Tag>{taskDailyAgentReport.runner}</Tag>
                    ) : (
                      "-"
                    )}
                  </Descriptions.Item>
                </Descriptions>
              </div>
              <MarkdownContent
                token={token}
                content={taskDailyAgentReport.content}
                testId="asr-daily-agent-report-content"
              />
            </Space>
          ) : (
            <Text type="secondary">
              {taskDailyAgentReportLoading
                ? "Loading Daily Agent report..."
                : "Daily Agent report not found."}
            </Text>
          )}
        </Card>
      </div>
    );
  }

  const summary = taskDetail?.summary;
  const bulkRetry = taskDetail?.bulk_retry;
  const bulkRetryActive =
    bulkRetry?.status === "queued" || bulkRetry?.status === "running";
  const sourceCompression = taskDetail?.source_compression;
  const sourceCompressionActive =
    sourceCompression?.status === "queued" ||
    sourceCompression?.status === "running" ||
    sourceCompression?.status === "cancelling";
  const compressibleSourceFileCount = summary?.compressible_source_file_count ?? 0;
  const compressibleSourceBytes = summary?.compressible_source_bytes ?? 0;
  const sourceCompressionPercent =
    sourceCompression && sourceCompression.queued_files > 0
      ? Math.round(
          (sourceCompression.processed_files / sourceCompression.queued_files) * 100,
        )
      : 0;
  const failedChunkCount = summary?.failed_chunk_count ?? 0;
  const cleanableSourceFileCount = summary?.cleanable_source_file_count ?? 0;
  const cleanableSourceBytes = summary?.cleanable_source_bytes ?? 0;
  const bulkRetryPercent =
    bulkRetry && bulkRetry.queued_files > 0
      ? Math.round((bulkRetry.processed_files / bulkRetry.queued_files) * 100)
      : 0;
  const overviewTabContent = taskDetail ? (
    <Space direction="vertical" size={16} style={{ width: "100%" }}>
      <Descriptions size="small" bordered column={2}>
        <Descriptions.Item label="Directory" span={2}>
          {taskDetail.audio_dir}
        </Descriptions.Item>
        <Descriptions.Item label="Schedule">
          {taskDetail.enabled ? formatSchedule(taskDetail.schedule) : "Disabled"}
        </Descriptions.Item>
        <Descriptions.Item label="Run State">
          <Tag
            color={
              taskDetail.paused
                ? "warning"
                : taskDetail.summary.running
                  ? "processing"
                  : "success"
            }
          >
            {pauseStatusLabel(taskDetail)}
          </Tag>
        </Descriptions.Item>
        <Descriptions.Item label="Next Run">
          {formatTime(taskDetail.next_run_at_ms)}
        </Descriptions.Item>
        <Descriptions.Item label="Last Run">
          {formatTime(taskDetail.last_run_at_ms)}
        </Descriptions.Item>
        <Descriptions.Item label="Mode">
          {taskDetail.recursive ? "Recursive" : "Flat"}
        </Descriptions.Item>
        <Descriptions.Item label="Runtime">
          {taskDetail.runtime_strategy}
        </Descriptions.Item>
        <Descriptions.Item label="Speaker Diarization">
          {taskDetail.diarization?.enabled ? (
            <Space wrap>
              <Tag color={taskDetail.summary.diarization_ready ? "purple" : "warning"}>
                {taskDetail.summary.diarization_ready ? "Ready" : "Needs setup"}
              </Tag>
              <Text>{taskDetail.diarization.profile}</Text>
            </Space>
          ) : (
            "Disabled"
          )}
        </Descriptions.Item>
        <Descriptions.Item label="Speakers">
          {taskDetail.summary.diarized_files ?? 0} files,{" "}
          {taskDetail.summary.speaker_count ?? 0} speakers
        </Descriptions.Item>
        <Descriptions.Item label="Processed">
          {taskDetail.summary.processed}
        </Descriptions.Item>
        <Descriptions.Item label="Pending">
          {taskDetail.summary.pending}
        </Descriptions.Item>
        <Descriptions.Item label="Failed">{taskDetail.summary.failed}</Descriptions.Item>
        <Descriptions.Item label="Audio Files">
          <Space>
            <Text>{formatBytes(taskDetail.summary.audio_source_bytes)}</Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              ({taskDetail.summary.audio_source_file_count} files)
            </Text>
          </Space>
        </Descriptions.Item>
        <Descriptions.Item label="Cleanable Originals">
          <Space>
            <Text>{formatBytes(cleanableSourceBytes)}</Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              ({cleanableSourceFileCount} files)
            </Text>
          </Space>
        </Descriptions.Item>
        <Descriptions.Item label="Compressible WAV">
          <Space>
            <Text>{formatBytes(compressibleSourceBytes)}</Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              ({compressibleSourceFileCount} files)
            </Text>
          </Space>
        </Descriptions.Item>
        <Descriptions.Item label="Compression Saved">
          <Space>
            <Text>{formatBytes(summary?.compression_saved_bytes ?? 0)}</Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              ({summary?.compressed_source_file_count ?? 0} files)
            </Text>
          </Space>
        </Descriptions.Item>
        {summary && summary.partial_success > 0 ? (
          <Descriptions.Item label="Partial Success">
            <Space>
              <Tag color="warning">{summary.partial_success}</Tag>
              <Text type="secondary" style={{ fontSize: 12 }}>
                ({summary.failed_chunk_count} failed chunks)
              </Text>
            </Space>
          </Descriptions.Item>
        ) : (
          <Descriptions.Item label="Deleted After Processing">
            {taskDetail.summary.deleted_after_processing}
          </Descriptions.Item>
        )}
        <Descriptions.Item label="Last Error" span={2}>
          {taskDetail.last_error || "-"}
        </Descriptions.Item>
      </Descriptions>
      {bulkRetry ? (
        <Alert
          data-testid="asr-bulk-retry-status"
          type={
            bulkRetry.status === "failed"
              ? "warning"
              : bulkRetry.status === "completed"
                ? "success"
                : "info"
          }
          showIcon
          message={
            <Space wrap>
              <Text strong>Bulk chunk retry</Text>
              <Tag
                color={
                  bulkRetry.status === "failed"
                    ? "warning"
                    : bulkRetry.status === "completed"
                      ? "success"
                      : "processing"
                }
              >
                {bulkRetry.status}
              </Tag>
              <Text>
                {bulkRetry.processed_files}/{bulkRetry.queued_files} files
              </Text>
              <Text>
                {bulkRetry.recovered_chunks}/{bulkRetry.total_failed_chunks} chunks recovered
              </Text>
              {bulkRetry.still_failed_chunks > 0 ? (
                <Text type="warning">{bulkRetry.still_failed_chunks} still failed</Text>
              ) : null}
            </Space>
          }
          description={
            <Space direction="vertical" size={6} style={{ width: "100%" }}>
              <Text type="secondary">{bulkRetry.message}</Text>
              {bulkRetry.current_source_path ? (
                <Text
                  style={{ fontSize: 12, width: "100%" }}
                  ellipsis={{ tooltip: bulkRetry.current_source_path }}
                >
                  Current: {bulkRetry.current_source_path}
                </Text>
              ) : null}
              {bulkRetryActive ? (
                <Progress
                  percent={bulkRetryPercent}
                  size="small"
                  status="active"
                  format={() => `${bulkRetry.processed_files}/${bulkRetry.queued_files}`}
                />
              ) : null}
              {bulkRetry.results.length > 0 ? (
                <Text type="secondary" style={{ fontSize: 12 }}>
                  Last file: {bulkRetry.results.at(-1)?.message}
                </Text>
              ) : null}
            </Space>
          }
        />
      ) : null}
      {sourceCompression ? (
        <Alert
          data-testid="asr-source-compression-status"
          type={
            sourceCompression.status === "completed"
              ? "success"
              : sourceCompression.status === "completed_with_errors" ||
                  sourceCompression.status === "failed" ||
                  sourceCompression.status === "interrupted"
                ? "warning"
                : "info"
          }
          showIcon
          message={
            <Space wrap>
              <Text strong>Lossless source compression</Text>
              <Tag color={sourceCompression.status === "completed" ? "success" : "processing"}>
                {sourceCompression.status}
              </Tag>
              <Text>
                {sourceCompression.processed_files}/{sourceCompression.queued_files} files
              </Text>
              <Text>Saved {formatBytes(sourceCompression.saved_bytes)}</Text>
            </Space>
          }
          description={
            <Space direction="vertical" size={6} style={{ width: "100%" }}>
              <Text type="secondary">{sourceCompression.message}</Text>
              {sourceCompression.current_source_path ? (
                <Text ellipsis={{ tooltip: sourceCompression.current_source_path }}>
                  Current: {sourceCompression.current_source_path}
                </Text>
              ) : null}
              {sourceCompressionActive ? (
                <Progress percent={sourceCompressionPercent} size="small" status="active" />
              ) : null}
            </Space>
          }
        />
      ) : null}
    </Space>
  ) : null;

  return (
    <div className="asr-task-detail-page" data-testid="asr-task-detail-page">
      <Card
        className="asr-task-detail-card asr-task-detail-card--tabs"
        title={
          <Space>
            <Button
              icon={<ArrowLeftOutlined />}
              onClick={onBackToTasks}
              aria-label="Back to directory tasks"
            />
            <span>{taskDetail ? `Directory Task: ${taskDetail.name}` : "Directory Task"}</span>
          </Space>
        }
        extra={
          taskDetail ? (
            <Space>
              <Button size="small" onClick={() => onRefreshTask(taskDetail.id)}>
                Refresh
              </Button>
              {sourceCompressionActive ? (
                <Button
                  size="small"
                  danger
                  icon={<StopOutlined />}
                  loading={changingSourceCompression}
                  disabled={sourceCompression?.status === "cancelling"}
                  onClick={handleCancelSourceCompression}
                >
                  {sourceCompression?.status === "cancelling"
                    ? "Cancelling compression..."
                    : "Cancel compression"}
                </Button>
              ) : (
                <Popconfirm
                  title={`Losslessly compress ${compressibleSourceFileCount} completed WAV file${compressibleSourceFileCount === 1 ? "" : "s"}?`}
                  description="Each WAV is replaced with FLAC only after decoded PCM verification succeeds. Transcripts are kept and partial/failed files are skipped."
                  disabled={
                    compressibleSourceFileCount === 0 ||
                    changingSourceCompression ||
                    taskDetail.summary.running ||
                    bulkRetryActive
                  }
                  onConfirm={handleStartSourceCompression}
                >
                  <Button
                    size="small"
                    icon={<FileZipOutlined />}
                    loading={changingSourceCompression}
                    disabled={
                      compressibleSourceFileCount === 0 ||
                      taskDetail.summary.running ||
                      bulkRetryActive
                    }
                  >
                    Compress WAVs
                  </Button>
                </Popconfirm>
              )}
              <Popconfirm
                title={`Delete ${cleanableSourceFileCount} completed source audio file${cleanableSourceFileCount === 1 ? "" : "s"}?`}
                description="Transcript and timeline outputs are kept. Partial-success files are not deleted so failed chunks can still be retried."
                disabled={
                  cleanableSourceFileCount === 0 ||
                  cleaningSourceAudio ||
                  taskDetail.summary.running ||
                  bulkRetryActive ||
                  sourceCompressionActive
                }
                onConfirm={handleCleanupSourceAudio}
              >
                <Button
                  size="small"
                  danger
                  icon={cleaningSourceAudio ? <LoadingOutlined /> : <DeleteOutlined />}
                  disabled={
                    cleanableSourceFileCount === 0 ||
                    cleaningSourceAudio ||
                    taskDetail.summary.running ||
                    bulkRetryActive ||
                    sourceCompressionActive
                  }
                >
                  Clean originals
                </Button>
              </Popconfirm>
              <Popconfirm
                title={`Retry all ${failedChunkCount} failed chunk${failedChunkCount > 1 ? "s" : ""}?`}
                description="Files are queued and retried one at a time."
                disabled={
                  failedChunkCount === 0 ||
                  bulkRetryActive ||
                  startingBulkRetry ||
                  sourceCompressionActive
                }
                onConfirm={handleRetryAllFailedChunks}
              >
                <Button
                  size="small"
                  icon={
                    startingBulkRetry || bulkRetryActive ? (
                      <LoadingOutlined />
                    ) : (
                      <ReloadOutlined />
                    )
                  }
                  disabled={
                    failedChunkCount === 0 ||
                    bulkRetryActive ||
                    startingBulkRetry ||
                    sourceCompressionActive
                  }
                >
                  {bulkRetryActive ? "Retrying..." : "Retry all failed chunks"}
                </Button>
              </Popconfirm>
              <Button
                size="small"
                type="primary"
                disabled={
                  taskDetail.summary.running ||
                  Boolean(taskDetail.paused) ||
                  sourceCompressionActive
                }
                icon={taskDetail.summary.running ? <LoadingOutlined /> : undefined}
                onClick={() => onRunTask(taskDetail.id)}
              >
                {taskDetail.summary.running ? "Running..." : "Run"}
              </Button>
              {taskDetail.paused ? (
                <Button
                  size="small"
                  icon={<PlayCircleOutlined />}
                  onClick={() => onResumeTask(taskDetail.id)}
                >
                  Resume
                </Button>
              ) : (
                <Dropdown
                  trigger={["click"]}
                  menu={{
                    items: [
                      {
                        key: "temporary",
                        label: "Pause until next schedule",
                        icon: <PauseCircleOutlined />,
                      },
                      {
                        key: "long_term",
                        label: "Pause indefinitely",
                        icon: <StopOutlined />,
                      },
                    ],
                    onClick: ({ key }) =>
                      onPauseTask(taskDetail.id, false, key as AsrPauseMode),
                  }}
                >
                  <Button size="small" icon={<PauseCircleOutlined />}>
                    Pause
                  </Button>
                </Dropdown>
              )}
              {taskDetail.summary.running && !taskDetail.paused ? (
                <Popconfirm
                  title="Force pause this ASR task?"
                  description="The current native ASR process will be terminated and the file will resume from pending later."
                  onConfirm={() => onPauseTask(taskDetail.id, true, "long_term")}
                >
                  <Button size="small" danger icon={<StopOutlined />}>
                    Force Pause
                  </Button>
                </Popconfirm>
              ) : null}
            </Space>
          ) : null
        }
      >
        {taskDetail ? (
          <Space
            className="asr-task-detail-content"
            direction="vertical"
            size={16}
          >
            <Tabs
              className="asr-task-detail-tabs"
              activeKey={selectedTaskTab}
              onChange={(key) => {
                if (isDirectoryTaskDetailTabKey(key)) {
                  onChangeTaskTab(key);
                }
              }}
              items={[
                {
                  key: "overview",
                  label: "Overview",
                  children: <div data-testid="asr-task-overview-tab">{overviewTabContent}</div>,
                },
                {
                  key: "files",
                  label: `Files (${taskDetail.files.length})`,
                  children: (
                    <div data-testid="asr-task-files-table">
                      <Space
                        style={{
                          width: "100%",
                          justifyContent: "space-between",
                          marginBottom: 12,
                        }}
                        wrap
                      >
                        <Segmented<FileStatusFilter>
                          size="small"
                          value={fileStatusFilter}
                          onChange={(value) => setFileStatusFilter(value)}
                          options={[
                            {
                              label: `Processing (${fileStatusCounts.processing})`,
                              value: "processing",
                            },
                            { label: `Pending (${fileStatusCounts.pending})`, value: "pending" },
                            {
                              label: `Completed (${fileStatusCounts.completed})`,
                              value: "completed",
                            },
                            { label: `Failed (${fileStatusCounts.failed})`, value: "failed" },
                            { label: `All (${fileStatusCounts.all})`, value: "all" },
                          ]}
                        />
                        <Text type="secondary" style={{ fontSize: 12 }}>
                          Showing {filteredTaskFiles.length} of {fileStatusCounts.all} files
                        </Text>
                      </Space>
                      <Table<AsrTaskFileRecord>
                rowKey="key"
                size="small"
                tableLayout="fixed"
                loading={taskDetailLoading}
                dataSource={filteredTaskFiles}
                scroll={{ x: TASK_FILE_TABLE_SCROLL_X }}
                pagination={{
                  current: fileTablePagination.current,
                  pageSize: fileTablePagination.pageSize,
                  pageSizeOptions: TASK_FILE_PAGE_SIZE_OPTIONS,
                  showSizeChanger: true,
                  total: filteredTaskFiles.length,
                  onChange: (current, pageSize) =>
                    setFileTablePagination({ current, pageSize }),
                  onShowSizeChange: (current, pageSize) =>
                    setFileTablePagination({ current, pageSize }),
                }}
                columns={[
                  {
                    title: "File",
                    dataIndex: "source_path",
                    width: 360,
                    render: (value, record) => (
                      <Space direction="vertical" size={2} style={{ width: 320 }}>
                        <Text style={{ width: 320, fontSize: 12 }} ellipsis={{ tooltip: value }}>
                          {value}
                        </Text>
                        {record.output_timeline_path ? (
                          <Button
                            type="link"
                            size="small"
                            style={{ padding: 0, height: "auto", alignSelf: "flex-start" }}
                            onClick={() => onOpenFile(record)}
                          >
                            Open transcript
                          </Button>
                        ) : null}
                      </Space>
                    ),
                  },
                  {
                    title: "Status",
                    dataIndex: "status",
                    width: 180,
                    render: (value, record) => {
                      const failedChunks = record.failed_chunks ?? [];
                      const tag = (
                        <Tag color={fileStatusColor(value)}>{fileStatusLabel(value)}</Tag>
                      );
                      if (
                        value === "processing" &&
                        record.progress_total != null &&
                        record.progress_total > 0
                      ) {
                        const pct = Math.round(
                          ((record.progress_current ?? 0) / record.progress_total) * 100,
                        );
                        return (
                          <Space direction="vertical" size={2} style={{ width: "100%" }}>
                            {tag}
                            <Progress
                              percent={pct}
                              size="small"
                              status="active"
                              format={() =>
                                `${record.progress_current ?? 0}/${record.progress_total}`
                              }
                            />
                          </Space>
                        );
                      }
                      if (value === "partial_success" && failedChunks.length > 0) {
                        return (
                          <Space direction="vertical" size={4} style={{ width: "100%" }}>
                            {tag}
                            <Tooltip
                              title={
                                <div>
                                  {failedChunks.map((fc) => (
                                    <div key={fc.chunk_index} style={{ fontSize: 11 }}>
                                      Chunk {fc.chunk_index}: {fc.offset_secs}s–
                                      {fc.offset_secs + fc.duration_secs}s ({fc.attempts} attempts)
                                      <br />
                                      <span style={{ color: "#ff7875" }}>{fc.error}</span>
                                    </div>
                                  ))}
                                </div>
                              }
                            >
                              <Text type="warning" style={{ fontSize: 12, cursor: "help" }}>
                                <WarningOutlined /> {failedChunks.length} failed chunk
                                {failedChunks.length > 1 ? "s" : ""}
                              </Text>
                            </Tooltip>
                            <Popconfirm
                              title={`Retry ${failedChunks.length} failed chunk${failedChunks.length > 1 ? "s" : ""}?`}
                              description="This will re-transcribe only the failed segments."
                              onConfirm={() => handleRetryChunks(record.key)}
                            >
                              <Button
                                type="link"
                                size="small"
                                icon={
                                  retryingFileKey === record.key ? (
                                    <LoadingOutlined />
                                  ) : (
                                    <ReloadOutlined />
                                  )
                                }
                                disabled={retryingFileKey !== null}
                                style={{ padding: 0, height: "auto" }}
                              >
                                Retry chunks
                              </Button>
                            </Popconfirm>
                          </Space>
                        );
                      }
                      return tag;
                    },
                  },
                  {
                    title: "Diarization",
                    width: 150,
                    render: (_value, record) =>
                      record.diarization_status ? (
                        <Space direction="vertical" size={2}>
                          <Tag color={record.diarization_status === "success" ? "purple" : "default"}>
                            {record.diarization_status}
                          </Tag>
                          {record.speaker_count ? (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              {record.speaker_count} speakers
                            </Text>
                          ) : null}
                        </Space>
                      ) : (
                        <Text type="secondary">-</Text>
                      ),
                  },
                  {
                    title: "Text",
                    dataIndex: "text_chars",
                    width: 90,
                    render: (value) => `${value} chars`,
                  },
                  {
                    title: "Runtime",
                    width: 180,
                    render: (_value, record) => {
                      const metrics = record.chunk_metrics ?? [];
                      const last = metrics.at(-1);
                      return (
                        <Space direction="vertical" size={0}>
                          <Tag>{record.runtime_strategy ?? taskDetail.runtime_strategy}</Tag>
                          {last ? (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              {metrics.length} metrics, last {last.runner} RTF{" "}
                              {last.rtf.toFixed(3)}
                            </Text>
                          ) : null}
                          {record.fallback_reason ? (
                            <Text type="warning" style={{ fontSize: 11 }}>
                              {record.fallback_reason}
                            </Text>
                          ) : null}
                        </Space>
                      );
                    },
                  },
                  {
                    title: "Recorded",
                    dataIndex: "source_created_at_ms",
                    width: 190,
                    render: (value, record) => (
                      <Space direction="vertical" size={0}>
                        <Text style={{ fontSize: 12 }}>{formatTime(value)}</Text>
                        {record.source_created_at_source ? (
                          <Text type="secondary" style={{ fontSize: 11 }}>
                            {record.source_created_at_source}
                          </Text>
                        ) : null}
                      </Space>
                    ),
                  },
                  {
                    title: "Audio",
                    dataIndex: "media_duration_ms",
                    width: 110,
                    render: (value) => formatDuration(value),
                  },
                  {
                    title: "Source Size",
                    dataIndex: "source_size",
                    width: 110,
                    render: (value) => formatBytes(value),
                  },
                  {
                    title: "Started",
                    dataIndex: "started_at_ms",
                    width: 180,
                    render: (value) => formatTime(value),
                  },
                  {
                    title: "Elapsed",
                    width: 110,
                    render: (_value, record) => formatDuration(fileElapsedMs(record, nowMs)),
                  },
                  {
                    title: "Finished",
                    dataIndex: "finished_at_ms",
                    width: 180,
                    render: (value) => formatTime(value),
                  },
                  {
                    title: "Result",
                    width: 260,
                    render: (_value, record) => (
                      <Space direction="vertical" size={0}>
                        <Text
                          style={{ width: 240, fontSize: 12 }}
                          ellipsis={{ tooltip: record.output_text_path }}
                        >
                          {record.output_text_path || "-"}
                        </Text>
                        {record.error ? (
                          <Text type="danger" style={{ fontSize: 12 }}>
                            {record.error}
                          </Text>
                        ) : null}
                      </Space>
                    ),
                  },
                ]}
                      />
                    </div>
                  ),
                },
                {
                  key: "daily",
                  label: `Daily Docs (${dailyDocuments.length})`,
                  children: (
                    <div data-testid="asr-task-daily-docs-tab">
                      <style>
                        {`
                          .asr-daily-documents-table .ant-table-thead > tr > th {
                            height: 40px !important;
                            padding-top: 8px !important;
                            padding-bottom: 8px !important;
                            vertical-align: middle !important;
                            white-space: nowrap;
                          }
                        `}
                      </style>
                      {dailyDocuments.length > 0 ? (
                        <Table<AsrTaskDailyDocument>
                          className="asr-daily-documents-table"
                          rowKey="date"
                          size="small"
                          tableLayout="fixed"
                          scroll={{ x: DAILY_DOCUMENT_TABLE_SCROLL_X }}
                          dataSource={dailyDocuments}
                          pagination={{
                            pageSize: DAILY_DOCUMENT_PAGE_SIZE,
                            hideOnSinglePage: true,
                          }}
                          columns={[
                            {
                              title: "Date",
                              dataIndex: "date",
                              width: 140,
                              render: (value) => <Text strong>{value}</Text>,
                            },
                            {
                              title: "Document",
                              dataIndex: "path",
                              render: (value) => (
                                <Text style={{ fontSize: 12 }} ellipsis={{ tooltip: value }}>
                                  {value}
                                </Text>
                              ),
                            },
                            {
                              title: "Size",
                              dataIndex: "size",
                              width: 110,
                              render: (value) => formatBytes(value),
                            },
                            {
                              title: "Text",
                              dataIndex: "text_chars",
                              width: 110,
                              render: (value) => `${value} chars`,
                            },
                            {
                              title: "Modified",
                              dataIndex: "modified_ms",
                              width: 180,
                              render: (value) => formatTime(value),
                            },
                            {
                              title: "Action",
                              width: 220,
                              render: (_value, record) => (
                                <Space size={8} wrap>
                                  <Button
                                    type="link"
                                    size="small"
                                    style={{ padding: 0 }}
                                    onClick={() => onOpenDailyDocument(record.date)}
                                  >
                                    Open document
                                  </Button>
                                  <Dropdown.Button
                                    type="link"
                                    size="small"
                                    style={{ padding: 0 }}
                                    loading={dailyAgentRunDate === record.date}
                                    disabled={Boolean(dailyAgentRunDate)}
                                    data-testid={`asr-daily-doc-run-agent-${record.date}`}
                                    menu={{
                                      items: dailyAgentRunMenuItems,
                                      onClick: ({ key }) =>
                                        handleRunDailyAgentForDocument(
                                          record.date,
                                          key === "__all__" ? undefined : key,
                                        ),
                                    }}
                                    onClick={() =>
                                      handleRunDailyAgentForDocument(record.date)
                                    }
                                  >
                                    Run All Agents
                                  </Dropdown.Button>
                                </Space>
                              ),
                            },
                          ]}
                        />
                      ) : dailyDocumentsUnavailable ? (
                        <Empty description="Daily documents are available after the ASR backend is updated" />
                      ) : (
                        <Empty description="No daily documents yet" />
                      )}
                    </div>
                  ),
                },
                {
                  key: "daily-agent",
                  label: "Daily Agent",
                  children: taskDetail ? (
                    <DailyAgentTab taskId={taskDetail.id} />
                  ) : null,
                },
                {
                  key: "daily-agent-records",
                  label: "Daily Agent Records",
                  children: taskDetail ? (
                    <DailyAgentRecordsTab
                      taskId={taskDetail.id}
                      onOpenReport={onOpenDailyAgentReport}
                    />
                  ) : null,
                },
              ]}
            />
          </Space>
        ) : (
          <Text type="secondary">
            {taskDetailLoading
              ? "Loading directory task..."
              : "Select a task to view progress and file results."}
          </Text>
        )}
      </Card>
    </div>
  );
}
