import { buildApiUrl, buildWsUrl } from "../runtime";
import { getClientId } from "../services/clientId";
import { getAdminToken } from "../services/adminAuth";
import { get, post } from "./client";

export interface AsrConnectionParams {
  host?: string;
  port?: number;
  language?: string;
  model?: string;
  ownerModule?: string;
  ownerId?: string;
}

export interface VoiceRealtimeParams extends AsrConnectionParams {
  chunkMs?: number;
}

export interface SpeechPipelineProfile {
  id: string;
  label: string;
  mode: string[];
  resources: {
    priority: number;
    preemptible: boolean;
    max_concurrent_files?: number | null;
    pause_on_realtime_voice: boolean;
  };
}

export interface SpeechPipelinesStatus {
  profiles: SpeechPipelineProfile[];
  runtime: {
    platform_supported: boolean;
    asr_ready: boolean;
    diarization_ready: boolean;
    realtime_voice_active: boolean;
    offline_asr_active: boolean;
  };
  resources: {
    leases: unknown[];
    realtime_voice_active: boolean;
    offline_asr_active: boolean;
    wake_listener_active: boolean;
  };
}

export interface AsrOfflineJob {
  job_id: string;
  status: "queued" | "running" | "succeeded" | "failed";
  source_name: string;
  model: string;
  language: string;
  pipeline_profile: string;
  speaker_aware: boolean;
  created_at_ms: number;
  updated_at_ms: number;
  error?: string | null;
  artifacts?: Record<string, string> | null;
  events: Array<{
    at_ms: number;
    stage: string;
    message: string;
  }>;
}

export interface AsrOfflineJobCreateOptions {
  model?: string;
  language?: string;
  pipelineProfile?: string;
  speakerAware?: boolean;
}

export interface AsrStatus {
  status: "unsupported" | "missing" | "installed" | "ready";
  ready: boolean;
  installed: boolean;
  platform_supported: boolean;
  unsupported_reason?: string;
  ffmpeg_available: boolean;
  managed: boolean;
  server_url: string;
  install_dir: string;
  model_dir: string;
  model: string;
  language: string;
  owner_module: string;
  owner_id?: string;
  message: string;
}

export interface AsrCapabilityFlag {
  enabled: boolean;
  hidden: boolean;
  platform_supported: boolean;
  reason?: string;
}

export interface AsrCapabilities {
  platform: string;
  arch: string;
  supported_target: "macos-aarch64";
  qwen3_asr: AsrCapabilityFlag;
  local_transcription: AsrCapabilityFlag;
  speech_workbench: AsrCapabilityFlag;
  directory_tasks: AsrCapabilityFlag;
  speaker_diarization: AsrCapabilityFlag;
  voiceprint: AsrCapabilityFlag;
  voice_wake_asr: AsrCapabilityFlag;
}

export interface AsrDiarizationProfileInfo {
  id: string;
  label: string;
  engine: string;
  quality_tier: string;
  requires_init: boolean;
  ready: boolean;
  install_dir: string;
  message: string;
}

export interface AsrDiarizationStatus {
  profile: AsrDiarizationProfileInfo;
  voiceprint_dir: string;
  speaker_profile_count: number;
}

export interface AsrSpeakerProfileSummary {
  id: string;
  display_name: string;
  embedding_dim: number;
}

export interface AsrSpeakerEnrollmentPrompt {
  id: string;
  text: string;
}

export interface AsrSpeakerEnrollmentSession {
  id: string;
  speaker_name: string;
  diarization_profile: string;
  sample_rate: number;
  audio_format: string;
  prompts: AsrSpeakerEnrollmentPrompt[];
}

export interface AsrSpeakerEnrollmentResult {
  profile: {
    id: string;
    display_name: string;
    source: string;
    diarization_profile: string;
    embedding_dim: number;
    total_duration_ms: number;
  };
  profile_path: string;
}

export interface AsrSpeakerProfilesResponse {
  profiles: AsrSpeakerProfileSummary[];
}

export interface VoiceWakeStatus {
  enabled: boolean;
  profile_count: number;
  binding_count: number;
  event_count: number;
  mode: string;
  fallback?: string | null;
  requires_qwen_by_default?: boolean;
  kws?: {
    engine: string;
    profile: string;
    ready: boolean;
    install_dir: string;
    missing_files: string[];
  };
  store_path: string;
  default_dry_run: boolean;
  listener: VoiceWakeListenerStatus;
}

export interface VoiceWakeListenerStatus {
  running: boolean;
  source: string;
  engine?: string;
  device?: string | null;
  device_label?: string | null;
  worker_pid?: number | null;
  chunk_ms: number;
  started_at_ms: number | null;
  stopped_at_ms: number | null;
  last_transcript: string | null;
  last_transcript_at_ms: number | null;
  last_error: string | null;
  last_error_at_ms: number | null;
  last_speaker_profile_id: string | null;
  last_speaker_confidence: number | null;
  last_speaker_status: string | null;
  last_match_status: string | null;
  last_match_binding_id: string | null;
  last_match_phrase: string | null;
  last_action_result: VoiceWakeActionResult | null;
  last_action_at_ms: number | null;
  trigger_count: number;
}

export interface VoiceWakeProfile {
  id: string;
  display_name: string;
  voiceprint_profile_id: string | null;
  speaker_threshold: number;
  created_at_ms: number;
  updated_at_ms: number;
}

export interface VoiceWakeProfilesResponse {
  profiles: VoiceWakeProfile[];
}

export interface VoiceWakeActionKeyPress {
  type: "key_press";
  key: string | null;
  keycode: number | null;
  modifiers: string[];
  press_count: number;
  repeat_delay_ms?: number;
}

export type VoiceWakeAction = VoiceWakeActionKeyPress;

export interface VoiceWakeBinding {
  id: string;
  enabled: boolean;
  phrase: string;
  normalized_phrase: string;
  profile_id: string;
  kws_score: number;
  kws_threshold: number;
  speaker_threshold: number;
  cooldown_ms: number;
  action: VoiceWakeAction;
  created_at_ms: number;
  updated_at_ms: number;
  last_triggered_at_ms: number | null;
}

export interface VoiceWakeBindingsResponse {
  bindings: VoiceWakeBinding[];
}

export interface VoiceWakeActionResult {
  action_type: string;
  dry_run: boolean;
  executed: boolean;
  platform: string;
  message: string;
  command_preview: string | null;
}

