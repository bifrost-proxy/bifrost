import { type KeyboardEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Col,
  Form,
  Input,
  Progress,
  Row,
  Select,
  Space,
  Switch,
  Table,
  Tag,
  Tooltip,
  Typography,
  message,
  theme,
} from "antd";
import {
  AudioOutlined,
  CheckCircleOutlined,
  KeyOutlined,
  PlayCircleOutlined,
  ReloadOutlined,
  SaveOutlined,
  StopOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import type {
  AsrSpeakerProfileSummary,
  VoiceWakeBinding,
  VoiceWakeEvent,
  VoiceWakeListenerStatus,
  VoiceWakeProfile,
  VoiceWakeStatus,
} from "../../../api/asr";
import {
  createVoiceWakeBinding,
  createVoiceWakeProfile,
  getVoiceWakeProfiles,
  getVoiceWakeBindings,
  getVoiceWakeEvents,
  getVoiceWakeStatus,
  listAsrSpeakerProfiles,
  startVoiceWakeListener,
  stopVoiceWakeListener,
} from "../../../api/asr";

const { Text } = Typography;

function formatShortcut(key: string, modifiers: string[]): string {
  const prefix = modifiers.length ? `${modifiers.join("+")}+` : "";
  return `${prefix}${key}`;
}

function formatWakeAction(key: string, modifiers: string[], pressCount: number): string {
  const suffix = pressCount > 1 ? ` x${pressCount}` : "";
  return `${formatShortcut(key, modifiers)}${suffix}`;
}

function isModifierKey(key: string): boolean {
  return ["cmd", "ctrl", "option", "shift"].includes(key);
}

function sanitizeModifiersForKey(key: string, modifiers: string[]): string[] {
  if (!isModifierKey(key)) {
    return modifiers;
  }
  return modifiers.filter((modifier) => modifier !== key);
}

function normalizeCapturedKey(event: KeyboardEvent<HTMLInputElement>): string | null {
  if (event.key === " " || event.key === "Spacebar") {
    return "space";
  }
  if (event.key === "Enter") {
    return "return";
  }
  if (event.key === "Escape") {
    return "escape";
  }
  if (event.key === "Tab") {
    return "tab";
  }
  if (event.key.startsWith("Arrow")) {
    return event.key.slice("Arrow".length).toLowerCase();
  }
  if (["Meta", "Control", "Shift", "Alt"].includes(event.key)) {
    return null;
  }
  if (event.key.length === 1) {
    return event.key.toLowerCase();
  }
  return null;
}

function capturedModifierKey(event: KeyboardEvent<HTMLInputElement>): string | null {
  if (event.key === "Meta") {
    return "cmd";
  }
  if (event.key === "Control") {
    return "ctrl";
  }
  if (event.key === "Shift") {
    return "shift";
  }
  if (event.key === "Alt") {
    return "option";
  }
  return null;
}

function capturedModifiers(event: KeyboardEvent<HTMLInputElement>): string[] {
  return [
    event.metaKey ? "cmd" : null,
    event.shiftKey ? "shift" : null,
    event.ctrlKey ? "ctrl" : null,
    event.altKey ? "option" : null,
  ].filter((value): value is string => Boolean(value));
}

function bindingShortcut(binding: VoiceWakeBinding): string {
  const modifiers = binding.action.modifiers.length ? `${binding.action.modifiers.join("+")}+` : "";
  const actionPressCount = binding.action.press_count ?? 1;
  const suffix = actionPressCount > 1 ? ` x${actionPressCount}` : "";
  if (binding.action.keycode !== null) {
    return `${modifiers}keycode:${binding.action.keycode}${suffix}`;
  }
  return `${modifiers}${binding.action.key ?? "-"}${suffix}`;
}

function latestBinding(bindings: VoiceWakeBinding[]): VoiceWakeBinding | null {
  return bindings
    .slice()
    .sort((left, right) => right.updated_at_ms - left.updated_at_ms)[0] ?? null;
}

function voiceWakeErrorMessage(error: unknown, fallback: string): string {
  const messageText = error instanceof Error ? error.message : "";
  if (messageText === "Failed to fetch") {
    return "Bifrost backend is not reachable. Restart Bifrost and record wake audio again.";
  }
  return messageText || fallback;
}

function wakeMatchStatusLabel(status?: string | null): string {
  switch (status) {
    case "worker_started":
      return "Worker started";
    case "capturing":
      return "Capturing mic";
    case "captured":
      return "Audio captured";
    case "transcribing":
      return "Checking";
    case "empty_transcript":
      return "No speech";
    case "capture_error":
      return "Capture error";
    case "asr_error":
      return "Wake engine error";
    case "kws_error":
      return "KWS error";
    case "recognized":
      return "Recognized";
    case "no_match":
      return "No match";
    case "cooldown":
      return "Cooldown";
    case "phrase_matched":
      return "Phrase matched";
    case "speaker_identifying":
      return "Checking voice";
    case "speaker_allowed":
      return "Voice matched";
    case "speaker_error":
      return "Voice error";
    case "trigger_error":
      return "Action error";
    case "speaker_rejected":
      return "Voice rejected";
    case "phrase_only_dry_run":
      return "Phrase only";
    case "dry_run_matched":
      return "Dry-run matched";
    case "executed":
      return "Executed";
    case "matched":
      return "Matched";
    default:
      return "Idle";
  }
}

function wakeMatchStatusColor(status?: string | null): string {
  if (status === "executed") return "success";
  if (
    status === "dry_run_matched" ||
    status === "phrase_matched" ||
    status === "matched" ||
    status === "capturing" ||
    status === "captured" ||
    status === "transcribing" ||
    status === "speaker_identifying" ||
    status === "speaker_allowed"
  ) return "processing";
  if (
    status === "speaker_rejected" ||
    status === "no_match" ||
    status === "empty_transcript"
  ) return "warning";
  if (status === "capture_error" || status === "asr_error" || status === "kws_error" || status === "trigger_error" || status === "speaker_error") return "error";
  if (status === "cooldown") return "default";
  return "default";
}

function wakeListenerStatusMessage(listener?: VoiceWakeListenerStatus | null, lastResult?: string, liveTranscript?: string): string {
  if (listener?.model_download_status === "downloading") {
    const progress = listener.model_download_progress ?? 0;
    const total = listener.model_download_total;
    const progressMB = (progress / 1024 / 1024).toFixed(1);
    if (total && total > 0) {
      const totalMB = (total / 1024 / 1024).toFixed(1);
      const percent = Math.round((progress / total) * 100);
      return `Downloading wake word model: ${progressMB}MB / ${totalMB}MB (${percent}%)`;
    }
    return `Downloading wake word model: ${progressMB}MB...`;
  }
  if (listener?.model_download_status === "extracting") {
    return "Extracting wake word model...";
  }
  if (listener?.model_download_status === "downloaded") {
    return "Model ready, starting listener...";
  }
  if (listener?.model_download_status === "failed") {
    return `Model download failed: ${listener.last_error ?? "unknown error"}`;
  }
  if (liveTranscript) return `Heard: ${liveTranscript}`;
  if (lastResult) return lastResult;
  const device = listener?.device_label || listener?.device || "default microphone";
  switch (listener?.last_match_status) {
    case "worker_started":
      return `Backend worker started on ${device}.`;
    case "capturing":
      return `Continuously listening from ${device} with a sliding wake window.`;
    case "captured":
      return `Checking the latest sliding wake window from ${device}.`;
    case "transcribing":
      return "Checking the latest wake window.";
    case "empty_transcript":
      return "No wake keyword candidate was detected in the latest window.";
    case "no_match":
      return "The wake engine did not match any saved command.";
    case "phrase_matched":
      return "Wake phrase matched; checking speaker/action policy.";
    case "speaker_identifying":
      return "Checking the wake-window voice against the selected voiceprint.";
    case "speaker_allowed":
      return "Voice verification passed; action is being triggered.";
    case "speaker_rejected":
      return "Wake phrase matched, but the speaker did not pass voice verification.";
    case "capture_error":
      return "Bifrost could not capture microphone audio. Check the selected input device and macOS microphone permission.";
    case "asr_error":
      return "The wake worker captured audio but the wake engine failed.";
    case "trigger_error":
      return "The wake phrase matched, but action execution failed.";
    default:
      return "Backend listener reads the microphone from the Bifrost process after start.";
  }
}

export default function VoiceWakeActionsCard() {
  const { token } = theme.useToken();
  const [status, setStatus] = useState<VoiceWakeStatus | null>(null);
  const [wakeProfiles, setWakeProfiles] = useState<VoiceWakeProfile[]>([]);
  const [speakerProfiles, setSpeakerProfiles] = useState<AsrSpeakerProfileSummary[]>([]);
  const [bindings, setBindings] = useState<VoiceWakeBinding[]>([]);
  const [events, setEvents] = useState<VoiceWakeEvent[]>([]);
  const [wakePhrase, setWakePhrase] = useState("");
  const [selectedVoiceprintProfileId, setSelectedVoiceprintProfileId] = useState<string | null>(null);
  const [key, setKey] = useState("space");
  const [modifiers, setModifiers] = useState<string[]>(["cmd"]);
  const [pressCount, setPressCount] = useState(1);
  const [activeBindingId, setActiveBindingId] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [recordingSample, setRecordingSample] = useState(false);
  const [sampleUrl, setSampleUrl] = useState<string | null>(null);
  const [sampleDurationMs, setSampleDurationMs] = useState<number | null>(null);
  const [listening, setListening] = useState(false);
  const [liveTranscript, setLiveTranscript] = useState("");
  const [lastResult, setLastResult] = useState("");

  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  const sampleStreamRef = useRef<MediaStream | null>(null);
  const sampleChunksRef = useRef<Blob[]>([]);
  const sampleStartedAtRef = useRef(0);

  const activeBinding = useMemo(
    () =>
      bindings.find((binding) => binding.id === activeBindingId) ??
      latestBinding(bindings),
    [activeBindingId, bindings],
  );
  const activeWakeProfile = useMemo(
    () => wakeProfiles.find((profile) => profile.id === activeBinding?.profile_id) ?? null,
    [activeBinding, wakeProfiles],
  );
  const selectedSpeakerProfile = useMemo(
    () =>
      speakerProfiles.find((profile) => profile.id === selectedVoiceprintProfileId) ??
      null,
    [selectedVoiceprintProfileId, speakerProfiles],
  );
  const currentConfigSaved = useMemo(
    () =>
      Boolean(
        activeBinding &&
          activeBinding.phrase === wakePhrase.trim() &&
          activeWakeProfile?.voiceprint_profile_id === selectedVoiceprintProfileId &&
          (activeBinding.action.key ?? "space") === key &&
          activeBinding.action.modifiers.join(",") === modifiers.join(",") &&
          (activeBinding.action.press_count ?? 1) === pressCount,
      ),
    [activeBinding, activeWakeProfile, key, modifiers, pressCount, selectedVoiceprintProfileId, wakePhrase],
  );
  const shortcutValue = useMemo(() => formatWakeAction(key, modifiers, pressCount), [key, modifiers, pressCount]);

  const captureShortcut = useCallback((event: KeyboardEvent<HTMLInputElement>) => {
    event.preventDefault();
    event.stopPropagation();
    const modifier = capturedModifierKey(event);
    if (modifier) {
      setKey(modifier);
      setModifiers((current) => sanitizeModifiersForKey(modifier, current));
      return;
    }
    const nextKey = normalizeCapturedKey(event);
    if (!nextKey) {
      message.warning("Press a letter, space, return, tab, escape, or arrow key.");
      return;
    }
    setKey(nextKey);
    const captured = capturedModifiers(event);
    if (captured.length) {
      setModifiers(sanitizeModifiersForKey(nextKey, captured));
    }
  }, []);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [nextStatus, nextWakeProfiles, nextBindings, nextEvents, nextSpeakerProfiles] = await Promise.all([
        getVoiceWakeStatus(),
        getVoiceWakeProfiles(),
        getVoiceWakeBindings(),
        getVoiceWakeEvents(),
        listAsrSpeakerProfiles(),
      ]);
      const newestBinding = latestBinding(nextBindings.bindings);
      setStatus(nextStatus);
      setListening(nextStatus.listener.running);
      setLiveTranscript(nextStatus.listener.last_transcript ?? "");
      setLastResult(nextStatus.listener.last_error ?? "");
      setWakeProfiles(nextWakeProfiles.profiles);
      setSpeakerProfiles(nextSpeakerProfiles);
      setBindings(nextBindings.bindings);
      setEvents(nextEvents.events.slice().reverse());
      if (newestBinding && !activeBindingId) {
        const wakeProfile = nextWakeProfiles.profiles.find(
          (profile) => profile.id === newestBinding.profile_id,
        );
        setActiveBindingId(newestBinding.id);
        setWakePhrase(newestBinding.phrase);
        setSelectedVoiceprintProfileId(
          wakeProfile?.voiceprint_profile_id ?? nextSpeakerProfiles[0]?.id ?? null,
        );
        setKey(newestBinding.action.key ?? "space");
        setModifiers(newestBinding.action.modifiers);
        setPressCount(newestBinding.action.press_count ?? 1);
      } else if (!selectedVoiceprintProfileId && nextSpeakerProfiles[0]) {
        setSelectedVoiceprintProfileId(nextSpeakerProfiles[0].id);
      }
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to load voice wake actions");
    } finally {
      setLoading(false);
    }
  }, [activeBindingId, selectedVoiceprintProfileId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!listening) {
      return undefined;
    }
    const timer = window.setInterval(() => {
      void refresh();
    }, 500);
    return () => window.clearInterval(timer);
  }, [listening, refresh]);

  useEffect(
    () => () => {
      if (mediaRecorderRef.current?.state !== "inactive") {
        mediaRecorderRef.current?.stop();
      }
      sampleStreamRef.current?.getTracks().forEach((track) => track.stop());
      if (sampleUrl) {
        URL.revokeObjectURL(sampleUrl);
      }
    },
    [sampleUrl],
  );

  const stopSampleRecording = useCallback(() => {
    const recorder = mediaRecorderRef.current;
    if (recorder && recorder.state !== "inactive") {
      recorder.stop();
    }
    sampleStreamRef.current?.getTracks().forEach((track) => track.stop());
    sampleStreamRef.current = null;
    setRecordingSample(false);
  }, []);

  const startSampleRecording = useCallback(async () => {
    if (!navigator.mediaDevices?.getUserMedia) {
      message.error("Current browser cannot access microphone recording.");
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      sampleStreamRef.current = stream;
      sampleChunksRef.current = [];
      sampleStartedAtRef.current = Date.now();
      const recorder = new MediaRecorder(stream);
      mediaRecorderRef.current = recorder;
      recorder.ondataavailable = (event) => {
        if (event.data.size > 0) {
          sampleChunksRef.current.push(event.data);
        }
      };
      recorder.onstop = () => {
        if (sampleUrl) {
          URL.revokeObjectURL(sampleUrl);
        }
        const blob = new Blob(sampleChunksRef.current, {
          type: recorder.mimeType || "audio/webm",
        });
        setSampleUrl(URL.createObjectURL(blob));
        setSampleDurationMs(Date.now() - sampleStartedAtRef.current);
        setRecordingSample(false);
        setLastResult("Wake audio sample captured. Enter the wake phrase and save.");
        message.success("Wake audio sample captured.");
      };
      recorder.start();
      setRecordingSample(true);
    } catch (error) {
      sampleStreamRef.current?.getTracks().forEach((track) => track.stop());
      sampleStreamRef.current = null;
      setRecordingSample(false);
      message.error(voiceWakeErrorMessage(error, "Failed to record wake audio"));
    }
  }, [sampleUrl]);

  const saveWakeAction = useCallback(async (): Promise<VoiceWakeBinding | null> => {
    const phrase = wakePhrase.trim();
    if (!phrase) {
      message.error("Wake phrase is required.");
      return null;
    }
    setSaving(true);
    try {
      const profile = await createVoiceWakeProfile({
        display_name: selectedSpeakerProfile?.display_name ?? "Phrase only wake",
        voiceprint_profile_id: selectedSpeakerProfile?.id,
      });
      const binding = await createVoiceWakeBinding({
        phrase,
        profile_id: profile.id,
        cooldown_ms: 1500,
        action: {
          type: "key_press",
          key,
          keycode: null,
          modifiers,
          press_count: pressCount,
          repeat_delay_ms: 100,
        },
      });
      setActiveBindingId(binding.id);
      message.success(
        `Saved ${selectedSpeakerProfile?.display_name ?? "phrase-only"}: ${phrase} -> ${formatWakeAction(key, modifiers, pressCount)}`,
      );
      await refresh();
      return binding;
    } catch (error) {
      message.error(voiceWakeErrorMessage(error, "Failed to save wake action"));
      return null;
    } finally {
      setSaving(false);
    }
  }, [key, modifiers, pressCount, refresh, selectedSpeakerProfile, wakePhrase]);

  const listenerBlockReason = useMemo(() => {
    if (!activeBinding) {
      return "Record wake audio, capture a shortcut, and save the voice command before enabling.";
    }
    return null;
  }, [activeBinding]);

  const ensureListenerCanStart = useCallback(
    (binding: VoiceWakeBinding | null): binding is VoiceWakeBinding => {
      if (!binding) {
        message.warning("Save a wake command before enabling voice commands.");
        return false;
      }
      return true;
    },
    [],
  );

  const stopListening = useCallback(async () => {
    try {
      const response = await stopVoiceWakeListener();
      setListening(response.listener.running);
      setLastResult("Backend listener stopped.");
      await refresh();
    } catch (error) {
      message.error(voiceWakeErrorMessage(error, "Failed to stop backend listener"));
    }
  }, [refresh]);

  const startListening = useCallback(async (providedBinding?: VoiceWakeBinding) => {
    const binding = providedBinding ?? activeBinding;
    if (!ensureListenerCanStart(binding)) {
      return;
    }
    try {
      setLastResult("Starting voice wake listener...");
      const response = await startVoiceWakeListener();
      setListening(response.listener.running);
      setLastResult("");
      setLiveTranscript("");
      if (response.listener.running) {
        message.success("Backend listener started");
      }
      await refresh();
    } catch (error) {
      setListening(false);
      message.error(voiceWakeErrorMessage(error, "Failed to start backend listener"));
    }
  }, [activeBinding, ensureListenerCanStart, refresh]);

  const toggleListening = useCallback(
    (checked: boolean) => {
      if (checked) {
        void startListening(activeBinding ?? undefined);
      } else {
        void stopListening();
      }
    },
    [activeBinding, startListening, stopListening],
  );

  return (
    <Card
      size="small"
      data-testid="voice-wake-actions-card"
      title={
        <Space>
          <ThunderboltOutlined />
          <span>Voice Wake Actions</span>
          <Tag color={status?.enabled ? "success" : "default"}>
            {listening ? "Listening" : status?.enabled ? "Enabled" : "Disabled"}
          </Tag>
        </Space>
      }
      extra={
        <Button
          size="small"
          icon={<ReloadOutlined />}
          loading={loading}
          onClick={() => void refresh()}
          aria-label="Refresh voice wake actions"
        />
      }
      style={{ marginTop: 16 }}
    >
      <Space direction="vertical" size={16} style={{ width: "100%" }}>
        <Row gutter={[16, 16]} align="top">
          <Col xs={24} lg={14}>
            <Space direction="vertical" size={8} style={{ width: "100%" }}>
              <Space>
                <AudioOutlined />
                <Text strong>Wake Audio</Text>
              </Space>
              <div
                style={{
                  minHeight: 116,
                  padding: 12,
                  border: `1px dashed ${token.colorBorder}`,
                  borderRadius: 6,
                  background: token.colorFillQuaternary,
                }}
              >
                <Space direction="vertical" size={10} style={{ width: "100%" }}>
                  {recordingSample ? (
                    <Button
                      danger
                      icon={<StopOutlined />}
                      onClick={stopSampleRecording}
                      data-testid="voice-wake-stop-record"
                      block
                    >
                      Stop Recording
                    </Button>
                  ) : (
                    <Button
                      icon={<AudioOutlined />}
                      onClick={() => void startSampleRecording()}
                      data-testid="voice-wake-record-button"
                      block
                    >
                      Record Wake Audio
                    </Button>
                  )}
                  {sampleUrl ? (
                    <audio
                      src={sampleUrl}
                      controls
                      aria-label="Wake audio sample"
                      style={{ width: "100%" }}
                    />
                  ) : null}
                  {sampleDurationMs ? (
                    <Tag color="processing">{Math.max(1, Math.round(sampleDurationMs / 1000))}s</Tag>
                  ) : null}
                </Space>
              </div>
              <Form layout="vertical">
                <Form.Item label="Wake phrase" required style={{ marginBottom: 0 }}>
                  <Input
                    value={wakePhrase}
                    onChange={(event) => setWakePhrase(event.target.value)}
                    placeholder="Enter wake phrase"
                    data-testid="voice-wake-phrase-input"
                  />
                </Form.Item>
                <Form.Item label="Voiceprint" style={{ marginBottom: 0, marginTop: 8 }}>
                  <Select
                    allowClear
                    value={selectedVoiceprintProfileId ?? undefined}
                    placeholder="Optional speaker verification"
                    options={speakerProfiles.map((profile) => ({
                      label: `${profile.display_name} (${profile.embedding_dim}d)`,
                      value: profile.id,
                    }))}
                    onChange={setSelectedVoiceprintProfileId}
                    data-testid="voice-wake-voiceprint-select"
                  />
                </Form.Item>
              </Form>
              <Space>
                <CheckCircleOutlined style={{ color: currentConfigSaved ? token.colorSuccess : token.colorTextDisabled }} />
                <Text type={currentConfigSaved ? undefined : "secondary"}>
                  {currentConfigSaved && activeBinding && selectedSpeakerProfile
                    ? `${selectedSpeakerProfile.display_name}: ${activeBinding.phrase} -> ${bindingShortcut(activeBinding)}`
                    : "Not saved"}
                </Text>
              </Space>
            </Space>
          </Col>
          <Col xs={24} lg={10}>
            <Form layout="vertical">
              <Form.Item label="Global shortcut">
                <Space direction="vertical" size={8} style={{ width: "100%" }}>
                  <Input
                    value={shortcutValue}
                    readOnly
                    prefix={<KeyOutlined />}
                    onKeyDown={captureShortcut}
                    onFocus={(event) => event.currentTarget.select()}
                    placeholder="Press shortcut key"
                    data-testid="voice-wake-shortcut-input"
                  />
                  <Select
                    mode="multiple"
                    value={modifiers}
                    onChange={(values) => setModifiers(sanitizeModifiersForKey(key, values))}
                    placeholder="Optional modifiers"
                    data-testid="voice-wake-modifier-select"
                    options={[
                      { value: "cmd", label: "cmd" },
                      { value: "ctrl", label: "ctrl" },
                      { value: "option", label: "option" },
                      { value: "shift", label: "shift" },
                    ]}
                    style={{ width: "100%" }}
                  />
                  <Space>
                    <Switch
                      checked={pressCount === 2}
                      onChange={(checked) => setPressCount(checked ? 2 : 1)}
                      data-testid="voice-wake-double-press-switch"
                    />
                    <Text>Double press</Text>
                    {pressCount === 2 ? <Tag>100ms</Tag> : null}
                  </Space>
                </Space>
              </Form.Item>
              <Space wrap>
                <Tooltip title={!listening ? listenerBlockReason : null}>
                  <Switch
                    checked={listening}
                    disabled={!listening && Boolean(listenerBlockReason)}
                    onChange={toggleListening}
                    checkedChildren="On"
                    unCheckedChildren="Off"
                    data-testid="voice-wake-listener-switch"
                  />
                </Tooltip>
                <Text type={listenerBlockReason && !listening ? "secondary" : undefined}>
                  Voice command
                </Text>
                {!listening && listenerBlockReason ? (
                  <Alert
                    type="info"
                    showIcon
                    message={listenerBlockReason}
                    data-testid="voice-wake-listener-block-reason"
                    style={{ padding: "4px 8px" }}
                  />
                ) : null}
                <Button
                  type="primary"
                  icon={<SaveOutlined />}
                  loading={saving}
                  onClick={() => void saveWakeAction()}
                  data-testid="voice-wake-save-action"
                >
                  Save
                </Button>
                {listening ? (
                  <Button
                    danger
                    icon={<StopOutlined />}
                    onClick={() => void stopListening()}
                    data-testid="voice-wake-stop-listening"
                  >
                    Stop Listening
                  </Button>
                ) : (
                  <Button
                    icon={<PlayCircleOutlined />}
                    disabled={Boolean(listenerBlockReason)}
                    onClick={() => void startListening(activeBinding ?? undefined)}
                    data-testid="voice-wake-start-listening"
                  >
                    Start Listening
                  </Button>
                )}
              </Space>
            </Form>
          </Col>
        </Row>

        <div
          data-testid="voice-wake-listening-status"
          style={{
            minHeight: 72,
            padding: "10px 12px",
            border: `1px solid ${token.colorBorderSecondary}`,
            borderRadius: 6,
            background: listening ? token.colorSuccessBg : token.colorFillQuaternary,
          }}
        >
          <Space direction="vertical" size={4} style={{ width: "100%" }}>
            <Space>
              <KeyOutlined />
              <Text strong>{listening ? "Backend Listening" : "Idle"}</Text>
              {status?.listener.last_match_status ? (
                <Tag color={wakeMatchStatusColor(status.listener.last_match_status)}>
                  {wakeMatchStatusLabel(status.listener.last_match_status)}
                </Tag>
              ) : null}
              {status?.listener.trigger_count ? (
                <Tag color="success">{status.listener.trigger_count} triggered</Tag>
              ) : null}
              {status?.listener.last_speaker_profile_id ? (
                <Tag color="blue">
                  {Math.round((status.listener.last_speaker_confidence ?? 0) * 100)}% voice
                </Tag>
              ) : null}
            </Space>
            <Text type={lastResult ? "danger" : "secondary"}>
              {wakeListenerStatusMessage(status?.listener, lastResult, liveTranscript)}
            </Text>
            {status?.listener.model_download_status === "downloading" && (
              <Progress
                percent={
                  status.listener.model_download_total && status.listener.model_download_total > 0
                    ? Math.round(((status.listener.model_download_progress ?? 0) / status.listener.model_download_total) * 100)
                    : 0
                }
                size="small"
                status="active"
                style={{ maxWidth: 320 }}
              />
            )}
            {status?.listener.model_download_status === "extracting" && (
              <Progress percent={100} size="small" status="active" style={{ maxWidth: 320 }} />
            )}
            {status?.listener.device_label || status?.listener.device ? (
              <Text type="secondary">
                Input: {status.listener.device_label || status.listener.device}
                {status.listener.worker_pid ? ` · PID ${status.listener.worker_pid}` : ""}
              </Text>
            ) : null}
            {status?.listener.last_match_phrase ? (
              <Text>
                Matched command: <Text strong>{status.listener.last_match_phrase}</Text>
              </Text>
            ) : null}
            {status?.listener.last_action_result ? (
              <Text type={status.listener.last_action_result.executed ? "success" : "secondary"}>
                Action: {status.listener.last_action_result.message}
              </Text>
            ) : null}
          </Space>
        </div>

        <Table<VoiceWakeEvent>
          data-testid="voice-wake-event-table"
          size="small"
          rowKey="id"
          dataSource={events.slice(0, 5)}
          pagination={false}
          columns={[
            { title: "Phrase", dataIndex: "phrase" },
            {
              title: "Shortcut",
              render: (_, event) => {
                const binding = bindings.find((item) => item.id === event.binding_id);
                return binding ? bindingShortcut(binding) : event.binding_id;
              },
            },
            {
              title: "Result",
              render: (_, event) => (
                <Tag color={event.action_result.executed ? "success" : "default"}>
                  {event.action_result.executed ? "Executed" : "Matched"}
                </Tag>
              ),
            },
            {
              title: "Message",
              render: (_, event) => event.action_result.message,
            },
          ]}
        />
      </Space>
    </Card>
  );
}
