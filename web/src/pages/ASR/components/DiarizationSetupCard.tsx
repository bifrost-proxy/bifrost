import { useEffect, useState } from "react";
import { Button, Card, Descriptions, Input, List, Modal, Popconfirm, Progress, Space, Tag, Typography, message } from "antd";
import { AudioOutlined, DeleteOutlined, ReloadOutlined, SafetyCertificateOutlined, ToolOutlined, UserAddOutlined } from "@ant-design/icons";
import type {
  AsrDiarizationStatus,
  AsrSpeakerIdentifyResult,
  AsrSpeakerEnrollmentPrompt,
  AsrSpeakerProfileSummary,
} from "../../../api/asr";
import {
  appendAsrSpeakerEnrollmentAudio,
  createAsrSpeakerEnrollmentSession,
  deleteAsrSpeakerProfile,
  finishAsrSpeakerEnrollment,
  getAsrDiarizationStatus,
  identifyAsrSpeakerVoice,
  initAsrDiarizationProfile,
  listAsrSpeakerProfiles,
  verifyAsrSpeakerEnrollmentPrompt,
} from "../../../api/asr";

const { Text } = Typography;
const DEFAULT_PROFILE = "sherpa-onnx-balanced";
const MIN_PROMPT_RECORDING_MS = 1200;
const MAX_PROMPT_RECORDING_MS = 30000;
const PROMPT_VERIFY_INTERVAL_MS = 2200;
const PROMPT_MATCH_THRESHOLD = 0.72;
const IDENTITY_VERIFY_MIN_RECORDING_MS = 1200;
const IDENTITY_VERIFY_MAX_RECORDING_MS = 12000;
const IDENTITY_VERIFY_INTERVAL_MS = 1400;

