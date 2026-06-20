import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Button,
  Form,
  Grid,
  Input,
  Progress,
  Space,
  Table,
  Tag,
  Typography,
  message,
  theme,
} from "antd";
import type { ColumnsType } from "antd/es/table";
import {
  DownloadOutlined,
  ExportOutlined,
  FolderOpenOutlined,
  LinkOutlined,
  PlayCircleOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import {
  createVideoDownload,
  getVideoDefaults,
  listVideoDownloads,
  openVideoFile,
  revealVideoFile,
  retryVideoDownload,
  videoFileUrl,
  type VideoDownloadStatus,
  type VideoDownloadTask,
} from "../../api/videos";

const { Text } = Typography;
const { useBreakpoint } = Grid;

interface VideoFormValues {
  url: string;
  download_dir: string;
}

const statusColor: Record<VideoDownloadStatus, string> = {
  queued: "default",
  running: "processing",
  completed: "success",
  failed: "error",
};

export default function VideosTool() {
  const [form] = Form.useForm<VideoFormValues>();
  const { token } = theme.useToken();
  const [defaultDir, setDefaultDir] = useState("");
  const [downloads, setDownloads] = useState<VideoDownloadTask[]>([]);
  const [loading, setLoading] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const screens = useBreakpoint();
  const isCompact = !screens.md;

  const running = useMemo(
    () =>
      downloads.some(
        (download) => download.status === "queued" || download.status === "running",
      ),
    [downloads],
  );

  const refreshDownloads = useCallback(async () => {
    setLoading(true);
    try {
      const data = await listVideoDownloads();
      setDownloads(data.downloads || []);
    } catch (error) {
      message.error(
        error instanceof Error ? error.message : "Failed to load video downloads",
      );
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let alive = true;
    void getVideoDefaults()
      .then((data) => {
        if (!alive) return;
        setDefaultDir(data.download_dir);
        form.setFieldsValue({ download_dir: data.download_dir });
      })
      .catch((error) => {
        message.error(error instanceof Error ? error.message : "Failed to load video defaults");
      });
    void refreshDownloads();
    return () => {
      alive = false;
    };
  }, [form, refreshDownloads]);

  useEffect(() => {
    if (!running) return undefined;
    const timer = window.setInterval(() => {
      void refreshDownloads();
    }, 1000);
    return () => window.clearInterval(timer);
  }, [refreshDownloads, running]);

  const handleSubmit = useCallback(
    async (values: VideoFormValues) => {
      setSubmitting(true);
      try {
        const task = await createVideoDownload({
          url: values.url.trim(),
          download_dir: values.download_dir.trim(),
        });
        setDownloads((prev) => [
          task,
          ...prev.filter((item) => item.id !== task.id),
        ]);
        form.setFieldsValue({ url: "" });
        message.success("Download started");
      } catch (error) {
        message.error(error instanceof Error ? error.message : "Download failed");
      } finally {
        setSubmitting(false);
      }
    },
    [form],
  );

  const handlePlay = useCallback((record: VideoDownloadTask) => {
    window.open(videoFileUrl(record.id), "_blank", "noopener,noreferrer");
  }, []);

  const handleOpenFile = useCallback(async (record: VideoDownloadTask) => {
    try {
      await openVideoFile(record.id);
      message.success("Opening video file");
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to open video file");
    }
  }, []);

  const handleRevealFile = useCallback(async (record: VideoDownloadTask) => {
    try {
      await revealVideoFile(record.id);
      message.success("Opening folder");
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to open folder");
    }
  }, []);

  const handleRetry = useCallback(async (record: VideoDownloadTask) => {
    try {
      const task = await retryVideoDownload(record.id);
      setDownloads((prev) =>
        prev.map((item) => (item.id === task.id ? task : item)),
      );
      message.success("Download retry started");
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to retry download");
    }
  }, []);

  const columns = useMemo<ColumnsType<VideoDownloadTask>>(
    () => [
      {
        title: "Video",
        dataIndex: "url",
        key: "url",
        render: (_, record) => (
          <Space direction="vertical" size={2} style={{ maxWidth: 520 }}>
            <Text ellipsis={{ tooltip: record.url }}>{record.url}</Text>
            <Text
              type="secondary"
              style={{ fontSize: 12 }}
              ellipsis={{ tooltip: record.file_path || record.download_dir }}
            >
              {record.file_path || record.download_dir}
            </Text>
          </Space>
        ),
      },
      {
        title: "Progress",
        key: "progress",
        width: 320,
        render: (_, record) => {
          const percent =
            record.status === "completed"
              ? 100
              : Math.max(
                  0,
                  Math.min(100, Math.round(record.progress_percent || 0)),
                );
          return (
            <Space direction="vertical" size={2} style={{ width: "100%" }}>
              <Progress
                percent={percent}
                size="small"
                status={
                  record.status === "failed"
                    ? "exception"
                    : record.status === "completed"
                      ? "success"
                      : "active"
                }
              />
              <Text type="secondary" style={{ fontSize: 12 }}>
                {[record.total, record.speed, record.eta ? `ETA ${record.eta}` : null]
                  .filter(Boolean)
                  .join(" · ") ||
                  record.message ||
                  "queued"}
              </Text>
            </Space>
          );
        },
      },
      {
        title: "Status",
        dataIndex: "status",
        key: "status",
        width: 120,
        render: (status: VideoDownloadStatus) => (
          <Tag color={statusColor[status]}>{status}</Tag>
        ),
      },
      {
        title: "Updated",
        dataIndex: "updated_at_ms",
        key: "updated_at_ms",
        width: 180,
        render: (value: number) => new Date(value).toLocaleString(),
      },
      {
        title: "Actions",
        key: "actions",
        width: 320,
        render: (_, record) => {
          const fileDisabled = record.status !== "completed" || !record.file_path;
          const retryDisabled = record.status !== "failed";
          return (
            <Space.Compact>
              <Button
                icon={<ReloadOutlined />}
                disabled={retryDisabled}
                onClick={() => void handleRetry(record)}
              >
                Retry
              </Button>
              <Button
                icon={<PlayCircleOutlined />}
                disabled={fileDisabled}
                onClick={() => handlePlay(record)}
              >
                Play
              </Button>
              <Button
                icon={<ExportOutlined />}
                disabled={fileDisabled}
                onClick={() => void handleOpenFile(record)}
              >
                Open
              </Button>
              <Button
                icon={<FolderOpenOutlined />}
                disabled={fileDisabled}
                onClick={() => void handleRevealFile(record)}
              >
                Reveal
              </Button>
            </Space.Compact>
          );
        },
      },
    ],
    [handleOpenFile, handlePlay, handleRetry, handleRevealFile],
  );

  return (
    <div
      data-testid="videos-tool-page"
      style={{
        height: "100%",
        minHeight: 0,
        overflow: "auto",
        background: token.colorBgLayout,
        padding: 16,
      }}
    >
      <section
        style={{
          border: `1px solid ${token.colorBorderSecondary}`,
          background: token.colorBgContainer,
          borderRadius: 6,
          overflow: "hidden",
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            gap: 12,
            padding: "14px 16px",
            borderBottom: `1px solid ${token.colorBorderSecondary}`,
          }}
        >
          <Space size={8}>
            <DownloadOutlined />
            <Text strong>Videos</Text>
          </Space>
          <Button icon={<ReloadOutlined />} onClick={refreshDownloads} loading={loading}>
            Refresh
          </Button>
        </div>

        <div
          style={{
            padding: 16,
            borderBottom: `1px solid ${token.colorBorderSecondary}`,
          }}
        >
          <Form
            form={form}
            layout="vertical"
            onFinish={handleSubmit}
            initialValues={{ download_dir: defaultDir }}
          >
            <div
              style={{
                display: "grid",
                gridTemplateColumns: isCompact
                  ? "1fr"
                  : "minmax(260px, 1.2fr) minmax(260px, 1fr) auto",
                gap: 12,
                alignItems: "end",
              }}
            >
              <Form.Item
                name="url"
                label="YouTube URL"
                rules={[{ required: true, message: "URL is required" }]}
                style={{ marginBottom: 0 }}
              >
                <Input
                  prefix={<LinkOutlined />}
                  placeholder="https://www.youtube.com/watch?v=..."
                  allowClear
                />
              </Form.Item>
              <Form.Item
                name="download_dir"
                label="Download directory"
                rules={[{ required: true, message: "Download directory is required" }]}
                style={{ marginBottom: 0 }}
              >
                <Input prefix={<FolderOpenOutlined />} placeholder={defaultDir} />
              </Form.Item>
              <Space.Compact>
                <Button onClick={() => form.setFieldsValue({ download_dir: defaultDir })}>
                  Default
                </Button>
                <Button
                  type="primary"
                  htmlType="submit"
                  icon={<DownloadOutlined />}
                  loading={submitting}
                >
                  Download
                </Button>
              </Space.Compact>
            </div>
          </Form>
        </div>

        <Table
          rowKey="id"
          dataSource={downloads}
          columns={columns}
          loading={loading && downloads.length === 0}
          pagination={false}
          size="middle"
          locale={{ emptyText: "No downloads" }}
        />
      </section>
    </div>
  );
}
