/**
 * Shared types and constants for Agent settings
 */

// API base path
export const BASE = "/im-gateway";

// Defaults matching Rust AgentConfig::default()
export const DEFAULTS = {
  model: "gpt-5.4-2026-03-05",
  model_provider: "aidp_crawl",
  model_reasoning_effort: "medium",
  model_reasoning_summary: "auto",
  model_context_window: 250_000,
  // When null/undefined, backend derives from context_window × 90% (Codex-compatible).
  model_auto_compact_token_limit: undefined as number | undefined,
  max_completion_tokens: 16_384,
  max_turn_iterations: 20,
  max_history_messages: 50,
  session_ttl_secs: 3600,
  request_timeout_secs: 120,
  project_doc_max_bytes: 32_768,
  background_terminal_max_timeout: 300_000,
} as const;

/** Derive the effective compact threshold (context_window × 90%). */
export function getEffectiveCompactLimit(config: Partial<AgentConfig>): number {
  if (config.model_auto_compact_token_limit != null) {
    return config.model_auto_compact_token_limit;
  }
  const cw = config.model_context_window ?? DEFAULTS.model_context_window;
  return Math.floor(cw * 0.9);
}

// Types
export interface McpServerConfig {
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  cwd?: string;
  url?: string;
  bearer_token_env_var?: string;
  enabled: boolean;
  startup_timeout_secs?: number;
  tool_timeout_sec?: number;
}

export interface HistoryConfig {
  persistence?: "save-all" | "none";
  max_bytes?: number;
}

export interface MemoriesConfig {
  disable_on_external_context?: boolean;
  generate_memories?: boolean;
  use_memories?: boolean;
  max_raw_memories_for_consolidation?: number;
  max_unused_days?: number;
  max_rollout_age_days?: number;
  max_rollouts_per_startup?: number;
  min_rollout_idle_hours?: number;
  min_rate_limit_remaining_percent?: number;
  extract_model?: string;
  consolidation_model?: string;
}

export type MemoryScopeType = "global" | "user" | "project" | "session";
export type MemoryKind =
  | "fact"
  | "preference"
  | "rule"
  | "skill"
  | "task_context"
  | "other";

export interface MemoryScope {
  type: MemoryScopeType;
  value?: string;
}

export interface MemoryRecord {
  id: string;
  path: string;
  content: string;
}

export interface MemoryStats {
  memory_root: string;
  memory_summary_bytes: number;
  memory_md_bytes: number;
  rollout_summary_count: number;
  skill_count: number;
}

export interface AgentConfig {
  enabled: boolean;
  model?: string;
  model_provider?: string;
  model_providers: Record<string, Record<string, unknown>>;
  base_instructions?: string;
  developer_instructions?: string;
  user_instructions?: string;
  default_base_instructions?: string;
  effective_base_instructions?: string;
  model_reasoning_effort?: string;
  model_reasoning_summary?: string;
  model_context_window?: number;
  model_auto_compact_token_limit?: number;
  max_completion_tokens?: number;
  mcp_servers: Record<string, McpServerConfig>;
  skills?: Record<string, unknown>;
  project_doc_max_bytes?: number;
  max_turn_iterations?: number;
  max_history_messages?: number;
  session_ttl_secs?: number;
  tool_output_token_limit?: number;
  request_timeout_secs?: number;
  work_dir?: string;
  // History & Session (Codex-compatible)
  history?: HistoryConfig;
  ephemeral?: boolean;
  // Memories subsystem (Codex-compatible)
  memories?: MemoriesConfig;
  // Background terminal
  background_terminal_max_timeout?: number;
  // Provider-level fields (resolved from active provider)
  request_max_retries?: number;
  stream_idle_timeout_ms?: number;
  stream_max_retries?: number;
}

export interface SessionInfo {
  session_key: string;
  message_count?: number;
  total_tokens_used?: number;
  created_at?: number;
  last_active_at?: number;
  compaction_count?: number;
  estimated_tokens?: number;
  history_version?: number;
  work_dir?: string;
  source?: string;
}

