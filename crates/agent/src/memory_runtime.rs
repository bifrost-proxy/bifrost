//! Agent turn loop integration for Codex-style file-backed memories.
//!
//! Bifrost stores agent memories under `$BIFROST_DATA_DIR/agent/memory` and
//! injects Codex-compatible read-path instructions. The model decides when to
//! search `MEMORY.md`, rollout summaries, or memory skills. No database-backed
//! memory store is used.

use crate::config::{agent_home_dir, AgentConfig};
use crate::memory_extensions;
use crate::memory_guard;
use crate::session::AgentSession;
use crate::types::ChatMessage;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tracing::{info, warn};

const MEMORY_SUMMARY_TOKEN_LIMIT_CHARS: usize = 24_000;
const DEFAULT_MAX_RAW_MEMORIES_FOR_CONSOLIDATION: usize = 512;
const AUTO_MEMORY_MAX_ITEMS: usize = 8;

/// Approximate bytes per token, aligned with Codex `APPROX_BYTES_PER_TOKEN = 4`.
const APPROX_BYTES_PER_TOKEN: usize = 4;
/// Approximate token budget for Phase 1 user message input (~1500 tokens ≈ 6000 bytes).
const MEMORY_EXTRACT_USER_LIMIT_TOKENS: usize = 1_500;
/// Approximate token budget for Phase 1 assistant message input (~1500 tokens ≈ 6000 bytes).
const MEMORY_EXTRACT_ASSISTANT_LIMIT_TOKENS: usize = 1_500;
/// Approximate token budget for Phase 2 consolidation input (~15000 tokens ≈ 60000 bytes).
const MEMORY_CONSOLIDATION_INPUT_LIMIT_TOKENS: usize = 15_000;

/// Timeout budgets for background memory jobs. These are independent of the
/// user-visible request timeout so that a slow consolidation run does not
/// block the agent turn.
pub const MEMORY_EXTRACT_TIMEOUT_SECS: u64 = 30;
pub const MEMORY_CONSOLIDATION_TIMEOUT_SECS: u64 = 120;

/// Consolidation is skipped forever for the current `input_hash` once this
/// number of consecutive parse/LLM failures is observed. This prevents a
/// hard-failing model output from re-submitting the same input on every turn.
const MEMORY_CONSOLIDATION_FAILURE_LIMIT: usize = 5;

/// Retention thresholds — memory skills are written under this subdirectory
/// to keep them isolated from user-authored skills.
const MEMORY_SKILLS_SUBDIR: &str = "_memory";

use crate::memory_prompts::{
    CONSOLIDATION_SYSTEM_PROMPT, EXTRACT_INPUT_TEMPLATE, EXTRACT_SYSTEM_PROMPT, READ_PATH_TEMPLATE,
};

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

/// Return the Codex-compatible memory root: `$agent_home/memory`.
pub fn memory_root() -> PathBuf {
    agent_home_dir().join("memory")
}

/// 判断召回说明是否开启。
pub fn use_memories_enabled(config: &AgentConfig) -> bool {
    config.get_memories_config().use_memories != Some(false)
}

/// 判断后台记忆生成是否开启。
pub fn generate_memories_enabled(config: &AgentConfig) -> bool {
    config.get_memories_config().generate_memories != Some(false)
}

