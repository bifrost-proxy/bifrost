//! Memory retention sweep: archive old raw memories, trim rollout summaries,
//! and prune unused rollout records based on usage frequency.

use crate::config::AgentConfig;
use crate::memory::consolidation::parse_raw_memory_sections;
use crate::memory::constants::*;
use crate::memory::layout::ensure_memory_layout;
use crate::memory::state_db;
use crate::memory::telemetry::telemetry_event;
use crate::memory::utils::{atomic_write, now_secs};
use crate::memory_extensions;
use std::fs;
use std::time::SystemTime;
use tracing::warn;

/// Run the retention sweep:
///   * cap `raw_memories.md` to at most `max_raw_memories_for_consolidation * 4`
///     sections (moving older sections to `raw_memories.archive.md`);
///   * delete `rollout_summaries/*.md` older than `max_rollout_age_days`;
///   * cap total rollout-summary file count at `max_rollouts_per_startup`.
///
/// Called opportunistically from the auto-extract path. Returns `Ok(())` even
/// if individual steps are skipped (e.g., fields not set) — the sweep is
/// best-effort.
pub fn prune_memory_artifacts(config: &AgentConfig) -> Result<(), String> {
    let root = ensure_memory_layout()?;
    let memories_cfg = config.get_memories_config();

    // 1. raw_memories.md: retain recent N sections, archive the rest.
    let retention_window = memories_cfg
        .max_raw_memories_for_consolidation
        .unwrap_or(DEFAULT_MAX_RAW_MEMORIES_FOR_CONSOLIDATION)
        .saturating_mul(4)
        .max(16);
    let raw_path = root.join("raw_memories.md");
    if let Ok(raw) = fs::read_to_string(&raw_path) {
        let sections = parse_raw_memory_sections(&raw);
        if sections.len() > retention_window {
            let split = sections.len() - retention_window;
            let archived = &sections[..split];
            let kept = &sections[split..];
            let archive_path = root.join("raw_memories.archive.md");
            let mut archive_content = fs::read_to_string(&archive_path).unwrap_or_default();
            if archive_content.is_empty() {
                archive_content.push_str("# Raw Memories (archive)\n\n");
            }
            for section in archived {
                archive_content.push_str(section.content.trim());
                archive_content.push_str("\n\n");
            }
            atomic_write(&archive_path, &archive_content)
                .map_err(|error| format!("write archive: {error}"))?;
            let mut kept_content = String::from("# Raw Memories\n\n");
            for section in kept {
                kept_content.push_str(section.content.trim());
                kept_content.push_str("\n\n");
            }
            atomic_write(&raw_path, &kept_content)
                .map_err(|error| format!("write raw_memories.md: {error}"))?;
            telemetry_event(
                "retention.raw_archive",
                archived.len() as u64,
                true,
                Some("sections moved to archive"),
            );
        }
    }

    // 2. rollout_summaries: age + count based trimming.
    let dir = root.join("rollout_summaries");
    let entries: Vec<fs::DirEntry> = fs::read_dir(&dir)
        .map(|iter| iter.filter_map(Result::ok).collect())
        .unwrap_or_default();
    let max_age_days = memories_cfg.max_rollout_age_days;
    let max_count = memories_cfg.max_rollouts_per_startup;
    let now_secs_value = now_secs();

    let mut removed_by_age = 0usize;
    if let Some(days) = max_age_days {
        if days > 0 {
            let age_secs = (days as u64).saturating_mul(86_400);
            for entry in &entries {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(modified) = meta.modified() {
                        if let Ok(age) = SystemTime::now().duration_since(modified) {
                            if age.as_secs() > age_secs && fs::remove_file(entry.path()).is_ok() {
                                removed_by_age += 1;
                            }
                        } else if now_secs_value > 0 {
                            // modified is in the future; skip
                        }
                    }
                }
            }
        }
    }
    if removed_by_age > 0 {
        telemetry_event(
            "retention.rollout_expire",
            removed_by_age as u64,
            true,
            None,
        );
    }

    if let Some(cap) = max_count {
        if cap > 0 {
            let mut remaining: Vec<fs::DirEntry> = fs::read_dir(&dir)
                .map(|iter| iter.filter_map(Result::ok).collect())
                .unwrap_or_default();
            if remaining.len() > cap {
                remaining.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
                let drop_n = remaining.len() - cap;
                let mut removed_by_count = 0usize;
                for entry in remaining.into_iter().take(drop_n) {
                    if fs::remove_file(entry.path()).is_ok() {
                        removed_by_count += 1;
                    }
                }
                if removed_by_count > 0 {
                    telemetry_event("retention.rollout_cap", removed_by_count as u64, true, None);
                }
            }
        }
    }

    // 3. memory_summary.md: if larger than threshold, keep only the last 200 lines.
    let summary_path = root.join("memory_summary.md");
    if let Ok(meta) = fs::metadata(&summary_path) {
        if meta.len() > (MAX_MEMORY_FILE_BYTES / 2) {
            if let Ok(summary) = fs::read_to_string(&summary_path) {
                let lines: Vec<&str> = summary.lines().collect();
                let keep_from = lines.len().saturating_sub(200);
                let trimmed = lines[keep_from..].join("\n");
                atomic_write(&summary_path, &format!("{trimmed}\n"))
                    .map_err(|error| format!("write memory_summary.md: {error}"))?;
                telemetry_event(
                    "retention.summary_trim",
                    (lines.len() - (lines.len() - keep_from)) as u64,
                    true,
                    None,
                );
            }
        }
    }

    // 4. Prune expired extension resources
    if let Err(e) = memory_extensions::prune_expired_resources(
        &root,
        memories_cfg.max_rollout_age_days.map(|d| d.max(0) as u64),
    ) {
        warn!(error = %e, "extension resource pruning failed");
    }

    // 5. Usage-based rollout record pruning (aligned with Codex retention model).
    //    Removes DB records for rollouts that have never been cited and are older
    //    than max_rollout_age_days.
    if let Some(days) = memories_cfg.max_rollout_age_days {
        if days > 0 {
            match state_db::prune_unused_rollouts(&root, days as u64) {
                Ok(pruned) if pruned > 0 => {
                    telemetry_event(
                        "retention.rollout_db_prune",
                        pruned,
                        true,
                        Some("unused rollout records removed from state DB"),
                    );
                }
                Err(e) => {
                    warn!(error = %e, "rollout DB pruning failed");
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Prune Phase 1 (stage 1) output files for rollouts that have been superseded
/// or are below usage thresholds. This is a batch operation that removes
/// rollout summary files from disk whose corresponding DB records have been
/// pruned or whose rollouts have zero usage and are beyond the retention window.
///
/// Returns the number of files removed.
pub fn prune_stage1_outputs_for_retention(
    config: &AgentConfig,
    max_unused_days: Option<u64>,
) -> Result<usize, String> {
    let root = ensure_memory_layout()?;
    let memories_cfg = config.get_memories_config();
    let days = max_unused_days
        .or(memories_cfg.max_rollout_age_days.map(|d| d.max(0) as u64))
        .unwrap_or(30);

    let summaries_dir = root.join("rollout_summaries");
    if !summaries_dir.is_dir() {
        return Ok(0);
    }

    // Get all rollouts that are tracked in the DB.
    let tracked_rollouts = state_db::list_rollouts(&root, 10_000).unwrap_or_default();
    let tracked_ids: std::collections::HashSet<String> = tracked_rollouts
        .iter()
        .map(|r| r.thread_id.clone())
        .collect();

    let mut removed = 0usize;

    let entries: Vec<fs::DirEntry> = fs::read_dir(&summaries_dir)
        .map(|iter| iter.filter_map(Result::ok).collect())
        .unwrap_or_default();

    for entry in entries {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        // If the file has a thread_id-like slug and it's NOT tracked in the DB,
        // check its age for removal.
        if !tracked_ids.contains(&file_name) {
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if let Ok(age) = SystemTime::now().duration_since(modified) {
                        if age.as_secs() > days * 86_400 && fs::remove_file(&path).is_ok() {
                            removed += 1;
                        }
                    }
                }
            }
        }
    }

    if removed > 0 {
        telemetry_event(
            "retention.stage1_output_prune",
            removed as u64,
            true,
            Some("stale stage1 output files removed"),
        );
    }

    Ok(removed)
}
