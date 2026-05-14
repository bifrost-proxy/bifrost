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
  deleted_after_processing: number;
  running: boolean;
}

export type AsrTaskFileStatus = "pending" | "processing" | "success" | "failed";

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
  started_at_ms?: number;
  finished_at_ms?: number;
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
  schedule: AsrTaskSchedule;
  language: string;
  model: string;
  created_at_ms: number;
  updated_at_ms: number;
  last_run_at_ms?: number;
  next_run_at_ms?: number;
  last_error?: string;
  summary: AsrTaskSummary;
}

export interface AsrDirectoryTaskDetail extends AsrDirectoryTask {
  files: AsrTaskFileRecord[];
}

export interface CreateAsrTaskRequest {
  name?: string;
  audio_dir: string;
  recursive?: boolean;
  enabled?: boolean;
  schedule?: AsrTaskSchedule;
  language?: string;
  model?: string;
}

export interface RunAsrTaskResult {
  task: AsrDirectoryTask;
  processed_now: number;
  failed_now: number;
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

export async function runAsrTask(id: string): Promise<RunAsrTaskResult> {
  const response = await fetch(buildApiUrl(`/asr/tasks/${encodeURIComponent(id)}/run`), {
    method: "POST",
    headers: buildStreamHeaders(),
  });
  return readJsonResponse<RunAsrTaskResult>(response);
}

export async function deleteAsrTask(id: string): Promise<void> {
  const response = await fetch(buildApiUrl(`/asr/tasks/${encodeURIComponent(id)}`), {
    method: "DELETE",
    headers: buildStreamHeaders(),
  });
  await readJsonResponse<{ ok: boolean }>(response);
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
