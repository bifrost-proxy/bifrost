import { get, post } from './client';

export interface DebugPage {
  page_id: string;
  title?: string | null;
  url: string;
  origin: string;
  user_agent?: string | null;
  adapter: 'page_bridge';
  fidelity: 'fallback';
  state: 'candidate' | 'discoverable' | 'fallback_attached' | 'stale' | 'denied';
  mode: 'read' | 'control';
  last_seen_at_ms: number;
  capabilities: Record<string, string>;
  status_reason?: string | null;
}

export interface DebugSession {
  session_id: string;
  page_id: string;
  adapter: 'page_bridge';
  mode: 'read' | 'control';
  state: string;
}

export interface DevtoolsSnapshot {
  page: DebugPage;
  console: Array<{ level: string; text: string; at_ms: number }>;
  dom_snapshot?: string | null;
  network: unknown[];
}

export interface DevtoolsFrontendStatus {
  state: 'not_installed' | 'installed' | 'broken';
  version: string;
  source: 'npm_on_demand_cache';
  installed: boolean;
  installPath: string;
  inspectorPath: string;
  downloadUrl: string;
  totalSizeBytes?: number | null;
  reason?: string | null;
}

export interface SystemDevtoolsOpenResult {
  opened: boolean;
  url: string;
  command: string;
}

export async function listDevtoolsPages(online = true): Promise<DebugPage[]> {
  const res = await get<{ pages: DebugPage[] }>(`/devtools/pages?online=${online}`);
  return res.pages;
}

export async function openDevtoolsSession(pageId: string): Promise<DebugSession> {
  return post<DebugSession>('/devtools/sessions', { page_id: pageId });
}

export async function getDevtoolsSnapshot(sessionId: string): Promise<DevtoolsSnapshot> {
  return get<DevtoolsSnapshot>(`/devtools/sessions/${sessionId}/snapshot`);
}

export async function sendDevtoolsCommand(
  sessionId: string,
  command: string,
  params: unknown = {},
): Promise<unknown> {
  return post(`/devtools/sessions/${sessionId}/commands`, { command, params });
}

export async function getDevtoolsFrontendStatus(): Promise<DevtoolsFrontendStatus> {
  return get<DevtoolsFrontendStatus>('/devtools/frontend/status');
}

export async function installDevtoolsFrontend(): Promise<DevtoolsFrontendStatus> {
  return post<DevtoolsFrontendStatus>('/devtools/frontend/install');
}

export async function openSystemDevtoolsFrontend(
  pageId: string,
): Promise<SystemDevtoolsOpenResult> {
  return post<SystemDevtoolsOpenResult>(`/devtools/cdp/open/${pageId}`);
}
