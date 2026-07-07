use std::collections::HashMap;
use std::time::Duration;

use bifrost_core::{text::truncate_chars_with_suffix, BifrostError, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub(crate) const GITHUB_GIST_SNAPSHOT_FILE: &str = "bifrost-sync-snapshot.json";
const GITHUB_API_BASE_URL: &str = "https://api.github.com";
const GITHUB_GIST_DESCRIPTION: &str = "Bifrost Sync Snapshot";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GitHubGistSnapshot {
    pub version: u32,
    pub updated_at: String,
    pub user_id: String,
    #[serde(default)]
    pub rules: Vec<GitHubGistRule>,
    #[serde(default)]
    pub basic_configs: HashMap<String, GitHubGistBasicConfig>,
}

impl Default for GitHubGistSnapshot {
    fn default() -> Self {
        Self {
            version: 1,
            updated_at: String::new(),
            user_id: String::new(),
            rules: Vec::new(),
            basic_configs: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct GitHubGistRule {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub rule: String,
    pub create_time: String,
    pub update_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct GitHubGistBasicConfig {
    pub id: String,
    pub user_id: String,
    pub config_key: String,
    pub value_json: String,
    pub hash: String,
    pub create_time: String,
    pub update_time: String,
}

#[derive(Debug, Clone)]
pub(crate) struct GitHubGistRemoteSnapshot {
    pub gist_id: Option<String>,
    pub snapshot: GitHubGistSnapshot,
}

#[derive(Clone)]
pub(crate) struct GitHubGistClient {
    http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct GitHubGistSummary {
    id: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    files: HashMap<String, GitHubGistFile>,
}

#[derive(Debug, Deserialize)]
struct GitHubGistFile {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Serialize)]
struct GitHubGistWriteRequest<'a> {
    description: &'a str,
    public: bool,
    files: HashMap<&'static str, GitHubGistWriteFile<'a>>,
}

#[derive(Debug, Serialize)]
struct GitHubGistWriteFile<'a> {
    content: &'a str,
}

impl GitHubGistClient {
    pub(crate) fn new() -> Result<Self> {
        let http = bifrost_core::outbound_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|e| BifrostError::Network(format!("failed to build GitHub client: {e}")))?;
        Ok(Self { http })
    }

    pub(crate) async fn load_snapshot(&self, token: &str) -> Result<GitHubGistRemoteSnapshot> {
        let gist_id = self.find_snapshot_gist(token).await?;
        let Some(gist_id) = gist_id else {
            return Ok(GitHubGistRemoteSnapshot {
                gist_id: None,
                snapshot: GitHubGistSnapshot::default(),
            });
        };
        let gist = self.get_gist(token, &gist_id).await?;
        let content = gist
            .files
            .get(GITHUB_GIST_SNAPSHOT_FILE)
            .and_then(|file| file.content.clone())
            .unwrap_or_default();
        if content.trim().is_empty() {
            return Ok(GitHubGistRemoteSnapshot {
                gist_id: Some(gist_id),
                snapshot: GitHubGistSnapshot::default(),
            });
        }
        let snapshot = serde_json::from_str::<GitHubGistSnapshot>(&content)
            .map_err(|e| BifrostError::Config(format!("invalid GitHub Gist sync snapshot: {e}")))?;
        Ok(GitHubGistRemoteSnapshot {
            gist_id: Some(gist_id),
            snapshot,
        })
    }

    pub(crate) async fn save_snapshot(
        &self,
        token: &str,
        remote: &GitHubGistRemoteSnapshot,
    ) -> Result<String> {
        let content = serde_json::to_string_pretty(&remote.snapshot)
            .map_err(|e| BifrostError::Config(format!("encode GitHub Gist snapshot: {e}")))?;
        let body = write_request(&content);
        if let Some(gist_id) = &remote.gist_id {
            let url = format!("{GITHUB_API_BASE_URL}/gists/{gist_id}");
            let gist = self
                .request_json::<_, GitHubGistSummary>(
                    reqwest::Method::PATCH,
                    &url,
                    token,
                    Some(&body),
                )
                .await?;
            return Ok(gist.id);
        }
        let url = format!("{GITHUB_API_BASE_URL}/gists");
        let gist = self
            .request_json::<_, GitHubGistSummary>(reqwest::Method::POST, &url, token, Some(&body))
            .await?;
        Ok(gist.id)
    }

    async fn find_snapshot_gist(&self, token: &str) -> Result<Option<String>> {
        for page in 1..=5 {
            let url = format!("{GITHUB_API_BASE_URL}/gists?per_page=100&page={page}");
            let gists = self
                .request_json::<(), Vec<GitHubGistSummary>>(reqwest::Method::GET, &url, token, None)
                .await?;
            if gists.is_empty() {
                return Ok(None);
            }
            if let Some(gist) = gists.into_iter().find(|gist| {
                gist.files.contains_key(GITHUB_GIST_SNAPSHOT_FILE)
                    || gist.description.as_deref() == Some(GITHUB_GIST_DESCRIPTION)
            }) {
                return Ok(Some(gist.id));
            }
        }
        Ok(None)
    }

    async fn get_gist(&self, token: &str, gist_id: &str) -> Result<GitHubGistSummary> {
        let url = format!("{GITHUB_API_BASE_URL}/gists/{gist_id}");
        self.request_json::<(), GitHubGistSummary>(reqwest::Method::GET, &url, token, None)
            .await
    }

    async fn request_json<B, T>(
        &self,
        method: reqwest::Method,
        url: &str,
        token: &str,
        body: Option<&B>,
    ) -> Result<T>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let method_str = method.to_string();
        let mut request = self
            .http
            .request(method, url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "bifrost-sync")
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("GitHub Gist request failed: {e}")))?;
        let status = response.status();
        let response_text = response
            .text()
            .await
            .map_err(|e| BifrostError::Network(format!("GitHub Gist response read failed: {e}")))?;
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(BifrostError::Config(
                "GitHub token is invalid or missing the gist scope".to_string(),
            ));
        }
        if !status.is_success() {
            let preview = truncate_chars_with_suffix(
                &response_text,
                500,
                &format!("...(truncated, total {} bytes)", response_text.len()),
            );
            return Err(BifrostError::Network(format!(
                "GitHub Gist request failed with status {status} (method={method_str} url={url}): {preview}"
            )));
        }
        serde_json::from_str::<T>(&response_text).map_err(|e| {
            let preview = truncate_chars_with_suffix(
                &response_text,
                500,
                &format!("...(truncated, total {} bytes)", response_text.len()),
            );
            BifrostError::Network(format!(
                "invalid GitHub Gist response: {e} (method={method_str} url={url} body_preview={preview})"
            ))
        })
    }
}

fn write_request(content: &str) -> GitHubGistWriteRequest<'_> {
    let mut files = HashMap::new();
    files.insert(GITHUB_GIST_SNAPSHOT_FILE, GitHubGistWriteFile { content });
    GitHubGistWriteRequest {
        description: GITHUB_GIST_DESCRIPTION,
        public: false,
        files,
    }
}
