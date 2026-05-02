use crate::redact::Redactor;
use crate::schema::MEMORY_SCHEMA_V1;
use crate::types::{
    MemoryId, MemoryPatch, MemoryRecord, MemoryScope, MemorySearchQuery, MemorySource, MemoryStats,
    NewMemoryRecord,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;
use tracing::{info, info_span};

/// 记忆存储错误。
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("record not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

/// 记忆存储结果。
pub type Result<T> = std::result::Result<T, StoreError>;

/// JSONL 导入报告。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub inserted: usize,
    pub deduped: usize,
    pub failed: usize,
}

/// GC 策略。
#[derive(Debug, Clone)]
pub struct GcPolicy {
    pub now: u64,
    pub max_unused_days: Option<i64>,
    pub tombstone_path: Option<PathBuf>,
}

/// GC 报告。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcReport {
    pub soft_deleted: usize,
}

/// 长期记忆存储接口。
pub trait MemoryStore: Send + Sync {
    /// 插入或按 scope/dedupe 去重更新一条记忆。
    fn insert(&self, record: NewMemoryRecord) -> Result<MemoryRecord>;
    /// 更新一条记忆。
    fn update(&self, id: &MemoryId, patch: MemoryPatch) -> Result<MemoryRecord>;
    /// 软删除一条记忆。
    fn soft_delete(&self, id: &MemoryId) -> Result<bool>;
    /// 按 ID 获取记忆。
    fn get(&self, id: &MemoryId) -> Result<Option<MemoryRecord>>;
    /// 搜索记忆。
    fn search(&self, query: MemorySearchQuery) -> Result<Vec<MemoryRecord>>;
    /// 标记记忆被召回使用。
    fn mark_used(&self, ids: &[MemoryId], now: u64) -> Result<usize>;
    /// 获取统计信息。
    fn stats(&self, now: u64) -> Result<MemoryStats>;
    /// 导出 JSONL。
    fn export_jsonl<W: Write>(&self, writer: W) -> Result<usize>;
    /// 导入 JSONL。
    fn import_jsonl<R: BufRead>(&self, reader: R) -> Result<ImportReport>;
    /// 执行 GC。
    fn gc(&self, policy: GcPolicy) -> Result<GcReport>;
}

/// SQLite 记忆存储。
pub struct SqliteMemoryStore {
    db_path: PathBuf,
    conn: Mutex<Connection>,
    redactor: Redactor,
}

impl SqliteMemoryStore {
    /// 打开或创建 `$agent_home/memory/memories.sqlite`。
    pub fn open(agent_home: impl AsRef<Path>) -> Result<Self> {
        let memory_dir = agent_home.as_ref().join("memory");
        std::fs::create_dir_all(&memory_dir)?;
        Self::open_path(memory_dir.join("memories.sqlite"))
    }

    /// 打开指定 SQLite 文件。
    pub fn open_path(db_path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = db_path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path.as_ref())?;
        conn.execute_batch(MEMORY_SCHEMA_V1)?;
        Ok(Self {
            db_path: db_path.as_ref().to_path_buf(),
            conn: Mutex::new(conn),
            redactor: Redactor::new(),
        })
    }

    /// 返回底层数据库路径。
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("memory sqlite mutex poisoned")
    }
}

