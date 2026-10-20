import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Descriptions,
  Dropdown,
  Empty,
  Input,
  message,
  Modal,
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
import type { MenuProps } from "antd";
import {
  ArrowLeftOutlined,
  DeleteOutlined,
  FileZipOutlined,
  LoadingOutlined,
  MoreOutlined,
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
  AsrSourceAudioCompressionState,
  AsrTaskDailyDocument,
  AsrTaskDailyDocumentDetail,
  AsrTaskFileRecord,
  AsrTranscriptTimeline,
} from "../../../api/asr";
import {
  cleanupAsrSourceAudio,
  cancelAsrSourceAudioCompression,
  retryAllFailedChunks,
  retryAllFailedFiles,
  retryFailedChunks,
  retryFailedFile,
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
import { useI18n, type TFunction } from "../../../i18n";

const { Text } = Typography;
const DEFAULT_TASK_FILE_PAGE_SIZE = 8;
const TASK_FILE_PAGE_SIZE_OPTIONS = ["8", "10", "20", "50", "100"];
const TASK_FILE_TABLE_SCROLL_X = 1920;
const DAILY_DOCUMENT_PAGE_SIZE = 8;
const DAILY_DOCUMENT_TABLE_SCROLL_X = 900;

function pauseStatusLabel(t: TFunction, task: AsrDirectoryTaskDetail): string {
  if (!task.paused) {
    return task.summary.running
      ? t("asrTasks.status.running")
      : task.last_error
        ? t("asrTasks.status.error")
        : t("asrTasks.status.ready");
  }
  if (task.summary.running) {
    return t("asrTasks.status.pausing");
  }
  return task.next_run_at_ms
    ? t("asrTasks.status.pausedUntilSchedule")
    : t("asrTasks.status.paused");
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

function compressionDisplayForFile(
  t: TFunction,
  record: AsrTaskFileRecord,
  compression?: AsrSourceAudioCompressionState,
): { label: string; color?: string; detail?: string } {
  if (record.source_compression) {
    return {
      label: t("asrTasks.compression.compressed"),
      color: "success",
      detail: t("asrTasks.compression.flacSaved", {
        size: formatBytes(record.source_compression.saved_bytes),
      }),
    };
  }
  if (compression?.current_source_path === record.source_path) {
    return {
      label: t("asrTasks.compression.compressing"),
      color: "processing",
      detail: t("asrTasks.compression.currentFile"),
    };
  }
  const result = compression?.results.find(
    (candidate) =>
      candidate.source_path === record.source_path ||
      candidate.compressed_path === record.source_path,
  );
  if (result?.status === "failed") {
    return { label: t("common.failed"), color: "error", detail: result.message };
  }
  if (result?.status === "skipped") {
    return { label: t("asrTasks.compression.skipped"), color: "warning", detail: result.message };
  }
  if (record.source_compression_eligible) {
    return {
      label: t("asrTasks.compression.uncompressed"),
      color: "blue",
      detail: t("asrTasks.compression.readyToCompress"),
    };
  }
  if (record.status !== "success") {
    return {
      label: t("asrTasks.compression.notEligible"),
      detail: t("asrTasks.compression.transcriptionIncomplete"),
    };
  }
  return {
    label: t("asrTasks.compression.notEligible"),
    detail: t("asrTasks.compression.artifactsIncomplete"),
  };
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
  const { t } = useI18n();
  const [retryingFileKey, setRetryingFileKey] = useState<string | null>(null);
  const [startingBulkRetry, setStartingBulkRetry] = useState(false);
  const [retryingFailedFileKey, setRetryingFailedFileKey] = useState<string | null>(null);
  const [startingFailedFilesRetry, setStartingFailedFilesRetry] = useState(false);
  const [cleaningSourceAudio, setCleaningSourceAudio] = useState(false);
  const [cleanupConfirmOpen, setCleanupConfirmOpen] = useState(false);
  const [cleanupConfirmName, setCleanupConfirmName] = useState("");
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
    setCleanupConfirmOpen(false);
    setCleanupConfirmName("");
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
            t("asrTasks.detail.chunksPartiallyRecovered", {
              recovered: result.recovered,
              stillFailed: result.still_failed,
            }),
          );
        } else {
          message.success(t("asrTasks.detail.chunksRecovered", { count: result.recovered }));
        }
        onRefreshTask(taskDetail.id);
      } catch (error) {
        message.error(error instanceof Error ? error.message : t("asrTasks.detail.retryChunksFailed"));
      } finally {
        setRetryingFileKey(null);
      }
    },
    [taskDetail, onRefreshTask, t],
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
        error instanceof Error ? error.message : t("asrTasks.detail.retryFailedChunksFailed"),
      );
    } finally {
      setStartingBulkRetry(false);
    }
  }, [taskDetail, onRefreshTask, t]);

  const handleRetryFailedFile = useCallback(
    async (fileKey: string) => {
      if (!taskDetail) return;
      setRetryingFailedFileKey(fileKey);
      try {
        const result = await retryFailedFile(taskDetail.id, fileKey);
        message.success(result.message);
        onRefreshTask(taskDetail.id);
      } catch (error) {
        message.error(error instanceof Error ? error.message : t("asrTasks.detail.retryTranscriptionFailed"));
      } finally {
        setRetryingFailedFileKey(null);
      }
    },
    [taskDetail, onRefreshTask, t],
  );

  const handleRetryAllFailedFiles = useCallback(async () => {
    if (!taskDetail) return;
    setStartingFailedFilesRetry(true);
    try {
      const result = await retryAllFailedFiles(taskDetail.id);
      if (result.queued > 0) message.success(result.message);
      else message.info(result.message);
      onRefreshTask(taskDetail.id);
    } catch (error) {
      message.error(error instanceof Error ? error.message : t("asrTasks.detail.retryFailedFilesFailed"));
    } finally {
      setStartingFailedFilesRetry(false);
    }
  }, [taskDetail, onRefreshTask, t]);

  const handleCleanupSourceAudio = useCallback(async () => {
    if (!taskDetail || cleanupConfirmName !== taskDetail.name) return;
    setCleaningSourceAudio(true);
    try {
      const result = await cleanupAsrSourceAudio(taskDetail.id, cleanupConfirmName);
      if (result.failed_files.length > 0) {
        message.warning(
          t("asrTasks.detail.cleanupPartial", {
            deleted: result.deleted_files,
            failed: result.failed_files.length,
          }),
        );
      } else {
        message.success(
          t("asrTasks.detail.cleanupSuccess", {
            deleted: result.deleted_files,
            size: formatBytes(result.deleted_bytes),
          }),
        );
      }
      setCleanupConfirmOpen(false);
      setCleanupConfirmName("");
      onRefreshTask(taskDetail.id);
    } catch (error) {
      message.error(
        error instanceof Error ? error.message : t("asrTasks.detail.cleanupFailed"),
      );
    } finally {
      setCleaningSourceAudio(false);
    }
  }, [cleanupConfirmName, taskDetail, onRefreshTask, t]);

  const handleStartSourceCompression = useCallback(async () => {
    if (!taskDetail) return;
    setChangingSourceCompression(true);
    try {
      const result = await startAsrSourceAudioCompression(taskDetail.id);
      message.info(result.compression?.message || t("asrTasks.detail.compressionStarted"));
      onRefreshTask(taskDetail.id);
    } catch (error) {
      message.error(error instanceof Error ? error.message : t("asrTasks.detail.startCompressionFailed"));
    } finally {
      setChangingSourceCompression(false);
    }
  }, [taskDetail, onRefreshTask, t]);

  const handleCancelSourceCompression = useCallback(async () => {
    if (!taskDetail) return;
    setChangingSourceCompression(true);
    try {
      const result = await cancelAsrSourceAudioCompression(taskDetail.id);
      message.info(result.compression?.message || t("asrTasks.detail.compressionCancelRequested"));
      onRefreshTask(taskDetail.id);
    } catch (error) {
      message.error(error instanceof Error ? error.message : t("asrTasks.detail.cancelCompressionFailed"));
    } finally {
      setChangingSourceCompression(false);
    }
  }, [taskDetail, onRefreshTask, t]);

  const dailyAgentRunMenuItems = useMemo(() => {
    const agents = taskDetail?.daily_agent?.agents || [];
    return [
      {
        key: "__all__",
        label: t("asrTasks.field.runAllAgents"),
      },
      ...agents.map((agent) => ({
        key: agent.id,
        label: t("asrTasks.detail.runAgent", { name: agent.name || agent.id }),
        disabled: !agent.enabled,
      })),
    ];
  }, [taskDetail?.daily_agent?.agents, t]);

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
          message.warning(result.message || t("asrTasks.detail.dailyAgentAlreadyRunning"));
          return;
        }
        message.success(
          result.message ||
            (agentId
              ? t("asrTasks.detail.dailyAgentQueued", { agent: agentId, date })
              : t("asrTasks.detail.allDailyAgentsQueued", { date })),
        );
      } catch (error) {
        message.error(
          t("asrTasks.detail.dailyAgentRunFailed", {
            error: error instanceof Error ? error.message : String(error),
          }),
        );
      } finally {
        setDailyAgentRunDate(null);
      }
    },
    [dailyAgentRunDate, taskDetail, t],
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
                aria-label={t("asrTasks.detail.backToDailyDocs")}
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
                  <Descriptions.Item label={t("asrTasks.field.task")}>
                    {taskDailyDocument.task_name}
                  </Descriptions.Item>
                  <Descriptions.Item label={t("asrTasks.field.date")}>{taskDailyDocument.date}</Descriptions.Item>
                  <Descriptions.Item label={t("asrTasks.field.path")} span={2}>
                    <Text
                      style={{ display: "inline-block", fontSize: 12, whiteSpace: "nowrap" }}
                      ellipsis={{ tooltip: taskDailyDocument.path }}
                    >
                      {taskDailyDocument.path}
                    </Text>
                  </Descriptions.Item>
                  <Descriptions.Item label={t("asrTasks.field.size")}>
                    {formatBytes(taskDailyDocument.size)}
                  </Descriptions.Item>
                  <Descriptions.Item label={t("asrTasks.field.modified")}>
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
                ? t("asrTasks.detail.loadingDailyDocument")
                : t("asrTasks.detail.dailyDocumentNotFound")}
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
                aria-label={t("asrTasks.detail.backToDailyAgentReports")}
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
                  <Descriptions.Item label={t("asrTasks.field.task")}>
                    {taskDailyAgentReport.task_name}
                  </Descriptions.Item>
                  <Descriptions.Item label={t("asrTasks.field.date")}>
                    {taskDailyAgentReport.date}
                  </Descriptions.Item>
                  <Descriptions.Item label={t("asrTasks.field.agent")}>
                    <Tag>
                      {taskDailyAgentReport.agent_name ||
                        taskDailyAgentReport.agent_id ||
                        "daily_report"}
                    </Tag>
                  </Descriptions.Item>
                  <Descriptions.Item label={t("asrTasks.field.outputDir")}>
                    <Text code>{taskDailyAgentReport.output_dir || "report"}</Text>
                  </Descriptions.Item>
                  <Descriptions.Item label={t("asrTasks.field.path")} span={2}>
                    <Text
                      style={{ display: "inline-block", fontSize: 12, whiteSpace: "nowrap" }}
                      ellipsis={{ tooltip: taskDailyAgentReport.path }}
                    >
                      {taskDailyAgentReport.path}
                    </Text>
                  </Descriptions.Item>
                  <Descriptions.Item label={t("asrTasks.field.size")}>
                    {formatBytes(taskDailyAgentReport.size)}
                  </Descriptions.Item>
                  <Descriptions.Item label={t("asrTasks.field.modified")}>
                    {formatTime(taskDailyAgentReport.modified_ms)}
                  </Descriptions.Item>
                  <Descriptions.Item label={t("asrTasks.field.processedAt")}>
                    {formatTime(taskDailyAgentReport.processed_at_ms)}
                  </Descriptions.Item>
                  <Descriptions.Item label={t("asrTasks.field.runner")}>
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
                ? t("asrTasks.detail.loadingDailyAgentReport")
                : t("asrTasks.detail.dailyAgentReportNotFound")}
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
  const compressedSourceFileCount = summary?.compressed_source_file_count ?? 0;
  const compressionTrackedFileCount =
    compressedSourceFileCount + compressibleSourceFileCount;
  const overallCompressionPercent =
    compressionTrackedFileCount > 0
      ? Math.round((compressedSourceFileCount / compressionTrackedFileCount) * 100)
      : 0;
  const failedChunkCount = summary?.failed_chunk_count ?? 0;
  const failedFileCount =
    taskDetail?.files.filter((file) => file.status === "failed").length ?? 0;
  const cleanableSourceFileCount = summary?.cleanable_source_file_count ?? 0;
  const cleanableSourceBytes = summary?.cleanable_source_bytes ?? 0;
  const bulkRetryPercent =
    bulkRetry && bulkRetry.queued_files > 0
      ? Math.round((bulkRetry.processed_files / bulkRetry.queued_files) * 100)
      : 0;
  const sourceCompressionPanel = taskDetail ? (
    <Alert
      data-testid="asr-source-compression-status"
      type={
        sourceCompression?.status === "completed"
          ? "success"
          : sourceCompression?.status === "completed_with_errors" ||
              sourceCompression?.status === "failed" ||
              sourceCompression?.status === "interrupted"
            ? "warning"
            : "info"
      }
      showIcon
      message={
        <Space wrap>
          <Text strong>{t("asrTasks.detail.losslessCompression")}</Text>
          <Tag
            color={
              sourceCompression?.status === "completed"
                ? "success"
                : sourceCompressionActive
                  ? "processing"
                  : "default"
            }
          >
            {sourceCompression?.status ?? t("asrTasks.detail.compressionStatusReady")}
          </Tag>
          <Text>{t("asrTasks.detail.compressedCount", { count: compressedSourceFileCount })}</Text>
          <Text>
            {t("asrTasks.detail.uncompressedEligibleCount", { count: compressibleSourceFileCount })}
          </Text>
          <Text>
            {t("asrTasks.detail.compressedRatio", {
              compressed: compressedSourceFileCount,
              total: compressionTrackedFileCount,
            })}
          </Text>
          <Text>
            {t("asrTasks.detail.savedSize", {
              size: formatBytes(summary?.compression_saved_bytes ?? 0),
            })}
          </Text>
        </Space>
      }
      description={
        <Space direction="vertical" size={6} style={{ width: "100%" }}>
          <Text type="secondary">
            {sourceCompression?.message ?? t("asrTasks.detail.compressionDefaultMessage")}
          </Text>
          {sourceCompression?.current_source_path ? (
            <Text ellipsis={{ tooltip: sourceCompression.current_source_path }}>
              {t("asrTasks.detail.currentPath", { path: sourceCompression.current_source_path })}
            </Text>
          ) : null}
          {sourceCompression ? (
            <Text type="secondary">
              {t("asrTasks.detail.thisRun", {
                processed: sourceCompression.processed_files,
                queued: sourceCompression.queued_files,
              })}
            </Text>
          ) : null}
          <Progress
            percent={overallCompressionPercent}
            size="small"
            status={sourceCompressionActive ? "active" : "normal"}
            format={() =>
              `${compressedSourceFileCount}/${compressionTrackedFileCount}`
            }
          />
        </Space>
      }
    />
  ) : null;
  const overviewTabContent = taskDetail ? (
    <Space direction="vertical" size={16} style={{ width: "100%" }}>
      <Descriptions size="small" bordered column={2}>
        <Descriptions.Item label={t("asrTasks.detail.directory")} span={2}>
          {taskDetail.audio_dir}
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.field.schedule")}>
          {taskDetail.enabled ? formatSchedule(taskDetail.schedule) : t("common.disabled")}
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.detail.runState")}>
          <Tag
            color={
              taskDetail.paused
                ? "warning"
                : taskDetail.summary.running
                  ? "processing"
                  : "success"
            }
          >
            {pauseStatusLabel(t, taskDetail)}
          </Tag>
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.detail.nextRun")}>
          {formatTime(taskDetail.next_run_at_ms)}
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.field.lastRun")}>
          {formatTime(taskDetail.last_run_at_ms)}
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.field.mode")}>
          {taskDetail.recursive ? t("asrTasks.detail.recursive") : t("asrTasks.detail.flat")}
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.field.runtime")}>
          {taskDetail.runtime_strategy}
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.field.speakerDiarization")}>
          {taskDetail.diarization?.enabled ? (
            <Space wrap>
              <Tag color={taskDetail.summary.diarization_ready ? "purple" : "warning"}>
                {taskDetail.summary.diarization_ready
                  ? t("asrTasks.field.ready")
                  : t("asrTasks.detail.needsSetup")}
              </Tag>
              <Text>{taskDetail.diarization.profile}</Text>
            </Space>
          ) : (
            t("common.disabled")
          )}
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.field.speakers")}>
          {t("asrTasks.detail.speakersSummary", {
            files: taskDetail.summary.diarized_files ?? 0,
            speakers: taskDetail.summary.speaker_count ?? 0,
          })}
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.field.processed")}>
          {taskDetail.summary.processed}
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.field.pending")}>
          {taskDetail.summary.pending}
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.detail.failed")}>{taskDetail.summary.failed}</Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.detail.audioFiles")}>
          <Space>
            <Text>{formatBytes(taskDetail.summary.audio_source_bytes)}</Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {t("asrTasks.detail.filesCountParen", { count: taskDetail.summary.audio_source_file_count })}
            </Text>
          </Space>
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.detail.cleanableOriginals")}>
          <Space>
            <Text>{formatBytes(cleanableSourceBytes)}</Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {t("asrTasks.detail.filesCountParen", { count: cleanableSourceFileCount })}
            </Text>
          </Space>
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.detail.compressibleWav")}>
          <Space>
            <Text>{formatBytes(compressibleSourceBytes)}</Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {t("asrTasks.detail.filesCountParen", { count: compressibleSourceFileCount })}
            </Text>
          </Space>
        </Descriptions.Item>
        <Descriptions.Item label={t("asrTasks.detail.compressionSaved")}>
          <Space>
            <Text>{formatBytes(summary?.compression_saved_bytes ?? 0)}</Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {t("asrTasks.detail.filesCountParen", {
                count: summary?.compressed_source_file_count ?? 0,
              })}
            </Text>
          </Space>
        </Descriptions.Item>
        {summary && summary.partial_success > 0 ? (
          <Descriptions.Item label={t("asrTasks.detail.partialSuccess")}>
            <Space>
              <Tag color="warning">{summary.partial_success}</Tag>
              <Text type="secondary" style={{ fontSize: 12 }}>
                {t("asrTasks.detail.failedChunksParen", { count: summary.failed_chunk_count })}
              </Text>
            </Space>
          </Descriptions.Item>
        ) : (
          <Descriptions.Item label={t("asrTasks.detail.deletedAfterProcessing")}>
            {taskDetail.summary.deleted_after_processing}
          </Descriptions.Item>
        )}
        <Descriptions.Item label={t("asrTasks.field.lastError")} span={2}>
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
              <Text strong>{t("asrTasks.detail.bulkChunkRetry")}</Text>
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
                {t("asrTasks.detail.filesRatio", {
                  processed: bulkRetry.processed_files,
                  queued: bulkRetry.queued_files,
                })}
              </Text>
              <Text>
                {t("asrTasks.detail.chunksRecoveredRatio", {
                  recovered: bulkRetry.recovered_chunks,
                  total: bulkRetry.total_failed_chunks,
                })}
              </Text>
              {bulkRetry.still_failed_chunks > 0 ? (
                <Text type="warning">
                  {t("asrTasks.detail.stillFailed", { count: bulkRetry.still_failed_chunks })}
                </Text>
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
                  {t("asrTasks.detail.currentPath", { path: bulkRetry.current_source_path })}
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
                  {t("asrTasks.detail.lastFile", { message: bulkRetry.results.at(-1)?.message })}
                </Text>
              ) : null}
            </Space>
          }
        />
      ) : null}
      {sourceCompressionPanel}
    </Space>
  ) : null;

  const taskActionMenuItems: MenuProps["items"] = taskDetail
    ? [
        {
          type: "group",
          label: t("common.status"),
          children: [
            {
              key: "task-status",
              disabled: true,
              label: (
                <Space data-testid="asr-task-actions-status">
                  <Text type="secondary">{t("asrTasks.field.task")}</Text>
                  <Tag
                    color={
                      taskDetail.last_error && !taskDetail.summary.running
                        ? "error"
                        : taskDetail.summary.running
                          ? "processing"
                          : taskDetail.paused
                            ? "warning"
                            : "default"
                    }
                  >
                    {pauseStatusLabel(t, taskDetail)}
                  </Tag>
                </Space>
              ),
            },
            ...(bulkRetry
              ? [
                  {
                    key: "bulk-retry-status",
                    disabled: true,
                    label: (
                      <Space>
                        <Text type="secondary">{t("asrTasks.detail.bulkRetry")}</Text>
                        <Tag color={bulkRetryActive ? "processing" : "default"}>
                          {bulkRetry.status}
                        </Tag>
                      </Space>
                    ),
                  },
                ]
              : []),
            ...(sourceCompression
              ? [
                  {
                    key: "source-compression-status",
                    disabled: true,
                    label: (
                      <Space>
                        <Text type="secondary">{t("asrTasks.field.compression")}</Text>
                        <Tag color={sourceCompressionActive ? "processing" : "default"}>
                          {sourceCompression.status}
                        </Tag>
                      </Space>
                    ),
                  },
                ]
              : []),
          ],
        },
        { type: "divider" },
        {
          key: "refresh",
          icon: <ReloadOutlined />,
          label: t("common.refresh"),
          onClick: () => onRefreshTask(taskDetail.id),
        },
        { type: "divider" },
        sourceCompressionActive
          ? {
              key: "cancel-compression",
              danger: true,
              icon: changingSourceCompression ? <LoadingOutlined /> : <StopOutlined />,
              label:
                sourceCompression?.status === "cancelling"
                  ? t("asrTasks.detail.cancellingCompression")
                  : t("asrTasks.detail.cancelCompression"),
              disabled:
                changingSourceCompression || sourceCompression?.status === "cancelling",
              onClick: () => void handleCancelSourceCompression(),
            }
          : {
              key: "compress-wavs",
              icon: changingSourceCompression ? <LoadingOutlined /> : <FileZipOutlined />,
              label: t("asrTasks.detail.compressWavs"),
              disabled:
                compressibleSourceFileCount === 0 ||
                changingSourceCompression ||
                taskDetail.summary.running ||
                bulkRetryActive,
              onClick: () => {
                Modal.confirm({
                  title: t(
                    compressibleSourceFileCount === 1
                      ? "asrTasks.detail.compressConfirmTitleOne"
                      : "asrTasks.detail.compressConfirmTitleOther",
                    { count: compressibleSourceFileCount },
                  ),
                  content: t("asrTasks.detail.compressConfirmContent"),
                  onOk: handleStartSourceCompression,
                });
              },
            },
        {
          key: "clean-originals",
          danger: true,
          icon: cleaningSourceAudio ? <LoadingOutlined /> : <DeleteOutlined />,
          label: t("asrTasks.detail.cleanOriginals"),
          disabled:
            cleanableSourceFileCount === 0 ||
            cleaningSourceAudio ||
            taskDetail.summary.running ||
            bulkRetryActive ||
            sourceCompressionActive,
          onClick: () => {
            Modal.confirm({
              title: t(
                cleanableSourceFileCount === 1
                  ? "asrTasks.detail.cleanFirstConfirmTitleOne"
                  : "asrTasks.detail.cleanFirstConfirmTitleOther",
                { count: cleanableSourceFileCount },
              ),
              content: t("asrTasks.detail.cleanFirstConfirmContent"),
              okText: t("asrTasks.detail.continue"),
              onOk: () => setCleanupConfirmOpen(true),
            });
          },
        },
        {
          key: "retry-failed-files",
          icon: startingFailedFilesRetry ? <LoadingOutlined /> : <ReloadOutlined />,
          label: startingFailedFilesRetry
            ? t("asrTasks.detail.retryingFailedFiles")
            : t("asrTasks.detail.retryAllFailedFiles"),
          disabled:
            failedFileCount === 0 ||
            startingFailedFilesRetry ||
            startingBulkRetry ||
            taskDetail.summary.running ||
            Boolean(taskDetail.paused) ||
            bulkRetryActive ||
            sourceCompressionActive,
          onClick: () => {
            Modal.confirm({
              title: t(
                failedFileCount === 1
                  ? "asrTasks.detail.retryFilesConfirmTitleOne"
                  : "asrTasks.detail.retryFilesConfirmTitleOther",
                { count: failedFileCount },
              ),
              content: t("asrTasks.detail.retryFilesConfirmContent"),
              onOk: handleRetryAllFailedFiles,
            });
          },
        },
        {
          key: "retry-failed-chunks",
          icon: startingBulkRetry || bulkRetryActive ? <LoadingOutlined /> : <ReloadOutlined />,
          label: bulkRetryActive
            ? t("asrTasks.detail.retryingFailedChunks")
            : t("asrTasks.detail.retryAllFailedChunks"),
          disabled:
            failedChunkCount === 0 ||
            bulkRetryActive ||
            startingBulkRetry ||
            sourceCompressionActive,
          onClick: () => {
            Modal.confirm({
              title: t(
                failedChunkCount > 1
                  ? "asrTasks.detail.retryAllChunksConfirmTitleOther"
                  : "asrTasks.detail.retryAllChunksConfirmTitleOne",
                { count: failedChunkCount },
              ),
              content: t("asrTasks.detail.retryAllChunksConfirmContent"),
              onOk: handleRetryAllFailedChunks,
            });
          },
        },
        { type: "divider" },
        {
          key: "run",
          icon: taskDetail.summary.running ? <LoadingOutlined /> : <PlayCircleOutlined />,
          label: t("asrTasks.field.run"),
          disabled:
            taskDetail.summary.running ||
            Boolean(taskDetail.paused) ||
            sourceCompressionActive,
          onClick: () => onRunTask(taskDetail.id),
        },
        taskDetail.paused
          ? {
              key: "resume",
              icon: <PlayCircleOutlined />,
              label: t("common.resume"),
              onClick: () => onResumeTask(taskDetail.id),
            }
          : {
              key: "pause",
              icon: <PauseCircleOutlined />,
              label: t("common.pause"),
              children: [
                {
                  key: "pause-temporary",
                  icon: <PauseCircleOutlined />,
                  label: t("asrTasks.pause.untilNextSchedule"),
                  onClick: () => onPauseTask(taskDetail.id, false, "temporary"),
                },
                {
                  key: "pause-long-term",
                  icon: <StopOutlined />,
                  label: t("asrTasks.pause.indefinitely"),
                  onClick: () => onPauseTask(taskDetail.id, false, "long_term"),
                },
              ],
            },
        ...(taskDetail.summary.running && !taskDetail.paused
          ? [
              {
                key: "force-pause",
                danger: true,
                icon: <StopOutlined />,
                label: t("asrTasks.pause.force"),
                onClick: () => {
                  Modal.confirm({
                    title: t("asrTasks.pause.forceConfirmTitle"),
                    content: t("asrTasks.pause.forceConfirmDescription"),
                    okText: t("asrTasks.pause.force"),
                    okType: "danger",
                    onOk: () => onPauseTask(taskDetail.id, true, "long_term"),
                  });
                },
              },
            ]
          : []),
      ]
    : [];

  return (
    <div className="asr-task-detail-page" data-testid="asr-task-detail-page">
      <Card
        className="asr-task-detail-card asr-task-detail-card--tabs"
        title={
          <Space style={{ minWidth: 0 }}>
            <Button
              icon={<ArrowLeftOutlined />}
              onClick={onBackToTasks}
              aria-label={t("asrTasks.detail.backToDirectoryTasks")}
            />
            <span
              style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
            >
              {taskDetail
                ? t("asrTasks.detail.titleWithName", { name: taskDetail.name })
                : t("asrTasks.detail.title")}
            </span>
          </Space>
        }
        extra={
          taskDetail ? (
            <Dropdown menu={{ items: taskActionMenuItems }} trigger={["click"]} placement="bottomRight">
              <Button
                type="text"
                size="small"
                icon={<MoreOutlined />}
                aria-label={t("asrTasks.detail.moreActions")}
                title={t("asrTasks.detail.moreActions")}
                data-testid="asr-task-actions-menu-button"
              />
            </Dropdown>
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
                  label: t("asrTasks.detail.tab.overview"),
                  children: <div data-testid="asr-task-overview-tab">{overviewTabContent}</div>,
                },
                {
                  key: "files",
                  label: t("asrTasks.detail.tab.files", { count: taskDetail.files.length }),
                  children: (
                    <div data-testid="asr-task-files-table">
                      <div style={{ marginBottom: 12 }}>{sourceCompressionPanel}</div>
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
                              label: t("asrTasks.detail.filter.processing", {
                                count: fileStatusCounts.processing,
                              }),
                              value: "processing",
                            },
                            {
                              label: t("asrTasks.detail.filter.pending", { count: fileStatusCounts.pending }),
                              value: "pending",
                            },
                            {
                              label: t("asrTasks.detail.filter.completed", {
                                count: fileStatusCounts.completed,
                              }),
                              value: "completed",
                            },
                            {
                              label: t("asrTasks.detail.filter.failed", { count: fileStatusCounts.failed }),
                              value: "failed",
                            },
                            {
                              label: t("asrTasks.detail.filter.all", { count: fileStatusCounts.all }),
                              value: "all",
                            },
                          ]}
                        />
                        <Text type="secondary" style={{ fontSize: 12 }}>
                          {t("asrTasks.detail.showingFiles", {
                            shown: filteredTaskFiles.length,
                            total: fileStatusCounts.all,
                          })}
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
                    title: t("asrTasks.field.file"),
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
                            {t("asrTasks.detail.openTranscript")}
                          </Button>
                        ) : null}
                      </Space>
                    ),
                  },
                  {
                    title: t("common.status"),
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
                      if (value === "failed") {
                        const retryDisabled =
                          retryingFailedFileKey !== null ||
                          startingFailedFilesRetry ||
                          startingBulkRetry ||
                          taskDetail.summary.running ||
                          Boolean(taskDetail.paused) ||
                          bulkRetryActive ||
                          sourceCompressionActive;
                        return (
                          <Space direction="vertical" size={4} style={{ width: "100%" }}>
                            {tag}
                            <Popconfirm
                              title={t("asrTasks.detail.retryFileConfirmTitle")}
                              description={t("asrTasks.detail.retryFileConfirmDescription")}
                              onConfirm={() => handleRetryFailedFile(record.key)}
                            >
                              <Button
                                type="link"
                                size="small"
                                icon={
                                  retryingFailedFileKey === record.key ? (
                                    <LoadingOutlined />
                                  ) : (
                                    <ReloadOutlined />
                                  )
                                }
                                disabled={retryDisabled}
                                style={{ padding: 0, height: "auto" }}
                              >
                                {t("asrTasks.detail.retryTranscription")}
                              </Button>
                            </Popconfirm>
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
                                      {t("asrTasks.detail.chunkTooltip", {
                                        index: fc.chunk_index,
                                        start: fc.offset_secs,
                                        end: fc.offset_secs + fc.duration_secs,
                                        attempts: fc.attempts,
                                      })}
                                      <br />
                                      <span style={{ color: "#ff7875" }}>{fc.error}</span>
                                    </div>
                                  ))}
                                </div>
                              }
                            >
                              <Text type="warning" style={{ fontSize: 12, cursor: "help" }}>
                                <WarningOutlined />{" "}
                                {t(
                                  failedChunks.length > 1
                                    ? "asrTasks.detail.failedChunksOther"
                                    : "asrTasks.detail.failedChunksOne",
                                  { count: failedChunks.length },
                                )}
                              </Text>
                            </Tooltip>
                            <Popconfirm
                              title={t(
                                failedChunks.length > 1
                                  ? "asrTasks.detail.retryChunksConfirmTitleOther"
                                  : "asrTasks.detail.retryChunksConfirmTitleOne",
                                { count: failedChunks.length },
                              )}
                              description={t("asrTasks.detail.retryChunksConfirmDescription")}
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
                                {t("asrTasks.detail.retryChunks")}
                              </Button>
                            </Popconfirm>
                          </Space>
                        );
                      }
                      return tag;
                    },
                  },
                  {
                    title: t("asrTasks.field.compression"),
                    width: 170,
                    render: (_value, record) => {
                      const display = compressionDisplayForFile(t, record, sourceCompression);
                      return (
                        <Space direction="vertical" size={2} style={{ width: "100%" }}>
                          <Tag color={display.color}>{display.label}</Tag>
                          {display.detail ? (
                            <Text
                              type="secondary"
                              style={{ width: 150, fontSize: 11 }}
                              ellipsis={{ tooltip: display.detail }}
                            >
                              {display.detail}
                            </Text>
                          ) : null}
                        </Space>
                      );
                    },
                  },
                  {
                    title: t("asrTasks.detail.diarization"),
                    width: 150,
                    render: (_value, record) =>
                      record.diarization_status ? (
                        <Space direction="vertical" size={2}>
                          <Tag color={record.diarization_status === "success" ? "purple" : "default"}>
                            {record.diarization_status}
                          </Tag>
                          {record.speaker_count ? (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              {t("asrTasks.detail.speakerCount", { count: record.speaker_count })}
                            </Text>
                          ) : null}
                        </Space>
                      ) : (
                        <Text type="secondary">-</Text>
                      ),
                  },
                  {
                    title: t("asrTasks.field.text"),
                    dataIndex: "text_chars",
                    width: 90,
                    render: (value) => t("asrTasks.detail.chars", { count: value }),
                  },
                  {
                    title: t("asrTasks.field.runtime"),
                    width: 180,
                    render: (_value, record) => {
                      const metrics = record.chunk_metrics ?? [];
                      const last = metrics.at(-1);
                      return (
                        <Space direction="vertical" size={0}>
                          <Tag>{record.runtime_strategy ?? taskDetail.runtime_strategy}</Tag>
                          {last ? (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              {t("asrTasks.detail.metricsSummary", {
                                count: metrics.length,
                                runner: last.runner,
                                rtf: last.rtf.toFixed(3),
                              })}
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
                    title: t("asrTasks.field.recorded"),
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
                    title: t("asrTasks.detail.audio"),
                    dataIndex: "media_duration_ms",
                    width: 110,
                    render: (value) => formatDuration(value),
                  },
                  {
                    title: t("asrTasks.detail.sourceSize"),
                    dataIndex: "source_size",
                    width: 110,
                    render: (value) => formatBytes(value),
                  },
                  {
                    title: t("asrTasks.detail.started"),
                    dataIndex: "started_at_ms",
                    width: 180,
                    render: (value) => formatTime(value),
                  },
                  {
                    title: t("asrTasks.detail.elapsed"),
                    width: 110,
                    render: (_value, record) => formatDuration(fileElapsedMs(record, nowMs)),
                  },
                  {
                    title: t("asrTasks.detail.finished"),
                    dataIndex: "finished_at_ms",
                    width: 180,
                    render: (value) => formatTime(value),
                  },
                  {
                    title: t("asrTasks.detail.result"),
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
                  label: t("asrTasks.detail.tab.dailyDocs", { count: dailyDocuments.length }),
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
                              title: t("asrTasks.field.date"),
                              dataIndex: "date",
                              width: 140,
                              render: (value) => <Text strong>{value}</Text>,
                            },
                            {
                              title: t("asrTasks.detail.document"),
                              dataIndex: "path",
                              render: (value) => (
                                <Text style={{ fontSize: 12 }} ellipsis={{ tooltip: value }}>
                                  {value}
                                </Text>
                              ),
                            },
                            {
                              title: t("asrTasks.field.size"),
                              dataIndex: "size",
                              width: 110,
                              render: (value) => formatBytes(value),
                            },
                            {
                              title: t("asrTasks.field.text"),
                              dataIndex: "text_chars",
                              width: 110,
                              render: (value) => t("asrTasks.detail.chars", { count: value }),
                            },
                            {
                              title: t("asrTasks.field.modified"),
                              dataIndex: "modified_ms",
                              width: 180,
                              render: (value) => formatTime(value),
                            },
                            {
                              title: t("asrTasks.detail.action"),
                              width: 220,
                              render: (_value, record) => (
                                <Space size={8} wrap>
                                  <Button
                                    type="link"
                                    size="small"
                                    style={{ padding: 0 }}
                                    onClick={() => onOpenDailyDocument(record.date)}
                                  >
                                    {t("asrTasks.detail.openDocument")}
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
                                    {t("asrTasks.field.runAllAgents")}
                                  </Dropdown.Button>
                                </Space>
                              ),
                            },
                          ]}
                        />
                      ) : dailyDocumentsUnavailable ? (
                        <Empty description={t("asrTasks.detail.dailyDocsUnavailable")} />
                      ) : (
                        <Empty description={t("asrTasks.detail.noDailyDocs")} />
                      )}
                    </div>
                  ),
                },
                {
                  key: "daily-agent",
                  label: t("asrTasks.detail.tab.dailyAgent"),
                  children: taskDetail ? (
                    <DailyAgentTab taskId={taskDetail.id} />
                  ) : null,
                },
                {
                  key: "daily-agent-records",
                  label: t("asrTasks.detail.tab.dailyAgentRecords"),
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
              ? t("asrTasks.detail.loadingTask")
              : t("asrTasks.detail.selectTask")}
          </Text>
        )}
      </Card>
      <Modal
        title={t("asrTasks.detail.cleanFinalTitle")}
        open={cleanupConfirmOpen}
        okText={t("asrTasks.detail.cleanOriginals")}
        cancelText={t("common.cancel")}
        confirmLoading={cleaningSourceAudio}
        okButtonProps={{
          danger: true,
          disabled: cleanupConfirmName !== taskDetail?.name,
        }}
        onOk={() => void handleCleanupSourceAudio()}
        onCancel={() => {
          if (!cleaningSourceAudio) {
            setCleanupConfirmOpen(false);
            setCleanupConfirmName("");
          }
        }}
      >
        <Space direction="vertical" size={12} style={{ width: "100%" }}>
          <Alert
            type="error"
            showIcon
            message={t(
              cleanableSourceFileCount === 1
                ? "asrTasks.detail.cleanFinalMessageOne"
                : "asrTasks.detail.cleanFinalMessageOther",
              { count: cleanableSourceFileCount, size: formatBytes(cleanableSourceBytes) },
            )}
            description={t("asrTasks.detail.cleanFinalDescription")}
          />
          <Text>
            {t("asrTasks.detail.typeTaskNameBefore")}
            <Text code>{taskDetail?.name ?? ""}</Text>
            {t("asrTasks.detail.typeTaskNameAfter")}
          </Text>
          <Input
            autoFocus
            placeholder={t("asrTasks.field.typeTaskName")}
            value={cleanupConfirmName}
            disabled={cleaningSourceAudio}
            onChange={(event) => setCleanupConfirmName(event.target.value)}
          />
        </Space>
      </Modal>
    </div>
  );
}
