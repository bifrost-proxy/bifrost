use crate::model::{MemoryOp, SkillRecord, ToolBinding};
use memory::{
    DefaultMemoryRecaller, MemoryKind, MemoryRecaller, MemoryScope, MemorySearchQuery,
    MemorySource, MemoryStore, NewMemoryRecord, RecallContext, SqliteMemoryStore,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MemoryToolRequest {
    Read {
        query: String,
        limit: Option<usize>,
    },
    Write {
        content: String,
        kind: MemoryKind,
        scope: MemoryScope,
        tags: Option<Vec<String>>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MemoryToolResponse {
    Read { records: Vec<memory::MemoryRecord> },
    Write { id: String },
}

#[derive(Clone, Debug)]
pub struct SkillToolBridge {
    store_root: std::path::PathBuf,
}

impl SkillToolBridge {
    pub fn new(store_root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            store_root: store_root.into(),
        }
    }

    pub fn memory_allowed(record: &SkillRecord, desired: MemoryOp) -> bool {
        record.manifest.allowed_tools.iter().any(|binding| {
            matches!(
                (binding, &desired),
                (ToolBinding::Memory { op: MemoryOp::Both }, _)
                    | (ToolBinding::Memory { op: MemoryOp::Read }, MemoryOp::Read)
                    | (
                        ToolBinding::Memory {
                            op: MemoryOp::Write
                        },
                        MemoryOp::Write
                    )
            )
        })
    }

    pub fn handle_memory(
        &self,
        record: &SkillRecord,
        request: MemoryToolRequest,
    ) -> Result<MemoryToolResponse, String> {
        let store = SqliteMemoryStore::open(&self.store_root)
            .map_err(|error| format!("open memory store: {error}"))?;
        match request {
            MemoryToolRequest::Read { query, limit } => {
                if !Self::memory_allowed(record, MemoryOp::Read) {
                    return Err("skill is not allowed to read memory".to_string());
                }
                let recaller = DefaultMemoryRecaller::new(&store);
                let records = recaller
                    .recall(RecallContext {
                        user_id: None,
                        project_path: current_project_path(),
                        session_key: None,
                        latest_user_message: query,
                        history_tail_tokens: 0,
                        max_items: limit.unwrap_or(8),
                        max_chars: 4000,
                    })
                    .map_err(|error| format!("recall memory: {error}"))?;
                Ok(MemoryToolResponse::Read { records })
            }
            MemoryToolRequest::Write {
                content,
                kind,
                scope,
                tags,
            } => {
                if !Self::memory_allowed(record, MemoryOp::Write) {
                    return Err("skill is not allowed to write memory".to_string());
                }
                let mut tags = tags.unwrap_or_default();
                tags.push(format!("source_skill={}@{}", record.name, record.version));
                let inserted = store
                    .insert(NewMemoryRecord {
                        scope,
                        kind,
                        content,
                        source: MemorySource::UserExplicit,
                        tags,
                        pinned: false,
                        confidence: 1.0,
                        expires_at: None,
                    })
                    .map_err(|error| format!("write memory: {error}"))?;
                Ok(MemoryToolResponse::Write {
                    id: inserted.id.to_string(),
                })
            }
        }
    }

    pub fn search_raw(
        &self,
        query: MemorySearchQuery,
    ) -> Result<Vec<memory::MemoryRecord>, String> {
        let store = SqliteMemoryStore::open(&self.store_root)
            .map_err(|error| format!("open memory store: {error}"))?;
        store
            .search(query)
            .map_err(|error| format!("search memory: {error}"))
    }
}

fn current_project_path() -> Option<String> {
    std::env::current_dir()
        .ok()
        .as_deref()
        .map(Path::display)
        .map(|display| display.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SkillManifest, SkillScope};

    #[test]
    fn memory_permission_checks_requested_operation() {
        let mut manifest = SkillManifest::minimal_inline("mem", "mem", SkillScope::Project);
        manifest.allowed_tools = vec![ToolBinding::Memory { op: MemoryOp::Read }];
        let record = SkillRecord {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            scope: SkillScope::Project,
            effective_scope: SkillScope::Project,
            shadow_scopes: Vec::new(),
            enabled: true,
            path: ".".into(),
            skill_md_path: "SKILL.md".into(),
            checksum: String::new(),
            manifest,
        };
        assert!(SkillToolBridge::memory_allowed(&record, MemoryOp::Read));
        assert!(!SkillToolBridge::memory_allowed(&record, MemoryOp::Write));
    }
}
