//! Phase 2 consolidation: sub-agent loop that reads/writes memory files.

use crate::config::AgentConfig;
use crate::memory::constants::*;
use crate::memory::layout::ensure_memory_layout;
use crate::memory::lock::Phase2LockGuard;
use crate::memory::parse::parse_consolidated_memory;
use crate::memory::sub_agent::{execute_phase2_tool, phase2_agent_tools};
use crate::memory::telemetry::telemetry_event;
use crate::memory::types::{Phase2Input, Phase2State, RawMemorySection};
use crate::memory::utils::{
    atomic_write, now_secs, sanitize_skill_name, truncate_middle_approx_tokens,
};
use crate::memory_extensions;
use crate::memory_prompts::CONSOLIDATION_SYSTEM_PROMPT;
use crate::types::ChatMessage;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use tracing::{info, warn};

pub(crate) async fn run_phase2_consolidation(
    client: &crate::client::AgentClient,
    config: &AgentConfig,
) -> Result<(), String> {
    let begin = std::time::Instant::now();
    telemetry_event("phase2.begin", 0, true, None);
    let root = ensure_memory_layout()?;
    let Some(_lock) = Phase2LockGuard::try_acquire(&root)? else {
        info!(memory_root = %root.display(), "phase-2 memory consolidation skipped because another worker holds the lock");
        telemetry_event("phase2.skip", 0, true, Some("locked"));
        return Ok(());
    };
    let input = build_phase2_input(&root, phase2_input_limit(config))?;

    // Detect Phase 2 mode: init if MEMORY.md is empty/default, incremental otherwise
    let existing_memory = std::fs::read_to_string(root.join("MEMORY.md")).unwrap_or_default();
    let phase2_mode = if existing_memory.trim().is_empty()
        || existing_memory.trim() == "# Memory"
        || existing_memory.trim() == "# Memory\n\n"
    {
        "init"
    } else {
        "incremental"
    };

    // Build extensions context
    let extensions_ctx = memory_extensions::build_extensions_context(&root);

    let state = load_phase2_state(&root);
    if state.last_input_hash == input.input_hash && state.failure_count == 0 {
        telemetry_event("phase2.skip", 0, true, Some("hash-unchanged"));
        return Ok(());
    }
    if let Some(pinned) = state.pinned_failure_hash.as_ref() {
        if pinned == &input.input_hash {
            telemetry_event("phase2.skip", 0, false, Some("breaker-open"));
            return Ok(());
        }
    }

    if input.prompt.trim().is_empty() {
        return Ok(());
    }

    let consolidation_config = build_consolidation_agent_config(config);

    let mode_prefix = format!("Phase 2 mode: {}\n\n", phase2_mode);
    let extensions_section = if extensions_ctx.is_empty() {
        String::new()
    } else {
        format!("\n\nMemory Extensions:\n{}", extensions_ctx)
    };
    let full_prompt = format!("{}{}{}", mode_prefix, input.prompt, extensions_section);

    // Phase 2 sub-agent mode: multi-turn tool-calling loop.
    let tools = phase2_agent_tools();
    let mut messages = vec![
        ChatMessage::system(CONSOLIDATION_SYSTEM_PROMPT),
        ChatMessage::user(&full_prompt),
    ];
    let mut final_content = String::new();
    for _round in 0..PHASE2_AGENT_MAX_ROUNDS {
        let response = client
            .chat_completion(&consolidation_config, &messages, &tools)
            .await?;
        if !response.tool_calls.is_empty() {
            messages.push(ChatMessage::assistant_with_tool_calls(
                response.tool_calls.clone(),
            ));
            for tc in &response.tool_calls {
                let result = execute_phase2_tool(&root, tc.name(), tc.arguments());
                messages.push(ChatMessage::tool_result(&tc.id, &result));
            }
            continue;
        }
        final_content = response
            .content
            .or(response.reasoning_content)
            .unwrap_or_default();
        break;
    }

    if final_content.trim().is_empty() {
        return Err("phase-2 sub-agent produced no final content".to_string());
    }

    let consolidated = parse_consolidated_memory(&final_content)?;
    apply_consolidated_memory(&root, consolidated)?;
    save_phase2_state(
        &root,
        &Phase2State {
            last_input_hash: input.input_hash,
            processed_input_count: input.processed_input_count,
            total_input_count: input.total_input_count,
            has_more_inputs: input.has_more_inputs,
            updated_at_unix: now_secs(),
            failure_count: 0,
            pinned_failure_hash: None,
            phase2_mode: phase2_mode.to_string(),
            pollution_state: None,
        },
    )?;
    info!(memory_root = %root.display(), "phase-2 memory consolidation completed");
    telemetry_event("phase2.end", begin.elapsed().as_millis() as u64, true, None);
    Ok(())
}

