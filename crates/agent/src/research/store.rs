use super::normalize::{canonical_url, content_hash, result_id};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeItem {
    pub id: String,
    pub source: String,
    pub provider: String,
    pub query: Option<String>,
    pub title: String,
    pub url: String,
    pub canonical_url: String,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub content_markdown: Option<String>,
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub content_hash: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct KnowledgeItemInput {
    pub source: String,
    pub provider: String,
    pub query: Option<String>,
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub content_markdown: Option<String>,
    pub summary: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeSearchResult {
    pub id: String,
    pub source: String,
    pub provider: String,
    pub title: String,
    pub url: String,
    pub canonical_url: String,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub summary: Option<String>,
    pub matched_text: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeSaveReport {
    pub saved: usize,
    pub duplicates: usize,
}

#[derive(Debug, Clone)]
pub struct KnowledgeStore {
    db_path: PathBuf,
}

impl KnowledgeStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: path.into(),
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn init(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = self.open()?;
        conn.execute_batch(
            r#"
CREATE TABLE IF NOT EXISTS knowledge_items (
  id TEXT PRIMARY KEY,
  source TEXT NOT NULL,
  provider TEXT NOT NULL,
  query TEXT,
  title TEXT NOT NULL,
  url TEXT NOT NULL,
  canonical_url TEXT NOT NULL,
  author TEXT,
  published_at TEXT,
  content_markdown TEXT,
  summary TEXT,
  tags_json TEXT NOT NULL DEFAULT '[]',
  content_hash TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_knowledge_canonical
ON knowledge_items(canonical_url);
CREATE INDEX IF NOT EXISTS idx_knowledge_created
ON knowledge_items(created_at);
CREATE INDEX IF NOT EXISTS idx_knowledge_content_hash
ON knowledge_items(content_hash);
CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_fts
USING fts5(
  id UNINDEXED,
  title,
  summary,
  content_markdown,
  tokenize='unicode61'
);
"#,
        )?;
        Ok(())
    }

    pub fn upsert_items(&self, items: &[KnowledgeItem]) -> anyhow::Result<KnowledgeSaveReport> {
        self.init()?;
        let mut saved = 0usize;
        let mut duplicates = 0usize;
        let mut conn = self.open()?;
        let tx = conn.transaction()?;
        for raw in items {
            let mut item = raw.clone();
            item.canonical_url = canonical_url(&item.url);
            item.id = if item.id.trim().is_empty() {
                result_id(&item.canonical_url)
            } else {
                item.id.clone()
            };
            if item.content_hash.is_none() {
                item.content_hash = item
                    .content_markdown
                    .as_deref()
                    .filter(|content| !content.trim().is_empty())
                    .map(content_hash);
            }
            let existing: Option<String> = tx
                .query_row(
                    "SELECT id FROM knowledge_items WHERE canonical_url = ?1 OR (content_hash IS NOT NULL AND content_hash = ?2) LIMIT 1",
                    params![item.canonical_url, item.content_hash],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(existing_id) = existing {
                duplicates += 1;
                item.id = existing_id;
            }
            let tags_json = serde_json::to_string(&item.tags)?;
            tx.execute(
                r#"
INSERT INTO knowledge_items (
  id, source, provider, query, title, url, canonical_url, author, published_at,
  content_markdown, summary, tags_json, content_hash, created_at, updated_at
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
ON CONFLICT(id) DO UPDATE SET
  source=excluded.source,
  provider=excluded.provider,
  query=COALESCE(excluded.query, knowledge_items.query),
  title=excluded.title,
  url=excluded.url,
  canonical_url=excluded.canonical_url,
  author=COALESCE(excluded.author, knowledge_items.author),
  published_at=COALESCE(excluded.published_at, knowledge_items.published_at),
  content_markdown=COALESCE(excluded.content_markdown, knowledge_items.content_markdown),
  summary=COALESCE(excluded.summary, knowledge_items.summary),
  tags_json=excluded.tags_json,
  content_hash=COALESCE(excluded.content_hash, knowledge_items.content_hash),
  updated_at=excluded.updated_at
"#,
                params![
                    item.id,
                    item.source,
                    item.provider,
                    item.query,
                    item.title,
                    item.url,
                    item.canonical_url,
                    item.author,
                    item.published_at,
                    item.content_markdown,
                    item.summary,
                    tags_json,
                    item.content_hash,
                    item.created_at,
                    item.updated_at,
                ],
            )?;
            tx.execute("DELETE FROM knowledge_fts WHERE id = ?1", params![item.id])?;
            tx.execute(
                "INSERT INTO knowledge_fts (id, title, summary, content_markdown) VALUES (?1, ?2, ?3, ?4)",
                params![item.id, item.title, item.summary, item.content_markdown],
            )?;
            saved += 1;
        }
        tx.commit()?;
        Ok(KnowledgeSaveReport { saved, duplicates })
    }

    pub fn search(
        &self,
        query: &str,
        limit: usize,
        since_days: Option<u32>,
    ) -> anyhow::Result<Vec<KnowledgeSearchResult>> {
        self.init()?;
        let conn = self.open()?;
        let since = since_days.map(|days| now_unix() - (days as i64 * 86_400));
        let mut stmt = conn.prepare(
            r#"
SELECT k.id, k.source, k.provider, k.title, k.url, k.canonical_url,
       k.author, k.published_at, k.summary,
       snippet(knowledge_fts, 3, '[', ']', '...', 16) AS matched_text,
       k.created_at
FROM knowledge_fts
JOIN knowledge_items k ON k.id = knowledge_fts.id
WHERE knowledge_fts MATCH ?1
  AND (?2 IS NULL OR k.created_at >= ?2)
ORDER BY rank
LIMIT ?3
"#,
        )?;
        let rows = stmt.query_map(params![query, since, limit as i64], map_search_row)?;
        let results = rows.collect::<Result<Vec<_>, _>>()?;
        if !results.is_empty() {
            return Ok(results);
        }

        let mut stmt = conn.prepare(
            r#"
SELECT id, source, provider, title, url, canonical_url,
       author, published_at, summary,
       COALESCE(summary, substr(content_markdown, 1, 240), title) AS matched_text,
       created_at
FROM knowledge_items
WHERE (title LIKE '%' || ?1 || '%'
    OR summary LIKE '%' || ?1 || '%'
    OR content_markdown LIKE '%' || ?1 || '%'
    OR query LIKE '%' || ?1 || '%')
  AND (?2 IS NULL OR created_at >= ?2)
ORDER BY updated_at DESC
LIMIT ?3
"#,
        )?;
        let rows = stmt.query_map(params![query, since, limit as i64], map_search_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn stats(&self) -> anyhow::Result<KnowledgeStoreStats> {
        self.init()?;
        let conn = self.open()?;
        let item_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM knowledge_items", [], |row| row.get(0))?;
        let last_indexed_at: Option<i64> =
            conn.query_row("SELECT MAX(updated_at) FROM knowledge_items", [], |row| {
                row.get(0)
            })?;
        Ok(KnowledgeStoreStats {
            db_path: self.db_path.display().to_string(),
            item_count,
            last_indexed_at,
        })
    }

    fn open(&self) -> anyhow::Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeStoreStats {
    pub db_path: String,
    pub item_count: i64,
    pub last_indexed_at: Option<i64>,
}

fn map_search_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgeSearchResult> {
    Ok(KnowledgeSearchResult {
        id: row.get(0)?,
        source: row.get(1)?,
        provider: row.get(2)?,
        title: row.get(3)?,
        url: row.get(4)?,
        canonical_url: row.get(5)?,
        author: row.get(6)?,
        published_at: row.get(7)?,
        summary: row.get(8)?,
        matched_text: row.get(9)?,
        created_at: row.get(10)?,
    })
}

pub fn item_from_input(input: KnowledgeItemInput) -> KnowledgeItem {
    let canonical = canonical_url(&input.url);
    let now = now_unix();
    let hash = input
        .content_markdown
        .as_deref()
        .filter(|content| !content.trim().is_empty())
        .map(content_hash);
    KnowledgeItem {
        id: result_id(&canonical),
        source: input.source,
        provider: input.provider,
        query: input.query,
        title: input.title,
        url: input.url,
        canonical_url: canonical,
        author: input.author,
        published_at: input.published_at,
        content_markdown: input.content_markdown,
        summary: input.summary,
        tags: input.tags,
        content_hash: hash,
        created_at: now,
        updated_at: now,
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_search_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = KnowledgeStore::new(dir.path().join("research.db"));
        let item = item_from_input(KnowledgeItemInput {
            source: "web".to_string(),
            provider: "test".to_string(),
            query: Some("mcp".to_string()),
            title: "MCP Search".to_string(),
            url: "https://example.com/a#frag".to_string(),
            author: Some("Researcher".to_string()),
            published_at: Some("2026-05-13".to_string()),
            content_markdown: Some("Model context protocol web search".to_string()),
            summary: Some("A useful MCP search note".to_string()),
            tags: vec!["mcp".to_string()],
        });
        let report = store.upsert_items(&[item]).unwrap();
        assert_eq!(report.saved, 1);
        let results = store.search("MCP", 10, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].provider, "test");
        assert_eq!(results[0].author.as_deref(), Some("Researcher"));
    }

    #[test]
    fn chinese_query_falls_back_to_like_search() {
        let dir = tempfile::tempdir().unwrap();
        let store = KnowledgeStore::new(dir.path().join("research.db"));
        let item = item_from_input(KnowledgeItemInput {
            source: "web".to_string(),
            provider: "test".to_string(),
            query: Some("语音大模型".to_string()),
            title: "语音大模型技术观察".to_string(),
            url: "https://example.com/voice-model".to_string(),
            author: None,
            published_at: Some("2026-05-13".to_string()),
            content_markdown: Some("语音大模型正在融合 ASR、TTS 和实时交互。".to_string()),
            summary: Some("中文语音大模型资料".to_string()),
            tags: vec!["voice-model".to_string()],
        });
        store.upsert_items(&[item]).unwrap();

        let results = store.search("语音大模型", 10, None).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "语音大模型技术观察");
        assert_eq!(results[0].provider, "test");
    }

    #[test]
    fn canonical_url_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let store = KnowledgeStore::new(dir.path().join("research.db"));
        let first = item_from_input(KnowledgeItemInput {
            source: "web".to_string(),
            provider: "test".to_string(),
            query: None,
            title: "One".to_string(),
            url: "https://example.com/a#one".to_string(),
            author: None,
            published_at: None,
            content_markdown: Some("alpha".to_string()),
            summary: None,
            tags: vec![],
        });
        let second = item_from_input(KnowledgeItemInput {
            source: "web".to_string(),
            provider: "test".to_string(),
            query: None,
            title: "One updated".to_string(),
            url: "https://example.com/a#two".to_string(),
            author: None,
            published_at: None,
            content_markdown: Some("alpha".to_string()),
            summary: None,
            tags: vec![],
        });
        let report = store.upsert_items(&[first, second]).unwrap();
        assert_eq!(report.duplicates, 1);
    }
}
