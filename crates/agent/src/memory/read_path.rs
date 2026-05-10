//! Memory read-path: build model-visible recall instructions.

use crate::config::AgentConfig;
use crate::memory::constants::*;
use crate::memory::layout::{ensure_memory_layout, memory_root};
use crate::memory::telemetry::telemetry_event;
use crate::memory::utils::truncate_chars;
use crate::memory_prompts::READ_PATH_TEMPLATE;
use crate::session::AgentSession;
use crate::types::ChatMessage;
use std::fs;
use std::path::Path;
use tracing::{info, warn};

/// Check whether memory recall is enabled.
pub fn use_memories_enabled(config: &AgentConfig) -> bool {
    config.get_memories_config().use_memories != Some(false)
}

/// Check whether background memory generation is enabled.
pub fn generate_memories_enabled(config: &AgentConfig) -> bool {
    config.get_memories_config().generate_memories != Some(false)
}

/// Build the developer instructions that teach the model when and how to read memory.
pub fn build_memory_read_instructions() -> Option<String> {
    let root = match ensure_memory_layout() {
        Ok(root) => root,
        Err(error) => {
            warn!(error = %error, "failed to prepare memory layout, skipping memory injection");
            telemetry_event(
                "read_inject.skip",
                0,
                false,
                Some(&format!("layout: {error}")),
            );
            return None;
        }
    };
    let summary_path = root.join("memory_summary.md");
    if bifrost_core::text::check_file_size(&summary_path, MAX_MEMORY_FILE_BYTES).is_err() {
        warn!(path = %summary_path.display(), "memory summary file too large, skipping");
        telemetry_event("read_inject.skip", 0, false, Some("summary too large"));
        return None;
    }
    let summary = fs::read_to_string(&summary_path).ok()?.trim().to_string();
    if summary.is_empty() {
        telemetry_event("read_inject.skip", 0, true, Some("empty summary"));
        return None;
    }
    let summary = truncate_chars(&summary, MEMORY_SUMMARY_TOKEN_LIMIT_CHARS);
    telemetry_event("read_inject.hit", summary.len() as u64, true, None);
    Some(render_read_path_prompt(&root, &summary))
}

fn render_read_path_prompt(root: &Path, summary: &str) -> String {
    READ_PATH_TEMPLATE
        .replace("{{ base_path }}", &root.display().to_string())
        .replace("{{ memory_summary }}", summary)
}

/// Build the model-visible memory instruction message.
pub fn recall_system_message(
    config: &AgentConfig,
    _session: &AgentSession,
    _latest_user_message: &str,
) -> Option<ChatMessage> {
    if !use_memories_enabled(config) {
        return None;
    }
    let message = build_memory_read_instructions()?;
    info!(memory_root = %memory_root().display(), "memory read instructions injected");
    Some(ChatMessage::developer(&message))
}
