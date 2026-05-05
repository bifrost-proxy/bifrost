//! Memory filesystem layout management.

use crate::config::agent_home_dir;
use crate::memory::constants::MEMORY_SKILLS_SUBDIR;
use std::fs;
use std::path::{Path, PathBuf};

/// Return the Codex-compatible memory root: `$agent_home/memory`.
pub fn memory_root() -> PathBuf {
    agent_home_dir().join("memory")
}

/// Ensure the Codex-compatible memory folder layout exists.
pub fn ensure_memory_layout() -> Result<PathBuf, String> {
    let root = memory_root();
    fs::create_dir_all(root.join("rollout_summaries"))
        .map_err(|error| format!("create rollout_summaries: {error}"))?;
    fs::create_dir_all(root.join("skills")).map_err(|error| format!("create skills: {error}"))?;
    fs::create_dir_all(root.join("skills").join(MEMORY_SKILLS_SUBDIR))
        .map_err(|error| format!("create skills/_memory: {error}"))?;
    fs::create_dir_all(root.join("extensions"))
        .map_err(|error| format!("create extensions: {error}"))?;
    ensure_file(&root.join("MEMORY.md"), "# Memory\n\n")?;
    ensure_file(&root.join("memory_summary.md"), "")?;
    ensure_file(
        &root.join("raw_memories.md"),
        "# Raw Memories\n\nNo raw memories yet.\n",
    )?;
    Ok(root)
}

fn ensure_file(path: &Path, default_content: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    fs::write(path, default_content).map_err(|error| format!("write {}: {error}", path.display()))
}
