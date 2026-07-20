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
  ASR_STATUS_CHANGED_EVENT,
  getAsrStatus,
  getMossModelStatus,
  loadModelManagementParams,
  saveModelManagementParams,
  streamAsrInitialization,
  streamMossInitialization,
  type AsrConnectionParams,
  type MossModelStatus,
  type AsrProgressEvent,
  type AsrStatus,
  type AsrStreamEvent,
} from "../../../api/asr";

const { Text } = Typography;
const MOSS_MODEL_ID = "MOSS-Transcribe-Diarize-MLX-8bit";

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
  const defaults = useMemo(() => loadModelManagementParams(), []);
  const [params, setParams] = useState<AsrConnectionParams>(() => loadModelManagementParams());
  const [status, setStatus] = useState<AsrStatus | null>(null);
  const [mossStatus, setMossStatus] = useState<MossModelStatus | null>(null);
  const [phase, setPhase] = useState<RuntimePhase>("idle");
  const [download, setDownload] = useState<DownloadState | null>(null);
  const [errorText, setErrorText] = useState("");
  const [errorDetail, setErrorDetail] = useState("");
  const initAbortRef = useRef<AbortController | null>(null);
  const mossSelected = params.model === MOSS_MODEL_ID;

  const refreshStatus = useCallback(async () => {
    setPhase((prev) => (prev === "running" ? prev : "checking"));
    try {
      const next = mossSelected ? await getMossModelStatus() : await getAsrStatus(params);
      if (mossSelected) {
        setMossStatus(next as MossModelStatus);
      } else {
        setStatus(next as AsrStatus);
      }
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
  }, [mossSelected, params]);

  useEffect(() => {
    void refreshStatus();
    return () => {
      initAbortRef.current?.abort();
    };
  }, [refreshStatus]);

  useEffect(() => {
    saveModelManagementParams(params);
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
      if (mossSelected) {
        await streamMossInitialization(handleStreamEvent, controller.signal);
      } else {
        await streamAsrInitialization(params, handleStreamEvent, controller.signal);
      }
    } catch (error) {
      if (controller.signal.aborted) {
        return;
      }
      const text = error instanceof Error ? error.message : String(error);
      setPhase("error");
      setErrorText("Speech converter initialization stream failed.");
      setErrorDetail(text);
    }
  }, [handleStreamEvent, mossSelected, params]);

  const selectedStatus = mossSelected ? mossStatus : status;

  const statusTag = useMemo(() => {
    if (phase === "running" || phase === "checking") {
      return <Tag color="processing">Initializing</Tag>;
    }
    if (phase === "error") {
      return <Tag color="error">Error</Tag>;
    }
    if (
      selectedStatus?.platform_supported === false ||
      selectedStatus?.status === "unsupported"
    ) {
      return <Tag color="error">Unsupported</Tag>;
    }
    if (phase === "ready" || selectedStatus?.ready) {
      return <Tag color="success">Ready</Tag>;
    }
    if (selectedStatus?.installed) {
      return <Tag color="warning">Installed</Tag>;
    }
    return <Tag>Missing</Tag>;
  }, [phase, selectedStatus]);

  const unsupported =
    selectedStatus?.platform_supported === false || selectedStatus?.status === "unsupported";
  const showInitialization =
    !unsupported && (phase === "running" || phase === "error" || !selectedStatus?.installed);
  const progressPercent = Math.max(0, Math.min(100, Math.round(download?.percent ?? 0)));
  const downloadMeta = formatDownloadMeta(download);

  return (
    <div style={{ paddingBottom: 24 }}>
      <Card
        title={
          <Space>
            <ThunderboltOutlined />
            <span>Model Management</span>
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
          </Space>
        }
      >
        <Alert
          type="info"
          showIcon
          style={{ marginBottom: 16 }}
          message="ASR model assets are shared by all ASR entry points."
          description={
            mossSelected
              ? "Initialization verifies and installs the self-contained MLX/Python runtime, pinned dependencies, model metadata, and 8-bit weights. Directory Tasks use these assets on demand and retain automatic initialization as a fallback."
              : "Initialization prepares Qwen assets under ~/.bifrost/asr. The Speech Workbench, Directory Tasks, and CLI each choose their own model and lease the shared ASR service independently."
          }
        />
        {unsupported ? (
          <Alert
            type="error"
            showIcon
            style={{ marginBottom: 16 }}
            message={`${mossSelected ? "MOSS joint transcription" : "Qwen3-ASR"} is not supported on this computer.`}
            description={selectedStatus?.unsupported_reason || selectedStatus?.message}
          />
        ) : null}
        {!mossSelected && status && status.platform_supported && !status.ffmpeg_available ? (
          <Alert
            type="warning"
            showIcon
            style={{ marginBottom: 16 }}
            message="ffmpeg will be prepared during ASR self-check."
            description="Initialize will try to install ffmpeg with Homebrew when needed. If automatic installation is unavailable, the error will include the manual install command and you can retry initialization afterward."
          />
        ) : null}
        <Row gutter={[16, 16]}>
          {mossSelected ? (
            <>
              <Col xs={24} md={8}>
                <Text type="secondary">Execution</Text>
                <Input aria-label="MOSS execution" value="On demand / whole file" disabled />
              </Col>
              <Col xs={24} md={8}>
                <Text type="secondary">Language</Text>
                <Input aria-label="MOSS language" value="Automatic multilingual" disabled />
              </Col>
              <Col xs={24} md={8}>
                <Text type="secondary">Components</Text>
                <Input
                  aria-label="MOSS components"
                  value={`Runtime ${mossStatus?.runtime_ready ? "ready" : "missing"} / Model ${mossStatus?.model_ready ? "ready" : "missing"}`}
                  disabled
                />
              </Col>
            </>
          ) : (
            <>
              <Col xs={24} md={8}>
                <Text type="secondary">Host</Text>
                <Input value={params.host || defaults.host} disabled />
              </Col>
              <Col xs={24} md={8}>
                <Text type="secondary">Service Port</Text>
                <Input
                  value={status?.ready ? status.server_url : "No service leased by model management"}
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
            </>
          )}
          <Col xs={24} md={12}>
            <Text type="secondary">Model</Text>
            <Select
              aria-label="Managed ASR model"
              data-testid="asr-managed-model-select"
              value={params.model || defaults.model}
              style={{ width: "100%" }}
              options={[
                { value: "Qwen3-ASR-0.6B", label: "Qwen3-ASR-0.6B" },
                { value: "Qwen3-ASR-1.7B", label: "Qwen3-ASR-1.7B" },
                { value: MOSS_MODEL_ID, label: "MOSS joint transcription (MLX 8-bit)" },
              ]}
              onChange={(value) =>
                setParams((prev) => ({ ...prev, model: value }))
              }
            />
          </Col>
          <Col xs={24} md={12}>
            <Text type="secondary">Storage</Text>
            <Input
              aria-label="Managed ASR storage"
              value={mossSelected ? "~/.bifrost/asr/moss_joint_mlx" : "~/.bifrost/asr"}
              disabled
            />
          </Col>
        </Row>

        {mossSelected && mossStatus ? (
          <Space wrap style={{ marginTop: 12 }} data-testid="moss-managed-asset-status">
            <Tag color={mossStatus.runtime_ready ? "success" : "warning"}>
              Runtime {mossStatus.runtime_ready ? "verified" : "missing"}
            </Tag>
            <Tag color={mossStatus.model_ready ? "success" : "warning"}>
              Model {mossStatus.model_ready ? "verified" : "missing"}
            </Tag>
            <Text type="secondary">
              {formatBytes(mossStatus.installed_model_bytes)} / {formatBytes(mossStatus.expected_model_bytes)}
            </Text>
          </Space>
        ) : null}

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
