import type { Dispatch, RefObject, SetStateAction } from "react";
import {
  Alert,
  Button,
  Card,
  Input,
  Progress,
  Select,
  Space,
  Tag,
  Typography,
  theme,
} from "antd";
import {
  AudioOutlined,
  InboxOutlined,
  LoadingOutlined,
  PlayCircleOutlined,
  StopOutlined,
  UploadOutlined,
} from "@ant-design/icons";
import type { AsrConnectionParams, AsrStatus } from "../../../api/asr";
import type { WorkState } from "../asrUtils";

const { Text, Paragraph } = Typography;

interface SpeechWorkbenchProps {
  token: ReturnType<typeof theme.useToken>["token"];
  ready: boolean | undefined;
  status: AsrStatus | null;
  params: AsrConnectionParams;
  onParamsChange: Dispatch<SetStateAction<AsrConnectionParams>>;
  serviceBusy: boolean;
  workState: WorkState;
  progress: number;
  selectedName: string;
  transcript: string;
  events: string[];
  errorText: string;
  micLevels: number[];
  micPeak: number;
  fileInputRef: RefObject<HTMLInputElement | null>;
  busy: boolean;
  showFileProgress: boolean;
  onFile: (file: File) => void;
  onStartRecording: () => void;
  onStopRecording: () => void;
  onStartService: () => void;
  onStopService: () => void;
  onCancel: () => void;
}

