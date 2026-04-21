use bifrost_core::version_check::{self, FetchError, VersionCache, GITHUB_RELEASE_URL};
use bifrost_storage::data_dir;
use chrono::Duration;
use chrono::Utc;
use colored::Colorize;
use std::fs;
use std::path::PathBuf;
use std::thread;
use tracing::debug;

const CACHE_FILE_NAME: &str = "version_cache.json";
const CACHE_DURATION_HOURS: i64 = 24;

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

fn fetch_latest_release() -> Option<(String, Vec<String>)> {
    match version_check::fetch_latest_release_sync() {
        Ok(result) => Some(result),
        Err(e) => {
            debug!(error = %e, "fetch_latest_release failed");
            None
        }
    }
}

fn is_ci_environment() -> bool {
    if std::env::var_os("BIFROST_FORCE_UPDATE_CHECK").is_some() {
        return false;
    }
    std::env::var_os("CI").is_some()
        || std::env::var_os("GITHUB_ACTIONS").is_some()
        || std::env::var_os("JENKINS_URL").is_some()
        || std::env::var_os("GITLAB_CI").is_some()
        || std::env::var_os("TRAVIS").is_some()
        || std::env::var_os("CIRCLECI").is_some()
}

pub fn get_latest_version() -> Option<VersionCache> {
    if let Some(cache) = read_cache() {
        if is_cache_valid(&cache) {
            return Some(cache);
        }
    }

    if let Some((latest, highlights)) = fetch_latest_release() {
        let cache = VersionCache {
            latest_version: latest,
            release_highlights: highlights,
            checked_at: Utc::now(),
        };
        write_cache(&cache);
        return Some(cache);
    }

    if let Some(stale_cache) = read_cache() {
        debug!(
            version = %stale_cache.latest_version,
            checked_at = %stale_cache.checked_at,
            "using stale cache as network fallback"
        );
        return Some(stale_cache);
    }

    None
}

pub fn get_latest_version_fresh_with_diagnostics() -> Result<VersionCache, String> {
    match version_check::fetch_latest_release_sync() {
        Ok((latest, highlights)) => {
            let cache = VersionCache {
                latest_version: latest,
                release_highlights: highlights,
                checked_at: Utc::now(),
            };
            write_cache(&cache);
            Ok(cache)
        }
        Err(FetchError::Network(msg)) => Err(msg),
        Err(FetchError::Parse(msg)) => Err(msg),
    }
}

pub fn check_and_print_update_notice() {
    if is_ci_environment() {
        return;
    }

    let current_version = env!("CARGO_PKG_VERSION");

    let cache = match get_latest_version() {
        Some(c) => c,
        None => return,
    };

    if !version_check::is_newer_version(current_version, &cache.latest_version) {
        return;
    }

    print_update_notice(current_version, &cache);
}

pub fn spawn_update_check_notice() {
    if is_ci_environment() {
        return;
    }
    let _ = thread::Builder::new()
        .name("bifrost-update-check".to_string())
        .spawn(check_and_print_update_notice);
}

fn print_update_notice(current_version: &str, cache: &VersionCache) {
    let separator = "─".repeat(64);
    let release_url = format!("{}/v{}", GITHUB_RELEASE_URL, cache.latest_version);

    println!();
    println!("{}", separator.bright_yellow());
    println!(
        "{}",
        "  🚀 A new version of bifrost is available!"
            .bright_yellow()
            .bold()
    );
    println!();
    println!(
        "     Current version: {}",
        current_version.bright_red().bold()
    );
    println!(
        "     Latest version:  {}",
        cache.latest_version.bright_green().bold()
    );

    if !cache.release_highlights.is_empty() {
        println!();
        println!("     {}", "What's new:".bright_white().bold());
        for highlight in &cache.release_highlights {
            println!("       {} {}", "•".bright_cyan(), highlight.bright_white());
        }
        println!(
            "       {} {}",
            "→".dimmed(),
            format!("View full release notes: {}", release_url).dimmed()
        );
    }

    println!();
    println!("     {}", "To upgrade, run:".bright_white());
    println!("       {}", "bifrost upgrade".bright_cyan().bold());
    println!("{}", separator.bright_yellow());
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::version_check::{
        compare_versions, extract_any_commit_message, extract_commit_message, is_newer_version,
        parse_release_highlights,
    };

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
    #[ignore = "requires network access"]
    fn test_fetch_latest_release_from_github() {
        let result = fetch_latest_release();
        assert!(result.is_some(), "Should fetch release from GitHub API");

        let (version, _highlights) = result.unwrap();
        assert!(!version.is_empty(), "Version should not be empty");
        assert!(
            !version.starts_with('v'),
            "Version should not start with 'v'"
        );

        let parts: Vec<&str> = version.split('-').next().unwrap().split('.').collect();
        assert!(parts.len() >= 2, "Version should have at least major.minor");
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
    #[ignore = "visual test - run manually to see output"]
    fn test_visual_update_notice() {
        let cache = VersionCache {
            latest_version: "1.0.0".to_string(),
            release_highlights: vec![
                "Added WebSocket support for real-time communication".to_string(),
                "Improved HTTP/2 performance by 40%".to_string(),
                "New rule engine with wildcard matching".to_string(),
            ],
            checked_at: Utc::now(),
        };

        println!("\n=== Test with highlights ===");
        print_update_notice("0.0.1-alpha", &cache);

        let cache_no_highlights = VersionCache {
            latest_version: "1.0.0".to_string(),
            release_highlights: vec![],
            checked_at: Utc::now(),
        };

        println!("\n=== Test without highlights ===");
        print_update_notice("0.0.1-alpha", &cache_no_highlights);
    }
}