export interface SessionMessage {
  role: string;
  content: string;
  content_parts?: unknown[];
  tool_calls?: string[];
}

export interface SessionDetail extends SessionInfo {
  messages: SessionMessage[];
}

export interface ProviderInfo {
  id: string;
  name: string;
  base_url: string | null;
  env_key: string | null;
}

export interface SkillInfo {
  name: string;
  description: string;
  short_description?: string;
  path: string;
  scope: "repo" | "user" | "global" | "system";
}

export type JsonPrimitive = string | number | boolean | null;
export type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };

export type SkillScope = "repo" | "user" | "global" | "system";
export type ShellKind = "bash" | "sh" | "zsh" | "power_shell";
export type MemoryOp = "read" | "write" | "both";

export type Entrypoint =
  | { kind: "inline"; instructions_md: string }
  | { kind: "shell"; script: string; shell: ShellKind }
  | { kind: "python"; script: string; python?: string | null }
  | { kind: "node"; script: string };

export type ToolBinding =
  | { kind: "registry"; name: string }
  | { kind: "mcp"; server: string; tool: string }
  | { kind: "memory"; op: MemoryOp }
  | { kind: "owned"; name: string; description: string; input_schema: JsonValue };

export type TriggerRule =
  | { kind: "description_match" }
  | { kind: "keyword"; any_of: string[] }
  | { kind: "regex"; pattern: string }
  | { kind: "slash_command" };

export type SkillAuthor =
  | { user: { id: string } }
  | { agent: { session_id: string } }
  | { imported: { origin: string } };

export interface SkillManifest {
  name: string;
  version: string;
  description: string;
  scope: SkillScope;
  entrypoint: Entrypoint;
  allowed_tools: ToolBinding[];
  slash_command?: string | null;
  triggers: TriggerRule[];
  inputs_schema?: JsonValue | null;
  outputs_schema?: JsonValue | null;
  metadata: Record<string, string>;
  env?: Record<string, string>;
  created_by: SkillAuthor;
  created_at_unix: number;
  updated_at_unix: number;
  checksum: string;
  schema_version: number;
}

export interface SkillRecord {
  manifest: SkillManifest;
  name: string;
  version: string;
  description: string;
  scope: SkillScope;
  effective_scope: SkillScope;
  shadow_scopes: SkillScope[];
  enabled: boolean;
  path: string;
  skill_md_path: string;
  checksum: string;
}

export interface SkillListResponse {
  skills: SkillRecord[];
}

export interface SkillDetailResponse {
  record: SkillRecord;
  skill_md: string;
  manifest: SkillManifest;
  effective_scope: SkillScope;
  shadow_scopes: SkillScope[];
}

export interface SkillAssetPayload {
  path: string;
  content: string;
}

export interface SkillCreateRequest {
  manifest: SkillManifest;
  skill_md: string;
  assets?: SkillAssetPayload[];
}

export interface SkillPatchRequest {
  enabled?: boolean;
  skill_md?: string;
  manifest_overrides?: SkillManifest;
  assets?: SkillAssetPayload[];
}

export interface SkillTestReport {
  stdout: string;
  stderr: string;
  tool_calls: JsonValue[];
  duration_ms: number;
  exit_code?: number | null;
}

export interface HistoryFileInfo {
  path: string;
  filename: string;
  session_key: string;
  timestamp: number;
  // New fields from backend summary scan
  total_tokens?: number;
  user_turns?: number;
  assistant_turns?: number;
  tool_calls?: number;
  event_count?: number;
  work_dir?: string;
  source?: string;
  start_time?: number;
  end_time?: number;
  duration_secs?: number;
}

export interface HistoryMessage {
  role: string;
  content: string | null;
}

export interface ConversationEvent {
  timestamp: number;
  event_type:
    | "session_start"
    | "user_message"
    | "assistant_message"
    | "tool_call"
    | "tool_result"
    | "compaction"
    | "session_end"
    | "mcp_tools_loaded"
    | "skills_loaded";
  session_key: string;
  content: unknown;
}
