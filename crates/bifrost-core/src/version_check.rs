use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::debug;

pub const GITHUB_RELEASES_LATEST_URL: &str =
    "https://github.com/bifrost-proxy/bifrost/releases/latest";
pub const GITHUB_RELEASES_API_URL: &str =
    "https://api.github.com/repos/bifrost-proxy/bifrost/releases/latest";
pub const GITHUB_RELEASES_API_LIST_URL: &str =
    "https://api.github.com/repos/bifrost-proxy/bifrost/releases";
pub const GITHUB_RELEASES_HTML_URL: &str = "https://github.com/bifrost-proxy/bifrost/releases";
const GITHUB_RELEASE_API_URLS: (&str, &str) =
    (GITHUB_RELEASES_API_URL, GITHUB_RELEASES_API_LIST_URL);
pub const GITHUB_TAGS_API_URL: &str = "https://api.github.com/repos/bifrost-proxy/bifrost/tags";
pub const GITHUB_RELEASE_URL: &str = "https://github.com/bifrost-proxy/bifrost/releases/tag";
pub const REQUEST_TIMEOUT_SECS: u64 = 10;
const HIGHLIGHTS_TIMEOUT_SECS: u64 = 5;
pub const MAX_RETRIES: u32 = 2;
pub const RETRY_DELAY_MS: u64 = 500;
pub const GITHUB_RELEASES_PER_PAGE: usize = 100;
const GITHUB_RELEASES_HTML_MAX_PAGES: usize = 20;
const MAX_RELEASE_HIGHLIGHTS: usize = 50;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VersionCache {
    pub latest_version: String,
    pub release_highlights: Vec<String>,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
}

#[derive(Debug, Deserialize)]
pub struct GitHubTag {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseChannel {
    Stable,
    Prerelease(String),
}

#[derive(Debug)]
pub enum FetchError {
    Network(String),
    Parse(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Network(msg) => write!(f, "{}", msg),
            FetchError::Parse(msg) => write!(f, "{}", msg),
        }
    }
}

pub fn extract_version_from_redirect_url(url: &str) -> Result<String, FetchError> {
    if let Some(idx) = url.rfind("/tag/") {
        let tag = &url[idx + 5..];
        let tag = tag.trim_end_matches('/');
        let version = bifrost_version_from_release_tag(tag).ok_or_else(|| {
            FetchError::Parse(format!("not a Bifrost release tag in URL: {}", url))
        })?;
        debug!(version = %version, url = %url, "extracted version from redirect");
        Ok(version)
    } else {
        Err(FetchError::Parse(format!(
            "no /tag/ found in redirect URL: {}",
            url
        )))
    }
}

pub fn bifrost_version_from_release_tag(tag: &str) -> Option<String> {
    let version = tag.strip_prefix('v')?;
    let (core, prerelease) = match version.split_once('-') {
        Some((core, prerelease)) => (core, Some(prerelease)),
        None => (version, None),
    };
    let mut parts = core.split('.');
    let valid_core = (0..3).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
    }) && parts.next().is_none();
    let valid_prerelease = prerelease.is_none_or(|value| {
        !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
            && value.chars().any(|c| c.is_ascii_alphanumeric())
    });
    (valid_core && valid_prerelease).then(|| version.to_string())
}

pub fn stable_bifrost_release_version(release: &GitHubRelease) -> Option<String> {
    if release.draft || release.prerelease {
        return None;
    }
    let version = bifrost_version_from_release_tag(&release.tag_name)?;
    (!version.contains('-')).then_some(version)
}

fn decode_msi_prerelease(value: u32) -> Option<(&'static str, u32)> {
    match value {
        10_000..=19_999 => Some(("alpha", value - 10_000)),
        20_000..=29_999 => Some(("beta", value - 20_000)),
        30_000..=39_999 => Some(("rc", value - 30_000)),
        _ => None,
    }
}

fn canonical_release_version(version: &str) -> String {
    let normalized = version.trim().trim_start_matches('v');

    let msi_parts: Vec<_> = normalized.split('.').collect();
    if msi_parts.len() == 4
        && msi_parts.iter().all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
    {
        if let Ok(encoded) = msi_parts[3].parse::<u32>() {
            if let Some((channel, sequence)) = decode_msi_prerelease(encoded) {
                let core = msi_parts[..3].join(".");
                return if sequence == 0 {
                    format!("{core}-{channel}")
                } else {
                    format!("{core}-{channel}.{sequence}")
                };
            }
        }
    }

    if let Some((core, prerelease)) = normalized.split_once('-') {
        if let Ok(encoded) = prerelease.parse::<u32>() {
            if let Some((channel, sequence)) = decode_msi_prerelease(encoded) {
                return if sequence == 0 {
                    format!("{core}-{channel}")
                } else {
                    format!("{core}-{channel}.{sequence}")
                };
            }
        }

        let first_end = prerelease.find(['.', '-']).unwrap_or(prerelease.len());
        let first = &prerelease[..first_end];
        let label_end = first
            .find(|character: char| !character.is_ascii_alphabetic())
            .unwrap_or(first.len());
        if label_end > 0
            && label_end < first.len()
            && first[label_end..]
                .chars()
                .all(|character| character.is_ascii_digit())
        {
            return format!(
                "{core}-{}.{}{}",
                &first[..label_end],
                &first[label_end..],
                &prerelease[first_end..]
            );
        }
    }

    normalized.to_string()
}

pub fn release_channel(version: &str) -> ReleaseChannel {
    let canonical = canonical_release_version(version);
    let version = canonical.as_str();
    let Some((_, prerelease)) = version.split_once('-') else {
        return ReleaseChannel::Stable;
    };
    let first = prerelease.split(['.', '-']).next().unwrap_or_default();
    let label_end = first
        .find(|ch: char| !ch.is_ascii_alphabetic())
        .unwrap_or(first.len());
    let label = first[..label_end].to_ascii_lowercase();
    if label.is_empty() {
        ReleaseChannel::Prerelease(prerelease.to_ascii_lowercase())
    } else {
        ReleaseChannel::Prerelease(label)
    }
}

pub fn same_release_channel(current: &str, candidate: &str) -> bool {
    let current = canonical_release_version(current);
    let candidate = canonical_release_version(candidate);
    semver::Version::parse(&current).is_ok()
        && semver::Version::parse(&candidate).is_ok()
        && release_channel(&current) == release_channel(&candidate)
}

fn release_version_for_channel(
    release: &GitHubRelease,
    channel: &ReleaseChannel,
) -> Option<String> {
    if release.draft {
        return None;
    }
    let version = bifrost_version_from_release_tag(&release.tag_name)?;
    match channel {
        ReleaseChannel::Stable => {
            (!release.prerelease && !version.contains('-')).then_some(version)
        }
        ReleaseChannel::Prerelease(_) => {
            (release.prerelease && release_channel(&version) == *channel).then_some(version)
        }
    }
}

pub fn pick_latest_bifrost_release_for_channel(
    releases: Vec<GitHubRelease>,
    channel: &ReleaseChannel,
) -> Option<GitHubRelease> {
    releases
        .into_iter()
        .filter_map(|release| {
            release_version_for_channel(&release, channel).map(|version| (release, version))
        })
        .max_by(|(_, a), (_, b)| compare_versions(a, b))
        .map(|(release, _)| release)
}

pub fn pick_latest_bifrost_release(releases: Vec<GitHubRelease>) -> Option<GitHubRelease> {
    pick_latest_bifrost_release_for_channel(releases, &ReleaseChannel::Stable)
}

pub fn github_releases_api_list_url(page: usize) -> String {
    github_releases_api_list_url_from(GITHUB_RELEASES_API_LIST_URL, page)
}

fn github_releases_api_list_url_from(base_url: &str, page: usize) -> String {
    format!(
        "{base_url}?per_page={GITHUB_RELEASES_PER_PAGE}&page={}",
        page.max(1)
    )
}

fn github_releases_html_page_url_from(base_url: &str, page: usize) -> String {
    if page <= 1 {
        return base_url.to_string();
    }
    let separator = if base_url.contains('?') { '&' } else { '?' };
    format!("{base_url}{separator}page={page}")
}

fn release_versions_from_html(html: &str) -> Vec<String> {
    const RELEASE_TAG_MARKER: &str = "/bifrost-proxy/bifrost/releases/tag/";
    let mut versions = Vec::new();
    let mut remaining = html;
    while let Some(offset) = remaining.find(RELEASE_TAG_MARKER) {
        let tag_start = offset + RELEASE_TAG_MARKER.len();
        let tag_tail = &remaining[tag_start..];
        let tag_end = tag_tail
            .find(|character: char| {
                character.is_ascii_whitespace()
                    || matches!(character, '"' | '\'' | '<' | '>' | '?' | '#')
            })
            .unwrap_or(tag_tail.len());
        let tag = &tag_tail[..tag_end];
        if let Some(version) = bifrost_version_from_release_tag(tag) {
            if !versions.contains(&version) {
                versions.push(version);
            }
        }
        remaining = &tag_tail[tag_end..];
    }
    versions
}

fn pick_latest_release_version_from_html_for_channel(
    html: &str,
    channel: &ReleaseChannel,
) -> Option<String> {
    release_versions_from_html(html)
        .into_iter()
        .filter(|version| release_channel(version) == *channel)
        .max_by(|left, right| compare_versions(left, right))
}

fn published_release_request_error(error: &GithubRequestError) -> FetchError {
    if matches!(error, GithubRequestError::Status(status) if *status == reqwest::StatusCode::FORBIDDEN)
    {
        FetchError::Network(
            "GitHub releases API rate limited the unauthenticated request".to_string(),
        )
    } else {
        let reason = classify_github_request_error(error);
        FetchError::Network(format!("{reason}: {error}"))
    }
}

fn fetch_latest_release_from_html_sync_for_channel(
    client: &reqwest::blocking::Client,
    base_url: &str,
    channel: &ReleaseChannel,
) -> Result<(String, Vec<String>), FetchError> {
    for page in 1..=GITHUB_RELEASES_HTML_MAX_PAGES {
        let url = github_releases_html_page_url_from(base_url, page);
        let response = fetch_with_retry(client, &url).map_err(|error| {
            let reason = classify_github_request_error(&error);
            FetchError::Network(format!(
                "GitHub releases HTML fallback failed: {reason}: {error}"
            ))
        })?;
        let html = response.text().map_err(|error| {
            FetchError::Parse(format!(
                "failed to read GitHub releases HTML page {page}: {error}"
            ))
        })?;
        let versions = release_versions_from_html(&html);
        if versions.is_empty() {
            break;
        }
        if let Some(version) = pick_latest_release_version_from_html_for_channel(&html, channel) {
            return Ok((version, Vec::new()));
        }
    }

    let label = match channel {
        ReleaseChannel::Stable => "stable",
        ReleaseChannel::Prerelease(label) => label,
    };
    Err(FetchError::Parse(format!(
        "no published Bifrost releases found in releases HTML for channel {label}"
    )))
}

