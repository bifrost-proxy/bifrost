use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

const STORE_VERSION: u32 = 1;
const STORE_FILENAME: &str = "session_state.json";
const MAX_STORE_FILE_BYTES: u64 = 16 * 1024 * 1024;

static STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImAgentSessionState {
    pub session_key: String,
    pub adapter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_override_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort_override_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier_override_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_user_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_response: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<ImAgentSessionMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_imported_contexts: Vec<ImImportedRunnerContext>,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub updated_seq: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImAgentSessionMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_parts: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImImportedRunnerContext {
    pub call_id: String,
    pub source_session_key: String,
    pub target_runner_id: String,
    pub target_adapter: String,
    pub user_message: String,
    pub response: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoreData {
    version: u32,
    #[serde(default)]
    write_seq: u64,
    sessions: BTreeMap<String, ImAgentSessionState>,
}

impl Default for StoreData {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            write_seq: 0,
            sessions: BTreeMap::new(),
        }
    }
}

pub fn default_store_path() -> PathBuf {
    bifrost_agent::config::agent_home_dir()
        .join("im_gateway")
        .join(STORE_FILENAME)
}

pub fn load_session_state(
    session_key: &str,
    adapter: &str,
    runner_id: Option<&str>,
) -> Option<ImAgentSessionState> {
    load_latest_session_state(session_key, Some(adapter), runner_id)
}

pub fn list_session_states() -> Vec<ImAgentSessionState> {
    let Some(store) = load_store(&default_store_path()) else {
        return Vec::new();
    };
    store.sessions.into_values().collect()
}

pub fn load_latest_session_state(
    session_key: &str,
    adapter: Option<&str>,
    runner_id: Option<&str>,
) -> Option<ImAgentSessionState> {
    let session_key = session_key.trim();
    if session_key.is_empty() {
        return None;
    }
    let store = load_store(&default_store_path())?;
    if let (Some(adapter), Some(runner_id)) = (adapter, runner_id) {
        if let Some(state) = store
            .sessions
            .get(&state_key(session_key, adapter, Some(runner_id)))
        {
            return Some(state.clone());
        }
    }
    store
        .sessions
        .values()
        .filter(|state| state.session_key == session_key)
        .filter(|state| adapter.is_none_or(|adapter| state.adapter == adapter))
        .filter(|state| {
            runner_id.is_none_or(|runner_id| state.runner_id.as_deref() == Some(runner_id))
        })
        .max_by_key(|state| (state.updated_at, state.updated_seq))
        .cloned()
}

pub fn remember_session_state(mut state: ImAgentSessionState) -> Result<(), String> {
    normalize_state(&mut state)?;
    update_store(|store| {
        mark_state_updated(store, &mut state, false);
        let key = state_key(
            &state.session_key,
            &state.adapter,
            state.runner_id.as_deref(),
        );
        store.sessions.insert(key, state);
    })
}

pub fn upsert_session_state(
    session_key: &str,
    adapter: &str,
    runner_id: Option<&str>,
    update: impl FnOnce(&mut ImAgentSessionState),
) -> Result<ImAgentSessionState, String> {
    let mut output = None;
    update_store(|store| {
        store.write_seq = store.write_seq.saturating_add(1);
        let updated_seq = store.write_seq;
        let key = state_key(session_key, adapter, runner_id);
        let state = store
            .sessions
            .entry(key)
            .or_insert_with(|| ImAgentSessionState {
                session_key: session_key.trim().to_string(),
                adapter: adapter.trim().to_string(),
                runner_id: runner_id
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string),
                ..ImAgentSessionState::default()
            });
        update(state);
        state.updated_at = now_millis();
        state.updated_seq = updated_seq;
        output = Some(state.clone());
    })?;
    output.ok_or_else(|| "session state update did not produce a state".to_string())
}

pub fn clear_session_state(session_key: &str) -> Result<(), String> {
    clear_session_state_scope(session_key, None, None).map(|_| ())
}