export interface VoiceWakeEvent {
  id: string;
  binding_id: string;
  phrase: string;
  profile_id: string;
  speaker_confidence: number | null;
  dry_run: boolean;
  matched_at_ms: number;
  action_result: VoiceWakeActionResult;
}

export interface VoiceWakeEventsResponse {
  events: VoiceWakeEvent[];
}

export interface VoiceWakeTriggerResponse {
  matched: boolean;
  binding: VoiceWakeBinding;
  event: VoiceWakeEvent;
  action_result: VoiceWakeActionResult;
}

export interface CreateVoiceWakeProfileRequest {
  id?: string;
  display_name: string;
  voiceprint_profile_id?: string;
  speaker_threshold?: number;
}

export interface CreateVoiceWakeBindingRequest {
  id?: string;
  phrase: string;
  profile_id: string;
  enabled?: boolean;
  kws_score?: number;
  kws_threshold?: number;
  speaker_threshold?: number;
  cooldown_ms?: number;
  action: VoiceWakeAction;
}

export interface AsrServiceResult {
  ready: boolean;
  managed: boolean;
  server_url: string;
  message: string;
  detail?: string;
}

export function buildAsrQueryForTest(params: AsrConnectionParams): string {
  return buildAsrQuery(params);
}

export interface AsrProgressEvent {
  type: "progress" | "connected" | "stream" | "finish";
  phase: string;
  status: string;
  progress: number;
  message: string;
  detail?: string;
  file?: string;
  server_url?: string;
  downloaded_bytes?: number;
  total_bytes?: number;
  download_percent?: number;
  bytes_per_second?: number;
  eta_seconds?: number;
  elapsed_ms?: number;
  resumed?: boolean;
  complete?: boolean;
}

export interface AsrTextEvent {
  type: "text";
  text: string;
}

export interface AsrSegmentEvent {
  type: "partial" | "final";
  index: number;
  start_ms: number;
  end_ms: number;
  stable_start_ms: number;
  stable_end_ms: number;
  text: string;
  delta: string;
  committed: string;
  speaker?: string;
  speaker_display_name?: string;
  speaker_profile_id?: string;
  speaker_confidence?: number;
}

export interface AsrErrorEvent {
  type: "error";
  message: string;
  detail?: string;
}

export interface AsrDoneEvent {
  type: "done";
  ok?: boolean;
}

export interface AsrTaskSummary {
  discovered: number;
  processed: number;
  pending: number;
  failed: number;
  partial_success: number;
  failed_chunk_count: number;
  deleted_after_processing: number;
  audio_source_bytes: number;
  audio_source_file_count: number;
  cleanable_source_bytes: number;
  cleanable_source_file_count: number;
  running: boolean;
  diarization_enabled?: boolean;
  diarization_ready?: boolean;
  diarization_running?: boolean;
  diarized_files?: number;
  speaker_count?: number;
}

export type AsrRuntimeStrategy =
  | "fork_per_chunk"
  | "reuse_server"
  | "reuse_per_file"
  | "auto"
  | "compare";

export type AsrTaskFileStatus = "pending" | "processing" | "success" | "partial_success" | "failed";

export interface AsrFailedChunkRecord {
  chunk_index: number;
  offset_secs: number;
  duration_secs: number;
  error: string;
  attempts: number;
  energy_rms?: number;
  is_silent?: boolean;
}

export interface AsrTaskFileRecord {
  key: string;
  task_id: string;
  source_path: string;
  source_size?: number;
  source_modified_ms?: number;
  source_created_at_ms?: number;
  source_created_at_source?: string;
  media_duration_ms?: number;
  status: AsrTaskFileStatus;
  output_text_path?: string;
  output_metadata_path?: string;
  output_timeline_path?: string;
  text_chars: number;
  error?: string;
  runtime_strategy?: AsrRuntimeStrategy;
  fallback_reason?: string;
  chunk_metrics?: AsrChunkMetric[];
  started_at_ms?: number;
  finished_at_ms?: number;
  progress_current?: number;
  progress_total?: number;
  failed_chunks?: AsrFailedChunkRecord[];
  diarization_status?: string;
  diarization_manifest_path?: string;
  speaker_count?: number;
}

export interface AsrChunkMetric {
  chunk_index: number;
  offset_secs: number;
  duration_secs: number;
  runner: string;
  status: string;
  elapsed_ms: number;
  rtf: number;
  text_chars: number;
  text_sha1: string;
  server_url?: string;
  fallback_reason?: string;
  error?: string;
  recorded_at_ms: number;
}

export interface AsrTimelineSegment {
  index: number;
  audio_start_ms: number;
  audio_end_ms: number;
  absolute_start_ms?: number;
  absolute_end_ms?: number;
  speaker?: string;
  speaker_display_name?: string;
  overlap?: boolean;
  text: string;
}

export interface AsrTimelineSpeaker {
  id: string;
  display_name: string;
  mapped_profile_id?: string;
  confidence?: number;
  candidate_profile_id?: string;
  candidate_display_name?: string;
  candidate_confidence?: number;
}

export interface AsrTranscriptTimeline {
  task_id: string;
  task_name: string;
  source_path: string;
  source_size?: number;
  source_modified_ms?: number;
  source_created_at_ms?: number;
  source_created_at_source?: string;
  media_duration_ms?: number;
  model: string;
  language: string;
  diarization_profile?: string;
  speakers?: AsrTimelineSpeaker[];
  processed_at_ms: number;
  segments: AsrTimelineSegment[];
}

export interface AsrTaskDailyDocument {
  date: string;
  path: string;
  size?: number;
  modified_ms?: number;
  text_chars: number;
}

export interface AsrTaskDailyDocumentDetail extends AsrTaskDailyDocument {
  task_id: string;
  task_name: string;
  content: string;
}

export type AsrTaskSchedule =
  | { kind: "hourly"; minute: number }
  | { kind: "daily"; hour: number; minute: number }
  | { kind: "weekly"; weekday: number; hour: number; minute: number }
  | { kind: "monthly"; day: number; hour: number; minute: number };

export interface AsrDirectoryTask {
  id: string;
  name: string;
  audio_dir: string;
  recursive: boolean;
  enabled: boolean;
  paused?: boolean;
  paused_at_ms?: number;
  schedule: AsrTaskSchedule;
  language: string;
  model: string;
  runtime_strategy: AsrRuntimeStrategy;
  diarization?: {
    enabled: boolean;
    profile: string;
    min_speakers?: number;
    max_speakers?: number;
    known_speaker_count?: number;
    voiceprint_matching?: boolean;
  };
  created_at_ms: number;
  updated_at_ms: number;
  last_run_at_ms?: number;
  next_run_at_ms?: number;
  last_error?: string;
  external_devices?: AsrExternalDeviceBinding[];
  import_policy?: AsrExternalImportPolicy;
  summary: AsrTaskSummary;
  bulk_retry?: AsrBulkRetryState;
}