impl MemoryStore for SqliteMemoryStore {
    fn insert(&self, record: NewMemoryRecord) -> Result<MemoryRecord> {
        let _span = info_span!(
            "memory.insert",
            scope = %record.scope.scope_kind(),
            kind = record.kind.as_str()
        )
        .entered();
        let now = Self::now();
        let content = normalize_content(&self.redactor.redact(&record.content));
        if content.is_empty() {
            return Err(StoreError::InvalidInput(
                "content must not be empty".to_string(),
            ));
        }
        let tags = sanitize_tags(record.tags);
        let dedupe_hash = dedupe_hash(&content);
        let scope_kind = record.scope.scope_kind();
        let conn = self.conn();

        if let Some(existing) = find_by_scope_dedupe(&conn, &scope_kind, &dedupe_hash)? {
            conn.execute(
                "UPDATE memories
                 SET use_count = use_count + 1, updated_at = ?1, last_used_at = ?1
                 WHERE id = ?2",
                params![now as i64, existing.id.as_str()],
            )?;
            info!(id = %existing.id, "memory dedupe hit");
            return row_by_id(&conn, existing.id.as_str())?
                .ok_or_else(|| StoreError::NotFound(existing.id.to_string()));
        }

        let id = MemoryId::new();
        conn.execute(
            "INSERT INTO memories (
                id, scope_kind, scope_type, scope_value, kind, content, source_json, tags_json,
                pinned, confidence, created_at, updated_at, last_used_at, use_count, expires_at,
                dedupe_hash, deleted_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, NULL, 0, ?12, ?13, NULL)",
            params![
                id.as_str(),
                scope_kind,
                record.scope.scope_type(),
                record.scope.scope_value(),
                record.kind.as_str(),
                content,
                serde_json::to_string(&record.source)?,
                serde_json::to_string(&tags)?,
                record.pinned as i64,
                clamp_confidence(record.confidence),
                now as i64,
                record.expires_at.map(|value| value as i64),
                dedupe_hash,
            ],
        )?;
        let inserted =
            row_by_id(&conn, id.as_str())?.ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        info!(id = %inserted.id, "memory inserted");
        Ok(inserted)
    }

    fn update(&self, id: &MemoryId, patch: MemoryPatch) -> Result<MemoryRecord> {
        let _span = info_span!("memory.update", id = %id).entered();
        let conn = self.conn();
        let mut record =
            row_by_id(&conn, id.as_str())?.ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        if let Some(scope) = patch.scope {
            record.scope = scope;
        }
        if let Some(kind) = patch.kind {
            record.kind = kind;
        }
        if let Some(content) = patch.content {
            record.content = normalize_content(&self.redactor.redact(&content));
        }
        if let Some(tags) = patch.tags {
            record.tags = sanitize_tags(tags);
        }
        if let Some(pinned) = patch.pinned {
            record.pinned = pinned;
        }
        if let Some(confidence) = patch.confidence {
            record.confidence = clamp_confidence(confidence);
        }
        if let Some(expires_at) = patch.expires_at {
            record.expires_at = expires_at;
        }
        record.updated_at = Self::now();
        record.dedupe_hash = dedupe_hash(&record.content);

        conn.execute(
            "UPDATE memories
             SET scope_kind = ?1, scope_type = ?2, scope_value = ?3, kind = ?4, content = ?5,
                 tags_json = ?6, pinned = ?7, confidence = ?8, updated_at = ?9,
                 expires_at = ?10, dedupe_hash = ?11
             WHERE id = ?12 AND deleted_at IS NULL",
            params![
                record.scope.scope_kind(),
                record.scope.scope_type(),
                record.scope.scope_value(),
                record.kind.as_str(),
                record.content,
                serde_json::to_string(&record.tags)?,
                record.pinned as i64,
                record.confidence,
                record.updated_at as i64,
                record.expires_at.map(|value| value as i64),
                record.dedupe_hash,
                id.as_str(),
            ],
        )?;
        row_by_id(&conn, id.as_str())?.ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    fn soft_delete(&self, id: &MemoryId) -> Result<bool> {
        let _span = info_span!("memory.delete", id = %id).entered();
        let changed = self.conn().execute(
            "UPDATE memories SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            params![Self::now() as i64, id.as_str()],
        )?;
        Ok(changed > 0)
    }

    fn get(&self, id: &MemoryId) -> Result<Option<MemoryRecord>> {
        row_by_id(&self.conn(), id.as_str())
    }

    fn search(&self, query: MemorySearchQuery) -> Result<Vec<MemoryRecord>> {
        let conn = self.conn();
        let mut records = load_records(&conn, query.include_deleted)?;

        if !query.scopes.is_empty() {
            let allowed: HashSet<String> =
                query.scopes.iter().map(MemoryScope::scope_kind).collect();
            records.retain(|record| allowed.contains(&record.scope.scope_kind()));
        }
        if let Some(kind) = query.kind {
            records.retain(|record| record.kind == kind);
        }
        if let Some(tag) = query.tag {
            let tag = sanitize_tag(&tag);
            records.retain(|record| record.tags.iter().any(|candidate| candidate == &tag));
        }
        if let Some(text) = query.query.filter(|value| !value.trim().is_empty()) {
            let lower = text.to_lowercase();
            let fts_ids = fts_matching_ids(&conn, &text).unwrap_or_default();
            records.retain(|record| {
                fts_ids.contains(record.id.as_str())
                    || record.content.to_lowercase().contains(&lower)
                    || record.tags.iter().any(|tag| tag.contains(&lower))
            });
        }

        records.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| b.use_count.cmp(&a.use_count))
        });
        let limit = if query.limit == 0 { 100 } else { query.limit };
        Ok(records.into_iter().skip(query.offset).take(limit).collect())
    }

    fn mark_used(&self, ids: &[MemoryId], now: u64) -> Result<usize> {
        let _span = info_span!("memory.recall.mark_used", count = ids.len()).entered();
        let conn = self.conn();
        let mut changed = 0usize;
        for id in ids {
            changed += conn.execute(
                "UPDATE memories SET use_count = use_count + 1, last_used_at = ?1, updated_at = ?1
                 WHERE id = ?2 AND deleted_at IS NULL",
                params![now as i64, id.as_str()],
            )?;
        }
        Ok(changed)
    }

    fn stats(&self, now: u64) -> Result<MemoryStats> {
        let conn = self.conn();
        let total = conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE deleted_at IS NULL",
            [],
            |row| row.get::<_, u64>(0),
        )?;
        let by_scope = grouped_counts(&conn, "scope_type")?;
        let by_kind = grouped_counts(&conn, "kind")?;
        let seven_days_ago = now.saturating_sub(7 * 24 * 60 * 60);
        let written_last_7_days = conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE deleted_at IS NULL AND created_at >= ?1",
            params![seven_days_ago as i64],
            |row| row.get::<_, u64>(0),
        )?;
        let recalled_last_7_days = conn.query_row(
            "SELECT COALESCE(SUM(use_count), 0) FROM memories
             WHERE deleted_at IS NULL AND last_used_at >= ?1",
            params![seven_days_ago as i64],
            |row| row.get::<_, u64>(0),
        )?;
        Ok(MemoryStats {
            total,
            by_scope,
            by_kind,
            written_last_7_days,
            recalled_last_7_days,
        })
    }

    fn export_jsonl<W: Write>(&self, mut writer: W) -> Result<usize> {
        let records = load_records(&self.conn(), false)?;
        for record in &records {
            writeln!(writer, "{}", serde_json::to_string(record)?)?;
        }
        Ok(records.len())
    }

    fn import_jsonl<R: BufRead>(&self, reader: R) -> Result<ImportReport> {
        let mut report = ImportReport::default();
        for line in reader.lines() {
            let line = match line {
                Ok(line) if !line.trim().is_empty() => line,
                Ok(_) => continue,
                Err(_) => {
                    report.failed += 1;
                    continue;
                }
            };
            let parsed: MemoryRecord = match serde_json::from_str(&line) {
                Ok(record) => record,
                Err(_) => {
                    report.failed += 1;
                    continue;
                }
            };
            let before = self
                .search(MemorySearchQuery {
                    scopes: vec![parsed.scope.clone()],
                    query: Some(parsed.content.clone()),
                    limit: 1,
                    ..Default::default()
                })?
                .len();
            self.insert(NewMemoryRecord {
                scope: parsed.scope,
                kind: parsed.kind,
                content: parsed.content,
                source: MemorySource::Import,
                tags: parsed.tags,
                pinned: parsed.pinned,
                confidence: parsed.confidence,
                expires_at: parsed.expires_at,
            })?;
            if before == 0 {
                report.inserted += 1;
            } else {
                report.deduped += 1;
            }
        }
        Ok(report)
    }

    fn gc(&self, policy: GcPolicy) -> Result<GcReport> {
        let _span = info_span!("memory.gc").entered();
        let Some(max_unused_days) = policy.max_unused_days else {
            return Ok(GcReport::default());
        };
        if max_unused_days < 0 {
            return Ok(GcReport::default());
        }
        let cutoff = policy
            .now
            .saturating_sub(max_unused_days as u64 * 24 * 60 * 60);
        let conn = self.conn();
        let mut candidates = conn.prepare(
            "SELECT id FROM memories
             WHERE deleted_at IS NULL AND pinned = 0
               AND COALESCE(last_used_at, updated_at) <= ?1",
        )?;
        let ids: Vec<String> = candidates
            .query_map(params![cutoff as i64], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(candidates);

        let mut tombstone = if let Some(path) = &policy.tombstone_path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            Some(OpenOptions::new().create(true).append(true).open(path)?)
        } else {
            None
        };
        let mut deleted = 0usize;
        for id in ids {
            if let Some(record) = row_by_id(&conn, &id)? {
                conn.execute(
                    "UPDATE memories SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2",
                    params![policy.now as i64, id],
                )?;
                deleted += 1;
                if let Some(file) = tombstone.as_mut() {
                    writeln!(file, "{}", serde_json::to_string(&record)?)?;
                }
            }
        }
        info!(soft_deleted = deleted, "memory gc complete");
        Ok(GcReport {
            soft_deleted: deleted,
        })
    }
}

