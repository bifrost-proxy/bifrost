import { get, post } from './client';
import { buildWsUrl } from '../runtime';

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
  matched_rule?: {
    pattern: string;
    raw?: string | null;
    line?: number | null;
    evaluate_allowlist?: string[];
  } | null;
  traffic_ids?: string[];
  evaluate_allowlist?: string[];
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
  console: DebugConsoleMessage[];
  dom_snapshot?: string | null;
  dom_tree?: DebugDomNode | null;
  network: DebugNetworkEvent[];
  storage?: DebugStorageSnapshot | null;
}

export type DevtoolsLiveMessage =
  | { type: 'snapshot'; snapshot: DevtoolsSnapshot }
  | { type: 'console'; message: DebugConsoleMessage }
  | { type: 'network'; event: DebugNetworkEvent }
  | { type: 'disconnected'; page_id?: string; reason: string };

export interface DebugConsoleMessage {
  level: string;
  text: string;
  at_ms: number;
  args?: DebugConsoleValue[];
  raw?: string | null;
}

export interface DebugConsoleValue {
  type: string;
  subtype?: string;
  preview?: string;
  raw?: string;
  value?: unknown;
  properties?: Array<{ name: string; value: DebugConsoleValue }>;
  overflow?: number;
}

export interface DebugNetworkEvent {
  url: string;
  method: string;
  status?: number | null;
  resource_type: string;
  at_ms: number;
  client_req_id?: string | null;
  traffic_id?: string | null;
}

export interface DebugStorageSnapshot {
  local_storage: Array<[string, string]>;
  session_storage: Array<[string, string]>;
  cookies: Array<[string, string]>;
}

export interface DebugDomNode {
  nodeId?: number;
  backendNodeId?: number;
  nodeName?: string;
  nodeType?: number;
  nodeValue?: string;
  attributes?: Record<string, string> | string[] | null;
  children?: DebugDomNode[];
  [key: string]: unknown;
}

export interface DevtoolsCommandResponse<T = unknown> {
  ok: boolean;
  result: T;
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

export async function requestDevtoolsSnapshotRefresh(sessionId: string, scope = 'full'): Promise<void> {
  await post<{ ok: boolean }>(`/devtools/sessions/${sessionId}/refresh`, { scope });
}

export function buildDevtoolsSessionWsUrl(sessionId: string): string {
  return buildWsUrl(`/api/devtools/sessions/${encodeURIComponent(sessionId)}/ws`);
}

export async function findTrafficForDevtoolsRequest(clientReqId: string): Promise<string> {
  const res = await get<{ ok: boolean; traffic_id: string }>(
    `/devtools/network/traffic/${encodeURIComponent(clientReqId)}`,
  );
  return res.traffic_id;
}

export async function sendDevtoolsCommand(
  sessionId: string,
  command: string,
  params: unknown = {},
): Promise<DevtoolsCommandResponse> {
  return post<DevtoolsCommandResponse>(`/devtools/sessions/${sessionId}/commands`, { command, params });
}
