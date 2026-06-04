mod domain;
pub mod factory;
mod ip;
mod path_wildcard;
mod regex;
mod wildcard;

pub use domain::DomainMatcher;
pub use ip::IpMatcher;
pub use path_wildcard::{is_path_wildcard_pattern, PathWildcardMatcher};
pub use regex::RegexMatcher;
pub use wildcard::WildcardMatcher;

#[derive(Debug, Clone, Default)]
pub struct MatchResult {
    pub matched: bool,
    pub captures: Option<Vec<String>>,
}

impl MatchResult {
    pub fn matched() -> Self {
        Self {
            matched: true,
            captures: None,
        }
    }

    pub fn matched_with_captures(captures: Vec<String>) -> Self {
        Self {
            matched: true,
            captures: Some(captures),
        }
    }

    pub fn not_matched() -> Self {
        Self {
            matched: false,
            captures: None,
        }
    }
}

pub trait Matcher: Send + Sync {
    fn matches(&self, url: &str, host: &str, path: &str) -> MatchResult;
    fn matches_host(&self, url: &str, host: &str) -> bool {
        self.matches(url, host, "/").matched
    }
    fn is_negated(&self) -> bool;
    fn priority(&self) -> i32;
    fn can_trigger_tls_auto_intercept(&self) -> bool {
        false
    }
}

pub(crate) fn pattern_has_concrete_host_scope(pattern: &str) -> bool {
    let mut clean = pattern.strip_prefix('!').unwrap_or(pattern);
    clean = clean.strip_prefix('^').unwrap_or(clean);

    let clean = clean
        .strip_prefix("http://")
        .or_else(|| clean.strip_prefix("https://"))
        .or_else(|| clean.strip_prefix("http*://"))
        .or_else(|| clean.strip_prefix("ws://"))
        .or_else(|| clean.strip_prefix("wss://"))
        .or_else(|| clean.strip_prefix("ws*://"))
        .or_else(|| clean.strip_prefix("tunnel://"))
        .or_else(|| clean.strip_prefix("//"))
        .unwrap_or(clean);

    if clean.starts_with('/') {
        return false;
    }

    let mut host = clean.split('/').next().unwrap_or(clean);
    host = host.strip_prefix('$').unwrap_or(host);
    if host.is_empty() {
        return false;
    }

    let host_without_port = if host.starts_with('[') {
        host.find(']').map(|end| &host[..=end]).unwrap_or(host)
    } else if host.matches(':').count() == 1 {
        let (candidate_host, candidate_port) = host.split_once(':').unwrap_or((host, ""));
        if candidate_port
            .chars()
            .all(|c| c.is_ascii_digit() || c == '*')
        {
            candidate_host
        } else {
            host
        }
    } else {
        host
    };

    host_without_port.chars().any(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_result_matched() {
        let result = MatchResult::matched();
        assert!(result.matched);
        assert!(result.captures.is_none());
    }

    #[test]
    fn test_match_result_matched_with_captures() {
        let captures = vec!["group1".to_string(), "group2".to_string()];
        let result = MatchResult::matched_with_captures(captures.clone());
        assert!(result.matched);
        assert_eq!(result.captures, Some(captures));
    }

    #[test]
    fn test_match_result_not_matched() {
        let result = MatchResult::not_matched();
        assert!(!result.matched);
        assert!(result.captures.is_none());
    }
}
