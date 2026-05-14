import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Col,
  Input,
  Progress,
  Row,
  Select,
  Space,
  Tag,
  Typography,
} from "antd";
import {
  CloudDownloadOutlined,
  ReloadOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import {
  defaultAsrParams,
  ASR_STATUS_CHANGED_EVENT,
  getAsrStatus,
  loadAsrParams,
  saveAsrParams,
  startAsrService,
  stopAsrService,
  streamAsrInitialization,
  type AsrConnectionParams,
  type AsrProgressEvent,
  type AsrStatus,
  type AsrStreamEvent,
} from "../../../api/asr";

const { Text } = Typography;

type RuntimePhase = "idle" | "checking" | "running" | "ready" | "error";

interface DownloadState {
  file: string;
  percent: number;
  downloadedBytes?: number;
  totalBytes?: number;
  bytesPerSecond?: number;
  etaSeconds?: number;
  resumed?: boolean;
}

export default function SpeechTab() {
  const defaults = useMemo(() => defaultAsrParams(), []);
  const [params, setParams] = useState<AsrConnectionParams>(() => loadAsrParams());
  const [status, setStatus] = useState<AsrStatus | null>(null);
  const [phase, setPhase] = useState<RuntimePhase>("idle");
  const [download, setDownload] = useState<DownloadState | null>(null);
  const [errorText, setErrorText] = useState("");
  const [errorDetail, setErrorDetail] = useState("");
  const [serviceBusy, setServiceBusy] = useState(false);
  const initAbortRef = useRef<AbortController | null>(null);

  const refreshStatus = useCallback(async () => {
    setPhase((prev) => (prev === "running" ? prev : "checking"));
    try {
      const next = await getAsrStatus(params);
      setStatus(next);
      setErrorText("");
      setErrorDetail("");
      if (next.ready) {
        setPhase("ready");
        setDownload(null);
      } else {
        setPhase("idle");
        setDownload(null);
      }
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      setPhase("error");
      setErrorText("Failed to read speech converter status.");
      setErrorDetail(text);
    }
  }, [params]);

  useEffect(() => {
    void refreshStatus();
    return () => {
      initAbortRef.current?.abort();
    };
  }, [refreshStatus]);

  useEffect(() => {
    saveAsrParams(params);
  }, [params]);

  const handleStreamEvent = useCallback(
    (event: AsrStreamEvent) => {
      if (event.type === "progress") {
        const progressEvent = event as AsrProgressEvent;
        setPhase(progressEvent.status === "ready" ? "ready" : "running");
        if (progressEvent.phase === "download") {
          setDownload({
            file: progressEvent.file || "ASR resource",
            percent: progressEvent.download_percent ?? progressEvent.progress ?? 0,
            downloadedBytes: progressEvent.downloaded_bytes,
            totalBytes: progressEvent.total_bytes,
            bytesPerSecond: progressEvent.bytes_per_second,
            etaSeconds: progressEvent.eta_seconds,
            resumed: progressEvent.resumed,
          });
        } else if (progressEvent.status === "ready") {
          setDownload(null);
        }
      } else if (event.type === "error") {
        setPhase("error");
        setErrorText(event.message);
        setErrorDetail(event.detail || "");
      } else if (event.type === "done") {
        window.dispatchEvent(new Event(ASR_STATUS_CHANGED_EVENT));
        void refreshStatus();
      }
    },
    [refreshStatus],
  );

  const startInitialization = useCallback(async () => {
    initAbortRef.current?.abort();
    const controller = new AbortController();
    initAbortRef.current = controller;
    setPhase("running");
    setDownload({ file: "Preparing download", percent: 0 });
    setErrorText("");
    setErrorDetail("");

    try {
      await streamAsrInitialization(params, handleStreamEvent, controller.signal);
    } catch (error) {
      if (controller.signal.aborted) {
        return;
      }
      const text = error instanceof Error ? error.message : String(error);
      setPhase("error");
      setErrorText("Speech converter initialization stream failed.");
      setErrorDetail(text);
    }
  }, [handleStreamEvent, params]);

  const handleStartService = useCallback(async () => {
    setServiceBusy(true);
    setErrorText("");
    setErrorDetail("");
    try {
      const result = await startAsrService(params);
      if (!result.ready) {
        setPhase("error");
        setErrorText(result.message);
        setErrorDetail(result.detail || "");
      }
      window.dispatchEvent(new Event(ASR_STATUS_CHANGED_EVENT));
      await refreshStatus();
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      setPhase("error");
      setErrorText("Failed to start Qwen3-ASR model service.");
      setErrorDetail(text);
    } finally {
      setServiceBusy(false);
    }
  }, [params, refreshStatus]);

  const handleStopService = useCallback(async () => {
    setServiceBusy(true);
    setErrorText("");
    setErrorDetail("");
    try {
      await stopAsrService(params);
      window.dispatchEvent(new Event(ASR_STATUS_CHANGED_EVENT));
      await refreshStatus();
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      setPhase("error");
      setErrorText("Failed to stop Qwen3-ASR model service.");
      setErrorDetail(text);
    } finally {
      setServiceBusy(false);
    }
  }, [params, refreshStatus]);

  const statusTag = useMemo(() => {
    if (phase === "running" || phase === "checking") {
      return <Tag color="processing">Initializing</Tag>;
    }
    if (phase === "error") {
      return <Tag color="error">Error</Tag>;
    }
    if (status?.platform_supported === false || status?.status === "unsupported") {
      return <Tag color="error">Unsupported</Tag>;
    }
    if (phase === "ready" || status?.ready) {
      return <Tag color="success">Ready</Tag>;
    }
    if (status?.installed) {
      return <Tag color="warning">Installed</Tag>;
    }
    return <Tag>Missing</Tag>;
  }, [phase, status]);

  const unsupported = status?.platform_supported === false || status?.status === "unsupported";
  const showInitialization =
    !unsupported && (phase === "running" || phase === "error" || !status?.installed);
  const progressPercent = Math.max(0, Math.min(100, Math.round(download?.percent ?? 0)));
  const downloadMeta = formatDownloadMeta(download);

  return (
    <div style={{ paddingBottom: 24 }}>
      <Card
        title={
          <Space>
            <ThunderboltOutlined />
            <span>Speech Converter</span>
            {statusTag}
          </Space>
        }
        extra={
          <Space>
            <Button icon={<ReloadOutlined />} onClick={refreshStatus}>
              Refresh
            </Button>
            {showInitialization ? (
              phase === "running" ? (
                <Button loading>Initializing</Button>
              ) : (
                <Button
                  type="primary"
                  icon={<CloudDownloadOutlined />}
                  onClick={startInitialization}
                  disabled={unsupported}
                >
                  Initialize
                </Button>
              )
            ) : null}
            <Button
              onClick={handleStartService}
              loading={serviceBusy}
              disabled={unsupported || phase === "running" || status?.ready}
            >
              Start Service
            </Button>
            <Button
              danger
              onClick={handleStopService}
              loading={serviceBusy}
              disabled={!status?.managed}
            >
              Stop Service
            </Button>
          </Space>
        }
      >
        <Alert
          type="info"
          showIcon
          style={{ marginBottom: 16 }}
        message="ASR runs as a Bifrost-managed local model service."
        description="Initialization prepares files under ~/.bifrost/asr. Start Service dynamically selects a free local port and keeps the model service alive only while needed. It may need external network access for the native qwen3_asr_rs runtime on GitHub and Qwen model weights on Hugging Face; if either source is unreachable, the exact download or startup error is shown below."
        />
        {unsupported ? (
          <Alert
            type="error"
            showIcon
            style={{ marginBottom: 16 }}
            message="Qwen3-ASR is not supported on this computer."
            description={status?.unsupported_reason || status?.message}
          />
        ) : null}
        {status && status.platform_supported && !status.ffmpeg_available ? (
          <Alert
            type="warning"
            showIcon
            style={{ marginBottom: 16 }}
            message="ffmpeg will be prepared during ASR self-check."
            description="Initialize or Start Service will try to install ffmpeg with Homebrew. If automatic installation is unavailable, the error will include the manual install command and you can retry the same action afterward."
          />
        ) : null}
        {status?.ready && !status.managed ? (
          <Alert
            type="warning"
            showIcon
            style={{ marginBottom: 16 }}
            message="A Qwen3-ASR server is reachable but not managed by this Bifrost process."
            description="Stop Service can only stop services started from this panel. To release resources, stop the external asr-server process or restart it with Start Service here."
          />
        ) : null}
        <Row gutter={[16, 16]}>
          <Col xs={24} md={8}>
            <Text type="secondary">Host</Text>
            <Input value={params.host || defaults.host} disabled />
          </Col>
          <Col xs={24} md={8}>
            <Text type="secondary">Service Port</Text>
            <Input
              value={status?.ready ? status.server_url : "Auto-select on Start Service"}
              disabled
            />
          </Col>
          <Col xs={24} md={8}>
            <Text type="secondary">Language</Text>
            <Select
              value={params.language || defaults.language}
              style={{ width: "100%" }}
              options={[
                { value: "chinese", label: "Chinese" },
                { value: "english", label: "English" },
                { value: "auto", label: "Auto" },
              ]}
              onChange={(value) =>
                setParams((prev) => ({ ...prev, language: value }))
              }
            />
          </Col>
          <Col xs={24} md={12}>
            <Text type="secondary">Model</Text>
            <Select
              value={params.model || defaults.model}
              style={{ width: "100%" }}
              options={[
                { value: "Qwen3-ASR-1.7B", label: "Qwen3-ASR-1.7B" },
                { value: "Qwen3-ASR-0.6B", label: "Qwen3-ASR-0.6B" },
              ]}
              onChange={(value) =>
                setParams((prev) => ({ ...prev, model: value }))
              }
            />
          </Col>
          <Col xs={24} md={12}>
            <Text type="secondary">Storage</Text>
            <Input value="~/.bifrost/asr" disabled />
          </Col>
        </Row>

        {showInitialization ? (
          <div style={{ marginTop: 20 }}>
            <Progress
              percent={progressPercent}
              status={phase === "error" ? "exception" : "active"}
            />
            <Space size={8} wrap style={{ marginTop: 8 }}>
              <Text type="secondary">{download?.file || "Waiting for download"}</Text>
              {downloadMeta ? <Text type="secondary">{downloadMeta}</Text> : null}
              {download?.resumed ? <Tag color="blue">Resumed</Tag> : null}
            </Space>
          </div>
        ) : null}

        {errorText ? (
          <Alert
            style={{ marginTop: 16 }}
            type="error"
            showIcon
            message={errorText}
            description={errorDetail || undefined}
          />
        ) : null}
      </Card>
    </div>
  );
}

function formatDownloadMeta(download: DownloadState | null): string {
  if (!download?.downloadedBytes) {
    return "";
  }
  const size = download.totalBytes
    ? `${formatBytes(download.downloadedBytes)} / ${formatBytes(download.totalBytes)}`
    : formatBytes(download.downloadedBytes);
  const rate = download.bytesPerSecond ? `${formatBytes(download.bytesPerSecond)}/s` : "";
  const eta = download.etaSeconds ? `${download.etaSeconds}s left` : "";
  return [size, rate, eta].filter(Boolean).join(" | ");
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}
