import { get, post } from "./client";

export interface BreakpointSettings {
  enabled: boolean;
  max_body_bytes: number;
}

export interface BreakpointEdit {
  method?: string;
  url?: string;
  status?: number;
  headers?: [string, string][];
  body?: string;
}

export interface PendingBreakpoint {
  request_id: string;
  phase: "request" | "response";
  method?: string;
  url?: string;
  status?: number;
  headers: [string, string][];
  body?: string;
  body_omitted: boolean;
  body_size?: number;
  max_body_bytes: number;
  content_encoding?: string;
  paused_at_ms: number;
  deadline_at_ms: number;
}

export interface BreakpointResumeRequest {
  request_id: string;
  phase: "request" | "response";
  method?: string;
  url?: string;
  status?: number;
  headers?: [string, string][];
  body?: string;
}

export interface BreakpointResumeResponse {
  resumed: boolean;
  request_id: string;
  phase: "request" | "response";
}

export async function getBreakpointSettings(): Promise<BreakpointSettings> {
  return get<BreakpointSettings>("/breakpoint/settings");
}

export async function getPendingBreakpoints(): Promise<PendingBreakpoint[]> {
  return get<PendingBreakpoint[]>("/breakpoint/pending");
}

export async function updateBreakpointSettings(
  settings: BreakpointSettings,
): Promise<BreakpointSettings> {
  return post<BreakpointSettings>("/breakpoint/settings", settings);
}

export async function resumeBreakpoint(
  request: BreakpointResumeRequest,
): Promise<BreakpointResumeResponse> {
  return post<BreakpointResumeResponse>("/breakpoint/resume", request);
}