fn fetch_latest_published_release_sync(
    client: &reqwest::blocking::Client,
    base_url: &str,
) -> Result<(String, Vec<String>), FetchError> {
    fetch_latest_published_release_sync_for_channel(client, base_url, &ReleaseChannel::Stable)
}

fn fetch_latest_published_release_sync_for_channel(
    client: &reqwest::blocking::Client,
    base_url: &str,
    channel: &ReleaseChannel,
) -> Result<(String, Vec<String>), FetchError> {
    let mut published_releases = Vec::new();
    let mut page = 1;
    loop {
        let url = github_releases_api_list_url_from(base_url, page);
        let page_releases = fetch_with_retry(client, &url)
            .map_err(|error| published_release_request_error(&error))
            .and_then(|response| {
                response.json::<Vec<GitHubRelease>>().map_err(|error| {
                    FetchError::Parse(format!("failed to parse releases page {page}: {error}"))
                })
            });
        match page_releases {
            Ok(releases) if releases.is_empty() => break,
            Ok(releases) => published_releases.extend(releases),
            Err(error) if !published_releases.is_empty() => {
                debug!(
                    page,
                    error = %error,
                    "stopping published release pagination after a later page failed"
                );
                break;
            }
            Err(error) => return Err(error),
        }
        page += 1;
    }

    let release =
        pick_latest_bifrost_release_for_channel(published_releases, channel).ok_or_else(|| {
            let message = match channel {
                ReleaseChannel::Stable => "no published stable Bifrost releases found".to_string(),
                ReleaseChannel::Prerelease(label) => {
                    format!("no published Bifrost releases found for prerelease channel {label}")
                }
            };
            FetchError::Parse(message)
        })?;
    let version = release_version_for_channel(&release, channel)
        .expect("published release picker only returns releases from the requested channel");
    let highlights = parse_release_highlights(release.body.as_deref());
    Ok((version, highlights))
}

async fn fetch_latest_published_release_async(
    client: &reqwest::Client,
    base_url: &str,
) -> Option<(String, Vec<String>)> {
    fetch_latest_published_release_async_for_channel(client, base_url, &ReleaseChannel::Stable)
        .await
}

async fn fetch_latest_published_release_async_for_channel(
    client: &reqwest::Client,
    base_url: &str,
    channel: &ReleaseChannel,
) -> Option<(String, Vec<String>)> {
    let mut published_releases = Vec::new();
    let mut page = 1;
    loop {
        let response = match client
            .get(github_releases_api_list_url_from(base_url, page))
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) if !published_releases.is_empty() => break,
            Err(_) => return None,
        };
        if !response.status().is_success() {
            break;
        }
        let page_releases: Vec<GitHubRelease> = match response.json().await {
            Ok(releases) => releases,
            Err(_) => break,
        };
        if page_releases.is_empty() {
            break;
        }
        published_releases.extend(page_releases);
        page += 1;
    }

    let release = pick_latest_bifrost_release_for_channel(published_releases, channel)?;
    let version = release_version_for_channel(&release, channel)?;
    let highlights = parse_release_highlights(release.body.as_deref());
    Some((version, highlights))
}

async fn fetch_latest_release_from_html_async_for_channel(
    client: &reqwest::Client,
    base_url: &str,
    channel: &ReleaseChannel,
) -> Option<(String, Vec<String>)> {
    for page in 1..=GITHUB_RELEASES_HTML_MAX_PAGES {
        let response = client
            .get(github_releases_html_page_url_from(base_url, page))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let html = response.text().await.ok()?;
        if release_versions_from_html(&html).is_empty() {
            return None;
        }
        if let Some(version) = pick_latest_release_version_from_html_for_channel(&html, channel) {
            return Some((version, Vec::new()));
        }
    }
    None
}

pub fn make_release_tag(version: &str) -> String {
    if version.contains('-') || version.chars().next().is_none_or(|c| c.is_ascii_digit()) {
        format!("v{}", version)
    } else {
        version.to_string()
    }
}

pub fn release_api_url_for_tag(tag: &str) -> String {
    format!(
        "https://api.github.com/repos/bifrost-proxy/bifrost/releases/tags/{}",
        tag
    )
}

pub fn strip_tag_prefix(tag: &str) -> String {
    tag.strip_prefix('v').unwrap_or(tag).to_string()
}

pub fn pick_latest_tag(tags: Vec<GitHubTag>) -> Option<String> {
    tags.into_iter()
        .filter_map(|tag| bifrost_version_from_release_tag(&tag.name))
        .max_by(|a, b| compare_versions(a, b))
}

pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    fn normalize(version: &str) -> &str {
        version.trim().trim_start_matches('v')
    }

    fn compare_legacy(a: &str, b: &str) -> std::cmp::Ordering {
        let parse = |version: &str| {
            let (version, prerelease) = version
                .split_once('-')
                .map_or((version, String::new()), |(version, prerelease)| {
                    (version, prerelease.to_string())
                });
            let parts: Vec<u32> = version
                .split('.')
                .filter_map(|part| part.parse().ok())
                .collect();
            (
                parts.first().copied().unwrap_or(0),
                parts.get(1).copied().unwrap_or(0),
                parts.get(2).copied().unwrap_or(0),
                prerelease,
            )
        };

        let (a_major, a_minor, a_patch, a_prerelease) = parse(a);
        let (b_major, b_minor, b_patch, b_prerelease) = parse(b);
        match (a_major, a_minor, a_patch).cmp(&(b_major, b_minor, b_patch)) {
            std::cmp::Ordering::Equal => match (a_prerelease.is_empty(), b_prerelease.is_empty()) {
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                _ => a_prerelease.cmp(&b_prerelease),
            },
            ordering => ordering,
        }
    }

    let canonical_a = canonical_release_version(a);
    let canonical_b = canonical_release_version(b);
    match (
        semver::Version::parse(&canonical_a),
        semver::Version::parse(&canonical_b),
    ) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => compare_legacy(normalize(a), normalize(b)),
    }
}

pub fn is_newer_version(current: &str, latest: &str) -> bool {
    compare_versions(latest, current) == std::cmp::Ordering::Greater
}

pub fn classify_ureq_error(err: &ureq::Error) -> &'static str {
    match err {
        ureq::Error::Status(status, _) => {
            if *status == 403 {
                "GitHub API rate limit exceeded"
            } else if *status == 404 {
                "GitHub API endpoint not found"
            } else {
                "HTTP error from GitHub API"
            }
        }
        ureq::Error::Transport(transport) => match transport.kind() {
            ureq::ErrorKind::Dns => "DNS resolution failed (check network connectivity)",
            ureq::ErrorKind::ConnectionFailed => {
                "connection failed (GitHub may be unreachable from your network)"
            }
            ureq::ErrorKind::Io => {
                let msg = transport.to_string().to_lowercase();
                if msg.contains("timed out") || msg.contains("timeout") {
                    "connection timed out (GitHub may be unreachable from your network)"
                } else {
                    "network I/O error"
                }
            }
            ureq::ErrorKind::ProxyConnect | ureq::ErrorKind::ProxyUnauthorized => {
                "proxy-related error"
            }
            ureq::ErrorKind::InvalidUrl | ureq::ErrorKind::UnknownScheme => "invalid URL",
            ureq::ErrorKind::TooManyRedirects => "too many redirects",
            ureq::ErrorKind::InsecureRequestHttpsOnly => "TLS/SSL configuration error",
            _ => "network error",
        },
    }
}

pub fn fetch_with_retry(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<reqwest::blocking::Response, Box<GithubRequestError>> {
    let mut last_err = None;
    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            debug!(
                attempt = attempt + 1,
                max = MAX_RETRIES + 1,
                url,
                "retrying request"
            );
            std::thread::sleep(std::time::Duration::from_millis(
                RETRY_DELAY_MS * (attempt as u64),
            ));
        }
        match client.get(url).send() {
            Ok(resp) if resp.status().is_success() => return Ok(resp),
            Ok(resp) => {
                let status = resp.status();
                debug!(
                    url,
                    attempt = attempt + 1,
                    status = status.as_u16(),
                    "request returned non-success status"
                );
                last_err = Some(Box::new(GithubRequestError::Status(status)));
            }
            Err(e) => {
                let error = GithubRequestError::Transport(e);
                let reason = classify_github_request_error(&error);
                debug!(
                    url,
                    attempt = attempt + 1,
                    error = %error,
                    reason,
                    "request failed"
                );
                last_err = Some(Box::new(error));
            }
        }
    }
    Err(last_err.unwrap())
}

#[derive(Debug)]
pub enum GithubRequestError {
    Status(reqwest::StatusCode),
    Transport(reqwest::Error),
}

impl std::fmt::Display for GithubRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GithubRequestError::Status(status) => write!(f, "HTTP {}", status),
            GithubRequestError::Transport(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for GithubRequestError {}

pub fn classify_github_request_error(err: &GithubRequestError) -> &'static str {
    match err {
        GithubRequestError::Status(status) => {
            if *status == reqwest::StatusCode::FORBIDDEN {
                "GitHub API rate limit exceeded"
            } else if *status == reqwest::StatusCode::NOT_FOUND {
                "GitHub API endpoint not found"
            } else {
                "HTTP error from GitHub API"
            }
        }
        GithubRequestError::Transport(error) => {
            if error.is_timeout() {
                "connection timed out (GitHub may be unreachable from your network)"
            } else if error.is_connect() {
                "connection failed (GitHub may be unreachable from your network)"
            } else if error.is_request() {
                "invalid URL or request"
            } else {
                let msg = crate::format_reqwest_error(error).to_lowercase();
                if msg.contains("certificate")
                    || msg.contains("tls")
                    || msg.contains("ssl")
                    || msg.contains("unknownissuer")
                    || msg.contains("unknown issuer")
                {
                    "TLS/SSL certificate verification failed"
                } else {
                    "network error"
                }
            }
        }
    }
}

pub fn fetch_version_via_redirect_sync() -> Result<String, FetchError> {
    let client = crate::github_blocking_reqwest_client_builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent("bifrost-cli")
        .build()
        .map_err(|e| FetchError::Network(format!("failed to build GitHub HTTP client: {e}")))?;

    debug!(
        "fetching latest version via redirect from {}",
        GITHUB_RELEASES_LATEST_URL
    );

    match client.head(GITHUB_RELEASES_LATEST_URL).send() {
        Ok(resp) => {
            let final_url = resp.url().to_string();
            debug!(final_url = %final_url, "redirect followed, extracting version from final URL");
            extract_version_from_redirect_url(&final_url)
        }
        Err(e) => {
            let error = GithubRequestError::Transport(e);
            let reason = classify_github_request_error(&error);
            Err(FetchError::Network(format!("{}: {}", reason, error)))
        }
    }
}

pub fn release_page_url(version: &str) -> String {
    let tag = make_release_tag(version);
    format!("{}/{}", GITHUB_RELEASE_URL, tag)
}