#[cfg(test)]
pub(crate) fn build_phase2_prompt(root: &Path) -> Result<String, String> {
    Ok(build_phase2_input(root, DEFAULT_MAX_RAW_MEMORIES_FOR_CONSOLIDATION)?.prompt)
}

pub(crate) fn build_phase2_input(
    root: &Path,
    max_raw_memories: usize,
) -> Result<Phase2Input, String> {
    let raw_path = root.join("raw_memories.md");
    let raw = read_limited_text(&raw_path)?;
    if raw.trim().is_empty() || raw.trim() == "# Raw Memories\n\nNo raw memories yet." {
        return Ok(Phase2Input {
            input_hash: String::new(),
            prompt: String::new(),
            processed_input_count: 0,
            total_input_count: 0,
            has_more_inputs: false,
        });
    }
    let max_raw_memories = max_raw_memories.max(1);
    let raw_sections = parse_raw_memory_sections(&raw);
    let total_input_count = raw_sections.len();
    let start = raw_sections.len().saturating_sub(max_raw_memories);
    let selected_sections = &raw_sections[start..];
    let processed_input_count = selected_sections.len();
    let has_more_inputs = total_input_count > processed_input_count;
    let selected_raw = render_selected_raw_memories(selected_sections);
    let selected_rollout_names = selected_sections
        .iter()
        .filter_map(|section| section.rollout_summary_file.clone())
        .collect::<HashSet<_>>();
    let summaries =
        read_selected_rollout_summaries(root, &selected_rollout_names, max_raw_memories)?;
    let existing_memory = read_limited_text(&root.join("MEMORY.md")).unwrap_or_default();
    let existing_summary = read_limited_text(&root.join("memory_summary.md")).unwrap_or_default();
    let prompt = format!(
        "Memory root: {}\n\nPhase 2 selected inputs: {} of {} raw memory entries. has_more_inputs: {}\n\nExisting memory_summary.md:\n{}\n\nExisting MEMORY.md:\n{}\n\nselected raw_memories.md sections:\n{}\n\nselected rollout_summaries:\n{}",
        root.display(),
        processed_input_count,
        total_input_count,
        has_more_inputs,
        existing_summary,
        existing_memory,
        selected_raw,
        summaries
    );
    let mut hasher = DefaultHasher::new();
    selected_raw.hash(&mut hasher);
    summaries.hash(&mut hasher);
    processed_input_count.hash(&mut hasher);
    total_input_count.hash(&mut hasher);
    Ok(Phase2Input {
        input_hash: format!("{:016x}", hasher.finish()),
        prompt: truncate_middle_approx_tokens(&prompt, MEMORY_CONSOLIDATION_INPUT_LIMIT_TOKENS),
        processed_input_count,
        total_input_count,
        has_more_inputs,
    })
}

