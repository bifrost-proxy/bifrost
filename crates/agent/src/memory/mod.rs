//! Agent turn loop integration for file-backed memories.
//!
//! Bifrost stores agent memories under `$BIFROST_DATA_DIR/agent/memory` and
//! injects read-path instructions. The model decides when to
//! search `MEMORY.md`, rollout summaries, or memory skills. No database-backed
//! memory store is used.

pub(crate) mod citation_consumer;
pub(crate) mod consolidation;
pub(crate) mod constants;
mod extract;
pub(crate) mod layout;
mod lock;
pub(crate) mod mcp_tools;
pub(crate) mod parse;
pub(crate) mod pollution;
mod read_path;
mod retention;
pub(crate) mod rollout_tracker;
pub(crate) mod search;
pub(crate) mod state_db;
pub(crate) mod sub_agent;
pub(crate) mod telemetry;
#[cfg(test)]
mod tests;
pub(crate) mod types;
pub(crate) mod usage_tracking;
pub(crate) mod utils;
mod write;

// Re-export public memory API.
pub use constants::{MEMORY_CONSOLIDATION_TIMEOUT_SECS, MEMORY_EXTRACT_TIMEOUT_SECS};
pub use extract::{auto_extract_after_turn_blocking, auto_extract_after_turn_with_pollution_check};
pub use layout::{ensure_memory_layout, memory_root};
pub use pollution::PollutionDetector;
pub use read_path::{
    build_memory_read_instructions, generate_memories_enabled, recall_system_message,
    use_memories_enabled,
};
pub use retention::{prune_memory_artifacts, prune_stage1_outputs_for_retention};
pub use types::{
    MemoryFileEntry, MemoryFileStats, MemoryTool, MemoryToolResult, Phase1Status, RolloutRecord,
    SearchMatchMode, SearchQuery, SearchResponse, SearchResult,
};
pub use write::{
    forget_memory, list_visible_memories, memory_stats, remember_explicit, replace_memory,
    search_memory_files,
};
