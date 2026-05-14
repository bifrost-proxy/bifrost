import { useCallback, useEffect, useRef, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Col,
  Descriptions,
  Drawer,
  Form,
  Input,
  InputNumber,
  Popconfirm,
  Progress,
  Row,
  Select,
  Space,
  Switch,
  Table,
  Tag,
  Timeline as AntTimeline,
  Typography,
  message,
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
import {
  ASR_PARAMS_CHANGED_EVENT,
  ASR_STATUS_CHANGED_EVENT,
  buildAsrRealtimeUrl,
  createAsrTask,
  deleteAsrTask,
  getAsrStatus,
  getAsrTask,
  getAsrTaskFileTimeline,
  listAsrTasks,
  loadAsrParams,
  runAsrTask,
  streamAsrTranscription,
  type AsrDirectoryTask,
  type AsrDirectoryTaskDetail,
  type AsrStatus,
  type AsrStreamEvent,
  type AsrTaskFileRecord,
  type AsrTaskSchedule,
  type AsrTranscriptTimeline,
} from "../../api/asr";
import SpeechTab from "../Settings/tabs/SpeechTab";

const { Text, Paragraph } = Typography;

type WorkState = "idle" | "recording" | "transcribing" | "error";
const MIC_WINDOW_MS = 1000;
const MIC_METER_BARS = 40;
const EMPTY_MIC_LEVELS = Array.from({ length: MIC_METER_BARS }, () => 0);

export default function ASR() {
  const { token } = theme.useToken();
  const [taskForm] = Form.useForm();
  const taskScheduleKind = Form.useWatch("schedule_kind", taskForm) ?? "daily";
  const [status, setStatus] = useState<AsrStatus | null>(null);
  const [tasks, setTasks] = useState<AsrDirectoryTask[]>([]);
  const [tasksLoading, setTasksLoading] = useState(false);
  const [taskDetail, setTaskDetail] = useState<AsrDirectoryTaskDetail | null>(null);
  const [taskDetailOpen, setTaskDetailOpen] = useState(false);
  const [taskDetailLoading, setTaskDetailLoading] = useState(false);
  const [taskTimeline, setTaskTimeline] = useState<AsrTranscriptTimeline | null>(null);
  const [taskTimelineLoading, setTaskTimelineLoading] = useState(false);
  const [workState, setWorkState] = useState<WorkState>("idle");
  const [progress, setProgress] = useState(0);
  const [selectedName, setSelectedName] = useState("");
  const [transcript, setTranscript] = useState("");
  const [events, setEvents] = useState<string[]>([]);
  const [errorText, setErrorText] = useState("");
  const [micLevels, setMicLevels] = useState<number[]>(EMPTY_MIC_LEVELS);
  const [micPeak, setMicPeak] = useState(0);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const audioContextRef = useRef<AudioContext | null>(null);
  const micMeterRafRef = useRef<number | null>(null);
  const micSendQueueRef = useRef(Promise.resolve());
  const committedTranscriptRef = useRef("");
  const partialTranscriptRef = useRef("");
  const recordingActiveRef = useRef(false);
  const getCurrentAsrParams = useCallback(() => loadAsrParams(), []);

  const appendEvent = useCallback((line: string) => {
    setEvents((prev) => [...prev.slice(-79), line]);
  }, []);

  const refreshStatus = useCallback(async () => {
    try {
      const next = await getAsrStatus(getCurrentAsrParams());
      setStatus(next);
    } catch (error) {
      appendEvent(
        `status error: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }, [appendEvent, getCurrentAsrParams]);

  const refreshTasks = useCallback(async () => {
    setTasksLoading(true);
    try {
      setTasks(await listAsrTasks());
    } catch (error) {
      appendEvent(
        `task status error: ${error instanceof Error ? error.message : String(error)}`,
      );
    } finally {
      setTasksLoading(false);
    }
  }, [appendEvent]);

  const stopMicMeter = useCallback(() => {
    if (micMeterRafRef.current !== null) {
      window.cancelAnimationFrame(micMeterRafRef.current);
      micMeterRafRef.current = null;
    }
    const audioContext = audioContextRef.current;
    audioContextRef.current = null;
    if (audioContext) {
      void audioContext.close().catch(() => undefined);
    }
    setMicLevels(EMPTY_MIC_LEVELS);
    setMicPeak(0);
  }, []);

  const startMicMeter = useCallback(
    (stream: MediaStream) => {
      stopMicMeter();
      const AudioContextCtor =
        window.AudioContext ||
        (window as Window & { webkitAudioContext?: typeof AudioContext })
          .webkitAudioContext;
      if (!AudioContextCtor) {
        appendEvent("meter: Web Audio API is unavailable");
        return;
      }

      const audioContext = new AudioContextCtor();
      const analyser = audioContext.createAnalyser();
      analyser.fftSize = 1024;
      analyser.smoothingTimeConstant = 0.72;
      const source = audioContext.createMediaStreamSource(stream);
      source.connect(analyser);
      audioContextRef.current = audioContext;

      const samples = new Uint8Array(analyser.frequencyBinCount);
      let lastPaint = 0;
      const tick = (timestamp: number) => {
        analyser.getByteFrequencyData(samples);
        let peak = 0;
        const nextLevels = Array.from({ length: MIC_METER_BARS }, (_, index) => {
          const start = Math.floor((index * samples.length) / MIC_METER_BARS);
          const end = Math.max(
            start + 1,
            Math.floor(((index + 1) * samples.length) / MIC_METER_BARS),
          );
          let sum = 0;
          for (let sampleIndex = start; sampleIndex < end; sampleIndex += 1) {
            sum += samples[sampleIndex];
          }
          const level = Math.min(1, sum / (end - start) / 255);
          peak = Math.max(peak, level);
          return level;
        });
        if (timestamp - lastPaint >= 33) {
          lastPaint = timestamp;
          setMicLevels(nextLevels);
          setMicPeak(peak);
        }
        micMeterRafRef.current = window.requestAnimationFrame(tick);
      };
      micMeterRafRef.current = window.requestAnimationFrame(tick);
    },
    [appendEvent, stopMicMeter],
  );

  const renderTranscript = useCallback(() => {
    const committed = committedTranscriptRef.current;
    const partial = partialTranscriptRef.current;
    setTranscript(partial ? appendTranscriptDelta(committed, partial) : committed);
  }, []);

  const resetTranscript = useCallback(() => {
    committedTranscriptRef.current = "";
    partialTranscriptRef.current = "";
    setTranscript("");
  }, []);

  useEffect(() => {
    const initialRefreshTimer = window.setTimeout(() => {
      void refreshStatus();
      void refreshTasks();
    }, 0);
    const handleStatusRefresh = () => {
      void refreshStatus();
    };
    window.addEventListener(ASR_PARAMS_CHANGED_EVENT, handleStatusRefresh);
    window.addEventListener(ASR_STATUS_CHANGED_EVENT, handleStatusRefresh);
    const taskRefreshTimer = window.setInterval(() => {
      void refreshTasks();
    }, 10000);
    return () => {
      window.clearTimeout(initialRefreshTimer);
      window.clearInterval(taskRefreshTimer);
      window.removeEventListener(ASR_PARAMS_CHANGED_EVENT, handleStatusRefresh);
      window.removeEventListener(ASR_STATUS_CHANGED_EVENT, handleStatusRefresh);
      abortRef.current?.abort();
      wsRef.current?.close();
      streamRef.current?.getTracks().forEach((track) => track.stop());
      stopMicMeter();
    };
  }, [refreshStatus, refreshTasks, stopMicMeter]);

  const handleStreamEvent = useCallback(
    (event: AsrStreamEvent) => {
      if (
        event.type === "progress" ||
        event.type === "connected" ||
        event.type === "stream" ||
        event.type === "finish"
      ) {
        setProgress(event.progress);
        appendEvent(`${event.phase}: ${event.message}`);
      } else if (event.type === "partial") {
        partialTranscriptRef.current = dedupeTranscript(
          committedTranscriptRef.current,
          event.text || event.delta,
        );
        appendEvent(
          `partial[${event.index}]: ${event.stable_start_ms}-${event.stable_end_ms}ms`,
        );
        renderTranscript();
      } else if (event.type === "final") {
        const candidate = event.committed || event.delta || event.text;
        const delta = dedupeTranscript(committedTranscriptRef.current, candidate);
        if (delta) {
          committedTranscriptRef.current = appendTranscriptDelta(
            committedTranscriptRef.current,
            delta,
          );
        }
        partialTranscriptRef.current = "";
        appendEvent(
          `final[${event.index}]: ${event.stable_start_ms}-${event.stable_end_ms}ms`,
        );
        renderTranscript();
      } else if (event.type === "text") {
        const delta = dedupeTranscript(committedTranscriptRef.current, event.text);
        if (delta) {
          committedTranscriptRef.current = appendTranscriptDelta(
            committedTranscriptRef.current,
            delta,
          );
        }
        partialTranscriptRef.current = "";
        renderTranscript();
      } else if (event.type === "error") {
        setWorkState("error");
        setErrorText(event.detail ? `${event.message}\n${event.detail}` : event.message);
        appendEvent(`error: ${event.message}`);
      } else if (event.type === "done") {
        setWorkState(recordingActiveRef.current ? "recording" : "idle");
        setProgress(100);
        appendEvent("done");
        void refreshStatus();
      }
    },
    [appendEvent, refreshStatus, renderTranscript],
  );

  const transcribeBlob = useCallback(
    async (blob: Blob, fileName: string, reset = true) => {
      if (reset) {
        abortRef.current?.abort();
        resetTranscript();
        setEvents([]);
      }
      const controller = new AbortController();
      if (reset) {
        abortRef.current = controller;
      }
      setWorkState(recordingActiveRef.current ? "recording" : "transcribing");
      setSelectedName(fileName);
      setErrorText("");
      setProgress(1);

      try {
        await streamAsrTranscription(
          blob,
          fileName,
          getCurrentAsrParams(),
          handleStreamEvent,
          controller.signal,
        );
      } catch (error) {
        if (controller.signal.aborted) {
          return;
        }
        const text = error instanceof Error ? error.message : String(error);
        setWorkState("error");
        setErrorText(text);
        appendEvent(`stream error: ${text}`);
      }
    },
    [appendEvent, getCurrentAsrParams, handleStreamEvent, resetTranscript],
  );

  const handleFile = useCallback(
    (file: File) => {
      void transcribeBlob(file, file.name);
    },
    [transcribeBlob],
  );

  const startRecording = useCallback(async () => {
    if (!navigator.mediaDevices?.getUserMedia) {
      setWorkState("error");
      setErrorText("This browser does not expose microphone recording APIs.");
      return;
    }

    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      streamRef.current = stream;
      startMicMeter(stream);
      resetTranscript();
      setEvents([]);
      recordingActiveRef.current = true;
      micSendQueueRef.current = Promise.resolve();

      const ws = new WebSocket(buildAsrRealtimeUrl(getCurrentAsrParams()));
      wsRef.current = ws;
      ws.onmessage = (event) => {
        try {
          handleStreamEvent(JSON.parse(String(event.data)) as AsrStreamEvent);
        } catch (error) {
          appendEvent(
            `websocket parse error: ${error instanceof Error ? error.message : String(error)}`,
          );
        }
      };
      ws.onerror = () => {
        recordingActiveRef.current = false;
        stopMicMeter();
        stream.getTracks().forEach((track) => track.stop());
        streamRef.current = null;
        setWorkState("error");
        setErrorText("Microphone WebSocket connection failed.");
        appendEvent("websocket error: microphone stream failed");
      };
      ws.onclose = () => {
        wsRef.current = null;
        if (recordingActiveRef.current) {
          recordingActiveRef.current = false;
          setWorkState("idle");
          appendEvent("websocket: microphone stream closed");
        }
      };
      ws.onopen = () => {
        const recorder = new MediaRecorder(stream);
        recorderRef.current = recorder;
        ws.send(
          JSON.stringify({
            type: "start",
            mime_type: recorder.mimeType || "audio/webm",
            file_name: "microphone.webm",
          }),
        );
        recorder.ondataavailable = (event) => {
          if (event.data.size > 0 && ws.readyState === WebSocket.OPEN) {
            const audioBlob = event.data;
            micSendQueueRef.current = micSendQueueRef.current.then(async () => {
              const buffer = await audioBlob.arrayBuffer();
              if (ws.readyState === WebSocket.OPEN) {
                ws.send(buffer);
              }
            });
          }
        };
        recorder.onstop = () => {
          void micSendQueueRef.current.finally(() => {
            if (ws.readyState === WebSocket.OPEN) {
              ws.send(JSON.stringify({ type: "finish" }));
            }
          });
          stopMicMeter();
          stream.getTracks().forEach((track) => track.stop());
          streamRef.current = null;
        };
        recorder.start(MIC_WINDOW_MS);
        appendEvent("recording: microphone WebSocket stream opened");
      };
      setWorkState("recording");
      setProgress(0);
      setErrorText("");
      setSelectedName("");
      appendEvent("recording: microphone capture started with WebSocket streaming");
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      stopMicMeter();
      setWorkState("error");
      setErrorText(`Microphone capture failed: ${text}`);
      appendEvent(`microphone error: ${text}`);
    }
  }, [
    appendEvent,
    getCurrentAsrParams,
    handleStreamEvent,
    resetTranscript,
    startMicMeter,
    stopMicMeter,
  ]);

  const stopRecording = useCallback(() => {
    recordingActiveRef.current = false;
    stopMicMeter();
    if (recorderRef.current?.state === "recording") {
      recorderRef.current.stop();
    } else if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify({ type: "finish" }));
    } else if (wsRef.current) {
      wsRef.current.close();
    }
    recorderRef.current = null;
    appendEvent("recording: microphone capture stopped");
  }, [appendEvent, stopMicMeter]);

  const createDirectoryTask = useCallback(async () => {
    try {
      const values = await taskForm.validateFields();
      await createAsrTask({
        name: values.name,
        audio_dir: values.audio_dir,
        recursive: values.recursive,
        enabled: values.enabled,
        schedule: buildTaskSchedule(values),
        language: getCurrentAsrParams().language,
        model: getCurrentAsrParams().model,
      });
      taskForm.resetFields();
      await refreshTasks();
      message.success("ASR directory task created");
    } catch (error) {
      if (error && typeof error === "object" && "errorFields" in error) {
        return;
      }
      message.error(error instanceof Error ? error.message : "Failed to create ASR task");
    }
  }, [getCurrentAsrParams, refreshTasks, taskForm]);

  const openTaskDetail = useCallback(
    async (id: string, keepDrawerOpen = false) => {
      if (!keepDrawerOpen) {
        setTaskDetailOpen(true);
        setTaskTimeline(null);
      }
      setTaskDetailLoading(true);
      try {
        const nextDetail = await getAsrTask(id);
        setTaskDetail(nextDetail);
        const firstTimelineFile = nextDetail.files.find((file) => Boolean(file.output_timeline_path));
        if (firstTimelineFile) {
          setTaskTimelineLoading(true);
          try {
            setTaskTimeline(await getAsrTaskFileTimeline(firstTimelineFile.task_id, firstTimelineFile.key));
          } catch (error) {
            setTaskTimeline(null);
            message.error(error instanceof Error ? error.message : "Failed to load ASR timeline");
          } finally {
            setTaskTimelineLoading(false);
          }
        } else {
          setTaskTimeline(null);
        }
      } catch (error) {
        message.error(error instanceof Error ? error.message : "Failed to load ASR task");
      } finally {
        setTaskDetailLoading(false);
      }
    },
    [],
  );

  const openTaskTimeline = useCallback(async (file: AsrTaskFileRecord) => {
    setTaskTimelineLoading(true);
    try {
      setTaskTimeline(await getAsrTaskFileTimeline(file.task_id, file.key));
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to load ASR timeline");
    } finally {
      setTaskTimelineLoading(false);
    }
  }, []);

  const runDirectoryTask = useCallback(
    async (id: string) => {
      setTasksLoading(true);
      try {
        const result = await runAsrTask(id);
        message.success(
          `Processed ${result.processed_now}, failed ${result.failed_now}`,
        );
        if (taskDetail?.id === id) {
          void openTaskDetail(id, true);
        }
        await refreshTasks();
      } catch (error) {
        message.error(error instanceof Error ? error.message : "Failed to run ASR task");
      } finally {
        setTasksLoading(false);
      }
    },
    [openTaskDetail, refreshTasks, taskDetail?.id],
  );

  const removeDirectoryTask = useCallback(
    async (id: string) => {
      try {
        await deleteAsrTask(id);
        await refreshTasks();
        message.success("ASR directory task deleted");
      } catch (error) {
        message.error(error instanceof Error ? error.message : "Failed to delete ASR task");
      }
    },
    [refreshTasks],
  );

  const ready = status?.ready;
  const busy = workState === "recording" || workState === "transcribing";
  const showFileProgress =
    Boolean(selectedName) &&
    workState !== "recording" &&
    (workState === "transcribing" || workState === "error" || progress > 0);

  return (
    <div style={{ height: "100%", overflow: "auto" }}>
      <SpeechTab />
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
                  handleFile(file);
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
                    handleFile(file);
                    event.currentTarget.value = "";
                  }
                }}
              />
            </div>

            <Space style={{ marginTop: 16 }} wrap>
              {workState === "recording" ? (
                <Button
                  danger
                  icon={<StopOutlined />}
                  onClick={stopRecording}
                >
                  Stop Mic
                </Button>
              ) : (
                <Button
                  type="primary"
                  icon={<PlayCircleOutlined />}
                  onClick={startRecording}
                  disabled={!ready || workState === "transcribing"}
                >
                  Start Mic
                </Button>
              )}
              <Button
                onClick={() => {
                  abortRef.current?.abort();
                  recordingActiveRef.current = false;
                  if (recorderRef.current?.state === "recording") {
                    recorderRef.current.stop();
                    recorderRef.current = null;
                  }
                  stopMicMeter();
                  if (wsRef.current?.readyState === WebSocket.OPEN) {
                    wsRef.current.send(JSON.stringify({ type: "cancel" }));
                  }
                  wsRef.current?.close();
                  wsRef.current = null;
                  streamRef.current?.getTracks().forEach((track) => track.stop());
                  streamRef.current = null;
                  setWorkState("idle");
                  setProgress(0);
                  appendEvent("transcription stopped by user");
                }}
                disabled={!busy}
              >
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
                fontFamily:
                  "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
                fontSize: 12,
                whiteSpace: "pre-wrap",
              }}
            >
              {events.length ? events.join("\n") : "No stream events yet."}
            </div>
          </section>
        </Space>
      </Card>
      <Card
        title={
          <Space>
            <AudioOutlined />
            <span>Directory Tasks</span>
            <Tag>{tasks.length}</Tag>
          </Space>
        }
        style={{ marginTop: 16 }}
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
          }}
        >
          <Row gutter={[12, 0]} align="bottom">
            <Col xs={24} md={5}>
              <Form.Item name="name" label="Name">
                <Input placeholder="Meeting audio watcher" />
              </Form.Item>
            </Col>
            <Col xs={24} md={7}>
              <Form.Item
                name="audio_dir"
                label="Audio Directory"
                rules={[{ required: true, message: "Enter a local directory path" }]}
              >
                <Input placeholder="/Users/eden/Recordings" />
              </Form.Item>
            </Col>
            <Col xs={12} md={3}>
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
              <Col xs={12} md={3}>
                <Form.Item name="schedule_minute" label="Minute">
                  <InputNumber min={0} max={59} style={{ width: "100%" }} />
                </Form.Item>
              </Col>
            ) : null}
            {taskScheduleKind === "daily" ? (
              <Col xs={12} md={3}>
                <Form.Item name="schedule_time" label="Time">
                  <Input type="time" />
                </Form.Item>
              </Col>
            ) : null}
            {taskScheduleKind === "weekly" ? (
              <>
                <Col xs={12} md={3}>
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
                <Col xs={12} md={3}>
                  <Form.Item name="schedule_time" label="Time">
                    <Input type="time" />
                  </Form.Item>
                </Col>
              </>
            ) : null}
            {taskScheduleKind === "monthly" ? (
              <>
                <Col xs={12} md={2}>
                  <Form.Item name="schedule_day" label="Day">
                    <InputNumber min={1} max={31} style={{ width: "100%" }} />
                  </Form.Item>
                </Col>
                <Col xs={12} md={3}>
                  <Form.Item name="schedule_time" label="Time">
                    <Input type="time" />
                  </Form.Item>
                </Col>
              </>
            ) : null}
            <Col xs={6} md={2}>
              <Form.Item name="recursive" label="Recursive" valuePropName="checked">
                <Switch />
              </Form.Item>
            </Col>
            <Col xs={6} md={2}>
              <Form.Item name="enabled" label="Enabled" valuePropName="checked">
                <Switch />
              </Form.Item>
            </Col>
            <Col xs={24} md={1}>
              <Form.Item label=" ">
                <Button type="primary" onClick={createDirectoryTask}>
                  Add
                </Button>
              </Form.Item>
            </Col>
          </Row>
        </Form>
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
                    onClick={() => void openTaskDetail(record.id)}
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
                const percent = total
                  ? Math.round((record.summary.processed / total) * 100)
                  : 0;
                return (
                  <Space direction="vertical" style={{ width: "100%" }} size={0}>
                    <Progress percent={percent} size="small" />
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      processed {record.summary.processed}, pending {record.summary.pending},
                      failed {record.summary.failed}, deleted after processing{" "}
                      {record.summary.deleted_after_processing}
                    </Text>
                  </Space>
                );
              },
            },
            {
              title: "Schedule",
              render: (_value, record) => (
                <Space direction="vertical" size={0}>
                  <Text>{record.enabled ? formatSchedule(record.schedule) : "Paused"}</Text>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    next {formatTime(record.next_run_at_ms)}
                  </Text>
                </Space>
              ),
            },
            {
              title: "Status",
              render: (_value, record) => (
                <Space direction="vertical" size={0}>
                  <Tag color={record.last_error ? "error" : "success"}>
                    {record.last_error ? "Error" : "Ready"}
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
                  <Button size="small" onClick={() => void runDirectoryTask(record.id)}>
                    Run
                  </Button>
                  <Popconfirm
                    title="Delete this ASR task?"
                    onConfirm={() => void removeDirectoryTask(record.id)}
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
      <Drawer
        title={taskDetail ? `Directory Task: ${taskDetail.name}` : "Directory Task"}
        open={taskDetailOpen}
        onClose={() => setTaskDetailOpen(false)}
        width={860}
      >
        {taskDetail ? (
          <Space direction="vertical" size={16} style={{ width: "100%" }}>
            <Descriptions size="small" bordered column={2}>
              <Descriptions.Item label="Directory" span={2}>
                {taskDetail.audio_dir}
              </Descriptions.Item>
              <Descriptions.Item label="Schedule">
                {taskDetail.enabled ? formatSchedule(taskDetail.schedule) : "Paused"}
              </Descriptions.Item>
              <Descriptions.Item label="Next Run">
                {formatTime(taskDetail.next_run_at_ms)}
              </Descriptions.Item>
              <Descriptions.Item label="Last Run">
                {formatTime(taskDetail.last_run_at_ms)}
              </Descriptions.Item>
              <Descriptions.Item label="Mode">
                {taskDetail.recursive ? "Recursive" : "Flat"}
              </Descriptions.Item>
              <Descriptions.Item label="Processed">
                {taskDetail.summary.processed}
              </Descriptions.Item>
              <Descriptions.Item label="Pending">
                {taskDetail.summary.pending}
              </Descriptions.Item>
              <Descriptions.Item label="Failed">{taskDetail.summary.failed}</Descriptions.Item>
              <Descriptions.Item label="Deleted After Processing">
                {taskDetail.summary.deleted_after_processing}
              </Descriptions.Item>
              <Descriptions.Item label="Last Error" span={2}>
                {taskDetail.last_error || "-"}
              </Descriptions.Item>
            </Descriptions>
            <Table<AsrTaskFileRecord>
              rowKey="key"
              size="small"
              tableLayout="fixed"
              loading={taskDetailLoading}
              dataSource={taskDetail.files}
              pagination={{ pageSize: 8, hideOnSinglePage: true }}
              columns={[
                {
                  title: "File",
                  dataIndex: "source_path",
                  width: 360,
                  render: (value, record) => (
                    <Space direction="vertical" size={2} style={{ width: 320 }}>
                      <Text style={{ width: 320, fontSize: 12 }} ellipsis={{ tooltip: value }}>
                        {value}
                      </Text>
                      {record.output_timeline_path ? (
                        <Button
                          type="link"
                          size="small"
                          style={{ padding: 0, height: "auto", alignSelf: "flex-start" }}
                          loading={taskTimelineLoading}
                          onClick={() => void openTaskTimeline(record)}
                        >
                          Open timeline
                        </Button>
                      ) : null}
                    </Space>
                  ),
                },
                {
                  title: "Status",
                  dataIndex: "status",
                  width: 120,
                  render: (value) => <Tag color={fileStatusColor(value)}>{value}</Tag>,
                },
                {
                  title: "Text",
                  dataIndex: "text_chars",
                  width: 90,
                  render: (value) => `${value} chars`,
                },
                {
                  title: "Recorded",
                  dataIndex: "source_created_at_ms",
                  width: 190,
                  render: (value, record) => (
                    <Space direction="vertical" size={0}>
                      <Text style={{ fontSize: 12 }}>{formatTime(value)}</Text>
                      {record.source_created_at_source ? (
                        <Text type="secondary" style={{ fontSize: 11 }}>
                          {record.source_created_at_source}
                        </Text>
                      ) : null}
                    </Space>
                  ),
                },
                {
                  title: "Duration",
                  dataIndex: "media_duration_ms",
                  width: 110,
                  render: (value) => formatDuration(value),
                },
                {
                  title: "Finished",
                  dataIndex: "finished_at_ms",
                  width: 180,
                  render: (value) => formatTime(value),
                },
                {
                  title: "Result",
                  width: 260,
                  render: (_value, record) => (
                    <Space direction="vertical" size={0}>
                      <Text
                        style={{ width: 240, fontSize: 12 }}
                        ellipsis={{ tooltip: record.output_text_path }}
                      >
                        {record.output_text_path || "-"}
                      </Text>
                      {record.error ? (
                        <Text type="danger" style={{ fontSize: 12 }}>
                          {record.error}
                        </Text>
                      ) : null}
                    </Space>
                  ),
                },
              ]}
            />
            {taskTimeline ? (
              <section
                aria-label="Transcript timeline"
                style={{
                  border: `1px solid ${token.colorBorderSecondary}`,
                  borderRadius: 6,
                  padding: 16,
                  background: token.colorFillQuaternary,
                }}
              >
                <Space
                  align="center"
                  style={{ width: "100%", justifyContent: "space-between", marginBottom: 12 }}
                >
                  <Typography.Title level={5} style={{ margin: 0 }}>
                    File Timeline
                  </Typography.Title>
                  <Tag color="blue">{taskTimeline.segments.length} segments</Tag>
                </Space>
                <Descriptions size="small" bordered column={2} style={{ marginBottom: 12 }}>
                  <Descriptions.Item label="File" span={2}>
                    {taskTimeline.source_path}
                  </Descriptions.Item>
                  <Descriptions.Item label="Recorded">
                    {formatTime(taskTimeline.source_created_at_ms)}
                  </Descriptions.Item>
                  <Descriptions.Item label="Source">
                    {taskTimeline.source_created_at_source || "-"}
                  </Descriptions.Item>
                  <Descriptions.Item label="Duration">
                    {formatDuration(taskTimeline.media_duration_ms)}
                  </Descriptions.Item>
                  <Descriptions.Item label="Segments">
                    {taskTimeline.segments.length}
                  </Descriptions.Item>
                </Descriptions>
                <Row gutter={16}>
                  <Col xs={24} lg={11}>
                    <Text strong>Segments</Text>
                    <div
                      style={{
                        marginTop: 12,
                        maxHeight: 420,
                        overflow: "auto",
                        paddingRight: 8,
                      }}
                    >
                      {taskTimeline.segments.length > 0 ? (
                        <AntTimeline
                          mode="left"
                          items={taskTimeline.segments.map((segment) => ({
                            label: (
                              <Space direction="vertical" size={0}>
                                <Text code>
                                  {formatDuration(segment.audio_start_ms)} -{" "}
                                  {formatDuration(segment.audio_end_ms)}
                                </Text>
                                <Text type="secondary" style={{ fontSize: 12 }}>
                                  {segment.absolute_start_ms !== undefined &&
                                  segment.absolute_end_ms !== undefined
                                    ? `${formatTime(segment.absolute_start_ms)} - ${formatTime(segment.absolute_end_ms)}`
                                    : "No wall-clock time"}
                                </Text>
                              </Space>
                            ),
                            children: (
                              <Paragraph style={{ marginBottom: 0, whiteSpace: "pre-wrap" }}>
                                {segment.text}
                              </Paragraph>
                            ),
                          }))}
                        />
                      ) : (
                        <Text type="secondary">No transcript segments were produced.</Text>
                      )}
                    </div>
                  </Col>
                  <Col xs={24} lg={13}>
                    <Text strong>Full Transcript</Text>
                    <Paragraph
                      style={{
                        minHeight: 180,
                        maxHeight: 420,
                        overflow: "auto",
                        marginTop: 12,
                        marginBottom: 0,
                        padding: 12,
                        whiteSpace: "pre-wrap",
                        background: token.colorBgContainer,
                        border: `1px solid ${token.colorBorderSecondary}`,
                        borderRadius: 6,
                      }}
                    >
                      {taskTimeline.segments.length > 0
                        ? taskTimeline.segments.map((segment) => segment.text).join("\n")
                        : "No transcript text."}
                    </Paragraph>
                  </Col>
                </Row>
              </section>
            ) : null}
          </Space>
        ) : (
          <Text type="secondary">Select a task to view progress and file results.</Text>
        )}
      </Drawer>
    </div>
  );
}

function buildTaskSchedule(values: Record<string, unknown>): AsrTaskSchedule {
  const kind = String(values.schedule_kind ?? "daily");
  const { hour, minute } = parseTimeInput(values.schedule_time);
  if (kind === "hourly") {
    return {
      kind: "hourly",
      minute: Number(values.schedule_minute ?? 0),
    };
  }
  if (kind === "weekly") {
    return {
      kind: "weekly",
      weekday: Number(values.schedule_weekday ?? 1),
      hour,
      minute,
    };
  }
  if (kind === "monthly") {
    return {
      kind: "monthly",
      day: Number(values.schedule_day ?? 1),
      hour,
      minute,
    };
  }
  return { kind: "daily", hour, minute };
}

function parseTimeInput(value: unknown): { hour: number; minute: number } {
  if (typeof value !== "string") {
    return { hour: 2, minute: 0 };
  }
  const [hour, minute] = value.split(":").map((part) => Number(part));
  if (
    Number.isInteger(hour) &&
    Number.isInteger(minute) &&
    hour >= 0 &&
    hour <= 23 &&
    minute >= 0 &&
    minute <= 59
  ) {
    return { hour, minute };
  }
  return { hour: 2, minute: 0 };
}

function formatSchedule(schedule?: AsrTaskSchedule): string {
  if (!schedule) {
    return "Daily at 02:00";
  }
  switch (schedule.kind) {
    case "hourly":
      return `Hourly at minute ${padTime(schedule.minute)}`;
    case "daily":
      return `Daily at ${padTime(schedule.hour)}:${padTime(schedule.minute)}`;
    case "weekly":
      return `Weekly ${weekdayLabel(schedule.weekday)} ${padTime(schedule.hour)}:${padTime(schedule.minute)}`;
    case "monthly":
      return `Monthly day ${schedule.day} ${padTime(schedule.hour)}:${padTime(schedule.minute)}`;
  }
}

function padTime(value: number): string {
  return String(value).padStart(2, "0");
}

function weekdayLabel(value: number): string {
  return ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][value - 1] ?? "Mon";
}

function fileStatusColor(status: AsrTaskFileRecord["status"]): string {
  if (status === "success") {
    return "success";
  }
  if (status === "failed") {
    return "error";
  }
  if (status === "processing") {
    return "processing";
  }
  return "default";
}

function formatTime(value?: number): string {
  if (!value) {
    return "-";
  }
  return new Date(value).toLocaleString();
}

function formatDuration(value?: number): string {
  if (value === undefined || value === null) {
    return "-";
  }
  const millis = Math.max(0, Math.round(value));
  const ms = millis % 1000;
  const totalSeconds = Math.floor(millis / 1000);
  const seconds = totalSeconds % 60;
  const totalMinutes = Math.floor(totalSeconds / 60);
  const minutes = totalMinutes % 60;
  const hours = Math.floor(totalMinutes / 60);
  return `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}.${String(ms).padStart(3, "0")}`;
}

function dedupeTranscript(committed: string, candidate: string): string {
  const base = committed.trim();
  const next = candidate.trim();
  if (!next || base.includes(next)) {
    return "";
  }
  if (!base) {
    return next;
  }
  const max = Math.min(base.length, next.length);
  for (let len = max; len > 0; len -= 1) {
    if (base.slice(base.length - len) === next.slice(0, len)) {
      return next.slice(len);
    }
  }
  return next;
}

function appendTranscriptDelta(committed: string, delta: string): string {
  delta = delta.trim();
  if (!delta) {
    return committed;
  }
  if (!committed) {
    return delta;
  }
  const first = delta[0];
  const last = committed[committed.length - 1];
  const cjk = /[\u3400-\u9fff\uf900-\ufaff]/;
  const punctuation = /^[\p{P}\p{S}]/u;
  if (punctuation.test(delta) || cjk.test(first) || cjk.test(last)) {
    return `${committed}${delta}`;
  }
  return `${committed} ${delta}`;
}
