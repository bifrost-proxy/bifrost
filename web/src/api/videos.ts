import { apiFetch } from "./apiFetch";
import { buildBackendUrl } from "../runtime";

export type VideoDownloadStatus = "queued" | "running" | "completed" | "failed";

export interface VideoDownloadTask {
  id: string;
  url: string;
  download_dir: string;
  status: VideoDownloadStatus;
  progress_percent?: number | null;
  downloaded?: string | null;
  total?: string | null;
  speed?: string | null;
  eta?: string | null;
  file_path?: string | null;
  message?: string | null;
  error?: string | null;
  created_at_ms: number;
  updated_at_ms: number;
}

export interface VideoDownloadsResponse {
  downloads: VideoDownloadTask[];
}

export interface VideoDefaultsResponse {
  download_dir: string;
}

async function readError(resp: Response, fallback: string): Promise<Error> {
  const text = await resp.text().catch(() => "");
  try {
    const parsed = JSON.parse(text) as { error?: string; message?: string };
    return new Error(parsed.error || parsed.message || fallback);
  } catch {
    return new Error(text || fallback);
  }
}

export async function getVideoDefaults(): Promise<VideoDefaultsResponse> {
  const resp = await apiFetch("/api/videos/defaults", { method: "GET" });
  if (!resp.ok) {
    throw await readError(resp, `Failed to load video defaults: ${resp.status}`);
  }
  return (await resp.json()) as VideoDefaultsResponse;
}

export async function listVideoDownloads(): Promise<VideoDownloadsResponse> {
  const resp = await apiFetch("/api/videos/downloads", { method: "GET" });
  if (!resp.ok) {
    throw await readError(resp, `Failed to list video downloads: ${resp.status}`);
  }
  return (await resp.json()) as VideoDownloadsResponse;
}

export async function createVideoDownload(input: {
  url: string;
  download_dir?: string;
}): Promise<VideoDownloadTask> {
  const resp = await apiFetch("/api/videos/downloads", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!resp.ok) {
    throw await readError(resp, `Failed to start video download: ${resp.status}`);
  }
  return (await resp.json()) as VideoDownloadTask;
}

export function videoFileUrl(id: string): string {
  return buildBackendUrl(`/api/videos/downloads/${encodeURIComponent(id)}/file`);
}

export async function openVideoFile(id: string): Promise<void> {
  const resp = await apiFetch(`/api/videos/downloads/${encodeURIComponent(id)}/open`, {
    method: "POST",
  });
  if (!resp.ok) {
    throw await readError(resp, `Failed to open video file: ${resp.status}`);
  }
}

export async function revealVideoFile(id: string): Promise<void> {
  const resp = await apiFetch(`/api/videos/downloads/${encodeURIComponent(id)}/reveal`, {
    method: "POST",
  });
  if (!resp.ok) {
    throw await readError(resp, `Failed to reveal video file: ${resp.status}`);
  }
}

export async function retryVideoDownload(id: string): Promise<VideoDownloadTask> {
  const resp = await apiFetch(`/api/videos/downloads/${encodeURIComponent(id)}/retry`, {
    method: "POST",
  });
  if (!resp.ok) {
    throw await readError(resp, `Failed to retry video download: ${resp.status}`);
  }
  return (await resp.json()) as VideoDownloadTask;
}
