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
import { t as translate, useI18n } from "../../../i18n";

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
  const { t } = useI18n();
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
        const mossNext = next as MossModelStatus;
        setMossStatus(mossNext);
        if (mossNext.initializing) {
          const progress = mossNext.initialization;
          setPhase("running");
          setDownload(
            progress
              ? {
                  file: progress.label,
                  percent: progress.percent ?? 0,
                  downloadedBytes: progress.downloaded_bytes,
                  totalBytes: progress.total_bytes,
                  bytesPerSecond: progress.bytes_per_second,
                  etaSeconds: progress.eta_seconds,
                  resumed: progress.resumed,
                }
              : { file: t("asr.models.preparingMossRuntime"), percent: 0 },
          );
        } else if (mossNext.ready) {
          setPhase("ready");
          setDownload(null);
        } else {
          setPhase("idle");
          setDownload(null);
        }
      } else {
        setStatus(next as AsrStatus);
        if (next.ready) {
          setPhase("ready");
          setDownload(null);
        } else {
          setPhase("idle");
          setDownload(null);
        }
      }
      setErrorText("");
      setErrorDetail("");
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      setPhase("error");
      setErrorText(t("asr.models.readStatusFailed"));
      setErrorDetail(text);
    }
  }, [mossSelected, params, t]);

  useEffect(() => {
    void refreshStatus();
    return () => {
      initAbortRef.current?.abort();
    };
  }, [refreshStatus]);

  useEffect(() => {
    saveModelManagementParams(params);
  }, [params]);

  useEffect(() => {
    if (!mossSelected || !mossStatus?.initializing) {
      return;
    }
    const timer = window.setInterval(() => {
      void refreshStatus();
    }, 1_000);
    return () => window.clearInterval(timer);
  }, [mossSelected, mossStatus?.initializing, refreshStatus]);

  const handleStreamEvent = useCallback(
    (event: AsrStreamEvent) => {
      if (event.type === "progress") {
        const progressEvent = event as AsrProgressEvent;
        setPhase(progressEvent.status === "ready" ? "ready" : "running");
        if (progressEvent.phase === "download") {
          setDownload({
            file: progressEvent.file || t("asr.models.asrResource"),
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
    [refreshStatus, t],
  );

  const startInitialization = useCallback(async () => {
    initAbortRef.current?.abort();
    const controller = new AbortController();
    initAbortRef.current = controller;
    setPhase("running");
    setDownload({ file: t("asr.models.preparingDownload"), percent: 0 });
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
      setErrorText(t("asr.models.initStreamFailed"));
      setErrorDetail(text);
    }
  }, [handleStreamEvent, mossSelected, params, t]);

  const selectedStatus = mossSelected ? mossStatus : status;

  const statusTag = useMemo(() => {
    if (phase === "running" || phase === "checking") {
      return <Tag color="processing">{t("asr.models.initializing")}</Tag>;
    }
    if (phase === "error") {
      return <Tag color="error">{t("common.error")}</Tag>;
    }
    if (
      selectedStatus?.platform_supported === false ||
      selectedStatus?.status === "unsupported"
    ) {
      return <Tag color="error">{t("asr.models.unsupported")}</Tag>;
    }
    if (phase === "ready" || selectedStatus?.ready) {
      return <Tag color="success">{t("asr.models.ready")}</Tag>;
    }
    if (selectedStatus?.installed) {
      return <Tag color="warning">{t("asr.models.installed")}</Tag>;
    }
    return <Tag>{t("asr.models.missing")}</Tag>;
  }, [phase, selectedStatus, t]);

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
            <span>{t("asr.models.title")}</span>
            {statusTag}
          </Space>
        }
        extra={
          <Space>
            <Button icon={<ReloadOutlined />} onClick={refreshStatus}>
              {t("common.refresh")}
            </Button>
            {showInitialization ? (
              phase === "running" ? (
                <Button loading>{t("asr.models.initializing")}</Button>
              ) : (
                <Button
                  type="primary"
                  icon={<CloudDownloadOutlined />}
                  onClick={startInitialization}
                  disabled={unsupported}
                >
                  {t("asr.models.initialize")}
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
          message={t("asr.models.sharedAssets")}
          description={
            mossSelected
              ? t("asr.models.mossInitDesc")
              : t("asr.models.qwenInitDesc")
          }
        />
        {unsupported ? (
          <Alert
            type="error"
            showIcon
            style={{ marginBottom: 16 }}
            message={t("asr.models.notSupported", {
              model: mossSelected ? t("asr.models.mossJoint") : "Qwen3-ASR",
            })}
            description={selectedStatus?.unsupported_reason || selectedStatus?.message}
          />
        ) : null}
        {!mossSelected && status && status.platform_supported && !status.ffmpeg_available ? (
          <Alert
            type="warning"
            showIcon
            style={{ marginBottom: 16 }}
            message={t("asr.models.ffmpegPrepared")}
            description={t("asr.models.ffmpegDesc")}
          />
        ) : null}
        <Row gutter={[16, 16]}>
          {mossSelected ? (
            <>
              <Col xs={24} md={8}>
                <Text type="secondary">{t("asr.models.execution")}</Text>
                <Input
                  aria-label={t("asr.models.mossExecutionAria")}
                  value={t("asr.models.mossExecutionValue")}
                  disabled
                />
              </Col>
              <Col xs={24} md={8}>
                <Text type="secondary">{t("asr.models.language")}</Text>
                <Input
                  aria-label={t("asr.models.mossLanguageAria")}
                  value={t("asr.models.mossLanguageValue")}
                  disabled
                />
              </Col>
              <Col xs={24} md={8}>
                <Text type="secondary">{t("asr.models.components")}</Text>
                <Input
                  aria-label={t("asr.models.mossComponentsAria")}
                  value={`${
                    mossStatus?.runtime_ready
                      ? t("asr.models.runtimeReady")
                      : t("asr.models.runtimeMissing")
                  } / ${
                    mossStatus?.model_ready ? t("asr.models.modelReady") : t("asr.models.modelMissing")
                  }`}
                  disabled
                />
              </Col>
            </>
          ) : (
            <>
              <Col xs={24} md={8}>
                <Text type="secondary">{t("asr.models.host")}</Text>
                <Input value={params.host || defaults.host} disabled />
              </Col>
              <Col xs={24} md={8}>
                <Text type="secondary">{t("asr.models.servicePort")}</Text>
                <Input
                  value={status?.ready ? status.server_url : t("asr.models.noServiceLeased")}
                  disabled
                />
              </Col>
              <Col xs={24} md={8}>
                <Text type="secondary">{t("asr.models.language")}</Text>
                <Select
                  value={params.language || defaults.language}
                  style={{ width: "100%" }}
                  options={[
                    { value: "chinese", label: t("asr.language.chinese") },
                    { value: "english", label: t("asr.language.english") },
                    { value: "auto", label: t("asr.language.auto") },
                  ]}
                  onChange={(value) =>
                    setParams((prev) => ({ ...prev, language: value }))
                  }
                />
              </Col>
            </>
          )}
          <Col xs={24} md={12}>
            <Text type="secondary">{t("asr.models.model")}</Text>
            <Select
              aria-label={t("asr.models.managedModelAria")}
              data-testid="asr-managed-model-select"
              value={params.model || defaults.model}
              style={{ width: "100%" }}
              options={[
                { value: "Qwen3-ASR-0.6B", label: "Qwen3-ASR-0.6B" },
                { value: "Qwen3-ASR-1.7B", label: "Qwen3-ASR-1.7B" },
                { value: MOSS_MODEL_ID, label: t("asr.models.mossModelOption") },
              ]}
              onChange={(value) =>
                setParams((prev) => ({ ...prev, model: value }))
              }
            />
          </Col>
          <Col xs={24} md={12}>
            <Text type="secondary">{t("asr.models.storage")}</Text>
            <Input
              aria-label={t("asr.models.managedStorageAria")}
              value={mossSelected ? "~/.bifrost/asr/moss_joint_mlx" : "~/.bifrost/asr"}
              disabled
            />
          </Col>
        </Row>

        {mossSelected && mossStatus ? (
          <Space wrap style={{ marginTop: 12 }} data-testid="moss-managed-asset-status">
            <Tag color={mossStatus.runtime_ready ? "success" : "warning"}>
              {mossStatus.runtime_ready ? t("asr.models.runtimeVerified") : t("asr.models.runtimeMissing")}
            </Tag>
            <Tag color={mossStatus.model_ready ? "success" : "warning"}>
              {mossStatus.model_ready ? t("asr.models.modelVerified") : t("asr.models.modelMissing")}
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
              <Text type="secondary">{download?.file || t("asr.models.waitingDownload")}</Text>
              {downloadMeta ? <Text type="secondary">{downloadMeta}</Text> : null}
              {download?.resumed ? <Tag color="blue">{t("asr.models.resumed")}</Tag> : null}
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
  const eta = download.etaSeconds
    ? translate("asr.models.etaLeft", { seconds: download.etaSeconds })
    : "";
  return [size, rate, eta].filter(Boolean).join(" | ");
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}
