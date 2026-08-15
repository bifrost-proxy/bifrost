/**
 * Shared types and constants for Agent settings
 */

// API base path
export const BASE = "/im-gateway";

export interface AgentConfig {
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
  project_doc_max_bytes?: number;
  session_ttl_secs?: number;
  work_dir?: string;
  resolved_work_dir?: string;
}
