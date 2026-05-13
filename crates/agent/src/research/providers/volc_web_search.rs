use crate::config::resolve_env_value;
use crate::research::config::{ResearchProviderConfig, ResearchSource};
use crate::research::normalize::{canonical_url, content_hash, dedupe_results, result_id};
use crate::research::provider::{
    ResearchProvider, ResearchProviderKind, ResearchSearchRequest, ResearchSearchResponse,
    ResearchSearchResult,
};
use anyhow::{anyhow, bail};
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{Map, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_VOLC_WEB_SEARCH_URL: &str = "https://open.feedcoopapi.com/search_api/web_search";

pub struct VolcWebSearchProvider {
    id: String,
    config: ResearchProviderConfig,
    client: reqwest::Client,
}

impl VolcWebSearchProvider {
    pub fn new(id: String, config: ResearchProviderConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { id, config, client })
    }

    fn endpoint(&self) -> &str {
        self.config
            .base_url
            .as_deref()
            .or(self.config.search_url.as_deref())
            .unwrap_or(DEFAULT_VOLC_WEB_SEARCH_URL)
    }

    fn api_key(&self) -> String {
        self.config
            .api_key
            .as_deref()
            .map(resolve_env_value)
            .or_else(|| {
                self.config
                    .env_key
                    .as_deref()
                    .and_then(|key| std::env::var(key).ok())
            })
            .unwrap_or_default()
    }

    fn headers(&self) -> anyhow::Result<HeaderMap> {
        let api_key = self.api_key();
        if api_key.trim().is_empty() {
            bail!("provider '{}' missing volc web search API key", self.id);
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", api_key.trim()))?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(headers)
    }

    fn request_body(&self, req: &ResearchSearchRequest) -> Value {
        let search_type = self
            .config
            .search_type
            .clone()
            .unwrap_or_else(|| "web".to_string());
        let mut body = Map::new();
        body.insert("Query".to_string(), Value::String(req.query.clone()));
        body.insert("SearchType".to_string(), Value::String(search_type.clone()));
        body.insert(
            "Count".to_string(),
            Value::from(
                self.config
                    .count
                    .or(req.limit)
                    .unwrap_or(10)
                    .min(max_count(&search_type)),
            ),
        );

        let mut filter = Map::new();
        insert_bool(&mut filter, "NeedContent", self.config.need_content);
        insert_bool(&mut filter, "NeedUrl", self.config.need_url);
        insert_string(&mut filter, "Sites", self.config.sites.as_deref());
        insert_string(
            &mut filter,
            "BlockHosts",
            self.config.block_hosts.as_deref(),
        );
        if let Some(level) = self.config.auth_info_level {
            filter.insert("AuthInfoLevel".to_string(), Value::from(level));
        }
        if !filter.is_empty() {
            body.insert("Filter".to_string(), Value::Object(filter));
        }

        if let Some(need_summary) = self
            .config
            .need_summary
            .or_else(|| (search_type == "web_summary").then_some(true))
        {
            body.insert("NeedSummary".to_string(), Value::Bool(need_summary));
        }
        insert_string(&mut body, "TimeRange", self.config.time_range.as_deref());
        insert_string(
            &mut body,
            "ContentFormats",
            self.config.content_formats.as_deref(),
        );
        insert_string(&mut body, "Industry", self.config.industry.as_deref());
        if let Some(query_rewrite) = self.config.query_rewrite {
            body.insert(
                "QueryControl".to_string(),
                serde_json::json!({ "QueryRewrite": query_rewrite }),
            );
        }

        Value::Object(body)
    }
}

#[async_trait]
impl ResearchProvider for VolcWebSearchProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ResearchProviderKind {
        ResearchProviderKind::GenericWebSearch
    }

    async fn search(&self, req: ResearchSearchRequest) -> anyhow::Result<ResearchSearchResponse> {
        let body = self.request_body(&req);
        let value = self
            .client
            .post(self.endpoint())
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let results = value_to_volc_results(&self.id, &req.query, value)?;
        Ok(ResearchSearchResponse {
            query: req.query,
            results: dedupe_results(results),
        })
    }
}

fn value_to_volc_results(
    provider: &str,
    query: &str,
    value: Value,
) -> anyhow::Result<Vec<ResearchSearchResult>> {
    if let Some(error) = value
        .get("ResponseMetadata")
        .and_then(|metadata| metadata.get("Error"))
    {
        let code = error
            .get("Code")
            .and_then(|value| value.as_str())
            .or_else(|| {
                error
                    .get("CodeN")
                    .and_then(|value| value.as_i64())
                    .map(|_| "")
            })
            .unwrap_or("unknown");
        let message = error
            .get("Message")
            .and_then(|value| value.as_str())
            .unwrap_or("volc web search failed");
        return Err(anyhow!("volc web search error {code}: {message}"));
    }

    let result = value.get("Result").unwrap_or(&value);
    let retrieved_at = now_unix();
    let mut results = Vec::new();

    if let Some(items) = result.get("WebResults").and_then(|value| value.as_array()) {
        for item in items {
            if let Some(result) = web_item(provider, query, item, retrieved_at) {
                results.push(result);
            }
        }
    }
    if let Some(items) = result
        .get("ImageResults")
        .and_then(|value| value.as_array())
    {
        for item in items {
            if let Some(result) = image_item(provider, query, item, retrieved_at) {
                results.push(result);
            }
        }
    }

    Ok(results)
}

