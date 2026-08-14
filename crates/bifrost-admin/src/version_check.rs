use bifrost_core::version_check::{self, VersionCache, GITHUB_RELEASE_URL};
use bifrost_storage::data_dir;
use chrono::Duration;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

const CACHE_FILE_NAME: &str = "version_cache.json";
const CACHE_DURATION_HOURS: i64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionCheckResponse {
    pub has_update: bool,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub release_highlights: Vec<String>,
    pub release_url: Option<String>,
    pub checked_at: Option<String>,
}

pub struct VersionChecker {
    cache: RwLock<Option<VersionCache>>,
}

pub type SharedVersionChecker = Arc<VersionChecker>;

impl VersionChecker {
    pub fn new() -> Self {
        let cache = read_cache();
        Self {
            cache: RwLock::new(cache),
        }
    }

    pub async fn check(&self, force_refresh: bool) -> VersionCheckResponse {
        self.check_with_current_version(force_refresh, env!("CARGO_PKG_VERSION").to_string())
            .await
    }

    pub async fn check_with_current_version(
        &self,
        force_refresh: bool,
        current_version: String,
    ) -> VersionCheckResponse {
        if !force_refresh {
            let cache = self.cache.read().await;
            if let Some(ref c) = *cache {
                if is_cache_valid(c)
                    && version_check::same_release_channel(&current_version, &c.latest_version)
                {
                    return self.build_response(&current_version, Some(c.clone()));
                }
            }
        }

        match version_check::fetch_latest_release_async_for_current(&current_version).await {
            Some((latest, highlights)) => {
                let cache = VersionCache {
                    latest_version: latest,
                    release_highlights: highlights,
                    checked_at: Utc::now(),
                };
                write_cache(&cache);

                {
                    let mut cached = self.cache.write().await;
                    *cached = Some(cache.clone());
                }

                self.build_response(&current_version, Some(cache))
            }
            None => {
                let cache = self.cache.read().await;
                let cache = cache.clone().filter(|cached| {
                    version_check::same_release_channel(&current_version, &cached.latest_version)
                });
                self.build_response(&current_version, cache)
            }
        }
    }

    fn build_response(
        &self,
        current_version: &str,
        cache: Option<VersionCache>,
    ) -> VersionCheckResponse {
        match cache {
            Some(c) => {
                let has_update =
                    version_check::is_newer_version(current_version, &c.latest_version);
                VersionCheckResponse {
                    has_update,
                    current_version: current_version.to_string(),
                    latest_version: Some(c.latest_version.clone()),
                    release_highlights: c.release_highlights.clone(),
                    release_url: Some(format!("{}/v{}", GITHUB_RELEASE_URL, c.latest_version)),
                    checked_at: Some(c.checked_at.to_rfc3339()),
                }
            }
            None => VersionCheckResponse {
                has_update: false,
                current_version: current_version.to_string(),
                latest_version: None,
                release_highlights: vec![],
                release_url: None,
                checked_at: None,
            },
        }
    }
}

impl Default for VersionChecker {
    fn default() -> Self {
        Self::new()
    }
}

fn cache_file_path() -> PathBuf {
    data_dir().join(CACHE_FILE_NAME)
}

fn read_cache() -> Option<VersionCache> {
    let path = cache_file_path();
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

fn write_cache(cache: &VersionCache) {
    let path = cache_file_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(content) = serde_json::to_string_pretty(cache) {
        let _ = fs::write(&path, content);
    }
}

fn is_cache_valid(cache: &VersionCache) -> bool {
    let now = Utc::now();
    let cache_age = now.signed_duration_since(cache.checked_at);
    cache_age < Duration::hours(CACHE_DURATION_HOURS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::version_check::{
        compare_versions, is_newer_version, parse_release_highlights,
    };
    use chrono::Utc;

    #[test]
    fn test_compare_versions() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("1.0.0", "0.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("0.9.9", "1.0.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.0", "1.0.0-alpha"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0-alpha", "1.0.0"), Ordering::Less);
    }

    #[test]
    fn test_is_newer_version() {
        assert!(is_newer_version("0.0.1", "1.0.0"));
        assert!(is_newer_version("0.0.1-alpha", "0.0.1"));
        assert!(!is_newer_version("1.0.0", "0.0.1"));
        assert!(!is_newer_version("1.0.0", "1.0.0"));
    }

    #[test]
    fn build_response_marks_desktop_app_version_older_than_latest() {
        let checker = VersionChecker::new();
        let response = checker.build_response(
            "0.0.144",
            Some(VersionCache {
                latest_version: "0.0.145".to_string(),
                release_highlights: vec![],
                checked_at: Utc::now(),
            }),
        );

        assert!(response.has_update);
        assert_eq!(response.current_version, "0.0.144");
        assert_eq!(response.latest_version.as_deref(), Some("0.0.145"));
    }

    #[tokio::test]
    async fn check_reuses_fresh_cache_only_for_the_running_channel() {
        let cached = VersionCache {
            latest_version: "0.0.181-alpha.11".to_string(),
            release_highlights: vec!["cached alpha".to_string()],
            checked_at: Utc::now(),
        };
        let checker = VersionChecker {
            cache: RwLock::new(Some(cached)),
        };

        let response = checker
            .check_with_current_version(false, "0.0.181-alpha.10".to_string())
            .await;

        assert!(response.has_update);
        assert_eq!(response.latest_version.as_deref(), Some("0.0.181-alpha.11"));
        assert_eq!(response.release_highlights, vec!["cached alpha"]);
    }

    #[test]
    fn test_parse_release_highlights() {
        let body = r#"## ✨ Highlights

- Added new feature A
- Improved performance by 50%
- Fixed critical bug

## What's Changed
"#;
        let highlights = parse_release_highlights(Some(body));
        assert_eq!(highlights.len(), 3);
        assert_eq!(highlights[0], "Added new feature A");
    }
}
