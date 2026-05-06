//! Memory write operations: remember, forget, replace, search, stats.

use crate::config::AgentConfig;
use crate::memory::consolidation::load_phase2_state;
use crate::memory::constants::*;
use crate::memory::layout::ensure_memory_layout;
use crate::memory::telemetry::telemetry_event;
use crate::memory::types::{ExtractedMemories, MemoryFileEntry, MemoryFileStats};
use crate::memory::utils::{
    append_line, atomic_write, dir_entry_count, file_len, now_secs, sanitize_id, short_hash,
};
use crate::session::AgentSession;
use std::fs;
use std::path::Path;
use tracing::info;

/// Write Codex-style Phase 1 extraction output:
///   - `rollout_summary` → `rollout_summaries/<slug>.md`
///   - `raw_memory` → appended to `raw_memories.md`
///   - brief summary line → appended to `memory_summary.md`
pub(crate) fn write_codex_style_extraction(
    session_key: &str,
    extracted: &ExtractedMemories,
) -> Result<(), String> {
    let root = ensure_memory_layout()?;
    let now = now_secs();
    let slug = extracted
        .rollout_slug
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(sanitize_id)
        .unwrap_or_else(|| sanitize_id(session_key));

    // 1. Write rollout_summary to rollout_summaries/<slug>.md
    let rollout_file = if let Some(ref summary) = extracted.rollout_summary {
        if !summary.trim().is_empty() {
            let filename = format!("{slug}.md");
            let path = root.join("rollout_summaries").join(&filename);
            let content = format!(
                "session: {session_key}\nupdated_at_unix: {now}\nrollout_slug: {slug}\n\n{}\n",
                summary.trim()
            );
            fs::write(&path, content).map_err(|e| format!("write {}: {e}", path.display()))?;
            info!(rollout_slug = %slug, "wrote rollout summary");
            filename
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // 2. Append raw_memory to raw_memories.md
    if let Some(ref raw_mem) = extracted.raw_memory {
        if !raw_mem.trim().is_empty() {
            let raw_path = root.join("raw_memories.md");
            let current = fs::read_to_string(&raw_path).unwrap_or_default();
            if current.contains("No raw memories yet.") {
                fs::write(&raw_path, "# Raw Memories\n\n")
                    .map_err(|e| format!("reset {}: {e}", raw_path.display()))?;
            }
            append_line(
                &raw_path,
                &format!(
                    "## Session `{session_key}`\nsource: auto_extract\nsession: {session_key}\nrollout_slug: {slug}\nrollout_summary_file: {rollout_file}\nupdated_at_unix: {now}\n\n{}\n\n",
                    raw_mem.trim()
                ),
            )?;
        }
    }

    // 3. Append brief summary line to memory_summary.md
    let brief = extracted
        .rollout_summary
        .as_deref()
        .and_then(|s| s.lines().find(|l| !l.trim().is_empty()))
        .map(|l| l.trim_start_matches('#').trim())
        .unwrap_or("auto-extracted memory");
    let summary_path = root.join("memory_summary.md");
    append_line(&summary_path, &format!("- {brief}\n"))?;

    info!(
        rollout_slug = %slug,
        has_raw_memory = extracted.raw_memory.is_some(),
        has_rollout_summary = extracted.rollout_summary.is_some(),
        "codex-style extraction written"
    );
    telemetry_event("auto_extract.codex_write", 1, true, Some(&slug));
    Ok(())
}

pub(crate) fn remember_auto_from_session_key(
    session_key: &str,
    content: &str,
) -> Result<MemoryFileEntry, String> {
    let root = ensure_memory_layout()?;
    let content = content.trim();
    if content.is_empty() {
        return Err("memory content must not be empty".to_string());
    }
    let id = format!("auto-{}-{}", now_secs(), sanitize_id(session_key));
    let memory_path = root.join("MEMORY.md");
    append_line(
        &memory_path,
        &format!(
            "\n- id: `{id}`\n  source: auto_extract\n  session: `{}`\n  content: {}\n",
            session_key, content
        ),
    )?;
    let summary_path = root.join("memory_summary.md");
    append_line(&summary_path, &format!("- {content}\n"))?;
    append_raw_memory_artifacts(&root, &id, "auto_extract", session_key, content, true)?;
    info!(memory_id = %id, "auto memory extracted");
    telemetry_event("auto_extract.write", content.len() as u64, true, None);
    Ok(MemoryFileEntry {
        id,
        path: "MEMORY.md".to_string(),
        content: content.to_string(),
    })
}

#[cfg(test)]
pub(crate) fn remember_auto(
    _config: &AgentConfig,
    session: &AgentSession,
    content: &str,
) -> Result<MemoryFileEntry, String> {
    remember_auto_from_session_key(&session.session_key, content)
}

/// Explicitly remember text by appending to Codex-compatible files.
pub fn remember_explicit(
    _config: &AgentConfig,
    session: &AgentSession,
    content: &str,
) -> Result<MemoryFileEntry, String> {
    let root = ensure_memory_layout()?;
    let content = content.trim();
    if content.is_empty() {
        return Err("memory content must not be empty".to_string());
    }
    let id = format!(
        "manual-{}-{}",
        now_secs(),
        sanitize_id(&session.session_key)
    );
    let memory_path = root.join("MEMORY.md");
    append_line(
        &memory_path,
        &format!(
            "\n- id: `{id}`\n  source: user_explicit\n  session: `{}`\n  content: {}\n",
            session.session_key, content
        ),
    )?;
    let summary_path = root.join("memory_summary.md");
    append_line(&summary_path, &format!("- {content}\n"))?;
    append_raw_memory_artifacts(
        &root,
        &id,
        "user_explicit",
        &session.session_key,
        content,
        false,
    )?;
    telemetry_event("remember_explicit", content.len() as u64, true, None);
    Ok(MemoryFileEntry {
        id,
        path: "MEMORY.md".to_string(),
        content: content.to_string(),
    })
}

/// Replace a memory entry's content in `MEMORY.md` by id.
pub fn replace_memory(
    _config: &AgentConfig,
    _session: &AgentSession,
    id: &str,
    new_content: &str,
) -> Result<Option<String>, String> {
    let root = ensure_memory_layout()?;
    let path = root.join("MEMORY.md");
    let original =
        fs::read_to_string(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let marker = format!("id: `{id}`");
    if !original.contains(&marker) {
        return Ok(None);
    }
    let mut lines: Vec<String> = original.lines().map(String::from).collect();
    let mut target_block_start = None;
    for (idx, line) in lines.iter().enumerate() {
        if line.contains(&marker) {
            target_block_start = Some(idx);
            break;
        }
    }
    let Some(start) = target_block_start else {
        return Ok(None);
    };
    let mut content_idx = None;
    for (offset, line) in lines[start..].iter().enumerate() {
        if line.trim().is_empty() && offset > 0 {
            break;
        }
        if line.trim_start().starts_with("content:") {
            content_idx = Some(start + offset);
            break;
        }
    }
    if let Some(idx) = content_idx {
        let indent_end = lines[idx].len() - lines[idx].trim_start().len();
        let indent = &lines[idx][..indent_end];
        lines[idx] = format!("{indent}content: {}", new_content.trim());
    } else {
        lines.insert(start + 1, format!("  content: {}", new_content.trim()));
    }
    atomic_write(&path, &format!("{}\n", lines.join("\n")))
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    Ok(Some(id.to_string()))
}

/// Remove a memory entry.
pub fn forget_memory(
    _config: &AgentConfig,
    _session: &AgentSession,
    id_or_last: &str,
) -> Result<Option<String>, String> {
    let root = ensure_memory_layout()?;

    // Handle `file:line` format IDs
    if let Some((file_name, line_str)) = id_or_last.rsplit_once(':') {
        if let Ok(line_num) = line_str.parse::<usize>() {
            let allowed = ["MEMORY.md", "memory_summary.md"];
            if allowed.contains(&file_name) {
                let path = root.join(file_name);
                let original = fs::read_to_string(&path)
                    .map_err(|error| format!("read {}: {error}", path.display()))?;
                let mut lines = original.lines().map(str::to_string).collect::<Vec<_>>();
                if line_num >= 1 && line_num <= lines.len() {
                    lines.remove(line_num - 1);
                    fs::write(&path, format!("{}\n", lines.join("\n")))
                        .map_err(|error| format!("write {}: {error}", path.display()))?;
                    telemetry_event("forget", 1, true, Some(id_or_last));
                    return Ok(Some(id_or_last.to_string()));
                }
                return Ok(None);
            }
        }
    }

    let path = root.join("MEMORY.md");
    let original =
        fs::read_to_string(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let mut lines = original.lines().map(str::to_string).collect::<Vec<_>>();

    let target = if id_or_last == "last" {
        lines.iter().rev().find_map(|line| extract_entry_id(line))
    } else {
        let exact = lines
            .iter()
            .find_map(|line| extract_entry_id(line).filter(|id| id == id_or_last));
        exact.or_else(|| {
            if id_or_last.len() >= 8 {
                lines
                    .iter()
                    .find_map(|line| extract_entry_id(line).filter(|id| id.starts_with(id_or_last)))
            } else {
                Some(id_or_last.to_string())
            }
        })
    };
    let Some(target) = target else {
        return Ok(None);
    };
    let before = lines.len();
    lines.retain(|line| !line.contains(&format!("`{target}`")));
    if lines.len() == before {
        return Ok(None);
    }
    fs::write(&path, format!("{}\n", lines.join("\n")))
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    telemetry_event("forget", 1, true, Some(&target));
    Ok(Some(target))
}

pub(crate) fn extract_entry_id(line: &str) -> Option<String> {
    line.split("id: `")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .filter(|id| id.starts_with("manual-") || id.starts_with("auto-"))
        .map(str::to_string)
}

fn append_raw_memory_artifacts(
    root: &Path,
    id: &str,
    source: &str,
    session_key: &str,
    content: &str,
    write_rollout_summary: bool,
) -> Result<(), String> {
    let now = now_secs();
    let safe_session = sanitize_id(session_key);
    let hash = short_hash(&format!("{id}:{content}"));
    let summary_file = format!("{now}-{hash}-{safe_session}.md");
    let rollout_path = root.join("rollout_summaries").join(&summary_file);

    if write_rollout_summary {
        let rollout_summary = format!(
            "memory_id: {id}\nsource: {source}\nsession: {session_key}\nupdated_at_unix: {now}\n\n{content}\n"
        );
        fs::write(&rollout_path, rollout_summary)
            .map_err(|error| format!("write {}: {error}", rollout_path.display()))?;
    }

    let raw_path = root.join("raw_memories.md");
    let current = fs::read_to_string(&raw_path).unwrap_or_default();
    if current.contains("No raw memories yet.") {
        fs::write(&raw_path, "# Raw Memories\n\n")
            .map_err(|error| format!("reset {}: {error}", raw_path.display()))?;
    }
    append_line(
        &raw_path,
        &format!(
            "## Memory `{id}`\nsource: {source}\nsession: {session_key}\nrollout_summary_file: {}\n\n{content}\n\n",
            if write_rollout_summary {
                summary_file.as_str()
            } else {
                ""
            }
        ),
    )
}

/// List matching memory lines from `memory_summary.md` and `MEMORY.md`.
pub fn list_visible_memories(
    _config: &AgentConfig,
    _session: &AgentSession,
    limit: usize,
) -> Result<Vec<MemoryFileEntry>, String> {
    let root = ensure_memory_layout()?;
    search_memory_files("", limit, &root)
}

pub fn search_memory_files(
    query: &str,
    limit: usize,
    root: &Path,
) -> Result<Vec<MemoryFileEntry>, String> {
    let query = query.trim().to_lowercase();
    let mut entries = Vec::new();
    for file_name in ["memory_summary.md", "MEMORY.md"] {
        let path = root.join(file_name);
        if bifrost_core::text::check_file_size(&path, MAX_MEMORY_FILE_BYTES).is_err() {
            continue;
        }
        let content = fs::read_to_string(&path).unwrap_or_default();
        for (idx, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if !query.is_empty() && !trimmed.to_lowercase().contains(&query) {
                continue;
            }
            let id =
                extract_entry_id(trimmed).unwrap_or_else(|| format!("{file_name}:{}", idx + 1));
            entries.push(MemoryFileEntry {
                id,
                path: file_name.to_string(),
                content: trimmed.to_string(),
            });
            if entries.len() >= limit {
                return Ok(entries);
            }
        }
    }
    Ok(entries)
}

pub fn memory_stats() -> Result<MemoryFileStats, String> {
    let root = ensure_memory_layout()?;
    let state = load_phase2_state(&root);
    Ok(MemoryFileStats {
        memory_summary_bytes: file_len(root.join("memory_summary.md")),
        memory_md_bytes: file_len(root.join("MEMORY.md")),
        raw_memories_bytes: file_len(root.join("raw_memories.md")),
        rollout_summary_count: dir_entry_count(root.join("rollout_summaries")),
        skill_count: dir_entry_count(root.join("skills")),
        memory_skill_count: dir_entry_count(root.join("skills").join(MEMORY_SKILLS_SUBDIR)),
        memory_root: root.display().to_string(),
        phase2_last_input_hash: if state.last_input_hash.is_empty() {
            None
        } else {
            Some(state.last_input_hash)
        },
        phase2_processed_input_count: state.processed_input_count,
        phase2_total_input_count: state.total_input_count,
        phase2_has_more_inputs: state.has_more_inputs,
        phase2_failure_count: state.failure_count,
        phase2_updated_at_unix: state.updated_at_unix,
    })
}
