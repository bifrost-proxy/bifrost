use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{DateTime, SecondsFormat, Utc};

use super::{CLAUDE_CODE_ADAPTER, DEFAULT_ADAPTER, TRAEX_ADAPTER};

const DEFAULT_LIST_LIMIT: usize = 20;
const MAX_DISCOVERED_FILES: usize = 50_000;
const MAX_SESSION_SCAN_BYTES: u64 = 4 * 1024 * 1024;
const MAX_INDEX_SCAN_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TITLE_CHARS: usize = 96;
const MIN_ID_PREFIX_CHARS: usize = 8;

#[cfg(test)]
pub(crate) fn local_session_test_env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalExternalSession {
    pub id: String,
    pub title: String,
    pub datetime: String,
    pub updated_at_millis: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalCliResumeSlashCommand {
    List,
    Pick(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSessionSelectionContext {
    pub session_key: String,
    pub runner_id: String,
}

#[derive(Clone, Debug, Default)]
struct SessionHints {
    title: Option<String>,
    updated_at_millis: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderKind {
    Codex,
    Traex,
    ClaudeCode,
}

pub fn supports_external_cli_resume_slash(adapter: &str) -> bool {
    provider_kind(adapter).is_some()
}

pub fn parse_external_cli_resume_slash_command(
    message: &str,
) -> Option<Result<ExternalCliResumeSlashCommand, String>> {
    let trimmed = message.trim();
    let mut parts = trimmed.split_whitespace();
    let command = parts.next()?;
    if !command.eq_ignore_ascii_case("/resume") {
        return None;
    }
    let Some(id) = parts.next() else {
        return Some(Ok(ExternalCliResumeSlashCommand::List));
    };
    if parts.next().is_some() {
        return Some(Err("用法: /resume [session-id]".to_string()));
    }
    if id.len() > 128
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
    {
        return Some(Err(
            "session id 只能包含字母、数字、短横线、下划线、点或冒号，且不能超过 128 个字符。"
                .to_string(),
        ));
    }
    Some(Ok(ExternalCliResumeSlashCommand::Pick(id.to_string())))
}

pub fn discover_local_sessions(
    adapter: &str,
    limit: Option<usize>,
) -> Result<Vec<LocalExternalSession>, String> {
    let provider = provider_kind(adapter).ok_or_else(|| {
        format!(
            "adapter `{}` does not support local session resume",
            adapter.trim()
        )
    })?;
    let root = provider_home(provider);
    let mut sessions = match provider {
        ProviderKind::Codex => discover_codex_like_sessions(
            &root.join("sessions"),
            load_codex_index(&root.join("session_index.jsonl")),
        ),
        ProviderKind::Traex => discover_codex_like_sessions(
            &root.join("cli").join("sessions"),
            load_prompt_history(
                &root.join("cli").join("history.jsonl"),
                "session_id",
                "text",
                "ts",
            ),
        ),
        ProviderKind::ClaudeCode => discover_claude_sessions(
            &root.join("projects"),
            load_prompt_history(
                &root.join("history.jsonl"),
                "sessionId",
                "display",
                "timestamp",
            ),
        ),
    };
    sessions.sort_by(|left, right| {
        right
            .updated_at_millis
            .cmp(&left.updated_at_millis)
            .then_with(|| right.id.cmp(&left.id))
    });
    sessions.truncate(
        limit
            .unwrap_or(DEFAULT_LIST_LIMIT)
            .min(MAX_DISCOVERED_FILES),
    );
    Ok(sessions)
}

pub fn resolve_local_session(
    adapter: &str,
    requested_id: &str,
) -> Result<LocalExternalSession, String> {
    let requested_id = requested_id.trim();
    let sessions = discover_local_sessions(adapter, Some(MAX_DISCOVERED_FILES))?;
    if let Some(session) = sessions.iter().find(|session| session.id == requested_id) {
        return Ok(session.clone());
    }
    if requested_id.chars().count() < MIN_ID_PREFIX_CHARS {
        return Err(format!(
            "未找到完整 session id `{requested_id}`；使用前缀时请至少输入 {MIN_ID_PREFIX_CHARS} 个字符。"
        ));
    }
    let mut matches = sessions
        .into_iter()
        .filter(|session| session.id.starts_with(requested_id));
    let Some(first) = matches.next() else {
        return Err(format!(
            "本地没有找到 session `{requested_id}`。请先发送 /resume 查看最近会话。"
        ));
    };
    if matches.next().is_some() {
        return Err(format!(
            "session 前缀 `{requested_id}` 匹配多条会话，请使用更长或完整的 id。"
        ));
    }
    Ok(first)
}

pub fn format_local_session_list(adapter: &str, sessions: &[LocalExternalSession]) -> String {
    let label = match provider_kind(adapter) {
        Some(ProviderKind::Codex) => "Codex",
        Some(ProviderKind::Traex) => "Traex",
        Some(ProviderKind::ClaudeCode) => "Claude Code",
        None => adapter.trim(),
    };
    if sessions.is_empty() {
        return format!("没有找到 {label} 本地 session。");
    }
    let mut output = format!("最近 {} 个 {label} 本地 session：\n", sessions.len());
    for (index, session) in sessions.iter().enumerate() {
        output.push_str(&format!(
            "\n{}. {} / {} / {}",
            index + 1,
            session.id,
            session.title,
            session.datetime
        ));
    }
    output.push_str("\n\n发送 `/resume <id>` 选择；也可以使用至少 8 位的唯一 id 前缀。");
    output
}

pub fn persist_local_session_selection(
    session_key: &str,
    adapter: &str,
    runner_id: &str,
    session: &LocalExternalSession,
) -> Result<(), String> {
    crate::im_gateway::session_state::upsert_session_state(
        session_key,
        adapter,
        Some(runner_id),
        |state| {
            state.external_thread_id = Some(session.id.clone());
            state.external_conversation_id = None;
        },
    )
    .map(|_| ())
}

pub async fn execute_local_session_resume_command(
    adapter: String,
    command: ExternalCliResumeSlashCommand,
    selection: Option<LocalSessionSelectionContext>,
) -> Result<String, String> {
    if !supports_external_cli_resume_slash(&adapter) {
        return Err("/resume 当前仅支持 Codex、Traex 或 Claude Code Runner。".to_string());
    }
    tokio::task::spawn_blocking(move || match command {
        ExternalCliResumeSlashCommand::List => {
            let sessions = discover_local_sessions(&adapter, Some(DEFAULT_LIST_LIMIT))?;
            Ok(format_local_session_list(&adapter, &sessions))
        }
        ExternalCliResumeSlashCommand::Pick(id) => {
            let selection = selection.ok_or_else(|| {
                "sessionKey is required to persist /resume selection".to_string()
            })?;
            let session = resolve_local_session(&adapter, &id)?;
            persist_local_session_selection(
                &selection.session_key,
                &adapter,
                &selection.runner_id,
                &session,
            )?;
            Ok(format!(
                "已选择本地 session：\n- id: `{}`\n- title: {}\n- datetime: {}\n\n下一条普通消息将恢复此会话。",
                session.id, session.title, session.datetime
            ))
        }
    })
    .await
    .map_err(|error| format!("读取本地 session 的后台任务失败：{error}"))?
}

fn provider_kind(adapter: &str) -> Option<ProviderKind> {
    match adapter.trim() {
        DEFAULT_ADAPTER => Some(ProviderKind::Codex),
        TRAEX_ADAPTER => Some(ProviderKind::Traex),
        CLAUDE_CODE_ADAPTER => Some(ProviderKind::ClaudeCode),
        _ => None,
    }
}

fn provider_home(provider: ProviderKind) -> PathBuf {
    match provider {
        ProviderKind::Codex => std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| bifrost_agent::config::user_home_dir().join(".codex")),
        ProviderKind::Traex => std::env::var_os("TRAE_HOME")
            .or_else(|| std::env::var_os("TRAEX_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| bifrost_agent::config::user_home_dir().join(".trae")),
        ProviderKind::ClaudeCode => std::env::var_os("CLAUDE_CONFIG_DIR")
            .or_else(|| std::env::var_os("CLAUDE_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| bifrost_agent::config::user_home_dir().join(".claude")),
    }
}

fn discover_codex_like_sessions(
    sessions_root: &Path,
    hints: HashMap<String, SessionHints>,
) -> Vec<LocalExternalSession> {
    let mut sessions = HashMap::<String, LocalExternalSession>::new();
    for path in collect_jsonl_files(sessions_root) {
        let fallback_millis = file_modified_millis(&path);
        let Some((id, event_millis, fallback_title)) = read_codex_like_session_meta(&path) else {
            continue;
        };
        let hint = hints.get(&id);
        let updated_at_millis = hint
            .map(|hint| hint.updated_at_millis)
            .unwrap_or_default()
            .max(event_millis)
            .max(fallback_millis);
        let title = hint
            .and_then(|hint| hint.title.clone())
            .or(fallback_title)
            .unwrap_or_else(|| "Untitled session".to_string());
        insert_latest(
            &mut sessions,
            LocalExternalSession {
                id,
                title: clean_title(&title),
                datetime: format_datetime(updated_at_millis),
                updated_at_millis,
            },
        );
    }
    sessions.into_values().collect()
}

fn discover_claude_sessions(
    sessions_root: &Path,
    hints: HashMap<String, SessionHints>,
) -> Vec<LocalExternalSession> {
    let mut sessions = HashMap::<String, LocalExternalSession>::new();
    for path in collect_jsonl_files(sessions_root) {
        let fallback_millis = file_modified_millis(&path);
        let Some((id, event_millis, ai_title, fallback_title)) = read_claude_session_meta(&path)
        else {
            continue;
        };
        let hint = hints.get(&id);
        let updated_at_millis = hint
            .map(|hint| hint.updated_at_millis)
            .unwrap_or_default()
            .max(event_millis)
            .max(fallback_millis);
        let title = ai_title
            .or_else(|| hint.and_then(|hint| hint.title.clone()))
            .or(fallback_title)
            .unwrap_or_else(|| "Untitled session".to_string());
        insert_latest(
            &mut sessions,
            LocalExternalSession {
                id,
                title: clean_title(&title),
                datetime: format_datetime(updated_at_millis),
                updated_at_millis,
            },
        );
    }
    sessions.into_values().collect()
}

fn read_codex_like_session_meta(path: &Path) -> Option<(String, u64, Option<String>)> {
    let mut id = None;
    let mut updated_at_millis = 0;
    let mut title = None;
    visit_json_lines(path, MAX_SESSION_SCAN_BYTES, |value| {
        updated_at_millis = updated_at_millis.max(timestamp_from_value(
            value.get("timestamp").unwrap_or(&serde_json::Value::Null),
        ));
        let Some(payload) = value.get("payload") else {
            return;
        };
        if value.get("type").and_then(serde_json::Value::as_str) == Some("session_meta") {
            id = json_string(payload, "session_id").or_else(|| json_string(payload, "id"));
            updated_at_millis = updated_at_millis.max(timestamp_from_value(
                payload.get("timestamp").unwrap_or(&serde_json::Value::Null),
            ));
        }
        if title.is_none()
            && payload.get("type").and_then(serde_json::Value::as_str) == Some("message")
            && payload.get("role").and_then(serde_json::Value::as_str) == Some("user")
        {
            title = extract_message_text(payload.get("content"));
        }
    });
    Some((id?, updated_at_millis, title))
}

fn read_claude_session_meta(path: &Path) -> Option<(String, u64, Option<String>, Option<String>)> {
    let mut id = None;
    let mut updated_at_millis = 0;
    let mut ai_title = None;
    let mut fallback_title = None;
    visit_json_lines(path, MAX_SESSION_SCAN_BYTES, |value| {
        id = id
            .take()
            .or_else(|| json_string(value, "sessionId"))
            .or_else(|| json_string(value, "session_id"));
        updated_at_millis = updated_at_millis.max(timestamp_from_value(
            value.get("timestamp").unwrap_or(&serde_json::Value::Null),
        ));
        if value.get("type").and_then(serde_json::Value::as_str) == Some("ai-title") {
            ai_title = json_string(value, "aiTitle").or_else(|| json_string(value, "title"));
        }
        if fallback_title.is_none()
            && value.get("type").and_then(serde_json::Value::as_str) == Some("user")
        {
            fallback_title = extract_message_text(value.get("message"));
        }
    });
    let id = id.or_else(|| {
        path.file_stem()
            .and_then(|value| value.to_str())
            .map(str::to_string)
    })?;
    Some((id, updated_at_millis, ai_title, fallback_title))
}

fn load_codex_index(path: &Path) -> HashMap<String, SessionHints> {
    let mut hints = HashMap::new();
    visit_json_lines(path, MAX_INDEX_SCAN_BYTES, |value| {
        let Some(id) = json_string(value, "id") else {
            return;
        };
        let candidate = SessionHints {
            title: json_string(value, "thread_name").map(|title| clean_title(&title)),
            updated_at_millis: timestamp_from_value(
                value.get("updated_at").unwrap_or(&serde_json::Value::Null),
            ),
        };
        merge_hint(&mut hints, id, candidate);
    });
    hints
}

fn load_prompt_history(
    path: &Path,
    id_key: &str,
    title_key: &str,
    timestamp_key: &str,
) -> HashMap<String, SessionHints> {
    let mut hints = HashMap::new();
    visit_json_lines(path, MAX_INDEX_SCAN_BYTES, |value| {
        let Some(id) = json_string(value, id_key) else {
            return;
        };
        let timestamp =
            timestamp_from_value(value.get(timestamp_key).unwrap_or(&serde_json::Value::Null));
        let title = json_string(value, title_key).map(|title| clean_title(&title));
        let entry = hints.entry(id).or_insert_with(SessionHints::default);
        if entry.title.is_none() {
            entry.title = title;
        }
        entry.updated_at_millis = entry.updated_at_millis.max(timestamp);
    });
    hints
}

fn merge_hint(hints: &mut HashMap<String, SessionHints>, id: String, candidate: SessionHints) {
    let entry = hints.entry(id).or_default();
    if candidate.updated_at_millis >= entry.updated_at_millis {
        if candidate.title.is_some() {
            entry.title = candidate.title;
        }
        entry.updated_at_millis = candidate.updated_at_millis;
    }
}

fn insert_latest(
    sessions: &mut HashMap<String, LocalExternalSession>,
    candidate: LocalExternalSession,
) {
    let replace = sessions
        .get(&candidate.id)
        .is_none_or(|existing| candidate.updated_at_millis >= existing.updated_at_millis);
    if replace {
        sessions.insert(candidate.id.clone(), candidate);
    }
}

fn collect_jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if files.len() >= MAX_DISCOVERED_FILES {
                return files;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file()
                && path.extension().and_then(|value| value.to_str()) == Some("jsonl")
            {
                files.push(path);
            }
        }
    }
    files
}

fn visit_json_lines(path: &Path, max_bytes: u64, mut visit: impl FnMut(&serde_json::Value)) {
    let Ok(file) = File::open(path) else {
        return;
    };
    let reader = BufReader::new(file).take(max_bytes);
    for line in reader.lines().map_while(Result::ok) {
        if line.len() > 1024 * 1024 {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
            visit(&value);
        }
    }
}

fn extract_message_text(value: Option<&serde_json::Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        return nonempty(text);
    }
    if let Some(object) = value.as_object() {
        if let Some(text) = object
            .get("text")
            .or_else(|| object.get("content"))
            .and_then(serde_json::Value::as_str)
        {
            return nonempty(text);
        }
        return extract_message_text(object.get("content"));
    }
    value.as_array()?.iter().find_map(|item| {
        item.as_str()
            .and_then(nonempty)
            .or_else(|| {
                item.get("text")
                    .and_then(serde_json::Value::as_str)
                    .and_then(nonempty)
            })
            .or_else(|| {
                item.get("content")
                    .and_then(serde_json::Value::as_str)
                    .and_then(nonempty)
            })
    })
}

fn json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().and_then(nonempty)
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn clean_title(value: &str) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = collapsed.chars();
    let mut output = chars.by_ref().take(MAX_TITLE_CHARS).collect::<String>();
    if chars.next().is_some() {
        output.push('…');
    }
    if output.is_empty() {
        "Untitled session".to_string()
    } else {
        output
    }
}

fn timestamp_from_value(value: &serde_json::Value) -> u64 {
    if let Some(value) = value.as_u64() {
        return normalize_epoch(value);
    }
    if let Some(value) = value.as_i64().and_then(|value| u64::try_from(value).ok()) {
        return normalize_epoch(value);
    }
    value
        .as_str()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .and_then(|value| u64::try_from(value.timestamp_millis()).ok())
        .unwrap_or_default()
}

fn normalize_epoch(value: u64) -> u64 {
    if value < 10_000_000_000 {
        value.saturating_mul(1000)
    } else {
        value
    }
}

fn file_modified_millis(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

fn format_datetime(timestamp_millis: u64) -> String {
    i64::try_from(timestamp_millis)
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct EnvGuard {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let old = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, old }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(old) = self.old.take() {
                std::env::set_var(self.key, old);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    fn write_lines(path: &Path, values: &[serde_json::Value]) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        let mut file = File::create(path).expect("create fixture");
        for value in values {
            writeln!(file, "{}", serde_json::to_string(value).expect("json")).expect("write");
        }
    }

    #[test]
    fn resume_slash_parser_supports_list_pick_and_rejects_invalid_input() {
        assert_eq!(
            parse_external_cli_resume_slash_command(" /resume "),
            Some(Ok(ExternalCliResumeSlashCommand::List))
        );
        assert_eq!(
            parse_external_cli_resume_slash_command("/RESUME abc-def"),
            Some(Ok(ExternalCliResumeSlashCommand::Pick(
                "abc-def".to_string()
            )))
        );
        assert!(matches!(
            parse_external_cli_resume_slash_command("/resume a b"),
            Some(Err(_))
        ));
        assert!(matches!(
            parse_external_cli_resume_slash_command("/resume ../../secret"),
            Some(Err(_))
        ));
        assert!(matches!(
            parse_external_cli_resume_slash_command(&format!("/resume {}", "a".repeat(129))),
            Some(Err(_))
        ));
        assert_eq!(parse_external_cli_resume_slash_command("/resume-ish"), None);
    }

    #[tokio::test]
    async fn resume_executes_list_and_pick_off_thread_and_persists_the_selection() {
        let _lock = local_session_test_env_lock().lock().await;
        let home = tempfile::tempdir().expect("home tempdir");
        let data_dir = tempfile::tempdir().expect("data tempdir");
        let _home_env = EnvGuard::set("CODEX_HOME", home.path());
        let _data_guard = crate::test_env::BifrostDataDirGuard::set(data_dir.path());
        let id = "cccccccc-0000-0000-0000-000000000004";
        write_lines(
            &home.path().join("sessions/session.jsonl"),
            &[serde_json::json!({
                "timestamp": "2026-08-07T03:04:05Z",
                "type": "session_meta",
                "payload": {"id": id}
            })],
        );

        let listed = execute_local_session_resume_command(
            "codex".to_string(),
            ExternalCliResumeSlashCommand::List,
            None,
        )
        .await
        .expect("list");
        assert!(listed.contains(id));
        assert!(listed.contains("id 前缀"));

        let missing_context = execute_local_session_resume_command(
            "codex".to_string(),
            ExternalCliResumeSlashCommand::Pick(id.to_string()),
            None,
        )
        .await
        .expect_err("pick requires session context");
        assert!(missing_context.contains("sessionKey"));

        let picked = execute_local_session_resume_command(
            "codex".to_string(),
            ExternalCliResumeSlashCommand::Pick("cccccccc".to_string()),
            Some(LocalSessionSelectionContext {
                session_key: "web:async-resume".to_string(),
                runner_id: "Codex".to_string(),
            }),
        )
        .await
        .expect("pick");
        assert!(picked.contains(id));
        let state = crate::im_gateway::session_state::load_session_state(
            "web:async-resume",
            "codex",
            Some("Codex"),
        )
        .expect("persisted state");
        assert_eq!(state.external_thread_id.as_deref(), Some(id));

        let unsupported = execute_local_session_resume_command(
            "mock".to_string(),
            ExternalCliResumeSlashCommand::List,
            None,
        )
        .await
        .expect_err("unsupported adapter");
        assert!(unsupported.contains("仅支持"));
    }

    #[test]
    fn resume_discovers_codex_sessions_sorted_with_index_titles_and_limit() {
        let _lock = local_session_test_env_lock().blocking_lock();
        let home = tempfile::tempdir().expect("tempdir");
        let _env = EnvGuard::set("CODEX_HOME", home.path());
        for index in 0..22 {
            let id = format!("00000000-0000-0000-0000-{index:012}");
            write_lines(
                &home
                    .path()
                    .join("sessions/2026/08/07")
                    .join(format!("rollout-{id}.jsonl")),
                &[serde_json::json!({
                    // Keep fixture timestamps ahead of the file mtime so the
                    // index timestamp remains the deterministic winner.
                    "timestamp": format!("2099-08-07T12:{index:02}:00Z"),
                    "type": "session_meta",
                    "payload": {"id": id, "timestamp": format!("2099-08-07T12:{index:02}:00Z")}
                })],
            );
        }
        let index_values = (0..22)
            .map(|index| {
                serde_json::json!({
                    "id": format!("00000000-0000-0000-0000-{index:012}"),
                    "thread_name": format!("Title {index}\nwith whitespace"),
                    "updated_at": format!("2099-08-07T12:{index:02}:30Z")
                })
            })
            .collect::<Vec<_>>();
        write_lines(&home.path().join("session_index.jsonl"), &index_values);

        let sessions = discover_local_sessions("codex", None).expect("sessions");
        assert_eq!(sessions.len(), 20);
        assert_eq!(sessions[0].id, "00000000-0000-0000-0000-000000000021");
        assert_eq!(sessions[0].title, "Title 21 with whitespace");
        assert_eq!(sessions[0].datetime, "2099-08-07T12:21:30Z");
    }

    #[test]
    fn resume_discovers_traex_and_claude_sessions_from_separate_homes() {
        let _lock = local_session_test_env_lock().blocking_lock();
        let trae_home = tempfile::tempdir().expect("trae tempdir");
        let claude_home = tempfile::tempdir().expect("claude tempdir");
        let _trae_env = EnvGuard::set("TRAE_HOME", trae_home.path());
        let _claude_env = EnvGuard::set("CLAUDE_CONFIG_DIR", claude_home.path());

        let trae_id = "11111111-1111-1111-1111-111111111111";
        write_lines(
            &trae_home.path().join("cli/sessions/rollout-trae.jsonl"),
            &[serde_json::json!({
                "timestamp": "2026-08-06T01:00:00Z",
                "type": "session_meta",
                "payload": {"id": trae_id, "timestamp": "2026-08-06T01:00:00Z"}
            })],
        );
        write_lines(
            &trae_home.path().join("cli/history.jsonl"),
            &[
                serde_json::json!({"session_id": trae_id, "ts": 1785981600_u64, "text": "Trae title"}),
            ],
        );

        let claude_id = "22222222-2222-2222-2222-222222222222";
        write_lines(
            &claude_home.path().join("projects/work/claude.jsonl"),
            &[
                serde_json::json!({"type": "user", "sessionId": claude_id, "timestamp": "2026-08-05T01:00:00Z", "message": {"content": "fallback title"}}),
                serde_json::json!({"type": "ai-title", "sessionId": claude_id, "aiTitle": "AI title"}),
            ],
        );

        let trae = discover_local_sessions("traex", None).expect("trae sessions");
        assert_eq!(trae.len(), 1);
        assert_eq!(trae[0].title, "Trae title");
        let claude = discover_local_sessions("claude_code", None).expect("claude sessions");
        assert_eq!(claude.len(), 1);
        assert_eq!(claude[0].id, claude_id);
        assert_eq!(claude[0].title, "AI title");
    }

    #[test]
    fn resume_resolves_exact_and_unique_prefix_but_rejects_ambiguous_prefix() {
        let _lock = local_session_test_env_lock().blocking_lock();
        let home = tempfile::tempdir().expect("tempdir");
        let _env = EnvGuard::set("CODEX_HOME", home.path());
        for id in [
            "aaaaaaaa-0000-0000-0000-000000000001",
            "aaaaaaaa-0000-0000-0000-000000000002",
            "bbbbbbbb-0000-0000-0000-000000000003",
        ] {
            write_lines(
                &home.path().join("sessions").join(format!("{id}.jsonl")),
                &[serde_json::json!({"type": "session_meta", "payload": {"id": id}})],
            );
        }

        assert_eq!(
            resolve_local_session("codex", "bbbbbbbb")
                .expect("unique")
                .id,
            "bbbbbbbb-0000-0000-0000-000000000003"
        );
        assert!(resolve_local_session("codex", "aaaaaaaa").is_err());
        assert!(resolve_local_session("traex", "bbbbbbbb").is_err());
        assert!(resolve_local_session("codex", "missing").is_err());
        assert!(resolve_local_session("codex", "cccccccc").is_err());
        assert!(discover_local_sessions("mock", None).is_err());
    }

    #[test]
    fn resume_formatting_and_parsing_helpers_cover_empty_nested_and_invalid_values() {
        assert_eq!(
            format_local_session_list("codex", &[]),
            "没有找到 Codex 本地 session。"
        );
        assert_eq!(clean_title("  \n\t "), "Untitled session");
        assert!(clean_title(&"界".repeat(MAX_TITLE_CHARS + 1)).ends_with('…'));
        assert_eq!(normalize_epoch(2), 2_000);
        assert_eq!(normalize_epoch(10_000_000_000), 10_000_000_000);
        assert_eq!(timestamp_from_value(&serde_json::json!(-1)), 0);
        assert_eq!(timestamp_from_value(&serde_json::json!("bad")), 0);
        assert_eq!(format_datetime(u64::MAX), "unknown");
        assert_eq!(
            extract_message_text(Some(&serde_json::json!(" text "))).as_deref(),
            Some("text")
        );
        assert_eq!(
            extract_message_text(Some(&serde_json::json!({"content": [{"text": "nested"}]})))
                .as_deref(),
            Some("nested")
        );
        assert!(extract_message_text(Some(&serde_json::json!(42))).is_none());

        let root = tempfile::tempdir().expect("root tempdir");
        write_lines(
            &root.path().join("projects/work/fallback-id.jsonl"),
            &[serde_json::json!({
                "type": "user",
                "timestamp": 1_786_080_000_u64,
                "message": {"text": "fallback"}
            })],
        );
        let sessions = discover_claude_sessions(&root.path().join("projects"), HashMap::new());
        assert_eq!(sessions[0].id, "fallback-id");
        assert_eq!(sessions[0].title, "fallback");
    }

    #[test]
    fn resume_persists_selected_thread_without_dropping_other_session_overrides() {
        let _lock = local_session_test_env_lock().blocking_lock();
        let data_dir = tempfile::tempdir().expect("data tempdir");
        let _data_guard = crate::test_env::BifrostDataDirGuard::set(data_dir.path());
        crate::im_gateway::session_state::upsert_session_state(
            "web:resume-test",
            "claude_code",
            Some("Claude-Code"),
            |state| {
                state.external_conversation_id = Some("old-conversation".to_string());
                state.model_override = Some("sonnet".to_string());
            },
        )
        .expect("seed state");
        let selected = LocalExternalSession {
            id: "22222222-2222-2222-2222-222222222222".to_string(),
            title: "Selected".to_string(),
            datetime: "2026-08-07T00:00:00Z".to_string(),
            updated_at_millis: 1,
        };

        persist_local_session_selection("web:resume-test", "claude_code", "Claude-Code", &selected)
            .expect("persist selection");

        let state = crate::im_gateway::session_state::load_session_state(
            "web:resume-test",
            "claude_code",
            Some("Claude-Code"),
        )
        .expect("state");
        assert_eq!(
            state.external_thread_id.as_deref(),
            Some("22222222-2222-2222-2222-222222222222")
        );
        assert!(state.external_conversation_id.is_none());
        assert_eq!(state.model_override.as_deref(), Some("sonnet"));
    }

    #[test]
    #[ignore = "read-only smoke against the current user's installed CLI session stores"]
    fn resume_real_local_session_stores_are_readable_without_exposing_message_bodies() {
        let _lock = local_session_test_env_lock().blocking_lock();
        for adapter in ["codex", "traex", "claude_code"] {
            let sessions = discover_local_sessions(adapter, Some(20)).expect("local sessions");
            assert!(sessions.len() <= 20);
            for session in sessions {
                assert!(!session.id.trim().is_empty());
                assert!(!session.title.contains('\n'));
                assert!(session.title.chars().count() <= MAX_TITLE_CHARS + 1);
                assert!(
                    session.datetime == "unknown"
                        || DateTime::parse_from_rfc3339(&session.datetime).is_ok()
                );
            }
        }
    }
}
