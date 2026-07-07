use std::collections::{BTreeMap, HashMap};
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
    pub basic_configs: BTreeMap<String, GitHubGistBasicConfig>,
}

impl Default for GitHubGistSnapshot {
    fn default() -> Self {
        Self {
            version: 1,
            updated_at: String::new(),
            user_id: String::new(),
            rules: Vec::new(),
            basic_configs: BTreeMap::new(),
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
    api_base_url: String,
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
        Self::new_with_base_url(GITHUB_API_BASE_URL)
    }

    pub(crate) fn new_with_base_url(api_base_url: &str) -> Result<Self> {
        let http = bifrost_core::outbound_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|e| BifrostError::Network(format!("failed to build GitHub client: {e}")))?;
        Ok(Self {
            http,
            api_base_url: api_base_url.trim_end_matches('/').to_string(),
        })
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
        let content = encode_snapshot_content(&remote.snapshot)?;
        let body = write_request(&content);
        if let Some(gist_id) = &remote.gist_id {
            let url = format!("{}/gists/{gist_id}", self.api_base_url);
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
        let url = format!("{}/gists", self.api_base_url);
        let gist = self
            .request_json::<_, GitHubGistSummary>(reqwest::Method::POST, &url, token, Some(&body))
            .await?;
        Ok(gist.id)
    }

    async fn find_snapshot_gist(&self, token: &str) -> Result<Option<String>> {
        for page in 1..=5 {
            let url = format!("{}/gists?per_page=100&page={page}", self.api_base_url);
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
        let url = format!("{}/gists/{gist_id}", self.api_base_url);
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

fn encode_snapshot_content(snapshot: &GitHubGistSnapshot) -> Result<String> {
    let mut snapshot = snapshot.clone();
    snapshot
        .rules
        .sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    serde_json::to_string_pretty(&snapshot)
        .map_err(|e| BifrostError::Config(format!("encode GitHub Gist snapshot: {e}")))
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

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn load_snapshot_discovers_and_reads_existing_gist() {
        let server = MockServer::start().await;
        let client = GitHubGistClient::new_with_base_url(&server.uri()).unwrap();
        let snapshot = GitHubGistSnapshot {
            version: 1,
            updated_at: "2026-07-07T00:00:00Z".to_string(),
            user_id: "github:1".to_string(),
            rules: vec![GitHubGistRule {
                id: "gist:rule-1".to_string(),
                user_id: "github:1".to_string(),
                name: "rule-1".to_string(),
                rule: "example.com host://127.0.0.1:3000".to_string(),
                create_time: "2026-07-07T00:00:00Z".to_string(),
                update_time: "2026-07-07T00:00:00Z".to_string(),
            }],
            basic_configs: BTreeMap::new(),
        };
        let snapshot_content = serde_json::to_string(&snapshot).unwrap();

        Mock::given(method("GET"))
            .and(path("/gists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": "gist-1",
                    "description": GITHUB_GIST_DESCRIPTION,
                    "files": {
                        GITHUB_GIST_SNAPSHOT_FILE: {}
                    }
                }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/gists/gist-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "gist-1",
                "description": GITHUB_GIST_DESCRIPTION,
                "files": {
                    GITHUB_GIST_SNAPSHOT_FILE: {
                        "content": snapshot_content
                    }
                }
            })))
            .mount(&server)
            .await;

        let loaded = client.load_snapshot("token").await.unwrap();
        assert_eq!(loaded.gist_id.as_deref(), Some("gist-1"));
        assert_eq!(loaded.snapshot.user_id, "github:1");
        assert_eq!(loaded.snapshot.rules[0].name, "rule-1");
    }

    #[tokio::test]
    async fn load_snapshot_returns_default_when_not_found() {
        let server = MockServer::start().await;
        let client = GitHubGistClient::new_with_base_url(&server.uri()).unwrap();
        Mock::given(method("GET"))
            .and(path("/gists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let loaded = client.load_snapshot("token").await.unwrap();
        assert!(loaded.gist_id.is_none());
        assert!(loaded.snapshot.rules.is_empty());
    }

    #[tokio::test]
    async fn load_snapshot_returns_default_when_snapshot_file_is_empty() {
        let server = MockServer::start().await;
        let client = GitHubGistClient::new_with_base_url(&server.uri()).unwrap();
        Mock::given(method("GET"))
            .and(path("/gists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": "gist-empty",
                    "description": GITHUB_GIST_DESCRIPTION,
                    "files": {
                        GITHUB_GIST_SNAPSHOT_FILE: {}
                    }
                }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/gists/gist-empty"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "gist-empty",
                "description": GITHUB_GIST_DESCRIPTION,
                "files": {
                    GITHUB_GIST_SNAPSHOT_FILE: {
                        "content": "   "
                    }
                }
            })))
            .mount(&server)
            .await;

        let loaded = client.load_snapshot("token").await.unwrap();
        assert_eq!(loaded.gist_id.as_deref(), Some("gist-empty"));
        assert!(loaded.snapshot.rules.is_empty());
        assert!(loaded.snapshot.basic_configs.is_empty());
    }

