import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Button,
  Checkbox,
  Form,
  Input,
  Modal,
  Select,
  Space,
  Table,
  Tag,
  Typography,
  Upload,
  message,
} from "antd";
import type { ColumnsType } from "antd/es/table";
import {
  DeleteOutlined,
  DownloadOutlined,
  EditOutlined,
  PlusOutlined,
  PushpinOutlined,
  SearchOutlined,
  UploadOutlined,
} from "@ant-design/icons";
import { del, get, patch, post } from "../../../../api/client";
import type { MemoryKind, MemoryRecord, MemoryScope, MemoryStats } from "./types";

const { Text } = Typography;
const { TextArea } = Input;

const KIND_OPTIONS: Array<{ label: string; value: MemoryKind }> = [
  { label: "Fact", value: "fact" },
  { label: "Preference", value: "preference" },
  { label: "Rule", value: "rule" },
  { label: "Skill", value: "skill" },
  { label: "Task Context", value: "task_context" },
  { label: "Other", value: "other" },
];

type MemoryFormValues = {
  content: string;
  kind: MemoryKind;
  scope_type: MemoryScope["type"];
  scope_value?: string;
  tags?: string;
  pinned?: boolean;
};

export default function MemoriesSection() {
  const [records, setRecords] = useState<MemoryRecord[]>([]);
  const [stats, setStats] = useState<MemoryStats | null>(null);
  const [loading, setLoading] = useState(false);
  const [query, setQuery] = useState("");
  const [scopeType, setScopeType] = useState<string>();
  const [kind, setKind] = useState<MemoryKind>();
  const [tag, setTag] = useState("");
  const [editing, setEditing] = useState<MemoryRecord | null>(null);
  const [creating, setCreating] = useState(false);
  const [form] = Form.useForm<MemoryFormValues>();

  const fetchRecords = useCallback(async () => {
    setLoading(true);
    try {
      const params = new URLSearchParams();
      if (query.trim()) params.set("query", query.trim());
      if (scopeType) params.set("scope_type", scopeType);
      if (kind) params.set("kind", kind);
      if (tag.trim()) params.set("tag", tag.trim());
      params.set("limit", "100");
      const [list, nextStats] = await Promise.all([
        get<MemoryRecord[]>(`/agent/memories?${params.toString()}`),
        get<MemoryStats>("/agent/memories/stats"),
      ]);
      setRecords(list);
      setStats(nextStats);
    } catch {
      message.error("Failed to load memories");
    } finally {
      setLoading(false);
    }
  }, [kind, query, scopeType, tag]);

  useEffect(() => {
    fetchRecords();
  }, [fetchRecords]);

  const recentUsed = useMemo(
    () =>
      [...records]
        .filter((record) => record.last_used_at)
        .sort((a, b) => (b.last_used_at ?? 0) - (a.last_used_at ?? 0))
        .slice(0, 5),
    [records],
  );

  const openCreate = () => {
    setCreating(true);
    setEditing(null);
    form.setFieldsValue({
      kind: "fact",
      scope_type: "global",
      content: "",
      tags: "",
      pinned: false,
    });
  };

  const openEdit = (record: MemoryRecord) => {
    setEditing(record);
    setCreating(false);
    form.setFieldsValue({
      content: record.content,
      kind: record.kind,
      scope_type: record.scope.type,
      scope_value: record.scope.value,
      tags: record.tags.join(", "),
      pinned: record.pinned,
    });
  };

  const closeModal = () => {
    setCreating(false);
    setEditing(null);
    form.resetFields();
  };

  const saveMemory = async () => {
    const values = await form.validateFields();
    const scope =
      values.scope_type === "global"
        ? { type: "global" }
        : { type: values.scope_type, value: values.scope_value || "" };
    const payload = {
      content: values.content,
      kind: values.kind,
      scope,
      tags: parseTags(values.tags),
      pinned: values.pinned ?? false,
    };
    try {
      if (editing) {
        await patch<MemoryRecord>(`/agent/memories/${editing.id}`, payload);
      } else {
        await post<MemoryRecord>("/agent/memories", payload);
      }
      message.success("Memory saved");
      closeModal();
      fetchRecords();
    } catch {
      message.error("Failed to save memory");
    }
  };

  const togglePin = async (record: MemoryRecord) => {
    try {
      await patch<MemoryRecord>(`/agent/memories/${record.id}`, {
        pinned: !record.pinned,
      });
      fetchRecords();
    } catch {
      message.error("Failed to update pin");
    }
  };

  const deleteMemory = async (record: MemoryRecord) => {
    try {
      await del(`/agent/memories/${record.id}`);
      message.success("Memory deleted");
      fetchRecords();
    } catch {
      message.error("Failed to delete memory");
    }
  };

  const exportJsonl = async () => {
    try {
      const response = await get<{ content: string; count: number }>("/agent/memories/export");
      const blob = new Blob([response.content], { type: "application/jsonl" });
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = `bifrost-memories-${Date.now()}.jsonl`;
      link.click();
      URL.revokeObjectURL(url);
      message.success(`Exported ${response.count} memories`);
    } catch {
      message.error("Failed to export memories");
    }
  };

  const columns: ColumnsType<MemoryRecord> = [
    {
      title: "",
      width: 48,
      render: (_, record) => (
        <Button
          aria-label="Pin memory"
          icon={<PushpinOutlined />}
          type={record.pinned ? "primary" : "text"}
          size="small"
          onClick={() => togglePin(record)}
        />
      ),
    },
    {
      title: "Kind",
      dataIndex: "kind",
      width: 120,
      render: (value: MemoryKind) => <Tag>{value}</Tag>,
    },
    {
      title: "Scope",
      width: 180,
      render: (_, record) => (
        <Text code style={{ fontSize: 11 }}>
          {record.scope.type}
          {record.scope.value ? `:${record.scope.value}` : ""}
        </Text>
      ),
    },
    {
      title: "Content",
      dataIndex: "content",
      ellipsis: true,
    },
    {
      title: "Tags",
      width: 180,
      render: (_, record) => record.tags.map((item) => <Tag key={item}>{item}</Tag>),
    },
    {
      title: "Used",
      width: 90,
      dataIndex: "use_count",
    },
    {
      title: "",
      width: 96,
      render: (_, record) => (
        <Space>
          <Button icon={<EditOutlined />} size="small" onClick={() => openEdit(record)} />
          <Button
            danger
            icon={<DeleteOutlined />}
            size="small"
            onClick={() => deleteMemory(record)}
          />
        </Space>
      ),
    },
  ];

  return (
    <Space direction="vertical" style={{ width: "100%" }} size="middle">
      <Space wrap>
        <Input
          prefix={<SearchOutlined />}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onPressEnter={fetchRecords}
          placeholder="Search"
          style={{ width: 240 }}
          size="small"
        />
        <Select
          allowClear
          value={scopeType}
          onChange={setScopeType}
          placeholder="Scope"
          style={{ width: 140 }}
          size="small"
          options={[
            { label: "Global", value: "global" },
            { label: "User", value: "user" },
            { label: "Project", value: "project" },
            { label: "Session", value: "session" },
          ]}
        />
        <Select
          allowClear
          value={kind}
          onChange={setKind}
          placeholder="Kind"
          style={{ width: 160 }}
          size="small"
          options={KIND_OPTIONS}
        />
        <Input
          value={tag}
          onChange={(event) => setTag(event.target.value)}
          onPressEnter={fetchRecords}
          placeholder="Tag"
          style={{ width: 140 }}
          size="small"
        />
        <Button onClick={fetchRecords} loading={loading} size="small">
          Search
        </Button>
        <Button icon={<PlusOutlined />} type="primary" onClick={openCreate} size="small">
          New
        </Button>
        <Upload
          showUploadList={false}
          beforeUpload={async (file) => {
            const content = await file.text();
            await post("/agent/memories/import", content, {
              headers: { "Content-Type": "text/plain" },
            });
            message.success("Memories imported");
            fetchRecords();
            return false;
          }}
        >
          <Button icon={<UploadOutlined />} size="small">
            Import
          </Button>
        </Upload>
        <Button icon={<DownloadOutlined />} onClick={exportJsonl} size="small">
          Export
        </Button>
      </Space>

      {stats && (
        <Space wrap>
          <Tag>Total {stats.total}</Tag>
          <Tag>Written 7d {stats.written_last_7_days}</Tag>
          <Tag>Recalled 7d {stats.recalled_last_7_days}</Tag>
        </Space>
      )}

      <Table
        rowKey="id"
        size="small"
        loading={loading}
        columns={columns}
        dataSource={records}
        pagination={{ pageSize: 12, showSizeChanger: true }}
      />

      {recentUsed.length > 0 && (
        <Space direction="vertical" style={{ width: "100%" }}>
          <Text strong>Recently Recalled</Text>
          {recentUsed.map((record) => (
            <Text key={record.id} ellipsis>
              {record.content}
            </Text>
          ))}
        </Space>
      )}

      <Modal
        title={editing ? "Edit Memory" : "New Memory"}
        open={creating || !!editing}
        onOk={saveMemory}
        onCancel={closeModal}
        destroyOnClose
      >
        <Form form={form} layout="vertical">
          <Form.Item name="content" label="Content" rules={[{ required: true }]}>
            <TextArea rows={5} />
          </Form.Item>
          <Form.Item name="kind" label="Kind" rules={[{ required: true }]}>
            <Select options={KIND_OPTIONS} />
          </Form.Item>
          <Space style={{ width: "100%" }} align="start">
            <Form.Item name="scope_type" label="Scope" rules={[{ required: true }]}>
              <Select
                style={{ width: 160 }}
                options={[
                  { label: "Global", value: "global" },
                  { label: "User", value: "user" },
                  { label: "Project", value: "project" },
                  { label: "Session", value: "session" },
                ]}
              />
            </Form.Item>
            <Form.Item name="scope_value" label="Scope Value">
              <Input style={{ width: 260 }} />
            </Form.Item>
          </Space>
          <Form.Item name="tags" label="Tags">
            <Input placeholder="tag-one, tag-two" />
          </Form.Item>
          <Form.Item name="pinned" valuePropName="checked">
            <Checkbox>Pinned</Checkbox>
          </Form.Item>
        </Form>
      </Modal>
    </Space>
  );
}

function parseTags(input?: string): string[] {
  return (input ?? "")
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}
