use crate::model::{MemoryOp, SkillRecord, ToolBinding};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Fact,
    Preference,
    Rule,
    Skill,
    TaskContext,
    Other,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum MemoryScope {
    Global,
    User(String),
    Project(String),
    Session(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryFileEntry {
    pub id: String,
    pub path: String,
    pub content: String,
}

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
    Read { records: Vec<MemoryFileEntry> },
    Write { id: String },
}

#[derive(Clone, Debug)]
pub struct SkillToolBridge {
    memory_root: PathBuf,
}

impl SkillToolBridge {
    pub fn new(memory_root: impl Into<PathBuf>) -> Self {
        Self {
            memory_root: memory_root.into(),
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
        ensure_layout(&self.memory_root)?;
        match request {
            MemoryToolRequest::Read { query, limit } => {
                if !Self::memory_allowed(record, MemoryOp::Read) {
                    return Err("skill is not allowed to read memory".to_string());
                }
                let records = search_memory_files(&self.memory_root, &query, limit.unwrap_or(8))?;
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
                let id = format!("skill-{}-{}", now_secs(), record.name);
                let tags = tags.unwrap_or_default().join(",");
                let line = format!(
                    "\n- id: `{id}`\n  source_skill: `{}@{}`\n  kind: {:?}\n  scope: {:?}\n  tags: {tags}\n  content: {}\n",
                    record.name,
                    record.version,
                    kind,
                    scope,
                    content.trim()
                );
                append_line(&self.memory_root.join("MEMORY.md"), &line)?;
                append_line(
                    &self.memory_root.join("memory_summary.md"),
                    &format!("- {}\n", content.trim()),
                )?;
                Ok(MemoryToolResponse::Write { id })
            }
        }
    }

    pub fn search_raw(&self, query: &str, limit: usize) -> Result<Vec<MemoryFileEntry>, String> {
        ensure_layout(&self.memory_root)?;
        search_memory_files(&self.memory_root, query, limit)
    }
}

fn ensure_layout(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root.join("rollout_summaries"))
        .map_err(|error| format!("create rollout_summaries: {error}"))?;
    fs::create_dir_all(root.join("skills")).map_err(|error| format!("create skills: {error}"))?;
    ensure_file(&root.join("MEMORY.md"), "# Memory\n\n")?;
    ensure_file(&root.join("memory_summary.md"), "")?;
    Ok(())
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

fn append_line(path: &Path, line: &str) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    file.write_all(line.as_bytes())
        .map_err(|error| format!("append {}: {error}", path.display()))
}

/// Maximum memory file size we will load (8 MiB).
const MAX_MEMORY_FILE_BYTES: u64 = 8 * 1024 * 1024;

fn search_memory_files(
    root: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<MemoryFileEntry>, String> {
    let query = query.trim().to_lowercase();
    let mut entries = Vec::new();
    for file_name in ["memory_summary.md", "MEMORY.md"] {
        let path = root.join(file_name);
        // Guard against OOM from unexpectedly large files.
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_MEMORY_FILE_BYTES {
                return Err(format!(
                    "memory file {} is too large ({} bytes, limit {} bytes)",
                    path.display(),
                    meta.len(),
                    MAX_MEMORY_FILE_BYTES
                ));
            }
        }
        let content = fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
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

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SkillManifest, SkillScope};

    fn record_with_tools(tools: Vec<ToolBinding>) -> SkillRecord {
        let mut manifest = SkillManifest::minimal_inline("mem", "mem", SkillScope::Repo);
        manifest.allowed_tools = tools;
        SkillRecord {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            scope: SkillScope::Repo,
            effective_scope: SkillScope::Repo,
            shadow_scopes: Vec::new(),
            enabled: true,
            path: ".".into(),
            skill_md_path: "SKILL.md".into(),
            checksum: String::new(),
            manifest,
        }
    }

    #[test]
    fn memory_permission_checks_requested_operation() {
        let record = record_with_tools(vec![ToolBinding::Memory { op: MemoryOp::Read }]);
        assert!(SkillToolBridge::memory_allowed(&record, MemoryOp::Read));
        assert!(!SkillToolBridge::memory_allowed(&record, MemoryOp::Write));
    }

    #[test]
    fn memory_bridge_reads_and_writes_files() {
        let temp = tempfile::tempdir().expect("temp dir");
        let bridge = SkillToolBridge::new(temp.path());
        let record = record_with_tools(vec![ToolBinding::Memory { op: MemoryOp::Both }]);
        let written = bridge
            .handle_memory(
                &record,
                MemoryToolRequest::Write {
                    content: "Skill memory content".to_string(),
                    kind: MemoryKind::Fact,
                    scope: MemoryScope::Global,
                    tags: None,
                },
            )
            .expect("write");
        assert!(matches!(written, MemoryToolResponse::Write { .. }));
        let read = bridge
            .handle_memory(
                &record,
                MemoryToolRequest::Read {
                    query: "Skill memory".to_string(),
                    limit: Some(4),
                },
            )
            .expect("read");
        let MemoryToolResponse::Read { records } = read else {
            panic!("expected read response");
        };
        assert!(records
            .iter()
            .any(|entry| entry.content.contains("Skill memory")));
        assert!(!temp.path().join("memories.sqlite").exists());
    }

    #[test]
    fn read_denied_without_permission() {
        let temp = tempfile::tempdir().unwrap();
        let bridge = SkillToolBridge::new(temp.path());
        let record = record_with_tools(vec![ToolBinding::Memory {
            op: MemoryOp::Write,
        }]);
        let err = bridge
            .handle_memory(
                &record,
                MemoryToolRequest::Read {
                    query: "x".into(),
                    limit: None,
                },
            )
            .unwrap_err();
        assert!(err.contains("not allowed to read"));
    }

    #[test]
    fn write_denied_without_permission() {
        let temp = tempfile::tempdir().unwrap();
        let bridge = SkillToolBridge::new(temp.path());
        let record = record_with_tools(vec![ToolBinding::Memory { op: MemoryOp::Read }]);
        let err = bridge
            .handle_memory(
                &record,
                MemoryToolRequest::Write {
                    content: "c".into(),
                    kind: MemoryKind::Rule,
                    scope: MemoryScope::User("u".into()),
                    tags: Some(vec!["t1".into(), "t2".into()]),
                },
            )
            .unwrap_err();
        assert!(err.contains("not allowed to write"));
    }

    #[test]
    fn both_op_grants_read_and_write() {
        let record = record_with_tools(vec![ToolBinding::Memory { op: MemoryOp::Both }]);
        assert!(SkillToolBridge::memory_allowed(&record, MemoryOp::Read));
        assert!(SkillToolBridge::memory_allowed(&record, MemoryOp::Write));
        // No memory binding at all -> denied.
        let none = record_with_tools(vec![]);
        assert!(!SkillToolBridge::memory_allowed(&none, MemoryOp::Read));
    }

    #[test]
    fn write_with_tags_and_search_raw() {
        let temp = tempfile::tempdir().unwrap();
        let bridge = SkillToolBridge::new(temp.path());
        let record = record_with_tools(vec![ToolBinding::Memory { op: MemoryOp::Both }]);
        bridge
            .handle_memory(
                &record,
                MemoryToolRequest::Write {
                    content: "Tagged note about widgets".into(),
                    kind: MemoryKind::Preference,
                    scope: MemoryScope::Project("p".into()),
                    tags: Some(vec!["a".into(), "b".into()]),
                },
            )
            .unwrap();
        // search_raw queries the same store directly.
        let hits = bridge.search_raw("widgets", 5).unwrap();
        assert!(hits.iter().any(|e| e.content.contains("widgets")));
        // Empty query returns all non-empty/non-heading lines (up to limit).
        let all = bridge.search_raw("", 1).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn ensure_layout_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        ensure_layout(temp.path()).unwrap();
        // Second call hits the "file exists" early-return in ensure_file.
        ensure_layout(temp.path()).unwrap();
        assert!(temp.path().join("MEMORY.md").exists());
        assert!(temp.path().join("skills").is_dir());
    }

    #[test]
    fn memory_kind_and_scope_round_trip() {
        let v = serde_json::to_value(MemoryKind::TaskContext).unwrap();
        assert_eq!(v, serde_json::json!("task_context"));
        let s = serde_json::to_value(MemoryScope::Session("s".into())).unwrap();
        assert_eq!(s, serde_json::json!({"type":"session","value":"s"}));
    }
}
