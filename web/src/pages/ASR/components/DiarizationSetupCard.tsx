import { useEffect, useRef, useState } from "react";
import { Alert, Button, Card, Descriptions, Divider, Input, List, Modal, Popconfirm, Progress, Segmented, Select, Space, Tag, Typography, message } from "antd";
import { AudioOutlined, DeleteOutlined, FolderOpenOutlined, PlayCircleOutlined, ReloadOutlined, SafetyCertificateOutlined, ToolOutlined, UserAddOutlined } from "@ant-design/icons";
import type {
  AsrAssistedCandidateLabel,
  AsrAssistedVoiceprintSessionPayload,
  AsrDirectoryTask,
  AsrDirectoryTaskDetail,
  AsrDiarizationStatus,
  AsrSpeakerIdentifyResult,
  AsrSpeakerEnrollmentPrompt,
  AsrSpeakerProfileDetail,
  AsrSpeakerProfileSummary,
} from "../../../api/asr";
import {
  appendAsrSpeakerEnrollmentAudio,
  buildAsrTaskFileSourceUrl,
  createAsrAssistedVoiceprintSession,
  createAsrSpeakerEnrollmentSession,
  deleteAsrAssistedVoiceprintSession,
  deleteAsrSpeakerProfile,
  deleteAsrSpeakerProfileSample,
  finishAsrAssistedVoiceprintSession,
  finishAsrSpeakerEnrollment,
  getAsrSpeakerProfile,
  getAsrTask,
  getAsrDiarizationStatus,
  identifyAsrSpeakerVoice,
  initAsrDiarizationProfile,
  listAsrTasks,
  listAsrSpeakerProfiles,
  updateAsrAssistedVoiceprintLabels,
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
  const [assistedOpen, setAssistedOpen] = useState(false);
  const [assistedBusy, setAssistedBusy] = useState(false);
  const [assistedProfileId, setAssistedProfileId] = useState<string | undefined>();
  const [assistedTaskId, setAssistedTaskId] = useState<string>();
  const [assistedFileKey, setAssistedFileKey] = useState<string>();
  const [assistedTasks, setAssistedTasks] = useState<AsrDirectoryTask[]>([]);
  const [assistedTask, setAssistedTask] = useState<AsrDirectoryTaskDetail | null>(null);
  const [assistedSession, setAssistedSession] = useState<AsrAssistedVoiceprintSessionPayload | null>(null);
  const [profileDetail, setProfileDetail] = useState<AsrSpeakerProfileDetail | null>(null);
  const [profileDetailOpen, setProfileDetailOpen] = useState(false);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const audioStopTimerRef = useRef<number | undefined>(undefined);

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

  const openAssistedEnrollment = async (profile?: AsrSpeakerProfileSummary) => {
    setAssistedOpen(true);
    setAssistedProfileId(profile?.id);
    setSpeakerName(profile?.display_name ?? "");
    setAssistedTaskId(undefined);
    setAssistedFileKey(undefined);
    setAssistedTask(null);
    setAssistedSession(null);
    try {
      setAssistedTasks(await listAsrTasks());
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to load ASR tasks");
    }
  };

  const selectAssistedTask = async (taskId: string) => {
    setAssistedTaskId(taskId);
    setAssistedFileKey(undefined);
    setAssistedTask(null);
    try {
      setAssistedTask(await getAsrTask(taskId));
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to load task recordings");
    }
  };

  const createAssistedSession = async () => {
    if (!speakerName.trim() || !assistedTaskId || !assistedFileKey) return;
    setAssistedBusy(true);
    try {
      const payload = await createAsrAssistedVoiceprintSession({
        name: speakerName.trim(),
        profile_id: assistedProfileId,
        task_id: assistedTaskId,
        file_key: assistedFileKey,
      });
      setAssistedSession(payload);
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to prepare voiceprint candidates");
    } finally {
      setAssistedBusy(false);
    }
  };

  const labelAssistedCandidate = async (candidateId: string, label: AsrAssistedCandidateLabel) => {
    if (!assistedSession) return;
    try {
      const payload = await updateAsrAssistedVoiceprintLabels(assistedSession.session.id, [
        { candidate_id: candidateId, label },
      ]);
      setAssistedSession(payload);
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to save candidate label");
    }
  };

  const playAssistedCandidate = (startMs: number, endMs: number) => {
    const audio = audioRef.current;
    if (!audio) return;
    if (audioStopTimerRef.current !== undefined) {
      window.clearTimeout(audioStopTimerRef.current);
    }
    audio.currentTime = startMs / 1000;
    void audio.play();
    audioStopTimerRef.current = window.setTimeout(() => {
      audio.pause();
      audioStopTimerRef.current = undefined;
    }, Math.max(0, endMs - startMs));
  };

  const finishAssistedSession = async () => {
    if (!assistedSession?.ready_to_finish) return;
    setAssistedBusy(true);
    try {
      const result = await finishAsrAssistedVoiceprintSession(assistedSession.session.id);
      message.success(`Voiceprint samples saved for ${result.profile.display_name}`);
      setAssistedOpen(false);
      setAssistedSession(null);
      await refresh();
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to finish voiceprint enrollment");
    } finally {
      setAssistedBusy(false);
    }
  };

  const closeAssistedEnrollment = async () => {
    if (assistedBusy) return;
    if (audioStopTimerRef.current !== undefined) {
      window.clearTimeout(audioStopTimerRef.current);
      audioStopTimerRef.current = undefined;
    }
    const sessionId = assistedSession?.session.id;
    setAssistedOpen(false);
    setAssistedSession(null);
    setSpeakerName("");
    if (sessionId) {
      try {
        await deleteAsrAssistedVoiceprintSession(sessionId);
      } catch {
        // Expiry cleanup is a server-side fallback; closing the UI should stay responsive.
      }
    }
  };

  const showProfileDetail = async (profileId: string) => {
    try {
      setProfileDetail(await getAsrSpeakerProfile(profileId));
      setProfileDetailOpen(true);
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to load voiceprint samples");
    }
  };

  const deleteProfileSample = async (profileId: string, sampleId: string) => {
    try {
      await deleteAsrSpeakerProfileSample(profileId, sampleId);
      setProfileDetail(await getAsrSpeakerProfile(profileId));
      await refresh();
      message.success("Voiceprint sample deleted and profile rebuilt");
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to delete voiceprint sample");
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
              type="primary"
              icon={<FolderOpenOutlined />}
              data-testid="asr-assisted-enroll-button"
              disabled={!status?.profile.ready}
              onClick={() => void openAssistedEnrollment()}
            >
              Add from Recording
            </Button>
            <Button
              size="small"
              icon={<UserAddOutlined />}
              data-testid="asr-enroll-voiceprint-button"
              disabled={!status?.profile.ready}
              onClick={() => setEnrollOpen(true)}
            >
              Record Voice
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
                  <Button key="samples" size="small" onClick={() => void showProfileDetail(profile.id)}>
                    Samples
                  </Button>,
                  <Button key="append" size="small" onClick={() => void openAssistedEnrollment(profile)}>
                    Add samples
                  </Button>,
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
                  <Text type="secondary">
                    {profile.template_count} samples · {profile.prototype_count} prototypes · {Math.round(profile.total_duration_ms / 100) / 10}s
                  </Text>
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
      <Modal
        title={assistedProfileId ? "Add Voiceprint Samples" : "Initialize Voiceprint from Recording"}
        open={assistedOpen}
        width={860}
        confirmLoading={assistedBusy}
        okText={assistedSession ? "Save Voiceprint" : "Find Speaker Segments"}
        onOk={() => void (assistedSession ? finishAssistedSession() : createAssistedSession())}
        onCancel={() => void closeAssistedEnrollment()}
        okButtonProps={{
          disabled: assistedSession
            ? !assistedSession.ready_to_finish
            : !speakerName.trim() || !assistedTaskId || !assistedFileKey,
          "data-testid": "asr-assisted-finish-button",
        }}
        cancelButtonProps={{ disabled: assistedBusy }}
        destroyOnClose
      >
        {!assistedSession ? (
          <Space direction="vertical" size="middle" style={{ width: "100%" }}>
            <Alert
              type="info"
              showIcon
              message="Choose a real meeting recording, then confirm only the segments spoken by you."
              description="Bifrost excludes overlapping and short segments. Nothing is added permanently until you save the voiceprint."
            />
            <Input
              data-testid="asr-assisted-speaker-name"
              placeholder="Speaker name"
              value={speakerName}
              disabled={Boolean(assistedProfileId)}
              onChange={(event) => setSpeakerName(event.target.value)}
            />
            <Select
              data-testid="asr-assisted-task-select"
              style={{ width: "100%" }}
              placeholder="Select an ASR task"
              value={assistedTaskId}
              options={assistedTasks.map((task) => ({ label: task.name, value: task.id }))}
              onChange={(value) => void selectAssistedTask(value)}
            />
            <Select
              data-testid="asr-assisted-file-select"
              style={{ width: "100%" }}
              placeholder="Select a completed speaker-aware recording"
              value={assistedFileKey}
              disabled={!assistedTask}
              options={(assistedTask?.files ?? [])
                .filter((file) => ["success", "partial_success"].includes(file.status) && file.output_timeline_path)
                .map((file) => ({
                  label: `${file.source_path.split("/").at(-1) ?? file.source_path}${file.speaker_count ? ` · ${file.speaker_count} speakers` : ""}`,
                  value: file.key,
                }))}
              onChange={setAssistedFileKey}
            />
          </Space>
        ) : (
          <Space direction="vertical" size="middle" style={{ width: "100%" }}>
            <audio
              ref={audioRef}
              controls
              preload="metadata"
              src={buildAsrTaskFileSourceUrl(
                assistedSession.session.task_id,
                assistedSession.session.file_key,
              )}
              style={{ width: "100%" }}
              data-testid="asr-assisted-audio-player"
            />
            <Alert
              type={assistedSession.ready_to_finish ? "success" : "warning"}
              showIcon
              message={`${assistedSession.selected_count} segments selected · ${Math.round(assistedSession.selected_duration_ms / 100) / 10}s`}
              description={assistedSession.ready_to_finish
                ? "Enough confirmed speech is selected. More varied, clean segments can improve cross-device recognition."
                : `Select at least ${assistedSession.minimum_clips} segments and ${assistedSession.minimum_duration_ms / 1000}s of your speech.`}
            />
            <List
              size="small"
              data-testid="asr-assisted-candidate-list"
              dataSource={assistedSession.session.candidates}
              renderItem={(candidate) => (
                <List.Item
                  actions={[
                    <Button
                      key="play"
                      size="small"
                      icon={<PlayCircleOutlined />}
                      onClick={() => playAssistedCandidate(candidate.start_ms, candidate.end_ms)}
                    >
                      Play
                    </Button>,
                    <Segmented<AsrAssistedCandidateLabel>
                      key="label"
                      size="small"
                      value={candidate.label}
                      options={[
                        { label: "Mine", value: "mine" },
                        { label: "Not mine", value: "not_mine" },
                        { label: "Skip", value: "unsure" },
                      ]}
                      onChange={(value) => void labelAssistedCandidate(candidate.id, value)}
                    />,
                  ]}
                >
                  <Space direction="vertical" size={2} style={{ minWidth: 0 }}>
                    <Space>
                      <Tag>{candidate.speaker}</Tag>
                      <Text type="secondary">
                        {(candidate.start_ms / 1000).toFixed(1)}–{(candidate.end_ms / 1000).toFixed(1)}s · {(candidate.duration_ms / 1000).toFixed(1)}s
                      </Text>
                      <Tag color={candidate.quality >= 0.8 ? "success" : "processing"}>
                        {Math.round(candidate.quality * 100)}% quality
                      </Tag>
                    </Space>
                    <Text ellipsis={{ tooltip: candidate.text }}>{candidate.text || "No transcript text"}</Text>
                  </Space>
                </List.Item>
              )}
            />
          </Space>
        )}
      </Modal>
      <Modal
        title={profileDetail ? `${profileDetail.display_name} Voiceprint Samples` : "Voiceprint Samples"}
        open={profileDetailOpen}
        footer={null}
        onCancel={() => setProfileDetailOpen(false)}
        destroyOnClose
      >
        {profileDetail ? (
          <>
            <Descriptions size="small" column={2}>
              <Descriptions.Item label="Templates">{profileDetail.templates.length}</Descriptions.Item>
              <Descriptions.Item label="Total speech">{Math.round(profileDetail.total_duration_ms / 100) / 10}s</Descriptions.Item>
            </Descriptions>
            <Divider />
            {profileDetail.templates.length ? (
              <List
                size="small"
                dataSource={profileDetail.templates}
                renderItem={(sample) => (
                  <List.Item
                    actions={[
                      <Popconfirm
                        key="delete"
                        title="Delete this sample and rebuild the profile?"
                        onConfirm={() => void deleteProfileSample(profileDetail.id, sample.id)}
                      >
                        <Button danger size="small" icon={<DeleteOutlined />} disabled={profileDetail.templates.length <= 1} />
                      </Popconfirm>,
                    ]}
                  >
                    <Space direction="vertical" size={2}>
                      <Space>
                        <Tag>{sample.source_kind}</Tag>
                        <Text>{(sample.duration_ms / 1000).toFixed(1)}s</Text>
                        {sample.speaker ? <Text type="secondary">{sample.speaker}</Text> : null}
                      </Space>
                      <Text type="secondary">{sample.id}</Text>
                    </Space>
                  </List.Item>
                )}
              />
            ) : (
              <Alert type="info" message="Legacy centroid profile" description="Add recording samples to migrate this profile to editable multi-template format." />
            )}
          </>
        ) : null}
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

      const settle = (finish: () => void) => {
        if (settled) {
          return;
        }
        settled = true;
        window.clearTimeout(timeoutId);
        if (verifyTimer !== undefined) {
          window.clearTimeout(verifyTimer);
        }
        finish();
      };
      const timeoutId = window.setTimeout(() => {
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
      let bestResult: AsrSpeakerIdentifyResult | null = null;
      let lastResult: AsrSpeakerIdentifyResult | null = null;

      const settle = (finish: () => void) => {
        if (settled) {
          return;
        }
        settled = true;
        window.clearTimeout(timeoutId);
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

      const timeoutId = window.setTimeout(() => {
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
