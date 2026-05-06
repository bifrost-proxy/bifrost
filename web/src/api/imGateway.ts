import { get, post, del, patch } from "./client";

// Types
export type ImProviderType = "feishu" | "wechat" | "webhook";
export type ConnectionState =
  | "disconnected"
  | "connecting"
  | "connected"
  | "reconnecting"
  | "failed";

export interface ImProviderConfig {
  id: string;
  provider_type: ImProviderType;
  display_name: string;
  enabled: boolean;
  base_url?: string;
  app_id?: string;
  secret_configured?: boolean;
  owner_open_id?: string;
  event_connection_enabled: boolean;
  event_types: string[];
  agent_config?: {
    work_dir?: string;
    instructions?: string;
    base_instructions?: string;
    developer_instructions?: string;
    user_instructions?: string;
  };
  created_at: number;
  updated_at: number;
}

export interface ImTarget {
  id: string;
  provider_id: string;
  display_name: string;
  receive_id_type: string;
  receive_id: string;
  default_msg_type: string;
  enabled: boolean;
  created_at: number;
  updated_at: number;
}

export interface ImProviderPolicy {
  policy_id: string;
  provider_id: string;
  provider_type: ImProviderType;
  identity_key_type: string;
  identity_key_masked: string;
  status: string;
  permissions: string[];
  script_policy_binding?: {
    shell_policy_ids: string[];
    default_policy_id?: string;
    allow_script_text: boolean;
    allow_script_file: boolean;
  };
  created_at: number;
  updated_at: number;
}

export interface ImRoute {
  id: string;
  provider_id: string;
  name: string;
  enabled: boolean;
  event_type: string;
  matcher: {
    chat_ids: string[];
    user_ids: string[];
    keyword?: string;
    regex?: string;
  };
  action: {
    type: string;
    script_text?: string;
    script_file?: string;
    cwd?: string;
    env?: Record<string, string>;
    reply_target: string;
    reply_mode: string;
  };
  timeout_ms: number;
  max_output_bytes: number;
  created_at: number;
  updated_at: number;
}

export interface ImSchedule {
  id: string;
  name: string;
  enabled: boolean;
  target_id: string;
  trigger: {
    type: "cron" | "interval";
    expr?: string;
    timezone?: string;
    every_ms?: number;
  };
  script: {
    script_text?: string;
    script_file?: string;
    cwd?: string;
    env?: Record<string, string>;
  };
  timeout_ms: number;
  max_output_bytes: number;
  next_run_at?: number;
  last_run_at?: number;
  created_at: number;
  updated_at: number;
}

export interface ImTaskRun {
  run_id: string;
  trigger_source: string;
  route_id?: string;
  schedule_id?: string;
  provider_id?: string;
  target_id?: string;
  status: string;
  started_at: number;
  ended_at?: number;
  duration_ms?: number;
  exit_code?: number;
  stdout_preview?: string;
  stderr_preview?: string;
  error?: string;
}

export interface ImEvent {
  event_id: string;
  provider_id: string;
  provider_type: string;
  event_type: string;
  source: {
    chat_id?: string;
    user_id?: string;
    message_id?: string;
  };
  message?: {
    text: string;
    mentions: string[];
    raw_type: string;
  };
  received_at: number;
}

export interface ConnectionStatus {
  state: ConnectionState;
  last_connected_at?: number;
  last_event_at?: number;
  reconnect_count: number;
  last_error?: string;
}

// API functions
const BASE = "/im-gateway";

// Providers
export async function listProviders(): Promise<ImProviderConfig[]> {
  return get(`${BASE}/providers`);
}

export async function createProvider(
  data: Partial<ImProviderConfig> & { app_secret?: string },
): Promise<ImProviderConfig> {
  return post(`${BASE}/providers`, data);
}

export async function getProvider(id: string): Promise<ImProviderConfig> {
  return get(`${BASE}/providers/${id}`);
}

export async function updateProvider(
  id: string,
  data: Partial<ImProviderConfig>,
): Promise<ImProviderConfig> {
  return patch(`${BASE}/providers/${id}`, data);
}

export async function deleteProvider(id: string): Promise<void> {
  return del(`${BASE}/providers/${id}`);
}

export async function getProviderStatus(
  id: string,
): Promise<ConnectionStatus> {
  return get(`${BASE}/providers/${id}/status`);
}

export async function getProviderPolicy(
  id: string,
): Promise<ImProviderPolicy> {
  return get(`${BASE}/providers/${id}/policy`);
}

export async function updateProviderPolicy(
  id: string,
  data: Partial<ImProviderPolicy>,
): Promise<ImProviderPolicy> {
  return patch(`${BASE}/providers/${id}/policy`, data);
}

// Targets
export async function listTargets(): Promise<ImTarget[]> {
  return get(`${BASE}/targets`);
}

export async function createTarget(
  data: Partial<ImTarget>,
): Promise<ImTarget> {
  return post(`${BASE}/targets`, data);
}

export async function updateTarget(
  id: string,
  data: Partial<ImTarget>,
): Promise<ImTarget> {
  return patch(`${BASE}/targets/${id}`, data);
}

export async function deleteTarget(id: string): Promise<void> {
  return del(`${BASE}/targets/${id}`);
}

// Messages
export async function sendMessage(data: {
  target_id: string;
  msg_type?: string;
  card?: unknown;
  text?: string;
}): Promise<{ message_id: string; request_id: string }> {
  return post(`${BASE}/messages/send`, data);
}

// Routes
export async function listRoutes(): Promise<ImRoute[]> {
  return get(`${BASE}/routes`);
}

export async function createRoute(data: Partial<ImRoute>): Promise<ImRoute> {
  return post(`${BASE}/routes`, data);
}

export async function updateRoute(
  id: string,
  data: Partial<ImRoute>,
): Promise<ImRoute> {
  return patch(`${BASE}/routes/${id}`, data);
}

export async function deleteRoute(id: string): Promise<void> {
  return del(`${BASE}/routes/${id}`);
}

export async function pauseRoute(id: string): Promise<ImRoute> {
  return post(`${BASE}/routes/${id}/pause`, {});
}

export async function resumeRoute(id: string): Promise<ImRoute> {
  return post(`${BASE}/routes/${id}/resume`, {});
}

// Schedules
export async function listSchedules(): Promise<ImSchedule[]> {
  return get(`${BASE}/schedules`);
}

export async function createSchedule(
  data: Partial<ImSchedule>,
): Promise<ImSchedule> {
  return post(`${BASE}/schedules`, data);
}

export async function updateSchedule(
  id: string,
  data: Partial<ImSchedule>,
): Promise<ImSchedule> {
  return patch(`${BASE}/schedules/${id}`, data);
}

export async function deleteSchedule(id: string): Promise<void> {
  return del(`${BASE}/schedules/${id}`);
}

export async function pauseSchedule(id: string): Promise<ImSchedule> {
  return post(`${BASE}/schedules/${id}/pause`, {});
}

export async function resumeSchedule(id: string): Promise<ImSchedule> {
  return post(`${BASE}/schedules/${id}/resume`, {});
}

export async function runSchedule(id: string): Promise<ImTaskRun> {
  return post(`${BASE}/schedules/${id}/run`, {});
}

export async function getScheduleRuns(id: string): Promise<ImTaskRun[]> {
  return get(`${BASE}/schedules/${id}/runs`);
}

// History
export async function listHistoryEvents(): Promise<ImEvent[]> {
  return get(`${BASE}/history/events`);
}

export async function listHistoryRuns(): Promise<ImTaskRun[]> {
  return get(`${BASE}/history/runs`);
}
