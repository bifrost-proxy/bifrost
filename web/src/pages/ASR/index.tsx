import { useCallback, useEffect, useRef, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Form, message, theme } from "antd";
import {
  ASR_PARAMS_CHANGED_EVENT,
  ASR_STATUS_CHANGED_EVENT,
  buildAsrRealtimeUrl,
  createAsrTask,
  deleteAsrTask,
  getDailyAgentReport,
  getAsrStatus,
  getAsrTask,
  getAsrTaskDailyDocument,
  getAsrTaskFileTimeline,
  listAsrTasks,
  loadAsrParams,
  pauseAsrTask,
  resumeAsrTask,
  runAsrTask,
  streamAsrTranscription,
  type AsrDirectoryTask,
  type AsrDirectoryTaskDetail,
  type AsrDailyAgentReportDetail,
  type AsrStatus,
  type AsrStreamEvent,
  type AsrTaskDailyDocumentDetail,
  type AsrTaskFileRecord,
  type AsrTranscriptTimeline,
} from "../../api/asr";
import SpeechTab from "../Settings/tabs/SpeechTab";
import {
  appendTranscriptDelta,
  buildTaskSchedule,
  dedupeTranscript,
  EMPTY_MIC_LEVELS,
  MIC_METER_BARS,
  MIC_WINDOW_MS,
  type WorkState,
} from "./asrUtils";
import DirectoryTaskDetailPage from "./components/DirectoryTaskDetailPage";
import {
  isDirectoryTaskDetailTabKey,
  type DirectoryTaskDetailTabKey,
} from "./components/taskDetailRoute";
import DirectoryTasksPanel from "./components/DirectoryTasksPanel";
import SpeechWorkbench from "./components/SpeechWorkbench";

