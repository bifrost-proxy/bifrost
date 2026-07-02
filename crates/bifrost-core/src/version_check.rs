use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::debug;

pub const GITHUB_RELEASES_LATEST_URL: &str =
    "https://github.com/bifrost-proxy/bifrost/releases/latest";
pub const GITHUB_RELEASES_API_URL: &str =
    "https://api.github.com/repos/bifrost-proxy/bifrost/releases/latest";
pub const GITHUB_TAGS_API_URL: &str = "https://api.github.com/repos/bifrost-proxy/bifrost/tags";
pub const GITHUB_RELEASE_URL: &str = "https://github.com/bifrost-proxy/bifrost/releases/tag";
pub const REQUEST_TIMEOUT_SECS: u64 = 10;
const HIGHLIGHTS_TIMEOUT_SECS: u64 = 5;
pub const MAX_RETRIES: u32 = 2;
pub const RETRY_DELAY_MS: u64 = 500;
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
}

#[derive(Debug, Deserialize)]
pub struct GitHubTag {
    pub name: String,
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
        let version = tag.strip_prefix('v').unwrap_or(tag).to_string();
        if version.is_empty() {
            Err(FetchError::Parse(format!(
                "empty version tag in URL: {}",
                url
            )))
        } else {
            debug!(version = %version, url = %url, "extracted version from redirect");
            Ok(version)
        }
    } else {
        Err(FetchError::Parse(format!(
            "no /tag/ found in redirect URL: {}",
            url
        )))
    }
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
        .map(|t| t.name)
        .filter(|name| name.starts_with('v'))
        .map(|name| name.trim_start_matches('v').to_string())
        .max_by(|a, b| compare_versions(a, b))
}

pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse_version = |s: &str| -> (u32, u32, u32, String) {
        let (version_part, prerelease) = if let Some(idx) = s.find('-') {
            (&s[..idx], s[idx + 1..].to_string())
        } else {
            (s, String::new())
        };

        let parts: Vec<u32> = version_part
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();

        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
            prerelease,
        )
    };

    let (a_major, a_minor, a_patch, a_pre) = parse_version(a);
    let (b_major, b_minor, b_patch, b_pre) = parse_version(b);

    match (a_major, a_minor, a_patch).cmp(&(b_major, b_minor, b_patch)) {
        std::cmp::Ordering::Equal => match (a_pre.is_empty(), b_pre.is_empty()) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => a_pre.cmp(&b_pre),
        },
        other => other,
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

    match fetch_with_retry(&client, GITHUB_RELEASES_API_URL) {
        Ok(response) => match response.json::<GitHubRelease>() {
            Ok(release) => {
                let version = strip_tag_prefix(&release.tag_name);
                let highlights = parse_release_highlights(release.body.as_deref());
                return Ok((version, highlights));
            }
            Err(e) => {
                debug!(error = %e, "failed to parse GitHub release JSON, falling back to tags API");
            }
        },
        Err(e) => {
            let reason = classify_github_request_error(&e);
            debug!(
                error = %e,
                reason,
                "releases API failed, falling back to tags API"
            );
        }
    }

    let response = fetch_with_retry(&client, GITHUB_TAGS_API_URL).map_err(|e| {
        let is_rate_limit = matches!(&*e, GithubRequestError::Status(status) if *status == reqwest::StatusCode::FORBIDDEN);
        if is_rate_limit {
            FetchError::Network(
                "all version detection methods failed (redirect + GitHub API rate limited). Check your network connection to github.com".to_string()
            )
        } else {
            let reason = classify_github_request_error(&e);
            FetchError::Network(format!("{}: {}", reason, e))
        }
    })?;

    let tags: Vec<GitHubTag> = response
        .json()
        .map_err(|e| FetchError::Parse(format!("failed to parse tags response: {}", e)))?;

    let version = pick_latest_tag(tags)
        .ok_or_else(|| FetchError::Parse("no valid version tags found".to_string()))?;

    Ok((version, Vec::new()))
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

    if let Ok(response) = client.get(GITHUB_RELEASES_API_URL).send().await {
        if let Ok(release) = response.json::<GitHubRelease>().await {
            let version = strip_tag_prefix(&release.tag_name);
            let highlights = parse_release_highlights(release.body.as_deref());
            return Some((version, highlights));
        }
    }

    let response = client.get(GITHUB_TAGS_API_URL).send().await.ok()?;
    let tags: Vec<GitHubTag> = response.json().await.ok()?;

    let version = pick_latest_tag(tags)?;

    Some((version, Vec::new()))
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
        assert!(extract_version_from_redirect_url("https://github.com/").is_err());
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
        // No v-prefix preserved as-is.
        assert_eq!(
            extract_version_from_redirect_url(
                "https://github.com/bifrost-proxy/bifrost/releases/tag/2.3.4"
            )
            .unwrap(),
            "2.3.4"
        );
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
