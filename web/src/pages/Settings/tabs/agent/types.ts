/**
 * Shared types and constants for Agent settings
 */

// API base path
export const BASE = "/im-gateway";

export interface HistoryConfig {
  persistence?: "save-all" | "none";
  max_bytes?: number;
}

export interface AgentConfig {
  enabled: boolean;
  runner?: string;
  model?: string;
  model_provider?: string;
  base_instructions?: string;
  developer_instructions?: string;
  user_instructions?: string;
  default_base_instructions?: string;
  effective_base_instructions?: string;
  model_reasoning_effort?: string;
  model_reasoning_summary?: string;
  model_context_window?: number;
  skills?: Record<string, unknown>;
  project_doc_max_bytes?: number;
  session_ttl_secs?: number;
  work_dir?: string;
  resolved_work_dir?: string;
  // History & Session (Codex-compatible)
  history?: HistoryConfig;
  ephemeral?: boolean;
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
