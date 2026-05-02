//! Agent turn loop integration for Codex-style file-backed memories.
//!
//! Bifrost stores agent memories under `$BIFROST_DATA_DIR/agent/memory` and
//! injects Codex-compatible read-path instructions. The model decides when to
//! search `MEMORY.md`, rollout summaries, or memory skills. No database-backed
//! memory store is used.

use crate::config::{agent_home_dir, AgentConfig};
use crate::session::AgentSession;
use crate::types::ChatMessage;
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
const MEMORY_EXTRACT_USER_LIMIT_CHARS: usize = 6_000;
const MEMORY_EXTRACT_ASSISTANT_LIMIT_CHARS: usize = 6_000;
const MEMORY_CONSOLIDATION_INPUT_LIMIT_CHARS: usize = 60_000;
const DEFAULT_MAX_RAW_MEMORIES_FOR_CONSOLIDATION: usize = 512;
const PHASE2_LOCK_STALE_SECS: u64 = 10 * 60;
const AUTO_MEMORY_MAX_ITEMS: usize = 8;

const EXTRACT_SYSTEM_PROMPT: &str = r#"You extract durable memories from a Bifrost Agent conversation.

Return only JSON in this shape:
{"memories":["short durable fact", "..."]}

Rules:
- Include only stable preferences, project facts, recurring instructions, decisions, or facts likely to help future sessions.
- Exclude secrets, credentials, one-off transient details, and generic conversation summaries.
- If there is nothing durable, return {"memories":[]}.
"#;

const CONSOLIDATION_SYSTEM_PROMPT: &str = r##"You are the Bifrost memory consolidation agent.

You consolidate file-backed raw memories into durable, concise memory artifacts.

Return only JSON in this shape:
{
  "memory_summary": "short markdown summary for future read-path injection",
  "memory": "complete markdown content for MEMORY.md",
  "skills": [
    {"name": "optional-skill-name", "skill_md": "optional SKILL.md content"}
  ]
}

Rules:
- Preserve durable facts, stable preferences, project decisions, and reusable workflows.
- Remove duplicates and low-signal one-off details.
- Do not invent facts not supported by raw_memories.md or rollout_summaries.
- Do not include secrets, tokens, credentials, or large verbatim tool output.
- If there is no durable memory, return empty strings and an empty skills array.
- MEMORY.md content must be a complete markdown file and should start with a level-1 heading "Memory".
- memory_summary should be compact; it is injected into future model prompts.
"##;

const READ_PATH_TEMPLATE: &str = r#"## Memory

You have access to a memory folder with guidance from prior runs. It can save
time and help you stay consistent. Use it whenever it is likely to help.

Never update memories. You can only read them.

Decision boundary: should you use memory for a new user query?

- Skip memory ONLY when the request is clearly self-contained and does not need
  workspace history, conventions, or prior decisions.
- Hard skip examples: current time/date, simple translation, simple sentence
  rewrite, one-line shell command, trivial formatting.
- Use memory by default when ANY of these are true:
  - the query mentions workspace/repo/module/path/files in MEMORY_SUMMARY below,
  - the user asks for prior context / consistency / previous decisions,
  - the task is ambiguous and could depend on earlier project choices,
  - the ask is a non-trivial and related to MEMORY_SUMMARY below.
- If unsure, do a quick memory pass.

Memory layout (general -> specific):

- {{ base_path }}/memory_summary.md (already provided below; do NOT open again)
- {{ base_path }}/MEMORY.md (searchable registry; primary file to query)
- {{ base_path }}/skills/<skill-name>/ (skill folder)
  - SKILL.md (entrypoint instructions)
  - scripts/ (optional helper scripts)
  - examples/ (optional example outputs)
  - templates/ (optional templates)
- {{ base_path }}/rollout_summaries/ (per-rollout recaps + evidence snippets)
  - The paths of these entries can be found in {{ base_path }}/MEMORY.md or {{ base_path }}/rollout_summaries/ as `rollout_path`
  - These files are append-only `jsonl`: `session_meta.payload.id` identifies the session, `turn_context` marks turn boundaries, `event_msg` is the lightweight status stream, and `response_item` contains actual messages, tool calls, and tool outputs.
  - For efficient lookup, prefer matching the filename suffix or `session_meta.payload.id`; avoid broad full-content scans unless needed.

Quick memory pass (when applicable):

