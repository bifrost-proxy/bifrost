use crate::config::resolve_env_value;
use crate::research::config::{ResearchProviderConfig, ResearchProviderType, ResearchSource};
use crate::research::normalize::{dedupe_results, value_to_results};
use crate::research::provider::{
    ResearchProvider, ResearchProviderKind, ResearchSearchRequest, ResearchSearchResponse,
};
use anyhow::anyhow;
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION};
use std::time::Duration;

pub struct GenericHttpProvider {
    id: String,
    config: ResearchProviderConfig,
    client: reqwest::Client,
}

impl GenericHttpProvider {
    pub fn new(id: String, config: ResearchProviderConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { id, config, client })
    }

    fn headers(&self) -> anyhow::Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        for (name, value) in &self.config.headers {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes())?,
                HeaderValue::from_str(value)?,
            );
        }
        for (name, env_var) in &self.config.env_headers {
            if let Ok(value) = std::env::var(env_var) {
                headers.insert(
                    HeaderName::from_bytes(name.as_bytes())?,
                    HeaderValue::from_str(&value)?,
                );
            }
        }
        let api_key = self
            .config
            .api_key
            .as_deref()
            .map(resolve_env_value)
            .or_else(|| {
                self.config
                    .env_key
                    .as_deref()
                    .and_then(|key| std::env::var(key).ok())
            })
            .unwrap_or_default();
        if !api_key.is_empty() {
            if self.config.provider_type == ResearchProviderType::Exa {
                let name = HeaderName::from_static("x-api-key");
                if !headers.contains_key(&name) {
                    headers.insert(name, HeaderValue::from_str(&api_key)?);
                }
            } else if !headers.contains_key(AUTHORIZATION) {
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {api_key}"))?,
                );
            }
        }
        Ok(headers)
    }

    fn search_url(&self) -> anyhow::Result<&str> {
        self.config
            .base_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                self.config
                    .search_url
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })
            .or(match self.config.provider_type {
                ResearchProviderType::Tavily => Some("https://api.tavily.com/search"),
                ResearchProviderType::Exa => Some("https://api.exa.ai/search"),
                _ => None,
            })
            .ok_or_else(|| anyhow!("provider '{}' missing base_url/search_url", self.id))
    }

    fn request_body(&self, req: &ResearchSearchRequest) -> serde_json::Value {
        match self.config.provider_type {
            ResearchProviderType::Tavily => serde_json::json!({
                "query": req.query,
                "max_results": req.limit.unwrap_or(10),
                "include_raw_content": req.fetch_content,
            }),
            ResearchProviderType::Exa => serde_json::json!({
                "query": req.query,
                "numResults": req.limit.unwrap_or(10),
                "contents": if req.fetch_content {
                    serde_json::json!({ "text": true })
                } else {
                    serde_json::Value::Null
                },
            }),
            _ => serde_json::json!({
                "query": req.query,
                "limit": req.limit.unwrap_or(10),
                "freshness": req.freshness,
                "language": req.language,
                "template": self.config.request_template,
            }),
        }
    }
}

#[async_trait]
impl ResearchProvider for GenericHttpProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ResearchProviderKind {
        ResearchProviderKind::GenericWebSearch
    }

    async fn search(&self, req: ResearchSearchRequest) -> anyhow::Result<ResearchSearchResponse> {
        let url = self.search_url()?;
        let body = self.request_body(&req);
        let value = self
            .client
            .post(url)
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;
        let results = value_to_results(&self.id, ResearchSource::Web, &req.query, value);
        Ok(ResearchSearchResponse {
            query: req.query,
            results: dedupe_results(results),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tavily_and_exa_have_builtin_endpoints() {
        let tavily = GenericHttpProvider::new(
            "tavily".to_string(),
            ResearchProviderConfig {
                provider_type: ResearchProviderType::Tavily,
                env_key: Some("TAVILY_API_KEY".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        let exa = GenericHttpProvider::new(
            "exa".to_string(),
            ResearchProviderConfig {
                provider_type: ResearchProviderType::Exa,
                env_key: Some("EXA_API_KEY".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            tavily.search_url().unwrap(),
            "https://api.tavily.com/search"
        );
        assert_eq!(exa.search_url().unwrap(), "https://api.exa.ai/search");
    }
}