export type AsrPauseMode = "temporary" | "long_term";

export interface AsrDirectoryTaskDetail extends AsrDirectoryTask {
  files: AsrTaskFileRecord[];
  daily_documents?: AsrTaskDailyDocument[];
}

export interface AsrBulkRetryFileResult {
  file_key: string;
  source_path: string;
  failed_before: number;
  recovered: number;
  still_failed: number;
  status: string;
  elapsed_ms: number;
  message: string;
  daily_documents_refreshed?: string[];
  persist_warnings?: string[];
}

export interface AsrBulkRetryState {
  task_id: string;
  status: "queued" | "running" | "completed" | "failed";
  queued_files: number;
  processed_files: number;
  total_failed_chunks: number;
  recovered_chunks: number;
  still_failed_chunks: number;
  started_at_ms?: number;
  updated_at_ms: number;
  finished_at_ms?: number;
  current_file_key?: string;
  current_source_path?: string;
  message: string;
  results: AsrBulkRetryFileResult[];
}

export interface CreateAsrTaskRequest {
  name?: string;
  audio_dir: string;
  recursive?: boolean;
  enabled?: boolean;
  schedule?: AsrTaskSchedule;
  language?: string;
  model?: string;
  runtime_strategy?: AsrRuntimeStrategy;
  diarization?: AsrDirectoryTask["diarization"];
  external_devices?: AsrExternalDeviceBinding[];
  import_policy?: AsrExternalImportPolicy;
}

export interface UpdateAsrTaskRequest extends Partial<CreateAsrTaskRequest> {
  paused?: boolean;
}

export interface AsrExternalDeviceBinding {
  name: string;
  display_name?: string;
  volume_uuid?: string;
  device_identifier?: string;
  enabled?: boolean;
  include_globs?: string[];
  exclude_globs?: string[];
  last_seen_at_ms?: number;
  last_import_at_ms?: number;
  last_status?: string;
}

export interface AsrExternalImportPolicy {
  enabled: boolean;
  file_stable_secs: number;
  min_free_bytes: number;
  max_file_bytes: number;
  auto_run_after_import: boolean;
  content_hash_dedupe_enabled: boolean;
  content_hash_algorithm: string;
  delete_source_after_import: boolean;
}

export interface AsrExternalVolume {
  name: string;
  mount_path: string;
  volume_uuid?: string;
  device_identifier?: string;
  kind: string;
  read_only: boolean;
  available_bytes?: number;
}

export interface AsrExternalImportStatus {
  policy: AsrExternalImportPolicy;
  devices: Array<{
    binding: AsrExternalDeviceBinding;
    connected: boolean;
    mount_path?: string;
    status: string;
    last_error?: string;
  }>;
  runs: Array<{
    run_id: string;
    device_name?: string;
    started_at_ms: number;
    finished_at_ms: number;
    imported: number;
    skipped: number;
    failed: number;
    status: string;
  }>;
  current_run?: AsrExternalImportRunProgress;
}

export interface AsrExternalImportRunProgress {
  run_id: string;
  trigger: string;
  started_at_ms: number;
  updated_at_ms: number;
  finished_at_ms?: number;
  imported: number;
  skipped: number;
  processed_record_skipped: number;
  failed: number;
  status: string;
  current_device?: string;
  current_file?: string;
  current_file_size?: number;
  current_file_copied_bytes: number;
  total_files_discovered: number;
  processed_files: number;
  message: string;
}

export interface RunAsrTaskResult {
  task: AsrDirectoryTask;
  processed_now: number;
  failed_now: number;
  message: string;
}

export interface ControlAsrTaskResult {
  task: AsrDirectoryTask;
  paused: boolean;
  running: boolean;
  force?: boolean;
  pause_mode?: AsrPauseMode;
  message: string;
}

export interface RetryChunksResult {
  message: string;
  recovered: number;
  still_failed: number;
  still_failed_chunks: AsrFailedChunkRecord[];
  status?: AsrTaskFileStatus;
  daily_documents_refreshed?: string[];
  persist_warnings?: string[];
}

export interface RetryAllFailedChunksResult {
  message: string;
  retry: AsrBulkRetryState;
}

export interface CleanupAsrSourceAudioFailure {
  source_path: string;
  error: string;
}

export interface CleanupAsrSourceAudioResult {
  ok: boolean;
  deleted_files: number;
  deleted_bytes: number;
  skipped_files: number;
  skipped_bytes: number;
  failed_files: CleanupAsrSourceAudioFailure[];
  summary: AsrTaskSummary;
  message: string;
}

export type AsrStreamEvent =
  | AsrProgressEvent
  | AsrTextEvent
  | AsrSegmentEvent
  | AsrErrorEvent
  | AsrDoneEvent;

export type VoiceRealtimeEventType =
  | "connected"
  | "source_ready"
  | "asr_partial"
  | "asr_stable_delta"
  | "asr_final_utterance"
  | "worker_idle_unloaded"
  | "error"
  | "done";

export interface VoiceRealtimeEvent {
  type: VoiceRealtimeEventType;
  session_id?: string;
  source?: string;
  text?: string;
  raw_text?: string;
  delta?: string;
  committed?: string;
  window_start_ms?: number;
  window_end_ms?: number;
  window_index?: number;
  utterance_index?: number;
  speaker?: string;
  speaker_display_name?: string;
  speaker_profile_id?: string;
  speaker_confidence?: number;
  candidate_profile_id?: string;
  candidate_display_name?: string;
  candidate_confidence?: number;
  captured_at_ms?: number;
  emitted_at_ms?: number;
  inference_ms?: number;
  message?: string;
  detail?: string;
}

export interface VoiceRealtimeTranscriptSegment {
  id: string;
  start_ms?: number;
  end_ms?: number;
  speaker?: string;
  speaker_display_name?: string;
  speaker_profile_id?: string;
  speaker_confidence?: number;
  candidate_profile_id?: string;
  candidate_display_name?: string;
  candidate_confidence?: number;
  text: string;
  final?: boolean;
}