export default function SpeechWorkbench({
  token,
  ready,
  status,
  params,
  onParamsChange,
  serviceBusy,
  workState,
  progress,
  selectedName,
  transcript,
  events,
  errorText,
  micLevels,
  micPeak,
  fileInputRef,
  busy,
  showFileProgress,
  onFile,
  onStartRecording,
  onStopRecording,
  onStartService,
  onStopService,
  onCancel,
}: SpeechWorkbenchProps) {
  return (
    <Card
      data-testid="asr-workbench-card"
      title={
        <Space>
          <AudioOutlined />
          <span>Speech to Text</span>
          {ready ? <Tag color="success">Ready</Tag> : <Tag color="warning">Not Ready</Tag>}
        </Space>
      }
    >
      <Space direction="vertical" size={16} style={{ width: "100%" }}>
        <section aria-label="Workbench ASR Service">
          <Space style={{ marginBottom: 12 }}>
            <AudioOutlined />
            <Text strong>Workbench Model and Service</Text>
          </Space>
          <Space wrap style={{ width: "100%" }}>
            <Select
              aria-label="Workbench ASR model"
              value={params.model || "Qwen3-ASR-1.7B"}
              style={{ minWidth: 180 }}
              options={[
                { value: "Qwen3-ASR-1.7B", label: "Qwen3-ASR-1.7B" },
                { value: "Qwen3-ASR-0.6B", label: "Qwen3-ASR-0.6B" },
              ]}
              onChange={(model) => onParamsChange((previous) => ({ ...previous, model }))}
            />
            <Select
              aria-label="Workbench ASR language"
              value={params.language || "chinese"}
              style={{ minWidth: 140 }}
              options={[
                { value: "chinese", label: "Chinese" },
                { value: "english", label: "English" },
                { value: "auto", label: "Auto" },
              ]}
              onChange={(language) =>
                onParamsChange((previous) => ({ ...previous, language }))
              }
            />
            <Input
              aria-label="Workbench ASR host"
              value={params.host || "127.0.0.1"}
              style={{ width: 150 }}
              onChange={(event) =>
                onParamsChange((previous) => ({ ...previous, host: event.target.value }))
              }
            />
            <Button type="primary" loading={serviceBusy} onClick={onStartService}>
              Start Service
            </Button>
            <Button danger loading={serviceBusy} disabled={!status?.managed} onClick={onStopService}>
              Stop Service
            </Button>
            {status?.server_url ? <Tag>{status.server_url}</Tag> : null}
          </Space>
          <Text type="secondary" style={{ display: "block", marginTop: 8 }}>
            Upload and microphone transcription share this workbench model. Directory Tasks and
            CLI commands keep their own model selections.
          </Text>
        </section>

        <section aria-label="Audio Input">
          <Space style={{ marginBottom: 12 }}>
            <AudioOutlined />
            <Text strong>Audio Input</Text>
          </Space>
          {!ready ? (
            <Alert
              type="warning"
              showIcon
              message="Speech converter is not ready"
              description={
                status?.message ||
                "Initialize Qwen3-ASR from AI > Tools > ASR before transcribing audio."
              }
              style={{ marginBottom: 16 }}
            />
          ) : null}

          <div
            onDragOver={(event) => {
              event.preventDefault();
            }}
            onDrop={(event) => {
              event.preventDefault();
              const file = event.dataTransfer.files.item(0);
              if (file) {
                onFile(file);
              }
            }}
            style={{
              minHeight: 180,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              flexDirection: "column",
              gap: 12,
              border: `1px dashed ${token.colorBorder}`,
              borderRadius: 6,
              background: token.colorFillQuaternary,
              color: token.colorTextSecondary,
              cursor: "pointer",
            }}
            onClick={() => fileInputRef.current?.click()}
          >
            <InboxOutlined style={{ fontSize: 32, color: token.colorPrimary }} />
            <Text>Drop an audio file here</Text>
            <Button icon={<UploadOutlined />}>Choose File</Button>
            <input
              ref={fileInputRef}
              type="file"
              accept="audio/*,.wav,.mp3,.m4a,.webm,.ogg,.flac"
              style={{ display: "none" }}
              onChange={(event) => {
                const file = event.target.files?.item(0);
                if (file) {
                  onFile(file);
                  event.currentTarget.value = "";
                }
              }}
            />
          </div>

          <Space style={{ marginTop: 16 }} wrap>
            {workState === "recording" ? (
              <Button danger icon={<StopOutlined />} onClick={onStopRecording}>
                Stop Mic
              </Button>
            ) : (
              <Button
                type="primary"
                icon={<PlayCircleOutlined />}
                onClick={onStartRecording}
                disabled={!ready || workState === "transcribing"}
              >
                Start Mic
              </Button>
            )}
            <Button onClick={onCancel} disabled={!busy}>
              Cancel
            </Button>
          </Space>

          <div
            aria-label="Microphone input level"
            style={{
              marginTop: 16,
              minHeight: 76,
              padding: "12px 14px",
              border: `1px solid ${token.colorBorderSecondary}`,
              borderRadius: 6,
              background: token.colorFillQuaternary,
            }}
          >
            <div
              style={{
                height: 42,
                display: "flex",
                alignItems: "center",
                gap: 3,
              }}
            >
              {micLevels.map((level, index) => {
                const active = workState === "recording";
                const height = active ? Math.max(4, 6 + level * 36) : 4;
                return (
                  <span
                    key={index}
                    style={{
                      flex: "1 1 0",
                      height,
                      minWidth: 3,
                      borderRadius: 3,
                      background: active
                        ? `linear-gradient(180deg, ${token.colorPrimary}, ${token.colorSuccess})`
                        : token.colorFillSecondary,
                      opacity: active ? 0.45 + level * 0.55 : 0.7,
                      transition: "height 80ms ease, opacity 80ms ease",
                    }}
                  />
                );
              })}
            </div>
            <div
              style={{
                display: "flex",
                justifyContent: "space-between",
                alignItems: "center",
                marginTop: 8,
                color: token.colorTextSecondary,
                fontSize: 12,
              }}
            >
              <span>{workState === "recording" ? "Live microphone level" : "Mic level"}</span>
              <span>{Math.round(micPeak * 100)}%</span>
            </div>
          </div>

          {showFileProgress ? (
            <div
              aria-label="File transcription progress"
              data-testid="asr-file-progress"
              style={{ marginTop: 16 }}
            >
              <Progress
                percent={progress}
                status={
                  workState === "error"
                    ? "exception"
                    : workState === "transcribing"
                      ? "active"
                      : progress === 100
                        ? "success"
                        : "normal"
                }
              />
              <Text type="secondary">
                {workState === "transcribing"
                  ? "Streaming file transcription status"
                  : selectedName}
              </Text>
            </div>
          ) : null}
        </section>

        <section
          aria-label="Transcript"
          style={{
            borderTop: `1px solid ${token.colorBorderSecondary}`,
            paddingTop: 16,
          }}
        >
          <Space style={{ marginBottom: 12 }}>
            {workState === "transcribing" ? <LoadingOutlined /> : <AudioOutlined />}
            <Text strong>Transcript</Text>
          </Space>
          {errorText ? (
            <Alert
              type="error"
              showIcon
              message="Transcription failed"
              description={errorText}
              style={{ marginBottom: 16, whiteSpace: "pre-wrap" }}
            />
          ) : null}
          <Paragraph
            style={{
              minHeight: 220,
              padding: 12,
              background: token.colorFillTertiary,
              border: `1px solid ${token.colorBorderSecondary}`,
              borderRadius: 6,
              whiteSpace: "pre-wrap",
              color: token.colorText,
            }}
          >
            {transcript || "Waiting for transcription text."}
          </Paragraph>
          <div
            style={{
              minHeight: 140,
              maxHeight: 220,
              overflow: "auto",
              padding: 12,
              background: token.colorFillQuaternary,
              border: `1px solid ${token.colorBorderSecondary}`,
              borderRadius: 6,
              fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
              fontSize: 12,
              whiteSpace: "pre-wrap",
            }}
          >
            {events.length ? events.join("\n") : "No stream events yet."}
          </div>
        </section>
      </Space>
    </Card>
  );
}
