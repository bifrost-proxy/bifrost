import { buildApiUrl, buildWsUrl } from "../runtime";
import { getClientId } from "../services/clientId";
import { getAdminToken } from "../services/adminAuth";
import { get } from "./client";

export interface AsrConnectionParams {
  host?: string;
  port?: number;
  language?: string;
  model?: string;
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
  message: string;
}

export interface AsrServiceResult {
  ready: boolean;
  managed: boolean;
  server_url: string;
  message: string;
  detail?: string;
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
  text: string;
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

const ASR_LEGACY_PARAMS_STORAGE_KEYS = [
  "bifrost.asr.connection",
  "bifrost.asr.connection.v2",
];
const ASR_PARAMS_STORAGE_KEY = "bifrost.asr.connection.v3";
export const ASR_PARAMS_CHANGED_EVENT = "bifrost.asr.params.changed";
export const ASR_STATUS_CHANGED_EVENT = "bifrost.asr.status.changed";

export function defaultAsrParams(): Required<
  Pick<AsrConnectionParams, "host" | "language" | "model">
> {
  return {
    host: "127.0.0.1",
    language: "chinese",
    model: "Qwen3-ASR-1.7B",
  };
}

export function loadAsrParams(): AsrConnectionParams {
  try {
    const raw = window.localStorage.getItem(ASR_PARAMS_STORAGE_KEY);
    if (!raw) {
      ASR_LEGACY_PARAMS_STORAGE_KEYS.forEach((key) => window.localStorage.removeItem(key));
      return defaultAsrParams();
    }
    return { ...defaultAsrParams(), ...JSON.parse(raw) };
  } catch {
    return defaultAsrParams();
  }
}

export function saveAsrParams(params: AsrConnectionParams): void {
  window.localStorage.setItem(ASR_PARAMS_STORAGE_KEY, JSON.stringify(params));
  window.dispatchEvent(new Event(ASR_PARAMS_CHANGED_EVENT));
}

export async function getAsrStatus(
  params: AsrConnectionParams = {},
): Promise<AsrStatus> {
  return get<AsrStatus>(`/asr/status?${buildAsrQuery(params)}`);
}

export async function startAsrService(
  params: AsrConnectionParams,
): Promise<AsrServiceResult> {
  const response = await fetch(buildApiUrl(`/asr/service/start?${buildAsrQuery(params)}`), {
    method: "POST",
    headers: buildStreamHeaders(),
  });
  return readJsonResponse<AsrServiceResult>(response);
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
): Promise<{ imported: number; message: string; task: AsrDirectoryTask }> {
  const response = await fetch(
    buildApiUrl(`/asr/tasks/${encodeURIComponent(taskId)}/external-import/run`),
    {
      method: "POST",
      headers: buildStreamHeaders(),
    },
  );
  return readJsonResponse<{ imported: number; message: string; task: AsrDirectoryTask }>(
    response,
  );
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
  options?: { force?: boolean },
): Promise<ControlAsrTaskResult> {
  const query = options?.force ? "?force=true" : "";
  const response = await fetch(buildApiUrl(`/asr/tasks/${encodeURIComponent(id)}/pause${query}`), {
    method: "POST",
    headers: buildStreamHeaders(),
  });
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

export function buildAsrRealtimeUrl(params: AsrConnectionParams): string {
  const query = new URLSearchParams(buildAsrQuery(params));
  const token = getAdminToken();
  if (token) {
    query.set("token", token);
  }
  return buildWsUrl("/api/asr/transcribe-ws", query);
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