fn extract_highlights_from_html(html: &str) -> Vec<String> {
    let body_content = if let Some(start) = html.find("data-test-selector=\"body-content\"") {
        let chunk = &html[start..];
        if let Some(div_start) = chunk.find('>') {
            let inner = &chunk[div_start + 1..];
            if let Some(end) = find_closing_div(inner) {
                &inner[..end]
            } else {
                inner
            }
        } else {
            return Vec::new();
        }
    } else {
        return Vec::new();
    };

    let mut items = Vec::new();
    let mut search_from = 0;
    while let Some(li_start) = body_content[search_from..].find("<li>") {
        let content_start = search_from + li_start + 4;
        if let Some(li_end) = body_content[content_start..].find("</li>") {
            let raw = &body_content[content_start..content_start + li_end];
            let text = strip_html_tags(raw).trim().to_string();
            if !text.is_empty() {
                items.push(text);
            }
            search_from = content_start + li_end + 5;
        } else {
            break;
        }
    }

    if items.is_empty() {
        return Vec::new();
    }

    let markdown_lines: Vec<String> = items.iter().map(|item| format!("- {}", item)).collect();
    let pseudo_body = markdown_lines.join("\n");
    parse_release_highlights(Some(&pseudo_body))
}

fn find_closing_div(html: &str) -> Option<usize> {
    let mut depth = 1i32;
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut pos = 0;
    while pos < len {
        if pos + 6 <= len && &bytes[pos..pos + 6] == b"</div>" {
            depth -= 1;
            if depth == 0 {
                return Some(pos);
            }
            pos += 6;
        } else if pos + 4 <= len && &bytes[pos..pos + 4] == b"<div" {
            depth += 1;
            pos += 4;
        } else {
            pos += 1;
        }
    }
    None
}

fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut inside_tag = false;
    for ch in html.chars() {
        if ch == '<' {
            inside_tag = true;
        } else if ch == '>' {
            inside_tag = false;
        } else if !inside_tag {
            result.push(ch);
        }
    }
    result
}

pub fn fetch_highlights_from_html_sync(version: &str) -> Vec<String> {
    let url = release_page_url(version);
    debug!(url = %url, "fetching release highlights from HTML page");

    let client = match crate::github_blocking_reqwest_client_builder()
        .timeout(std::time::Duration::from_secs(HIGHLIGHTS_TIMEOUT_SECS))
        .user_agent("bifrost-cli")
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            debug!(error = %e, "failed to build GitHub HTTP client for release page");
            return Vec::new();
        }
    };

    match client.get(&url).send() {
        Ok(resp) => match resp.text() {
            Ok(html) => {
                let highlights = extract_highlights_from_html(&html);
                if highlights.is_empty() {
                    debug!("no highlights extracted from HTML page");
                }
                highlights
            }
            Err(e) => {
                debug!(error = %e, "failed to read HTML response body");
                Vec::new()
            }
        },
        Err(e) => {
            debug!(error = %e, "failed to fetch release page HTML (non-critical)");
            Vec::new()
        }
    }
}

pub fn fetch_release_body_for_version_sync(version: &str) -> Vec<String> {
    let tag = make_release_tag(version);
    let url = release_api_url_for_tag(&tag);

    debug!(url = %url, "fetching release body via API (fallback)");

    let client = match crate::github_blocking_reqwest_client_builder()
        .timeout(std::time::Duration::from_secs(HIGHLIGHTS_TIMEOUT_SECS))
        .user_agent("bifrost-cli")
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            debug!(error = %e, "failed to build GitHub HTTP client for release API");
            return Vec::new();
        }
    };

    match client.get(&url).send() {
        Ok(response) => match response.json::<GitHubRelease>() {
            Ok(release) => parse_release_highlights(release.body.as_deref()),
            Err(e) => {
                debug!(error = %e, "failed to parse release body");
                Vec::new()
            }
        },
        Err(e) => {
            debug!(error = %e, "failed to fetch release body via API (non-critical)");
            Vec::new()
        }
    }
}

pub fn fetch_latest_release_sync() -> Result<(String, Vec<String>), FetchError> {
    let client = crate::github_blocking_reqwest_client_builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent("bifrost-cli")
        .build()
        .map_err(|e| FetchError::Network(format!("failed to build GitHub HTTP client: {e}")))?;

    match fetch_version_via_redirect_sync() {
        Ok(version) => {
            let mut highlights = fetch_release_body_for_version_sync(&version);
            if highlights.is_empty() {
                debug!("API highlights empty or rate limited, trying HTML page fallback");
                highlights = fetch_highlights_from_html_sync(&version);
            }
            return Ok((version, highlights));
        }
        Err(e) => {
            debug!(error = %e, "redirect-based version detection failed, falling back to GitHub API");
        }
    }

    fetch_latest_release_from_api_sync(&client, GITHUB_RELEASE_API_URLS)
}

pub fn fetch_latest_release_sync_for_current(
    current_version: &str,
) -> Result<(String, Vec<String>), FetchError> {
    fetch_latest_release_sync_for_current_from(current_version, GITHUB_RELEASES_API_LIST_URL)
}

fn fetch_latest_release_sync_for_current_from(
    current_version: &str,
    prerelease_url: &str,
) -> Result<(String, Vec<String>), FetchError> {
    fetch_latest_release_sync_for_current_from_sources(
        current_version,
        prerelease_url,
        GITHUB_RELEASES_HTML_URL,
    )
}

fn fetch_latest_release_sync_for_current_from_sources(
    current_version: &str,
    prerelease_url: &str,
    releases_html_url: &str,
) -> Result<(String, Vec<String>), FetchError> {
    let channel = release_channel(current_version);
    if channel == ReleaseChannel::Stable {
        return fetch_latest_release_sync();
    }

    let client = crate::github_blocking_reqwest_client_builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent("bifrost-cli")
        .build()
        .map_err(|e| FetchError::Network(format!("failed to build GitHub HTTP client: {e}")))?;
    match fetch_latest_published_release_sync_for_channel(&client, prerelease_url, &channel) {
        Ok(release) => Ok(release),
        Err(api_error) => {
            debug!(
                error = %api_error,
                "prerelease API discovery failed, trying public releases HTML"
            );
            fetch_latest_release_from_html_sync_for_channel(
                &client,
                releases_html_url,
                &channel,
            )
            .map_err(|html_error| {
                FetchError::Network(format!(
                    "failed to check prerelease channel via GitHub API ({api_error}) and releases HTML ({html_error})"
                ))
            })
        }
    }
}

fn fetch_latest_release_from_api_sync(
    client: &reqwest::blocking::Client,
    (latest_url, releases_url): (&str, &str),
) -> Result<(String, Vec<String>), FetchError> {
    match fetch_with_retry(client, latest_url) {
        Ok(response) => match response.json::<GitHubRelease>() {
            Ok(release) => {
                if let Some(version) = stable_bifrost_release_version(&release) {
                    let highlights = parse_release_highlights(release.body.as_deref());
                    return Ok((version, highlights));
                }
                debug!(tag = %release.tag_name, "latest GitHub release is not a stable Bifrost release; scanning published releases");
            }
            Err(e) => {
                debug!(error = %e, "failed to parse latest GitHub release JSON, scanning published releases");
            }
        },
        Err(e) => {
            let reason = classify_github_request_error(&e);
            debug!(
                error = %e,
                reason,
                "latest release API failed, scanning published releases"
            );
        }
    }

    fetch_latest_published_release_sync(client, releases_url)
}

pub async fn fetch_version_via_redirect_async() -> Option<String> {
    debug!(
        "fetching latest version via redirect from {}",
        GITHUB_RELEASES_LATEST_URL
    );

    let client = crate::github_reqwest_client_builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent("bifrost-admin")
        .build()
        .ok()?;

    let resp = client.head(GITHUB_RELEASES_LATEST_URL).send().await.ok()?;

    let final_url = resp.url().to_string();
    debug!(final_url = %final_url, "redirect followed (async), extracting version from final URL");
    extract_version_from_redirect_url(&final_url).ok()
}

pub async fn fetch_highlights_from_html_async(version: &str) -> Vec<String> {
    let url = release_page_url(version);
    debug!(url = %url, "fetching release highlights from HTML page (async)");

    let client = match crate::github_reqwest_client_builder()
        .timeout(std::time::Duration::from_secs(HIGHLIGHTS_TIMEOUT_SECS))
        .user_agent("bifrost-admin")
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    match client.get(&url).send().await {
        Ok(resp) => match resp.text().await {
            Ok(html) => {
                let highlights = extract_highlights_from_html(&html);
                if highlights.is_empty() {
                    debug!("no highlights extracted from HTML page (async)");
                }
                highlights
            }
            Err(e) => {
                debug!(error = %e, "failed to read HTML response body (async)");
                Vec::new()
            }
        },
        Err(e) => {
            debug!(error = %e, "failed to fetch release page HTML (async, non-critical)");
            Vec::new()
        }
    }
}

pub async fn fetch_release_body_for_version_async(version: &str) -> Vec<String> {
    let tag = make_release_tag(version);
    let url = release_api_url_for_tag(&tag);

    debug!(url = %url, "fetching release body via API (async fallback)");

    let client = match crate::github_reqwest_client_builder()
        .timeout(std::time::Duration::from_secs(HIGHLIGHTS_TIMEOUT_SECS))
        .user_agent("bifrost-admin")
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    match client.get(&url).send().await {
        Ok(response) => match response.json::<GitHubRelease>().await {
            Ok(release) => parse_release_highlights(release.body.as_deref()),
            Err(e) => {
                debug!(error = %e, "failed to parse release body");
                Vec::new()
            }
        },
        Err(e) => {
            debug!(error = %e, "failed to fetch release body via API (async, non-critical)");
            Vec::new()
        }
    }
}

pub async fn fetch_latest_release_async() -> Option<(String, Vec<String>)> {
    let client = crate::github_reqwest_client_builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent("bifrost-admin")
        .build()
        .ok()?;

    if let Some(version) = fetch_version_via_redirect_async().await {
        let mut highlights = fetch_release_body_for_version_async(&version).await;
        if highlights.is_empty() {
            debug!("API highlights empty or rate limited (async), trying HTML page fallback");
            highlights = fetch_highlights_from_html_async(&version).await;
        }
        return Some((version, highlights));
    }

    debug!("redirect-based version detection failed, falling back to GitHub API");

    fetch_latest_release_from_api_async(&client, GITHUB_RELEASE_API_URLS).await
}

pub async fn fetch_latest_release_async_for_current(
    current_version: &str,
) -> Option<(String, Vec<String>)> {
    fetch_latest_release_async_for_current_from(current_version, GITHUB_RELEASES_API_LIST_URL).await
}

async fn fetch_latest_release_async_for_current_from(
    current_version: &str,
    prerelease_url: &str,
) -> Option<(String, Vec<String>)> {
    fetch_latest_release_async_for_current_from_sources(
        current_version,
        prerelease_url,
        GITHUB_RELEASES_HTML_URL,
    )
    .await
}

