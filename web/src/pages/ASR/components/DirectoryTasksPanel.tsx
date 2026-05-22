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
} from "../../../api/asr";
import { listAsrExternalVolumes } from "../../../api/asr";
import { formatSchedule, formatTime } from "../asrUtils";

const { Text } = Typography;

function pauseStatusLabel(task: AsrDirectoryTask): string {
  if (!task.paused) {
    return task.summary.running ? "Running" : task.last_error ? "Error" : "Ready";
  }
  if (task.summary.running) {
    return "Pausing";
  }
  return task.next_run_at_ms ? "Paused until schedule" : "Paused";
}

const RUNTIME_STRATEGY_OPTIONS: Array<{
  value: AsrRuntimeStrategy;
  label: string;
  description: string;
}> = [
  {
    value: "reuse_per_file",
    label: "Reuse / file",
    description:
      "Default for most offline tasks. Reuses one managed ASR server within each file, then releases it at the file boundary.",
  },
  {
    value: "fork_per_chunk",
    label: "Fork / chunk",
    description:
      "Most isolated mode. Starts a fresh ASR process for every chunk, which is slower but useful when debugging stability issues.",
  },
  {
    value: "reuse_server",
    label: "Reuse server",
    description:
      "Keeps one ASR server for the whole task run. Faster when stable, but can carry memory or server-state issues across files.",
  },
  {
    value: "auto",
    label: "Auto fallback",
    description:
      "Starts with server reuse and automatically falls back to isolated chunk processing after server errors or clear speed regression.",
  },
  {
    value: "compare",
    label: "Compare",
    description:
      "Diagnostic mode. Runs both isolated and server paths for each chunk, keeps the isolated output, and records performance differences.",
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

function externalImportProgressLabel(progress: AsrExternalImportRunProgress): string {
  if (progress.status === "importing") {
    if (progress.current_device) {
      return `${progress.current_device}: processed ${progress.processed_files}/${progress.total_files_discovered}`;
    }
    return "Import queued";
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

function externalImportCurrentFileCopyLabel(progress: AsrExternalImportRunProgress): string | null {
  if (!progress.current_file_size || progress.current_file_size <= 0) {
    return null;
  }
  const copied = Math.min(progress.current_file_copied_bytes, progress.current_file_size);
  const percent = Math.max(
    0,
    Math.min(100, Math.round((copied / progress.current_file_size) * 100)),
  );
  return `Current file ${percent}%`;
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
  const promptedVolumeKeysRef = useRef(new Set<string>());
  const volumePromptOpenRef = useRef(false);

  const configOpen = createOpen || Boolean(editingTask);
  const configTitle = editingTask
    ? `Edit Directory Task: ${editingTask.name}`
    : "New Directory Task";
  const expandedImportTaskIds = tasks
    .filter((task) => {
      const progress = externalImportProgressByTask[task.id];
      return progress ? shouldShowExternalImportProgress(progress) : false;
    })
    .map((task) => task.id);

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
          <span>Directory Tasks</span>
          <Tag>{tasks.length}</Tag>
        </Space>
      }
      extra={
        <Button
          type="primary"
          icon={<PlusOutlined />}
          onClick={openCreate}
        >
          New
        </Button>
      }
      style={{ marginTop: 16 }}
    >
      <Modal
        title={configTitle}
        open={configOpen}
        okText={editingTask ? "Save" : "Create"}
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
          }}
        >
          <Row gutter={[12, 0]}>
            <Col xs={24} md={8}>
              <Form.Item name="name" label="Name">
                <Input placeholder="Meeting audio watcher" />
              </Form.Item>
            </Col>
            <Col xs={24} md={16}>
              <Form.Item
                name="audio_dir"
                label="Audio Directory"
                rules={[{ required: true, message: "Enter a local directory path" }]}
              >
                <Input placeholder="~/Recordings" />
              </Form.Item>
            </Col>
            <Col xs={24} md={8}>
              <Form.Item name="schedule_kind" label="Cycle">
                <Select
                  options={[
                    { value: "hourly", label: "Hourly" },
                    { value: "daily", label: "Daily" },
                    { value: "weekly", label: "Weekly" },
                    { value: "monthly", label: "Monthly" },
                  ]}
                />
              </Form.Item>
            </Col>
            {taskScheduleKind === "hourly" ? (
              <Col xs={24} md={8}>
                <Form.Item name="schedule_minute" label="Minute">
                  <InputNumber min={0} max={59} style={{ width: "100%" }} />
                </Form.Item>
              </Col>
            ) : null}
            {taskScheduleKind === "daily" ? (
              <Col xs={24} md={8}>
                <Form.Item name="schedule_time" label="Time">
                  <Input type="time" />
                </Form.Item>
              </Col>
            ) : null}
            {taskScheduleKind === "weekly" ? (
              <>
                <Col xs={24} md={8}>
                  <Form.Item name="schedule_weekday" label="Weekday">
                    <Select
                      options={[
                        { value: 1, label: "Mon" },
                        { value: 2, label: "Tue" },
                        { value: 3, label: "Wed" },
                        { value: 4, label: "Thu" },
                        { value: 5, label: "Fri" },
                        { value: 6, label: "Sat" },
                        { value: 7, label: "Sun" },
                      ]}
                    />
                  </Form.Item>
                </Col>
                <Col xs={24} md={8}>
                  <Form.Item name="schedule_time" label="Time">
                    <Input type="time" />
                  </Form.Item>
                </Col>
              </>
            ) : null}
            {taskScheduleKind === "monthly" ? (
              <>
                <Col xs={24} md={8}>
                  <Form.Item name="schedule_day" label="Day">
                    <InputNumber min={1} max={31} style={{ width: "100%" }} />
                  </Form.Item>
                </Col>
                <Col xs={24} md={8}>
                  <Form.Item name="schedule_time" label="Time">
                    <Input type="time" />
                  </Form.Item>
                </Col>
              </>
            ) : null}
            <Col xs={24} md={8}>
              <Form.Item name="runtime_strategy" label="Runtime">
                <Select
                  data-testid="asr-runtime-strategy-select"
                  listHeight={420}
                  optionLabelProp="label"
                  popupMatchSelectWidth={false}
                >
                  {RUNTIME_STRATEGY_OPTIONS.map((option) => (
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
              <Form.Item name="model" label="Task Model">
                <Select
                  options={[
                    { value: "Qwen3-ASR-1.7B", label: "Qwen3-ASR-1.7B" },
                    { value: "Qwen3-ASR-0.6B", label: "Qwen3-ASR-0.6B" },
                  ]}
                />
              </Form.Item>
            </Col>
            <Col xs={24} md={8}>
              <Form.Item name="language" label="Task Language">
                <Select
                  options={[
                    { value: "chinese", label: "Chinese" },
                    { value: "english", label: "English" },
                    { value: "auto", label: "Auto" },
                  ]}
                />
              </Form.Item>
            </Col>
            <Col xs={12} md={8}>
              <Form.Item name="recursive" label="Recursive" valuePropName="checked">
                <Switch />
              </Form.Item>
            </Col>
            <Col xs={12} md={8}>
              <Form.Item name="enabled" label="Enabled" valuePropName="checked">
                <Switch />
              </Form.Item>
            </Col>
            <Col xs={24}>
              <Form.Item name="external_devices" label="External Devices">
                <Input.TextArea
                  rows={2}
                  placeholder="Optional device names, one per line or comma-separated, e.g. LEFT, RIGHT"
                />
              </Form.Item>
              <Text type="secondary" style={{ fontSize: 12 }}>
                Matching mounted devices are imported into Audio Directory under a root folder with
                the same device name. Imported files keep their original relative paths.
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
            title: "Task",
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
                  View details
                </Button>
                <Text type="secondary" style={{ fontSize: 12 }}>
                  {record.audio_dir}
                </Text>
                <Space size={4} wrap>
                  <Tag>{record.model}</Tag>
                  <Tag>{record.language}</Tag>
                </Space>
              </Space>
            ),
          },
          {
            title: "Progress",
            render: (_value, record) => {
              const total = record.summary.processed + record.summary.pending;
              const percent = total ? Math.round((record.summary.processed / total) * 100) : 0;
              return (
                <Space direction="vertical" style={{ width: "100%" }} size={0}>
                  <Progress percent={percent} size="small" />
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    processed {record.summary.processed}, pending {record.summary.pending},
                    failed {record.summary.failed}
                    {record.summary.partial_success > 0
                      ? `, partial ${record.summary.partial_success} (${record.summary.failed_chunk_count} chunks)`
                      : ""}
                    , deleted after processing {record.summary.deleted_after_processing}
                  </Text>
                </Space>
              );
            },
          },
          {
            title: "Schedule",
            render: (_value, record) => (
              <Space direction="vertical" size={0}>
                <Text>{record.enabled ? formatSchedule(record.schedule) : "Disabled"}</Text>
                <Text type="secondary" style={{ fontSize: 12 }}>
                  next {formatTime(record.next_run_at_ms)}
                </Text>
                <Tag>{record.runtime_strategy}</Tag>
              </Space>
            ),
          },
          {
            title: "Status",
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
                  {pauseStatusLabel(record)}
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
            title: "Actions",
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
                    {record.summary.running ? "Running..." : "Run"}
                  </Button>
                  {record.external_devices?.length ? (
                    <Button
                      size="small"
                      icon={<ImportOutlined />}
                      loading={importing}
                      disabled={record.summary.running || importing}
                      title={
                        record.summary.running
                          ? "Pause the ASR run before importing external files."
                          : undefined
                      }
                      onClick={() => onRunExternalImport(record.id)}
                    >
                      {importing ? "Importing" : "Import External"}
                    </Button>
                  ) : null}
                  {record.paused ? (
                    <Button
                      size="small"
                      type="primary"
                      icon={<PlayCircleOutlined />}
                      onClick={() => onResumeTask(record.id)}
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
                          onPauseTask(record.id, false, key as AsrPauseMode),
                      }}
                    >
                      <Button size="small" icon={<PauseCircleOutlined />}>
                        Pause
                      </Button>
                    </Dropdown>
                  )}
                  {record.summary.running && !record.paused ? (
                    <Popconfirm
                      title="Force pause this ASR task?"
                      description="The current native ASR process will be terminated and the file will resume from pending later."
                      onConfirm={() => onPauseTask(record.id, true, "long_term")}
                    >
                      <Button size="small" danger icon={<StopOutlined />}>
                        Force Pause
                      </Button>
                    </Popconfirm>
                  ) : null}
                  <Button
                    size="small"
                    icon={<SettingOutlined />}
                    onClick={() => openEdit(record)}
                  >
                    Edit
                  </Button>
                  <Button
                    size="small"
                    danger
                    onClick={() => {
                      setDeleteTarget(record);
                      setDeleteConfirmName("");
                    }}
                  >
                    Delete
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
            const currentFileCopy = externalImportCurrentFileCopyLabel(importProgress);
            return (
              <div style={{ padding: "8px 16px 12px" }}>
                <Progress
                  size="small"
                  percent={externalImportPercent(importProgress)}
                  status={externalImportProgressStatus(importProgress)}
                />
                <Space wrap size={[12, 6]} style={{ marginTop: 8 }}>
                  <Text strong>{externalImportProgressLabel(importProgress)}</Text>
                  <Text type="secondary">
                    Current file: {externalImportCurrentFileName(importProgress)}
                  </Text>
                  <Tag>Scanned total: {importProgress.total_files_discovered}</Tag>
                  <Tag color="success">Imported: {importProgress.imported}</Tag>
                  {importProgress.processed_record_skipped > 0 ? (
                    <Tag color="blue">
                      Already processed: {importProgress.processed_record_skipped}
                    </Tag>
                  ) : null}
                  <Tag>Processed: {importProgress.processed_files}</Tag>
                  {importProgress.failed > 0 ? (
                    <Tag color="error">Failed: {importProgress.failed}</Tag>
                  ) : null}
                  {currentFileCopy ? <Text type="secondary">{currentFileCopy}</Text> : null}
                </Space>
              </div>
            );
          },
        }}
      />
      <Modal
        title="Delete ASR task"
        open={Boolean(deleteTarget)}
        okText="Delete"
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
            This is a destructive operation. Type the full task name to confirm.
          </Text>
          {deleteTarget ? (
            <>
              <Text strong>{deleteTarget.name}</Text>
              <Text type="secondary">{deleteTarget.audio_dir}</Text>
            </>
          ) : null}
          <Input
            value={deleteConfirmName}
            placeholder="Type task name"
            onChange={(event) => setDeleteConfirmName(event.target.value)}
          />
        </Space>
      </Modal>
      <Modal
        title={
          volumePrompt && volumePrompt.volumes.length === 1
            ? "External device detected"
            : "External devices detected"
        }
        open={Boolean(volumePrompt)}
        okText="Add"
        cancelText="Skip"
        confirmLoading={volumePromptLoading}
        onOk={() => void addPromptedVolumes()}
        onCancel={closeVolumePrompt}
        destroyOnClose
      >
        {volumePrompt ? (
          <Space direction="vertical" size={8} style={{ width: "100%" }}>
            <Text type="secondary">
              {volumePrompt.task
                ? "Add the following device to this task and import its data."
                : "Add the following device to this task configuration."}
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
    model: "Qwen3-ASR-1.7B",
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
    external_devices: (task.external_devices ?? []).map((device) => device.name).join("\n"),
  };
}
