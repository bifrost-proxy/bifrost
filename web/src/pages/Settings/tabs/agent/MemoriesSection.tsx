import { useCallback, useEffect, useState } from "react";
import { Button, Form, Input, Modal, Space, Table, Tag, Typography, Upload, message } from "antd";
import type { ColumnsType } from "antd/es/table";
import {
  DeleteOutlined,
  DownloadOutlined,
  PlusOutlined,
  SearchOutlined,
  UploadOutlined,
} from "@ant-design/icons";
import { del, get, post } from "../../../../api/client";
import type { MemoryRecord, MemoryStats } from "./types";

const { Text } = Typography;
const { TextArea } = Input;

type MemoryFormValues = {
  content: string;
};

export default function MemoriesSection() {
  const [records, setRecords] = useState<MemoryRecord[]>([]);
  const [stats, setStats] = useState<MemoryStats | null>(null);
  const [loading, setLoading] = useState(false);
  const [query, setQuery] = useState("");
  const [creating, setCreating] = useState(false);
  const [form] = Form.useForm<MemoryFormValues>();

  const fetchRecords = useCallback(async () => {
    setLoading(true);
    try {
      const params = new URLSearchParams();
      if (query.trim()) params.set("query", query.trim());
      params.set("limit", "100");
      const [list, nextStats] = await Promise.all([
        get<MemoryRecord[]>(`/agent/memories?${params.toString()}`),
        get<MemoryStats>("/agent/memories/stats"),
      ]);
      setRecords(list);
      setStats(nextStats);
    } catch {
      message.error("Failed to load memory files");
    } finally {
      setLoading(false);
    }
  }, [query]);

  useEffect(() => {
    fetchRecords();
  }, [fetchRecords]);

  const openCreate = () => {
    setCreating(true);
    form.setFieldsValue({ content: "" });
  };

  const closeModal = () => {
    setCreating(false);
    form.resetFields();
  };

  const saveMemory = async () => {
    const values = await form.validateFields();
    try {
      await post<MemoryRecord>("/agent/memories", { content: values.content });
      message.success("Memory appended");
      closeModal();
      fetchRecords();
    } catch {
      message.error("Failed to append memory");
    }
  };

  const deleteMemory = async (record: MemoryRecord) => {
    try {
      await del(`/agent/memories/${encodeURIComponent(record.id)}`);
      message.success("Memory entry deleted");
      fetchRecords();
    } catch {
      message.error("Failed to delete memory entry");
    }
  };

  const exportMemoryFiles = async () => {
    try {
      const response = await get<{ content: string; count: number }>("/agent/memories/export");
      const blob = new Blob([response.content], { type: "text/markdown" });
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = `bifrost-agent-memory-${Date.now()}.md`;
      link.click();
      URL.revokeObjectURL(url);
      message.success(`Exported ${response.count} lines`);
    } catch {
      message.error("Failed to export memory files");
    }
  };

  const columns: ColumnsType<MemoryRecord> = [
    {
      title: "Path",
      dataIndex: "path",
      width: 180,
      render: (value: string) => <Text code>{value}</Text>,
    },
    {
      title: "Content",
      dataIndex: "content",
      ellipsis: true,
    },
    {
      title: "",
      width: 56,
      render: (_, record) => (
        <Button
          danger
          icon={<DeleteOutlined />}
          size="small"
          onClick={() => deleteMemory(record)}
        />
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
          placeholder="Search memory files"
          style={{ width: 280 }}
          size="small"
        />
        <Button onClick={fetchRecords} loading={loading} size="small">
          Search
        </Button>
        <Button icon={<PlusOutlined />} type="primary" onClick={openCreate} size="small">
          Append
        </Button>
        <Upload
          showUploadList={false}
          beforeUpload={async (file) => {
            const content = await file.text();
            await post("/agent/memories/import", content, {
              headers: { "Content-Type": "text/plain" },
            });
            message.success("Memory file content imported");
            fetchRecords();
            return false;
          }}
        >
          <Button icon={<UploadOutlined />} size="small">
            Import
          </Button>
        </Upload>
        <Button icon={<DownloadOutlined />} onClick={exportMemoryFiles} size="small">
          Export
        </Button>
      </Space>

      {stats && (
        <Space wrap>
          <Tag>{stats.memory_root}</Tag>
          <Tag>summary {stats.memory_summary_bytes} B</Tag>
          <Tag>MEMORY.md {stats.memory_md_bytes} B</Tag>
          <Tag>rollouts {stats.rollout_summary_count}</Tag>
          <Tag>skills {stats.skill_count}</Tag>
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

      <Modal
        title="Append Memory"
        open={creating}
        onOk={saveMemory}
        onCancel={closeModal}
        destroyOnClose
      >
        <Form form={form} layout="vertical">
          <Form.Item name="content" label="Content" rules={[{ required: true }]}>
            <TextArea rows={5} />
          </Form.Item>
        </Form>
      </Modal>
    </Space>
  );
}