const ASR_LEGACY_PARAMS_STORAGE_KEYS = [
  "bifrost.asr.connection",
  "bifrost.asr.connection.v2",
  "bifrost.asr.connection.v3",
  "bifrost.asr.workbench.connection.v1",
  "bifrost.asr.model-management.connection.v1",
];
const ASR_PARAMS_STORAGE_KEY = "bifrost.asr.workbench.connection.v2";
const ASR_MODEL_MANAGEMENT_PARAMS_STORAGE_KEY = "bifrost.asr.model-management.connection.v2";
export const ASR_PARAMS_CHANGED_EVENT = "bifrost.asr.params.changed";
export const ASR_STATUS_CHANGED_EVENT = "bifrost.asr.status.changed";
export const DEFAULT_ASR_MODEL = "Qwen3-ASR-0.6B";

function clearLegacyAsrParams(): void {
  ASR_LEGACY_PARAMS_STORAGE_KEYS.forEach((key) => window.localStorage.removeItem(key));
}

export function defaultAsrParams(): Required<
  Pick<AsrConnectionParams, "host" | "language" | "model" | "ownerModule">
> {
  return {
    host: "127.0.0.1",
    language: "chinese",
    model: DEFAULT_ASR_MODEL,
    ownerModule: "speech_workbench",
  };
}

export function defaultVoiceRealtimeParams(): Required<
  Pick<VoiceRealtimeParams, "host" | "language" | "model" | "chunkMs" | "ownerModule">
> {
  const workbenchDefaults = defaultAsrParams();
  return {
    host: workbenchDefaults.host,
    language: workbenchDefaults.language,
    model: workbenchDefaults.model,
    chunkMs: 1000,
    ownerModule: "speech_workbench",
  };
}

export function defaultModelManagementParams(): Required<
  Pick<AsrConnectionParams, "host" | "language" | "model" | "ownerModule">
> {
  return {
    host: "127.0.0.1",
    language: "chinese",
    model: DEFAULT_ASR_MODEL,
    ownerModule: "model_management",
  };
}

export function loadAsrParams(): AsrConnectionParams {
  try {
    const raw = window.localStorage.getItem(ASR_PARAMS_STORAGE_KEY);
    if (!raw) {
      clearLegacyAsrParams();
      return defaultAsrParams();
    }
    return { ...defaultAsrParams(), ...JSON.parse(raw) };
  } catch {
    return defaultAsrParams();
  }
}

export function loadModelManagementParams(): AsrConnectionParams {
  try {
    const raw = window.localStorage.getItem(ASR_MODEL_MANAGEMENT_PARAMS_STORAGE_KEY);
    if (!raw) {
      clearLegacyAsrParams();
      return defaultModelManagementParams();
    }
    return { ...defaultModelManagementParams(), ...JSON.parse(raw) };
  } catch {
    return defaultModelManagementParams();
  }
}

export function loadVoiceRealtimeParams(): VoiceRealtimeParams {
  try {
    const raw = window.localStorage.getItem(ASR_PARAMS_STORAGE_KEY);
    if (!raw) {
      return defaultVoiceRealtimeParams();
    }
    const saved = JSON.parse(raw) as VoiceRealtimeParams;
    return {
      ...defaultVoiceRealtimeParams(),
      host: saved.host || defaultVoiceRealtimeParams().host,
      language: saved.language || defaultVoiceRealtimeParams().language,
      model: saved.model || defaultVoiceRealtimeParams().model,
      chunkMs: saved.chunkMs || defaultVoiceRealtimeParams().chunkMs,
      ownerModule: "speech_workbench",
    };
  } catch {
    return defaultVoiceRealtimeParams();
  }
}

export function saveAsrParams(params: AsrConnectionParams): void {
  window.localStorage.setItem(
    ASR_PARAMS_STORAGE_KEY,
    JSON.stringify({ ...params, ownerModule: "speech_workbench" }),
  );
  window.dispatchEvent(new Event(ASR_PARAMS_CHANGED_EVENT));
}

export function saveModelManagementParams(params: AsrConnectionParams): void {
  window.localStorage.setItem(
    ASR_MODEL_MANAGEMENT_PARAMS_STORAGE_KEY,
    JSON.stringify({ ...params, ownerModule: "model_management" }),
  );
}

export async function getAsrStatus(
  params: AsrConnectionParams = {},
): Promise<AsrStatus> {
  return get<AsrStatus>(`/asr/status?${buildAsrQuery(params)}`);
}

export async function getAsrCapabilities(): Promise<AsrCapabilities> {
  return get<AsrCapabilities>("/asr/capabilities");
}

export async function getAsrDiarizationStatus(
  profile = "sherpa-onnx-balanced",
): Promise<AsrDiarizationStatus> {
  return get<AsrDiarizationStatus>(
    `/asr/diarization/status?profile=${encodeURIComponent(profile)}`,
  );
}

export async function getAsrSpeakerProfiles(): Promise<AsrSpeakerProfilesResponse> {
  return get<AsrSpeakerProfilesResponse>("/asr/speaker-profiles");
}

export async function getVoiceWakeStatus(): Promise<VoiceWakeStatus> {
  return get<VoiceWakeStatus>("/voice/wake/status");
}

export async function getVoiceWakeProfiles(): Promise<VoiceWakeProfilesResponse> {
  return get<VoiceWakeProfilesResponse>("/voice/wake/profiles");
}

export async function createVoiceWakeProfile(
  payload: CreateVoiceWakeProfileRequest,
): Promise<VoiceWakeProfile> {
  return post<VoiceWakeProfile>("/voice/wake/profiles", payload);
}

export async function getVoiceWakeBindings(): Promise<VoiceWakeBindingsResponse> {
  return get<VoiceWakeBindingsResponse>("/voice/wake/bindings");
}

export async function createVoiceWakeBinding(
  payload: CreateVoiceWakeBindingRequest,
): Promise<VoiceWakeBinding> {
  return post<VoiceWakeBinding>("/voice/wake/bindings", payload);
}

export async function triggerVoiceWake(
  phrase: string,
  profileId: string,
  execute = true,
): Promise<VoiceWakeTriggerResponse> {
  return post<VoiceWakeTriggerResponse>("/voice/wake/trigger", {
    phrase,
    profile_id: profileId,
    dry_run: !execute,
  });
}

export async function startVoiceWakeListener(): Promise<{ listener: VoiceWakeListenerStatus }> {
  return post<{ listener: VoiceWakeListenerStatus }>("/voice/wake/listener/start", {});
}

export async function stopVoiceWakeListener(): Promise<{ listener: VoiceWakeListenerStatus }> {
  return post<{ listener: VoiceWakeListenerStatus }>("/voice/wake/listener/stop", {});
}

