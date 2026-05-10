//! Memory system data types.

use serde::{Deserialize, Serialize};

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