export default function ASR() {
  const { token } = theme.useToken();
  const [searchParams, setSearchParams] = useSearchParams();
  const [taskForm] = Form.useForm();
  const taskScheduleKind = Form.useWatch("schedule_kind", taskForm) ?? "daily";
  const selectedTaskId = searchParams.get("asrTask");
  const selectedFileKey = searchParams.get("asrFile");
  const selectedDailyDate = searchParams.get("asrDay");
  const selectedDailyAgentReportDate = searchParams.get("asrDailyReport");
  const selectedTaskTabParam = searchParams.get("asrTaskTab");
  const selectedTaskTab: DirectoryTaskDetailTabKey = (() => {
    if (isDirectoryTaskDetailTabKey(selectedTaskTabParam)) {
      return selectedTaskTabParam;
    }
    if (selectedDailyAgentReportDate) {
      return "daily-agent-records";
    }
    if (selectedDailyDate) {
      return "daily";
    }
    return "files";
  })();
  const [status, setStatus] = useState<AsrStatus | null>(null);
  const [tasks, setTasks] = useState<AsrDirectoryTask[]>([]);
  const [tasksLoading, setTasksLoading] = useState(false);
  const [taskDetail, setTaskDetail] = useState<AsrDirectoryTaskDetail | null>(null);
  const [taskDetailLoading, setTaskDetailLoading] = useState(false);
  const [taskTimeline, setTaskTimeline] = useState<AsrTranscriptTimeline | null>(null);
  const [taskTimelineLoading, setTaskTimelineLoading] = useState(false);
  const [taskDailyDocument, setTaskDailyDocument] =
    useState<AsrTaskDailyDocumentDetail | null>(null);
  const [taskDailyDocumentLoading, setTaskDailyDocumentLoading] = useState(false);
  const [taskDailyAgentReport, setTaskDailyAgentReport] =
    useState<AsrDailyAgentReportDetail | null>(null);
  const [taskDailyAgentReportLoading, setTaskDailyAgentReportLoading] = useState(false);
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

  const cancelWork = useCallback(() => {
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
        runtime_strategy: values.runtime_strategy,
      });
      taskForm.resetFields();
      await refreshTasks();
      message.success("ASR directory task created");
      return true;
    } catch (error) {
      if (error && typeof error === "object" && "errorFields" in error) {
        return false;
      }
      message.error(error instanceof Error ? error.message : "Failed to create ASR task");
      return false;
    }
  }, [getCurrentAsrParams, refreshTasks, taskForm]);

  const loadTaskDetail = useCallback(async (id: string) => {
    setTaskDetail(null);
    setTaskDetailLoading(true);
    try {
      setTaskDetail(await getAsrTask(id));
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to load ASR task");
    } finally {
      setTaskDetailLoading(false);
    }
  }, []);

  const loadTaskTimeline = useCallback(async (taskId: string, fileKey: string) => {
    setTaskTimeline(null);
    setTaskTimelineLoading(true);
    try {
      setTaskTimeline(await getAsrTaskFileTimeline(taskId, fileKey));
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to load ASR timeline");
    } finally {
      setTaskTimelineLoading(false);
    }
  }, []);

  const loadTaskDailyDocument = useCallback(async (taskId: string, date: string) => {
    setTaskDailyDocument(null);
    setTaskDailyDocumentLoading(true);
    try {
      setTaskDailyDocument(await getAsrTaskDailyDocument(taskId, date));
    } catch (error) {
      message.error(
        error instanceof Error ? error.message : "Failed to load ASR daily document",
      );
    } finally {
      setTaskDailyDocumentLoading(false);
    }
  }, []);

  const loadDailyAgentReport = useCallback(async (taskId: string, date: string) => {
    setTaskDailyAgentReport(null);
    setTaskDailyAgentReportLoading(true);
    try {
      setTaskDailyAgentReport(await getDailyAgentReport(taskId, date));
    } catch (error) {
      message.error(
        error instanceof Error ? error.message : "Failed to load Daily Agent report",
      );
    } finally {
      setTaskDailyAgentReportLoading(false);
    }
  }, []);

  const openTaskDetail = useCallback(
    (id: string) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set("aiSection", "tools-asr");
          next.set("asrTask", id);
          next.delete("asrFile");
          next.delete("asrDay");
          next.delete("asrDailyReport");
          next.delete("asrTaskTab");
          return next;
        },
        { replace: false },
      );
    },
    [setSearchParams],
  );

  const closeTaskDetail = useCallback(() => {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.delete("asrTask");
        next.delete("asrFile");
        next.delete("asrDay");
        next.delete("asrDailyReport");
        next.delete("asrTaskTab");
        return next;
      },
      { replace: false },
    );
  }, [setSearchParams]);

  const openTaskFile = useCallback(
    (file: AsrTaskFileRecord) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set("aiSection", "tools-asr");
          next.set("asrTask", file.task_id);
          next.set("asrFile", file.key);
          next.delete("asrDay");
          next.delete("asrDailyReport");
          next.delete("asrTaskTab");
          return next;
        },
        { replace: false },
      );
    },
    [setSearchParams],
  );

  const closeTaskFile = useCallback(() => {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.delete("asrFile");
        return next;
      },
      { replace: false },
    );
  }, [setSearchParams]);

  const openTaskDailyDocument = useCallback(
    (date: string) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set("aiSection", "tools-asr");
          if (selectedTaskId) {
            next.set("asrTask", selectedTaskId);
          }
          next.set("asrTaskTab", "daily");
          next.set("asrDay", date);
          next.delete("asrFile");
          next.delete("asrDailyReport");
          return next;
        },
        { replace: false },
      );
    },
    [selectedTaskId, setSearchParams],
  );

  const closeTaskDailyDocument = useCallback(() => {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.delete("asrDay");
        next.set("asrTaskTab", "daily");
        return next;
      },
      { replace: false },
    );
  }, [setSearchParams]);

  const openDailyAgentReport = useCallback(
    (date: string) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set("aiSection", "tools-asr");
          if (selectedTaskId) {
            next.set("asrTask", selectedTaskId);
          }
          next.set("asrTaskTab", "daily-agent-records");
          next.set("asrDailyReport", date);
          next.delete("asrFile");
          next.delete("asrDay");
          return next;
        },
        { replace: false },
      );
    },
    [selectedTaskId, setSearchParams],
  );

  const closeDailyAgentReport = useCallback(() => {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.delete("asrDailyReport");
        next.set("asrTaskTab", "daily-agent-records");
        return next;
      },
      { replace: false },
    );
  }, [setSearchParams]);

  const changeTaskTab = useCallback(
    (tab: DirectoryTaskDetailTabKey) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set("aiSection", "tools-asr");
          if (tab === "files") {
            next.delete("asrTaskTab");
          } else {
            next.set("asrTaskTab", tab);
          }
          next.delete("asrFile");
          next.delete("asrDay");
          next.delete("asrDailyReport");
          return next;
        },
        { replace: false },
      );
    },
    [setSearchParams],
  );

  useEffect(() => {
    if (!selectedTaskId) {
      setTaskDetail(null);
      setTaskTimeline(null);
      setTaskDailyDocument(null);
      setTaskDailyAgentReport(null);
      return;
    }
    void loadTaskDetail(selectedTaskId);
  }, [loadTaskDetail, selectedTaskId]);

  useEffect(() => {
    if (!selectedTaskId || !selectedFileKey) {
      setTaskTimeline(null);
      return;
    }
    void loadTaskTimeline(selectedTaskId, selectedFileKey);
  }, [loadTaskTimeline, selectedFileKey, selectedTaskId]);

  useEffect(() => {
    if (!selectedTaskId || !selectedDailyDate) {
      setTaskDailyDocument(null);
      return;
    }
    void loadTaskDailyDocument(selectedTaskId, selectedDailyDate);
  }, [loadTaskDailyDocument, selectedDailyDate, selectedTaskId]);

  useEffect(() => {
    if (!selectedTaskId || !selectedDailyAgentReportDate) {
      setTaskDailyAgentReport(null);
      return;
    }
    void loadDailyAgentReport(selectedTaskId, selectedDailyAgentReportDate);
  }, [loadDailyAgentReport, selectedDailyAgentReportDate, selectedTaskId]);

  // Auto-refresh task detail every 3 seconds while the task or bulk chunk retry is running.
  useEffect(() => {
    const bulkRetryActive =
      taskDetail?.bulk_retry?.status === "queued" ||
      taskDetail?.bulk_retry?.status === "running";
    if ((!taskDetail?.summary?.running && !bulkRetryActive) || !taskDetail?.id) return;
    const timer = setInterval(() => {
      void (async () => {
        try {
          const updated = await getAsrTask(taskDetail.id);
          setTaskDetail(updated);
          const updatedBulkRetryActive =
            updated.bulk_retry?.status === "queued" ||
            updated.bulk_retry?.status === "running";
          if (!updated.summary.running && !updatedBulkRetryActive) {
            // Task finished — also refresh the task list.
            void refreshTasks();
          }
        } catch {
          // Ignore refresh errors silently.
        }
      })();
    }, 3000);
    return () => clearInterval(timer);
  }, [
    taskDetail?.summary?.running,
    taskDetail?.bulk_retry?.status,
    taskDetail?.id,
    refreshTasks,
  ]);

  const runDirectoryTask = useCallback(
    async (id: string) => {
      try {
        await runAsrTask(id);
        message.info("ASR task started");
        // Immediately refresh to show running state.
        if (taskDetail?.id === id) {
          void loadTaskDetail(id);
        }
        await refreshTasks();
      } catch (error) {
        message.error(error instanceof Error ? error.message : "Failed to run ASR task");
      }
    },
    [loadTaskDetail, refreshTasks, taskDetail?.id],
  );

  const pauseDirectoryTask = useCallback(
    async (id: string, force = false) => {
      try {
        const result = await pauseAsrTask(id, { force });
        message.info(result.message);
        if (taskDetail?.id === id) {
          void loadTaskDetail(id);
        }
        await refreshTasks();
      } catch (error) {
        message.error(error instanceof Error ? error.message : "Failed to pause ASR task");
      }
    },
    [loadTaskDetail, refreshTasks, taskDetail?.id],
  );

  const resumeDirectoryTask = useCallback(
    async (id: string) => {
      try {
        const result = await resumeAsrTask(id);
        message.success(result.message);
        if (taskDetail?.id === id) {
          void loadTaskDetail(id);
        }
        await refreshTasks();
      } catch (error) {
        message.error(error instanceof Error ? error.message : "Failed to resume ASR task");
      }
    },
    [loadTaskDetail, refreshTasks, taskDetail?.id],
  );

  const removeDirectoryTask = useCallback(
    async (id: string) => {
      try {
        await deleteAsrTask(id);
        if (taskDetail?.id === id) {
          closeTaskDetail();
        }
        await refreshTasks();
        message.success("ASR directory task deleted");
      } catch (error) {
        message.error(error instanceof Error ? error.message : "Failed to delete ASR task");
      }
    },
    [closeTaskDetail, refreshTasks, taskDetail?.id],
  );

  const ready = status?.ready;
  const busy = workState === "recording" || workState === "transcribing";
  const showFileProgress =
    Boolean(selectedName) &&
    workState !== "recording" &&
    (workState === "transcribing" || workState === "error" || progress > 0);

  if (selectedTaskId) {
    return (
      <DirectoryTaskDetailPage
        token={token}
        taskDetail={taskDetail}
        taskDetailLoading={taskDetailLoading}
        selectedFileKey={selectedFileKey}
        selectedDailyDate={selectedDailyDate}
        selectedDailyAgentReportDate={selectedDailyAgentReportDate}
        selectedTaskTab={selectedTaskTab}
        taskTimeline={taskTimeline}
        taskTimelineLoading={taskTimelineLoading}
        taskDailyDocument={taskDailyDocument}
        taskDailyDocumentLoading={taskDailyDocumentLoading}
        taskDailyAgentReport={taskDailyAgentReport}
        taskDailyAgentReportLoading={taskDailyAgentReportLoading}
        onBackToTasks={closeTaskDetail}
        onBackToTaskFiles={closeTaskFile}
        onBackToDailyDocuments={closeTaskDailyDocument}
        onBackToDailyAgentReports={closeDailyAgentReport}
        onRefreshTask={(id) => void loadTaskDetail(id)}
        onRunTask={(id) => void runDirectoryTask(id)}
        onPauseTask={(id, force) => void pauseDirectoryTask(id, force)}
        onResumeTask={(id) => void resumeDirectoryTask(id)}
        onOpenFile={openTaskFile}
        onOpenDailyDocument={openTaskDailyDocument}
        onOpenDailyAgentReport={openDailyAgentReport}
        onChangeTaskTab={changeTaskTab}
      />
    );
  }

  return (
    <div style={{ height: "100%", overflow: "auto" }}>
      <SpeechTab />
      <DirectoryTasksPanel
        taskForm={taskForm}
        taskScheduleKind={taskScheduleKind}
        tasks={tasks}
        tasksLoading={tasksLoading}
        onCreateTask={createDirectoryTask}
        onOpenTask={openTaskDetail}
        onRunTask={(id) => void runDirectoryTask(id)}
        onPauseTask={(id, force) => void pauseDirectoryTask(id, force)}
        onResumeTask={(id) => void resumeDirectoryTask(id)}
        onRemoveTask={(id) => void removeDirectoryTask(id)}
      />
      <SpeechWorkbench
        token={token}
        ready={ready}
        status={status}
        workState={workState}
        progress={progress}
        selectedName={selectedName}
        transcript={transcript}
        events={events}
        errorText={errorText}
        micLevels={micLevels}
        micPeak={micPeak}
        fileInputRef={fileInputRef}
        busy={busy}
        showFileProgress={showFileProgress}
        onFile={handleFile}
        onStartRecording={() => void startRecording()}
        onStopRecording={stopRecording}
        onCancel={cancelWork}
      />
    </div>
  );
}