export async function getVoiceWakeEvents(): Promise<VoiceWakeEventsResponse> {
  return get<VoiceWakeEventsResponse>("/voice/wake/events");
}

export async function initAsrDiarizationProfile(
  profile = "sherpa-onnx-balanced",
): Promise<AsrDiarizationStatus> {
  const response = await fetch(
    buildApiUrl(`/asr/diarization/init-stream?profile=${encodeURIComponent(profile)}`),
    {
      method: "GET",
      headers: buildStreamHeaders(),
    },
  );
  const text = await response.text();
  if (!response.ok) {
    throw new Error(text || `HTTP ${response.status}`);
  }
  const dataLine = text
    .split("\n")
    .filter((line) => line.startsWith("data: "))
    .map((line) => line.slice("data: ".length))
    .at(-1);
  if (!dataLine) {
    return getAsrDiarizationStatus(profile);
  }
  const payload = JSON.parse(dataLine) as { status?: AsrDiarizationStatus };
  if (payload.status) {
    return payload.status;
  }
  return getAsrDiarizationStatus(profile);
}

export async function listAsrSpeakerProfiles(): Promise<AsrSpeakerProfileSummary[]> {
  const response = await get<{ profiles: AsrSpeakerProfileSummary[] }>("/asr/speaker-profiles");
  return response.profiles;
}

export async function deleteAsrSpeakerProfile(profileId: string): Promise<void> {
  const response = await fetch(
    buildApiUrl(`/asr/speaker-profiles/${encodeURIComponent(profileId)}`),
    {
      method: "DELETE",
      headers: {
        Accept: "application/json",
      },
    },
  );
  await readJsonResponse<unknown>(response);
}

export type AsrSpeakerIdentifyResult = {
  matched: boolean;
  profile_id: string | null;
  display_name: string;
  speaker: string;
  confidence: number;
  status?: "matched" | "unmatched" | "insufficient_audio";
  reason?: string | null;
  audio_duration_ms?: number;
  speech_duration_ms?: number;
};

export async function identifyAsrSpeakerVoice(
  pcm16leBase64: string,
): Promise<AsrSpeakerIdentifyResult> {
  const response = await fetch(buildApiUrl("/asr/speaker-profiles/identify"), {
    method: "POST",
    headers: {
      ...buildJsonHeaders(),
      Accept: "application/json",
    },
    body: JSON.stringify({
      pcm16le_base64: pcm16leBase64,
      sample_rate: 16000,
      channels: 1,
    }),
  });
  return readJsonResponse<AsrSpeakerIdentifyResult>(response);
}

export async function createAsrSpeakerEnrollmentSession(
  name: string,
  diarizationProfile = "sherpa-onnx-balanced",
): Promise<AsrSpeakerEnrollmentSession> {
  const response = await fetch(buildApiUrl("/asr/speaker-profiles/enrollment-sessions"), {
    method: "POST",
    headers: {
      ...buildJsonHeaders(),
      Accept: "application/json",
    },
    body: JSON.stringify({ name, diarization_profile: diarizationProfile }),
  });
  const payload = await readJsonResponse<{ session: AsrSpeakerEnrollmentSession }>(response);
  return payload.session;
}

export async function appendAsrSpeakerEnrollmentAudio(
  sessionId: string,
  promptId: string,
  pcm16leBase64: string,
  finalChunk = true,
): Promise<void> {
  const response = await fetch(
    buildApiUrl(
      `/asr/speaker-profiles/enrollment-sessions/${encodeURIComponent(sessionId)}/audio`,
    ),
    {
      method: "POST",
      headers: {
        ...buildJsonHeaders(),
        Accept: "application/json",
      },
      body: JSON.stringify({
        prompt_id: promptId,
        pcm16le_base64: pcm16leBase64,
        sample_rate: 16000,
        channels: 1,
        final_chunk: finalChunk,
      }),
    },
  );
  await readJsonResponse<unknown>(response);
}

export type AsrSpeakerEnrollmentVerifyResult = {
  prompt_id: string;
  transcript: string;
  match_score: number;
  matched: boolean;
};

export async function verifyAsrSpeakerEnrollmentPrompt(
  sessionId: string,
  promptId: string,
  pcm16leBase64: string,
): Promise<AsrSpeakerEnrollmentVerifyResult> {
  const response = await fetch(
    buildApiUrl(
      `/asr/speaker-profiles/enrollment-sessions/${encodeURIComponent(sessionId)}/verify`,
    ),
    {
      method: "POST",
      headers: {
        ...buildJsonHeaders(),
        Accept: "application/json",
      },
      body: JSON.stringify({
        prompt_id: promptId,
        pcm16le_base64: pcm16leBase64,
        sample_rate: 16000,
        channels: 1,
      }),
    },
  );
  return readJsonResponse<AsrSpeakerEnrollmentVerifyResult>(response);
}

export async function finishAsrSpeakerEnrollment(
  sessionId: string,
): Promise<AsrSpeakerEnrollmentResult> {
  const response = await fetch(
    buildApiUrl(
      `/asr/speaker-profiles/enrollment-sessions/${encodeURIComponent(sessionId)}/finish`,
    ),
    {
      method: "POST",
      headers: {
        ...buildJsonHeaders(),
        Accept: "application/json",
      },
      body: "{}",
    },
  );
  return readJsonResponse<AsrSpeakerEnrollmentResult>(response);
}

export async function startAsrService(
  params: AsrConnectionParams,
): Promise<AsrServiceResult> {
  const response = await fetch(buildApiUrl(`/asr/service/start?${buildAsrQuery(params)}`), {
    method: "POST",
    headers: buildStreamHeaders(),
  });
  return readAsrServiceResponse(response);
}

export async function stopAsrService(
  params: AsrConnectionParams,
): Promise<AsrServiceResult> {
  const response = await fetch(buildApiUrl(`/asr/service/stop?${buildAsrQuery(params)}`), {
    method: "POST",
    headers: buildStreamHeaders(),
  });
  return readJsonResponse<AsrServiceResult>(response);
}

export async function listAsrTasks(): Promise<AsrDirectoryTask[]> {
  const response = await get<{ tasks: AsrDirectoryTask[] }>("/asr/tasks");
  return response.tasks;
}

export async function getAsrTask(id: string): Promise<AsrDirectoryTaskDetail> {
  return get<AsrDirectoryTaskDetail>(`/asr/tasks/${encodeURIComponent(id)}`);
}

