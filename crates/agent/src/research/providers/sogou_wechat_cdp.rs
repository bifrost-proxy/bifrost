use crate::research::config::{ResearchProviderConfig, ResearchSource};
use crate::research::normalize::{canonical_url, content_hash, dedupe_results, result_id};
use crate::research::provider::{
    FetchedDocument, FetchedDocumentInput, ResearchFetchRequest, ResearchProvider,
    ResearchProviderKind, ResearchSearchRequest, ResearchSearchResponse, ResearchSearchResult,
};
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

pub struct SogouWechatCdpProvider {
    id: String,
    config: ResearchProviderConfig,
    client: reqwest::Client,
}

impl SogouWechatCdpProvider {
    pub fn new(id: String, config: ResearchProviderConfig) -> anyhow::Result<Self> {
        validate_cdp_endpoint(&config.cdp_endpoint_or_default())?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { id, config, client })
    }

    async fn open_page(&self, url: &str) -> anyhow::Result<CdpPage> {
        self.ensure_cdp_browser().await?;
        CdpPage::open(
            self.client.clone(),
            self.config.cdp_endpoint_or_default(),
            url,
        )
        .await
    }

    async fn ensure_cdp_browser(&self) -> anyhow::Result<()> {
        let endpoint = self.config.cdp_endpoint_or_default();
        if cdp_endpoint_reachable(&self.client, &endpoint).await {
            return Ok(());
        }
        let edge = find_edge_browser().ok_or_else(|| {
            anyhow!(
                "Sogou WeChat search needs Microsoft Edge CDP, but Edge was not found. Set BIFROST_RESEARCH_EDGE_BIN or configure a reachable CDP endpoint."
            )
        })?;
        let user_data_dir = expand_user_path(&self.config.browser_user_data_dir_or_default());
        std::fs::create_dir_all(&user_data_dir).with_context(|| {
            format!(
                "create Sogou WeChat browser data dir {}",
                user_data_dir.display()
            )
        })?;
        let url =
            Url::parse(&endpoint).with_context(|| format!("parse CDP endpoint {endpoint}"))?;
        let host = url.host_str().unwrap_or("127.0.0.1");
        let port = url
            .port()
            .ok_or_else(|| anyhow!("CDP endpoint must include a port: {endpoint}"))?;
        tracing::info!(
            endpoint = %endpoint,
            user_data_dir = %user_data_dir.display(),
            "starting Microsoft Edge CDP for Sogou WeChat research"
        );
        Command::new(edge)
            .arg("--headless")
            .arg(format!("--remote-debugging-address={host}"))
            .arg(format!("--remote-debugging-port={port}"))
            .arg(format!("--user-data-dir={}", user_data_dir.display()))
            .arg("--disable-gpu")
            .arg("--disable-dev-shm-usage")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("about:blank")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| "start Microsoft Edge for Sogou WeChat CDP")?;
        for _ in 0..80 {
            if cdp_endpoint_reachable(&self.client, &endpoint).await {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(anyhow!(
            "Microsoft Edge CDP did not become reachable at {endpoint}"
        ))
    }
}

#[async_trait]
impl ResearchProvider for SogouWechatCdpProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> ResearchProviderKind {
        ResearchProviderKind::BrowserCdp
    }

    async fn search(&self, req: ResearchSearchRequest) -> anyhow::Result<ResearchSearchResponse> {
        let limit = req.limit.unwrap_or(10);
        let search_url = sogou_wechat_search_url(&req.query)?;
        let mut page = self.open_page(search_url.as_str()).await?;
        let mut value = Value::Array(Vec::new());
        for _ in 0..50 {
            value = page
                .evaluate_json(sogou_results_script(limit), Duration::from_secs(5))
                .await?;
            if value.as_array().is_some_and(|items| !items.is_empty()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let _ = page.close().await;
        let results = value
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| search_result_from_value(&self.id, item))
            .collect::<Vec<_>>();
        Ok(ResearchSearchResponse {
            query: req.query,
            results: dedupe_results(results),
        })
    }

    async fn fetch(&self, req: ResearchFetchRequest) -> anyhow::Result<Option<FetchedDocument>> {
        let mut page = self.open_page(&req.url).await?;
        let mut value = Value::Null;
        for _ in 0..60 {
            value = page
                .evaluate_json(fetch_wechat_article_script(), Duration::from_secs(5))
                .await?;
            if value
                .get("blocked")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
                || value
                    .get("content_markdown")
                    .and_then(|value| value.as_str())
                    .is_some_and(|content| content.trim().len() > 120)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let _ = page.close().await;
        if value
            .get("blocked")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            return Err(anyhow!(
                "browser crawler was blocked by site challenge while fetching {}",
                req.url
            ));
        }
        let content = value
            .get("content_markdown")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if content.is_empty() {
            return Ok(None);
        }
        let final_url = value
            .get("url")
            .and_then(|value| value.as_str())
            .unwrap_or(&req.url)
            .to_string();
        Ok(Some(FetchedDocument::from_markdown(FetchedDocumentInput {
            url: final_url,
            source: Some(ResearchSource::Wechat),
            provider: Some(self.id.clone()),
            site_name: Some("微信公众号".to_string()),
            title: string_field(&value, "title"),
            author: string_field(&value, "author"),
            published_at: string_field(&value, "published_at"),
            content_hash: content_hash(&content),
            content_markdown: content,
            retrieved_at: crate::research::store::now_unix(),
        })))
    }
}

