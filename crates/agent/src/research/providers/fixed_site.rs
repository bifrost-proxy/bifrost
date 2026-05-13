use crate::research::config::{ResearchProviderConfig, ResearchSiteKind, ResearchSource};
use crate::research::normalize::{canonical_url, dedupe_results, result_id};
use crate::research::provider::{
    ResearchProvider, ResearchProviderKind, ResearchSearchRequest, ResearchSearchResponse,
    ResearchSearchResult,
};
use anyhow::anyhow;
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, USER_AGENT};
use serde_json::Value;
use std::time::Duration;

pub struct FixedSiteProvider {
    id: String,
    site: ResearchSiteKind,
    client: reqwest::Client,
}

impl FixedSiteProvider {
    pub fn new(id: String, config: ResearchProviderConfig) -> anyhow::Result<Self> {
        let site = config
            .site
            .ok_or_else(|| anyhow!("fixed_site provider '{}' missing site", id))?;
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "BifrostResearch/0.1 (+https://github.com/bifrost-proxy/bifrost)",
            ),
        );
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json,text/xml,*/*"),
        );
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self { id, site, client })
    }

    async fn search_arxiv(
        &self,
        req: &ResearchSearchRequest,
    ) -> anyhow::Result<Vec<ResearchSearchResult>> {
        let limit = req.limit.unwrap_or(10).min(25);
        let url = format!(
            "https://export.arxiv.org/api/query?search_query=all:{}&start=0&max_results={limit}&sortBy=submittedDate&sortOrder=descending",
            encode_query(&req.query)
        );
        let xml = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(parse_arxiv_entries(&self.id, &xml))
    }

    async fn search_hacker_news(
        &self,
        req: &ResearchSearchRequest,
    ) -> anyhow::Result<Vec<ResearchSearchResult>> {
        let limit = req.limit.unwrap_or(10).min(50);
        let url = format!(
            "https://hn.algolia.com/api/v1/search_by_date?query={}&tags=story&hitsPerPage={limit}",
            encode_query(&req.query)
        );
        let value = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let hits = value
            .get("hits")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(hits
            .into_iter()
            .filter_map(|hit| hn_result(&self.id, hit))
            .collect())
    }

    async fn search_github_repositories(
        &self,
        req: &ResearchSearchRequest,
    ) -> anyhow::Result<Vec<ResearchSearchResult>> {
        let limit = req.limit.unwrap_or(10).min(30);
        let url = format!(
            "https://api.github.com/search/repositories?q={}&sort=updated&order=desc&per_page={limit}",
            encode_query(&req.query)
        );
        let value = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let items = value
            .get("items")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(items
            .into_iter()
            .filter_map(|item| github_repo_result(&self.id, item))
            .collect())
    }
}

#[async_trait]
impl ResearchProvider for FixedSiteProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ResearchProviderKind {
        ResearchProviderKind::FixedSite
    }

    async fn search(&self, req: ResearchSearchRequest) -> anyhow::Result<ResearchSearchResponse> {
        let results = match self.site {
            ResearchSiteKind::Arxiv => self.search_arxiv(&req).await?,
            ResearchSiteKind::HackerNews => self.search_hacker_news(&req).await?,
            ResearchSiteKind::GithubRepositories => self.search_github_repositories(&req).await?,
        };
        Ok(ResearchSearchResponse {
            query: req.query,
            results: dedupe_results(results),
        })
    }
}

fn hn_result(provider: &str, hit: Value) -> Option<ResearchSearchResult> {
    let title = string_field(&hit, &["title", "story_title"])?;
    let url = string_field(&hit, &["url", "story_url"]).unwrap_or_else(|| {
        let id = string_field(&hit, &["objectID"]).unwrap_or_default();
        format!("https://news.ycombinator.com/item?id={id}")
    });
    Some(result(FixedSiteResultInput {
        provider,
        site_name: "Hacker News",
        title,
        url,
        snippet: string_field(&hit, &["story_text", "comment_text"]),
        author: string_field(&hit, &["author"]),
        published_at: string_field(&hit, &["created_at"]),
        score: hit
            .get("points")
            .and_then(|value| value.as_f64())
            .map(|value| value as f32),
    }))
}

fn github_repo_result(provider: &str, item: Value) -> Option<ResearchSearchResult> {
    let title = string_field(&item, &["full_name", "name"])?;
    let url = string_field(&item, &["html_url"])?;
    Some(result(FixedSiteResultInput {
        provider,
        site_name: "GitHub Repositories",
        title,
        url,
        snippet: string_field(&item, &["description"]),
        author: string_field(
            &item.get("owner").cloned().unwrap_or(Value::Null),
            &["login"],
        ),
        published_at: string_field(&item, &["updated_at", "pushed_at", "created_at"]),
        score: item
            .get("stargazers_count")
            .and_then(|value| value.as_f64())
            .map(|value| value as f32),
    }))
}

fn parse_arxiv_entries(provider: &str, xml: &str) -> Vec<ResearchSearchResult> {
    let mut results = Vec::new();
    for raw_entry in xml.split("<entry>").skip(1) {
        let entry = raw_entry.split("</entry>").next().unwrap_or_default();
        let Some(title) = xml_tag(entry, "title") else {
            continue;
        };
        let url = xml_tag(entry, "id")
            .map(|url| normalize_arxiv_url(&url))
            .unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        let authors = xml_authors(entry);
        results.push(result(FixedSiteResultInput {
            provider,
            site_name: "arXiv",
            title,
            url,
            snippet: xml_tag(entry, "summary"),
            author: (!authors.is_empty()).then(|| authors.join(", ")),
            published_at: xml_tag(entry, "published").or_else(|| xml_tag(entry, "updated")),
            score: None,
        }));
    }
    results
}

struct FixedSiteResultInput<'a> {
    provider: &'a str,
    site_name: &'a str,
    title: String,
    url: String,
    snippet: Option<String>,
    author: Option<String>,
    published_at: Option<String>,
    score: Option<f32>,
}

fn result(input: FixedSiteResultInput<'_>) -> ResearchSearchResult {
    let canonical = canonical_url(&input.url);
    ResearchSearchResult {
        id: result_id(&canonical),
        source: ResearchSource::Web,
        provider: input.provider.to_string(),
        title: clean_text(&input.title),
        url: canonical.clone(),
        canonical_url: Some(canonical),
        snippet: input.snippet.map(|value| clean_text(&value)),
        site_name: Some(input.site_name.to_string()),
        author: input.author,
        published_at: input.published_at,
        score: input.score,
        content_hash: None,
        content_markdown: None,
        retrieved_at: Some(crate::research::store::now_unix()),
    }
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

fn xml_tag(entry: &str, tag: &str) -> Option<String> {
    let start = entry.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = entry[start..].find(&format!("</{tag}>"))? + start;
    Some(clean_text(&decode_basic_entities(&entry[start..end])))
}

fn xml_authors(entry: &str) -> Vec<String> {
    entry
        .split("<author>")
        .skip(1)
        .filter_map(|author| author.split("</author>").next())
        .filter_map(|author| xml_tag(author, "name"))
        .collect()
}

fn clean_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn decode_basic_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn normalize_arxiv_url(url: &str) -> String {
    url.strip_prefix("http://arxiv.org/")
        .map(|path| format!("https://arxiv.org/{path}"))
        .unwrap_or_else(|| url.to_string())
}

fn encode_query(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arxiv_parser_extracts_marked_fields() {
        let xml = r#"
        <feed><entry><id>http://arxiv.org/abs/2601.00001</id><title>Agent Paper</title>
        <summary>Useful MCP agent result</summary><published>2026-01-01T00:00:00Z</published>
        <author><name>Alice</name></author></entry></feed>
        "#;
        let results = parse_arxiv_entries("arxiv", xml);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].site_name.as_deref(), Some("arXiv"));
        assert_eq!(results[0].author.as_deref(), Some("Alice"));
        assert_eq!(results[0].url, "https://arxiv.org/abs/2601.00001");
    }
}