pub(crate) fn parse_raw_memory_sections(raw: &str) -> Vec<RawMemorySection> {
    let mut sections = Vec::new();
    let mut current = String::new();
    for line in raw.lines() {
        let is_section_header = line.starts_with("## Memory `") || line.starts_with("## Session `");
        if is_section_header && !current.trim().is_empty() && current.trim() != "# Raw Memories" {
            sections.push(raw_memory_section_from_content(std::mem::take(
                &mut current,
            )));
        } else if is_section_header && current.trim() == "# Raw Memories" {
            current.clear();
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        let trimmed = current.trim();
        if trimmed != "# Raw Memories" {
            sections.push(raw_memory_section_from_content(current));
        }
    }
    if sections.is_empty() && !raw.trim().is_empty() {
        sections.push(raw_memory_section_from_content(raw.to_string()));
    }
    sections
}

fn raw_memory_section_from_content(content: String) -> RawMemorySection {
    let rollout_summary_file = content.lines().find_map(|line| {
        line.trim()
            .strip_prefix("rollout_summary_file:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    });
    RawMemorySection {
        content,
        rollout_summary_file,
    }
}

pub(crate) fn render_selected_raw_memories(sections: &[RawMemorySection]) -> String {
    let mut output = String::from("# Raw Memories\n\n");
    for section in sections {
        output.push_str(section.content.trim());
        output.push_str("\n\n");
    }
    output
}

fn read_selected_rollout_summaries(
    root: &Path,
    selected_names: &HashSet<String>,
    fallback_limit: usize,
) -> Result<String, String> {
    let dir = root.join("rollout_summaries");
    let mut paths = fs::read_dir(&dir)
        .map_err(|error| format!("read {}: {error}", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    paths.sort();
    if !selected_names.is_empty() {
        paths.retain(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| selected_names.contains(name))
        });
    } else if paths.len() > fallback_limit {
        paths = paths.split_off(paths.len() - fallback_limit);
    }
    let mut output = String::new();
    for path in paths {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        output.push_str("\n--- ");
        output.push_str(name);
        output.push_str(" ---\n");
        output.push_str(&read_limited_text(&path)?);
    }
    Ok(output)
}

pub(crate) fn read_limited_text(path: &Path) -> Result<String, String> {
    if bifrost_core::text::check_file_size(path, MAX_MEMORY_FILE_BYTES).is_err() {
        return Err(format!("{} is too large", path.display()));
    }
    fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))
}

use crate::memory::types::ConsolidatedMemory;

pub(crate) fn apply_consolidated_memory(
    root: &Path,
    consolidated: ConsolidatedMemory,
) -> Result<(), String> {
    let memory_summary = consolidated.memory_summary.trim();
    let memory = consolidated.memory.trim();
    if !memory_summary.is_empty() {
        atomic_write(
            &root.join("memory_summary.md"),
            &format!("{memory_summary}\n"),
        )
        .map_err(|error| format!("write memory_summary.md: {error}"))?;
    }
    if !memory.is_empty() {
        let memory = if memory.starts_with("# Memory") {
            memory.to_string()
        } else {
            format!("# Memory\n\n{memory}")
        };
        atomic_write(&root.join("MEMORY.md"), &format!("{}\n", memory.trim()))
            .map_err(|error| format!("write MEMORY.md: {error}"))?;
    }
    let memory_skills_dir = root.join("skills").join(MEMORY_SKILLS_SUBDIR);
    fs::create_dir_all(&memory_skills_dir)
        .map_err(|error| format!("create {}: {error}", memory_skills_dir.display()))?;
    let user_skill_dir = root.join("skills");
    for skill in consolidated.skills {
        let name = sanitize_skill_name(&skill.name);
        let skill_md = skill.skill_md.trim();
        if name.is_empty() || skill_md.is_empty() {
            continue;
        }
        // refuse to shadow an existing user-authored skill
        let user_dir = user_skill_dir.join(&name);
        if user_dir.exists() && user_dir != memory_skills_dir.join(&name) {
            warn!(
                skill = %name,
                "refusing to overwrite user skill with memory-skill of the same name"
            );
            continue;
        }
        let dir = memory_skills_dir.join(&name);
        fs::create_dir_all(&dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
        atomic_write(&dir.join("SKILL.md"), &format!("{skill_md}\n"))
            .map_err(|error| format!("write skill {name}: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn phase2_input_hash(root: &Path) -> Result<String, String> {
    Ok(build_phase2_input(root, DEFAULT_MAX_RAW_MEMORIES_FOR_CONSOLIDATION)?.input_hash)
}

/// Build an isolated agent configuration for Phase 2 consolidation.
///
/// Mirrors Codex's `agent::get_config()` in `phase2.rs`:
/// - `ephemeral = true` — not persisted
/// - `generate_memories = false` — prevent recursive memory generation
/// - `use_memories = false` — don't load memories into consolidation agent
/// - `mcp_servers = empty` — no external tools
/// - `skills = disabled` — consolidation agent only uses its own tools
fn build_consolidation_agent_config(config: &AgentConfig) -> AgentConfig {
    let consolidation_model = config.get_memories_config().consolidation_model.clone();
    let mut agent_config = config.clone();

    // Core isolation: prevent recursive memory generation (Codex: generate_memories = false)
    agent_config.ephemeral = true;
    agent_config.memories = Some(crate::config::MemoriesConfig {
        generate_memories: Some(false),
        use_memories: Some(false),
        ..config.get_memories_config()
    });

    // Disable all external integrations (Codex: mcp_servers = allow_only(empty))
    agent_config.mcp_servers = HashMap::new();

    // Disable skills (consolidation agent only uses read_file/write_file/list_files)
    agent_config.skills = None;

    // Use consolidation-specific model if configured
    if let Some(model) = consolidation_model
        .as_ref()
        .filter(|m| !m.trim().is_empty())
    {
        agent_config.model = Some(model.trim().to_string());
    }

    agent_config
}

pub(crate) fn phase2_input_limit(config: &AgentConfig) -> usize {
    config
        .get_memories_config()
        .max_raw_memories_for_consolidation
        .unwrap_or(DEFAULT_MAX_RAW_MEMORIES_FOR_CONSOLIDATION)
        .clamp(1, 4096)
}

pub(crate) fn load_phase2_state(root: &Path) -> Phase2State {
    crate::memory::state_db::load_phase2_state_from_db(root).unwrap_or_default()
}

pub(crate) fn save_phase2_state(root: &Path, state: &Phase2State) -> Result<(), String> {
    crate::memory::state_db::save_phase2_state_to_db(root, state)
}