    #[tokio::test]
    async fn load_snapshot_searches_up_to_five_pages_before_defaulting() {
        let server = MockServer::start().await;
        let client = GitHubGistClient::new_with_base_url(&server.uri()).unwrap();
        for page in 1..=5 {
            Mock::given(method("GET"))
                .and(path("/gists"))
                .and(query_param("page", page.to_string()))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    {
                        "id": format!("other-{page}"),
                        "description": "unrelated",
                        "files": {
                            "notes.md": {}
                        }
                    }
                ])))
                .expect(1)
                .mount(&server)
                .await;
        }

        let loaded = client.load_snapshot("token").await.unwrap();
        assert!(loaded.gist_id.is_none());
        assert!(loaded.snapshot.rules.is_empty());
    }

    #[tokio::test]
    async fn save_snapshot_creates_or_updates_secret_gist() {
        let server = MockServer::start().await;
        let client = GitHubGistClient::new_with_base_url(&server.uri()).unwrap();
        let snapshot = GitHubGistSnapshot {
            version: 1,
            updated_at: "2026-07-07T00:00:00Z".to_string(),
            user_id: "github:1".to_string(),
            rules: Vec::new(),
            basic_configs: BTreeMap::new(),
        };

        Mock::given(method("POST"))
            .and(path("/gists"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": "created-gist",
                "description": GITHUB_GIST_DESCRIPTION,
                "files": {}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/gists/existing-gist"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "existing-gist",
                "description": GITHUB_GIST_DESCRIPTION,
                "files": {}
            })))
            .mount(&server)
            .await;

        let created = client
            .save_snapshot(
                "token",
                &GitHubGistRemoteSnapshot {
                    gist_id: None,
                    snapshot: snapshot.clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(created, "created-gist");

        let updated = client
            .save_snapshot(
                "token",
                &GitHubGistRemoteSnapshot {
                    gist_id: Some("existing-gist".to_string()),
                    snapshot,
                },
            )
            .await
            .unwrap();
        assert_eq!(updated, "existing-gist");
    }

    #[test]
    fn encode_snapshot_content_sorts_rules_and_basic_configs_stably() {
        let mut basic_configs = BTreeMap::new();
        basic_configs.insert(
            "domain_allowlist".to_string(),
            GitHubGistBasicConfig {
                id: "domain_allowlist".to_string(),
                config_key: "domain_allowlist".to_string(),
                value_json: "[\"b.example.com\",\"a.example.com\"]".to_string(),
                ..GitHubGistBasicConfig::default()
            },
        );
        basic_configs.insert(
            "app_allowlist".to_string(),
            GitHubGistBasicConfig {
                id: "app_allowlist".to_string(),
                config_key: "app_allowlist".to_string(),
                value_json: "[\"com.example.App\"]".to_string(),
                ..GitHubGistBasicConfig::default()
            },
        );
        let snapshot = GitHubGistSnapshot {
            version: 1,
            updated_at: "2026-07-07T00:00:00Z".to_string(),
            user_id: "github:1".to_string(),
            rules: vec![
                GitHubGistRule {
                    id: "gist:z".to_string(),
                    name: "zeta".to_string(),
                    rule: "z.example.com host://127.0.0.1:3000".to_string(),
                    ..GitHubGistRule::default()
                },
                GitHubGistRule {
                    id: "gist:a".to_string(),
                    name: "alpha".to_string(),
                    rule: "a.example.com host://127.0.0.1:3000".to_string(),
                    ..GitHubGistRule::default()
                },
            ],
            basic_configs,
        };

        let content = encode_snapshot_content(&snapshot).unwrap();
        assert!(content.find(r#""name": "alpha""#) < content.find(r#""name": "zeta""#));
        assert!(
            content.find(r#""app_allowlist""#) < content.find(r#""domain_allowlist""#),
            "{content}"
        );
    }

    #[tokio::test]
    async fn request_json_reports_auth_and_parse_errors() {
        let server = MockServer::start().await;
        let client = GitHubGistClient::new_with_base_url(&server.uri()).unwrap();
        Mock::given(method("GET"))
            .and(path("/gists"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .mount(&server)
            .await;

        let auth_error = client.load_snapshot("token").await.unwrap_err();
        assert!(auth_error.to_string().contains("missing the gist scope"));

        let server = MockServer::start().await;
        let client = GitHubGistClient::new_with_base_url(&server.uri()).unwrap();
        Mock::given(method("GET"))
            .and(path("/gists"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;
        let parse_error = client.load_snapshot("token").await.unwrap_err();
        assert!(parse_error
            .to_string()
            .contains("invalid GitHub Gist response"));
    }

    #[tokio::test]
    async fn request_json_reports_non_success_with_body_preview() {
        let server = MockServer::start().await;
        let client = GitHubGistClient::new_with_base_url(&server.uri()).unwrap();
        Mock::given(method("GET"))
            .and(path("/gists"))
            .respond_with(ResponseTemplate::new(500).set_body_string("server exploded"))
            .mount(&server)
            .await;

        let error = client.load_snapshot("token").await.unwrap_err();
        let message = error.to_string();
        assert!(message.contains("status 500"));
        assert!(message.contains("server exploded"));
    }
}
