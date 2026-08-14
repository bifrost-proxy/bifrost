/**
 * Shared types and constants for Agent settings
 */

// API base path
export const BASE = "/im-gateway";

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