export default function DiarizationSetupCard() {
  const [status, setStatus] = useState<AsrDiarizationStatus | null>(null);
  const [speakerProfiles, setSpeakerProfiles] = useState<AsrSpeakerProfileSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [initializing, setInitializing] = useState(false);
  const [enrollOpen, setEnrollOpen] = useState(false);
  const [speakerName, setSpeakerName] = useState("");
  const [enrolling, setEnrolling] = useState(false);
  const [activePrompt, setActivePrompt] = useState<AsrSpeakerEnrollmentPrompt | null>(null);
  const [activePromptIndex, setActivePromptIndex] = useState(0);
  const [activePromptTotal, setActivePromptTotal] = useState(0);
  const [activeTranscript, setActiveTranscript] = useState("");
  const [activeMatchScore, setActiveMatchScore] = useState(0);
  const [identifying, setIdentifying] = useState(false);
  const [identityResult, setIdentityResult] = useState<AsrSpeakerIdentifyResult | null>(null);

  const refresh = async () => {
    setLoading(true);
    try {
      const [nextStatus, profiles] = await Promise.all([
        getAsrDiarizationStatus(DEFAULT_PROFILE),
        listAsrSpeakerProfiles(),
      ]);
      setStatus(nextStatus);
      setSpeakerProfiles(profiles);
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to load diarization status");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const initProfile = async () => {
    setInitializing(true);
    try {
      const next = await initAsrDiarizationProfile(DEFAULT_PROFILE);
      setStatus(next);
      message.success("Speaker diarization profile initialized");
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to initialize diarization");
    } finally {
      setInitializing(false);
    }
  };

  const enrollSpeaker = async () => {
    const name = speakerName.trim();
    if (!name) {
      message.warning("Speaker name is required");
      return;
    }
    setEnrolling(true);
    try {
      const session = await createAsrSpeakerEnrollmentSession(name, DEFAULT_PROFILE);
      setActivePromptTotal(session.prompts.length);
      for (const [index, prompt] of session.prompts.entries()) {
        setActivePrompt(prompt);
        setActivePromptIndex(index + 1);
        setActiveTranscript("");
        setActiveMatchScore(0);
        const audio = await recordPromptPcm16(prompt, {
          sessionId: session.id,
          minDurationMs: MIN_PROMPT_RECORDING_MS,
          maxDurationMs: MAX_PROMPT_RECORDING_MS,
          verifyIntervalMs: PROMPT_VERIFY_INTERVAL_MS,
          matchThreshold: PROMPT_MATCH_THRESHOLD,
          onRecognitionUpdate: ({ transcript, score }) => {
            setActiveTranscript(transcript);
            setActiveMatchScore(score);
          },
        });
        await appendAsrSpeakerEnrollmentAudio(session.id, prompt.id, audio, true);
      }
      const result = await finishAsrSpeakerEnrollment(session.id);
      message.success(`Voiceprint enrolled for ${result.profile.display_name}`);
      setEnrollOpen(false);
      setSpeakerName("");
      setActivePrompt(null);
      setActivePromptIndex(0);
      setActivePromptTotal(0);
      setActiveTranscript("");
      setActiveMatchScore(0);
      await refresh();
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to enroll speaker voiceprint");
    } finally {
      setEnrolling(false);
    }
  };

  const deleteProfile = async (profileId: string) => {
    try {
      await deleteAsrSpeakerProfile(profileId);
      message.success("Voiceprint deleted");
      if (identityResult?.profile_id === profileId) {
        setIdentityResult(null);
      }
      await refresh();
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to delete voiceprint");
    }
  };

  const verifyCurrentSpeaker = async () => {
    setIdentifying(true);
    setIdentityResult(null);
    try {
      const result = await recordIdentityPcm16({
        minDurationMs: IDENTITY_VERIFY_MIN_RECORDING_MS,
        maxDurationMs: IDENTITY_VERIFY_MAX_RECORDING_MS,
        verifyIntervalMs: IDENTITY_VERIFY_INTERVAL_MS,
        onIdentifyUpdate: setIdentityResult,
      });
      setIdentityResult(result);
      if (result.matched) {
        message.success(`Current speaker: ${result.display_name}`);
      } else if (result.status === "insufficient_audio") {
        message.warning("Need more speech to identify the current speaker");
      } else {
        message.warning(`Current speaker: ${result.display_name}`);
      }
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to identify current speaker");
    } finally {
      setIdentifying(false);
    }
  };

  return (
    <>
      <Card
        size="small"
        data-testid="asr-diarization-setup-card"
        title={
          <Space>
            <AudioOutlined />
            <span>Speaker Diarization</span>
            <Tag color={status?.profile.ready ? "success" : "warning"}>
              {status?.profile.ready ? "Ready" : "Not initialized"}
            </Tag>
          </Space>
        }
        extra={
          <Space>
            <Button size="small" icon={<ReloadOutlined />} loading={loading} onClick={() => void refresh()} />
            <Button
              size="small"
              icon={<UserAddOutlined />}
              data-testid="asr-enroll-voiceprint-button"
              disabled={!status?.profile.ready}
              onClick={() => setEnrollOpen(true)}
            >
              Enroll Voiceprint
            </Button>
            <Button
              size="small"
              icon={<SafetyCertificateOutlined />}
              data-testid="asr-identify-speaker-button"
              disabled={!status?.profile.ready || speakerProfiles.length === 0}
              loading={identifying}
              onClick={() => void verifyCurrentSpeaker()}
            >
              Verify Voice
            </Button>
            <Button
              size="small"
              type="primary"
              icon={<ToolOutlined />}
              loading={initializing}
              onClick={() => void initProfile()}
            >
              Initialize
            </Button>
          </Space>
        }
        style={{ marginTop: 16 }}
      >
        <Descriptions size="small" column={{ xs: 1, sm: 2, md: 4 }}>
          <Descriptions.Item label="Profile">
            {status?.profile.id ?? DEFAULT_PROFILE}
          </Descriptions.Item>
          <Descriptions.Item label="Engine">
            {status?.profile.engine ?? "-"}
          </Descriptions.Item>
          <Descriptions.Item label="Voiceprints">
            {status?.speaker_profile_count ?? speakerProfiles.length}
          </Descriptions.Item>
          <Descriptions.Item label="Install dir">
            <Text ellipsis={{ tooltip: status?.profile.install_dir }} style={{ maxWidth: 260 }}>
              {status?.profile.install_dir ?? "-"}
            </Text>
          </Descriptions.Item>
        </Descriptions>
        {speakerProfiles.length > 0 ? (
          <List
            size="small"
            dataSource={speakerProfiles}
            renderItem={(profile) => (
              <List.Item
                actions={[
                  <Popconfirm
                    key="delete"
                    title="Delete this voiceprint?"
                    okText="Delete"
                    okButtonProps={{ danger: true }}
                    onConfirm={() => void deleteProfile(profile.id)}
                  >
                    <Button
                      danger
                      size="small"
                      icon={<DeleteOutlined />}
                      aria-label={`Delete ${profile.display_name} voiceprint`}
                      data-testid={`asr-delete-speaker-${profile.id}`}
                    />
                  </Popconfirm>,
                ]}
              >
                <Space>
                  <Tag color="blue">{profile.display_name}</Tag>
                  <Text type="secondary">{profile.id}</Text>
                  <Text type="secondary">{profile.embedding_dim}d</Text>
                </Space>
              </List.Item>
            )}
          />
        ) : null}
        {identityResult ? (
          <Card size="small" style={{ marginTop: 12 }} data-testid="asr-identify-result">
            <Space>
              <Tag color={identityResult.matched ? "success" : identityResult.status === "insufficient_audio" ? "processing" : "default"}>
                {identityResult.display_name}
              </Tag>
              <Text type="secondary">
                {identityResult.status === "insufficient_audio"
                  ? `Listening: ${Math.round((identityResult.speech_duration_ms ?? 0) / 100) / 10}s speech`
                  : `${Math.round(identityResult.confidence * 100)}% match`}
              </Text>
            </Space>
          </Card>
        ) : null}
      </Card>
      <Modal
        title="Enroll Voiceprint"
        open={enrollOpen}
        okText={enrolling ? "Recording" : "Start"}
        confirmLoading={enrolling}
        onOk={() => void enrollSpeaker()}
        onCancel={() => {
          if (!enrolling) {
            setEnrollOpen(false);
            setActivePrompt(null);
            setActivePromptIndex(0);
            setActivePromptTotal(0);
            setActiveTranscript("");
            setActiveMatchScore(0);
          }
        }}
        okButtonProps={{ disabled: !speakerName.trim() || enrolling }}
        cancelButtonProps={{ disabled: enrolling }}
        destroyOnClose
      >
        <Space direction="vertical" style={{ width: "100%" }}>
          <Input
            data-testid="asr-enroll-speaker-name-input"
            placeholder="Speaker name"
            value={speakerName}
            disabled={enrolling}
            onChange={(event) => setSpeakerName(event.target.value)}
          />
          {activePrompt ? (
            <Card size="small" data-testid="asr-enroll-active-prompt">
              <Space direction="vertical">
                <Tag color="processing">
                  Listening {activePromptIndex}/{activePromptTotal || 1}
                </Tag>
                <Text>{activePrompt.text}</Text>
                <Text type="secondary" data-testid="asr-enroll-recognized-text">
                  {activeTranscript ? `Recognized: ${activeTranscript}` : "Waiting for recognized speech"}
                </Text>
                <Progress
                  percent={Math.round(activeMatchScore * 100)}
                  size="small"
                  status={activeMatchScore >= PROMPT_MATCH_THRESHOLD ? "success" : "active"}
                  data-testid="asr-enroll-match-progress"
                />
              </Space>
            </Card>
          ) : null}
        </Space>
      </Modal>
    </>
  );
}

type PromptRecordingOptions = {
  sessionId: string;
  minDurationMs: number;
  maxDurationMs: number;
  verifyIntervalMs: number;
  matchThreshold: number;
  onRecognitionUpdate: (update: { transcript: string; score: number }) => void;
};

async function recordPromptPcm16(
  prompt: AsrSpeakerEnrollmentPrompt,
  options: PromptRecordingOptions,
): Promise<string> {
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      channelCount: 1,
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    },
  });
  const AudioContextCtor = window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
  const context = new AudioContextCtor();
  const sourceSampleRate = context.sampleRate;
  const source = context.createMediaStreamSource(stream);
  const processor = context.createScriptProcessor(4096, 1, 1);
  const chunks: Float32Array[] = [];

  processor.onaudioprocess = (event) => {
    chunks.push(new Float32Array(event.inputBuffer.getChannelData(0)));
  };
  source.connect(processor);
  processor.connect(context.destination);

  try {
    await new Promise<void>((resolve, reject) => {
      let settled = false;
      const startedAt = Date.now();
      let verifyTimer: number | undefined;
      let timeoutId: number | undefined;

      const settle = (finish: () => void) => {
        if (settled) {
          return;
        }
        settled = true;
        if (timeoutId !== undefined) {
          window.clearTimeout(timeoutId);
        }
        if (verifyTimer !== undefined) {
          window.clearTimeout(verifyTimer);
        }
        finish();
      };
      timeoutId = window.setTimeout(() => {
        settle(() =>
          reject(new Error("The recognized speech did not match the prompt before timeout")),
        );
      }, options.maxDurationMs);

      const verify = async () => {
        if (settled) {
          return;
        }
        const elapsedMs = Date.now() - startedAt;
        if (elapsedMs < options.minDurationMs) {
          verifyTimer = window.setTimeout(verify, options.minDurationMs - elapsedMs);
          return;
        }
        try {
          const audio = currentRecordingBase64(chunks, sourceSampleRate);
          const result = await verifyAsrSpeakerEnrollmentPrompt(options.sessionId, prompt.id, audio);
          options.onRecognitionUpdate({
            transcript: result.transcript,
            score: result.match_score,
          });
          if (result.matched || result.match_score >= options.matchThreshold) {
            settle(() => resolve());
            return;
          }
          verifyTimer = window.setTimeout(verify, options.verifyIntervalMs);
        } catch (error) {
          const messageText = error instanceof Error ? error.message : String(error);
          if (messageText.includes("model service is not running")) {
            settle(() => reject(error instanceof Error ? error : new Error(messageText)));
            return;
          }
          if (!settled) {
            verifyTimer = window.setTimeout(verify, options.verifyIntervalMs);
          }
        }
      };

      verifyTimer = window.setTimeout(verify, options.minDurationMs);
    });
  } finally {
    processor.disconnect();
    source.disconnect();
    stream.getTracks().forEach((track) => track.stop());
    await context.close();
  }

  return currentRecordingBase64(chunks, sourceSampleRate);
}

