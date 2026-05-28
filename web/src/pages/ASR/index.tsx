import { useCallback, useEffect, useRef, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Form, message, theme } from "antd";
import {
  ASR_PARAMS_CHANGED_EVENT,
  ASR_STATUS_CHANGED_EVENT,
  buildVoiceRealtimeUrl,
  createAsrOfflineJob,
  createAsrTask,
  deleteAsrTask,
  getAsrOfflineJob,
  getAsrOfflineJobArtifact,
  getDailyAgentReport,
  getAsrCapabilities,
  getAsrStatus,
  getAsrTask,
  getAsrTaskDailyDocument,
  getAsrTaskFileTimeline,
  getAsrExternalImportStatus,
  listAsrTasks,
  loadAsrParams,
  loadVoiceRealtimeParams,
  pauseAsrTask,
  resumeAsrTask,
  runAsrExternalImport,
  runAsrTask,
  saveAsrParams,
  startAsrService,
  stopAsrService,
  updateAsrTask,
  getSpeechPipelinesStatus,
  type AsrDirectoryTask,
  type AsrDirectoryTaskDetail,
  type AsrDailyAgentReportDetail,
  type AsrExternalDeviceBinding,
  type AsrExternalImportRunProgress,
  type AsrPauseMode,
  type AsrStatus,
  type AsrConnectionParams,
  type AsrCapabilities,
  type AsrOfflineJob,
  type SpeechPipelinesStatus,
  type AsrTaskDailyDocumentDetail,
  type AsrTaskFileRecord,
  type AsrTranscriptTimeline,
  type VoiceRealtimeEvent,
} from "../../api/asr";
import SpeechTab from "../Settings/tabs/SpeechTab";
import {
  appendTranscriptDelta,
  buildTaskSchedule,
  dedupeTranscript,
  encodePcm16Chunk,
  EMPTY_MIC_LEVELS,
  MIC_METER_BARS,
  VOICE_REALTIME_CHUNK_MS,
  VOICE_REALTIME_SAMPLE_RATE,
  type WorkState,
} from "./asrUtils";
import DirectoryTaskDetailPage from "./components/DirectoryTaskDetailPage";
import {
  isDirectoryTaskDetailTabKey,
  type DirectoryTaskDetailTabKey,
} from "./components/taskDetailRoute";
import DirectoryTasksPanel from "./components/DirectoryTasksPanel";
import DiarizationSetupCard from "./components/DiarizationSetupCard";
import SpeechWorkbench from "./components/SpeechWorkbench";
import VoiceWakeActionsCard from "./components/VoiceWakeActionsCard";

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
  const [capabilities, setCapabilities] = useState<AsrCapabilities | null>(null);
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
  const [externalImportProgressByTask, setExternalImportProgressByTask] = useState<
    Record<string, AsrExternalImportRunProgress>
  >({});
  const [workState, setWorkState] = useState<WorkState>("idle");
  const [progress, setProgress] = useState(0);
  const [selectedName, setSelectedName] = useState("");
  const [transcript, setTranscript] = useState("");
  const [events, setEvents] = useState<string[]>([]);
  const [errorText, setErrorText] = useState("");
  const [speechStatus, setSpeechStatus] = useState<SpeechPipelinesStatus | null>(null);
  const [offlineJob, setOfflineJob] = useState<AsrOfflineJob | null>(null);
  const [offlineArtifacts, setOfflineArtifacts] = useState<Record<string, string>>({});
  const [micLevels, setMicLevels] = useState<number[]>(EMPTY_MIC_LEVELS);
  const [micPeak, setMicPeak] = useState(0);
  const [workbenchParams, setWorkbenchParams] = useState(() => loadAsrParams());
  const [serviceBusy, setServiceBusy] = useState(false);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const audioContextRef = useRef<AudioContext | null>(null);
  const voiceAudioContextRef = useRef<AudioContext | null>(null);
  const voiceSourceRef = useRef<MediaStreamAudioSourceNode | null>(null);
  const voiceProcessorRef = useRef<ScriptProcessorNode | null>(null);
  const voiceWorkletRef = useRef<AudioWorkletNode | null>(null);
  const micMeterRafRef = useRef<number | null>(null);
  const committedTranscriptRef = useRef("");
  const partialTranscriptRef = useRef("");
  const recordingActiveRef = useRef(false);
  const asrSupported =
    capabilities?.qwen3_asr.enabled === true && capabilities.qwen3_asr.hidden === false;

  useEffect(() => {
    let alive = true;
    void getAsrCapabilities()
      .then((next) => {
        if (alive) {
          setCapabilities(next);
        }
      })
      .catch(() => {
        if (alive) {
          setCapabilities({
            platform: "unknown",
            arch: "unknown",
            supported_target: "macos-aarch64",
            qwen3_asr: { enabled: false, hidden: true, platform_supported: false },
            local_transcription: { enabled: false, hidden: true, platform_supported: false },
            speech_workbench: { enabled: false, hidden: true, platform_supported: false },
            directory_tasks: { enabled: false, hidden: true, platform_supported: false },
            speaker_diarization: { enabled: false, hidden: true, platform_supported: false },
            voiceprint: { enabled: false, hidden: true, platform_supported: false },
            voice_wake_asr: { enabled: false, hidden: true, platform_supported: false },
          });
        }
      });
    return () => {
      alive = false;
    };
  }, []);

  const getCurrentAsrParams = useCallback(
    (): AsrConnectionParams => ({
      ...workbenchParams,
      ownerModule: "speech_workbench",
    }),
    [workbenchParams],
  );
  const getCurrentVoiceParams = useCallback(
    () => ({
      ...loadVoiceRealtimeParams(),
      host: workbenchParams.host,
      language: workbenchParams.language,
      model: workbenchParams.model,
      ownerModule: "speech_workbench",
    }),
    [workbenchParams],
  );

  const appendEvent = useCallback((line: string) => {
    setEvents((prev) => [...prev.slice(-79), line]);
  }, []);

  const refreshStatus = useCallback(async () => {
    try {
      const [next, nextSpeechStatus] = await Promise.all([
        getAsrStatus(getCurrentAsrParams()),
        getSpeechPipelinesStatus(),
      ]);
      setStatus(next);
      setSpeechStatus(nextSpeechStatus);
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

  const applyTaskControlUpdate = useCallback((nextTask: AsrDirectoryTask) => {
    setTasks((prev) =>
      prev.map((task) =>
        task.id === nextTask.id
          ? {
              ...task,
              ...nextTask,
              summary: {
                ...task.summary,
                ...nextTask.summary,
              },
              bulk_retry: nextTask.bulk_retry,
            }
          : task,
      ),
    );
    setTaskDetail((prev) =>
      prev?.id === nextTask.id
        ? {
            ...prev,
            ...nextTask,
            summary: {
              ...prev.summary,
              ...nextTask.summary,
            },
            bulk_retry: nextTask.bulk_retry,
            files: prev.files,
            daily_documents: prev.daily_documents,
          }
        : prev,
    );
  }, []);

  const refreshExternalImportStatus = useCallback(
    async (taskId: string) => {
      try {
        const status = await getAsrExternalImportStatus(taskId);
        setExternalImportProgressByTask((prev) => {
          if (!status.current_run) {
            const next = { ...prev };
            delete next[taskId];
            return next;
          }
          return { ...prev, [taskId]: status.current_run };
        });
        return status.current_run;
      } catch (error) {
        appendEvent(
          `external import status error: ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
        return undefined;
      }
    },
    [appendEvent],
  );

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

  const stopVoicePcmStreaming = useCallback(() => {
    voiceWorkletRef.current?.disconnect();
    voiceWorkletRef.current = null;
    voiceProcessorRef.current?.disconnect();
    voiceProcessorRef.current = null;
    voiceSourceRef.current?.disconnect();
    voiceSourceRef.current = null;
    void voiceAudioContextRef.current?.close();
    voiceAudioContextRef.current = null;
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
    if (!asrSupported) {
      return;
    }
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
      stopVoicePcmStreaming();
      stopMicMeter();
    };
  }, [asrSupported, refreshStatus, refreshTasks, stopMicMeter, stopVoicePcmStreaming]);

  useEffect(() => {
    saveAsrParams(workbenchParams);
  }, [workbenchParams]);

  const updateWorkbenchParams = useCallback(
    (next: Parameters<typeof setWorkbenchParams>[0]) => {
      setWorkbenchParams((previous) => ({
        ...(typeof next === "function" ? next(previous) : next),
        ownerModule: "speech_workbench",
      }));
    },
    [],
  );

  const startWorkbenchService = useCallback(async () => {
    setServiceBusy(true);
    setErrorText("");
    try {
      const result = await startAsrService({
        ...getCurrentAsrParams(),
        ownerModule: "speech_workbench",
      });
      if (!result.ready) {
        setErrorText(result.detail || result.message);
      }
      window.dispatchEvent(new Event(ASR_STATUS_CHANGED_EVENT));
      await refreshStatus();
    } catch (error) {
      setErrorText(error instanceof Error ? error.message : String(error));
    } finally {
      setServiceBusy(false);
    }
  }, [getCurrentAsrParams, refreshStatus]);

  const stopWorkbenchService = useCallback(async () => {
    setServiceBusy(true);
    setErrorText("");
    try {
      await stopAsrService({
        ...getCurrentAsrParams(),
        ownerModule: "speech_workbench",
      });
      window.dispatchEvent(new Event(ASR_STATUS_CHANGED_EVENT));
      await refreshStatus();
    } catch (error) {
      setErrorText(error instanceof Error ? error.message : String(error));
    } finally {
      setServiceBusy(false);
    }
  }, [getCurrentAsrParams, refreshStatus]);

  const handleVoiceRealtimeEvent = useCallback(
    (event: VoiceRealtimeEvent) => {
      if (event.type === "connected" || event.type === "source_ready") {
        setProgress(event.type === "source_ready" ? 5 : 1);
        appendEvent(`${event.type}: ${event.message || event.detail || "voice stream ready"}`);
      } else if (event.type === "asr_partial") {
        if (event.committed !== undefined) {
          committedTranscriptRef.current = event.committed;
        }
        partialTranscriptRef.current = event.text || event.delta || "";
        appendEvent(
          `partial[${event.window_index ?? 0}]: captured ${event.captured_at_ms ?? 0}ms`,
        );
        renderTranscript();
      } else if (event.type === "asr_stable_delta") {
        const delta = dedupeTranscript(
          committedTranscriptRef.current,
          event.delta || event.text || "",
        );
        if (delta) {
          committedTranscriptRef.current = appendTranscriptDelta(
            committedTranscriptRef.current,
            delta,
          );
        }
        partialTranscriptRef.current = "";
        appendEvent(
          `stable[${event.window_index ?? 0}]: emitted ${event.emitted_at_ms ?? 0}ms`,
        );
        renderTranscript();
      } else if (event.type === "asr_final_utterance") {
        if (event.committed !== undefined) {
          committedTranscriptRef.current = event.committed;
        } else {
          const candidate = event.delta || event.text || "";
          const delta = dedupeTranscript(committedTranscriptRef.current, candidate);
          if (delta) {
            committedTranscriptRef.current = appendTranscriptDelta(
              committedTranscriptRef.current,
              delta,
            );
          }
        }
        partialTranscriptRef.current = "";
        appendEvent(`final: emitted ${event.emitted_at_ms ?? 0}ms`);
        renderTranscript();
      } else if (event.type === "worker_idle_unloaded") {
        if (event.committed !== undefined) {
          committedTranscriptRef.current = event.committed;
        }
        partialTranscriptRef.current = "";
        appendEvent(event.message || "voice worker unloaded after idle timeout");
        renderTranscript();
      } else if (event.type === "error") {
        setWorkState("error");
        setErrorText(event.detail ? `${event.message}\n${event.detail}` : event.message || "");
        appendEvent(`error: ${event.message || "voice stream failed"}`);
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
      setOfflineJob(null);
      setOfflineArtifacts({});

      try {
        const created = await createAsrOfflineJob(
          blob,
          fileName,
          getCurrentAsrParams(),
          {
            pipelineProfile: "offline-speaker-subtitle-local",
            speakerAware: true,
          },
        );
        setOfflineJob(created);
        appendEvent(`offline job: ${created.job_id} queued`);
        let current = created;
        while (!controller.signal.aborted) {
          current = await getAsrOfflineJob(created.job_id);
          setOfflineJob(current);
          appendEvent(`offline job: ${current.status}`);
          if (current.status === "succeeded" || current.status === "failed") {
            break;
          }
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
        if (controller.signal.aborted) {
          return;
        }
        if (current.status !== "succeeded") {
          throw new Error(current.error || "Offline subtitle job failed");
        }
        const [timeline, text, srt, vtt] = await Promise.all([
          getAsrOfflineJobArtifact(created.job_id, "timeline_json"),
          getAsrOfflineJobArtifact(created.job_id, "txt"),
          getAsrOfflineJobArtifact(created.job_id, "srt"),
          getAsrOfflineJobArtifact(created.job_id, "vtt"),
        ]);
        setOfflineArtifacts({ timeline_json: timeline, txt: text, srt, vtt });
        committedTranscriptRef.current = text;
        partialTranscriptRef.current = "";
        setTranscript(text);
        setProgress(100);
        setWorkState(recordingActiveRef.current ? "recording" : "idle");
        appendEvent("offline job: artifacts ready");
        void refreshStatus();
      } catch (error) {
        if (controller.signal.aborted) {
          return;
        }
        const text = error instanceof Error ? error.message : String(error);
        setWorkState("error");
        setErrorText(text);
        appendEvent(`offline job error: ${text}`);
      }
    },
    [appendEvent, getCurrentAsrParams, refreshStatus, resetTranscript],
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

      const voiceParams = getCurrentVoiceParams();
      const ws = new WebSocket(
        buildVoiceRealtimeUrl({
          ...voiceParams,
          chunkMs: VOICE_REALTIME_CHUNK_MS,
        }),
      );
      wsRef.current = ws;
      ws.onmessage = (event) => {
        try {
          handleVoiceRealtimeEvent(JSON.parse(String(event.data)) as VoiceRealtimeEvent);
        } catch (error) {
          appendEvent(
            `websocket parse error: ${error instanceof Error ? error.message : String(error)}`,
          );
        }
      };
      ws.onerror = () => {
        recordingActiveRef.current = false;
        stopVoicePcmStreaming();
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
          stopVoicePcmStreaming();
          stopMicMeter();
          stream.getTracks().forEach((track) => track.stop());
          streamRef.current = null;
          setWorkState("idle");
          appendEvent("websocket: microphone stream closed");
        }
      };
      ws.onopen = async () => {
        try {
          const VoiceAudioContext =
            window.AudioContext ||
            (window as Window & { webkitAudioContext?: typeof AudioContext })
              .webkitAudioContext;
          if (!VoiceAudioContext) {
            throw new Error("This browser does not expose WebAudio recording APIs.");
          }
          const voiceAudioContext = new VoiceAudioContext({
            sampleRate: VOICE_REALTIME_SAMPLE_RATE,
          });
          voiceAudioContextRef.current = voiceAudioContext;
          const source = voiceAudioContext.createMediaStreamSource(stream);
          voiceSourceRef.current = source;
          ws.send(
            JSON.stringify({
              type: "start",
              source: "web_mic",
              sample_rate: VOICE_REALTIME_SAMPLE_RATE,
              channels: 1,
              format: "pcm_s16le",
            }),
          );
          const WorkletNode =
            (window as Window & { AudioWorkletNode?: typeof AudioWorkletNode })
              .AudioWorkletNode;
          if (voiceAudioContext.audioWorklet && WorkletNode) {
            const workletUrl = URL.createObjectURL(
              new Blob(
                [
                  `
class BifrostVoicePcm16Processor extends AudioWorkletProcessor {
  process(inputs, outputs) {
    const input = inputs[0] && inputs[0][0];
    const output = outputs[0] && outputs[0][0];
    if (output) {
      output.fill(0);
    }
    if (input && input.length > 0) {
      const targetRate = ${VOICE_REALTIME_SAMPLE_RATE};
      const sourceRate = typeof sampleRate === "number" && sampleRate > 0 ? sampleRate : targetRate;
      const outputLength = Math.max(1, Math.round((input.length * targetRate) / sourceRate));
      const buffer = new ArrayBuffer(outputLength * 2);
      const view = new DataView(buffer);
      const scale = sourceRate / targetRate;
      for (let index = 0; index < outputLength; index += 1) {
        const position = index * scale;
        const left = Math.min(input.length - 1, Math.floor(position));
        const right = Math.min(input.length - 1, left + 1);
        const ratio = position - left;
        const normalized = input[left] + (input[right] - input[left]) * ratio;
        const sample = Math.max(-1, Math.min(1, normalized));
        const value = sample < 0 ? sample * 0x8000 : sample * 0x7fff;
        view.setInt16(index * 2, value, true);
      }
      this.port.postMessage(buffer, [buffer]);
    }
    return true;
  }
}
registerProcessor("bifrost-voice-pcm16", BifrostVoicePcm16Processor);
`,
                ],
                { type: "application/javascript" },
              ),
            );
            try {
              await voiceAudioContext.audioWorklet.addModule(workletUrl);
            } finally {
              URL.revokeObjectURL(workletUrl);
            }
            const worklet = new WorkletNode(voiceAudioContext, "bifrost-voice-pcm16", {
              numberOfInputs: 1,
              numberOfOutputs: 1,
              channelCount: 1,
            });
            worklet.port.onmessage = (message) => {
              if (!recordingActiveRef.current || ws.readyState !== WebSocket.OPEN) {
                return;
              }
              ws.send(message.data as ArrayBuffer);
            };
            voiceWorkletRef.current = worklet;
            source.connect(worklet);
            worklet.connect(voiceAudioContext.destination);
            appendEvent("recording: microphone Voice stream opened with AudioWorklet");
          } else {
            const processor = voiceAudioContext.createScriptProcessor(4096, 1, 1);
            voiceProcessorRef.current = processor;
            processor.onaudioprocess = (event) => {
              if (!recordingActiveRef.current || ws.readyState !== WebSocket.OPEN) {
                return;
              }
              const input = event.inputBuffer.getChannelData(0);
              const output = event.outputBuffer.getChannelData(0);
              output.fill(0);
              ws.send(encodePcm16Chunk(input, voiceAudioContext.sampleRate));
            };
            source.connect(processor);
            processor.connect(voiceAudioContext.destination);
            appendEvent("recording: microphone Voice stream opened");
          }
        } catch (error) {
          const text = error instanceof Error ? error.message : String(error);
          recordingActiveRef.current = false;
          stopVoicePcmStreaming();
          stopMicMeter();
          stream.getTracks().forEach((track) => track.stop());
          streamRef.current = null;
          setWorkState("error");
          setErrorText(`Microphone capture failed: ${text}`);
          appendEvent(`microphone error: ${text}`);
          ws.close();
        }
      };
      setWorkState("recording");
      setProgress(0);
      setErrorText("");
      setSelectedName("");
      appendEvent("recording: microphone capture started with Voice streaming");
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      stopVoicePcmStreaming();
      stopMicMeter();
      setWorkState("error");
      setErrorText(`Microphone capture failed: ${text}`);
      appendEvent(`microphone error: ${text}`);
    }
  }, [
    appendEvent,
    getCurrentVoiceParams,
    handleVoiceRealtimeEvent,
    resetTranscript,
    startMicMeter,
    stopMicMeter,
    stopVoicePcmStreaming,
  ]);

  const stopRecording = useCallback(() => {
    recordingActiveRef.current = false;
    stopVoicePcmStreaming();
    stopMicMeter();
    streamRef.current?.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify({ type: "finish" }));
    } else if (wsRef.current) {
      wsRef.current.close();
    }
    appendEvent("recording: microphone capture stopped");
  }, [appendEvent, stopMicMeter, stopVoicePcmStreaming]);

  const cancelWork = useCallback(() => {
    abortRef.current?.abort();
    recordingActiveRef.current = false;
    stopVoicePcmStreaming();
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
  }, [appendEvent, stopMicMeter, stopVoicePcmStreaming]);

  const createDirectoryTask = useCallback(async () => {
    try {
      const values = await taskForm.validateFields();
      const externalDevices = parseExternalDeviceBindings(values.external_devices);
      await createAsrTask({
        name: values.name,
        audio_dir: values.audio_dir,
        recursive: values.recursive,
        enabled: values.enabled,
        schedule: buildTaskSchedule(values),
        language: values.language,
        model: values.model,
        runtime_strategy: values.runtime_strategy,
        diarization: {
          enabled: Boolean(values.diarization_enabled),
          profile: values.diarization_profile || "sherpa-onnx-balanced",
          known_speaker_count: values.diarization_known_speaker_count,
          voiceprint_matching: Boolean(values.voiceprint_matching),
        },
        external_devices: externalDevices,
        import_policy: externalDevices.length
          ? {
              enabled: true,
              file_stable_secs: 10,
              min_free_bytes: 10 * 1024 * 1024 * 1024,
              max_file_bytes: 50 * 1024 * 1024 * 1024,
              auto_run_after_import: true,
              content_hash_dedupe_enabled: true,
              content_hash_algorithm: "blake3",
              delete_source_after_import: false,
            }
          : undefined,
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
  }, [refreshTasks, taskForm]);

  const updateDirectoryTask = useCallback(
    async (id: string) => {
      try {
        const values = await taskForm.validateFields();
        const externalDevices = parseExternalDeviceBindings(values.external_devices);
        const updated = await updateAsrTask(id, {
          name: values.name,
          audio_dir: values.audio_dir,
          recursive: values.recursive,
          enabled: values.enabled,
          schedule: buildTaskSchedule(values),
          language: values.language,
          model: values.model,
          runtime_strategy: values.runtime_strategy,
          diarization: {
            enabled: Boolean(values.diarization_enabled),
            profile: values.diarization_profile || "sherpa-onnx-balanced",
            known_speaker_count: values.diarization_known_speaker_count,
            voiceprint_matching: Boolean(values.voiceprint_matching),
          },
          external_devices: externalDevices,
          import_policy: externalDevices.length
            ? {
                enabled: true,
                file_stable_secs: 10,
                min_free_bytes: 10 * 1024 * 1024 * 1024,
                max_file_bytes: 50 * 1024 * 1024 * 1024,
                auto_run_after_import: true,
                content_hash_dedupe_enabled: true,
                content_hash_algorithm: "blake3",
                delete_source_after_import: false,
              }
            : {
                enabled: false,
                file_stable_secs: 10,
                min_free_bytes: 10 * 1024 * 1024 * 1024,
                max_file_bytes: 50 * 1024 * 1024 * 1024,
                auto_run_after_import: true,
                content_hash_dedupe_enabled: true,
                content_hash_algorithm: "blake3",
                delete_source_after_import: false,
              },
        });
        setTaskDetail((previous) => (previous?.id === id ? { ...previous, ...updated } : previous));
        await refreshTasks();
        message.success("ASR directory task updated");
        return true;
      } catch (error) {
        if (error && typeof error === "object" && "errorFields" in error) {
          return false;
        }
        message.error(error instanceof Error ? error.message : "Failed to update ASR task");
        return false;
      }
    },
    [refreshTasks, taskForm],
  );

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

  useEffect(() => {
    const taskIds = new Set(tasks.map((task) => task.id));
    const externalTaskIds = tasks
      .filter((task) => Boolean(task.external_devices?.length))
      .map((task) => task.id);
    if (tasks.length === 0) {
      setExternalImportProgressByTask({});
      return undefined;
    }

    let stopped = false;
    void Promise.all(
      externalTaskIds.map(async (taskId) => {
        try {
          const status = await getAsrExternalImportStatus(taskId);
          return { taskId, progress: status.current_run };
        } catch (error) {
          appendEvent(
            `external import status error: ${
              error instanceof Error ? error.message : String(error)
            }`,
          );
          return { taskId, progress: undefined };
        }
      }),
    ).then((results) => {
      if (stopped) {
        return;
      }
      setExternalImportProgressByTask((prev) => {
        const next: Record<string, AsrExternalImportRunProgress> = {};
        Object.entries(prev).forEach(([taskId, progress]) => {
          if (taskIds.has(taskId)) {
            next[taskId] = progress;
          }
        });
        results.forEach(({ taskId, progress }) => {
          if (progress) {
            next[taskId] = progress;
          } else {
            delete next[taskId];
          }
        });
        return next;
      });
    });

    return () => {
      stopped = true;
    };
  }, [appendEvent, tasks]);

  useEffect(() => {
    const importingTaskIds = Object.entries(externalImportProgressByTask)
      .filter(([, progress]) => progress.status === "importing")
      .map(([taskId]) => taskId);
    if (importingTaskIds.length === 0) {
      return undefined;
    }
    const timer = window.setInterval(() => {
      importingTaskIds.forEach((taskId) => {
        void refreshExternalImportStatus(taskId).then((progress) => {
          if (progress && progress.status !== "importing") {
            void refreshTasks();
            if (taskDetail?.id === taskId) {
              void loadTaskDetail(taskId);
            }
          }
        });
      });
    }, 1500);
    return () => window.clearInterval(timer);
  }, [
    externalImportProgressByTask,
    loadTaskDetail,
    refreshExternalImportStatus,
    refreshTasks,
    taskDetail?.id,
  ]);

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
    if (!asrSupported || !selectedTaskId) {
      setTaskDetail(null);
      setTaskTimeline(null);
      setTaskDailyDocument(null);
      setTaskDailyAgentReport(null);
      return;
    }
    void loadTaskDetail(selectedTaskId);
  }, [asrSupported, loadTaskDetail, selectedTaskId]);

  useEffect(() => {
    if (!asrSupported || !selectedTaskId || !selectedFileKey) {
      setTaskTimeline(null);
      return;
    }
    void loadTaskTimeline(selectedTaskId, selectedFileKey);
  }, [asrSupported, loadTaskTimeline, selectedFileKey, selectedTaskId]);

  useEffect(() => {
    if (!asrSupported || !selectedTaskId || !selectedDailyDate) {
      setTaskDailyDocument(null);
      return;
    }
    void loadTaskDailyDocument(selectedTaskId, selectedDailyDate);
  }, [asrSupported, loadTaskDailyDocument, selectedDailyDate, selectedTaskId]);

  useEffect(() => {
    if (!asrSupported || !selectedTaskId || !selectedDailyAgentReportDate) {
      setTaskDailyAgentReport(null);
      return;
    }
    void loadDailyAgentReport(selectedTaskId, selectedDailyAgentReportDate);
  }, [asrSupported, loadDailyAgentReport, selectedDailyAgentReportDate, selectedTaskId]);

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
        const result = await runAsrTask(id);
        message.info("ASR task started");
        applyTaskControlUpdate(result.task);
      } catch (error) {
        message.error(error instanceof Error ? error.message : "Failed to run ASR task");
      }
    },
    [applyTaskControlUpdate],
  );

  const pauseDirectoryTask = useCallback(
    async (id: string, force = false, mode: AsrPauseMode = "long_term") => {
      try {
        const result = await pauseAsrTask(id, { force, mode });
        message.info(result.message);
        applyTaskControlUpdate(result.task);
      } catch (error) {
        message.error(error instanceof Error ? error.message : "Failed to pause ASR task");
      }
    },
    [applyTaskControlUpdate],
  );

  const resumeDirectoryTask = useCallback(
    async (id: string) => {
      try {
        const result = await resumeAsrTask(id);
        message.success(result.message);
        applyTaskControlUpdate(result.task);
      } catch (error) {
        message.error(error instanceof Error ? error.message : "Failed to resume ASR task");
      }
    },
    [applyTaskControlUpdate],
  );

  const runExternalImport = useCallback(
    async (id: string) => {
      try {
        const result = await runAsrExternalImport(id);
        if (result.progress) {
          setExternalImportProgressByTask((prev) => ({
            ...prev,
            [id]: result.progress!,
          }));
        }
        message.info(result.message);
        if (taskDetail?.id === id) {
          void loadTaskDetail(id);
        }
        await refreshTasks();
        void refreshExternalImportStatus(id);
      } catch (error) {
        message.error(
          error instanceof Error ? error.message : "Failed to import external device data",
        );
      }
    },
    [loadTaskDetail, refreshExternalImportStatus, refreshTasks, taskDetail?.id],
  );

  const removeDirectoryTask = useCallback(
    async (id: string, confirmName: string) => {
      try {
        await deleteAsrTask(id, confirmName);
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

  if (capabilities === null || !asrSupported) {
    return <div style={{ height: "100%" }} />;
  }

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
        onPauseTask={(id, force, mode) => void pauseDirectoryTask(id, force, mode)}
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
      <DiarizationSetupCard />
      <VoiceWakeActionsCard />
      <DirectoryTasksPanel
        taskForm={taskForm}
        taskScheduleKind={taskScheduleKind}
        tasks={tasks}
        tasksLoading={tasksLoading}
        externalImportProgressByTask={externalImportProgressByTask}
        onCreateTask={createDirectoryTask}
        onUpdateTask={updateDirectoryTask}
        onRunExternalImport={runExternalImport}
        onOpenTask={openTaskDetail}
        onRunTask={(id) => void runDirectoryTask(id)}
        onPauseTask={(id, force, mode) => void pauseDirectoryTask(id, force, mode)}
        onResumeTask={(id) => void resumeDirectoryTask(id)}
        onRemoveTask={(id, confirmName) => void removeDirectoryTask(id, confirmName)}
      />
      <SpeechWorkbench
        token={token}
        ready={ready}
        status={status}
        params={workbenchParams}
        onParamsChange={updateWorkbenchParams}
        serviceBusy={serviceBusy}
        workState={workState}
        progress={progress}
        selectedName={selectedName}
        transcript={transcript}
        offlineJob={offlineJob}
        offlineArtifacts={offlineArtifacts}
        speechStatus={speechStatus}
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
        onStartService={() => void startWorkbenchService()}
        onStopService={() => void stopWorkbenchService()}
        onCancel={cancelWork}
      />
    </div>
  );
}

function parseExternalDeviceBindings(value: unknown): AsrExternalDeviceBinding[] {
  return String(value || "")
    .split(/[,\n]/)
    .map((name) => name.trim())
    .filter(Boolean)
    .filter((name, index, names) => names.indexOf(name) === index)
    .map((name): AsrExternalDeviceBinding => ({ name, enabled: true }));
}
