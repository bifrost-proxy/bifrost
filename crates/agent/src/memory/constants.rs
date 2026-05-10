//! Memory system constants.

/// Maximum character limit for memory summary injection.
pub(crate) const MEMORY_SUMMARY_TOKEN_LIMIT_CHARS: usize = 24_000;
/// Default maximum raw memories available for Phase 2 consolidation.
pub(crate) const DEFAULT_MAX_RAW_MEMORIES_FOR_CONSOLIDATION: usize = 512;
/// Approximate bytes per token, aligned with Codex `APPROX_BYTES_PER_TOKEN = 4`.
pub(crate) const APPROX_BYTES_PER_TOKEN: usize = 4;
/// Approximate token budget for Phase 1 user message input (~1500 tokens ≈ 6000 bytes).
pub(crate) const MEMORY_EXTRACT_USER_LIMIT_TOKENS: usize = 1_500;
/// Approximate token budget for Phase 1 assistant message input (~1500 tokens ≈ 6000 bytes).
pub(crate) const MEMORY_EXTRACT_ASSISTANT_LIMIT_TOKENS: usize = 1_500;
/// Approximate token budget for Phase 2 consolidation input (~15000 tokens ≈ 60000 bytes).
pub(crate) const MEMORY_CONSOLIDATION_INPUT_LIMIT_TOKENS: usize = 15_000;

/// Timeout budgets for background memory jobs.
pub const MEMORY_EXTRACT_TIMEOUT_SECS: u64 = 30;
pub const MEMORY_CONSOLIDATION_TIMEOUT_SECS: u64 = 120;

/// Consolidation is skipped forever for the current `input_hash` once this
/// number of consecutive parse/LLM failures is observed.
pub(crate) const MEMORY_CONSOLIDATION_FAILURE_LIMIT: usize = 5;

/// Maximum number of tool-calling rounds the Phase 2 sub-agent is allowed (P2-1).
pub(crate) const PHASE2_AGENT_MAX_ROUNDS: usize = 10;

/// Memory skills subdirectory name.
pub(crate) const MEMORY_SKILLS_SUBDIR: &str = "_memory";

/// Maximum size for memory files we will read into memory (8 MiB).
pub(crate) const MAX_MEMORY_FILE_BYTES: u64 = 8 * 1024 * 1024;