pub fn clear_session_state_scope(
    session_key: &str,
    adapter: Option<&str>,
    runner_id: Option<&str>,
) -> Result<Vec<ImAgentSessionState>, String> {
    let session_key = session_key.trim();
    if session_key.is_empty() {
        return Ok(Vec::new());
    }
    let adapter = adapter.map(str::trim).filter(|value| !value.is_empty());
    let runner_id = runner_id.map(str::trim).filter(|value| !value.is_empty());
    let mut removed = Vec::new();
    update_store(|store| {
        store.sessions.retain(|_, state| {
            let matches = state.session_key == session_key
                && adapter.is_none_or(|adapter| state.adapter == adapter)
                && runner_id.is_none_or(|runner_id| state.runner_id.as_deref() == Some(runner_id));
            if matches {
                removed.push(state.clone());
                false
            } else {
                true
            }
        });
    })?;
    Ok(removed)
}

pub fn metadata_from_state(state: &ImAgentSessionState) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::new();
    if let Some(thread_id) = state
        .external_thread_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("threadId".to_string(), thread_id.to_string());
    }
    if let Some(conversation_id) = state
        .external_conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("conversationId".to_string(), conversation_id.to_string());
    }
    if let Some(model) = state
        .model_override
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("modelOverride".to_string(), model.to_string());
    }
    if let Some(source) = state
        .model_override_source
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("modelOverrideSource".to_string(), source.to_string());
    }
    if let Some(effort) = state
        .reasoning_effort_override
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("modelReasoningEffort".to_string(), effort.to_string());
        metadata.insert("reasoningEffortOverride".to_string(), effort.to_string());
    }
    if let Some(source) = state
        .reasoning_effort_override_source
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert(
            "reasoningEffortOverrideSource".to_string(),
            source.to_string(),
        );
    }
    if let Some(service_tier) = state
        .service_tier_override
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("serviceTierOverride".to_string(), service_tier.to_string());
    }
    if let Some(source) = state
        .service_tier_override_source
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        metadata.insert("serviceTierOverrideSource".to_string(), source.to_string());
    }
    metadata
}

pub fn push_imported_context(
    session_key: &str,
    adapter: &str,
    runner_id: Option<&str>,
    context: ImImportedRunnerContext,
) -> Result<(), String> {
    let mut context = normalize_imported_context(context)?;
    upsert_session_state(session_key, adapter, runner_id, |state| {
        if state
            .pending_imported_contexts
            .iter()
            .any(|item| item.call_id == context.call_id)
        {
            return;
        }
        if context.created_at == 0 {
            context.created_at = now_millis();
        }
        state.pending_imported_contexts.push(context);
    })
    .map(|_| ())
}

pub fn take_imported_contexts(
    session_key: &str,
    adapter: &str,
    runner_id: Option<&str>,
) -> Result<Vec<ImImportedRunnerContext>, String> {
    let mut imported = Vec::new();
    update_store(|store| {
        store.write_seq = store.write_seq.saturating_add(1);
        let key = state_key(session_key, adapter, runner_id);
        if let Some(state) = store.sessions.get_mut(&key) {
            imported = std::mem::take(&mut state.pending_imported_contexts);
            if !imported.is_empty() {
                state.updated_at = now_millis();
                state.updated_seq = store.write_seq;
            }
        }
    })?;
    Ok(imported)
}

pub fn render_imported_contexts(contexts: &[ImImportedRunnerContext]) -> Option<String> {
    if contexts.is_empty() {
        return None;
    }
    let mut rendered = String::from("## Imported Runner Results\n\n");
    rendered.push_str(
        "The following results were produced by user-triggered slash Runner calls in this conversation. Use them as current conversation context.\n\n",
    );
    for context in contexts {
        rendered.push_str("### Runner Call ");
        rendered.push_str(&context.call_id);
        rendered.push('\n');
        rendered.push_str("- Target Runner: ");
        rendered.push_str(&context.target_runner_id);
        rendered.push_str(" (");
        rendered.push_str(&context.target_adapter);
        rendered.push_str(")\n");
        rendered.push_str("- User Request: ");
        rendered.push_str(&context.user_message);
        rendered.push_str("\n\nResult:\n");
        rendered.push_str(&context.response);
        rendered.push_str("\n\n");
    }
    Some(rendered)
}