/// Ensure the Codex-compatible memory folder layout exists.
pub fn ensure_memory_layout() -> Result<PathBuf, String> {
    let root = memory_root();
    fs::create_dir_all(root.join("rollout_summaries"))
        .map_err(|error| format!("create rollout_summaries: {error}"))?;
    fs::create_dir_all(root.join("skills")).map_err(|error| format!("create skills: {error}"))?;
    fs::create_dir_all(root.join("skills").join(MEMORY_SKILLS_SUBDIR))
        .map_err(|error| format!("create skills/_memory: {error}"))?;
    // Create extensions directory
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

/// Maximum size for memory files we will read into memory (8 MiB).
/// This prevents OOM if a memory file grows unexpectedly large.
const MAX_MEMORY_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Build the Codex-style developer instructions that teach the model when and how to read memory.
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

fn truncate_chars(input: &str, limit: usize) -> String {
    if input.chars().count() <= limit {
        return input.to_string();
    }
    let mut output = input.chars().take(limit).collect::<String>();
    output.push_str("\n[truncated]");
    output
}

/// Approximate token count for a UTF-8 string using `bytes / 4` heuristic.
/// Aligned with Codex `APPROX_BYTES_PER_TOKEN = 4`.
fn approx_token_count(text: &str) -> usize {
    text.len().saturating_add(APPROX_BYTES_PER_TOKEN - 1) / APPROX_BYTES_PER_TOKEN
}

/// Truncate text to approximately `max_tokens` tokens, preserving the beginning
/// and end. Middle content is replaced with a `…N tokens truncated…` marker.
/// Aligned with Codex `truncate_middle_with_token_budget`.
fn truncate_middle_approx_tokens(text: &str, max_tokens: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    if max_tokens == 0 {
        let total = approx_token_count(text);
        return format!("…{total} tokens truncated…");
    }
    let max_bytes = max_tokens.saturating_mul(APPROX_BYTES_PER_TOKEN);
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let total_tokens = approx_token_count(text);
    // Reserve ~40 bytes for the truncation marker
    let keep_bytes = max_bytes.saturating_sub(40);
    let left_budget = keep_bytes / 2;
    let right_budget = keep_bytes - left_budget;
    // Find char boundary for left portion
    let mut left_end = left_budget.min(text.len());
    while left_end > 0 && !text.is_char_boundary(left_end) {
        left_end -= 1;
    }
    // Find char boundary for right portion
    let mut right_start = text.len().saturating_sub(right_budget);
    while right_start < text.len() && !text.is_char_boundary(right_start) {
        right_start += 1;
    }
    if right_start <= left_end {
        right_start = left_end;
    }
    let removed_tokens = total_tokens.saturating_sub(max_tokens);
    format!(
        "{}…{} tokens truncated…{}",
        &text[..left_end],
        removed_tokens,
        &text[right_start..]
    )
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
    Some(ChatMessage::system(&message))
}

#[derive(Debug, Deserialize)]
struct ExtractedMemories {
    /// Legacy field: individual memory lines (backward compat with old prompt format)
    #[serde(default)]
    memories: Vec<String>,
    /// Structured raw memory document (Codex-aligned: YAML frontmatter + task-grouped body)
    #[serde(default)]
    raw_memory: Option<String>,
    /// Rollout summary — task-level description of what happened
    #[serde(default)]
    rollout_summary: Option<String>,
    /// Filesystem-safe slug for the session (≤80 chars)
    #[serde(default)]
    rollout_slug: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConsolidatedSkill {
    name: String,
    skill_md: String,
}

#[derive(Debug, Deserialize)]
struct ConsolidatedMemory {
    memory_summary: String,
    memory: String,
    #[serde(default)]
    skills: Vec<ConsolidatedSkill>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Phase2State {
    #[serde(default)]
    last_input_hash: String,
    #[serde(default)]
    processed_input_count: usize,
    #[serde(default)]
    total_input_count: usize,
    #[serde(default)]
    has_more_inputs: bool,
    #[serde(default)]
    updated_at_unix: u64,
    /// Consecutive parse/LLM failures observed for `last_input_hash`.
    #[serde(default)]
    failure_count: usize,
    /// When `failure_count >= MEMORY_CONSOLIDATION_FAILURE_LIMIT`, we pin the
    /// hash that tripped the breaker so we skip re-running until inputs change.
    #[serde(default)]
    pinned_failure_hash: Option<String>,
    /// Phase 2 mode: "init" for first run, "incremental" for updates
    #[serde(default)]
    phase2_mode: String,
    /// Memory pollution state for the session that triggered this consolidation
    #[serde(default)]
    pollution_state: Option<String>,
}

#[derive(Debug)]
struct Phase2Input {
    input_hash: String,
    prompt: String,
    processed_input_count: usize,
    total_input_count: usize,
    has_more_inputs: bool,
}

#[derive(Debug)]
struct RawMemorySection {
    content: String,
    rollout_summary_file: Option<String>,
}

/// Cross-process exclusive lock for Phase 2 consolidation. Uses real `fs2`
/// advisory locking so that if a process dies abruptly the OS releases the
/// lock — no staleness heuristic required.
struct Phase2LockGuard {
    file: Option<fs::File>,
    path: PathBuf,
}

impl Drop for Phase2LockGuard {
    fn drop(&mut self) {
        if let Some(ref file) = self.file {
            let _ = FileExt::unlock(file);
        }
        // Keep the lock file on disk so that `ls -la` still shows it — the
        // advisory lock is what matters, not the file existence. Cleaning up
        // unconditionally can race with another process that is right now
        // trying to open+lock.
        let _ = &self.path;
    }
}

impl Phase2LockGuard {
    fn try_acquire(root: &Path) -> Result<Option<Self>, String> {
        let path = root.join(".phase2.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                // best-effort: record pid + timestamp for human debugging
                let note = format!("pid={} acquired_at={}\n", std::process::id(), now_secs());
                let _ = (&file).write_all(note.as_bytes());
                let _ = file.sync_data();
                Ok(Some(Self {
                    file: Some(file),
                    path,
                }))
            }
            Err(error) => {
                // contention: another process holds the lock
                if error.kind() == std::io::ErrorKind::WouldBlock {
                    return Ok(None);
                }
                Err(format!("lock {}: {error}", path.display()))
            }
        }
    }
}

/// Generate durable file-backed memories after a turn.
///
/// Spawns a background task so that the turn can return immediately. A
/// per-job timeout protects against a hanging LLM call.
pub fn auto_extract_after_turn(
    client: std::sync::Arc<crate::client::AgentClient>,
    config: AgentConfig,
    session_key: String,
    user_message: String,
    assistant_message: String,
) {
    if !generate_memories_enabled(&config) {
        return;
    }
    tokio::spawn(async move {
        let begin = std::time::Instant::now();
        telemetry_event("auto_extract.begin", 0, true, None);
        let deadline = Duration::from_secs(MEMORY_EXTRACT_TIMEOUT_SECS);
        let work = auto_extract_after_turn_inner(
            &client,
            &config,
            &session_key,
            &user_message,
            &assistant_message,
        );
        match tokio::time::timeout(deadline, work).await {
            Ok(Ok(())) => {
                telemetry_event(
                    "auto_extract.end",
                    begin.elapsed().as_millis() as u64,
                    true,
                    None,
                );
            }
            Ok(Err(error)) => {
                warn!(error = %error, "failed to generate file-backed memories");
                telemetry_event(
                    "auto_extract.end",
                    begin.elapsed().as_millis() as u64,
                    false,
                    Some(&error),
                );
            }
            Err(_) => {
                warn!(
                    secs = MEMORY_EXTRACT_TIMEOUT_SECS,
                    "auto memory extraction timed out"
                );
                telemetry_event(
                    "auto_extract.end",
                    begin.elapsed().as_millis() as u64,
                    false,
                    Some("timeout"),
                );
            }
        }
    });
}

/// Synchronous variant that drives extraction deterministically without
/// spawning a task. Intended for tests and for rare callers that must observe
/// the final on-disk memory state before returning. Production turn paths
/// should continue to use [`auto_extract_after_turn`] so the assistant reply
/// is not blocked on memory work.
pub async fn auto_extract_after_turn_blocking(
    client: &crate::client::AgentClient,
    config: &AgentConfig,
    session: &AgentSession,
    user_message: &str,
    assistant_message: &str,
) -> Result<(), String> {
    if !generate_memories_enabled(config) {
        return Ok(());
    }
    auto_extract_after_turn_inner(
        client,
        config,
        &session.session_key,
        user_message,
        assistant_message,
    )
    .await
}

async fn auto_extract_after_turn_inner(
    client: &crate::client::AgentClient,
    config: &AgentConfig,
    session_key: &str,
    user_message: &str,
    assistant_message: &str,
) -> Result<(), String> {
    ensure_memory_layout()?;

    let user_message = user_message.trim();
    let assistant_message = assistant_message.trim();
    let user_message = memory_guard::filter_developer_content(user_message);
    let assistant_message = memory_guard::filter_developer_content(assistant_message);
    let user_message = user_message.trim();
    let assistant_message = assistant_message.trim();
    if user_message.is_empty() && assistant_message.is_empty() {
        return Ok(());
    }

    let mut extract_config = config.clone();
    if let Some(model) = config
        .get_memories_config()
        .extract_model
        .as_ref()
        .filter(|model| !model.trim().is_empty())
    {
        extract_config.model = Some(model.trim().to_string());
    }

    let prompt = EXTRACT_INPUT_TEMPLATE
        .replace("{session_key}", session_key)
        .replace(
            "{user_message}",
            &truncate_middle_approx_tokens(user_message, MEMORY_EXTRACT_USER_LIMIT_TOKENS),
        )
        .replace(
            "{assistant_message}",
            &truncate_middle_approx_tokens(
                assistant_message,
                MEMORY_EXTRACT_ASSISTANT_LIMIT_TOKENS,
            ),
        );
    let response = client
        .chat_completion(
            &extract_config,
            &[
                ChatMessage::system(EXTRACT_SYSTEM_PROMPT),
                ChatMessage::user(&prompt),
            ],
            &[],
        )
        .await?;
    let content = response
        .content
        .or(response.reasoning_content)
        .unwrap_or_default();
    let extracted = parse_extracted_memories(&content);
    let mut wrote_memory = false;

    // New path: Codex-style structured output (raw_memory + rollout_summary)
    if extracted
        .raw_memory
        .as_ref()
        .is_some_and(|s| !s.trim().is_empty())
    {
        write_codex_style_extraction(session_key, &extracted)?;
        wrote_memory = true;
    }

    // Legacy fallback: individual memory lines
    if !wrote_memory {
        for memory in extracted.memories.iter().take(AUTO_MEMORY_MAX_ITEMS) {
            remember_auto_from_session_key(session_key, memory)?;
            wrote_memory = true;
        }
    }
    if wrote_memory {
        // Run retention opportunistically before consolidation so the phase-2
        // input window sees the trimmed set rather than the unbounded file.
        if let Err(error) = prune_memory_artifacts(config) {
            warn!(error = %error, "memory retention sweep failed");
        }
        let consolidation_deadline = Duration::from_secs(MEMORY_CONSOLIDATION_TIMEOUT_SECS);
        match tokio::time::timeout(
            consolidation_deadline,
            run_phase2_consolidation(client, config),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                warn!(error = %error, "phase-2 consolidation failed");
                bump_phase2_failure(&error);
            }
            Err(_) => {
                warn!(
                    secs = MEMORY_CONSOLIDATION_TIMEOUT_SECS,
                    "phase-2 consolidation timed out"
                );
                bump_phase2_failure("timeout");
            }
        }
    }
    Ok(())
}

fn bump_phase2_failure(reason: &str) {
    if let Ok(root) = ensure_memory_layout() {
        let mut state = load_phase2_state(&root);
        state.failure_count = state.failure_count.saturating_add(1);
        if state.failure_count >= MEMORY_CONSOLIDATION_FAILURE_LIMIT {
            state.pinned_failure_hash = Some(state.last_input_hash.clone());
        }
        state.updated_at_unix = now_secs();
        let _ = save_phase2_state(&root, &state);
        telemetry_event(
            "phase2.failure",
            state.failure_count as u64,
            false,
            Some(reason),
        );
    }
}

fn parse_extracted_memories(content: &str) -> ExtractedMemories {
    let trimmed = strip_markdown_fences(content.trim());
    let mut extracted = serde_json::from_str::<ExtractedMemories>(trimmed)
        .or_else(|_| {
            extract_balanced_json(trimmed)
                .ok_or_else(|| serde_json::Error::io(std::io::Error::other("missing json")))
                .and_then(serde_json::from_str::<ExtractedMemories>)
        })
        .unwrap_or_else(|_| ExtractedMemories {
            memories: parse_memory_lines(trimmed),
            raw_memory: None,
            rollout_summary: None,
            rollout_slug: None,
        });

    // Deduplicate and normalize the legacy memories field
    let mut output = Vec::new();
    for memory in std::mem::take(&mut extracted.memories) {
        let memory = normalize_memory_line(&memory);
        if memory.is_empty() || output.iter().any(|existing: &String| existing == &memory) {
            continue;
        }
        output.push(memory);
    }
    extracted.memories = output;
    extracted
}

/// Strip a leading ```json / ```JSON / ``` fence and its trailing ``` if
/// present. Only the first fenced block is unwrapped; nested or multiple
/// blocks fall through to the balanced-brace scanner.
fn strip_markdown_fences(content: &str) -> &str {
    let trimmed = content.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        // drop optional language tag up to first newline
        let after_lang = rest.find('\n').map(|idx| &rest[idx + 1..]).unwrap_or(rest);
        if let Some(end) = after_lang.rfind("```") {
            return after_lang[..end].trim();
        }
        return after_lang.trim();
    }
    trimmed
}