async fn fetch_latest_release_async_for_current_from_sources(
    current_version: &str,
    prerelease_url: &str,
    releases_html_url: &str,
) -> Option<(String, Vec<String>)> {
    let channel = release_channel(current_version);
    if channel == ReleaseChannel::Stable {
        return fetch_latest_release_async().await;
    }

    let client = crate::github_reqwest_client_builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent("bifrost-admin")
        .build()
        .ok()?;
    match fetch_latest_published_release_async_for_channel(&client, prerelease_url, &channel).await
    {
        Some(release) => Some(release),
        None => {
            debug!("prerelease API discovery failed, trying public releases HTML (async)");
            fetch_latest_release_from_html_async_for_channel(&client, releases_html_url, &channel)
                .await
        }
    }
}

async fn fetch_latest_release_from_api_async(
    client: &reqwest::Client,
    (latest_url, releases_url): (&str, &str),
) -> Option<(String, Vec<String>)> {
    if let Ok(response) = client.get(latest_url).send().await {
        if let Ok(release) = response.json::<GitHubRelease>().await {
            if let Some(version) = stable_bifrost_release_version(&release) {
                let highlights = parse_release_highlights(release.body.as_deref());
                return Some((version, highlights));
            }
            debug!(tag = %release.tag_name, "latest GitHub release is not a stable Bifrost release; scanning published releases (async)");
        }
    }

    fetch_latest_published_release_async(client, releases_url).await
}

pub fn parse_release_highlights(body: Option<&str>) -> Vec<String> {
    let body = match body {
        Some(b) if !b.trim().is_empty() => b,
        _ => return Vec::new(),
    };

    let mut highlights = Vec::new();

    let normalize = |s: &str| -> String {
        let mapped: String = s
            .chars()
            .filter(|c| !c.is_control())
            .map(|c| match c {
                '\u{2018}' | '\u{2019}' => '\'',
                _ => c,
            })
            .collect();
        mapped
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || c.is_whitespace() || *c == '\'')
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string()
    };

    let lines_iter = body.lines().enumerate().peekable();
    for (idx, line) in lines_iter {
        let l = line.trim();
        if l.starts_with("## ") {
            let title = normalize(l.trim_start_matches("## ").trim());
            if title.contains("highlights")
                || title.contains("what's new")
                || title.contains("whats new")
                || title.contains("what\u{2019}s new")
            {
                let mut j = idx + 1;
                while let Some(next_line) = body.lines().nth(j) {
                    let nl = next_line.trim();
                    if nl.starts_with("## ") {
                        break;
                    }
                    if !nl.is_empty() {
                        let cleaned = nl
                            .trim_start_matches("- ")
                            .trim_start_matches("* ")
                            .trim_start_matches("• ")
                            .trim();
                        if !cleaned.is_empty() {
                            highlights.push(cleaned.to_string());
                            if highlights.len() >= MAX_RELEASE_HIGHLIGHTS {
                                return highlights;
                            }
                        }
                    }
                    j += 1;
                }
            }
        }
    }

    if highlights.is_empty() {
        let mut k = 0usize;
        while k < body.lines().count() {
            let ln = body.lines().nth(k).unwrap().trim();
            if ln.starts_with("### ") {
                let title = normalize(ln.trim_start_matches("### ").trim());
                if title.contains("features")
                    || title.contains("new features")
                    || title.contains("improvements")
                    || title.contains("enhancements")
                {
                    let mut t = k + 1;
                    while let Some(nl) = body.lines().nth(t) {
                        let nlt = nl.trim();
                        if nlt.starts_with("### ") || nlt.starts_with("## ") {
                            break;
                        }
                        if nlt.starts_with("- ") || nlt.starts_with("* ") || nlt.starts_with("• ")
                        {
                            let cleaned = nlt
                                .trim_start_matches("- ")
                                .trim_start_matches("* ")
                                .trim_start_matches("• ")
                                .trim();
                            if let Some(msg) = extract_commit_message(cleaned) {
                                highlights.push(msg);
                                if highlights.len() >= MAX_RELEASE_HIGHLIGHTS {
                                    return highlights;
                                }
                            }
                        }
                        t += 1;
                    }
                }
            }
            k += 1;
        }
    }

    if highlights.is_empty() {
        let mut total_count = 0usize;
        let mut k = 0usize;
        while k < body.lines().count() {
            let ln = body.lines().nth(k).unwrap().trim();
            if ln.starts_with("### ") {
                let mut t = k + 1;
                while let Some(nl) = body.lines().nth(t) {
                    let nlt = nl.trim();
                    if nlt.starts_with("### ") || nlt.starts_with("## ") {
                        break;
                    }
                    if nlt.starts_with("- ") || nlt.starts_with("* ") || nlt.starts_with("• ") {
                        let cleaned = nlt
                            .trim_start_matches("- ")
                            .trim_start_matches("* ")
                            .trim_start_matches("• ")
                            .trim();
                        if let Some(msg) = extract_any_commit_message(cleaned) {
                            total_count += 1;
                            if highlights.len() < MAX_RELEASE_HIGHLIGHTS {
                                highlights.push(msg);
                            }
                        }
                    }
                    t += 1;
                }
            }
            k += 1;
        }
        if total_count > MAX_RELEASE_HIGHLIGHTS {
            highlights.push(format!(
                "... and {} more",
                total_count - MAX_RELEASE_HIGHLIGHTS
            ));
        }
    }

    if highlights.is_empty() {
        highlights = fallback_extract_lines(body);
    }

    highlights
}

fn fallback_extract_lines(body: &str) -> Vec<String> {
    const FALLBACK_LINES: usize = 50;

    let mut lines = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with("**Full Changelog**")
            || line.starts_with("---")
            || line.starts_with("## 📥")
            || line.starts_with("| ")
            || line.starts_with("```")
        {
            continue;
        }

        let cleaned = line
            .trim_start_matches("- ")
            .trim_start_matches("* ")
            .trim_start_matches("• ")
            .trim();

        if !cleaned.is_empty() && cleaned.len() > 5 {
            let display = if let Some(idx) = cleaned.rfind(" (") {
                if cleaned.ends_with(')') && cleaned.len() - idx < 15 {
                    cleaned[..idx].trim().to_string()
                } else {
                    cleaned.to_string()
                }
            } else {
                cleaned.to_string()
            };

            if !display.is_empty() {
                lines.push(display);
                if lines.len() >= FALLBACK_LINES {
                    break;
                }
            }
        }
    }
    lines
}