fn update_store(update: impl FnOnce(&mut StoreData)) -> Result<(), String> {
    let lock = STORE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().map_err(|_| "session state lock poisoned")?;
    let path = default_store_path();
    let mut store = match load_store_result(&path) {
        Ok(Some(store)) => store,
        Ok(None) => StoreData::default(),
        Err(error) => {
            backup_invalid_store(&path, &error)?;
            StoreData::default()
        }
    };
    update(&mut store);
    save_store(&path, &store)
}

fn load_store(path: &Path) -> Option<StoreData> {
    match load_store_result(path) {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to load IM Gateway session state store"
            );
            None
        }
    }
}

fn load_store_result(path: &Path) -> Result<Option<StoreData>, String> {
    if !path.exists() {
        return Ok(None);
    }
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > MAX_STORE_FILE_BYTES {
        return Err(format!(
            "session state file exceeds {} bytes",
            MAX_STORE_FILE_BYTES
        ));
    }
    let content = std::fs::read_to_string(path)
        .map_err(|error| format!("read session state failed: {error}"))?;
    match serde_json::from_str::<StoreData>(&content) {
        Ok(data) if data.version == STORE_VERSION => Ok(Some(data)),
        Ok(data) => Err(format!(
            "unsupported session state version {}",
            data.version
        )),
        Err(error) => Err(format!("parse session state failed: {error}")),
    }
}

fn backup_invalid_store(path: &Path, reason: &str) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(STORE_FILENAME);
    let backup_path = path.with_file_name(format!(
        "{file_name}.invalid.{}.{}.bak",
        now_millis(),
        std::process::id()
    ));
    std::fs::rename(path, &backup_path).map_err(|error| {
        format!(
            "backup invalid session state file '{}' to '{}' failed after {reason}: {error}",
            path.display(),
            backup_path.display()
        )
    })?;
    tracing::warn!(
        path = %path.display(),
        backup_path = %backup_path.display(),
        reason = %reason,
        "backed up invalid IM Gateway session state store before recreating it"
    );
    Ok(())
}

fn save_store(path: &Path, store: &StoreData) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create session state dir failed: {error}"))?;
    }
    let content = serde_json::to_string_pretty(store)
        .map_err(|error| format!("serialize session state failed: {error}"))?;
    let tmp_path = path.with_file_name(format!(
        "{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(STORE_FILENAME),
        std::process::id()
    ));
    std::fs::write(&tmp_path, content)
        .map_err(|error| format!("write session state temp file failed: {error}"))?;
    std::fs::rename(&tmp_path, path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("replace session state file failed: {error}")
    })
}

fn normalize_state(state: &mut ImAgentSessionState) -> Result<(), String> {
    state.session_key = state.session_key.trim().to_string();
    state.adapter = state.adapter.trim().to_string();
    if state.session_key.is_empty() {
        return Err("session_key cannot be empty".to_string());
    }
    if state.adapter.is_empty() {
        return Err("adapter cannot be empty".to_string());
    }
    state.runner_id = normalize_optional(state.runner_id.take());
    state.external_conversation_id = normalize_optional(state.external_conversation_id.take());
    state.external_thread_id = normalize_optional(state.external_thread_id.take());
    state.history_path = normalize_optional(state.history_path.take());
    state.work_dir = normalize_optional(state.work_dir.take());
    state.model_override = normalize_optional(state.model_override.take());
    state.model_override_source = normalize_optional(state.model_override_source.take());
    state.reasoning_effort_override = normalize_optional(state.reasoning_effort_override.take());
    state.reasoning_effort_override_source =
        normalize_optional(state.reasoning_effort_override_source.take());
    state.service_tier_override = normalize_optional(state.service_tier_override.take());
    state.service_tier_override_source =
        normalize_optional(state.service_tier_override_source.take());
    state.latest_run_id = normalize_optional(state.latest_run_id.take());
    state.title = normalize_optional(state.title.take());
    state.last_user_message = normalize_optional(state.last_user_message.take());
    state.last_response = normalize_optional(state.last_response.take());
    state.status = normalize_optional(state.status.take());
    state.messages = std::mem::take(&mut state.messages)
        .into_iter()
        .filter_map(|mut message| {
            message.role = message.role.trim().to_string();
            message.content = message.content.trim().to_string();
            if message.role.is_empty() || message.content.is_empty() {
                None
            } else {
                Some(message)
            }
        })
        .collect();
    state.pending_imported_contexts = std::mem::take(&mut state.pending_imported_contexts)
        .into_iter()
        .filter_map(|context| normalize_imported_context(context).ok())
        .collect();
    if state.updated_at == 0 {
        state.updated_at = now_millis();
    }
    Ok(())
}

