import { get, post, del, patch } from "./client";

// Types
export type ImProviderType = "feishu" | "weixin" | "wechat" | "webhook";
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
    runner?: string;
    work_dir?: string;
    base_instructions?: string;
    developer_instructions?: string;
    user_instructions?: string;
  };
  created_at: number;
  updated_at: number;
}

export interface ExternalCliAdapterConfig {
  executable?: string;
  args?: string[];
  env?: Record<string, string>;
  profile?: string;
  profileV2?: string;
  model?: string;
  sandbox?: string;
  approvalPolicy?: string;
  reasoningEffort?: string;
  reasoningSummary?: string;
  dangerFullAccess?: boolean;
  dangerouslyBypassHookTrust?: boolean;
  strictConfig?: boolean;
  skipGitRepoCheck?: boolean;
  ignoreUserConfig?: boolean;
  ignoreRules?: boolean;
  oss?: boolean;
  localProvider?: string;
  outputSchema?: string;
  color?: string;
  addDirs?: string[];
  configOverrides?: string[];
  enableFeatures?: string[];
  disableFeatures?: string[];
  search?: boolean;
  ephemeral?: boolean;
  timeoutSecs?: number;
  browser?: {
    channel?: "edge" | "chrome" | string;
    executable?: string;
    profileDir?: string;
    openOnAuthRequired?: boolean;
    keepBrowserOpenAfterLogin?: boolean;
    executionMode?: "headless" | "headed" | string;
    closeTargetAfterRun?: boolean;
  };
  chatgpt?: {
    baseUrl?: string;
    pollIntervalMs?: number;
    timeoutSecs?: number;
  };
  auth?: {
    statePath?: string;
  };
}

export interface ExternalCliAgentSettings {
  enabled: boolean;
  adapter: string;
  instructions?: string;
  adapterConfig: ExternalCliAdapterConfig;
  injectBifrostTools: boolean;
  skillPaths: string[];
  deliveryMode: "no_im" | "final_reply" | "progress_card";
}

export interface ExternalCliChannelSettings {
  enabled?: boolean;
  runnerId?: string;
  deliveryMode?: "no_im" | "final_reply" | "progress_card";
}

export interface ExternalCliGatewayConfig {
  version: number;
  defaultRunnerId: string;
  runners: Record<string, ExternalCliAgentSettings>;
  channels: Record<string, ExternalCliChannelSettings>;
}

export interface ExternalCliEffectiveConfig {
  providerId?: string;
  runnerId: string;
  settings: ExternalCliAgentSettings;
  sources: Record<string, string>;
}

export interface ExternalCliRunResult {
  runId: string;
  sessionKey?: string;
  runtime: string;
  adapter: string;
  status: string;
  exitCode?: number;
  response: string;
  startedAt: number;
  finishedAt: number;
  durationMs: number;
  events: Array<{ eventType: string; content: string; title?: string; raw?: unknown }>;
  metadata?: Record<string, string>;
  artifacts: {
    runDir: string;
    prompt: string;
    commandSnapshot: string;
    stdout: string;
    stderr: string;
    normalizedEvents: string;
    lastMessage: string;
  };
}