pub fn sogou_wechat_search_url(query: &str) -> anyhow::Result<Url> {
    let mut search_url = Url::parse("https://weixin.sogou.com/weixin")?;
    search_url
        .query_pairs_mut()
        .append_pair("type", "2")
        .append_pair("p", "44351200")
        .append_pair("ie", "utf8")
        .append_pair("query", query);
    Ok(search_url)
}

fn search_result_from_value(provider: &str, value: Value) -> Option<ResearchSearchResult> {
    let title = string_field(&value, "title")?;
    let url = string_field(&value, "url")?;
    Some(ResearchSearchResult {
        id: result_id(&url),
        source: ResearchSource::Wechat,
        provider: provider.to_string(),
        title,
        url: canonical_url(&url),
        canonical_url: Some(canonical_url(&url)),
        snippet: string_field(&value, "snippet"),
        site_name: string_field(&value, "site_name"),
        author: None,
        published_at: string_field(&value, "published_at"),
        score: value
            .get("score")
            .and_then(|value| value.as_f64())
            .map(|value| value as f32),
        content_hash: None,
        content_markdown: None,
        retrieved_at: Some(crate::research::store::now_unix()),
    })
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn validate_cdp_endpoint(endpoint: &str) -> anyhow::Result<()> {
    let url = Url::parse(endpoint).map_err(|error| anyhow!("invalid CDP endpoint: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(anyhow!("CDP endpoint must use http or https"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("CDP endpoint has no host"))?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if !matches!(host.as_str(), "127.0.0.1" | "::1" | "localhost") {
        return Err(anyhow!(
            "CDP endpoint must point to a local browser on localhost"
        ));
    }
    Ok(())
}

async fn cdp_endpoint_reachable(client: &reqwest::Client, endpoint: &str) -> bool {
    let url = format!("{}/json/version", endpoint.trim_end_matches('/'));
    client
        .get(url)
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

fn find_edge_browser() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("BIFROST_RESEARCH_EDGE_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let mut candidates = Vec::new();
    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from(
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ));
        candidates.push(
            crate::config::user_home_dir()
                .join("Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
        );
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(program_files) = std::env::var("ProgramFiles") {
            candidates
                .push(PathBuf::from(program_files).join("Microsoft/Edge/Application/msedge.exe"));
        }
        if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
            candidates.push(
                PathBuf::from(program_files_x86).join("Microsoft/Edge/Application/msedge.exe"),
            );
        }
    }
    #[cfg(target_os = "linux")]
    {
        candidates.push(PathBuf::from("/usr/bin/microsoft-edge"));
        candidates.push(PathBuf::from("/usr/bin/microsoft-edge-stable"));
        candidates.push(PathBuf::from("/usr/bin/msedge"));
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn expand_user_path(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        return crate::config::user_home_dir().join(rest);
    }
    PathBuf::from(value)
}

struct CdpPage {
    client: reqwest::Client,
    endpoint: String,
    target_id: String,
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next_id: u64,
}

impl CdpPage {
    async fn open(client: reqwest::Client, endpoint: String, url: &str) -> anyhow::Result<Self> {
        let endpoint = endpoint.trim_end_matches('/').to_string();
        let create_url = format!("{endpoint}/json/new?about%3Ablank");
        let mut response = client.put(&create_url).send().await?;
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            response = client.get(&create_url).send().await?;
        }
        let value = response
            .error_for_status()
            .with_context(|| format!("create CDP target at {endpoint}"))?
            .json::<Value>()
            .await?;
        let target_id = value
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("CDP target response missing id"))?
            .to_string();
        let ws_url = value
            .get("webSocketDebuggerUrl")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("CDP target response missing webSocketDebuggerUrl"))?;
        let (socket, _) = tokio_tungstenite::connect_async(ws_url).await?;
        let mut page = Self {
            client,
            endpoint,
            target_id,
            socket,
            next_id: 1,
        };
        let _ = page.call("Page.enable", serde_json::json!({})).await?;
        let _ = page.call("Network.enable", serde_json::json!({})).await?;
        let _ = page
            .call(
                "Network.setUserAgentOverride",
                serde_json::json!({
                    "userAgent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36",
                    "acceptLanguage": "zh-CN,zh;q=0.9,en;q=0.8",
                    "platform": "macOS",
                }),
            )
            .await?;
        let _ = page.call("Runtime.enable", serde_json::json!({})).await?;
        page.navigate(url).await?;
        Ok(page)
    }

    async fn navigate(&mut self, url: &str) -> anyhow::Result<()> {
        let _ = self
            .call("Page.navigate", serde_json::json!({ "url": url }))
            .await?;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        Ok(())
    }

    async fn evaluate_json(
        &mut self,
        expression: String,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let params = serde_json::json!({
            "expression": expression,
            "awaitPromise": true,
            "returnByValue": true,
        });
        let result = tokio::time::timeout(timeout, async {
            let mut last_error = None;
            for _ in 0..4 {
                match self.call("Runtime.evaluate", params.clone()).await {
                    Ok(result) => return Ok(result),
                    Err(error) => {
                        let text = error.to_string();
                        if text.contains("Execution context was destroyed")
                            || text.contains("Cannot find context")
                        {
                            last_error = Some(error);
                            tokio::time::sleep(Duration::from_millis(700)).await;
                            continue;
                        }
                        return Err(error);
                    }
                }
            }
            Err(last_error.unwrap_or_else(|| anyhow!("CDP evaluation failed")))
        })
        .await
        .map_err(|_| anyhow!("CDP evaluation timed out"))??;
        if let Some(exception) = result.get("exceptionDetails") {
            return Err(anyhow!("CDP evaluation failed: {exception}"));
        }
        Ok(result
            .get("result")
            .and_then(|result| result.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    async fn call(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
        });
        self.socket
            .send(Message::Text(request.to_string().into()))
            .await?;
        while let Some(message) = self.socket.next().await {
            let message = message?;
            if !message.is_text() {
                continue;
            }
            let value: Value = serde_json::from_str(message.to_text()?)?;
            if value.get("id").and_then(|value| value.as_u64()) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(anyhow!("CDP {method} failed: {error}"));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(anyhow!("CDP socket closed while waiting for {method}"))
    }

    async fn close(self) -> anyhow::Result<()> {
        let close_url = format!("{}/json/close/{}", self.endpoint, self.target_id);
        let _ = self.client.get(close_url).send().await;
        Ok(())
    }
}

#[cfg(test)]
fn encode_query_value(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn sogou_results_script(limit: usize) -> String {
    format!(
        r#"(() => {{
  const clean = (s) => (s || '').replace(/\s+/g, ' ').trim();
  return Array.from(document.querySelectorAll('li[id^="sogou_vr_11002601_box_"]'))
    .slice(0, {limit})
    .map((li, index) => {{
      const link = li.querySelector('h3 a');
      const href = link ? link.getAttribute('href') : '';
      const published = clean(li.querySelector('.s2')?.innerText || li.querySelector('.s-p span:last-child')?.innerText || '');
      return {{
        title: clean(link?.innerText),
        url: href ? new URL(href, location.href).href : '',
        snippet: clean(li.querySelector('.txt-info')?.innerText),
        site_name: clean(li.querySelector('.s-p .all-time-y2')?.innerText || li.querySelector('.s-p span')?.innerText),
        published_at: published,
        score: Math.max(0, 1 - index * 0.01),
      }};
    }})
    .filter((item) => item.title && item.url);
}})()"#
    )
}

