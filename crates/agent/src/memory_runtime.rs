//! Agent turn loop 与长期记忆系统的集成层。
//!
//! 该模块负责把 `memory` crate 的纯存储/召回能力接到 AgentConfig、
//! AgentClient 与 session turn loop。失败时默认降级为不注入/不抽取，
//! 避免记忆系统影响主对话可用性。

use crate::client::AgentClient;
use crate::config::{agent_home_dir, AgentConfig, MemoriesConfig};
use crate::session::AgentSession;
use crate::types::ChatMessage;
use memory::extract::{
    ConsolidationRequest, LlmMemoryExtractor, MemoryCandidate, MemoryConsolidator,
    NoopMemoryConsolidator,
};
use memory::recall::{format_memory_system_message, project_hash};
use memory::{
    DefaultMemoryRecaller, MemoryKind, MemoryPatch, MemoryRecaller, MemoryRecord, MemoryScope,
    MemorySearchQuery, MemorySource, MemoryStore, NewMemoryRecord, RecallContext,
    SqliteMemoryStore,
};
use tracing::{info, warn};

/// 默认自动抽取跳过阈值：8 KiB。
pub const DEFAULT_EXTRACT_SKIP_THRESHOLD_BYTES: usize = 8 * 1024;

/// 判断召回是否开启。
pub fn use_memories_enabled(config: &AgentConfig) -> bool {
    config.get_memories_config().use_memories != Some(false)
}

/// 判断自动抽取是否开启。
pub fn generate_memories_enabled(config: &AgentConfig) -> bool {
    config.get_memories_config().generate_memories != Some(false)
}

/// 打开默认长期记忆存储。
pub fn open_default_store() -> Result<SqliteMemoryStore, String> {
    SqliteMemoryStore::open(agent_home_dir()).map_err(|error| format!("open memory store: {error}"))
}

/// 显式记住一条用户输入。
pub fn remember_explicit(
    config: &AgentConfig,
    session: &AgentSession,
    content: &str,
) -> Result<MemoryRecord, String> {
    let store = open_default_store()?;
    let record = store
        .insert(NewMemoryRecord {
            scope: MemoryScope::User(memory_user_id(session)),
            kind: MemoryKind::Fact,
            content: content.to_string(),
            source: MemorySource::UserExplicit,
            tags: vec!["explicit".to_string()],
            pinned: false,
            confidence: 1.0,
            expires_at: None,
        })
        .map_err(|error| format!("insert explicit memory: {error}"))?;
    run_configured_gc(config);
    Ok(record)
}

/// 列出当前用户/项目/session 可见的记忆。
pub fn list_visible_memories(
    config: &AgentConfig,
    session: &AgentSession,
    limit: usize,
) -> Result<Vec<MemoryRecord>, String> {
    let store = open_default_store()?;
    let scopes = recall_scopes_for_session(config, session);
    store
        .search(MemorySearchQuery {
            scopes,
            limit,
            ..Default::default()
        })
        .map_err(|error| format!("list memories: {error}"))
}

/// 忘记一条记忆，`last` 表示最近更新的当前可见记忆。
pub fn forget_memory(
    config: &AgentConfig,
    session: &AgentSession,
    id_or_last: &str,
) -> Result<Option<String>, String> {
    let store = open_default_store()?;
    let id = if id_or_last == "last" {
        store
            .search(MemorySearchQuery {
                scopes: recall_scopes_for_session(config, session),
                limit: 1,
                ..Default::default()
            })
            .map_err(|error| format!("find last memory: {error}"))?
            .into_iter()
            .next()
            .map(|record| record.id.to_string())
    } else {
        Some(id_or_last.to_string())
    };
    let Some(id) = id else {
        return Ok(None);
    };
    let deleted = store
        .soft_delete(&memory::MemoryId::from_string(id.clone()))
        .map_err(|error| format!("delete memory: {error}"))?;
    Ok(deleted.then_some(id))
}