export interface ChatGptWebAuthStatus {
  state: string;
  loggedIn: boolean;
  identityComplete: boolean;
  accountCheckOk: boolean;
  accountStatus?: number;
  cookieCount: number;
  capturedHeaderNames: string[];
  profileDir: string;
  statePath: string;
  message?: string;
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

export type MessageTargetMode =
  | "source_thread"
  | "source_user"
  | "owner"
  | "configured_target";

export interface ImMessageChannelBinding {
  provider_id: string;
  target_id: string;
  target_mode: MessageTargetMode;
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
  message_channel?: ImMessageChannelBinding;
  trigger: {
    type: "cron" | "interval";
    expr?: string;
    timezone?: string;
    every_ms?: number;
  };
  task_type?: "script" | "agent";
  script: {
    script_text?: string;
    script_file?: string;
    cwd?: string;
    env?: Record<string, string>;
  };
  agent?: {
    prompt: string;
    runner_id?: string;
    initial_prompt?: string;
    session_key?: string;
    work_dir?: string;
    system_prompt?: string;
    adapter_config?: ExternalCliAdapterConfig;
    conversation_ref?: {
      adapter: string;
      conversationId?: string;
      threadId?: string;
      updatedAt?: number;
    };
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
  input_preview?: string;
  stdout_preview?: string;
  stderr_preview?: string;
  stdout_digest?: string;
  stderr_digest?: string;
  error?: string;
  task_type?: "script" | "agent";
  runner_id?: string;
  agent_final_response?: string;
  agent_tool_calls?: Array<{
    tool_name: string;
    arguments: string;
    result: string;
    success: boolean;
  }>;
  agent_plan_steps?: Array<{
    step: string;
    status: string;
  }>;
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
  data: Partial<ImProviderConfig> & { app_secret?: string },
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

export async function connectProvider(
  id: string,
): Promise<{ success: boolean; message?: string }> {
  return post(`${BASE}/providers/${id}/connect`);
}

export async function disconnectProvider(
  id: string,
): Promise<{ success: boolean; message?: string }> {
  return post(`${BASE}/providers/${id}/disconnect`);
}

export async function startWeixinLogin(
  id: string,
): Promise<{ success: boolean; scan_url: string; expires_in_seconds: number }> {
  return post(`${BASE}/providers/${id}/weixin-login/start`, {});
}

export async function getWeixinLoginStatus(id: string): Promise<{
  success: boolean;
  status: "idle" | "pending" | "expired" | "confirmed" | "authorized";
  expires_at?: number;
  provider?: ImProviderConfig;
  account?: { account_id: string; user_id: string; base_url: string };
}> {
  return get(`${BASE}/providers/${id}/weixin-login/status`);
}

export async function completeWeixinLogin(
  id: string,
): Promise<{
  success: boolean;
  provider: ImProviderConfig;
  account: { account_id: string; user_id: string; base_url: string };
}> {
  return post(`${BASE}/providers/${id}/weixin-login/complete`, {});
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

export async function getExternalCliConfig(): Promise<ExternalCliGatewayConfig> {
  return get(`${BASE}/chat/config`);
}

export async function updateExternalCliConfig(
  data: ExternalCliGatewayConfig,
): Promise<ExternalCliGatewayConfig> {
  return patch(`${BASE}/chat/config`, data);
}

export async function getExternalCliChannel(
  providerId: string,
): Promise<{
  providerId: string;
  override?: ExternalCliChannelSettings;
  effective: ExternalCliEffectiveConfig;
}> {
  return get(`${BASE}/chat/config/channels/${encodeURIComponent(providerId)}`);
}

export async function updateExternalCliChannel(
  providerId: string,
  data: ExternalCliChannelSettings,
): Promise<{
  providerId: string;
  override?: ExternalCliChannelSettings;
  effective: ExternalCliEffectiveConfig;
}> {
  return patch(`${BASE}/chat/config/channels/${encodeURIComponent(providerId)}`, data);
}

export async function runExternalCliChat(data: {
  message: string;
  operation?: string;
  params?: Record<string, unknown>;
  providerId?: string;
  sessionKey?: string;
  adapter?: string;
  workDir?: string;
  instructions?: string;
  adapterConfig?: ExternalCliAdapterConfig;
}): Promise<ExternalCliRunResult> {
  return post(`${BASE}/chat`, data);
}

export async function getChatGptWebAuthStatus(
  runnerId?: string,
): Promise<ChatGptWebAuthStatus> {
  const query = runnerId ? `?runnerId=${encodeURIComponent(runnerId)}` : "";
  return get(`${BASE}/chat/adapters/chatgpt-web/auth/status${query}`);
}

export async function openChatGptWebLogin(
  runnerId?: string,
): Promise<ChatGptWebAuthStatus> {
  return post(`${BASE}/chat/adapters/chatgpt-web/auth/open`, { runnerId }, { timeout: 0 });
}

export async function stopChatGptWebLogin(
  runnerId?: string,
): Promise<ChatGptWebAuthStatus> {
  return post(`${BASE}/chat/adapters/chatgpt-web/auth/stop`, { runnerId });
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