export async function getAsrTaskFileTimeline(
  taskId: string,
  fileKey: string,
): Promise<AsrTranscriptTimeline> {
  return get<AsrTranscriptTimeline>(
    `/asr/tasks/${encodeURIComponent(taskId)}/files/${encodeURIComponent(fileKey)}/timeline`,
  );
}

export async function getAsrTaskDailyDocument(
  taskId: string,
  date: string,
): Promise<AsrTaskDailyDocumentDetail> {
  return get<AsrTaskDailyDocumentDetail>(
    `/asr/tasks/${encodeURIComponent(taskId)}/daily/${encodeURIComponent(date)}`,
  );
}

export function buildAsrTaskFileSourceUrl(taskId: string, fileKey: string): string {
  return buildApiUrl(
    `/asr/tasks/${encodeURIComponent(taskId)}/files/${encodeURIComponent(fileKey)}/source`,
  );
}

export async function createAsrTask(
  request: CreateAsrTaskRequest,
): Promise<AsrDirectoryTask> {
  const response = await fetch(buildApiUrl("/asr/tasks"), {
    method: "POST",
    headers: {
      ...buildStreamHeaders(),
      "Content-Type": "application/json",
    },
    body: JSON.stringify(request),
  });
  return readJsonResponse<AsrDirectoryTask>(response);
}

export async function updateAsrTask(
  id: string,
  request: UpdateAsrTaskRequest,
): Promise<AsrDirectoryTask> {
  const response = await fetch(buildApiUrl(`/asr/tasks/${encodeURIComponent(id)}`), {
    method: "PATCH",
    headers: {
      ...buildStreamHeaders(),
      "Content-Type": "application/json",
    },
    body: JSON.stringify(request),
  });
  return readJsonResponse<AsrDirectoryTask>(response);
}

export async function listAsrExternalVolumes(): Promise<AsrExternalVolume[]> {
  const response = await get<{ volumes: AsrExternalVolume[] }>("/asr/external-volumes");
  return response.volumes;
}

export async function getAsrExternalImportStatus(
  taskId: string,
): Promise<AsrExternalImportStatus> {
  return get<AsrExternalImportStatus>(
    `/asr/tasks/${encodeURIComponent(taskId)}/external-import`,
  );
}

export async function runAsrExternalImport(
  taskId: string,
): Promise<{
  imported: number;
  message: string;
  task: AsrDirectoryTask;
  running?: boolean;
  progress?: AsrExternalImportRunProgress;
}> {
  const response = await fetch(
    buildApiUrl(`/asr/tasks/${encodeURIComponent(taskId)}/external-import/run`),
    {
      method: "POST",
      headers: buildStreamHeaders(),
    },
  );
  return readJsonResponse<{
    imported: number;
    message: string;
    task: AsrDirectoryTask;
    running?: boolean;
    progress?: AsrExternalImportRunProgress;
  }>(response);
}

export async function runAsrTask(id: string): Promise<RunAsrTaskResult> {
  const response = await fetch(buildApiUrl(`/asr/tasks/${encodeURIComponent(id)}/run`), {
    method: "POST",
    headers: buildStreamHeaders(),
  });
  return readJsonResponse<RunAsrTaskResult>(response);
}

export async function pauseAsrTask(
  id: string,
  options?: { force?: boolean; mode?: AsrPauseMode },
): Promise<ControlAsrTaskResult> {
  const query = new URLSearchParams();
  if (options?.force) {
    query.set("force", "true");
  }
  if (options?.mode) {
    query.set("mode", options.mode);
  }
  const queryString = query.toString() ? `?${query.toString()}` : "";
  const response = await fetch(
    buildApiUrl(`/asr/tasks/${encodeURIComponent(id)}/pause${queryString}`),
    {
      method: "POST",
      headers: buildStreamHeaders(),
    },
  );
  return readJsonResponse<ControlAsrTaskResult>(response);
}

export async function resumeAsrTask(id: string): Promise<ControlAsrTaskResult> {
  const response = await fetch(buildApiUrl(`/asr/tasks/${encodeURIComponent(id)}/resume`), {
    method: "POST",
    headers: buildStreamHeaders(),
  });
  return readJsonResponse<ControlAsrTaskResult>(response);
}

export async function deleteAsrTask(id: string, confirmName: string): Promise<void> {
  const query = new URLSearchParams({ confirm_name: confirmName });
  const response = await fetch(buildApiUrl(`/asr/tasks/${encodeURIComponent(id)}?${query}`), {
    method: "DELETE",
    headers: buildStreamHeaders(),
  });
  await readJsonResponse<{ ok: boolean }>(response);
}

export async function retryFailedChunks(
  taskId: string,
  fileKey: string,
): Promise<RetryChunksResult> {
  const response = await fetch(
    buildApiUrl(
      `/asr/tasks/${encodeURIComponent(taskId)}/files/${encodeURIComponent(fileKey)}/retry-chunks`,
    ),
    {
      method: "POST",
      headers: buildStreamHeaders(),
    },
  );
  return readJsonResponse<RetryChunksResult>(response);
}

export async function retryAllFailedChunks(
  taskId: string,
): Promise<RetryAllFailedChunksResult> {
  const response = await fetch(
    buildApiUrl(`/asr/tasks/${encodeURIComponent(taskId)}/retry-failed-chunks`),
    {
      method: "POST",
      headers: buildStreamHeaders(),
    },
  );
  return readJsonResponse<RetryAllFailedChunksResult>(response);
}

export async function cleanupAsrSourceAudio(
  taskId: string,
): Promise<CleanupAsrSourceAudioResult> {
  const response = await fetch(
    buildApiUrl(`/asr/tasks/${encodeURIComponent(taskId)}/cleanup-source-audio`),
    {
      method: "POST",
      headers: buildStreamHeaders(),
    },
  );
  return readJsonResponse<CleanupAsrSourceAudioResult>(response);
}

