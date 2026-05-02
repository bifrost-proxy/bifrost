use crate::store::{MemoryStore, Result};
use crate::types::{MemoryId, MemoryRecord, MemoryScope, MemorySearchQuery};
use tracing::info_span;

/// 召回上下文。
#[derive(Debug, Clone)]
pub struct RecallContext {
    pub user_id: Option<String>,
    pub project_path: Option<String>,
    pub session_key: Option<String>,
    pub latest_user_message: String,
    pub history_tail_tokens: usize,
    pub max_items: usize,
    pub max_chars: usize,
}

/// 记忆召回接口。
pub trait MemoryRecaller {
    /// 按上下文召回可注入的记忆。
    fn recall(&self, context: RecallContext) -> Result<Vec<MemoryRecord>>;
}

/// 默认关键词 + scope + 时间衰减召回器。
pub struct DefaultMemoryRecaller<'a, S: MemoryStore> {
    store: &'a S,
}

impl<'a, S: MemoryStore> DefaultMemoryRecaller<'a, S> {
    /// 创建默认召回器。
    pub fn new(store: &'a S) -> Self {
        Self { store }
    }
}

impl<S: MemoryStore> MemoryRecaller for DefaultMemoryRecaller<'_, S> {
    fn recall(&self, context: RecallContext) -> Result<Vec<MemoryRecord>> {
        let _span = info_span!(
            "memory.recall",
            user_id = context.user_id.as_deref().unwrap_or(""),
            max_items = context.max_items,
            max_chars = context.max_chars
        )
        .entered();
        let scopes = recall_scopes(&context);
        let mut records = self.store.search(MemorySearchQuery {
            query: Some(context.latest_user_message.clone()),
            scopes: scopes.clone(),
            limit: context.max_items.saturating_mul(4).max(20),
            ..Default::default()
        })?;

        if records.len() < context.max_items {
            let mut fallback = self.store.search(MemorySearchQuery {
                scopes,
                limit: context.max_items.saturating_mul(4).max(20),
                ..Default::default()
            })?;
            records.append(&mut fallback);
        }

        records.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.scope.specificity().cmp(&a.scope.specificity()))
                .then_with(|| {
                    b.last_used_at
                        .unwrap_or(b.updated_at)
                        .cmp(&a.last_used_at.unwrap_or(a.updated_at))
                })
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
        records.dedup_by(|a, b| a.id == b.id);

        let mut total_chars = 0usize;
        let mut selected = Vec::new();
        for record in records {
            if selected.len() >= context.max_items {
                break;
            }
            let next_len = record.content.chars().count();
            if total_chars + next_len > context.max_chars && !selected.is_empty() {
                break;
            }
            total_chars += next_len;
            selected.push(record);
        }
        let ids: Vec<MemoryId> = selected.iter().map(|record| record.id.clone()).collect();
        if !ids.is_empty() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            self.store.mark_used(&ids, now)?;
        }
        Ok(selected)
    }
}

/// 构造召回 scope 列表。
pub fn recall_scopes(context: &RecallContext) -> Vec<MemoryScope> {
    let mut scopes = vec![MemoryScope::Global];
    if let Some(user_id) = &context.user_id {
        scopes.push(MemoryScope::User(user_id.clone()));
    }
    if let Some(project_path) = &context.project_path {
        scopes.push(MemoryScope::Project(project_hash(project_path)));
    }
    if let Some(session_key) = &context.session_key {
        scopes.push(MemoryScope::Session(session_key.clone()));
    }
    scopes
}

/// 项目路径 hash，避免把完整本机路径作为 scope 主键外泄。
pub fn project_hash(path: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in path.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// 把召回结果格式化为稳定 system message。
pub fn format_memory_system_message(records: &[MemoryRecord]) -> Option<String> {
    if records.is_empty() {
        return None;
    }
    let mut output = String::from("# Long-term memories (per user)\n");
    for record in records {
        let tags = if record.tags.is_empty() {
            "-".to_string()
        } else {
            record.tags.join(",")
        };
        output.push_str(&format!(
            "- [{} {} tags:{}] {}\n",
            record.kind.as_str(),
            record.scope.scope_kind(),
            tags,
            record.content
        ));
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SqliteMemoryStore;
    use crate::types::{MemoryKind, MemorySource, NewMemoryRecord};
    use tempfile::TempDir;

    fn store() -> SqliteMemoryStore {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("memories.sqlite");
        std::mem::forget(temp);
        SqliteMemoryStore::open_path(path).expect("open store")
    }

    fn insert(store: &SqliteMemoryStore, scope: MemoryScope, content: &str, pinned: bool) {
        store
            .insert(NewMemoryRecord {
                scope,
                kind: MemoryKind::Fact,
                content: content.to_string(),
                source: MemorySource::UserExplicit,
                tags: vec!["recall".to_string()],
                pinned,
                confidence: 1.0,
                expires_at: None,
            })
            .unwrap();
    }

    #[test]
    fn recall_prefers_pinned_and_specific_scope() {
        let store = store();
        insert(&store, MemoryScope::Global, "global recall", false);
        insert(
            &store,
            MemoryScope::Session("s1".to_string()),
            "session recall",
            false,
        );
        insert(
            &store,
            MemoryScope::User("u1".to_string()),
            "pinned recall",
            true,
        );
        let recaller = DefaultMemoryRecaller::new(&store);
        let records = recaller
            .recall(RecallContext {
                user_id: Some("u1".to_string()),
                project_path: None,
                session_key: Some("s1".to_string()),
                latest_user_message: "recall".to_string(),
                history_tail_tokens: 0,
                max_items: 3,
                max_chars: 2000,
            })
            .unwrap();
        assert_eq!(records[0].content, "pinned recall");
        assert_eq!(records[1].content, "session recall");
    }

    #[test]
    fn recall_respects_max_chars() {
        let store = store();
        insert(&store, MemoryScope::Global, "short", false);
        insert(
            &store,
            MemoryScope::Global,
            "this is a much longer memory",
            false,
        );
        let recaller = DefaultMemoryRecaller::new(&store);
        let records = recaller
            .recall(RecallContext {
                user_id: None,
                project_path: None,
                session_key: None,
                latest_user_message: "memory".to_string(),
                history_tail_tokens: 0,
                max_items: 10,
                max_chars: 10,
            })
            .unwrap();
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn formats_stable_system_message() {
        let store = store();
        insert(&store, MemoryScope::Global, "stable content", false);
        let records = store
            .search(MemorySearchQuery {
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        let formatted = format_memory_system_message(&records).unwrap();
        assert!(formatted.starts_with("# Long-term memories (per user)\n- [fact global:*"));
    }
}