fn fetch_wechat_article_script() -> String {
    r#"(() => {
  const clean = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const isBlocked = () => location.href.includes('antispider') ||
    document.body.innerText.includes('验证码') ||
    document.body.innerText.includes('请输入验证码');
  if (isBlocked()) {
    return { blocked: true, url: location.href, title: document.title, content_markdown: '' };
  }
  const content = document.querySelector('#js_content') || document.querySelector('article') || document.body;
  const text = clean(content?.innerText);
  return {
    blocked: false,
    url: location.href,
    title: clean(document.querySelector('#activity-name')?.innerText || document.querySelector('h1')?.innerText || document.title),
    author: clean(document.querySelector('#js_name')?.innerText || document.querySelector('[id*="author"]')?.innerText || ''),
    published_at: clean(document.querySelector('#publish_time')?.innerText || document.querySelector('[id*="publish"]')?.innerText || ''),
    content_markdown: text,
  };
})()"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sogou_wechat_search_url_matches_required_entrypoint() {
        let url = sogou_wechat_search_url("AI HUB").expect("build search url");

        assert_eq!(
            url.as_str(),
            "https://weixin.sogou.com/weixin?type=2&p=44351200&ie=utf8&query=AI+HUB"
        );
    }

    #[test]
    fn query_value_encoding_keeps_sogou_link_safe() {
        assert_eq!(
            encode_query_value("https://weixin.sogou.com/link?query=AI Agent"),
            "https%3A%2F%2Fweixin.sogou.com%2Flink%3Fquery%3DAI+Agent"
        );
    }

    #[test]
    fn cdp_endpoint_must_be_local() {
        assert!(validate_cdp_endpoint("http://127.0.0.1:9222").is_ok());
        assert!(validate_cdp_endpoint("http://localhost:9222").is_ok());
        assert!(validate_cdp_endpoint("https://example.com:9222").is_err());
    }
}
