import { useState } from "react";
import type { FormInstance } from "antd";
import {
  Button,
  Card,
  Col,
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
  PlusOutlined,
  StopOutlined,
} from "@ant-design/icons";
import type { AsrDirectoryTask } from "../../../api/asr";
import { formatSchedule, formatTime } from "../asrUtils";

const { Text } = Typography;

interface DirectoryTasksPanelProps {
  taskForm: FormInstance;
  taskScheduleKind: string;
  tasks: AsrDirectoryTask[];
  tasksLoading: boolean;
  onCreateTask: () => boolean | Promise<boolean>;
  onOpenTask: (id: string) => void;
  onRunTask: (id: string) => void;
  onPauseTask: (id: string, force?: boolean) => void;
  onResumeTask: (id: string) => void;
  onRemoveTask: (id: string) => void;
}

export default function DirectoryTasksPanel({
  taskForm,
  taskScheduleKind,
  tasks,
  tasksLoading,
  onCreateTask,
  onOpenTask,
  onRunTask,
  onPauseTask,
  onResumeTask,
  onRemoveTask,
}: DirectoryTasksPanelProps) {
  const [createOpen, setCreateOpen] = useState(false);
  const [creating, setCreating] = useState(false);

  const handleCreate = async () => {
    setCreating(true);
    try {
      const ok = await onCreateTask();
      if (ok) {
        setCreateOpen(false);
      }
    } finally {
      setCreating(false);
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
          onClick={() => setCreateOpen(true)}
        >
          New
        </Button>
      }
      style={{ marginTop: 16 }}
    >
      <Modal
        title="New Directory Task"
        open={createOpen}
        okText="Create"
        confirmLoading={creating}
        onOk={() => void handleCreate()}
        onCancel={() => setCreateOpen(false)}
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
                  options={[
                    { value: "reuse_per_file", label: "Reuse / file" },
                    { value: "fork_per_chunk", label: "Fork / chunk" },
                    { value: "reuse_server", label: "Reuse server" },
                    { value: "auto", label: "Auto fallback" },
                    { value: "compare", label: "Compare" },
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
                  {record.paused
                    ? record.summary.running
                      ? "Pausing"
                      : "Paused"
                    : record.summary.running
                      ? "Running"
                      : record.last_error
                        ? "Error"
                        : "Ready"}
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
            render: (_value, record) => (
              <Space>
                <Button
                  size="small"
                  disabled={record.summary.running || Boolean(record.paused)}
                  onClick={() => onRunTask(record.id)}
                >
                  {record.summary.running ? "Running..." : "Run"}
                </Button>
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
                  <Button
                    size="small"
                    icon={<PauseCircleOutlined />}
                    onClick={() => onPauseTask(record.id)}
                  >
                    Pause
                  </Button>
                )}
                {record.summary.running && !record.paused ? (
                  <Popconfirm
                    title="Force pause this ASR task?"
                    description="The current native ASR process will be terminated and the file will resume from pending later."
                    onConfirm={() => onPauseTask(record.id, true)}
                  >
                    <Button size="small" danger icon={<StopOutlined />}>
                      Force Pause
                    </Button>
                  </Popconfirm>
                ) : null}
                <Popconfirm
                  title="Delete this ASR task?"
                  onConfirm={() => onRemoveTask(record.id)}
                >
                  <Button size="small" danger>
                    Delete
                  </Button>
                </Popconfirm>
              </Space>
            ),
          },
        ]}
      />
    </Card>
  );
}