type IdentityRecordingOptions = {
  minDurationMs: number;
  maxDurationMs: number;
  verifyIntervalMs: number;
  onIdentifyUpdate: (result: AsrSpeakerIdentifyResult) => void;
};

async function recordIdentityPcm16(options: IdentityRecordingOptions): Promise<AsrSpeakerIdentifyResult> {
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      channelCount: 1,
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    },
  });
  const AudioContextCtor = window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
  const context = new AudioContextCtor();
  const sourceSampleRate = context.sampleRate;
  const source = context.createMediaStreamSource(stream);
  const processor = context.createScriptProcessor(4096, 1, 1);
  const chunks: Float32Array[] = [];

  processor.onaudioprocess = (event) => {
    chunks.push(new Float32Array(event.inputBuffer.getChannelData(0)));
  };
  source.connect(processor);
  processor.connect(context.destination);

  try {
    return await new Promise<AsrSpeakerIdentifyResult>((resolve, reject) => {
      let settled = false;
      const startedAt = Date.now();
      let verifyTimer: number | undefined;
      let timeoutId: number | undefined;
      let bestResult: AsrSpeakerIdentifyResult | null = null;
      let lastResult: AsrSpeakerIdentifyResult | null = null;

      const settle = (finish: () => void) => {
        if (settled) {
          return;
        }
        settled = true;
        if (timeoutId !== undefined) {
          window.clearTimeout(timeoutId);
        }
        if (verifyTimer !== undefined) {
          window.clearTimeout(verifyTimer);
        }
        finish();
      };

      const rememberResult = (result: AsrSpeakerIdentifyResult) => {
        lastResult = result;
        if (result.status !== "insufficient_audio" && (!bestResult || result.confidence > bestResult.confidence)) {
          bestResult = result;
        }
        options.onIdentifyUpdate(result);
      };

      const verify = async () => {
        if (settled) {
          return;
        }
        const elapsedMs = Date.now() - startedAt;
        if (elapsedMs < options.minDurationMs) {
          verifyTimer = window.setTimeout(verify, options.minDurationMs - elapsedMs);
          return;
        }
        try {
          const audio = currentRecordingBase64(chunks, sourceSampleRate);
          const result = await identifyAsrSpeakerVoice(audio);
          rememberResult(result);
          if (result.matched) {
            settle(() => resolve(result));
            return;
          }
          if (!settled) {
            verifyTimer = window.setTimeout(verify, options.verifyIntervalMs);
          }
        } catch (error) {
          settle(() => reject(error instanceof Error ? error : new Error(String(error))));
        }
      };

      timeoutId = window.setTimeout(() => {
        settle(() =>
          resolve(
            bestResult ??
              lastResult ?? {
              matched: false,
              profile_id: null,
              display_name: "用户A",
              speaker: "speaker_00",
              confidence: 0,
              status: "insufficient_audio",
              reason: "need_more_speech",
              audio_duration_ms: Date.now() - startedAt,
              speech_duration_ms: 0,
            },
          ),
        );
      }, options.maxDurationMs);

      verifyTimer = window.setTimeout(verify, options.minDurationMs);
    });
  } finally {
    processor.disconnect();
    source.disconnect();
    stream.getTracks().forEach((track) => track.stop());
    await context.close();
  }
}