fn web_item(
    provider: &str,
    query: &str,
    item: &Value,
    retrieved_at: i64,
) -> Option<ResearchSearchResult> {
    let url = string_field(item, "Url")?;
    let canonical = canonical_url(&url);
    let content = string_field(item, "Content");
    Some(ResearchSearchResult {
        id: result_id(&canonical),
        source: ResearchSource::Web,
        provider: provider.to_string(),
        title: string_field(item, "Title").unwrap_or_else(|| query.to_string()),
        url: canonical.clone(),
        canonical_url: Some(canonical),
        snippet: string_field(item, "Summary").or_else(|| string_field(item, "Snippet")),
        site_name: string_field(item, "SiteName"),
        author: None,
        published_at: string_field(item, "PublishTime"),
        score: item
            .get("RankScore")
            .and_then(|value| value.as_f64())
            .map(|value| value as f32),
        content_hash: content.as_deref().map(content_hash),
        content_markdown: content,
        retrieved_at: Some(retrieved_at),
    })
}

fn image_item(
    provider: &str,
    query: &str,
    item: &Value,
    retrieved_at: i64,
) -> Option<ResearchSearchResult> {
    let image_url = item
        .get("Image")
        .and_then(|image| image.get("Url"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())?;
    let landing_url = string_field(item, "Url").unwrap_or_else(|| image_url.to_string());
    let canonical = canonical_url(&landing_url);
    Some(ResearchSearchResult {
        id: result_id(&canonical),
        source: ResearchSource::Web,
        provider: provider.to_string(),
        title: string_field(item, "Title").unwrap_or_else(|| query.to_string()),
        url: canonical.clone(),
        canonical_url: Some(canonical),
        snippet: Some(format!("image: {image_url}")),
        site_name: string_field(item, "SiteName"),
        author: None,
        published_at: string_field(item, "PublishTime"),
        score: item
            .get("RankScore")
            .and_then(|value| value.as_f64())
            .map(|value| value as f32),
        content_hash: None,
        content_markdown: None,
        retrieved_at: Some(retrieved_at),
    })
}

fn insert_bool(map: &mut Map<String, Value>, key: &str, value: Option<bool>) {
    if let Some(value) = value {
        map.insert(key.to_string(), Value::Bool(value));
    }
}

fn insert_string(map: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn max_count(search_type: &str) -> usize {
    if search_type == "image" {
        5
    } else {
        50
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volc_request_body_maps_documented_fields() {
        let provider = VolcWebSearchProvider::new(
            "volc".to_string(),
            ResearchProviderConfig {
                provider_type: crate::research::config::ResearchProviderType::VolcWebSearch,
                search_type: Some("web_summary".to_string()),
                count: Some(99),
                need_content: Some(true),
                need_url: Some(true),
                need_summary: Some(true),
                content_formats: Some("markdown".to_string()),
                time_range: Some("OneWeek".to_string()),
                query_rewrite: Some(true),
                sites: Some("mp.qq.com|volcengine.com".to_string()),
                block_hosts: Some("example.com".to_string()),
                auth_info_level: Some(1),
                industry: Some("finance".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

        let body = provider.request_body(&ResearchSearchRequest {
            query: "AI HUB".to_string(),
            sources: vec![ResearchSource::Web],
            provider_ids: Vec::new(),
            freshness: None,
            limit: Some(10),
            fetch_content: false,
            language: None,
        });

        assert_eq!(body["Query"], "AI HUB");
        assert_eq!(body["SearchType"], "web_summary");
        assert_eq!(body["Count"], 50);
        assert_eq!(body["Filter"]["NeedContent"], true);
        assert_eq!(body["Filter"]["NeedUrl"], true);
        assert_eq!(body["Filter"]["Sites"], "mp.qq.com|volcengine.com");
        assert_eq!(body["Filter"]["BlockHosts"], "example.com");
        assert_eq!(body["Filter"]["AuthInfoLevel"], 1);
        assert_eq!(body["NeedSummary"], true);
        assert_eq!(body["ContentFormats"], "markdown");
        assert_eq!(body["TimeRange"], "OneWeek");
        assert_eq!(body["QueryControl"]["QueryRewrite"], true);
        assert_eq!(body["Industry"], "finance");
    }

    #[test]
    fn volc_results_parse_web_and_content() {
        let value = serde_json::json!({
            "Result": {
                "WebResults": [{
                    "Title": "AI HUB report",
                    "SiteName": "Volc",
                    "Url": "https://example.com/a#frag",
                    "Snippet": "short",
                    "Summary": "summary",
                    "Content": "# Full markdown",
                    "PublishTime": "2026-05-14T12:00:00+08:00",
                    "RankScore": 0.9
                }]
            }
        });

        let results = value_to_volc_results("volc", "AI HUB", value).unwrap();
        assert_eq!(results.len(), 1);
        let item = &results[0];
        assert_eq!(item.title, "AI HUB report");
        assert_eq!(item.url, "https://example.com/a");
        assert_eq!(item.snippet.as_deref(), Some("summary"));
        assert_eq!(item.content_markdown.as_deref(), Some("# Full markdown"));
        assert!(item.content_hash.is_some());
        assert_eq!(item.site_name.as_deref(), Some("Volc"));
    }
}