/// Find the first balanced JSON object/array in the given text using a
/// bracket-counting scan that respects string and escape semantics. Returns
/// a slice over the original text when a balanced block is found.
fn extract_balanced_json(content: &str) -> Option<&str> {
    let bytes = content.as_bytes();
    let mut start = None;
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut opener = b'{';
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => {
                if start.is_none() {
                    start = Some(i);
                    opener = b;
                }
                depth += 1;
            }
            b'}' | b']' if start.is_some() => {
                let closer = if opener == b'{' { b'}' } else { b']' };
                depth -= 1;
                if depth == 0 {
                    if b == closer {
                        let s = start.unwrap();
                        return Some(&content[s..=i]);
                    }
                    // mismatched closer — reset scan
                    start = None;
                    depth = 0;
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_memory_lines(content: &str) -> Vec<String> {
    content
        .lines()
        .map(normalize_memory_line)
        .filter(|line| !line.is_empty())
        .collect()
}

fn normalize_memory_line(line: &str) -> String {
    line.trim()
        .trim_start_matches(|ch: char| ch == '-' || ch == '*' || ch.is_ascii_digit())
        .trim_start_matches(['.', ')', ':', ' '])
        .trim()
        .trim_matches('"')
        .trim()
        .to_string()
}

/// Write Codex-style Phase 1 extraction output:
///   - `rollout_summary` → `rollout_summaries/<slug>.md`
///   - `raw_memory` → appended to `raw_memories.md`
///   - brief summary line → appended to `memory_summary.md`
fn write_codex_style_extraction(
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

fn remember_auto_from_session_key(
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
fn remember_auto(
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

/// Replace a memory entry's content in `MEMORY.md` by id. The entry is
/// rewritten in place so the append-only history remains but the visible
/// content reflects the new value. Returns `Ok(Some(id))` on success,
/// `Ok(None)` if the id does not exist.
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
    // find the `content:` line in the same block (next blank line ends block)
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

async fn run_phase2_consolidation(
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

    let mut consolidation_config = config.clone();
    if let Some(model) = config
        .get_memories_config()
        .consolidation_model
        .as_ref()
        .filter(|model| !model.trim().is_empty())
    {
        consolidation_config.model = Some(model.trim().to_string());
    }

    let mode_prefix = format!("Phase 2 mode: {}\n\n", phase2_mode);
    let extensions_section = if extensions_ctx.is_empty() {
        String::new()
    } else {
        format!("\n\nMemory Extensions:\n{}", extensions_ctx)
    };
    let full_prompt = format!("{}{}{}", mode_prefix, input.prompt, extensions_section);

    let response = client
        .chat_completion(
            &consolidation_config,
            &[
                ChatMessage::system(CONSOLIDATION_SYSTEM_PROMPT),
                ChatMessage::user(&full_prompt),
            ],
            &[],
        )
        .await?;
    let content = response
        .content
        .or(response.reasoning_content)
        .unwrap_or_default();
    let consolidated = parse_consolidated_memory(&content)?;
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
fn build_phase2_prompt(root: &Path) -> Result<String, String> {
    Ok(build_phase2_input(root, DEFAULT_MAX_RAW_MEMORIES_FOR_CONSOLIDATION)?.prompt)
}

fn build_phase2_input(root: &Path, max_raw_memories: usize) -> Result<Phase2Input, String> {
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

fn parse_raw_memory_sections(raw: &str) -> Vec<RawMemorySection> {
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

fn render_selected_raw_memories(sections: &[RawMemorySection]) -> String {
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

fn read_limited_text(path: &Path) -> Result<String, String> {
    if bifrost_core::text::check_file_size(path, MAX_MEMORY_FILE_BYTES).is_err() {
        return Err(format!("{} is too large", path.display()));
    }
    fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))
}

fn parse_consolidated_memory(content: &str) -> Result<ConsolidatedMemory, String> {
    let trimmed = strip_markdown_fences(content.trim());
    let consolidated = serde_json::from_str::<ConsolidatedMemory>(trimmed)
        .or_else(|_| {
            extract_balanced_json(trimmed)
                .ok_or_else(|| serde_json::Error::io(std::io::Error::other("missing json")))
                .and_then(serde_json::from_str::<ConsolidatedMemory>)
        })
        .map_err(|error| format!("parse consolidated memory JSON: {error}"))?;
    Ok(consolidated)
}

fn apply_consolidated_memory(root: &Path, consolidated: ConsolidatedMemory) -> Result<(), String> {
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
fn phase2_input_hash(root: &Path) -> Result<String, String> {
    Ok(build_phase2_input(root, DEFAULT_MAX_RAW_MEMORIES_FOR_CONSOLIDATION)?.input_hash)
}

fn load_phase2_state(root: &Path) -> Phase2State {
    let path = root.join(".phase2_state.json");
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<Phase2State>(&content).ok())
        .unwrap_or_default()
}

fn save_phase2_state(root: &Path, state: &Phase2State) -> Result<(), String> {
    let path = root.join(".phase2_state.json");
    let content =
        serde_json::to_string_pretty(state).map_err(|error| format!("serialize state: {error}"))?;
    atomic_write(&path, &format!("{content}\n"))
        .map_err(|error| format!("write {}: {error}", path.display()))
}

fn phase2_input_limit(config: &AgentConfig) -> usize {
    config
        .get_memories_config()
        .max_raw_memories_for_consolidation
        .unwrap_or(DEFAULT_MAX_RAW_MEMORIES_FOR_CONSOLIDATION)
        .clamp(1, 4096)
}

fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let tmp = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        now_secs()
    ));
    fs::write(&tmp, content)?;
    // best-effort fsync of tmp before rename
    if let Ok(file) = fs::OpenOptions::new().read(true).open(&tmp) {
        let _ = file.sync_data();
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Append a line to `path` under an exclusive advisory lock. The lock is held
/// for the duration of the write and explicitly released before the file is
/// dropped. `write_all` is followed by `flush` + `sync_data` so that the
/// bytes are durably on disk when the function returns.
fn append_line(path: &Path, line: &str) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    file.lock_exclusive()
        .map_err(|error| format!("lock {}: {error}", path.display()))?;
    let write_result = file
        .write_all(line.as_bytes())
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_data());
    let _ = FileExt::unlock(&file);
    write_result.map_err(|error| format!("append {}: {error}", path.display()))
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

/// Remove a memory entry. Supports:
/// - exact id (`manual-...` or `auto-...`)
/// - the literal `last` → removes the most recent entry (manual OR auto)
/// - id prefix (≥8 chars) for convenience
/// - `file:line` format (e.g. `MEMORY.md:5`) for entries without embedded IDs
pub fn forget_memory(
    _config: &AgentConfig,
    _session: &AgentSession,
    id_or_last: &str,
) -> Result<Option<String>, String> {
    let root = ensure_memory_layout()?;

    // Handle `file:line` format IDs (e.g. "MEMORY.md:5", "memory_summary.md:3")
    if let Some((file_name, line_str)) = id_or_last.rsplit_once(':') {
        if let Ok(line_num) = line_str.parse::<usize>() {
            let allowed = ["MEMORY.md", "memory_summary.md"];
            if allowed.contains(&file_name) {
                let path = root.join(file_name);
                let original = fs::read_to_string(&path)
                    .map_err(|error| format!("read {}: {error}", path.display()))?;
                let mut lines = original.lines().map(str::to_string).collect::<Vec<_>>();
                // line_num is 1-based
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
        // exact match first, then prefix
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

fn extract_entry_id(line: &str) -> Option<String> {
    line.split("id: `")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .filter(|id| id.starts_with("manual-") || id.starts_with("auto-"))
        .map(str::to_string)
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
        // Skip files that are too large rather than OOM.
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
            // Prefer the embedded entry ID (e.g. `manual-...` / `auto-...`)
            // so that delete/patch APIs can locate the entry correctly.
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

fn file_len(path: PathBuf) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn dir_entry_count(path: PathBuf) -> usize {
    fs::read_dir(path)
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or(0)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn sanitize_id(input: &str) -> String {
    input
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn short_hash(input: &str) -> String {
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:08x}", hasher.finish() as u32)
}

fn sanitize_skill_name(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

// ---------------------------------------------------------------------------
// Retention sweep (P1)
// ---------------------------------------------------------------------------

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

    Ok(())
}

// ---------------------------------------------------------------------------
// Telemetry (P2)
// ---------------------------------------------------------------------------

/// Append a single JSONL telemetry line to `agent/memory/.telemetry.jsonl`.
///
/// Swallows all errors — telemetry must never break the memory hot path.
/// Callers get a cheap, cross-session local audit trail of memory behavior
/// without needing any external system.
fn telemetry_event(event: &str, value: u64, success: bool, detail: Option<&str>) {
    let root = memory_root();
    if fs::create_dir_all(&root).is_err() {
        return;
    }
    let path = root.join(".telemetry.jsonl");
    let line = serde_json::json!({
        "ts": now_secs(),
        "event": event,
        "value": value,
        "success": success,
        "detail": detail,
    });
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(&path)
    {
        if file.lock_exclusive().is_ok() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
            let _ = FileExt::unlock(&file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let mutex = LOCK.get_or_init(|| Mutex::new(()));
        // Recover from poisoned mutex so a single test panic doesn't cascade.
        mutex.lock().unwrap_or_else(|e| e.into_inner())
    }

    struct EnvGuard {
        old_agent_home: Option<String>,
        old_data_dir: Option<String>,
    }

    impl EnvGuard {
        fn set_agent_home(path: &Path) -> Self {
            let guard = Self {
                old_agent_home: std::env::var("BIFROST_AGENT_HOME").ok(),
                old_data_dir: std::env::var("BIFROST_DATA_DIR").ok(),
            };
            std::env::set_var("BIFROST_AGENT_HOME", path);
            std::env::remove_var("BIFROST_DATA_DIR");
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old_agent_home {
                Some(value) => std::env::set_var("BIFROST_AGENT_HOME", value),
                None => std::env::remove_var("BIFROST_AGENT_HOME"),
            }
            match &self.old_data_dir {
                Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
                None => std::env::remove_var("BIFROST_DATA_DIR"),
            }
        }
    }

    #[test]
    fn use_memories_disabled_short_circuits() {
        let _lock = env_lock();
        let config = AgentConfig {
            memories: Some(crate::config::MemoriesConfig {
                use_memories: Some(false),
                ..Default::default()
            }),
            ..AgentConfig::default()
        };
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        assert!(recall_system_message(&config, &AgentSession::new("s"), "hello").is_none());
    }

    #[test]
    fn memory_read_instructions_use_agent_memory_root() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        ensure_memory_layout().expect("layout");
        fs::write(
            memory_root().join("memory_summary.md"),
            "Bifrost prefers Codex-style memory loading.",
        )
        .expect("write summary");
        let prompt = build_memory_read_instructions().expect("prompt");
        assert!(prompt.contains("/memory/memory_summary.md"));
        assert!(prompt.contains("/memory/MEMORY.md"));
        assert!(prompt.contains("Bifrost prefers Codex-style memory loading."));
        assert!(prompt.contains("<oai-mem-citation>"));
    }

    #[test]
    fn empty_memory_summary_does_not_inject() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        ensure_memory_layout().expect("layout");
        assert!(build_memory_read_instructions().is_none());
    }

    #[test]
    fn remember_writes_codex_files_without_sqlite() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        let session = AgentSession::new("explicit");
        let record = remember_explicit(
            &AgentConfig::default(),
            &session,
            "Prefer Codex-style file memories",
        )
        .expect("remember");
        assert!(record.id.starts_with("manual-"));
        assert!(memory_root().join("MEMORY.md").exists());
        assert!(memory_root().join("memory_summary.md").exists());
        assert!(memory_root().join("raw_memories.md").exists());
        assert!(!memory_root().join("memories.sqlite").exists());
        let summary = fs::read_to_string(memory_root().join("memory_summary.md")).unwrap();
        assert!(summary.contains("Prefer Codex-style file memories"));
        let raw = fs::read_to_string(memory_root().join("raw_memories.md")).unwrap();
        assert!(raw.contains("source: user_explicit"));
        assert!(raw.contains("Prefer Codex-style file memories"));
    }

    #[test]
    fn parse_extracted_memories_accepts_json_and_dedupes() {
        let extracted = parse_extracted_memories(
            r#"{"memories":["Prefer file-backed memory", "Prefer file-backed memory", ""]}"#,
        );
        assert_eq!(extracted.memories, vec!["Prefer file-backed memory"]);
    }

    #[test]
    fn parse_extracted_memories_strips_markdown_fences() {
        let extracted =
            parse_extracted_memories("```json\n{\"memories\":[\"hello from fenced block\"]}\n```");
        assert_eq!(extracted.memories, vec!["hello from fenced block"]);
    }

    #[test]
    fn parse_extracted_memories_handles_prose_wrapped_json() {
        let content = "Here is the JSON you asked for:\n```\n{\n  \"memories\": [\"nested { brace } inside\"]\n}\n```\nHope this helps!";
        let extracted = parse_extracted_memories(content);
        assert_eq!(extracted.memories, vec!["nested { brace } inside"]);
    }

    #[test]
    fn parse_extracted_memories_codex_format_with_raw_memory() {
        let content = serde_json::json!({
            "rollout_summary": "# Fixed a bug\n\n## Task 1: bug fix\nOutcome: success",
            "rollout_slug": "fix-auth-bug",
            "raw_memory": "---\ndescription: Fixed auth bug\ntask: fix auth\n---\n### Task 1: fix auth\nReusable knowledge:\n- Check token expiry first"
        })
        .to_string();
        let extracted = parse_extracted_memories(&content);
        assert!(extracted.raw_memory.is_some());
        assert_eq!(extracted.rollout_slug.as_deref(), Some("fix-auth-bug"));
        assert!(extracted
            .rollout_summary
            .as_deref()
            .unwrap()
            .contains("Fixed a bug"));
        assert!(extracted
            .raw_memory
            .as_deref()
            .unwrap()
            .contains("Check token expiry"));
    }

    #[test]
    fn parse_extracted_memories_noop_returns_empty() {
        let content = r#"{"rollout_summary":"","rollout_slug":"","raw_memory":""}"#;
        let extracted = parse_extracted_memories(content);
        assert!(extracted.memories.is_empty());
        assert_eq!(extracted.raw_memory.as_deref(), Some(""));
        assert_eq!(extracted.rollout_slug.as_deref(), Some(""));
    }

    #[test]
    fn parse_consolidated_memory_handles_fenced_block() {
        let consolidated = parse_consolidated_memory(
            "```json\n{\"memory_summary\":\"s\",\"memory\":\"m\",\"skills\":[]}\n```",
        )
        .expect("parse");
        assert_eq!(consolidated.memory_summary, "s");
        assert_eq!(consolidated.memory, "m");
        assert!(consolidated.skills.is_empty());
    }

    #[test]
    fn parse_consolidated_memory_errors_on_garbage() {
        assert!(parse_consolidated_memory("not json at all").is_err());
    }

    #[test]
    fn remember_auto_writes_codex_files_without_sqlite() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        let session = AgentSession::new("auto-session");
        let record = remember_auto(
            &AgentConfig::default(),
            &session,
            "Auto memory survives sessions",
        )
        .expect("remember auto");
        assert!(record.id.starts_with("auto-"));
        let memory = fs::read_to_string(memory_root().join("MEMORY.md")).unwrap();
        assert!(memory.contains("source: auto_extract"));
        assert!(memory.contains("Auto memory survives sessions"));
        let summary = fs::read_to_string(memory_root().join("memory_summary.md")).unwrap();
        assert!(summary.contains("Auto memory survives sessions"));
        let raw = fs::read_to_string(memory_root().join("raw_memories.md")).unwrap();
        assert!(raw.contains("source: auto_extract"));
        assert!(raw.contains("rollout_summary_file:"));
        assert!(raw.contains("Auto memory survives sessions"));
        let rollout_summary_count = fs::read_dir(memory_root().join("rollout_summaries"))
            .unwrap()
            .filter_map(Result::ok)
            .count();
        assert_eq!(rollout_summary_count, 1);
        assert!(!memory_root().join("memories.sqlite").exists());
    }

    #[test]
    fn phase2_prompt_is_dirty_when_raw_memory_changes() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        let session = AgentSession::new("phase2-source");
        remember_auto(
            &AgentConfig::default(),
            &session,
            "Phase 2 should consolidate this",
        )
        .expect("remember auto");

        let first_hash = phase2_input_hash(&memory_root()).expect("first hash");
        let prompt = build_phase2_prompt(&memory_root()).expect("phase2 prompt");
        assert!(prompt.contains("raw_memories.md"));
        assert!(prompt.contains("Phase 2 should consolidate this"));

        remember_auto(&AgentConfig::default(), &session, "Another raw memory")
            .expect("remember second auto");
        let second_hash = phase2_input_hash(&memory_root()).expect("second hash");
        assert_ne!(first_hash, second_hash);
    }

    #[test]
    fn phase2_input_selection_uses_recent_bounded_inputs() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        let session = AgentSession::new("phase2-bounded");
        remember_auto(
            &AgentConfig::default(),
            &session,
            "old memory outside retention",
        )
        .expect("remember old");
        remember_auto(&AgentConfig::default(), &session, "recent memory one")
            .expect("remember one");
        remember_auto(&AgentConfig::default(), &session, "recent memory two")
            .expect("remember two");

        let input = build_phase2_input(&memory_root(), 2).expect("phase2 input");
        assert_eq!(input.processed_input_count, 2);
        assert_eq!(input.total_input_count, 3);
        assert!(input.has_more_inputs);
        let raw = fs::read_to_string(memory_root().join("raw_memories.md")).unwrap();
        let sections = parse_raw_memory_sections(&raw);
        let selected_raw = render_selected_raw_memories(&sections[sections.len() - 2..]);
        assert!(!selected_raw.contains("old memory outside retention"));
        assert!(input.prompt.contains("recent memory one"));
        assert!(input.prompt.contains("recent memory two"));

        let first_hash = input.input_hash;
        remember_auto(&AgentConfig::default(), &session, "newest memory included")
            .expect("remember newest");
        let second_hash = build_phase2_input(&memory_root(), 2)
            .expect("phase2 input")
            .input_hash;
        assert_ne!(first_hash, second_hash);
    }

    #[test]
    fn phase2_lock_prevents_concurrent_consolidation() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        let root = ensure_memory_layout().expect("layout");

        let first = Phase2LockGuard::try_acquire(&root)
            .expect("first lock")
            .expect("first acquired");
        assert!(
            Phase2LockGuard::try_acquire(&root)
                .expect("second lock")
                .is_none(),
            "second lock should be skipped while first is held"
        );
        drop(first);
        assert!(
            Phase2LockGuard::try_acquire(&root)
                .expect("third lock")
                .is_some(),
            "lock should be acquirable after guard drop"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn append_line_locks_concurrent_writers() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = Arc::new(temp.path().join("MEMORY.md"));
        let mut tasks = Vec::new();

        for task_id in 0..8usize {
            let path = Arc::clone(&path);
            tasks.push(tokio::task::spawn_blocking(move || {
                for line_id in 0..1000usize {
                    append_line(
                        &path,
                        &format!(
                            "{{\"task\":{task_id},\"line\":{line_id},\"content\":\"memory\"}}\n"
                        ),
                    )
                    .expect("append line");
                }
            }));
        }

        for task in tasks {
            task.await.expect("writer task");
        }

        let content = fs::read_to_string(path.as_ref()).expect("read memory");
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 8000);
        for line in lines {
            serde_json::from_str::<serde_json::Value>(line).expect("valid memory line");
        }
    }

    #[test]
    fn apply_consolidated_memory_rewrites_summary_memory_and_skills() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        let root = ensure_memory_layout().expect("layout");
        let consolidated = parse_consolidated_memory(
            r#"{
              "memory_summary": "- User prefers concise memory.",
              "memory": "- User prefers concise memory.\n  source: phase2_consolidated",
              "skills": [{
                "name": "Memory Skill!",
                "skill_md": "---\nname: memory-skill\n---\n# Skill\nUse concise memory."
              }]
            }"#,
        )
        .expect("parse consolidated memory");

        apply_consolidated_memory(&root, consolidated).expect("apply consolidated memory");

        let summary = fs::read_to_string(root.join("memory_summary.md")).unwrap();
        assert!(summary.contains("concise memory"));
        let memory = fs::read_to_string(root.join("MEMORY.md")).unwrap();
        assert!(memory.starts_with("# Memory"));
        assert!(memory.contains("phase2_consolidated"));
        // memory skills now live under skills/_memory/<name>/SKILL.md
        let skill = fs::read_to_string(
            root.join("skills")
                .join(MEMORY_SKILLS_SUBDIR)
                .join("memory-skill")
                .join("SKILL.md"),
        )
        .unwrap();
        assert!(skill.contains("Use concise memory."));
    }

    #[test]
    fn apply_consolidated_memory_preserves_user_authored_skill() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        let root = ensure_memory_layout().expect("layout");
        // pre-create a user-authored skill with the same name
        let user_skill_dir = root.join("skills").join("shared-name");
        fs::create_dir_all(&user_skill_dir).unwrap();
        fs::write(user_skill_dir.join("SKILL.md"), "user authored").unwrap();

        let consolidated = parse_consolidated_memory(
            r#"{
              "memory_summary": "",
              "memory": "",
              "skills": [{"name": "shared-name", "skill_md": "memory authored"}]
            }"#,
        )
        .unwrap();
        apply_consolidated_memory(&root, consolidated).unwrap();

        let user_content = fs::read_to_string(user_skill_dir.join("SKILL.md")).unwrap();
        assert_eq!(
            user_content, "user authored",
            "user skill must not be overwritten"
        );
        let memory_skill_path = root
            .join("skills")
            .join(MEMORY_SKILLS_SUBDIR)
            .join("shared-name")
            .join("SKILL.md");
        assert!(
            !memory_skill_path.exists(),
            "memory-skill should not shadow user skill"
        );
    }

    #[test]
    fn forget_memory_removes_auto_entries() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        let session = AgentSession::new("forget-auto");
        let record = remember_auto(&AgentConfig::default(), &session, "an auto entry").unwrap();
        let removed = forget_memory(&AgentConfig::default(), &session, &record.id).unwrap();
        assert_eq!(removed, Some(record.id.clone()));
        let memory = fs::read_to_string(memory_root().join("MEMORY.md")).unwrap();
        assert!(!memory.contains(&record.id));
    }

    #[test]
    fn forget_memory_last_matches_any_source() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        let session = AgentSession::new("forget-last");
        remember_explicit(&AgentConfig::default(), &session, "manual 1").unwrap();
        let last_auto = remember_auto(&AgentConfig::default(), &session, "auto last").unwrap();
        let removed = forget_memory(&AgentConfig::default(), &session, "last").unwrap();
        assert_eq!(removed, Some(last_auto.id));
    }

    #[test]
    fn replace_memory_rewrites_content_line() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        let session = AgentSession::new("replace");
        let record = remember_explicit(&AgentConfig::default(), &session, "original").unwrap();
        let replaced =
            replace_memory(&AgentConfig::default(), &session, &record.id, "updated").unwrap();
        assert_eq!(replaced, Some(record.id.clone()));
        let memory = fs::read_to_string(memory_root().join("MEMORY.md")).unwrap();
        assert!(memory.contains("content: updated"));
        assert!(!memory.contains("content: original"));
    }

    #[test]
    fn extract_balanced_json_handles_nested_braces_in_strings() {
        let content = "prefix {\"memories\":[\"{}\",\"a\"]} suffix";
        let block = extract_balanced_json(content).expect("found json");
        assert_eq!(block, "{\"memories\":[\"{}\",\"a\"]}");
    }

    #[test]
    fn phase2_failure_counter_opens_breaker() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        let root = ensure_memory_layout().expect("layout");
        for _ in 0..MEMORY_CONSOLIDATION_FAILURE_LIMIT {
            bump_phase2_failure("test");
        }
        let state = load_phase2_state(&root);
        assert_eq!(state.failure_count, MEMORY_CONSOLIDATION_FAILURE_LIMIT);
        assert!(state.pinned_failure_hash.is_some());
    }

    #[test]
    fn memory_stats_reports_phase2_counters() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        let stats = memory_stats().expect("stats");
        assert!(stats.memory_root.ends_with("memory"));
        assert_eq!(stats.phase2_failure_count, 0);
    }

    #[test]
    fn prune_memory_artifacts_caps_raw_memories() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        let session = AgentSession::new("prune-raw");
        for i in 0..40 {
            remember_auto(&AgentConfig::default(), &session, &format!("raw-{i}")).unwrap();
        }
        let config = AgentConfig {
            memories: Some(crate::config::MemoriesConfig {
                max_raw_memories_for_consolidation: Some(4),
                ..Default::default()
            }),
            ..AgentConfig::default()
        };
        prune_memory_artifacts(&config).expect("prune");
        let raw = fs::read_to_string(memory_root().join("raw_memories.md")).unwrap();
        let sections = parse_raw_memory_sections(&raw);
        // retention window = 4*4 = 16
        assert!(
            sections.len() <= 16,
            "expected bounded raw_memories, got {}",
            sections.len()
        );
        let archive = fs::read_to_string(memory_root().join("raw_memories.archive.md"))
            .expect("archive file created");
        assert!(archive.contains("Raw Memories (archive)"));
    }

    #[test]
    fn telemetry_event_writes_jsonl() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        ensure_memory_layout().expect("layout");
        telemetry_event("unit.test", 42, true, Some("hello"));
        let path = memory_root().join(".telemetry.jsonl");
        let content = fs::read_to_string(&path).expect("telemetry file");
        assert!(content.contains("\"event\":\"unit.test\""));
        assert!(content.contains("\"value\":42"));
    }

    #[test]
    fn approx_token_count_matches_codex_heuristic() {
        // 4 bytes per token
        assert_eq!(approx_token_count(""), 0);
        assert_eq!(approx_token_count("a"), 1); // ceil(1/4)
        assert_eq!(approx_token_count("abcd"), 1); // 4/4
        assert_eq!(approx_token_count("abcde"), 2); // ceil(5/4)
        assert_eq!(approx_token_count("abcdefgh"), 2); // 8/4
    }

    #[test]
    fn truncate_middle_approx_tokens_short_input_unchanged() {
        let text = "hello world"; // 11 bytes = ~3 tokens
        let result = truncate_middle_approx_tokens(text, 10);
        assert_eq!(result, text); // well under budget
    }

    #[test]
    fn truncate_middle_approx_tokens_long_input_truncated() {
        let text = "a".repeat(1000); // 1000 bytes = 250 tokens
        let result = truncate_middle_approx_tokens(&text, 50);
        assert!(result.len() < text.len());
        assert!(result.contains("tokens truncated"));
        // Should preserve start and end
        assert!(result.starts_with("aaa"));
        assert!(result.ends_with("aaa"));
    }

    #[test]
    fn truncate_middle_approx_tokens_empty_input() {
        assert_eq!(truncate_middle_approx_tokens("", 100), "");
    }

    #[test]
    fn truncate_middle_approx_tokens_zero_budget() {
        let result = truncate_middle_approx_tokens("hello", 0);
        assert!(result.contains("tokens truncated"));
    }

    #[test]
    fn write_codex_style_extraction_creates_artifacts() {
        let _lock = env_lock();
        let temp = tempfile::tempdir().expect("temp dir");
        let _guard = EnvGuard::set_agent_home(temp.path());
        ensure_memory_layout().expect("layout");
        let extracted = ExtractedMemories {
            memories: vec![],
            raw_memory: Some(
                "---\ndescription: test\ntask: fix\n---\n### Task 1\nReusable: check first"
                    .to_string(),
            ),
            rollout_summary: Some("# Fixed auth\n\n## Task 1: fix\nOutcome: success".to_string()),
            rollout_slug: Some("fix-auth-bug".to_string()),
        };
        write_codex_style_extraction("test-session", &extracted).expect("write");

        // Check rollout_summaries
        let rollout_path = memory_root()
            .join("rollout_summaries")
            .join("fix-auth-bug.md");
        assert!(rollout_path.exists());
        let rollout = fs::read_to_string(&rollout_path).unwrap();
        assert!(rollout.contains("session: test-session"));
        assert!(rollout.contains("Fixed auth"));

        // Check raw_memories.md
        let raw = fs::read_to_string(memory_root().join("raw_memories.md")).unwrap();
        assert!(raw.contains("## Session `test-session`"));
        assert!(raw.contains("rollout_slug: fix-auth-bug"));
        assert!(raw.contains("check first"));

        // Check memory_summary.md
        let summary = fs::read_to_string(memory_root().join("memory_summary.md")).unwrap();
        assert!(summary.contains("Fixed auth"));
    }

    #[test]
    fn parse_raw_memory_sections_handles_session_header() {
        let raw = "# Raw Memories\n\n## Session `s1`\nsource: auto_extract\nrollout_slug: slug1\n\ncontent1\n\n## Session `s2`\nsource: auto_extract\nrollout_slug: slug2\n\ncontent2\n\n";
        let sections = parse_raw_memory_sections(raw);
        assert_eq!(sections.len(), 2);
        assert!(sections[0].content.contains("content1"));
        assert!(sections[1].content.contains("content2"));
    }
}
