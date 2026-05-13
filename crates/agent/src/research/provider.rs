use super::config::{Freshness, ResearchSource};
use super::normalize::canonical_url;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchSearchRequest {
    pub query: String,
    #[serde(default)]
    pub sources: Vec<ResearchSource>,
    #[serde(default)]
    pub provider_ids: Vec<String>,
    pub freshness: Option<Freshness>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub fetch_content: bool,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchSearchResponse {
    pub query: String,
    pub results: Vec<ResearchSearchResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchSearchProviderEvent {
    pub provider_id: String,
    pub results: Vec<ResearchSearchResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchSearchResult {
    pub id: String,
    pub source: ResearchSource,
    pub provider: String,
    pub title: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_url: Option<String>,
    pub snippet: Option<String>,
    pub site_name: Option<String>,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub score: Option<f32>,
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_markdown: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieved_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchFetchRequest {
    pub url: String,
    #[serde(default = "default_fetch_format")]
    pub format: String,
    pub max_bytes: Option<usize>,
}

fn default_fetch_format() -> String {
    "markdown".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedDocument {
    pub url: String,
    pub canonical_url: String,
    pub source: Option<ResearchSource>,
    pub provider: Option<String>,
    pub site_name: Option<String>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub content_markdown: String,
    pub content_hash: String,
    pub fetched_at: i64,
    pub retrieved_at: i64,
    pub markdown_artifact: MarkdownArtifact,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkdownArtifact {
    pub title: Option<String>,
    pub url: String,
    pub canonical_url: String,
    pub source: Option<ResearchSource>,
    pub provider: Option<String>,
    pub site_name: Option<String>,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub retrieved_at: i64,
    pub content_markdown: String,
    pub content_hash: String,
}

impl FetchedDocument {
    pub fn from_markdown(input: FetchedDocumentInput) -> Self {
        let canonical = canonical_url(&input.url);
        let retrieved_at = input.retrieved_at;
        let markdown_artifact = MarkdownArtifact {
            title: input.title.clone(),
            url: input.url.clone(),
            canonical_url: canonical.clone(),
            source: input.source.clone(),
            provider: input.provider.clone(),
            site_name: input.site_name.clone(),
            author: input.author.clone(),
            published_at: input.published_at.clone(),
            retrieved_at,
            content_markdown: input.content_markdown.clone(),
            content_hash: input.content_hash.clone(),
        };
        Self {
            url: input.url,
            canonical_url: canonical,
            source: input.source,
            provider: input.provider,
            site_name: input.site_name,
            title: input.title,
            author: input.author,
            published_at: input.published_at,
            content_markdown: input.content_markdown,
            content_hash: input.content_hash,
            fetched_at: retrieved_at,
            retrieved_at,
            markdown_artifact,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchedDocumentInput {
    pub url: String,
    pub source: Option<ResearchSource>,
    pub provider: Option<String>,
    pub site_name: Option<String>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub content_markdown: String,
    pub content_hash: String,
    pub retrieved_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResearchProviderKind {
    GenericWebSearch,
    FixedSite,
    BrowserCdp,
    Other,
}

#[async_trait]
pub trait ResearchProvider: Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> ResearchProviderKind;

    async fn search(&self, req: ResearchSearchRequest) -> anyhow::Result<ResearchSearchResponse>;

    async fn fetch(&self, _req: ResearchFetchRequest) -> anyhow::Result<Option<FetchedDocument>> {
        Ok(None)
    }
}