export async function streamAsrInitialization(
  params: AsrConnectionParams,
  onEvent: (event: AsrStreamEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const response = await fetch(buildApiUrl(`/asr/init-stream?${buildAsrQuery(params)}`), {
    method: "GET",
    headers: buildStreamHeaders(),
    signal,
  });
  await readSseResponse(response, onEvent);
}

export async function streamAsrTranscription(
  file: Blob,
  fileName: string,
  params: AsrConnectionParams,
  onEvent: (event: AsrStreamEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const form = new FormData();
  form.append("file", file, fileName);
  form.append("language", params.language || defaultAsrParams().language);
  form.append("response_format", "text");

  const response = await fetch(
    buildApiUrl(`/asr/transcribe-stream?${buildAsrQuery(params)}`),
    {
      method: "POST",
      headers: buildStreamHeaders(),
      body: form,
      signal,
    },
  );
  await readSseResponse(response, onEvent);
}

export function getSpeechPipelinesStatus(): Promise<SpeechPipelinesStatus> {
  return get<SpeechPipelinesStatus>("/speech/pipelines/status");
}

export async function createAsrOfflineJob(
  file: Blob,
  fileName: string,
  params: AsrConnectionParams,
  options: AsrOfflineJobCreateOptions = {},
): Promise<AsrOfflineJob> {
  const form = new FormData();
  form.append("file", file, fileName);
  const defaults = defaultAsrParams();
  const query = new URLSearchParams();
  query.set("host", params.host || defaults.host);
  if (params.port) {
    query.set("port", String(params.port));
  }
  query.set("language", options.language || params.language || defaults.language);
  query.set("model", options.model || params.model || defaults.model);
  query.set(
    "pipeline_profile",
    options.pipelineProfile || "offline-speaker-subtitle-local",
  );
  query.set("speaker_aware", options.speakerAware === false ? "0" : "1");
  const response = await fetch(buildApiUrl(`/asr/offline-jobs?${query.toString()}`), {
    method: "POST",
    headers: {
      ...buildStreamHeaders(),
      Accept: "application/json",
    },
    body: form,
  });
  return readJsonResponse<AsrOfflineJob>(response);
}

export function getAsrOfflineJob(jobId: string): Promise<AsrOfflineJob> {
  return get<AsrOfflineJob>(`/asr/offline-jobs/${encodeURIComponent(jobId)}`);
}

export async function getAsrOfflineJobArtifact(
  jobId: string,
  format: string,
): Promise<string> {
  const response = await fetch(
    buildApiUrl(
      `/asr/offline-jobs/${encodeURIComponent(jobId)}/artifacts/${encodeURIComponent(format)}`,
    ),
    {
      headers: buildStreamHeaders(),
    },
  );
  if (!response.ok) {
    const body = await response.text().catch(() => "");
    throw new Error(body || `ASR artifact request failed with status ${response.status}`);
  }
  return response.text();
}

export function buildVoiceRealtimeUrl(params: VoiceRealtimeParams): string {
  const defaults = defaultVoiceRealtimeParams();
  const query = new URLSearchParams();
  query.set("source", "web_mic");
  query.set("provider", "qwen3_stateful_streaming");
  query.set("host", params.host || defaults.host);
  if (params.port) {
    query.set("port", String(params.port));
  }
  query.set("language", params.language || defaults.language);
  const model = params.model || defaults.model;
  query.set("model", model);
  query.set("owner_module", params.ownerModule || defaults.ownerModule);
  if (params.ownerId) {
    query.set("owner_id", params.ownerId);
  }
  query.set("chunk_ms", String(params.chunkMs || defaults.chunkMs));
  if (model === "Qwen3-ASR-1.7B") {
    query.set("allow_stateful_17b", "1");
  }
  const token = getAdminToken();
  if (token) {
    query.set("token", token);
  }
  return buildWsUrl("/api/voice/listen-ws", query);
}

function buildAsrQuery(params: AsrConnectionParams): string {
  const defaults = defaultAsrParams();
  const query = new URLSearchParams();
  query.set("host", params.host || defaults.host);
  if (params.port) {
    query.set("port", String(params.port));
  }
  query.set("language", params.language || defaults.language);
  query.set("model", params.model || defaults.model);
  query.set("owner_module", params.ownerModule || defaults.ownerModule);
  if (params.ownerId) {
    query.set("owner_id", params.ownerId);
  }
  return query.toString();
}

function buildStreamHeaders(): HeadersInit {
  const headers: Record<string, string> = {
    Accept: "text/event-stream",
    "X-Client-Id": getClientId(),
  };
  const token = getAdminToken();
  if (token) {
    headers.Authorization = `Bearer ${token}`;
  }
  return headers;
}

function buildJsonHeaders(): HeadersInit {
  return {
    ...buildStreamHeaders(),
    Accept: "application/json",
    "Content-Type": "application/json",
  };
}

async function readSseResponse(
  response: Response,
  onEvent: (event: AsrStreamEvent) => void,
): Promise<void> {
  if (!response.ok) {
    const body = await response.text().catch(() => "");
    throw new Error(body || `ASR stream failed with status ${response.status}`);
  }
  if (!response.body) {
    throw new Error("ASR stream response did not include a body");
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";

  while (true) {
    const { value, done } = await reader.read();
    if (done) {
      break;
    }
    buffer += decoder.decode(value, { stream: true });
    const parts = buffer.split("\n\n");
    buffer = parts.pop() || "";
    parts.forEach((part) => emitSsePart(part, onEvent));
  }

  buffer += decoder.decode();
  if (buffer.trim()) {
    emitSsePart(buffer, onEvent);
  }
}

async function readJsonResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    const body = await response.text().catch(() => "");
    throw new Error(body || `ASR request failed with status ${response.status}`);
  }
  return response.json() as Promise<T>;
}

async function readAsrServiceResponse(response: Response): Promise<AsrServiceResult> {
  if (response.ok || response.status === 409) {
    return response.json() as Promise<AsrServiceResult>;
  }
  return readJsonResponse<AsrServiceResult>(response);
}

function emitSsePart(
  part: string,
  onEvent: (event: AsrStreamEvent) => void,
): void {
  const lines = part.split(/\r?\n/);
  let eventName = "message";
  const dataLines: string[] = [];

  lines.forEach((line) => {
    if (line.startsWith("event:")) {
      eventName = line.slice("event:".length).trim();
    } else if (line.startsWith("data:")) {
      dataLines.push(line.slice("data:".length).trimStart());
    }
  });

  const dataText = dataLines.join("\n");
  const payload = dataText ? JSON.parse(dataText) : {};

  if (
    eventName === "progress" ||
    eventName === "connected" ||
    eventName === "stream" ||
    eventName === "finish"
  ) {
    onEvent({ type: "progress", ...payload });
  } else if (eventName === "text") {
    onEvent({ type: "text", ...payload });
  } else if (eventName === "partial" || eventName === "final") {
    onEvent({ type: eventName, ...payload });
  } else if (eventName === "error") {
    onEvent({ type: "error", ...payload });
  } else if (eventName === "done") {
    onEvent({ type: "done", ...payload });
  }
}

// ─── Daily Agent Types ────────────────────────────────────────────────────────

export interface AsrDailyAgentConfig {
  enabled: boolean;
  runner: string;
  timeout_ms: number;
  trigger_policy: "after_asr_run" | "manual_only";
  session_key?: string;
  instructions_source: "default" | "custom";
  im_delivery: AsrDailyAgentImDeliveryConfig;
  report_sync_dir?: string;
  last_report_sync?: AsrDailyAgentReportSyncResult;
  last_run_at_ms?: number;
  last_status?: string;
  last_error?: string;
  last_run_id?: string;
}

export interface AsrDailyAgentImDeliveryConfig {
  enabled: boolean;
  channel?: string;
  mode: "full_report" | "summary";
  send_policy: "on_success_with_report" | "on_success" | "always";
  last_sent_at_ms?: number;
  last_send_error?: string;
}

export interface AsrDailyAgentReportSyncResult {
  target_dir: string;
  total_files: number;
  copied_files: number;
  skipped_files: number;
  failed_files: number;
  synced_at_ms: number;
  errors?: string[];
}

export interface AsrDailyAgentWorkspaceStatus {
  daily_dir: string;
  report_dir: string;
  agents_path: string;
  agents_exists: boolean;
  git_available: boolean;
  git_initialized: boolean;
  git_error?: string;
  report_count: number;
}

export interface AsrDailyAgentReportIndexStatus {
  report_files: number;
  processed_documents: number;
  indexed_reports: number;
  unindexed_reports: number;
  processed_missing_report: number;
  unindexed_dates: string[];
}

export interface AsrDailyAgentConfigResponse {
  task_id: string;
  config: AsrDailyAgentConfig;
  workspace?: AsrDailyAgentWorkspaceStatus;
  report_index_status?: AsrDailyAgentReportIndexStatus;
  last_run: {
    run_id?: string;
    status?: string;
    error?: string;
    last_run_at_ms?: number;
  };
}

export interface AsrDailyAgentInstructionsResponse {
  task_id: string;
  content: string;
  source: "file" | "default";
}

export interface AsrDailyAgentProcessedDocument {
  date: string;
  source_sha256: string;
  source_len_bytes: number;
  processed_at_ms: number;
  runner: string;
  report_path?: string;
  last_run_id: string;
}

export interface AsrDailyAgentRunsResponse {
  task_id: string;
  processed_documents: AsrDailyAgentProcessedDocument[];
}

export interface AsrDailyAgentReportDetail {
  task_id: string;
  task_name: string;
  date: string;
  path: string;
  size?: number;
  modified_ms?: number;
  content: string;
  processed_at_ms?: number;
  runner?: string;
  last_run_id?: string;
}

// ─── Daily Agent API Functions ────────────────────────────────────────────────

export async function getDailyAgentConfig(
  taskId: string,
): Promise<AsrDailyAgentConfigResponse> {
  const url = buildApiUrl(`/asr/tasks/${taskId}/daily-agent`);
  const response = await fetch(url, {
    headers: { Authorization: `Bearer ${getAdminToken()}` },
  });
  return readJsonResponse(response);
}

export async function updateDailyAgentConfig(
  taskId: string,
  config: Partial<AsrDailyAgentConfig>,
): Promise<{ ok: boolean; config: AsrDailyAgentConfig }> {
  const url = buildApiUrl(`/asr/tasks/${taskId}/daily-agent`);
  const response = await fetch(url, {
    method: "PUT",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${getAdminToken()}`,
    },
    body: JSON.stringify(config),
  });
  return readJsonResponse(response);
}

export async function getDailyAgentInstructions(
  taskId: string,
): Promise<AsrDailyAgentInstructionsResponse> {
  const url = buildApiUrl(`/asr/tasks/${taskId}/daily-agent/agents`);
  const response = await fetch(url, {
    headers: { Authorization: `Bearer ${getAdminToken()}` },
  });
  return readJsonResponse(response);
}

export async function updateDailyAgentInstructions(
  taskId: string,
  content: string,
): Promise<{ ok: boolean }> {
  const url = buildApiUrl(`/asr/tasks/${taskId}/daily-agent/agents`);
  const response = await fetch(url, {
    method: "PUT",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${getAdminToken()}`,
    },
    body: JSON.stringify({ content }),
  });
  return readJsonResponse(response);
}