fn normalize_imported_context(
    mut context: ImImportedRunnerContext,
) -> Result<ImImportedRunnerContext, String> {
    context.call_id = context.call_id.trim().to_string();
    context.source_session_key = context.source_session_key.trim().to_string();
    context.target_runner_id = context.target_runner_id.trim().to_string();
    context.target_adapter = context.target_adapter.trim().to_string();
    context.user_message = context.user_message.trim().to_string();
    context.response = context.response.trim().to_string();
    if context.call_id.is_empty() {
        return Err("imported context call_id cannot be empty".to_string());
    }
    if context.source_session_key.is_empty() {
        return Err("imported context source_session_key cannot be empty".to_string());
    }
    if context.target_runner_id.is_empty() {
        return Err("imported context target_runner_id cannot be empty".to_string());
    }
    if context.target_adapter.is_empty() {
        return Err("imported context target_adapter cannot be empty".to_string());
    }
    if context.user_message.is_empty() || context.response.is_empty() {
        return Err("imported context user_message and response cannot be empty".to_string());
    }
    Ok(context)
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn state_key(session_key: &str, adapter: &str, runner_id: Option<&str>) -> String {
    format!(
        "{}::{}::{}",
        session_key.trim(),
        adapter.trim(),
        runner_id.map(str::trim).unwrap_or("")
    )
}

fn mark_state_updated(store: &mut StoreData, state: &mut ImAgentSessionState, always_time: bool) {
    store.write_seq = store.write_seq.saturating_add(1);
    if always_time || state.updated_at == 0 {
        state.updated_at = now_millis();
    }
    state.updated_seq = store.write_seq;
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvGuard {
        _guard: crate::test_env::BifrostDataDirGuard,
    }

    impl EnvGuard {
        fn new(path: &Path) -> Self {
            Self {
                _guard: crate::test_env::BifrostDataDirGuard::set(path),
            }
        }
    }

    #[test]
    fn remembers_and_loads_exact_session_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::new(dir.path());

        remember_session_state(ImAgentSessionState {
            session_key: "im:provider:user".to_string(),
            adapter: "codex".to_string(),
            runner_id: Some("runner-a".to_string()),
            external_thread_id: Some("thread-1".to_string()),
            updated_at: 1,
            ..ImAgentSessionState::default()
        })
        .expect("remember");

        let state =
            load_session_state("im:provider:user", "codex", Some("runner-a")).expect("state");
        assert_eq!(state.external_thread_id.as_deref(), Some("thread-1"));
        assert_eq!(
            metadata_from_state(&state)
                .get("threadId")
                .map(String::as_str),
            Some("thread-1")
        );
    }

    #[test]
    fn metadata_includes_service_tier_override_and_source() {
        let metadata = metadata_from_state(&ImAgentSessionState {
            service_tier_override: Some(" fast ".to_string()),
            service_tier_override_source: Some(" session slash command ".to_string()),
            ..Default::default()
        });

        assert_eq!(
            metadata.get("serviceTierOverride").map(String::as_str),
            Some("fast")
        );
        assert_eq!(
            metadata
                .get("serviceTierOverrideSource")
                .map(String::as_str),
            Some("session slash command")
        );
    }

    #[test]
    fn list_session_states_includes_external_runner_result_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::new(dir.path());

        remember_session_state(ImAgentSessionState {
            session_key: "codex-ui-session".to_string(),
            adapter: "codex".to_string(),
            runner_id: Some("codex".to_string()),
            latest_run_id: Some("run-1".to_string()),
            title: Some("Summarize issue".to_string()),
            last_user_message: Some("Summarize issue".to_string()),
            last_response: Some("Done".to_string()),
            status: Some("succeeded".to_string()),
            updated_at: 1,
            ..ImAgentSessionState::default()
        })
        .expect("remember");

        let states = list_session_states();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].session_key, "codex-ui-session");
        assert_eq!(states[0].latest_run_id.as_deref(), Some("run-1"));
        assert_eq!(states[0].last_response.as_deref(), Some("Done"));
    }

    #[test]
    fn session_state_normalizes_persisted_message_sequence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::new(dir.path());

        remember_session_state(ImAgentSessionState {
            session_key: "chatgpt-web-ui-session".to_string(),
            adapter: "chatgpt_web".to_string(),
            runner_id: Some("web".to_string()),
            messages: vec![
                ImAgentSessionMessage {
                    role: " user ".to_string(),
                    content: " first ".to_string(),
                    timestamp: Some(11),
                    content_parts: None,
                },
                ImAgentSessionMessage {
                    role: "assistant".to_string(),
                    content: " ".to_string(),
                    timestamp: Some(12),
                    content_parts: None,
                },
                ImAgentSessionMessage {
                    role: "assistant".to_string(),
                    content: "answer".to_string(),
                    timestamp: Some(13),
                    content_parts: None,
                },
            ],
            ..ImAgentSessionState::default()
        })
        .expect("remember");

        let state = load_session_state("chatgpt-web-ui-session", "chatgpt_web", Some("web"))
            .expect("state");
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].role, "user");
        assert_eq!(state.messages[0].content, "first");
        assert_eq!(state.messages[1].role, "assistant");
        assert_eq!(state.messages[1].content, "answer");
    }

    #[test]
    fn session_state_persists_message_content_parts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::new(dir.path());

        let content_parts = serde_json::json!([
            {"type": "text", "text": "describe"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,aGVsbG8=", "detail": "auto"}}
        ]);
        remember_session_state(ImAgentSessionState {
            session_key: "image-session".to_string(),
            adapter: "codex".to_string(),
            runner_id: Some("codex".to_string()),
            messages: vec![ImAgentSessionMessage {
                role: "user".to_string(),
                content: "describe".to_string(),
                timestamp: Some(11),
                content_parts: Some(content_parts.clone()),
            }],
            ..ImAgentSessionState::default()
        })
        .expect("remember");

        let state = load_session_state("image-session", "codex", Some("codex")).unwrap();
        assert_eq!(state.messages[0].content_parts, Some(content_parts));
    }

    #[test]
    fn falls_back_to_latest_adapter_state_when_runner_is_unspecified() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::new(dir.path());

        remember_session_state(ImAgentSessionState {
            session_key: "im:provider:user".to_string(),
            adapter: "codex".to_string(),
            runner_id: Some("runner-old".to_string()),
            external_thread_id: Some("thread-old".to_string()),
            updated_at: 10,
            ..ImAgentSessionState::default()
        })
        .expect("remember old");
        remember_session_state(ImAgentSessionState {
            session_key: "im:provider:user".to_string(),
            adapter: "codex".to_string(),
            runner_id: Some("runner-new".to_string()),
            external_thread_id: Some("thread-new".to_string()),
            updated_at: 20,
            ..ImAgentSessionState::default()
        })
        .expect("remember new");

        let state = load_latest_session_state("im:provider:user", Some("codex"), None)
            .expect("latest state");
        assert_eq!(state.external_thread_id.as_deref(), Some("thread-new"));
    }

    #[test]
    fn latest_state_uses_write_sequence_when_timestamp_ties() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::new(dir.path());

        remember_session_state(ImAgentSessionState {
            session_key: "im:provider:user".to_string(),
            adapter: "codex".to_string(),
            runner_id: Some("runner-a".to_string()),
            external_thread_id: Some("thread-a".to_string()),
            updated_at: 42,
            ..ImAgentSessionState::default()
        })
        .expect("remember a");
        remember_session_state(ImAgentSessionState {
            session_key: "im:provider:user".to_string(),
            adapter: "codex".to_string(),
            runner_id: Some("runner-b".to_string()),
            external_thread_id: Some("thread-b".to_string()),
            updated_at: 42,
            ..ImAgentSessionState::default()
        })
        .expect("remember b");

        let state = load_latest_session_state("im:provider:user", Some("codex"), None)
            .expect("latest state");
        assert_eq!(state.external_thread_id.as_deref(), Some("thread-b"));
        assert!(state.updated_seq > 0);
    }

    #[test]
    fn clear_session_state_scope_removes_only_matching_adapter_and_runner() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::new(dir.path());

        for (adapter, runner_id, thread_id) in [
            ("codex", Some("runner-a"), "thread-a"),
            ("codex", Some("runner-b"), "thread-b"),
            ("chatgpt_web", Some("chatgpt"), "conversation-a"),
            ("claude", Some("claude"), "thread-claude"),
        ] {
            remember_session_state(ImAgentSessionState {
                session_key: "im:provider:user".to_string(),
                adapter: adapter.to_string(),
                runner_id: runner_id.map(ToString::to_string),
                external_thread_id: Some(thread_id.to_string()),
                updated_at: 1,
                ..ImAgentSessionState::default()
            })
            .expect("remember");
        }

        let removed =
            clear_session_state_scope("im:provider:user", Some("codex"), Some("runner-a"))
                .expect("clear scoped");
        assert_eq!(removed.len(), 1);
        assert!(load_session_state("im:provider:user", "codex", Some("runner-a")).is_none());
        assert!(load_session_state("im:provider:user", "codex", Some("runner-b")).is_some());
        assert!(load_session_state("im:provider:user", "chatgpt_web", Some("chatgpt")).is_some());
        assert!(load_session_state("im:provider:user", "claude", Some("claude")).is_some());
    }

    #[test]
    fn invalid_store_is_backed_up_before_recreate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::new(dir.path());
        let store_path = default_store_path();
        std::fs::create_dir_all(store_path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&store_path, "{not-json").expect("write invalid");

        remember_session_state(ImAgentSessionState {
            session_key: "im:provider:user".to_string(),
            adapter: "codex".to_string(),
            runner_id: Some("runner-a".to_string()),
            external_thread_id: Some("thread-a".to_string()),
            ..ImAgentSessionState::default()
        })
        .expect("remember after invalid");

        let backups = std::fs::read_dir(store_path.parent().expect("parent"))
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".invalid."))
            .count();
        assert_eq!(backups, 1);
        assert!(load_session_state("im:provider:user", "codex", Some("runner-a")).is_some());
    }

    #[test]
    fn clear_session_state_removes_all_adapters_for_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::new(dir.path());

        for adapter in ["codex", "chatgpt_web", "claude"] {
            remember_session_state(ImAgentSessionState {
                session_key: "im:provider:user".to_string(),
                adapter: adapter.to_string(),
                updated_at: 1,
                ..ImAgentSessionState::default()
            })
            .expect("remember");
        }

        clear_session_state("im:provider:user").expect("clear");
        assert!(load_latest_session_state("im:provider:user", None, None).is_none());
    }

    #[test]
    fn imported_contexts_are_pushed_rendered_and_consumed_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = EnvGuard::new(dir.path());

        push_imported_context(
            "admin-chat-import",
            "codex",
            Some("codex"),
            ImImportedRunnerContext {
                call_id: "call-1".to_string(),
                source_session_key: "admin-chat-import".to_string(),
                target_runner_id: "web".to_string(),
                target_adapter: "chatgpt_web".to_string(),
                user_message: "summarize context".to_string(),
                response: "context summary".to_string(),
                created_at: 1,
            },
        )
        .expect("push");

        let state = load_session_state("admin-chat-import", "codex", Some("codex")).expect("state");
        assert_eq!(state.pending_imported_contexts.len(), 1);
        let rendered =
            render_imported_contexts(&state.pending_imported_contexts).expect("rendered contexts");
        assert!(rendered.contains("Imported Runner Results"));
        assert!(rendered.contains("context summary"));

        let consumed =
            take_imported_contexts("admin-chat-import", "codex", Some("codex")).expect("take");
        assert_eq!(consumed.len(), 1);
        assert!(
            take_imported_contexts("admin-chat-import", "codex", Some("codex"))
                .expect("take again")
                .is_empty()
        );
    }
}