1. Skim the MEMORY_SUMMARY below and extract task-relevant keywords.
2. Search {{ base_path }}/MEMORY.md using those keywords.
3. Only if MEMORY.md directly points to rollout summaries/skills, open the 1-2
   most relevant files under {{ base_path }}/rollout_summaries/ or
   {{ base_path }}/skills/.
4. If above are not clear and you need exact commands, error text, or precise evidence, search over `rollout_path` for more evidence.
5. If there are no relevant hits, stop memory lookup and continue normally.

Quick-pass budget:

- Keep memory lookup lightweight: ideally <= 4-6 search steps before main work.
- Avoid broad scans of all rollout summaries.

During execution: if you hit repeated errors, confusing behavior, or suspect
relevant prior context, redo the quick memory pass.

How to decide whether to verify memory:

- Consider both risk of drift and verification effort.
- If a fact is likely to drift and is cheap to verify, verify it before
  answering.
- If a fact is likely to drift but verification is expensive, slow, or
  disruptive, it is acceptable to answer from memory in an interactive turn,
  but you should say that it is memory-derived, note that it may be stale, and
  consider offering to refresh it live.
- If a fact is lower-drift and cheap to verify, use judgment: verification is
  more important when the fact is central to the answer or especially easy to
  confirm.
- If a fact is lower-drift and expensive to verify, it is usually fine to
  answer from memory directly.

When answering from memory without current verification:

- If you rely on memory for a fact that you did not verify in the current turn,
  say so briefly in the final answer.
- If that fact is plausibly drift-prone or comes from an older note, older
  snapshot, or prior run summary, say that it may be stale or outdated.
- If live verification was skipped and a refresh would be useful in the
  interactive context, consider offering to verify or refresh it live.
- Do not present unverified memory-derived facts as confirmed-current.
- For interactive requests, prefer a short refresh offer over silently doing
  expensive verification that the user did not ask for.
- When the unverified fact is about prior results, commands, timing, or an
  older snapshot, a concrete refresh offer can be especially helpful.

Memory citation requirements:

- If ANY relevant memory files were used: append exactly one
`<oai-mem-citation>` block as the VERY LAST content of the final reply.
  Normal responses should include the answer first, then append the
`<oai-mem-citation>` block at the end.
- Use this exact structure for programmatic parsing:
```
<oai-mem-citation>
<citation_entries>
MEMORY.md:234-236|note=[responsesapi citation extraction code pointer]
rollout_summaries/2026-02-17T21-23-02-LN3m-weekly_memory_report_pivot_from_git_history.md:10-12|note=[weekly report format]
</citation_entries>
<rollout_ids>
019c6e27-e55b-73d1-87d8-4e01f1f75043
019c7714-3b77-74d1-9866-e1f484aae2ab
</rollout_ids>
</oai-mem-citation>
```
- `citation_entries` is for rendering:
  - one citation entry per line
  - format: `<file>:<line_start>-<line_end>|note=[<how memory was used>]`
  - use file paths relative to the memory base path (for example, `MEMORY.md`,
    `rollout_summaries/...`, `skills/...`)
  - only cite files actually used under the memory base path (do not cite
    workspace files as memory citations)
  - if you used `MEMORY.md` and then a rollout summary/skill file, cite both
  - list entries in order of importance (most important first)
  - `note` should be short, single-line, and use simple characters only (avoid
    unusual symbols, no newlines)
- `rollout_ids` is for us to track what previous rollouts you find useful:
  - include one rollout id per line
  - rollout ids should look like UUIDs (for example,
    `019c6e27-e55b-73d1-87d8-4e01f1f75043`)
  - include unique ids only; do not repeat ids
  - an empty `<rollout_ids>` section is allowed if no rollout ids are available
  - you can find rollout ids in rollout summary files and MEMORY.md
  - do not include file paths or notes in this section
  - For every `citation_entries`, try to find and cite the corresponding rollout id if possible
- Never include memory citations inside pull-request messages.
- Never cite blank lines; double-check ranges.

========= MEMORY_SUMMARY BEGINS =========
{{ memory_summary }}
========= MEMORY_SUMMARY ENDS =========