export async function triggerDailyAgentRun(
  taskId: string,
  options?: { force?: boolean; date?: string },
): Promise<{ status: string; message: string }> {
  const params = new URLSearchParams();
  if (options?.force) params.set("force", "1");
  if (options?.date) params.set("date", options.date);
  const query = params.toString() ? `?${params.toString()}` : "";
  const url = buildApiUrl(`/asr/tasks/${taskId}/daily-agent/run${query}`);
  const response = await fetch(url, {
    method: "POST",
    headers: { Authorization: `Bearer ${getAdminToken()}` },
  });
  return readJsonResponse(response);
}

export async function sendDailyAgentReport(
  taskId: string,
): Promise<{ ok: boolean; sent_reports: string[] }> {
  const url = buildApiUrl(`/asr/tasks/${taskId}/daily-agent/send`);
  const response = await fetch(url, {
    method: "POST",
    headers: { Authorization: `Bearer ${getAdminToken()}` },
  });
  return readJsonResponse(response);
}

export async function syncDailyAgentReports(
  taskId: string,
): Promise<{ ok: boolean; sync: AsrDailyAgentReportSyncResult }> {
  const url = buildApiUrl(`/asr/tasks/${taskId}/daily-agent/sync`);
  const response = await fetch(url, {
    method: "POST",
    headers: { Authorization: `Bearer ${getAdminToken()}` },
  });
  return readJsonResponse(response);
}

export async function getDailyAgentRuns(
  taskId: string,
): Promise<AsrDailyAgentRunsResponse> {
  const url = buildApiUrl(`/asr/tasks/${taskId}/daily-agent/runs`);
  const response = await fetch(url, {
    headers: { Authorization: `Bearer ${getAdminToken()}` },
  });
  return readJsonResponse(response);
}

export async function getDailyAgentReport(
  taskId: string,
  date: string,
): Promise<AsrDailyAgentReportDetail> {
  const url = buildApiUrl(
    `/asr/tasks/${encodeURIComponent(taskId)}/daily-agent/reports/${encodeURIComponent(date)}`,
  );
  const response = await fetch(url, {
    headers: { Authorization: `Bearer ${getAdminToken()}` },
  });
  return readJsonResponse(response);
}
