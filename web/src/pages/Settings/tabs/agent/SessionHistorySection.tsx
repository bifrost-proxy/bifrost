/**
 * Session History Section - Manage persisted JSONL session files
 */
import { useCallback, useEffect, useState } from "react";
import {
  Button,
  Col,
  Empty,
  Popconfirm,
  Row,
  Space,
  Table,
  Tag,
  Tooltip,
  Typography,
  message,
} from "antd";
import {
  DeleteOutlined,
  EyeOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import { get, del } from "../../../../api/client";
import { BASE, type HistoryFileInfo } from "./types";

const { Text } = Typography;

interface SessionHistorySectionProps {
  onOpenSession?: (sessionKey: string, view: "active" | "history", filePath?: string) => void;
}

export default function SessionHistorySection({ onOpenSession }: SessionHistorySectionProps) {
  const [files, setFiles] = useState<HistoryFileInfo[]>([]);
  const [loading, setLoading] = useState(false);

  const fetchHistory = useCallback(async () => {
    setLoading(true);
    try {
      const data = await get<{ history: HistoryFileInfo[]; total: number }>(
        `${BASE}/agent/sessions/history`,
      );
      setFiles(data.history || []);
    } catch {
      // silent
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchHistory();
  }, [fetchHistory]);

  const handleDelete = async (file: HistoryFileInfo) => {
    try {
      await del(`${BASE}/agent/sessions/history/${encodeURIComponent(file.path)}`);
      message.success("History file deleted");
      fetchHistory();
    } catch {
      message.error("Failed to delete history file");
    }
  };

  const formatTs = (ts?: number) => {
    if (!ts) return "-";
    return new Date(ts * 1000).toLocaleString();
  };

  const columns = [
    {
      title: "Session Key",
      dataIndex: "session_key",
      key: "session_key",
      ellipsis: true,
      render: (val: string) => (
        <Text code style={{ fontSize: 11 }}>
          {val}
        </Text>
      ),
    },
    {
      title: "Source",
      dataIndex: "source",
      key: "source",
      width: 80,
      render: (val?: string) => {
        if (val === "feishu") return <Tag color="blue" style={{ fontSize: 10 }}>Feishu</Tag>;
        if (val === "api") return <Tag color="green" style={{ fontSize: 10 }}>API</Tag>;
        return <Tag style={{ fontSize: 10 }}>{val || "—"}</Tag>;
      },
    },
    {
      title: "Work Dir",
      dataIndex: "work_dir",
      key: "work_dir",
      ellipsis: true,
      width: 150,
      render: (val?: string) =>
        val ? (
          <Tooltip title={val}>
            <Text code style={{ fontSize: 10 }}>{val}</Text>
          </Tooltip>
        ) : (
          <Text type="secondary" style={{ fontSize: 10 }}>default</Text>
        ),
    },
    {
      title: "Turns",
      key: "turns",
      width: 80,
      render: (_: unknown, record: HistoryFileInfo) => (
        <Tooltip title={`${record.user_turns ?? 0} user / ${record.assistant_turns ?? 0} assistant`}>
          <Text style={{ fontSize: 11 }}>{(record.user_turns ?? 0) + (record.assistant_turns ?? 0)}</Text>
        </Tooltip>
      ),
    },
    {
      title: "Tokens",
      dataIndex: "total_tokens",
      key: "total_tokens",
      width: 90,
      render: (val?: number) => (val ? val.toLocaleString() : "-"),
    },
    {
      title: "Started",
      dataIndex: "start_time",
      key: "start_time",
      width: 160,
      render: (val?: number) => formatTs(val),
    },
    {
      title: "Duration",
      dataIndex: "duration_secs",
      key: "duration_secs",
      width: 90,
      render: (val?: number) => {
        if (!val) return "-";
        if (val < 60) return `${val}s`;
        if (val < 3600) return `${Math.floor(val / 60)}m ${val % 60}s`;
        return `${Math.floor(val / 3600)}h ${Math.floor((val % 3600) / 60)}m`;
      },
    },
    {
      title: "Actions",
      key: "actions",
      width: 100,
      render: (_: unknown, record: HistoryFileInfo) => (
        <Space size="small">
          <Tooltip title="View details">
            <Button
              size="small"
              icon={<EyeOutlined />}
              onClick={() => onOpenSession?.(record.session_key, "history", record.path)}
            />
          </Tooltip>
          <Popconfirm
            title="Delete this history file?"
            onConfirm={() => handleDelete(record)}
          >
            <Button size="small" danger icon={<DeleteOutlined />} />
          </Popconfirm>
        </Space>
      ),
    },
  ];

  return (
    <Space direction="vertical" style={{ width: "100%" }}>
      <Row justify="space-between" align="middle">
        <Col>
          <Text type="secondary" style={{ fontSize: 12 }}>
            {files.length} persisted session{files.length !== 1 ? "s" : ""}
          </Text>
        </Col>
        <Col>
          <Button
            icon={<ReloadOutlined />}
            size="small"
            onClick={fetchHistory}
            loading={loading}
          >
            Refresh
          </Button>
        </Col>
      </Row>
      <Table
        dataSource={files}
        columns={columns}
        rowKey="path"
        size="small"
        loading={loading}
        pagination={{ pageSize: 10, size: "small" }}
        locale={{ emptyText: <Empty description="No persisted sessions" /> }}
        scroll={{ x: 1200 }}
      />
    </Space>
  );
}
