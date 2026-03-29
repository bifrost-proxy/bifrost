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