/// 把内容规范化为 dedupe 与存储用正文。
pub fn normalize_content(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 生成稳定 dedupe hash。
pub fn dedupe_hash(content: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in content.to_lowercase().as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// 规范化 tag。
pub fn sanitize_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for tag in tags {
        let tag = sanitize_tag(&tag);
        if !tag.is_empty() && seen.insert(tag.clone()) {
            output.push(tag);
        }
    }
    output
}

fn sanitize_tag(tag: &str) -> String {
    tag.trim()
        .to_lowercase()
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                Some(ch)
            } else if ch.is_whitespace() {
                Some('_')
            } else {
                None
            }
        })
        .collect()
}

fn clamp_confidence(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn find_by_scope_dedupe(
    conn: &Connection,
    scope_kind: &str,
    dedupe_hash: &str,
) -> Result<Option<MemoryRecord>> {
    conn.query_row(
        "SELECT * FROM memories WHERE scope_kind = ?1 AND dedupe_hash = ?2 AND deleted_at IS NULL LIMIT 1",
        params![scope_kind, dedupe_hash],
        row_to_record,
    )
    .optional()
    .map_err(StoreError::from)
}

fn row_by_id(conn: &Connection, id: &str) -> Result<Option<MemoryRecord>> {
    conn.query_row(
        "SELECT * FROM memories WHERE id = ?1 AND deleted_at IS NULL",
        params![id],
        row_to_record,
    )
    .optional()
    .map_err(StoreError::from)
}

fn load_records(conn: &Connection, include_deleted: bool) -> Result<Vec<MemoryRecord>> {
    let sql = if include_deleted {
        "SELECT * FROM memories"
    } else {
        "SELECT * FROM memories WHERE deleted_at IS NULL"
    };
    let mut stmt = conn.prepare(sql)?;
    let records = stmt
        .query_map([], row_to_record)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(records)
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRecord> {
    let scope_type: String = row.get("scope_type")?;
    let scope_value: Option<String> = row.get("scope_value")?;
    let kind: String = row.get("kind")?;
    let source_json: String = row.get("source_json")?;
    let tags_json: String = row.get("tags_json")?;
    let scope = match scope_type.as_str() {
        "global" => MemoryScope::Global,
        "user" => MemoryScope::User(scope_value.unwrap_or_default()),
        "project" => MemoryScope::Project(scope_value.unwrap_or_default()),
        "session" => MemoryScope::Session(scope_value.unwrap_or_default()),
        _ => MemoryScope::Global,
    };
    let source = serde_json::from_str::<MemorySource>(&source_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let tags = serde_json::from_str::<Vec<String>>(&tags_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(MemoryRecord {
        id: MemoryId::from_string(row.get::<_, String>("id")?),
        scope,
        kind: kind.parse().map_err(|error: String| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
            )
        })?,
        content: row.get("content")?,
        source,
        tags,
        pinned: row.get::<_, i64>("pinned")? != 0,
        confidence: row.get::<_, f32>("confidence")?,
        created_at: row.get::<_, i64>("created_at")? as u64,
        updated_at: row.get::<_, i64>("updated_at")? as u64,
        last_used_at: row
            .get::<_, Option<i64>>("last_used_at")?
            .map(|value| value as u64),
        use_count: row.get::<_, i64>("use_count")? as u32,
        expires_at: row
            .get::<_, Option<i64>>("expires_at")?
            .map(|value| value as u64),
        dedupe_hash: row.get("dedupe_hash")?,
    })
}

fn fts_matching_ids(conn: &Connection, text: &str) -> Result<HashSet<String>> {
    let escaped = text.replace('"', "\"\"");
    let query = format!("\"{escaped}\"");
    let mut stmt = conn.prepare("SELECT id FROM memories_fts WHERE memories_fts MATCH ?1")?;
    let ids = stmt
        .query_map(params![query], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<HashSet<_>, _>>()?;
    Ok(ids)
}

fn grouped_counts(conn: &Connection, column: &str) -> Result<Vec<(String, u64)>> {
    let sql = format!(
        "SELECT {column}, COUNT(*) FROM memories WHERE deleted_at IS NULL GROUP BY {column}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 打开 export 文件并写入 JSONL。
pub fn export_to_path(store: &impl MemoryStore, path: impl AsRef<Path>) -> Result<usize> {
    if let Some(parent) = path.as_ref().parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(path)?;
    store.export_jsonl(file)
}

/// 从 JSONL 文件导入。
pub fn import_from_path(store: &impl MemoryStore, path: impl AsRef<Path>) -> Result<ImportReport> {
    let file = File::open(path)?;
    store.import_jsonl(BufReader::new(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MemoryKind;
    use tempfile::TempDir;

    fn store() -> SqliteMemoryStore {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("memories.sqlite");
        std::mem::forget(temp);
        SqliteMemoryStore::open_path(path).expect("open store")
    }

    fn new_record(content: &str) -> NewMemoryRecord {
        NewMemoryRecord {
            scope: MemoryScope::User("u1".to_string()),
            kind: MemoryKind::Fact,
            content: content.to_string(),
            source: MemorySource::UserExplicit,
            tags: vec!["Rust Lang".to_string(), "rust_lang".to_string()],
            pinned: false,
            confidence: 0.8,
            expires_at: None,
        }
    }

    #[test]
    fn insert_and_get_round_trip() {
        let store = store();
        let record = store
            .insert(new_record("Bifrost prefers deterministic memory recall"))
            .unwrap();
        let loaded = store.get(&record.id).unwrap().unwrap();
        assert_eq!(
            loaded.content,
            "Bifrost prefers deterministic memory recall"
        );
        assert_eq!(loaded.tags, vec!["rust_lang"]);
    }

    #[test]
    fn insert_dedupes_within_scope() {
        let store = store();
        let first = store.insert(new_record("same memory")).unwrap();
        let second = store.insert(new_record("same   memory")).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(second.use_count, 1);
    }

    #[test]
    fn update_changes_content_and_pin() {
        let store = store();
        let record = store.insert(new_record("old")).unwrap();
        let updated = store
            .update(
                &record.id,
                MemoryPatch {
                    content: Some("new content".to_string()),
                    pinned: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(updated.content, "new content");
        assert!(updated.pinned);
    }

    #[test]
    fn update_missing_record_returns_error() {
        let store = store();
        let error = store
            .update(&MemoryId::from_string("missing"), MemoryPatch::default())
            .unwrap_err();
        assert!(matches!(error, StoreError::NotFound(_)));
    }

    #[test]
    fn delete_hides_record() {
        let store = store();
        let record = store.insert(new_record("delete me")).unwrap();
        assert!(store.soft_delete(&record.id).unwrap());
        assert!(store.get(&record.id).unwrap().is_none());
    }

    #[test]
    fn delete_missing_returns_false() {
        let store = store();
        assert!(!store
            .soft_delete(&MemoryId::from_string("missing"))
            .unwrap());
    }

    #[test]
    fn search_matches_fts_and_tags() {
        let store = store();
        store
            .insert(new_record("deterministic recall for shell tasks"))
            .unwrap();
        let results = store
            .search(MemorySearchQuery {
                query: Some("recall".to_string()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_chinese_falls_back_to_like() {
        let store = store();
        store.insert(new_record("用户偏好中文回答")).unwrap();
        let results = store
            .search(MemorySearchQuery {
                query: Some("中文".to_string()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn stats_counts_scope_and_kind() {
        let store = store();
        store.insert(new_record("one")).unwrap();
        let stats = store.stats(SqliteMemoryStore::now()).unwrap();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.written_last_7_days, 1);
    }

    #[test]
    fn stats_empty_store_is_zero() {
        let store = store();
        let stats = store.stats(SqliteMemoryStore::now()).unwrap();
        assert_eq!(stats.total, 0);
    }

    #[test]
    fn export_import_jsonl_round_trip() {
        let source = store();
        source.insert(new_record("export me")).unwrap();
        let mut jsonl = Vec::new();
        assert_eq!(source.export_jsonl(&mut jsonl).unwrap(), 1);

        let target = store();
        let report = target
            .import_jsonl(BufReader::new(jsonl.as_slice()))
            .unwrap();
        assert_eq!(report.inserted, 1);
        assert_eq!(target.stats(SqliteMemoryStore::now()).unwrap().total, 1);
    }

    #[test]
    fn import_bad_json_counts_failure() {
        let store = store();
        let report = store
            .import_jsonl(BufReader::new(b"not-json\n".as_slice()))
            .unwrap();
        assert_eq!(report.failed, 1);
    }

    #[test]
    fn gc_soft_deletes_unused_unpinned() {
        let store = store();
        let record = store.insert(new_record("old unpinned")).unwrap();
        {
            let conn = store.conn();
            conn.execute(
                "UPDATE memories SET updated_at = 1 WHERE id = ?1",
                params![record.id.as_str()],
            )
            .unwrap();
        }
        let report = store
            .gc(GcPolicy {
                now: 10 * 24 * 60 * 60,
                max_unused_days: Some(1),
                tombstone_path: None,
            })
            .unwrap();
        assert_eq!(report.soft_deleted, 1);
    }

    #[test]
    fn gc_keeps_pinned_memory() {
        let store = store();
        let mut input = new_record("old pinned");
        input.pinned = true;
        let record = store.insert(input).unwrap();
        {
            let conn = store.conn();
            conn.execute(
                "UPDATE memories SET updated_at = 1 WHERE id = ?1",
                params![record.id.as_str()],
            )
            .unwrap();
        }
        let report = store
            .gc(GcPolicy {
                now: 10 * 24 * 60 * 60,
                max_unused_days: Some(1),
                tombstone_path: None,
            })
            .unwrap();
        assert_eq!(report.soft_deleted, 0);
        assert!(store.get(&record.id).unwrap().is_some());
    }

    #[test]
    fn mark_used_updates_count() {
        let store = store();
        let record = store.insert(new_record("use me")).unwrap();
        assert_eq!(
            store
                .mark_used(std::slice::from_ref(&record.id), 123)
                .unwrap(),
            1
        );
        let loaded = store.get(&record.id).unwrap().unwrap();
        assert_eq!(loaded.use_count, 1);
        assert_eq!(loaded.last_used_at, Some(123));
    }

    #[test]
    fn normalizes_content_for_dedupe() {
        assert_eq!(dedupe_hash("Hello world"), dedupe_hash("hello world"));
        assert_eq!(normalize_content(" hello\n  world "), "hello world");
    }
}
