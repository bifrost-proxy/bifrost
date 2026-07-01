import type {
  AsrDailyAgentConfig,
  AsrTaskFileRecord,
  AsrTaskSchedule,
  VoiceRealtimeEvent,
  VoiceRealtimeTranscriptSegment,
} from "../../api/asr";

export type WorkState = "idle" | "recording" | "transcribing" | "error";

export const VOICE_REALTIME_SAMPLE_RATE = 16_000;
export const VOICE_REALTIME_CHUNK_MS = 1000;
export const MIC_METER_BARS = 40;
export const EMPTY_MIC_LEVELS = Array.from({ length: MIC_METER_BARS }, () => 0);

export function hasRunningDailyAgent(
  dailyAgent?: Pick<AsrDailyAgentConfig, "last_status" | "agents"> | null,
  agentId?: string | null,
): boolean {
  if (!dailyAgent) return false;
  if (agentId) {
    const agent = dailyAgent.agents?.find((item) => item.id === agentId);
    if (agent) {
      return agent.last_status === "running";
    }
  }
  return (
    dailyAgent.last_status === "running" ||
    Boolean(dailyAgent.agents?.some((agent) => agent.last_status === "running"))
  );
}

export function resampleFloat32Linear(
  samples: Float32Array,
  sourceSampleRate: number,
  targetSampleRate = VOICE_REALTIME_SAMPLE_RATE,
): Float32Array {
  if (
    !Number.isFinite(sourceSampleRate) ||
    sourceSampleRate <= 0 ||
    Math.round(sourceSampleRate) === targetSampleRate
  ) {
    return samples;
  }
  if (samples.length === 0) {
    return samples;
  }
  const outputLength = Math.max(1, Math.round((samples.length * targetSampleRate) / sourceSampleRate));
  const output = new Float32Array(outputLength);
  const scale = sourceSampleRate / targetSampleRate;
  for (let index = 0; index < outputLength; index += 1) {
    const position = index * scale;
    const left = Math.min(samples.length - 1, Math.floor(position));
    const right = Math.min(samples.length - 1, left + 1);
    const ratio = position - left;
    output[index] = samples[left] + (samples[right] - samples[left]) * ratio;
  }
  return output;
}

export function encodePcm16Chunk(
  samples: Float32Array,
  sourceSampleRate = VOICE_REALTIME_SAMPLE_RATE,
): ArrayBuffer {
  const normalized = resampleFloat32Linear(
    samples,
    sourceSampleRate,
    VOICE_REALTIME_SAMPLE_RATE,
  );
  const buffer = new ArrayBuffer(normalized.length * 2);
  const view = new DataView(buffer);
  for (let index = 0; index < normalized.length; index += 1) {
    const clamped = Math.max(-1, Math.min(1, normalized[index]));
    const value = clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff;
    view.setInt16(index * 2, value, true);
  }
  return buffer;
}

export function buildTaskSchedule(values: Record<string, unknown>): AsrTaskSchedule {
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

export function formatSchedule(schedule?: AsrTaskSchedule): string {
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

export function fileStatusColor(status: AsrTaskFileRecord["status"]): string {
  if (status === "success") {
    return "success";
  }
  if (status === "partial_success") {
    return "warning";
  }
  if (status === "failed") {
    return "error";
  }
  if (status === "processing") {
    return "processing";
  }
  return "default";
}

export function fileStatusLabel(status: AsrTaskFileRecord["status"]): string {
  if (status === "partial_success") {
    return "partial success";
  }
  return status;
}

export function formatTime(value?: number): string {
  if (!value) {
    return "-";
  }
  return new Date(value).toLocaleString();
}

export function formatDuration(value?: number): string {
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

export function dedupeTranscript(committed: string, candidate: string): string {
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

export function appendTranscriptDelta(committed: string, delta: string): string {
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

export function voiceRealtimeSegmentFromEvent(
  event: VoiceRealtimeEvent,
  text: string,
  final: boolean,
): VoiceRealtimeTranscriptSegment | null {
  const cleanText = text.trim();
  if (!cleanText) {
    return null;
  }
  const index = event.utterance_index ?? event.window_index ?? Date.now();
  return {
    id: `${event.session_id || "voice"}-${index}-${final ? "final" : "partial"}`,
    start_ms: event.window_start_ms,
    end_ms: event.window_end_ms,
    speaker: event.speaker,
    speaker_display_name: event.speaker_display_name,
    speaker_profile_id: event.speaker_profile_id,
    speaker_confidence: event.speaker_confidence,
    candidate_profile_id: event.candidate_profile_id,
    candidate_display_name: event.candidate_display_name,
    candidate_confidence: event.candidate_confidence,
    text: cleanText,
    final,
  };
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

function padTime(value: number): string {
  return String(value).padStart(2, "0");
}

function weekdayLabel(value: number): string {
  return ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][value - 1] ?? "Mon";
}