/// 构造可注入的长期记忆 system message。
pub fn recall_system_message(
    config: &AgentConfig,
    session: &AgentSession,
    latest_user_message: &str,
) -> Option<ChatMessage> {
    if !use_memories_enabled(config) {
        return None;
    }
    let store = match open_default_store() {
        Ok(store) => store,
        Err(error) => {
            warn!(error = %error, "memory recall disabled for this turn");
            return None;
        }
    };
    let recaller = DefaultMemoryRecaller::new(&store);
    let context = RecallContext {
        user_id: Some(memory_user_id(session)),
        project_path: Some(project_path(config, session)),
        session_key: Some(session.session_key.clone()),
        latest_user_message: latest_user_message.to_string(),
        history_tail_tokens: session.estimate_tokens() as usize,
        max_items: 8,
        max_chars: 2000,
    };
    match recaller.recall(context) {
        Ok(records) => format_memory_system_message(&records).map(|content| {
            info!(count = records.len(), "memory recall injected");
            ChatMessage::system(&content)
        }),
        Err(error) => {
            warn!(error = %error, "memory recall failed");
            None
        }
    }
}

/// 自动抽取本轮值得沉淀的记忆。
pub async fn auto_extract_after_turn(
    client: &AgentClient,
    config: &AgentConfig,
    session: &AgentSession,
    user_message: &str,
    assistant_message: &str,
) {
    if !generate_memories_enabled(config) {
        run_configured_gc(config);
        return;
    }
    if should_skip_external_context(config.get_memories_config(), user_message) {
        info!(
            session_key = %session.session_key,
            user_message_bytes = user_message.len(),
            "memory auto extract skipped for external context"
        );
        return;
    }
    let extractor = LlmMemoryExtractor::new();
    let mut extract_config = config.clone();
    if let Some(model) = config.get_memories_config().extract_model {
        extract_config.model = Some(model);
    }
    let prompt = extractor.prompt();
    let payload = serde_json::json!({
        "session_key": session.session_key,
        "turn": session.history_version,
        "project_path": project_path(config, session),
        "user_message": user_message,
        "assistant_message": assistant_message,
    });
    let messages = vec![
        ChatMessage::system(prompt),
        ChatMessage::user(&payload.to_string()),
    ];
    let response = match client
        .chat_completion(&extract_config, &messages, &[])
        .await
    {
        Ok(response) => response,
        Err(error) => {
            warn!(error = %error, "memory auto extract model call failed");
            return;
        }
    };
    let raw = response
        .content
        .or(response.reasoning_content)
        .unwrap_or_else(|| "[]".to_string());
    let candidates = match extractor.parse_candidates(&raw) {
        Ok(candidates) => candidates,
        Err(error) => {
            warn!(error = %error, "memory auto extract parse failed");
            return;
        }
    };
    if let Err(error) = persist_candidates(config, session, candidates) {
        warn!(error = %error, "memory auto extract persist failed");
    }
    run_configured_gc(config);
    run_configured_consolidation(config).await;
}

fn persist_candidates(
    _config: &AgentConfig,
    session: &AgentSession,
    candidates: Vec<MemoryCandidate>,
) -> Result<(), String> {
    if candidates.is_empty() {
        return Ok(());
    }
    let store = open_default_store()?;
    let mut inserted = 0usize;
    for candidate in candidates {
        let scope = candidate
            .scope_hint
            .unwrap_or_else(|| MemoryScope::User(memory_user_id(session)));
        store
            .insert(NewMemoryRecord {
                scope,
                kind: candidate.kind,
                content: candidate.content,
                source: MemorySource::AutoExtract {
                    session_key: session.session_key.clone(),
                    turn: session.history_version,
                },
                tags: candidate.tags,
                pinned: false,
                confidence: candidate.confidence,
                expires_at: None,
            })
            .map_err(|error| format!("insert extracted memory: {error}"))?;
        inserted += 1;
    }
    info!(count = inserted, "memory auto extract persisted");
    Ok(())
}

