import { get, post } from "./client";

export interface BreakpointSettings {
  enabled: boolean;
  hook_request: boolean;
  hook_response: boolean;
}

export interface BreakpointEdit {
  headers: [string, string][];
  body?: string;
}

export interface BreakpointResumeRequest {
  request_id: string;
  phase: string;
  headers: [string, string][];
  body?: string;
}

export interface BreakpointResumeResponse {
  resumed: boolean;
  request_id: string;
}

export async function getBreakpointSettings(): Promise<BreakpointSettings> {
  return get<BreakpointSettings>("/breakpoint/settings");
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
