import { useCallback, useEffect, useRef, useState } from "react";
import type { FormInstance } from "antd";
import {
  Button,
  Card,
  Col,
  Dropdown,
  Form,
  Input,
  InputNumber,
  Modal,
  Popconfirm,
  Progress,
  Row,
  Select,
  Space,
  Switch,
  Table,
  Tag,
  Typography,
} from "antd";
import {
  AudioOutlined,
  PauseCircleOutlined,
  PlayCircleOutlined,
  SettingOutlined,
  PlusOutlined,
  ImportOutlined,
  StopOutlined,
} from "@ant-design/icons";
import type {
  AsrDirectoryTask,
  AsrExternalImportRunProgress,
  AsrExternalVolume,
  AsrPauseMode,
  AsrRuntimeStrategy,
  AsrTranscriptionMode,
} from "../../../api/asr";
import { getMossModelStatus, listAsrExternalVolumes } from "../../../api/asr";
import { formatSchedule, formatTime } from "../asrUtils";
import {
  directoryTaskModeHistoryNote,
  directoryTaskModeFields,
  directoryTaskModeOptions,
} from "../directoryTaskMode";
import { t as tNow, useI18n, type TFunction } from "../../../i18n";

const { Text } = Typography;

function pauseStatusLabel(t: TFunction, task: AsrDirectoryTask): string {
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

const runtimeStrategyOptions = (
  t: TFunction,
): Array<{
  value: AsrRuntimeStrategy;
  label: string;
  description: string;
}> => [
  {
    value: "reuse_per_file",
    label: t("asrTasks.runtime.reusePerFile"),
    description: t("asrTasks.runtime.reusePerFileDescription"),
  },
  {
    value: "fork_per_chunk",
    label: t("asrTasks.runtime.forkPerChunk"),
    description: t("asrTasks.runtime.forkPerChunkDescription"),
  },
  {
    value: "reuse_server",
    label: t("asrTasks.runtime.reuseServer"),
    description: t("asrTasks.runtime.reuseServerDescription"),
  },
  {
    value: "auto",
    label: t("asrTasks.runtime.auto"),
    description: t("asrTasks.runtime.autoDescription"),
  },
  {
    value: "compare",
    label: t("asrTasks.runtime.compare"),
    description: t("asrTasks.runtime.compareDescription"),
  },
];

function externalImportPercent(progress: AsrExternalImportRunProgress): number {
  if (progress.status === "completed" || progress.status === "completed_with_errors") {
    return 100;
  }
  if (progress.total_files_discovered > 0) {
    return Math.max(
      0,
      Math.min(
        99,
        Math.round((progress.processed_files / progress.total_files_discovered) * 100),
      ),
    );
  }
  return progress.status === "importing" ? 5 : 0;
}

function externalImportProgressStatus(
  progress: AsrExternalImportRunProgress,
): "success" | "exception" | "active" | "normal" {
  if (progress.status === "completed") return "success";
  if (progress.status === "failed" || progress.status === "completed_with_errors") {
    return "exception";
  }
  if (progress.status === "importing") return "active";
  return "normal";
}

function externalImportProgressLabel(
  t: TFunction,
  progress: AsrExternalImportRunProgress,
): string {
  if (progress.status === "importing") {
    if (progress.current_device) {
      return t("asrTasks.import.deviceProgress", {
        device: progress.current_device,
        processed: progress.processed_files,
        total: progress.total_files_discovered,
      });
    }
    return t("asrTasks.import.queued");
  }
  return progress.message;
}

function shouldShowExternalImportProgress(progress: AsrExternalImportRunProgress): boolean {
  if (progress.status === "importing") {
    return true;
  }
  return Boolean(
    progress.imported ||
      progress.skipped ||
      progress.processed_record_skipped ||
      progress.failed,
  );
}

function externalImportCurrentFileName(progress: AsrExternalImportRunProgress): string {
  if (!progress.current_file) {
    return "-";
  }
  return progress.current_file.split(/[\\/]/).filter(Boolean).pop() ?? progress.current_file;
}

function externalImportCurrentFileCopyLabel(
  t: TFunction,
  progress: AsrExternalImportRunProgress,
): string | null {
  if (!progress.current_file_size || progress.current_file_size <= 0) {
    return null;
  }
  const copied = Math.min(progress.current_file_copied_bytes, progress.current_file_size);
  const percent = Math.max(
    0,
    Math.min(100, Math.round((copied / progress.current_file_size) * 100)),
  );
  return t("asrTasks.import.currentFilePercent", { percent });
}

interface DirectoryTasksPanelProps {
  taskForm: FormInstance;
  taskScheduleKind: string;
  tasks: AsrDirectoryTask[];
  tasksLoading: boolean;
  externalImportProgressByTask: Record<string, AsrExternalImportRunProgress>;
  onCreateTask: () => boolean | Promise<boolean>;
  onUpdateTask: (id: string) => boolean | Promise<boolean>;
  onRunExternalImport: (id: string) => void | Promise<void>;
  onOpenTask: (id: string) => void;
  onRunTask: (id: string) => void;
  onPauseTask: (id: string, force?: boolean, mode?: AsrPauseMode) => void;
  onResumeTask: (id: string) => void;
  onRemoveTask: (id: string, confirmName: string) => void;
}

export default function DirectoryTasksPanel({
  taskForm,
  taskScheduleKind,
  tasks,
  tasksLoading,
  externalImportProgressByTask,
  onCreateTask,
  onUpdateTask,
  onRunExternalImport,
  onOpenTask,
  onRunTask,
  onPauseTask,
  onResumeTask,
  onRemoveTask,
}: DirectoryTasksPanelProps) {
  const { t } = useI18n();
  const [createOpen, setCreateOpen] = useState(false);
  const [editingTask, setEditingTask] = useState<AsrDirectoryTask | null>(null);
  const [creating, setCreating] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<AsrDirectoryTask | null>(null);
  const [deleteConfirmName, setDeleteConfirmName] = useState("");
  const [volumePrompt, setVolumePrompt] = useState<{
    task: AsrDirectoryTask | null;
    volumes: AsrExternalVolume[];
  } | null>(null);
  const [volumePromptLoading, setVolumePromptLoading] = useState(false);
  const [mossPlatformSupported, setMossPlatformSupported] = useState<boolean | null>(null);
  const [mossUnsupportedReason, setMossUnsupportedReason] = useState<string | null>(null);
  const promptedVolumeKeysRef = useRef(new Set<string>());
  const volumePromptOpenRef = useRef(false);

  const configOpen = createOpen || Boolean(editingTask);
  const transcriptionMode = Form.useWatch<AsrTranscriptionMode>("transcription_mode", taskForm);
  const transcriptionPrompt = Form.useWatch<string>("transcription_prompt", taskForm) ?? "";
  const { showMossPrompt, showStandardPipeline } = directoryTaskModeFields(transcriptionMode);
  const configTitle = editingTask
    ? t("asrTasks.tasks.editTitle", { name: editingTask.name })
    : t("asrTasks.tasks.newTitle");
  const expandedImportTaskIds = tasks
    .filter((task) => {
      const progress = externalImportProgressByTask[task.id];
      return progress ? shouldShowExternalImportProgress(progress) : false;
    })
    .map((task) => task.id);

  useEffect(() => {
    if (!configOpen) {
      return;
    }
    let cancelled = false;
    setMossPlatformSupported(null);
    setMossUnsupportedReason(null);
    void getMossModelStatus()
      .then((status) => {
        if (cancelled) return;
        setMossPlatformSupported(status.platform_supported);
        setMossUnsupportedReason(status.unsupported_reason ?? null);
      })
      .catch(() => {
        if (cancelled) return;
        setMossPlatformSupported(false);
        setMossUnsupportedReason(tNow("asrTasks.tasks.mossUnverified"));
      });
    return () => {
      cancelled = true;
    };
  }, [configOpen]);

  const openCreate = () => {
    setEditingTask(null);
    promptedVolumeKeysRef.current.clear();
    taskForm.resetFields();
    taskForm.setFieldsValue(defaultTaskFormValues());
    setCreateOpen(true);
  };

  const openEdit = (task: AsrDirectoryTask) => {
    setCreateOpen(false);
    setEditingTask(task);
    promptedVolumeKeysRef.current.clear();
    taskForm.setFieldsValue(taskToFormValues(task));
  };

  const closeConfig = () => {
    setCreateOpen(false);
    setEditingTask(null);
    setVolumePrompt(null);
    setVolumePromptLoading(false);
    volumePromptOpenRef.current = false;
    promptedVolumeKeysRef.current.clear();
  };

  const promptForConnectedVolumes = useCallback(
    async (task: AsrDirectoryTask | null) => {
      if (volumePromptOpenRef.current) {
        return;
      }
      let volumes: AsrExternalVolume[] = [];
      try {
        volumes = (await listAsrExternalVolumes()).filter(
          (volume) => volume.kind === "external" && !volume.read_only,
        );
      } catch {
        return;
      }
      const currentNames = new Set(
        String(taskForm.getFieldValue("external_devices") || "")
          .split(/[,\n]/)
          .map((name) => name.trim())
          .filter(Boolean),
      );
      const candidates = volumes.filter((volume) => {
        const key = `${task?.id ?? "new"}:${volume.name}:${volume.volume_uuid ?? volume.mount_path}`;
        return !currentNames.has(volume.name) && !promptedVolumeKeysRef.current.has(key);
      });
      if (candidates.length === 0) {
        return;
      }
      candidates.forEach((volume) => {
        const key = `${task?.id ?? "new"}:${volume.name}:${volume.volume_uuid ?? volume.mount_path}`;
        promptedVolumeKeysRef.current.add(key);
      });
      volumePromptOpenRef.current = true;
      setVolumePrompt({ task, volumes: candidates });
    },
    [taskForm],
  );

  useEffect(() => {
    if (!configOpen) {
      return;
    }

    let stopped = false;
    const prompt = () => {
      if (!stopped) {
        void promptForConnectedVolumes(editingTask);
      }
    };

    prompt();
    const timer = window.setInterval(prompt, 2_000);
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [configOpen, editingTask, promptForConnectedVolumes]);

  const handleSave = async () => {
    setCreating(true);
    try {
      const ok = editingTask ? await onUpdateTask(editingTask.id) : await onCreateTask();
      if (ok) {
        closeConfig();
      }
    } finally {
      setCreating(false);
    }
  };

  const closeVolumePrompt = () => {
    setVolumePrompt(null);
    setVolumePromptLoading(false);
    volumePromptOpenRef.current = false;
  };

  const addPromptedVolumes = async () => {
    if (!volumePrompt) {
      return;
    }
    setVolumePromptLoading(true);
    try {
      const existing = String(taskForm.getFieldValue("external_devices") || "")
        .split(/[,\n]/)
        .map((name) => name.trim())
        .filter(Boolean);
      const names = Array.from(
        new Set([...existing, ...volumePrompt.volumes.map((volume) => volume.name)]),
      );
      taskForm.setFieldsValue({
        external_devices: names.join("\n"),
      });
      if (volumePrompt.task) {
        await onUpdateTask(volumePrompt.task.id);
        await onRunExternalImport(volumePrompt.task.id);
      }
      closeVolumePrompt();
    } finally {
      setVolumePromptLoading(false);
    }
  };

  return (
    <Card
      title={
        <Space>
          <AudioOutlined />
          <span>{t("asrTasks.tasks.directoryTasks")}</span>
          <Tag>{tasks.length}</Tag>
        </Space>
      }
      extra={
        <Button
          type="primary"
          icon={<PlusOutlined />}
          onClick={openCreate}
        >
          {t("common.new")}
        </Button>
      }
      style={{ marginTop: 16 }}
    >
      <Modal
        title={configTitle}
        open={configOpen}
        okText={editingTask ? t("common.save") : t("common.create")}
        confirmLoading={creating}
        onOk={() => void handleSave()}
        onCancel={closeConfig}
        destroyOnClose={false}
        width={860}
      >
        <Form
          form={taskForm}
          layout="vertical"
          initialValues={{
            recursive: true,
            enabled: true,
            schedule_kind: "daily",
            schedule_time: "02:00",
            schedule_weekday: 1,
            schedule_day: 1,
            schedule_minute: 0,
            runtime_strategy: "reuse_per_file",
            transcription_mode: "standard",
            transcription_prompt: "",
            max_concurrent_files: 1,
            diarization_enabled: true,
            diarization_profile: "sherpa-onnx-balanced",
            voiceprint_matching: true,
            model: "Qwen3-ASR-0.6B",
            language: "chinese",
          }}
        >
          <Row gutter={[12, 0]}>
            <Col xs={24} md={8}>
              <Form.Item name="name" label={t("common.name")}>
                <Input placeholder={t("asrTasks.tasks.namePlaceholder")} />
              </Form.Item>
            </Col>
            <Col xs={24} md={16}>
              <Form.Item
                name="audio_dir"
                label={t("asrTasks.tasks.audioDirectory")}
                rules={[{ required: true, message: t("asrTasks.tasks.audioDirectoryRequired") }]}
              >
                <Input placeholder="~/Recordings" />
              </Form.Item>
            </Col>
            <Col xs={24} md={8}>
              <Form.Item name="schedule_kind" label={t("asrTasks.tasks.cycle")}>
                <Select
                  options={[
                    { value: "hourly", label: t("asrTasks.tasks.cycle.hourly") },
                    { value: "daily", label: t("asrTasks.tasks.cycle.daily") },
                    { value: "weekly", label: t("asrTasks.tasks.cycle.weekly") },
                    { value: "monthly", label: t("asrTasks.tasks.cycle.monthly") },
                  ]}
                />
              </Form.Item>
            </Col>
            {taskScheduleKind === "hourly" ? (
              <Col xs={24} md={8}>
                <Form.Item name="schedule_minute" label={t("asrTasks.tasks.minute")}>
                  <InputNumber min={0} max={59} style={{ width: "100%" }} />
                </Form.Item>
              </Col>
            ) : null}
            {taskScheduleKind === "daily" ? (
              <Col xs={24} md={8}>
                <Form.Item name="schedule_time" label={t("asrTasks.tasks.time")}>
                  <Input type="time" />
                </Form.Item>
              </Col>
            ) : null}
            {taskScheduleKind === "weekly" ? (
              <>
                <Col xs={24} md={8}>
                  <Form.Item name="schedule_weekday" label={t("asrTasks.tasks.weekday")}>
                    <Select
                      options={[
                        { value: 1, label: t("asrTasks.tasks.weekday.mon") },
                        { value: 2, label: t("asrTasks.tasks.weekday.tue") },
                        { value: 3, label: t("asrTasks.tasks.weekday.wed") },
                        { value: 4, label: t("asrTasks.tasks.weekday.thu") },
                        { value: 5, label: t("asrTasks.tasks.weekday.fri") },
                        { value: 6, label: t("asrTasks.tasks.weekday.sat") },
                        { value: 7, label: t("asrTasks.tasks.weekday.sun") },
                      ]}
                    />
                  </Form.Item>
                </Col>
                <Col xs={24} md={8}>
                  <Form.Item name="schedule_time" label={t("asrTasks.tasks.time")}>
                    <Input type="time" />
                  </Form.Item>
                </Col>
              </>
            ) : null}
            {taskScheduleKind === "monthly" ? (
              <>
                <Col xs={24} md={8}>
                  <Form.Item name="schedule_day" label={t("asrTasks.tasks.day")}>
                    <InputNumber min={1} max={31} style={{ width: "100%" }} />
                  </Form.Item>
                </Col>
                <Col xs={24} md={8}>
                  <Form.Item name="schedule_time" label={t("asrTasks.tasks.time")}>
                    <Input type="time" />
                  </Form.Item>
                </Col>
              </>
            ) : null}
            <Col xs={24}>
              <Form.Item name="transcription_mode" label={t("asrTasks.tasks.transcriptionMode")}>
                <Select
                  data-testid="asr-transcription-mode-select"
                  options={directoryTaskModeOptions(mossPlatformSupported)}
                />
              </Form.Item>
              <Text type="secondary" style={{ display: "block", marginTop: -16, marginBottom: 16 }}>
                {showMossPrompt
                  ? t("asrTasks.tasks.mossModeHint", { note: directoryTaskModeHistoryNote() })
                  : t("asrTasks.tasks.standardModeHint", { note: directoryTaskModeHistoryNote() })}
              </Text>
              {showMossPrompt && mossPlatformSupported === false ? (
                <Text type="warning" style={{ display: "block", marginTop: -12, marginBottom: 16 }}>
                  {mossUnsupportedReason ?? t("asrTasks.tasks.mossRequiresAppleSilicon")}
                </Text>
              ) : null}
            </Col>
            {showMossPrompt ? (
              <Col xs={24}>
                <Form.Item
                  name="transcription_prompt"
                  label={t("asrTasks.tasks.mossPrompt")}
                  extra={
                    <span
                      style={{
                        display: "flex",
                        alignItems: "flex-start",
                        justifyContent: "space-between",
                        gap: 12,
                      }}
                    >
                      <span>{t("asrTasks.tasks.mossPromptHelp")}</span>
                      <span data-testid="asr-transcription-prompt-count" style={{ whiteSpace: "nowrap" }}>
                        {Array.from(transcriptionPrompt).length} / 4000
                      </span>
                    </span>
                  }
                  rules={[
                    {
                      max: 4000,
                      message: t("asrTasks.tasks.mossPromptTooLong"),
                    },
                  ]}
                >
                  <Input.TextArea
                    data-testid="asr-transcription-prompt-input"
                    rows={4}
                    maxLength={4000}
                    placeholder={t("asrTasks.tasks.mossPromptPlaceholder")}
                  />
                </Form.Item>
              </Col>
            ) : null}
            {showStandardPipeline ? (
              <>
                <Col xs={24} md={8}>
                  <Form.Item name="runtime_strategy" label={t("asrTasks.field.runtime")}>
                    <Select
                      data-testid="asr-runtime-strategy-select"
                      listHeight={420}
                      optionLabelProp="label"
                      popupMatchSelectWidth={false}
                    >
                      {runtimeStrategyOptions(t).map((option) => (
                        <Select.Option key={option.value} value={option.value} label={option.label}>
                          <Space
                            direction="vertical"
                            size={2}
                            style={{ maxWidth: 420, whiteSpace: "normal" }}
                          >
                            <Text strong>{option.label}</Text>
                            <Text type="secondary" style={{ fontSize: 12, lineHeight: 1.4 }}>
                              {option.description}
                            </Text>
                          </Space>
                        </Select.Option>
                      ))}
                    </Select>
                  </Form.Item>
                </Col>
                <Col xs={24} md={8}>
                  <Form.Item name="max_concurrent_files" label={t("asrTasks.tasks.fileConcurrency")}>
                    <InputNumber min={1} max={16} style={{ width: "100%" }} />
                  </Form.Item>
                </Col>
                <Col xs={12} md={8}>
                  <Form.Item
                    name="diarization_enabled"
                    label={t("asrTasks.field.speakerDiarization")}
                    valuePropName="checked"
                  >
                    <Switch />
                  </Form.Item>
                </Col>
                <Col xs={12} md={8}>
                  <Form.Item name="diarization_profile" label={t("asrTasks.tasks.diarizationProfile")}>
                    <Select
                      options={[
                        { value: "sherpa-onnx-balanced", label: "sherpa-onnx balanced" },
                        { value: "pyannote-community-quality", label: "pyannote community quality" },
                      ]}
                    />
                  </Form.Item>
                </Col>
                <Col xs={12} md={8}>
                  <Form.Item name="diarization_known_speaker_count" label={t("asrTasks.tasks.knownSpeakers")}>
                    <InputNumber
                      min={1}
                      max={8}
                      placeholder={t("asrTasks.tasks.auto")}
                      style={{ width: "100%" }}
                    />
                  </Form.Item>
                </Col>
                <Col xs={12} md={8}>
                  <Form.Item
                    name="voiceprint_matching"
                    label={t("asrTasks.tasks.voiceprintMatching")}
                    valuePropName="checked"
                  >
                    <Switch />
                  </Form.Item>
                </Col>
                <Col xs={24} md={8}>
                  <Form.Item name="model" label={t("asrTasks.tasks.taskModel")}>
                    <Select
                      data-testid="asr-task-model-select"
                      options={[
                        { value: "Qwen3-ASR-0.6B", label: "Qwen3-ASR-0.6B" },
                        { value: "Qwen3-ASR-1.7B", label: "Qwen3-ASR-1.7B" },
                      ]}
                    />
                  </Form.Item>
                </Col>
                <Col xs={24} md={8}>
                  <Form.Item name="language" label={t("asrTasks.tasks.taskLanguage")}>
                    <Select
                      options={[
                        { value: "chinese", label: t("asrTasks.tasks.language.chinese") },
                        { value: "english", label: t("asrTasks.tasks.language.english") },
                        { value: "auto", label: t("asrTasks.tasks.auto") },
                      ]}
                    />
                  </Form.Item>
                </Col>
              </>
            ) : null}
            <Col xs={12} md={8}>
              <Form.Item name="recursive" label={t("asrTasks.tasks.recursive")} valuePropName="checked">
                <Switch />
              </Form.Item>
            </Col>
            <Col xs={12} md={8}>
              <Form.Item name="enabled" label={t("common.enabled")} valuePropName="checked">
                <Switch />
              </Form.Item>
            </Col>
            <Col xs={24}>
              <Form.Item name="external_devices" label={t("asrTasks.tasks.externalDevices")}>
                <Input.TextArea
                  rows={2}
                  placeholder={t("asrTasks.tasks.externalDevicesPlaceholder")}
                />
              </Form.Item>
              <Text type="secondary" style={{ fontSize: 12 }}>
                {t("asrTasks.tasks.externalDevicesHelp")}
              </Text>
            </Col>
          </Row>
        </Form>
      </Modal>
      <Table
        rowKey="id"
        size="small"
        loading={tasksLoading}
        dataSource={tasks}
        pagination={false}
        columns={[
          {
            title: t("asrTasks.field.task"),
            dataIndex: "name",
            render: (_value, record) => (
              <Space direction="vertical" size={0}>
                <Text strong>{record.name}</Text>
                <Button
                  type="link"
                  size="small"
                  style={{ padding: 0, height: "auto", alignSelf: "flex-start" }}
                  onClick={() => onOpenTask(record.id)}
                >
                  {t("asrTasks.tasks.viewDetails")}
                </Button>
                <Text type="secondary" style={{ fontSize: 12 }}>
                  {record.audio_dir}
                </Text>
                <Space size={4} wrap>
                  <Tag>{record.model}</Tag>
                  <Tag>{record.language}</Tag>
                  <Tag>
                    {t("asrTasks.tasks.filesConcurrency", {
                      effective:
                        record.summary.effective_max_concurrent_files ??
                        record.max_concurrent_files ??
                        1,
                      max: record.max_concurrent_files ?? 1,
                    })}
                  </Tag>
                </Space>
              </Space>
            ),
          },
          {
            title: t("asrTasks.tasks.progress"),
            render: (_value, record) => {
              const total = record.summary.processed + record.summary.pending;
              const percent = total ? Math.round((record.summary.processed / total) * 100) : 0;
              return (
                <Space direction="vertical" style={{ width: "100%" }} size={0}>
                  <Progress percent={percent} size="small" />
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {t("asrTasks.tasks.progressSummary", {
                      processed: record.summary.processed,
                      pending: record.summary.pending,
                      failed: record.summary.failed,
                      partial:
                        record.summary.partial_success > 0
                          ? t("asrTasks.tasks.progressPartial", {
                              partial: record.summary.partial_success,
                              chunks: record.summary.failed_chunk_count,
                            })
                          : "",
                      deleted: record.summary.deleted_after_processing,
                    })}
                  </Text>
                </Space>
              );
            },
          },
          {
            title: t("asrTasks.field.schedule"),
            render: (_value, record) => (
              <Space direction="vertical" size={0}>
                <Text>{record.enabled ? formatSchedule(record.schedule) : t("common.disabled")}</Text>
                <Text type="secondary" style={{ fontSize: 12 }}>
                  {t("asrTasks.tasks.nextRun", { time: formatTime(record.next_run_at_ms) })}
                </Text>
                <Tag>{record.runtime_strategy}</Tag>
                <Tag color={record.transcription_mode === "moss_joint" ? "cyan" : "default"}>
                  {record.transcription_mode === "moss_joint"
                    ? t("asrTasks.tasks.mode.mossJoint")
                    : t("asrTasks.tasks.mode.standard")}
                </Tag>
                {record.summary.effective_max_concurrent_files !== record.max_concurrent_files ? (
                  <Tag color="warning">{t("asrTasks.tasks.effectiveOne")}</Tag>
                ) : null}
                {record.transcription_mode !== "moss_joint" && record.diarization?.enabled ? (
                  <Tag color={record.summary.diarization_ready ? "purple" : "warning"}>
                    {record.summary.diarization_ready
                      ? t("asrTasks.tasks.speakersReady")
                      : t("asrTasks.tasks.speakersSetup")}
                  </Tag>
                ) : null}
              </Space>
            ),
          },
          {
            title: t("common.status"),
            render: (_value, record) => (
              <Space direction="vertical" size={0}>
                <Tag
                  color={
                    record.paused
                      ? "warning"
                      : record.summary.running
                        ? "processing"
                        : record.last_error
                          ? "error"
                          : "success"
                  }
                >
                  {pauseStatusLabel(t, record)}
                </Tag>
                {record.last_error ? (
                  <Text type="danger" style={{ fontSize: 12 }}>
                    {record.last_error}
                  </Text>
                ) : null}
              </Space>
            ),
          },
          {
            title: t("common.actions"),
            render: (_value, record) => {
              const importProgress = externalImportProgressByTask[record.id];
              const importing = importProgress?.status === "importing";
              return (
                <Space wrap>
                  <Button
                    size="small"
                    disabled={record.summary.running || Boolean(record.paused)}
                    onClick={() => onRunTask(record.id)}
                  >
                    {record.summary.running ? t("asrTasks.tasks.runningEllipsis") : t("asrTasks.field.run")}
                  </Button>
                  {record.external_devices?.length ? (
                    <Button
                      size="small"
                      icon={<ImportOutlined />}
                      loading={importing}
                      disabled={record.summary.running || importing}
                      title={
                        record.summary.running
                          ? t("asrTasks.tasks.pauseBeforeImport")
                          : undefined
                      }
                      onClick={() => onRunExternalImport(record.id)}
                    >
                      {importing ? t("asrTasks.tasks.importing") : t("asrTasks.tasks.importExternal")}
                    </Button>
                  ) : null}
                  {record.paused ? (
                    <Button
                      size="small"
                      type="primary"
                      icon={<PlayCircleOutlined />}
                      onClick={() => onResumeTask(record.id)}
                    >
                      {t("common.resume")}
                    </Button>
                  ) : (
                    <Dropdown
                      trigger={["click"]}
                      menu={{
                        items: [
                          {
                            key: "temporary",
                            label: t("asrTasks.pause.untilNextSchedule"),
                            icon: <PauseCircleOutlined />,
                          },
                          {
                            key: "long_term",
                            label: t("asrTasks.pause.indefinitely"),
                            icon: <StopOutlined />,
                          },
                        ],
                        onClick: ({ key }) =>
                          onPauseTask(record.id, false, key as AsrPauseMode),
                      }}
                    >
                      <Button size="small" icon={<PauseCircleOutlined />}>
                        {t("common.pause")}
                      </Button>
                    </Dropdown>
                  )}
                  {record.summary.running && !record.paused ? (
                    <Popconfirm
                      title={t("asrTasks.pause.forceConfirmTitle")}
                      description={t("asrTasks.pause.forceConfirmDescription")}
                      onConfirm={() => onPauseTask(record.id, true, "long_term")}
                    >
                      <Button size="small" danger icon={<StopOutlined />}>
                        {t("asrTasks.pause.force")}
                      </Button>
                    </Popconfirm>
                  ) : null}
                  <Button
                    size="small"
                    icon={<SettingOutlined />}
                    onClick={() => openEdit(record)}
                  >
                    {t("common.edit")}
                  </Button>
                  <Button
                    size="small"
                    danger
                    onClick={() => {
                      setDeleteTarget(record);
                      setDeleteConfirmName("");
                    }}
                  >
                    {t("common.delete")}
                  </Button>
                </Space>
              );
            },
          },
        ]}
        expandable={{
          expandedRowKeys: expandedImportTaskIds,
          rowExpandable: (record) => Boolean(externalImportProgressByTask[record.id]),
          showExpandColumn: false,
          expandedRowRender: (record) => {
            const importProgress = externalImportProgressByTask[record.id];
            if (!importProgress) {
              return null;
            }
            const currentFileCopy = externalImportCurrentFileCopyLabel(t, importProgress);
            return (
              <div style={{ padding: "8px 16px 12px" }}>
                <Progress
                  size="small"
                  percent={externalImportPercent(importProgress)}
                  status={externalImportProgressStatus(importProgress)}
                />
                <Space wrap size={[12, 6]} style={{ marginTop: 8 }}>
                  <Text strong>{externalImportProgressLabel(t, importProgress)}</Text>
                  <Text type="secondary">
                    {t("asrTasks.import.currentFile", {
                      name: externalImportCurrentFileName(importProgress),
                    })}
                  </Text>
                  <Tag>
                    {t("asrTasks.import.scannedTotal", {
                      count: importProgress.total_files_discovered,
                    })}
                  </Tag>
                  <Tag color="success">
                    {t("asrTasks.import.imported", { count: importProgress.imported })}
                  </Tag>
                  {importProgress.processed_record_skipped > 0 ? (
                    <Tag color="blue">
                      {t("asrTasks.import.alreadyProcessed", {
                        count: importProgress.processed_record_skipped,
                      })}
                    </Tag>
                  ) : null}
                  <Tag>
                    {t("asrTasks.import.processed", { count: importProgress.processed_files })}
                  </Tag>
                  {importProgress.failed > 0 ? (
                    <Tag color="error">
                      {t("asrTasks.import.failed", { count: importProgress.failed })}
                    </Tag>
                  ) : null}
                  {currentFileCopy ? <Text type="secondary">{currentFileCopy}</Text> : null}
                </Space>
              </div>
            );
          },
        }}
      />
      <Modal
        title={t("asrTasks.tasks.deleteTitle")}
        open={Boolean(deleteTarget)}
        okText={t("common.delete")}
        okButtonProps={{
          danger: true,
          disabled: !deleteTarget || deleteConfirmName !== deleteTarget.name,
        }}
        onOk={() => {
          if (!deleteTarget) return;
          onRemoveTask(deleteTarget.id, deleteConfirmName);
          setDeleteTarget(null);
          setDeleteConfirmName("");
        }}
        onCancel={() => {
          setDeleteTarget(null);
          setDeleteConfirmName("");
        }}
      >
        <Space direction="vertical" style={{ width: "100%" }}>
          <Text>
            {t("asrTasks.tasks.deleteWarning")}
          </Text>
          {deleteTarget ? (
            <>
              <Text strong>{deleteTarget.name}</Text>
              <Text type="secondary">{deleteTarget.audio_dir}</Text>
            </>
          ) : null}
          <Input
            value={deleteConfirmName}
            placeholder={t("asrTasks.field.typeTaskName")}
            onChange={(event) => setDeleteConfirmName(event.target.value)}
          />
        </Space>
      </Modal>
      <Modal
        title={
          volumePrompt && volumePrompt.volumes.length === 1
            ? t("asrTasks.tasks.deviceDetectedOne")
            : t("asrTasks.tasks.deviceDetectedOther")
        }
        open={Boolean(volumePrompt)}
        okText={t("common.add")}
        cancelText={t("asrTasks.tasks.skip")}
        confirmLoading={volumePromptLoading}
        onOk={() => void addPromptedVolumes()}
        onCancel={closeVolumePrompt}
        destroyOnClose
      >
        {volumePrompt ? (
          <Space direction="vertical" size={8} style={{ width: "100%" }}>
            <Text type="secondary">
              {volumePrompt.task
                ? t("asrTasks.tasks.addDeviceAndImport")
                : t("asrTasks.tasks.addDeviceToConfig")}
            </Text>
            {volumePrompt.volumes.map((volume) => (
              <Card key={`${volume.name}:${volume.mount_path}`} size="small">
                <Space direction="vertical" size={2}>
                  <Text strong>{volume.name}</Text>
                  <Text type="secondary">{volume.mount_path}</Text>
                </Space>
              </Card>
            ))}
          </Space>
        ) : null}
      </Modal>
    </Card>
  );
}

function defaultTaskFormValues() {
  return {
    recursive: true,
    enabled: true,
    schedule_kind: "daily",
    schedule_time: "02:00",
    schedule_weekday: 1,
    schedule_day: 1,
    schedule_minute: 0,
    runtime_strategy: "reuse_per_file",
    transcription_mode: "standard",
    transcription_prompt: "",
    max_concurrent_files: 1,
    diarization_enabled: true,
    diarization_profile: "sherpa-onnx-balanced",
    diarization_known_speaker_count: undefined,
    voiceprint_matching: true,
    model: "Qwen3-ASR-0.6B",
    language: "chinese",
    external_devices: "",
  };
}

function taskToFormValues(task: AsrDirectoryTask) {
  const scheduleValues =
    task.schedule.kind === "hourly"
      ? {
          schedule_kind: "hourly",
          schedule_minute: task.schedule.minute,
        }
      : task.schedule.kind === "weekly"
        ? {
            schedule_kind: "weekly",
            schedule_weekday: task.schedule.weekday,
            schedule_time: `${String(task.schedule.hour).padStart(2, "0")}:${String(
              task.schedule.minute,
            ).padStart(2, "0")}`,
          }
        : task.schedule.kind === "monthly"
          ? {
              schedule_kind: "monthly",
              schedule_day: task.schedule.day,
              schedule_time: `${String(task.schedule.hour).padStart(2, "0")}:${String(
                task.schedule.minute,
              ).padStart(2, "0")}`,
            }
          : {
              schedule_kind: "daily",
              schedule_time: `${String(task.schedule.hour).padStart(2, "0")}:${String(
                task.schedule.minute,
              ).padStart(2, "0")}`,
            };
  return {
    ...defaultTaskFormValues(),
    ...scheduleValues,
    name: task.name,
    audio_dir: task.audio_dir,
    recursive: task.recursive,
    enabled: task.enabled,
    model: task.model,
    language: task.language,
    runtime_strategy: task.runtime_strategy,
    transcription_mode: task.transcription_mode ?? "standard",
    transcription_prompt: task.transcription_prompt ?? "",
    max_concurrent_files: task.max_concurrent_files ?? 1,
    diarization_enabled: task.diarization?.enabled ?? true,
    diarization_profile: task.diarization?.profile ?? "sherpa-onnx-balanced",
    diarization_known_speaker_count: task.diarization?.known_speaker_count,
    voiceprint_matching: task.diarization?.voiceprint_matching ?? true,
    external_devices: (task.external_devices ?? []).map((device) => device.name).join("\n"),
  };
}