fn should_skip_external_context(memories: MemoriesConfig, user_message: &str) -> bool {
    memories.disable_on_external_context == Some(true)
        && user_message.len() > DEFAULT_EXTRACT_SKIP_THRESHOLD_BYTES
}

fn memory_user_id(session: &AgentSession) -> String {
    session
        .user_id
        .clone()
        .unwrap_or_else(|| session.session_key.clone())
}

fn project_path(config: &AgentConfig, session: &AgentSession) -> String {
    session
        .work_dir
        .clone()
        .unwrap_or_else(|| config.resolve_work_dir().display().to_string())
}

fn recall_scopes_for_session(config: &AgentConfig, session: &AgentSession) -> Vec<MemoryScope> {
    vec![
        MemoryScope::Global,
        MemoryScope::User(memory_user_id(session)),
        MemoryScope::Project(project_hash(&project_path(config, session))),
        MemoryScope::Session(session.session_key.clone()),
    ]
}

fn run_configured_gc(config: &AgentConfig) {
    let memories = config.get_memories_config();
    if let Ok(store) = open_default_store() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let tombstone_path = agent_home_dir().join("memory").join("tombstones.jsonl");
        if let Err(error) = store.gc(memory::GcPolicy {
            now,
            max_unused_days: memories.max_unused_days,
            tombstone_path: Some(tombstone_path),
        }) {
            warn!(error = %error, "memory configured gc failed");
        }
    }
    info!(
        max_raw_memories_for_consolidation = memories.max_raw_memories_for_consolidation,
        max_unused_days = memories.max_unused_days,
        max_rollout_age_days = memories.max_rollout_age_days,
        max_rollouts_per_startup = memories.max_rollouts_per_startup,
        min_rollout_idle_hours = memories.min_rollout_idle_hours,
        consolidation_model = memories.consolidation_model.as_deref().unwrap_or(""),
        "memory config consumed"
    );
}

async fn run_configured_consolidation(config: &AgentConfig) {
    let memories = config.get_memories_config();
    let max_items = memories.max_raw_memories_for_consolidation.unwrap_or(0);
    if memories.consolidation_model.is_none() && max_items == 0 {
        return;
    }
    let consolidator = NoopMemoryConsolidator;
    let _ = consolidator
        .consolidate(ConsolidationRequest {
            model: memories.consolidation_model,
            max_items,
        })
        .await;
}

/// 手动编辑显式记忆时复用的 patch helper。
pub fn patch_memory(id: &str, patch: MemoryPatch) -> Result<MemoryRecord, String> {
    let store = open_default_store()?;
    store
        .update(&memory::MemoryId::from_string(id), patch)
        .map_err(|error| format!("patch memory: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_memories_disabled_short_circuits() {
        let config = AgentConfig {
            memories: Some(MemoriesConfig {
                use_memories: Some(false),
                ..Default::default()
            }),
            ..AgentConfig::default()
        };
        assert!(!use_memories_enabled(&config));
    }

    #[test]
    fn generate_memories_disabled_short_circuits() {
        let config = AgentConfig {
            memories: Some(MemoriesConfig {
                generate_memories: Some(false),
                ..Default::default()
            }),
            ..AgentConfig::default()
        };
        assert!(!generate_memories_enabled(&config));

        let session = AgentSession::new("explicit-still-allowed");
        let temp = tempfile::tempdir().expect("temp agent home");
        let old_home = std::env::var("BIFROST_AGENT_HOME").ok();
        std::env::set_var("BIFROST_AGENT_HOME", temp.path());
        let remembered = remember_explicit(&config, &session, "explicit memory still writes");
        match old_home {
            Some(value) => std::env::set_var("BIFROST_AGENT_HOME", value),
            None => std::env::remove_var("BIFROST_AGENT_HOME"),
        }
        assert!(remembered.is_ok());
    }
}
