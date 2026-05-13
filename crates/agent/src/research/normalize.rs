use super::config::ResearchSource;
use super::provider::ResearchSearchResult;
use sha2::{Digest, Sha256};
use url::Url;

pub fn sha256_id(prefix: &str, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{prefix}:{:x}", hasher.finalize())
}

pub fn canonical_url(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw) else {
        return raw.trim().to_string();
    };
    url.set_fragment(None);
    if (url.scheme() == "http" && url.port() == Some(80))
        || (url.scheme() == "https" && url.port() == Some(443))
    {
        let _ = url.set_port(None);
    }
    url.to_string()
}

pub fn content_hash(content: &str) -> String {
    sha256_id("sha256", content.trim())
}

pub fn result_id(url: &str) -> String {
    sha256_id("sha256", &canonical_url(url))
}

pub fn dedupe_results(results: Vec<ResearchSearchResult>) -> Vec<ResearchSearchResult> {
    let mut seen_urls = std::collections::HashSet::new();
    let mut seen_weak_keys = std::collections::HashSet::new();
    let mut deduped = Vec::new();
    for mut result in results {
        let canonical = canonical_url(&result.url);
        let weak = format!(
            "{}|{}|{}",
            result.title.trim().to_lowercase(),
            result.site_name.clone().unwrap_or_default().to_lowercase(),
            result.published_at.clone().unwrap_or_default()
        );
        if !seen_urls.insert(canonical.clone()) {
            continue;
        }
        if !weak.trim_matches('|').is_empty() && !seen_weak_keys.insert(weak) {
            continue;
        }
        result.id = result_id(&canonical);
        result.url = canonical;
        deduped.push(result);
    }
    deduped
}

pub fn value_to_results(
    provider: &str,
    source: ResearchSource,
    query: &str,
    value: serde_json::Value,
) -> Vec<ResearchSearchResult> {
    let candidates = value
        .get("results")
        .or_else(|| value.get("data"))
        .or_else(|| value.get("items"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_else(|| {
            value
                .as_array()
                .cloned()
                .unwrap_or_else(|| flatten_openai_output(&value))
        });

    candidates
        .into_iter()
        .filter_map(|item| {
            let title =
                string_field(&item, &["title", "name"]).unwrap_or_else(|| query.to_string());
            let url = string_field(&item, &["url", "link", "href"])?;
            Some(ResearchSearchResult {
                id: result_id(&url),
                source: source.clone(),
                provider: provider.to_string(),
                title,
                url: canonical_url(&url),
                canonical_url: Some(canonical_url(&url)),
                snippet: string_field(&item, &["snippet", "summary", "description", "text"]),
                site_name: string_field(&item, &["site_name", "site", "source_name"]),
                author: string_field(&item, &["author"]),
                published_at: string_field(&item, &["published_at", "published", "date"]),
                score: item.get("score").and_then(|v| v.as_f64()).map(|v| v as f32),
                content_hash: None,
                content_markdown: string_field(&item, &["content_markdown", "markdown", "content"]),
                retrieved_at: None,
            })
        })
        .collect()
}

fn flatten_openai_output(value: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut items = Vec::new();
    if let Some(output) = value.get("output").and_then(|v| v.as_array()) {
        for entry in output {
            if let Some(content) = entry.get("content").and_then(|v| v.as_array()) {
                for part in content {
                    if part.get("type").and_then(|v| v.as_str()) == Some("web_search_result") {
                        items.push(part.clone());
                    }
                }
            }
        }
    }
    items
}

fn string_field(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(|v| v.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(
        url: &str,
        title: &str,
        site_name: Option<&str>,
        published_at: Option<&str>,
    ) -> ResearchSearchResult {
        ResearchSearchResult {
            id: result_id(url),
            source: ResearchSource::Web,
            provider: "test".to_string(),
            title: title.to_string(),
            url: url.to_string(),
            canonical_url: Some(canonical_url(url)),
            snippet: None,
            site_name: site_name.map(str::to_string),
            author: None,
            published_at: published_at.map(str::to_string),
            score: None,
            content_hash: None,
            content_markdown: None,
            retrieved_at: None,
        }
    }

    #[test]
    fn dedupe_results_removes_canonical_and_weak_duplicates() {
        let results = dedupe_results(vec![
            result(
                "https://example.com/a#section",
                "Same",
                Some("Example"),
                Some("2026-05-13"),
            ),
            result(
                "https://example.com/a",
                "Different",
                Some("Example"),
                Some("2026-05-13"),
            ),
            result(
                "https://mirror.example.com/a",
                "same",
                Some("example"),
                Some("2026-05-13"),
            ),
            result(
                "https://example.com/b",
                "Other",
                Some("Example"),
                Some("2026-05-13"),
            ),
        ]);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://example.com/a");
        assert_eq!(results[1].url, "https://example.com/b");
    }
}
