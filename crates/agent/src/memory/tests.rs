//! Tests for the memory subsystem.

use super::*;
use crate::config::AgentConfig;
use crate::session::AgentSession;
use consolidation::{
    apply_consolidated_memory, build_phase2_input, parse_raw_memory_sections,
    render_selected_raw_memories,
};
use extract::bump_phase2_failure;
use layout::{ensure_memory_layout, memory_root};
use lock::Phase2LockGuard;
use parse::{
    extract_balanced_json, parse_consolidated_memory, parse_extracted_memories,
    phase1_output_schema,
};
use read_path::build_memory_read_instructions;
use state_db;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use sub_agent::{execute_phase2_tool, phase2_agent_tools, sanitize_relative_path};
use telemetry::telemetry_event;
use types::{ExtractedMemories, Phase2State};
use utils::{append_line, approx_token_count, truncate_middle_approx_tokens};
use write::{forget_memory, memory_stats, remember_auto, remember_explicit, replace_memory};

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

    let first_hash = consolidation::phase2_input_hash(&memory_root()).expect("first hash");
    let prompt = consolidation::build_phase2_prompt(&memory_root()).expect("phase2 prompt");
    assert!(prompt.contains("raw_memories.md"));
    assert!(prompt.contains("Phase 2 should consolidate this"));

    remember_auto(&AgentConfig::default(), &session, "Another raw memory")
        .expect("remember second auto");
    let second_hash = consolidation::phase2_input_hash(&memory_root()).expect("second hash");
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
    remember_auto(&AgentConfig::default(), &session, "recent memory one").expect("remember one");
    remember_auto(&AgentConfig::default(), &session, "recent memory two").expect("remember two");

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
                    &format!("{{\"task\":{task_id},\"line\":{line_id},\"content\":\"memory\"}}\n"),
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
    let skill = fs::read_to_string(
        root.join("skills")
            .join(constants::MEMORY_SKILLS_SUBDIR)
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
        .join(constants::MEMORY_SKILLS_SUBDIR)
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
    for _ in 0..constants::MEMORY_CONSOLIDATION_FAILURE_LIMIT {
        bump_phase2_failure("test");
    }
    let state = consolidation::load_phase2_state(&root);
    assert_eq!(
        state.failure_count,
        constants::MEMORY_CONSOLIDATION_FAILURE_LIMIT
    );
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
    assert_eq!(approx_token_count(""), 0);
    assert_eq!(approx_token_count("a"), 1);
    assert_eq!(approx_token_count("abcd"), 1);
    assert_eq!(approx_token_count("abcde"), 2);
    assert_eq!(approx_token_count("abcdefgh"), 2);
}

#[test]
fn truncate_middle_approx_tokens_short_input_unchanged() {
    let text = "hello world";
    let result = truncate_middle_approx_tokens(text, 10);
    assert_eq!(result, text);
}

#[test]
fn truncate_middle_approx_tokens_long_input_truncated() {
    let text = "a".repeat(1000);
    let result = truncate_middle_approx_tokens(&text, 50);
    assert!(result.len() < text.len());
    assert!(result.contains("tokens truncated"));
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
            "---\ndescription: test\ntask: fix\n---\n### Task 1\nReusable: check first".to_string(),
        ),
        rollout_summary: Some("# Fixed auth\n\n## Task 1: fix\nOutcome: success".to_string()),
        rollout_slug: Some("fix-auth-bug".to_string()),
    };
    write::write_codex_style_extraction("test-session", &extracted).expect("write");

    let rollout_path = memory_root()
        .join("rollout_summaries")
        .join("fix-auth-bug.md");
    assert!(rollout_path.exists());
    let rollout = fs::read_to_string(&rollout_path).unwrap();
    assert!(rollout.contains("session: test-session"));
    assert!(rollout.contains("Fixed auth"));

    let raw = fs::read_to_string(memory_root().join("raw_memories.md")).unwrap();
    assert!(raw.contains("## Session `test-session`"));
    assert!(raw.contains("rollout_slug: fix-auth-bug"));
    assert!(raw.contains("check first"));

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

// -----------------------------------------------------------------------
// P2 feature tests
// -----------------------------------------------------------------------

#[test]
fn phase2_agent_tools_have_correct_definitions() {
    let tools = phase2_agent_tools();
    assert_eq!(tools.len(), 3);
    assert_eq!(tools[0].tool_type, "function");
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert_eq!(names, vec!["read_file", "write_file", "list_files"]);
}

#[test]
fn execute_phase2_tool_read_file_works() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    fs::write(root.join("test.txt"), "hello world").expect("write");
    let result = execute_phase2_tool(root, "read_file", r#"{"path":"test.txt"}"#);
    let parsed: serde_json::Value = serde_json::from_str(&result).expect("parse result");
    assert_eq!(parsed["content"].as_str(), Some("hello world"));
}

