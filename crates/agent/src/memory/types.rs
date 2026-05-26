//! Memory system data types.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryFileEntry {
    pub id: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryFileStats {
    pub memory_root: String,
    pub memory_summary_bytes: u64,
    pub memory_md_bytes: u64,
    pub raw_memories_bytes: u64,
    pub rollout_summary_count: usize,
    pub skill_count: usize,
    pub memory_skill_count: usize,
    #[serde(default)]
    pub phase2_last_input_hash: Option<String>,
    #[serde(default)]
    pub phase2_processed_input_count: usize,
    #[serde(default)]
    pub phase2_total_input_count: usize,
    #[serde(default)]
    pub phase2_has_more_inputs: bool,
    #[serde(default)]
    pub phase2_failure_count: usize,
    #[serde(default)]
    pub phase2_updated_at_unix: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ExtractedMemories {
    /// Structured raw memory document (YAML frontmatter + task-grouped body)
    #[serde(default)]
    pub(crate) raw_memory: Option<String>,
    /// Rollout summary — task-level description of what happened
    #[serde(default)]
    pub(crate) rollout_summary: Option<String>,
    /// Filesystem-safe slug for the session (≤80 chars)
    #[serde(default)]
    pub(crate) rollout_slug: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ConsolidatedSkill {
    pub(crate) name: String,
    pub(crate) skill_md: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ConsolidatedMemory {
    pub(crate) memory_summary: String,
    pub(crate) memory: String,
    #[serde(default)]
    pub(crate) skills: Vec<ConsolidatedSkill>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct Phase2State {
    #[serde(default)]
    pub(crate) last_input_hash: String,
    #[serde(default)]
    pub(crate) processed_input_count: usize,
    #[serde(default)]
    pub(crate) total_input_count: usize,
    #[serde(default)]
    pub(crate) has_more_inputs: bool,
    #[serde(default)]
    pub(crate) updated_at_unix: u64,
    #[serde(default)]
    pub(crate) failure_count: usize,
    #[serde(default)]
    pub(crate) pinned_failure_hash: Option<String>,
    #[serde(default)]
    pub(crate) phase2_mode: String,
    #[serde(default)]
    pub(crate) pollution_state: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Phase2Input {
    pub(crate) input_hash: String,
    pub(crate) prompt: String,
    pub(crate) processed_input_count: usize,
    pub(crate) total_input_count: usize,
    pub(crate) has_more_inputs: bool,
}

#[derive(Debug)]
pub(crate) struct RawMemorySection {
    pub(crate) content: String,
    pub(crate) rollout_summary_file: Option<String>,
}

// ---------------------------------------------------------------------------
// Rollout tracking types (aligned with Codex state DB)
// ---------------------------------------------------------------------------

/// Status of a Phase 1 extraction job for a rollout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Phase1Status {
    /// Extraction not yet started.
    Pending,
    /// Claimed by a worker; lease expires at `lease_expires` (unix seconds).
    Claimed { lease_expires: i64 },
    /// Extraction completed successfully.
    Completed,
    /// Extraction failed; retry allowed after `retry_after` (unix seconds).
    Failed { retry_after: i64 },
}

impl Phase1Status {
    /// Serialize to a stable string tag for DB storage.
    pub fn as_db_tag(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Claimed { .. } => "claimed",
            Self::Completed => "completed",
            Self::Failed { .. } => "failed",
        }
    }

    /// Associated expiry/retry timestamp (0 if not applicable).
    pub fn associated_timestamp(&self) -> i64 {
        match self {
            Self::Pending | Self::Completed => 0,
            Self::Claimed { lease_expires } => *lease_expires,
            Self::Failed { retry_after } => *retry_after,
        }
    }

    /// Reconstruct from DB columns.
    pub fn from_db(tag: &str, ts: i64) -> Self {
        match tag {
            "claimed" => Self::Claimed { lease_expires: ts },
            "completed" => Self::Completed,
            "failed" => Self::Failed { retry_after: ts },
            _ => Self::Pending,
        }
    }
}

/// Persistent record for a single rollout (session) tracked in the state DB.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RolloutRecord {
    /// Unique thread/session identifier.
    pub thread_id: String,
    /// Working directory at rollout creation time.
    pub cwd: Option<String>,
    /// Path to the rollout file on disk.
    pub rollout_path: Option<PathBuf>,
    /// Unix timestamp when rollout was first created.
    pub created_at: i64,
    /// Unix timestamp of the last update.
    pub updated_at: i64,
    /// Ownership token for the process that created this rollout.
    pub ownership_token: Option<String>,
    /// Phase 1 extraction job status.
    pub phase1_status: Phase1Status,
    /// Number of times this rollout was cited/used.
    pub usage_count: u64,
    /// Unix timestamp of the last citation usage.
    pub last_usage: Option<i64>,
}

// ---------------------------------------------------------------------------
// Search types (aligned with Codex ext/memories search)
// ---------------------------------------------------------------------------

/// How multiple query terms are matched against file content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SearchMatchMode {
    /// Any single query term matches the line.
    Any,
    /// All query terms must appear on the same line.
    AllOnSameLine,
    /// All query terms appear within a window of `line_count` lines.
    AllWithinLines { line_count: usize },
}

/// A structured search query against memory files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub queries: Vec<String>,
    #[serde(default = "default_match_mode")]
    pub match_mode: SearchMatchMode,
    #[serde(default = "default_true")]
    pub case_sensitive: bool,
    #[serde(default)]
    pub normalize: bool,
    #[serde(default)]
    pub context_lines: usize,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

fn default_match_mode() -> SearchMatchMode {
    SearchMatchMode::Any
}

fn default_true() -> bool {
    true
}

fn default_max_results() -> usize {
    50
}

/// A single search match in a memory file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchResult {
    /// Relative path within the memory store.
    pub file_path: String,
    /// 1-indexed line number of the match start.
    pub line_number: usize,
    /// 1-indexed line number where the returned content block starts.
    pub content_start_line_number: usize,
    /// Content block (match + context).
    pub content: String,
    /// Which query terms were matched.
    pub matched_queries: Vec<String>,
}

/// Response from a memory search operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub queries: Vec<String>,
    pub match_mode: SearchMatchMode,
    pub path: Option<String>,
    pub matches: Vec<SearchResult>,
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// MCP memory tool types
// ---------------------------------------------------------------------------

/// Commands that a memory MCP tool can execute.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum MemoryTool {
    /// List files/directories under a memory path.
    List {
        path: Option<String>,
        cursor: Option<String>,
        #[serde(default = "default_list_limit")]
        limit: usize,
    },
    /// Read a memory file's content.
    Read {
        path: String,
        line_offset: Option<usize>,
        max_lines: Option<usize>,
    },
    /// Search memory files.
    Search(SearchQuery),
}

fn default_list_limit() -> usize {
    100
}

/// Result of a memory tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryToolResult {
    pub content: String,
    pub next_cursor: Option<String>,
    pub total_count: Option<usize>,
}