pub fn extract_commit_message(line: &str) -> Option<String> {
    let cleaned = if let Some(idx) = line.rfind(" (") {
        if line.ends_with(')') {
            line[..idx].trim()
        } else {
            line
        }
    } else {
        line
    };

    let cleaned = cleaned
        .trim_start_matches("feat: ")
        .trim_start_matches("feat(")
        .split(')')
        .next_back()
        .unwrap_or(cleaned)
        .trim_start_matches(": ")
        .trim();

    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

pub fn extract_any_commit_message(line: &str) -> Option<String> {
    let cleaned = if let Some(idx) = line.rfind(" (") {
        if line.ends_with(')') {
            line[..idx].trim()
        } else {
            line
        }
    } else {
        line
    };

    let prefixes = [
        "feat: ",
        "fix: ",
        "chore: ",
        "ci: ",
        "docs: ",
        "refactor: ",
        "test: ",
        "perf: ",
        "style: ",
        "build: ",
    ];

    let mut result = cleaned;
    for prefix in prefixes {
        if let Some(rest) = result.strip_prefix(prefix) {
            result = rest;
            break;
        }
    }

    let scoped_prefixes = [
        "feat(",
        "fix(",
        "chore(",
        "ci(",
        "docs(",
        "refactor(",
        "test(",
        "perf(",
        "style(",
        "build(",
    ];
    for prefix in scoped_prefixes {
        if result.starts_with(prefix) {
            if let Some(idx) = result.find("): ") {
                result = &result[idx + 3..];
            }
            break;
        }
    }

    let result = result.trim();
    if result.is_empty() {
        None
    } else {
        Some(result.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_release_page_server(responses: Vec<(u16, String)>) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request);
                let reason = if status == 200 { "OK" } else { "Error" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        format!("http://{address}/releases")
    }

    fn spawn_truncated_release_page_server() -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 128\r\nConnection: close\r\n\r\nshort",
                )
                .unwrap();
        });
        format!("http://{address}/releases")
    }

    fn release_page_json(releases: &[(&str, bool, bool, &str)]) -> String {
        serde_json::to_string(
            &releases
                .iter()
                .map(|(tag_name, draft, prerelease, body)| {
                    serde_json::json!({
                        "tag_name": tag_name,
                        "body": body,
                        "draft": draft,
                        "prerelease": prerelease,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn release_json(tag_name: &str, draft: bool, prerelease: bool, body: &str) -> String {
        serde_json::json!({
            "tag_name": tag_name,
            "body": body,
            "draft": draft,
            "prerelease": prerelease,
        })
        .to_string()
    }

    #[test]
    fn test_extract_version_from_redirect_url() {
        assert_eq!(
            extract_version_from_redirect_url(
                "https://github.com/bifrost-proxy/bifrost/releases/tag/v0.0.53-beta"
            )
            .unwrap(),
            "0.0.53-beta"
        );
        assert_eq!(
            extract_version_from_redirect_url(
                "https://github.com/bifrost-proxy/bifrost/releases/tag/v1.0.0"
            )
            .unwrap(),
            "1.0.0"
        );
        assert!(extract_version_from_redirect_url(
            "https://github.com/bifrost-proxy/bifrost/releases/tag/moss-runtime-v1.0.0"
        )
        .is_err());
        assert!(extract_version_from_redirect_url("https://github.com/").is_err());
    }

    #[test]
    fn test_bifrost_release_tag_validation() {
        assert_eq!(
            bifrost_version_from_release_tag("v0.0.156"),
            Some("0.0.156".to_string())
        );
        assert_eq!(
            bifrost_version_from_release_tag("v0.0.157-beta.3"),
            Some("0.0.157-beta.3".to_string())
        );
        for tag in [
            "moss-runtime-v1.0.0",
            "vmoss-runtime-v1.0.0",
            "0.0.156",
            "v0.0",
            "v0.0.156.1",
            "v0.0.x",
            "v0.0.156-",
        ] {
            assert_eq!(
                bifrost_version_from_release_tag(tag),
                None,
                "unexpected valid Bifrost release tag: {tag}"
            );
        }
    }

    #[test]
    fn test_pick_latest_bifrost_release_ignores_resource_and_unpublished_channels() {
        let release = |tag_name: &str, draft: bool, prerelease: bool| GitHubRelease {
            tag_name: tag_name.to_string(),
            body: Some(format!("notes for {tag_name}")),
            draft,
            prerelease,
        };
        let picked = pick_latest_bifrost_release(vec![
            release("moss-runtime-v9.0.0", false, false),
            release("v9.0.0", true, false),
            release("v2.0.0-beta.1", false, true),
            release("v0.0.156", false, false),
            release("v1.2.3", false, false),
        ])
        .expect("stable Bifrost release");
        assert_eq!(picked.tag_name, "v1.2.3");
        assert_eq!(
            stable_bifrost_release_version(&picked),
            Some("1.2.3".to_string())
        );
    }

    #[test]
    fn prerelease_channel_tracks_only_its_published_channel_and_orders_numbers() {
        let release = |tag_name: &str, draft: bool, prerelease: bool| GitHubRelease {
            tag_name: tag_name.to_string(),
            body: Some(format!("notes for {tag_name}")),
            draft,
            prerelease,
        };

        assert_eq!(release_channel("0.0.181"), ReleaseChannel::Stable);
        assert_eq!(
            release_channel("v0.0.181-alpha.1"),
            ReleaseChannel::Prerelease("alpha".to_string())
        );
        assert_eq!(
            release_channel("0.0.181-alpha10"),
            ReleaseChannel::Prerelease("alpha".to_string())
        );
        assert_eq!(
            release_channel("0.0.181-10008"),
            ReleaseChannel::Prerelease("alpha".to_string())
        );
        assert_eq!(
            release_channel("0.0.181.10008"),
            ReleaseChannel::Prerelease("alpha".to_string())
        );
        assert_eq!(canonical_release_version("v0.0.181.10000"), "0.0.181-alpha");
        assert_eq!(canonical_release_version("0.0.181.20000"), "0.0.181-beta");
        assert_eq!(canonical_release_version("0.0.181.30001"), "0.0.181-rc.1");
        assert_eq!(canonical_release_version("0.0.181-10000"), "0.0.181-alpha");
        assert_eq!(canonical_release_version("0.0.181-20002"), "0.0.181-beta.2");
        assert_eq!(canonical_release_version("0.0.181-30003"), "0.0.181-rc.3");
        assert_eq!(
            canonical_release_version("0.0.181-alpha10"),
            "0.0.181-alpha.10"
        );
        assert_eq!(
            canonical_release_version("0.0.181-alpha.10"),
            "0.0.181-alpha.10"
        );
        assert_eq!(canonical_release_version("0.0.181-9999"), "0.0.181-9999");
        assert_eq!(canonical_release_version("0.0.181.9999"), "0.0.181.9999");
        assert_eq!(
            release_channel("0.0.181-1"),
            ReleaseChannel::Prerelease("1".to_string())
        );
        assert!(same_release_channel("0.0.181-alpha.1", "0.0.181-alpha.10"));
        assert!(!same_release_channel("0.0.181-alpha.1", "0.0.181-beta.1"));

        let picked = pick_latest_bifrost_release_for_channel(
            vec![
                release("v0.0.181-alpha.9", false, true),
                release("v0.0.181-alpha.10", false, true),
                release("v0.0.181-alpha.99", false, false),
                release("v0.0.181-beta.99", false, true),
                release("v9.0.0", false, true),
                release("v9.0.0", false, false),
                release("v0.0.182-alpha.1", true, true),
            ],
            &ReleaseChannel::Prerelease("alpha".to_string()),
        )
        .expect("latest published alpha release");
        assert_eq!(picked.tag_name, "v0.0.181-alpha.10");
        assert_eq!(
            compare_versions("0.0.181-alpha.10", "0.0.181-alpha.9"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("0.0.181-10008", "0.0.181-alpha.8"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("0.0.181.10009", "0.0.181-alpha.8"),
            std::cmp::Ordering::Greater
        );
        assert!(same_release_channel("0.0.181.10008", "0.0.181-alpha.9"));
        assert!(!same_release_channel("not-a-version", "also-invalid"));
    }

    #[test]
    fn release_channel_normalization_covers_msi_and_legacy_edge_cases() {
        assert_eq!(decode_msi_prerelease(9_999), None);
        assert_eq!(decode_msi_prerelease(40_000), None);
        assert_eq!(
            canonical_release_version(" v0.0.181.10001 "),
            "0.0.181-alpha.1"
        );
        assert_eq!(canonical_release_version("0.0.181.20001"), "0.0.181-beta.1");
        assert_eq!(canonical_release_version("0.0.181.30000"), "0.0.181-rc");
        assert_eq!(
            canonical_release_version("0.0.181.999999999999999999999999999999999"),
            "0.0.181.999999999999999999999999999999999"
        );
        assert_eq!(
            canonical_release_version("0.0.181-alpha10-x"),
            "0.0.181-alpha.10-x"
        );
        assert_eq!(
            canonical_release_version("0.0.181-alpha-x"),
            "0.0.181-alpha-x"
        );
        assert_eq!(canonical_release_version("0.0.181-123x"), "0.0.181-123x");
        assert_eq!(
            release_channel("0.0.181-123x"),
            ReleaseChannel::Prerelease("123x".into())
        );

        assert_eq!(
            compare_versions("1.foo.3-alpha", "1.foo.2-beta"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("1.0.0-invalid_legacy", "1.0.0"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("1.0.0", "1.0.0-invalid_legacy"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("1.0.0-invalid_2", "1.0.0-invalid_10"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn release_picker_covers_stable_prerelease_and_invalid_candidates() {
        let release = |tag_name: &str, draft: bool, prerelease: bool| GitHubRelease {
            tag_name: tag_name.to_string(),
            body: None,
            draft,
            prerelease,
        };
        let stable = ReleaseChannel::Stable;
        let alpha = ReleaseChannel::Prerelease("alpha".to_string());

        assert_eq!(
            release_version_for_channel(&release("v1.2.3", false, false), &stable).as_deref(),
            Some("1.2.3")
        );
        assert!(
            release_version_for_channel(&release("v1.2.3-alpha.1", false, true), &stable).is_none()
        );
        assert_eq!(
            release_version_for_channel(&release("v1.2.3-alpha.1", false, true), &alpha).as_deref(),
            Some("1.2.3-alpha.1")
        );
        assert!(
            release_version_for_channel(&release("v1.2.3-beta.1", false, true), &alpha).is_none()
        );
        assert!(release_version_for_channel(&release("invalid", false, true), &alpha).is_none());
        assert!(
            release_version_for_channel(&release("v1.2.3-alpha.2", true, true), &alpha).is_none()
        );
    }

    #[test]
    fn prerelease_release_scan_returns_latest_alpha_instead_of_stable_latest() {
        let base_url = spawn_release_page_server(vec![
            (
                200,
                release_page_json(&[
                    ("v0.0.180", false, false, "stable"),
                    (
                        "v0.0.181-alpha.8",
                        false,
                        true,
                        "## Highlights\n- alpha eight",
                    ),
                    (
                        "v0.0.181-alpha.9",
                        false,
                        true,
                        "## Highlights\n- alpha nine",
                    ),
                    ("v0.0.181-beta.1", false, true, "beta"),
                ]),
            ),
            (200, "[]".to_string()),
        ]);

        let selected =
            fetch_latest_release_sync_for_current_from("0.0.181-alpha.8", &base_url).unwrap();
        assert_eq!(
            selected,
            (
                "0.0.181-alpha.9".to_string(),
                vec!["alpha nine".to_string()]
            )
        );
    }

    #[tokio::test]
    async fn prerelease_async_release_scan_uses_the_current_alpha_channel() {
        let base_url = spawn_release_page_server(vec![
            (
                200,
                release_page_json(&[
                    ("v0.0.181", false, false, "stable"),
                    (
                        "v0.0.182-alpha.1",
                        false,
                        true,
                        "## Highlights\n- async alpha",
                    ),
                    ("v0.0.183-beta.1", false, true, "beta"),
                ]),
            ),
            (200, "[]".to_string()),
        ]);

        let selected = fetch_latest_release_async_for_current_from("0.0.181-alpha.9", &base_url)
            .await
            .expect("latest async alpha release");
        assert_eq!(
            selected,
            (
                "0.0.182-alpha.1".to_string(),
                vec!["async alpha".to_string()]
            )
        );
    }

    #[test]
    fn releases_html_parser_selects_the_latest_matching_prerelease_channel() {
        let html = r#"
            <a href="/bifrost-proxy/bifrost/releases/tag/v0.0.181-alpha.9">alpha 9</a>
            <a href="/bifrost-proxy/bifrost/releases/tag/v0.0.181-alpha.9">duplicate</a>
            <a href="/bifrost-proxy/bifrost/releases/tag/v0.0.181-alpha.10">alpha 10</a>
            <a href="/bifrost-proxy/bifrost/releases/tag/v0.0.182-beta.1">beta</a>
            <a href="/bifrost-proxy/bifrost/releases/tag/v0.0.182">stable</a>
            <a href="/bifrost-proxy/bifrost/releases/tag/not-a-bifrost-tag">invalid</a>
            <a href="/someone/else/releases/tag/v99.0.0-alpha.1">unrelated</a>
        "#;

        assert_eq!(
            release_versions_from_html(html),
            vec![
                "0.0.181-alpha.9".to_string(),
                "0.0.181-alpha.10".to_string(),
                "0.0.182-beta.1".to_string(),
                "0.0.182".to_string(),
            ]
        );
        assert_eq!(
            pick_latest_release_version_from_html_for_channel(
                html,
                &ReleaseChannel::Prerelease("alpha".to_string()),
            )
            .as_deref(),
            Some("0.0.181-alpha.10")
        );
    }

    #[test]
    fn prerelease_sync_falls_back_to_public_html_when_api_is_rate_limited() {
        let html = r#"
            <a href="/bifrost-proxy/bifrost/releases/tag/v0.0.181-alpha.8">alpha 8</a>
            <a href="/bifrost-proxy/bifrost/releases/tag/v0.0.181-alpha.9">alpha 9</a>
            <a href="/bifrost-proxy/bifrost/releases/tag/v0.0.181-beta.10">beta</a>
        "#;
        let base_url = spawn_release_page_server(vec![
            (403, "rate limited".to_string()),
            (403, "rate limited".to_string()),
            (403, "rate limited".to_string()),
            (200, html.to_string()),
        ]);

        let selected = fetch_latest_release_sync_for_current_from_sources(
            "0.0.181-alpha.8",
            &base_url,
            &base_url,
        )
        .expect("HTML fallback should bypass API rate limiting");
        assert_eq!(selected, ("0.0.181-alpha.9".to_string(), Vec::new()));
    }

    #[test]
    fn releases_html_sync_fallback_paginates_and_reports_terminal_failures() {
        let client = crate::github_blocking_reqwest_client_builder()
            .build()
            .unwrap();
        let beta_only = r#"<a href="/bifrost-proxy/bifrost/releases/tag/v0.0.181-beta.1">beta</a>"#;
        let alpha = r#"<a href="/bifrost-proxy/bifrost/releases/tag/v0.0.181-alpha.11">alpha</a>"#;
        let paginated =
            spawn_release_page_server(vec![(200, beta_only.to_string()), (200, alpha.to_string())]);
        assert_eq!(
            fetch_latest_release_from_html_sync_for_channel(
                &client,
                &format!("{paginated}?tab=all"),
                &ReleaseChannel::Prerelease("alpha".to_string()),
            )
            .unwrap(),
            ("0.0.181-alpha.11".to_string(), Vec::new())
        );

        let empty = spawn_release_page_server(vec![(200, "<html>none</html>".to_string())]);
        assert!(matches!(
            fetch_latest_release_from_html_sync_for_channel(
                &client,
                &empty,
                &ReleaseChannel::Prerelease("alpha".to_string()),
            ),
            Err(FetchError::Parse(message)) if message.contains("channel alpha")
        ));

        let no_stable = spawn_release_page_server(vec![
            (200, beta_only.to_string()),
            (200, "<html>none</html>".to_string()),
        ]);
        assert!(matches!(
            fetch_latest_release_from_html_sync_for_channel(
                &client,
                &no_stable,
                &ReleaseChannel::Stable,
            ),
            Err(FetchError::Parse(message)) if message.contains("channel stable")
        ));

        let failed = spawn_release_page_server(vec![
            (500, "failure one".to_string()),
            (500, "failure two".to_string()),
            (500, "failure three".to_string()),
        ]);
        assert!(matches!(
            fetch_latest_release_from_html_sync_for_channel(
                &client,
                &failed,
                &ReleaseChannel::Prerelease("alpha".to_string()),
            ),
            Err(FetchError::Network(message)) if message.contains("HTML fallback failed")
        ));

        let truncated = spawn_truncated_release_page_server();
        assert!(matches!(
            fetch_latest_release_from_html_sync_for_channel(
                &client,
                &truncated,
                &ReleaseChannel::Prerelease("alpha".to_string()),
            ),
            Err(FetchError::Parse(message)) if message.contains("failed to read GitHub releases HTML page 1")
        ));

        let exhausted = spawn_release_page_server(
            (0..GITHUB_RELEASES_HTML_MAX_PAGES)
                .map(|_| (200, beta_only.to_string()))
                .collect(),
        );
        assert!(matches!(
            fetch_latest_release_from_html_sync_for_channel(
                &client,
                &exhausted,
                &ReleaseChannel::Prerelease("alpha".to_string()),
            ),
            Err(FetchError::Parse(message)) if message.contains("channel alpha")
        ));

        let combined_failure = spawn_release_page_server(vec![
            (403, "rate limited one".to_string()),
            (403, "rate limited two".to_string()),
            (403, "rate limited three".to_string()),
            (500, "html failure one".to_string()),
            (500, "html failure two".to_string()),
            (500, "html failure three".to_string()),
        ]);
        assert!(matches!(
            fetch_latest_release_sync_for_current_from_sources(
                "0.0.181-alpha.10",
                &combined_failure,
                &combined_failure,
            ),
            Err(FetchError::Network(message))
                if message.contains("GitHub API") && message.contains("releases HTML")
        ));
    }

    #[tokio::test]
    async fn prerelease_async_falls_back_to_public_html_when_api_is_rate_limited() {
        let html = r#"
            <a href="/bifrost-proxy/bifrost/releases/tag/v0.0.181-alpha.9">alpha 9</a>
            <a href="/bifrost-proxy/bifrost/releases/tag/v0.0.182-rc.1">rc</a>
        "#;
        let base_url = spawn_release_page_server(vec![
            (403, "rate limited".to_string()),
            (200, html.to_string()),
        ]);

        let selected = fetch_latest_release_async_for_current_from_sources(
            "0.0.181-alpha.8",
            &base_url,
            &base_url,
        )
        .await
        .expect("async HTML fallback should bypass API rate limiting");
        assert_eq!(selected, ("0.0.181-alpha.9".to_string(), Vec::new()));
    }

    #[tokio::test]
    async fn releases_html_async_fallback_covers_pagination_and_error_exits() {
        let client = crate::github_reqwest_client_builder().build().unwrap();
        let beta_only = r#"<a href="/bifrost-proxy/bifrost/releases/tag/v0.0.181-beta.1">beta</a>"#;
        let alpha = r#"<a href="/bifrost-proxy/bifrost/releases/tag/v0.0.181-alpha.12">alpha</a>"#;
        let paginated =
            spawn_release_page_server(vec![(200, beta_only.to_string()), (200, alpha.to_string())]);
        assert_eq!(
            fetch_latest_release_from_html_async_for_channel(
                &client,
                &paginated,
                &ReleaseChannel::Prerelease("alpha".to_string()),
            )
            .await,
            Some(("0.0.181-alpha.12".to_string(), Vec::new()))
        );

        let failed = spawn_release_page_server(vec![(500, "failure".to_string())]);
        assert!(fetch_latest_release_from_html_async_for_channel(
            &client,
            &failed,
            &ReleaseChannel::Prerelease("alpha".to_string()),
        )
        .await
        .is_none());

        let empty = spawn_release_page_server(vec![(200, "<html>none</html>".to_string())]);
        assert!(fetch_latest_release_from_html_async_for_channel(
            &client,
            &empty,
            &ReleaseChannel::Prerelease("alpha".to_string()),
        )
        .await
        .is_none());

        let exhausted = spawn_release_page_server(
            (0..GITHUB_RELEASES_HTML_MAX_PAGES)
                .map(|_| (200, beta_only.to_string()))
                .collect(),
        );
        assert!(fetch_latest_release_from_html_async_for_channel(
            &client,
            &exhausted,
            &ReleaseChannel::Prerelease("alpha".to_string()),
        )
        .await
        .is_none());
    }

    #[test]
    fn test_github_releases_api_list_url_is_explicitly_paginated_without_a_fixed_cap() {
        assert_eq!(
            github_releases_api_list_url(0),
            "https://api.github.com/repos/bifrost-proxy/bifrost/releases?per_page=100&page=1"
        );
        assert_eq!(
            github_releases_api_list_url(1),
            "https://api.github.com/repos/bifrost-proxy/bifrost/releases?per_page=100&page=1"
        );
        assert_eq!(
            github_releases_api_list_url(10_001),
            "https://api.github.com/repos/bifrost-proxy/bifrost/releases?per_page=100&page=10001"
        );
    }

    #[test]
    fn published_release_sync_fallback_scans_all_pages_and_keeps_earlier_candidates() {
        let client = crate::github_blocking_reqwest_client_builder()
            .build()
            .unwrap();
        let base_url = spawn_release_page_server(vec![
            (
                200,
                release_page_json(&[
                    ("moss-runtime-v9.0.0", false, false, "resource"),
                    ("v1.2.3", false, false, "## Highlights\n- first"),
                ]),
            ),
            (
                200,
                release_page_json(&[("v2.0.0", false, false, "## Highlights\n- second")]),
            ),
            (200, "[]".to_string()),
        ]);
        let selected = fetch_latest_published_release_sync(&client, &base_url).unwrap();
        assert_eq!(selected, ("2.0.0".to_string(), vec!["second".to_string()]));

        let late_parse_failure = spawn_release_page_server(vec![
            (
                200,
                release_page_json(&[("v1.2.3", false, false, "## Highlights\n- kept")]),
            ),
            (200, "not-json".to_string()),
        ]);
        let selected = fetch_latest_published_release_sync(&client, &late_parse_failure).unwrap();
        assert_eq!(selected, ("1.2.3".to_string(), vec!["kept".to_string()]));

        let first_parse_failure = spawn_release_page_server(vec![(200, "not-json".to_string())]);
        assert!(matches!(
            fetch_latest_published_release_sync(&client, &first_parse_failure),
            Err(FetchError::Parse(message)) if message.contains("page 1")
        ));

        let empty = spawn_release_page_server(vec![(200, "[]".to_string())]);
        assert!(matches!(
            fetch_latest_published_release_sync(&client, &empty),
            Err(FetchError::Parse(message)) if message.contains("no published stable")
        ));

        let empty_alpha = spawn_release_page_server(vec![(200, "[]".to_string())]);
        assert!(matches!(
            fetch_latest_published_release_sync_for_channel(
                &client,
                &empty_alpha,
                &ReleaseChannel::Prerelease("alpha".to_string()),
            ),
            Err(FetchError::Parse(message)) if message.contains("prerelease channel alpha")
        ));
    }

    #[test]
    fn latest_release_sync_api_path_accepts_stable_and_falls_back_from_invalid_latest() {
        let client = crate::github_blocking_reqwest_client_builder()
            .build()
            .unwrap();

        let stable = spawn_release_page_server(vec![(
            200,
            release_json("v3.2.1", false, false, "## Highlights\n- stable latest"),
        )]);
        assert_eq!(
            fetch_latest_release_from_api_sync(&client, (&stable, &stable)).unwrap(),
            ("3.2.1".to_string(), vec!["stable latest".to_string()])
        );

        for latest_body in [
            release_json("moss-runtime-v9.0.0", false, false, "resource"),
            "not-json".to_string(),
        ] {
            let fallback = spawn_release_page_server(vec![
                (200, latest_body),
                (
                    200,
                    release_page_json(&[("v2.4.0", false, false, "## Highlights\n- fallback")]),
                ),
                (200, "[]".to_string()),
            ]);
            assert_eq!(
                fetch_latest_release_from_api_sync(&client, (&fallback, &fallback)).unwrap(),
                ("2.4.0".to_string(), vec!["fallback".to_string()])
            );
        }

        let failed_latest = spawn_release_page_server(vec![
            (500, "failure 1".to_string()),
            (500, "failure 2".to_string()),
            (500, "failure 3".to_string()),
            (
                200,
                release_page_json(&[("v2.5.0", false, false, "fallback after error")]),
            ),
            (200, "[]".to_string()),
        ]);
        assert_eq!(
            fetch_latest_release_from_api_sync(&client, (&failed_latest, &failed_latest))
                .unwrap()
                .0,
            "2.5.0"
        );
    }

    #[tokio::test]
    async fn published_release_async_fallback_scans_pages_and_tolerates_late_failures() {
        let client = crate::github_reqwest_client_builder().build().unwrap();
        let base_url = spawn_release_page_server(vec![
            (
                200,
                release_page_json(&[("v1.2.3", false, false, "## Highlights\n- first")]),
            ),
            (
                200,
                release_page_json(&[("v2.0.0", false, false, "## Highlights\n- second")]),
            ),
            (200, "[]".to_string()),
        ]);
        let selected = fetch_latest_published_release_async(&client, &base_url)
            .await
            .unwrap();
        assert_eq!(selected, ("2.0.0".to_string(), vec!["second".to_string()]));

        let late_status_failure = spawn_release_page_server(vec![
            (
                200,
                release_page_json(&[("v1.2.3", false, false, "## Highlights\n- kept")]),
            ),
            (500, "server error".to_string()),
        ]);
        let selected = fetch_latest_published_release_async(&client, &late_status_failure)
            .await
            .unwrap();
        assert_eq!(selected, ("1.2.3".to_string(), vec!["kept".to_string()]));

        let late_parse_failure = spawn_release_page_server(vec![
            (
                200,
                release_page_json(&[("v1.2.3", false, false, "## Highlights\n- kept")]),
            ),
            (200, "not-json".to_string()),
        ]);
        assert_eq!(
            fetch_latest_published_release_async(&client, &late_parse_failure)
                .await
                .unwrap()
                .0,
            "1.2.3"
        );

        let late_transport_failure = spawn_release_page_server(vec![(
            200,
            release_page_json(&[("v1.2.3", false, false, "kept")]),
        )]);
        assert_eq!(
            fetch_latest_published_release_async(&client, &late_transport_failure)
                .await
                .unwrap()
                .0,
            "1.2.3"
        );

        let first_status_failure =
            spawn_release_page_server(vec![(500, "server error".to_string())]);
        assert!(
            fetch_latest_published_release_async(&client, &first_status_failure)
                .await
                .is_none()
        );
        assert!(
            fetch_latest_published_release_async(&client, "http://127.0.0.1:1/releases")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn latest_release_async_api_path_accepts_stable_and_falls_back_from_invalid_latest() {
        let client = crate::github_reqwest_client_builder().build().unwrap();

        let stable = spawn_release_page_server(vec![(
            200,
            release_json("v3.2.1", false, false, "## Highlights\n- stable latest"),
        )]);
        assert_eq!(
            fetch_latest_release_from_api_async(&client, (&stable, &stable)).await,
            Some(("3.2.1".to_string(), vec!["stable latest".to_string()]))
        );

        for latest_body in [
            release_json("moss-runtime-v9.0.0", false, false, "resource"),
            "not-json".to_string(),
        ] {
            let fallback = spawn_release_page_server(vec![
                (200, latest_body),
                (
                    200,
                    release_page_json(&[("v2.4.0", false, false, "## Highlights\n- fallback")]),
                ),
                (200, "[]".to_string()),
            ]);
            assert_eq!(
                fetch_latest_release_from_api_async(&client, (&fallback, &fallback)).await,
                Some(("2.4.0".to_string(), vec!["fallback".to_string()]))
            );
        }

        let fallback = spawn_release_page_server(vec![
            (
                200,
                release_page_json(&[("v2.5.0", false, false, "transport fallback")]),
            ),
            (200, "[]".to_string()),
        ]);
        assert_eq!(
            fetch_latest_release_from_api_async(
                &client,
                ("http://127.0.0.1:1/releases/latest", &fallback),
            )
            .await
            .unwrap()
            .0,
            "2.5.0"
        );
    }

    #[test]
    fn published_release_request_errors_preserve_rate_limit_and_http_diagnostics() {
        assert!(matches!(
            published_release_request_error(&GithubRequestError::Status(
                reqwest::StatusCode::FORBIDDEN
            )),
            FetchError::Network(message) if message.contains("rate limited")
        ));
        assert!(matches!(
            published_release_request_error(&GithubRequestError::Status(
                reqwest::StatusCode::NOT_FOUND
            )),
            FetchError::Network(message) if message.contains("endpoint not found")
        ));
    }

    #[test]
    fn test_make_release_tag() {
        assert_eq!(make_release_tag("0.0.53-beta"), "v0.0.53-beta");
        assert_eq!(make_release_tag("1.0.0"), "v1.0.0");
    }

    #[test]
    fn test_compare_versions_basic() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("1.0.0", "0.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("0.9.9", "1.0.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
    }

    #[test]
    fn test_compare_versions_minor() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("0.2.0", "0.1.0"), Ordering::Greater);
        assert_eq!(compare_versions("0.1.0", "0.2.0"), Ordering::Less);
        assert_eq!(compare_versions("0.1.5", "0.1.5"), Ordering::Equal);
    }

    #[test]
    fn test_compare_versions_patch() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("0.0.2", "0.0.1"), Ordering::Greater);
        assert_eq!(compare_versions("0.0.1", "0.0.2"), Ordering::Less);
    }

    #[test]
    fn test_compare_versions_prerelease() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("1.0.0", "1.0.0-alpha"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0-alpha", "1.0.0"), Ordering::Less);
        assert_eq!(
            compare_versions("1.0.0-alpha", "1.0.0-alpha"),
            Ordering::Equal
        );
        assert_eq!(
            compare_versions("1.0.0-beta", "1.0.0-alpha"),
            Ordering::Greater
        );
    }

    #[test]
    fn test_is_newer_version() {
        assert!(is_newer_version("0.0.1", "1.0.0"));
        assert!(is_newer_version("0.0.1-alpha", "0.0.1"));
        assert!(is_newer_version("0.0.1-alpha", "0.0.2-alpha"));
        assert!(is_newer_version("0.0.1-alpha", "1.0.0"));
        assert!(!is_newer_version("1.0.0", "0.0.1"));
        assert!(!is_newer_version("1.0.0", "1.0.0"));
        assert!(!is_newer_version("0.0.1", "0.0.1-alpha"));
    }

    #[test]
    fn test_parse_release_highlights_from_highlights_section() {
        let body = r#"## ✨ Highlights

- Added new feature A
- Improved performance by 50%
- Fixed critical bug

## What's Changed

### 🚀 Features
- feat: something else
"#;
        let highlights = parse_release_highlights(Some(body));
        assert_eq!(highlights.len(), 3);
        assert_eq!(highlights[0], "Added new feature A");
        assert_eq!(highlights[1], "Improved performance by 50%");
        assert_eq!(highlights[2], "Fixed critical bug");
    }

    #[test]
    fn test_parse_release_highlights_from_features_section() {
        let body = r#"## What's Changed

### 🚀 Features
- feat: add proxy support (abc123)
- feat(cli): improve startup time (def456)
- feat: enable caching (ghi789)

### 🐛 Bug Fixes
- fix: resolve memory leak
"#;
        let highlights = parse_release_highlights(Some(body));
        assert_eq!(highlights.len(), 3);
        assert_eq!(highlights[0], "add proxy support");
        assert_eq!(highlights[1], "improve startup time");
        assert_eq!(highlights[2], "enable caching");
    }

    #[test]
    fn test_parse_release_highlights_empty() {
        assert!(parse_release_highlights(None).is_empty());
        assert!(parse_release_highlights(Some("")).is_empty());
        assert!(parse_release_highlights(Some("   ")).is_empty());
    }

    #[test]
    fn test_parse_release_highlights_fallback() {
        let body = r#"Some random release notes
without proper structure

- First change item (abc123)
- Second change item (def456)
- Third change here
- Fourth one
- Fifth item too
- Sixth one here

**Full Changelog**: https://example.com
"#;
        let highlights = parse_release_highlights(Some(body));
        assert_eq!(highlights.len(), 8);
        assert_eq!(highlights[0], "Some random release notes");
        assert_eq!(highlights[1], "without proper structure");
        assert_eq!(highlights[2], "First change item");
        assert_eq!(highlights[3], "Second change item");
        assert_eq!(highlights[4], "Third change here");
        assert_eq!(highlights[5], "Fourth one");
        assert_eq!(highlights[6], "Fifth item too");
        assert_eq!(highlights[7], "Sixth one here");
    }

    #[test]
    fn test_parse_release_highlights_plain_highlights() {
        let body = r#"## Highlights

- New rule engine
- Faster startup
- Better logs

## What's Changed
- other stuff
"#;
        let highlights = parse_release_highlights(Some(body));
        assert_eq!(highlights.len(), 3);
        assert_eq!(highlights[0], "New rule engine");
        assert_eq!(highlights[1], "Faster startup");
        assert_eq!(highlights[2], "Better logs");
    }

    #[test]
    fn test_parse_release_highlights_whats_new_curly_apostrophe() {
        let body = "## What\u{2019}s New\n\n- A\n- B\n- C\n";
        let highlights = parse_release_highlights(Some(body));
        assert_eq!(highlights.len(), 3);
        assert_eq!(highlights[0], "A");
        assert_eq!(highlights[1], "B");
        assert_eq!(highlights[2], "C");
    }

    #[test]
    fn test_parse_release_highlights_features_no_emoji() {
        let body = r#"## What's Changed

### Features
- feat: alpha (x1)
- feat(cli): bravo (x2)
- feat: charlie

### Bug Fixes
- nope
"#;
        let highlights = parse_release_highlights(Some(body));
        assert_eq!(highlights.len(), 3);
        assert_eq!(highlights[0], "alpha");
        assert_eq!(highlights[1], "bravo");
        assert_eq!(highlights[2], "charlie");
    }

    #[test]
    fn test_extract_commit_message() {
        assert_eq!(
            extract_commit_message("feat: add new feature (abc123)"),
            Some("add new feature".to_string())
        );
        assert_eq!(
            extract_commit_message("feat(scope): do something (xyz)"),
            Some("do something".to_string())
        );
        assert_eq!(
            extract_commit_message("simple message"),
            Some("simple message".to_string())
        );
    }

    #[test]
    fn test_extract_any_commit_message() {
        assert_eq!(
            extract_any_commit_message("fix(tls): 更新依赖版本并重构证书生成逻辑 (544f003)"),
            Some("更新依赖版本并重构证书生成逻辑".to_string())
        );
        assert_eq!(
            extract_any_commit_message("chore: bump version to 0.0.4-alpha (7c12d34)"),
            Some("bump version to 0.0.4-alpha".to_string())
        );
        assert_eq!(
            extract_any_commit_message("ci(workflow): 改进 Homebrew 公式更新流程 (e2c148a)"),
            Some("改进 Homebrew 公式更新流程".to_string())
        );
        assert_eq!(
            extract_any_commit_message("simple message"),
            Some("simple message".to_string())
        );
    }

    #[test]
    fn test_parse_release_highlights_bugfixes_only() {
        let body = "## What's Changed\n\n### 🐛 Bug Fixes\n- fix(tls): 更新依赖版本并重构证书生成逻辑 (544f003)\n\n### 📝 Other Changes\n- chore: bump version to 0.0.4-alpha (7c12d34)\n- ci(workflow): 改进 Homebrew 公式更新流程 (e2c148a)\n- ci(workflows): 添加对 Windows ARM64 架构的支持 (abe47fa)\n\n**Full Changelog**: https://github.com/bifrost-proxy/bifrost/compare/v0.0.3-alpha...v0.0.4-alpha\n";
        let highlights = parse_release_highlights(Some(body));
        assert_eq!(
            highlights.len(),
            4,
            "Should extract all 4 items from bug fixes and other changes sections"
        );
        assert!(highlights[0].contains("更新依赖版本"));
        assert!(highlights[1].contains("bump version"));
        assert!(highlights[2].contains("改进 Homebrew"));
        assert!(highlights[3].contains("Windows ARM64"));
    }

    #[test]
    fn test_parse_release_highlights_truncation() {
        let mut lines = Vec::new();
        for i in 1..=55 {
            lines.push(format!("- fix: item {} (abc)", i));
        }
        let body = format!(
            "## What's Changed\n\n### 🐛 Bug Fixes\n{}\n",
            lines.join("\n")
        );
        let highlights = parse_release_highlights(Some(&body));
        assert_eq!(
            highlights.len(),
            51,
            "Should show top 50 + '... and N more'"
        );
        assert!(highlights[50].contains("... and 5 more"));
    }

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(strip_html_tags("<b>bold</b>"), "bold");
        assert_eq!(strip_html_tags("no tags"), "no tags");
        assert_eq!(
            strip_html_tags(r#"<a href="url">link text</a>"#),
            "link text"
        );
        assert_eq!(strip_html_tags("<li>item</li>"), "item");
        assert_eq!(strip_html_tags(""), "");
    }

    #[test]
    fn test_find_closing_div() {
        assert_eq!(find_closing_div("hello</div>"), Some(5));
        assert_eq!(find_closing_div("<div>inner</div></div>"), Some(16));
        assert_eq!(find_closing_div("<div>no close"), None);
    }

    #[test]
    fn parse_release_highlights_caps_highlights_section_at_max() {
        // Build a "## Highlights" section with > MAX_RELEASE_HIGHLIGHTS bullets to
        // hit the early-return cap inside the highlights branch.
        let mut body = String::from("## Highlights\n");
        for i in 0..60 {
            body.push_str(&format!("- item {i}\n"));
        }
        let out = parse_release_highlights(Some(&body));
        assert_eq!(out.len(), 50);
        assert_eq!(out[0], "item 0");
    }

    #[test]
    fn parse_release_highlights_caps_features_section_at_max() {
        // No highlights section, but a "### Features" section with > MAX bullets
        // each carrying a commit message, to hit the features-branch cap.
        let mut body = String::from("### Features\n");
        for i in 0..60 {
            body.push_str(&format!("- feat: add thing {i}\n"));
        }
        let out = parse_release_highlights(Some(&body));
        assert_eq!(out.len(), 50);
    }

    #[test]
    fn test_extract_highlights_from_html_real_structure() {
        let html = r#"<div data-pjax="true" data-test-selector="body-content" class="markdown-body"><h2>What's Changed</h2>
<h3>📝 Other Changes</h3>
<ul>
<li>chore: bump version to 0.0.53-beta (<a href="https://github.com/example"><tt>afbfdf8</tt></a>)</li>
<li>perf: 优化数据库缓存大小和内存使用 (<a href="https://github.com/example"><tt>bedd423</tt></a>)</li>
</ul>
<p><strong>Full Changelog</strong>: <a href="https://example.com"><tt>v0.0.52-beta...v0.0.53-beta</tt></a></p></div>"#;
        let highlights = extract_highlights_from_html(html);
        assert!(
            !highlights.is_empty(),
            "should extract highlights from real GitHub HTML"
        );
        assert!(highlights.iter().any(|h| h.contains("bump version")));
        assert!(highlights.iter().any(|h| h.contains("优化数据库缓存")));
    }

    #[test]
    fn test_extract_highlights_from_html_with_features() {
        let html = r#"<div data-test-selector="body-content" class="markdown-body"><h2>What's Changed</h2>
<h3>🚀 Features</h3>
<ul>
<li>feat: add proxy support (<a><tt>abc123</tt></a>)</li>
<li>feat(cli): improve startup time (<a><tt>def456</tt></a>)</li>
</ul>
<h3>🐛 Bug Fixes</h3>
<ul>
<li>fix: resolve memory leak (<a><tt>ghi789</tt></a>)</li>
</ul></div>"#;
        let highlights = extract_highlights_from_html(html);
        assert!(
            !highlights.is_empty(),
            "should extract highlights from features section"
        );
    }

    #[test]
    fn test_extract_highlights_from_html_no_body_content() {
        let html = r#"<div class="other">no release body here</div>"#;
        let highlights = extract_highlights_from_html(html);
        assert!(highlights.is_empty());
    }

    #[test]
    fn test_extract_highlights_from_html_empty_list() {
        let html = r#"<div data-test-selector="body-content" class="markdown-body"><p>No list items</p></div>"#;
        let highlights = extract_highlights_from_html(html);
        assert!(highlights.is_empty());
    }

    #[test]
    fn test_release_page_url() {
        assert_eq!(
            release_page_url("0.0.53-beta"),
            "https://github.com/bifrost-proxy/bifrost/releases/tag/v0.0.53-beta"
        );
        assert_eq!(
            release_page_url("1.0.0"),
            "https://github.com/bifrost-proxy/bifrost/releases/tag/v1.0.0"
        );
    }

    #[test]
    fn fetch_error_display() {
        assert_eq!(
            format!("{}", FetchError::Network("boom".to_string())),
            "boom"
        );
        assert_eq!(format!("{}", FetchError::Parse("bad".to_string())), "bad");
    }

    #[test]
    fn classify_ureq_error_transport_invalid_url_and_scheme() {
        let agent = crate::http_client::direct_ureq_agent();
        // Unknown scheme → UnknownScheme/InvalidUrl transport error.
        let err = agent.get("ftp://example.invalid/x").call().unwrap_err();
        let reason = classify_ureq_error(&err);
        assert_eq!(reason, "invalid URL");

        // Malformed URL → InvalidUrl transport error.
        let err = agent.get("http://").call().unwrap_err();
        let reason = classify_ureq_error(&err);
        // Either invalid URL or a network error depending on parsing; both are
        // non-empty static strings from the transport arm.
        assert!(!reason.is_empty());
    }

    #[test]
    fn classify_ureq_error_dns_failure() {
        let agent = crate::http_client::direct_ureq_agent_builder()
            .timeout(std::time::Duration::from_secs(2))
            .build();
        // A host that cannot resolve exercises the DNS / connection-failed arm.
        let err = agent
            .get("http://nonexistent.invalid.localdomain.example/x")
            .call()
            .unwrap_err();
        let reason = classify_ureq_error(&err);
        assert!(!reason.is_empty());
    }

    #[test]
    fn extract_version_from_redirect_url_edge_cases() {
        // Trailing slash trimmed.
        assert_eq!(
            extract_version_from_redirect_url(
                "https://github.com/bifrost-proxy/bifrost/releases/tag/v2.3.4/"
            )
            .unwrap(),
            "2.3.4"
        );
        // Bifrost releases always require the v-prefix.
        assert!(extract_version_from_redirect_url(
            "https://github.com/bifrost-proxy/bifrost/releases/tag/2.3.4"
        )
        .is_err());
        // Independent resource releases must never become upgrade targets.
        assert!(extract_version_from_redirect_url(
            "https://github.com/bifrost-proxy/bifrost/releases/tag/moss-runtime-v1.0.0"
        )
        .is_err());
        // Empty tag -> Parse error.
        assert!(extract_version_from_redirect_url(
            "https://github.com/bifrost-proxy/bifrost/releases/tag/"
        )
        .is_err());
        // No /tag/ segment -> Parse error.
        assert!(extract_version_from_redirect_url("https://example.com/foo").is_err());
    }

    #[test]
    fn make_release_tag_branches() {
        // Starts with digit -> prefixed.
        assert_eq!(make_release_tag("2.0.0"), "v2.0.0");
        // Contains '-' -> prefixed.
        assert_eq!(make_release_tag("nightly-2020"), "vnightly-2020");
        // Non-digit, no dash -> left as-is.
        assert_eq!(make_release_tag("stable"), "stable");
        // Empty string -> next() is None -> is_none_or -> prefixed.
        assert_eq!(make_release_tag(""), "v");
    }

    #[test]
    fn release_api_url_and_strip_prefix() {
        assert_eq!(
            release_api_url_for_tag("v1.2.3"),
            "https://api.github.com/repos/bifrost-proxy/bifrost/releases/tags/v1.2.3"
        );
        assert_eq!(strip_tag_prefix("v1.2.3"), "1.2.3");
        assert_eq!(strip_tag_prefix("1.2.3"), "1.2.3");
    }

    #[test]
    fn pick_latest_tag_variants() {
        let tags = vec![
            GitHubTag {
                name: "v0.1.0".to_string(),
            },
            GitHubTag {
                name: "v0.2.0".to_string(),
            },
            GitHubTag {
                name: "v0.1.5".to_string(),
            },
            GitHubTag {
                name: "not-a-version".to_string(), // no 'v' prefix -> filtered out
            },
            GitHubTag {
                name: "vmoss-runtime-v9.0.0".to_string(),
            },
        ];
        assert_eq!(pick_latest_tag(tags), Some("0.2.0".to_string()));

        // No valid v-prefixed tags -> None.
        let none_tags = vec![GitHubTag {
            name: "main".to_string(),
        }];
        assert_eq!(pick_latest_tag(none_tags), None);

        // Empty input -> None.
        assert_eq!(pick_latest_tag(vec![]), None);
    }

    #[test]
    fn compare_versions_handles_missing_and_nonnumeric_parts() {
        use std::cmp::Ordering;
        // Missing patch defaults to 0.
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
        // Single number.
        assert_eq!(compare_versions("2", "1.9.9"), Ordering::Greater);
        // Non-numeric component is filtered out, shifting later numbers left:
        // "1.x.3" -> [1, 3] -> (1, 3, 0) which is > (1, 0, 3).
        assert_eq!(compare_versions("1.x.3", "1.0.3"), Ordering::Greater);
    }

    #[test]
    fn fallback_extract_lines_skips_noise() {
        let body = "\
# Title heading
**Full Changelog**: https://x
---
| table | row |
```code```
- valid item one
- short
- another good entry here (abc1234)
";
        let lines = fallback_extract_lines(body);
        // "short" (len<=5) skipped; heading/changelog/table/code/--- skipped.
        assert!(lines.iter().any(|l| l == "valid item one"));
        assert!(lines.iter().any(|l| l == "another good entry here"));
        assert!(!lines.iter().any(|l| l == "short"));
    }

    #[test]
    fn extract_commit_message_strips_trailing_hash() {
        // The trailing " (abc)" is removed; remaining "feat:" is left intact
        // because the "feat: " prefix (with trailing space) no longer matches.
        assert_eq!(
            extract_commit_message("feat:  (abc)"),
            Some("feat:".to_string())
        );
        // A scoped feat with content extracts the message after ")".
        assert_eq!(
            extract_commit_message("feat(scope): real msg (deadbee)"),
            Some("real msg".to_string())
        );
    }

    #[test]
    fn extract_any_commit_message_scoped_and_plain() {
        assert_eq!(
            extract_any_commit_message("refactor(core): tidy up (deadbee)"),
            Some("tidy up".to_string())
        );
        assert_eq!(
            extract_any_commit_message("perf: speed (1234567)"),
            Some("speed".to_string())
        );
        // Empty result -> None.
        assert_eq!(extract_any_commit_message("fix: "), None);
    }

    #[test]
    fn version_cache_serde_roundtrip() {
        let cache = VersionCache {
            latest_version: "1.2.3".to_string(),
            release_highlights: vec!["a".to_string(), "b".to_string()],
            checked_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&cache).unwrap();
        let back: VersionCache = serde_json::from_str(&json).unwrap();
        assert_eq!(back.latest_version, "1.2.3");
        assert_eq!(back.release_highlights.len(), 2);
    }
}
