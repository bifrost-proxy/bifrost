//! Agent turn loop integration for file-backed memories.
//!
//! Bifrost stores agent memories under `$BIFROST_DATA_DIR/agent/memory` and
//! injects read-path instructions. The model decides when to
//! search `MEMORY.md`, rollout summaries, or memory skills. No database-backed
//! memory store is used.

pub(crate) mod consolidation;
pub(crate) mod constants;
mod extract;
pub(crate) mod layout;
mod lock;
pub(crate) mod parse;
mod read_path;
mod retention;
pub(crate) mod state_db;
pub(crate) mod sub_agent;
pub(crate) mod telemetry;
#[cfg(test)]
mod tests;
pub(crate) mod types;
pub(crate) mod utils;
mod write;

// Re-export public memory API.
pub use constants::{MEMORY_CONSOLIDATION_TIMEOUT_SECS, MEMORY_EXTRACT_TIMEOUT_SECS};
pub use extract::{auto_extract_after_turn, auto_extract_after_turn_blocking};
pub use layout::{ensure_memory_layout, memory_root};
pub use read_path::{
    build_memory_read_instructions, generate_memories_enabled, recall_system_message,
    use_memories_enabled,
};
pub use retention::prune_memory_artifacts;
pub use types::{MemoryFileEntry, MemoryFileStats};
pub use write::{
    forget_memory, list_visible_memories, memory_stats, remember_explicit, replace_memory,
    search_memory_files,
};
