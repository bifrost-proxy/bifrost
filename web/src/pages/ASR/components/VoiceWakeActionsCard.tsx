import { type KeyboardEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Col,
  Form,
  Input,
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
  loadAsrParams,
  startAsrService,
  startVoiceWakeListener,
  stopVoiceWakeListener,
  streamAsrTranscription,
  type AsrStreamEvent,
} from "../../../api/asr";

const { Text } = Typography;

function formatShortcut(key: string, modifiers: string[]): string {
  const prefix = modifiers.length ? `${modifiers.join("+")}+` : "";
  return `${prefix}${key}`;
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
  if (binding.action.keycode !== null) {
    return `${modifiers}keycode:${binding.action.keycode}`;
  }
  return `${modifiers}${binding.action.key ?? "-"}`;
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
  const [activeBindingId, setActiveBindingId] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [recordingSample, setRecordingSample] = useState(false);
  const [recognizingSample, setRecognizingSample] = useState(false);
  const [sampleUrl, setSampleUrl] = useState<string | null>(null);
  const [sampleDurationMs, setSampleDurationMs] = useState<number | null>(null);
  const [listening, setListening] = useState(false);
  const [liveTranscript, setLiveTranscript] = useState("");
  const [lastResult, setLastResult] = useState("");

  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  const sampleStreamRef = useRef<MediaStream | null>(null);
  const sampleChunksRef = useRef<Blob[]>([]);
  const sampleStartedAtRef = useRef(0);
  const sampleSequenceRef = useRef(0);

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
          activeBinding.action.modifiers.join(",") === modifiers.join(","),
      ),
    [activeBinding, activeWakeProfile, key, modifiers, selectedVoiceprintProfileId, wakePhrase],
  );
  const shortcutValue = useMemo(() => formatShortcut(key, modifiers), [key, modifiers]);

  const captureShortcut = useCallback((event: KeyboardEvent<HTMLInputElement>) => {
    event.preventDefault();
    event.stopPropagation();
    const nextKey = normalizeCapturedKey(event);
    if (!nextKey) {
      message.warning("Press a letter, space, return, tab, escape, or arrow key.");
      return;
    }
    setKey(nextKey);
    setModifiers(capturedModifiers(event));
  }, []);

  const ensureBackendAsrService = useCallback(async () => {
    const result = await startAsrService({
      ...loadAsrParams(),
      ownerModule: "voice_wake_listener",
    });
    if (!result.ready) {
      throw new Error(result.detail || result.message || "Backend ASR service is not ready");
    }
  }, []);

  const recognizeWakePhrase = useCallback(async (blob: Blob, sequence: number) => {
    setRecognizingSample(true);
    setLastResult("Recognizing wake audio in backend...");
    let recognized = "";
    const handleEvent = (event: AsrStreamEvent) => {
      if (event.type === "text") {
        recognized = event.text;
      } else if (event.type === "final") {
        recognized = event.committed || event.text || event.delta || recognized;
      } else if (event.type === "partial") {
        recognized = event.text || recognized;
      } else if (event.type === "error") {
        throw new Error(event.detail ? `${event.message}\n${event.detail}` : event.message);
      }
    };
    try {
      await ensureBackendAsrService();
      await streamAsrTranscription(
        blob,
        `wake-sample-${sequence}.webm`,
        {
          ...loadAsrParams(),
          ownerModule: "voice_wake_sample",
        },
        handleEvent,
      );
      const phrase = recognized.trim();
      if (!phrase) {
        setLastResult("Backend ASR did not recognize text from this wake audio.");
        message.error("Backend ASR did not recognize text from this wake audio.");
        return;
      }
      setWakePhrase(phrase);
      setLastResult(`Wake phrase recognized: ${phrase}`);
      message.success(`Wake phrase recognized: ${phrase}`);
    } catch (error) {
      const text = voiceWakeErrorMessage(error, "Failed to recognize wake audio");
      setLastResult(text);
      message.error(text);
    } finally {
      setRecognizingSample(false);
    }
  }, [ensureBackendAsrService]);

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
    }, 1500);
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
        sampleSequenceRef.current += 1;
        void recognizeWakePhrase(blob, sampleSequenceRef.current);
      };
      recorder.start();
      setRecordingSample(true);
    } catch (error) {
      sampleStreamRef.current?.getTracks().forEach((track) => track.stop());
      sampleStreamRef.current = null;
      setRecordingSample(false);
      message.error(voiceWakeErrorMessage(error, "Failed to record wake audio"));
    }
  }, [recognizeWakePhrase, sampleUrl]);

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
          press_count: 1,
        },
      });
      setActiveBindingId(binding.id);
      message.success(
        `Saved ${selectedSpeakerProfile?.display_name ?? "phrase-only"}: ${phrase} -> ${formatShortcut(key, modifiers)}`,
      );
      await refresh();
      return binding;
    } catch (error) {
      message.error(voiceWakeErrorMessage(error, "Failed to save wake action"));
      return null;
    } finally {
      setSaving(false);
    }
  }, [key, modifiers, refresh, selectedSpeakerProfile, wakePhrase]);

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
                    readOnly
                    placeholder="Record wake audio to recognize"
                    suffix={recognizingSample ? <Tag color="processing">Recognizing</Tag> : null}
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
                <Input
                  value={shortcutValue}
                  readOnly
                  prefix={<KeyOutlined />}
                  onKeyDown={captureShortcut}
                  onFocus={(event) => event.currentTarget.select()}
                  placeholder="Press shortcut"
                  data-testid="voice-wake-shortcut-input"
                />
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
              {liveTranscript || lastResult || "Backend listener runs in Bifrost after start."}
            </Text>
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