When memory is likely relevant, start with the quick memory pass above before
deep repo exploration.
"#;

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
    pub rollout_summary_count: usize,
    pub skill_count: usize,
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
            return None;
        }
    };
    let summary_path = root.join("memory_summary.md");
    if bifrost_core::text::check_file_size(&summary_path, MAX_MEMORY_FILE_BYTES).is_err() {
        warn!(path = %summary_path.display(), "memory summary file too large, skipping");
        return None;
    }
    let summary = fs::read_to_string(&summary_path).ok()?.trim().to_string();
    if summary.is_empty() {
        return None;
    }
    let summary = truncate_chars(&summary, MEMORY_SUMMARY_TOKEN_LIMIT_CHARS);
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
    memories: Vec<String>,
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

struct Phase2LockGuard {
    path: PathBuf,
    acquired: bool,
}

impl Drop for Phase2LockGuard {
    fn drop(&mut self) {
        if self.acquired {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl Phase2LockGuard {
    fn try_acquire(root: &Path) -> Result<Option<Self>, String> {
        let path = root.join(".phase2.lock");
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                writeln!(file, "pid={}", std::process::id())
                    .map_err(|error| format!("write {}: {error}", path.display()))?;
                writeln!(file, "created_at_unix={}", now_secs())
                    .map_err(|error| format!("write {}: {error}", path.display()))?;
                Ok(Some(Self {
                    path,
                    acquired: true,
                }))
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if phase2_lock_is_stale(&path) {
                    let _ = fs::remove_file(&path);
                    return Self::try_acquire(root);
                }
                Ok(None)
            }
            Err(error) => Err(format!("create {}: {error}", path.display())),
        }
    }
}

/// Generate durable file-backed memories after a turn.
pub async fn auto_extract_after_turn(
    client: &crate::client::AgentClient,
    config: &AgentConfig,
    session: &AgentSession,
    user_message: &str,
    assistant_message: &str,
) {
    if !generate_memories_enabled(config) {
        return;
    }
    if let Err(error) =
        auto_extract_after_turn_inner(client, config, session, user_message, assistant_message)
            .await
    {
        warn!(error = %error, "failed to generate file-backed memories");
    }
}

async fn auto_extract_after_turn_inner(
    client: &crate::client::AgentClient,
    config: &AgentConfig,
    session: &AgentSession,
    user_message: &str,
    assistant_message: &str,
) -> Result<(), String> {
    ensure_memory_layout()?;

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

    let prompt = format!(
        "Session: {}\n\nUser message:\n{}\n\nAssistant response:\n{}",
        session.session_key,
        truncate_chars(user_message, MEMORY_EXTRACT_USER_LIMIT_CHARS),
        truncate_chars(assistant_message, MEMORY_EXTRACT_ASSISTANT_LIMIT_CHARS)
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
    let memories = parse_extracted_memories(&content);
    let mut wrote_memory = false;
    for memory in memories.into_iter().take(AUTO_MEMORY_MAX_ITEMS) {
        remember_auto(config, session, &memory)?;
        wrote_memory = true;
    }
    if wrote_memory {
        run_phase2_consolidation(client, config).await?;
    }
    Ok(())
}

fn parse_extracted_memories(content: &str) -> Vec<String> {
    let trimmed = content.trim();
    let parsed = serde_json::from_str::<ExtractedMemories>(trimmed)
        .or_else(|_| {
            extract_json_object(trimmed)
                .ok_or_else(|| serde_json::Error::io(std::io::Error::other("missing json")))
                .and_then(serde_json::from_str::<ExtractedMemories>)
        })
        .map(|payload| payload.memories)
        .unwrap_or_else(|_| parse_memory_lines(trimmed));

    let mut output = Vec::new();
    for memory in parsed {
        let memory = normalize_memory_line(&memory);
        if memory.is_empty() || output.iter().any(|existing| existing == &memory) {
            continue;
        }
        output.push(memory);
    }
    output
}

fn extract_json_object(content: &str) -> Option<&str> {
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    (start <= end).then(|| &content[start..=end])
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

fn remember_auto(
    _config: &AgentConfig,
    session: &AgentSession,
    content: &str,
) -> Result<MemoryFileEntry, String> {
    let root = ensure_memory_layout()?;
    let content = content.trim();
    if content.is_empty() {
        return Err("memory content must not be empty".to_string());
    }
    let id = format!("auto-{}-{}", now_secs(), sanitize_id(&session.session_key));
    let memory_path = root.join("MEMORY.md");
    append_line(
        &memory_path,
        &format!(
            "\n- id: `{id}`\n  source: auto_extract\n  session: `{}`\n  content: {}\n",
            session.session_key, content
        ),
    )?;
    let summary_path = root.join("memory_summary.md");
    append_line(&summary_path, &format!("- {content}\n"))?;
    append_raw_memory_artifacts(
        &root,
        &id,
        "auto_extract",
        &session.session_key,
        content,
        true,
    )?;
    info!(memory_id = %id, "auto memory extracted");
    Ok(MemoryFileEntry {
        id,
        path: "MEMORY.md".to_string(),
        content: content.to_string(),
    })
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
    Ok(MemoryFileEntry {
        id,
        path: "MEMORY.md".to_string(),
        content: content.to_string(),
    })
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
    let root = ensure_memory_layout()?;
    let Some(_lock) = Phase2LockGuard::try_acquire(&root)? else {
        info!(memory_root = %root.display(), "phase-2 memory consolidation skipped because another worker holds the lock");
        return Ok(());
    };
    let input = build_phase2_input(&root, phase2_input_limit(config))?;
    let state = load_phase2_state(&root);
    if state.last_input_hash == input.input_hash {
        return Ok(());
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

    let response = client
        .chat_completion(
            &consolidation_config,
            &[
                ChatMessage::system(CONSOLIDATION_SYSTEM_PROMPT),
                ChatMessage::user(&input.prompt),
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
        },
    )?;
    info!(memory_root = %root.display(), "phase-2 memory consolidation completed");
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
        prompt: truncate_chars(&prompt, MEMORY_CONSOLIDATION_INPUT_LIMIT_CHARS),
        processed_input_count,
        total_input_count,
        has_more_inputs,
    })
}

fn parse_raw_memory_sections(raw: &str) -> Vec<RawMemorySection> {
    let mut sections = Vec::new();
    let mut current = String::new();
    for line in raw.lines() {
        if line.starts_with("## Memory `")
            && !current.trim().is_empty()
            && current.trim() != "# Raw Memories"
        {
            sections.push(raw_memory_section_from_content(std::mem::take(
                &mut current,
            )));
        } else if line.starts_with("## Memory `") && current.trim() == "# Raw Memories" {
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
    let trimmed = content.trim();
    let consolidated = serde_json::from_str::<ConsolidatedMemory>(trimmed)
        .or_else(|_| {
            extract_json_object(trimmed)
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
    for skill in consolidated.skills {
        let name = sanitize_skill_name(&skill.name);
        let skill_md = skill.skill_md.trim();
        if name.is_empty() || skill_md.is_empty() {
            continue;
        }
        let dir = root.join("skills").join(&name);
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

fn phase2_lock_is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > Duration::from_secs(PHASE2_LOCK_STALE_SECS))
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
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn append_line(path: &Path, line: &str) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    file.write_all(line.as_bytes())
        .map_err(|error| format!("append {}: {error}", path.display()))
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

/// Remove a memory line by id. File-backed memories are append-oriented, so `last` removes the last
/// explicit `id:` block from MEMORY.md and leaves summary history intact.
pub fn forget_memory(
    _config: &AgentConfig,
    _session: &AgentSession,
    id_or_last: &str,
) -> Result<Option<String>, String> {
    let root = ensure_memory_layout()?;
    let path = root.join("MEMORY.md");
    let original =
        fs::read_to_string(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let mut lines = original.lines().map(str::to_string).collect::<Vec<_>>();
    let target = if id_or_last == "last" {
        lines.iter().rev().find_map(|line| extract_manual_id(line))
    } else {
        Some(id_or_last.to_string())
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
    Ok(Some(target))
}

fn extract_manual_id(line: &str) -> Option<String> {
    line.split("id: `")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
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
            entries.push(MemoryFileEntry {
                id: format!("{file_name}:{}", idx + 1),
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
    Ok(MemoryFileStats {
        memory_summary_bytes: file_len(root.join("memory_summary.md")),
        memory_md_bytes: file_len(root.join("MEMORY.md")),
        rollout_summary_count: dir_entry_count(root.join("rollout_summaries")),
        skill_count: dir_entry_count(root.join("skills")),
        memory_root: root.display().to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
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
        let memories = parse_extracted_memories(
            r#"{"memories":["Prefer file-backed memory", "Prefer file-backed memory", ""]}"#,
        );
        assert_eq!(memories, vec!["Prefer file-backed memory"]);
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
        let skill = fs::read_to_string(root.join("skills/memory-skill/SKILL.md")).unwrap();
        assert!(skill.contains("Use concise memory."));
    }
}
