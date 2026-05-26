//! Citation consumption pipeline for memory usage tracking.
//!
//! Connects the citation parsing logic (`memory_citations.rs`) to the usage
//! tracking system (`memory_guard::MemoryUsageRecord`). After each turn, this
//! module scans the assistant response for `<oai-mem-citation>` blocks and
//! records which memory files and rollout IDs were accessed, enabling the
//! retention system to know which memories are actively useful.

use crate::memory_citations::{parse_memory_citation, CitationEntry, MemoryCitation};
use crate::memory_guard::MemoryUsageRecord;
use std::collections::HashMap;
use tracing::debug;

// ─────────────────────────────────────────────────────────────────────────────
// Memory Access Kinds
// ─────────────────────────────────────────────────────────────────────────────

/// Kinds of memory file access detected from tool calls or citations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MemoryAccessKind {
    /// Access to MEMORY.md (the main handbook)
    MemoryMd,
    /// Access to memory_summary.md
    MemorySummary,
    /// Access to raw_memories.md
    RawMemories,
    /// Access to rollout_summaries/*.md
    RolloutSummaries,
    /// Access to skills/*
    Skills,
}

impl std::fmt::Display for MemoryAccessKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MemoryMd => write!(f, "MEMORY.md"),
            Self::MemorySummary => write!(f, "memory_summary.md"),
            Self::RawMemories => write!(f, "raw_memories.md"),
            Self::RolloutSummaries => write!(f, "rollout_summaries/"),
            Self::Skills => write!(f, "skills/"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Citation Result
// ─────────────────────────────────────────────────────────────────────────────

/// Result of processing citations from a turn.
#[derive(Debug, Clone, Default)]
pub struct CitationResult {
    /// Number of citation entries found.
    pub entry_count: usize,
    /// Number of rollout IDs referenced.
    pub rollout_id_count: usize,
    /// Memory access kinds detected from citations.
    pub access_kinds: Vec<MemoryAccessKind>,
    /// Rollout IDs that were cited (for retention tracking).
    pub cited_rollout_ids: Vec<String>,
    /// Files cited with their line ranges.
    pub cited_files: Vec<CitedFile>,
}

/// A file that was cited in a memory citation block.
#[derive(Debug, Clone)]
pub struct CitedFile {
    pub path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub note: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Citation Consumer
// ─────────────────────────────────────────────────────────────────────────────

/// Consumes memory citations from assistant responses and connects them to the
/// usage tracking subsystem.
///
/// This enables:
/// - Retention: frequently-cited memories are kept longer.
/// - Pruning: un-cited memories can be pruned after `max_unused_days`.
/// - Metrics: track how often the model actually uses memory.
#[derive(Debug, Default)]
pub struct CitationConsumer {
    /// Per-file usage records accumulated during this session.
    usage_records: HashMap<String, MemoryUsageRecord>,
    /// Total turns processed in this session.
    turns_processed: u64,
    /// Turns where at least one citation was found.
    turns_with_citations: u64,
}

impl CitationConsumer {
    /// Create a new citation consumer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan an assistant response for `<oai-mem-citation>` blocks and process them.
    ///
    /// Call this after each turn completes. It extracts citations, updates
    /// internal usage records, and returns a summary of what was found.
    pub fn process_turn_citations(&mut self, response: &str) -> CitationResult {
        self.turns_processed += 1;

        let citation = match parse_memory_citation(response) {
            Some(c) => c,
            None => return CitationResult::default(),
        };

        self.turns_with_citations += 1;
        self.process_citation(&citation)
    }

    /// Record that a specific turn used memory (independent of citation parsing).
    ///
    /// Useful for cases where memory was accessed via tool calls (read_file on
    /// memory paths) rather than through the citation block.
    pub fn record_memory_citation_for_turn(&mut self, turn_id: &str) {
        debug!(turn_id = turn_id, "recording memory usage for turn");
        self.turns_with_citations += 1;
    }

    /// Detect memory file access patterns from a shell command string.
    ///
    /// Inspects commands for known memory file path patterns to detect when
    /// the model is accessing memory files via tool calls (e.g., `read_file`,
    /// `grep`, `cat`).
    pub fn detect_memory_access_from_command(command: &str) -> Vec<MemoryAccessKind> {
        let mut kinds = Vec::new();
        let lower = command.to_lowercase();

        if lower.contains("memory.md") && !lower.contains("memory_summary") {
            kinds.push(MemoryAccessKind::MemoryMd);
        }
        if lower.contains("memory_summary.md") || lower.contains("memory_summary") {
            kinds.push(MemoryAccessKind::MemorySummary);
        }
        if lower.contains("raw_memories") {
            kinds.push(MemoryAccessKind::RawMemories);
        }
        if lower.contains("rollout_summaries") || lower.contains("rollout_summary") {
            kinds.push(MemoryAccessKind::RolloutSummaries);
        }
        if lower.contains("skills/") || lower.contains("skill.md") {
            kinds.push(MemoryAccessKind::Skills);
        }

        kinds
    }

    /// Get the accumulated usage records for all cited files.
    pub fn usage_records(&self) -> &HashMap<String, MemoryUsageRecord> {
        &self.usage_records
    }

    /// Get session-level citation statistics.
    pub fn session_stats(&self) -> CitationSessionStats {
        CitationSessionStats {
            turns_processed: self.turns_processed,
            turns_with_citations: self.turns_with_citations,
            unique_files_cited: self.usage_records.len() as u64,
        }
    }

    fn process_citation(&mut self, citation: &MemoryCitation) -> CitationResult {
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut access_kinds = Vec::new();
        let mut cited_files = Vec::new();

        for entry in &citation.entries {
            // Update usage record for this file.
            let record = self.usage_records.entry(entry.file.clone()).or_default();
            record.usage_count += 1;
            record.last_usage_unix = now_unix;

            // Detect access kind.
            let kind = classify_citation_entry(entry);
            if !access_kinds.contains(&kind) {
                access_kinds.push(kind);
            }

            cited_files.push(CitedFile {
                path: entry.file.clone(),
                line_start: entry.line_start,
                line_end: entry.line_end,
                note: entry.note.clone(),
            });
        }

        debug!(
            entry_count = citation.entries.len(),
            rollout_ids = citation.rollout_ids.len(),
            "processed turn citations"
        );

        CitationResult {
            entry_count: citation.entries.len(),
            rollout_id_count: citation.rollout_ids.len(),
            access_kinds,
            cited_rollout_ids: citation.rollout_ids.clone(),
            cited_files,
        }
    }
}

/// Session-level citation statistics.
#[derive(Debug, Clone, Default)]
pub struct CitationSessionStats {
    pub turns_processed: u64,
    pub turns_with_citations: u64,
    pub unique_files_cited: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Classify a citation entry into a MemoryAccessKind based on its file path.
fn classify_citation_entry(entry: &CitationEntry) -> MemoryAccessKind {
    let path_lower = entry.file.to_lowercase();
    if path_lower.starts_with("rollout_summaries/") || path_lower.contains("rollout_summar") {
        MemoryAccessKind::RolloutSummaries
    } else if path_lower.starts_with("skills/") || path_lower.contains("skill.md") {
        MemoryAccessKind::Skills
    } else if path_lower == "memory_summary.md" {
        MemoryAccessKind::MemorySummary
    } else if path_lower == "raw_memories.md" {
        MemoryAccessKind::RawMemories
    } else if path_lower == "memory.md" || path_lower.ends_with("/memory.md") {
        MemoryAccessKind::MemoryMd
    } else {
        // Default to MemoryMd for unrecognized paths within memory workspace.
        MemoryAccessKind::MemoryMd
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RESPONSE: &str = r#"Here is my analysis of the configuration.

The project uses a custom build system with the following key commands:
- `cargo build --workspace` for compilation
- `cargo test --workspace --all-features` for testing

<oai-mem-citation>
<citation_entries>
MEMORY.md:234-236|note=[build system commands]
rollout_summaries/2026-02-17T21-23-02-LN3m-weekly.md:10-12|note=[weekly report format]
skills/rust-build/SKILL.md:5-10|note=[build procedure]
</citation_entries>
<rollout_ids>
019c6e27-e55b-73d1-87d8-4e01f1f75043
019c7714-3b77-74d1-9866-e1f484aae2ab
</rollout_ids>
</oai-mem-citation>"#;

    #[test]
    fn process_turn_citations_extracts_all_info() {
        let mut consumer = CitationConsumer::new();
        let result = consumer.process_turn_citations(SAMPLE_RESPONSE);

        assert_eq!(result.entry_count, 3);
        assert_eq!(result.rollout_id_count, 2);
        assert_eq!(result.cited_rollout_ids.len(), 2);
        assert_eq!(
            result.cited_rollout_ids[0],
            "019c6e27-e55b-73d1-87d8-4e01f1f75043"
        );

        // Check access kinds
        assert!(result.access_kinds.contains(&MemoryAccessKind::MemoryMd));
        assert!(result
            .access_kinds
            .contains(&MemoryAccessKind::RolloutSummaries));
        assert!(result.access_kinds.contains(&MemoryAccessKind::Skills));

        // Check cited files
        assert_eq!(result.cited_files.len(), 3);
        assert_eq!(result.cited_files[0].path, "MEMORY.md");
        assert_eq!(result.cited_files[0].line_start, 234);
        assert_eq!(result.cited_files[0].line_end, 236);
    }

    #[test]
    fn process_turn_citations_updates_usage_records() {
        let mut consumer = CitationConsumer::new();
        consumer.process_turn_citations(SAMPLE_RESPONSE);

        let records = consumer.usage_records();
        assert_eq!(records.len(), 3);
        assert!(records.contains_key("MEMORY.md"));
        assert_eq!(records["MEMORY.md"].usage_count, 1);

        // Process again — counts should increment.
        consumer.process_turn_citations(SAMPLE_RESPONSE);
        let records = consumer.usage_records();
        assert_eq!(records["MEMORY.md"].usage_count, 2);
    }

    #[test]
    fn process_turn_citations_no_citation_returns_default() {
        let mut consumer = CitationConsumer::new();
        let result = consumer.process_turn_citations("Just a normal response with no citations.");
        assert_eq!(result.entry_count, 0);
        assert_eq!(result.rollout_id_count, 0);
        assert!(result.access_kinds.is_empty());
    }

    #[test]
    fn session_stats_tracks_turns() {
        let mut consumer = CitationConsumer::new();
        consumer.process_turn_citations("no citation here");
        consumer.process_turn_citations(SAMPLE_RESPONSE);
        consumer.process_turn_citations("another plain response");

        let stats = consumer.session_stats();
        assert_eq!(stats.turns_processed, 3);
        assert_eq!(stats.turns_with_citations, 1);
        assert_eq!(stats.unique_files_cited, 3);
    }

    #[test]
    fn detect_memory_access_from_command_finds_patterns() {
        let cmd = "read_file path=MEMORY.md";
        let kinds = CitationConsumer::detect_memory_access_from_command(cmd);
        assert!(kinds.contains(&MemoryAccessKind::MemoryMd));

        let cmd2 = "grep pattern rollout_summaries/some-file.md";
        let kinds2 = CitationConsumer::detect_memory_access_from_command(cmd2);
        assert!(kinds2.contains(&MemoryAccessKind::RolloutSummaries));

        let cmd3 = "cat skills/build/SKILL.md";
        let kinds3 = CitationConsumer::detect_memory_access_from_command(cmd3);
        assert!(kinds3.contains(&MemoryAccessKind::Skills));
    }

    #[test]
    fn detect_memory_access_empty_for_unrelated_command() {
        let cmd = "cargo build --release";
        let kinds = CitationConsumer::detect_memory_access_from_command(cmd);
        assert!(kinds.is_empty());
    }

    #[test]
    fn classify_citation_entry_identifies_kinds() {
        let entry = CitationEntry {
            file: "rollout_summaries/2026-01-01-test.md".to_string(),
            line_start: 1,
            line_end: 5,
            note: "test".to_string(),
        };
        assert_eq!(
            classify_citation_entry(&entry),
            MemoryAccessKind::RolloutSummaries
        );

        let entry2 = CitationEntry {
            file: "MEMORY.md".to_string(),
            line_start: 100,
            line_end: 110,
            note: "test".to_string(),
        };
        assert_eq!(classify_citation_entry(&entry2), MemoryAccessKind::MemoryMd);
    }
}