function currentRecordingBase64(chunks: Float32Array[], sourceSampleRate: number): string {
  const merged = mergeFloat32Chunks(chunks);
  const pcm16 = float32ToPcm16(resampleTo16k(merged, sourceSampleRate));
  return bytesToBase64(pcm16);
}

function mergeFloat32Chunks(chunks: Float32Array[]): Float32Array {
  const length = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const merged = new Float32Array(length);
  let offset = 0;
  chunks.forEach((chunk) => {
    merged.set(chunk, offset);
    offset += chunk.length;
  });
  return merged;
}

function resampleTo16k(input: Float32Array, sourceRate: number): Float32Array {
  if (sourceRate === 16000) {
    return input;
  }
  const ratio = sourceRate / 16000;
  const output = new Float32Array(Math.max(1, Math.floor(input.length / ratio)));
  for (let index = 0; index < output.length; index += 1) {
    output[index] = input[Math.min(input.length - 1, Math.floor(index * ratio))] || 0;
  }
  return output;
}

function float32ToPcm16(input: Float32Array): Uint8Array {
  const output = new Uint8Array(input.length * 2);
  const view = new DataView(output.buffer);
  input.forEach((sample, index) => {
    const clamped = Math.max(-1, Math.min(1, sample));
    view.setInt16(index * 2, clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff, true);
  });
  return output;
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
  }
  return window.btoa(binary);
}