#[test]
fn execute_phase2_tool_write_file_works() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    let result = execute_phase2_tool(
        root,
        "write_file",
        r#"{"path":"sub/new.txt","content":"written content"}"#,
    );
    let parsed: serde_json::Value = serde_json::from_str(&result).expect("parse result");
    assert_eq!(parsed["success"].as_bool(), Some(true));
    let content = fs::read_to_string(root.join("sub/new.txt")).expect("read written file");
    assert_eq!(content, "written content");
}

#[test]
fn execute_phase2_tool_list_files_works() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    fs::write(root.join("a.txt"), "").unwrap();
    fs::write(root.join("b.txt"), "").unwrap();
    let result = execute_phase2_tool(root, "list_files", r#"{"path":""}"#);
    let parsed: serde_json::Value = serde_json::from_str(&result).expect("parse result");
    let files = parsed["files"].as_array().expect("files array");
    assert!(files.len() >= 2);
}

#[test]
fn sanitize_relative_path_prevents_traversal() {
    let root = Path::new("/tmp/memory");
    let safe = sanitize_relative_path(root, "../../../etc/passwd");
    assert_eq!(safe, Some(root.join("etc/passwd")));
    let safe2 = sanitize_relative_path(root, "a/../b/./c");
    assert_eq!(safe2, Some(root.join("a/b/c")));
    let safe3 = sanitize_relative_path(root, "/absolute/path");
    assert_eq!(safe3, Some(root.join("absolute/path")));
    // Empty path returns root
    let safe4 = sanitize_relative_path(root, "");
    assert_eq!(safe4, Some(root.to_path_buf()));
}

#[test]
fn execute_phase2_tool_unknown_returns_error() {
    let temp = tempfile::tempdir().expect("temp dir");
    let result = execute_phase2_tool(temp.path(), "delete_all", "{}");
    assert!(result.contains("unknown tool"));
}

#[test]
fn phase1_output_schema_matches_codex() {
    let schema = phase1_output_schema();
    assert_eq!(schema["type"].as_str(), Some("object"));
    let required = schema["required"].as_array().expect("required array");
    let mut required_keys: Vec<&str> = required
        .iter()
        .map(|v| v.as_str().expect("string"))
        .collect();
    required_keys.sort_unstable();
    assert_eq!(
        required_keys,
        vec!["raw_memory", "rollout_slug", "rollout_summary"]
    );
    let slug_type = schema["properties"]["rollout_slug"]["type"]
        .as_array()
        .expect("slug type array");
    let mut slug_types: Vec<&str> = slug_type
        .iter()
        .map(|v| v.as_str().expect("type string"))
        .collect();
    slug_types.sort_unstable();
    assert_eq!(slug_types, vec!["null", "string"]);
    assert_eq!(schema["additionalProperties"].as_bool(), Some(false));
}

#[test]
fn state_db_roundtrip() {
    let _lock = env_lock();
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    fs::create_dir_all(root).unwrap();
    let state = Phase2State {
        last_input_hash: "abc123".to_string(),
        processed_input_count: 5,
        total_input_count: 10,
        has_more_inputs: true,
        updated_at_unix: 1700000000,
        failure_count: 2,
        pinned_failure_hash: Some("pinned".to_string()),
        phase2_mode: "incremental".to_string(),
        pollution_state: Some("clean".to_string()),
    };
    state_db::save_phase2_state_to_db(root, &state).expect("save to sqlite");
    let loaded = state_db::load_phase2_state_from_db(root).expect("load from sqlite");
    assert_eq!(loaded.last_input_hash, "abc123");
    assert_eq!(loaded.processed_input_count, 5);
    assert_eq!(loaded.total_input_count, 10);
    assert!(loaded.has_more_inputs);
    assert_eq!(loaded.updated_at_unix, 1700000000);
    assert_eq!(loaded.failure_count, 2);
    assert_eq!(loaded.pinned_failure_hash.as_deref(), Some("pinned"));
    assert_eq!(loaded.phase2_mode, "incremental");
    assert_eq!(loaded.pollution_state.as_deref(), Some("clean"));
}

#[test]
fn state_db_load_save_used_in_load_phase2_state() {
    let _lock = env_lock();
    let temp = tempfile::tempdir().expect("temp dir");
    let _guard = EnvGuard::set_agent_home(temp.path());
    let root = ensure_memory_layout().expect("layout");
    // Initially empty — should return default state
    let default_state = consolidation::load_phase2_state(&root);
    assert_eq!(default_state.failure_count, 0);
    assert!(default_state.last_input_hash.is_empty());
    // Save and reload
    let state = Phase2State {
        last_input_hash: "hash42".to_string(),
        failure_count: 3,
        ..Default::default()
    };
    consolidation::save_phase2_state(&root, &state).expect("save");
    let loaded = consolidation::load_phase2_state(&root);
    assert_eq!(loaded.last_input_hash, "hash42");
    assert_eq!(loaded.failure_count, 3);
    // Also verify JSON file was written (backward compat)
    let json_path = root.join(".phase2_state.json");
    assert!(json_path.exists(), "JSON backward compat file should exist");
    let json_state: Phase2State =
        serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
    assert_eq!(json_state.last_input_hash, "hash42");
}
