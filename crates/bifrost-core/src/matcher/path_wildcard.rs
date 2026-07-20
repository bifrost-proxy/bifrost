use regex::Regex;

use super::{pattern_has_concrete_host_scope, pattern_host_scope_matches, MatchResult, Matcher};

#[derive(Debug, Clone, PartialEq)]
pub enum PathWildcardType {
    Single,
    Double,
    Triple,
}

pub struct PathWildcardMatcher {
    pattern: Regex,
    negated: bool,
    raw_pattern: String,
    wildcard_type: PathWildcardType,
    capture_groups: usize,
}

impl PathWildcardMatcher {
    pub fn new(pattern: &str) -> Result<Self, regex::Error> {
        let (negated, clean_pattern) = Self::parse_negation(pattern);
        let pattern_without_prefix = clean_pattern.strip_prefix('^').unwrap_or(clean_pattern);
        let (wildcard_type, capture_groups, regex_pattern) = Self::to_regex(pattern_without_prefix);
        let compiled = Regex::new(&regex_pattern)?;

        Ok(Self {
            pattern: compiled,
            negated,
            raw_pattern: pattern.to_string(),
            wildcard_type,
            capture_groups,
        })
    }

    fn parse_negation(pattern: &str) -> (bool, &str) {
        if let Some(stripped) = pattern.strip_prefix('!') {
            (true, stripped)
        } else {
            (false, pattern)
        }
    }

    fn split_protocol_prefix(pattern: &str) -> (&'static str, &str) {
        if let Some(stripped) = pattern.strip_prefix("https://") {
            ("^https://", stripped)
        } else if let Some(stripped) = pattern.strip_prefix("http://") {
            ("^http://", stripped)
        } else if let Some(stripped) = pattern.strip_prefix("http*://") {
            ("^https?://", stripped)
        } else if let Some(stripped) = pattern.strip_prefix("//") {
            ("^https?://", stripped)
        } else {
            ("^https?://", pattern)
        }
    }

    fn to_regex(pattern: &str) -> (PathWildcardType, usize, String) {
        let (protocol_prefix, pattern) = Self::split_protocol_prefix(pattern);
        let mut result = String::with_capacity(pattern.len() * 2);
        let special_chars = ['.', '+', '^', '(', ')', '[', ']', '{', '}', '|', '\\', '$'];
        let mut capture_count = 0;
        let mut chars = pattern.chars().peekable();
        let mut wildcard_type = PathWildcardType::Single;

        result.push_str(protocol_prefix);

        while let Some(c) = chars.next() {
            match c {
                '*' => {
                    let star_count = 1 + Self::count_consecutive(&mut chars, '*');
                    capture_count += 1;
                    match star_count {
                        1 => {
                            result.push_str("([^?/]*)");
                        }
                        2 => {
                            wildcard_type = PathWildcardType::Double;
                            result.push_str("([^?]*)");
                        }
                        _ => {
                            wildcard_type = PathWildcardType::Triple;
                            result.push_str("(.*)");
                        }
                    }
                }
                '?' => {
                    if chars.peek() == Some(&'?') {
                        result.push_str("\\?\\?");
                        chars.next();
                    } else {
                        result.push('.');
                    }
                }
                _ if special_chars.contains(&c) => {
                    result.push('\\');
                    result.push(c);
                }
                _ => result.push(c),
            }
        }

        result.push('$');

        (wildcard_type, capture_count, result)
    }

    fn count_consecutive(chars: &mut std::iter::Peekable<std::str::Chars>, target: char) -> usize {
        let mut count = 0;
        while chars.peek() == Some(&target) {
            chars.next();
            count += 1;
        }
        count
    }

    pub fn raw_pattern(&self) -> &str {
        &self.raw_pattern
    }

    pub fn wildcard_type(&self) -> &PathWildcardType {
        &self.wildcard_type
    }

    pub fn capture_groups(&self) -> usize {
        self.capture_groups
    }

    fn extract_captures(&self, url: &str) -> Option<Vec<String>> {
        self.pattern.captures(url).map(|caps| {
            (1..=self.capture_groups)
                .filter_map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                .collect()
        })
    }
}

impl Matcher for PathWildcardMatcher {
    fn matches(&self, url: &str, _host: &str, _path: &str) -> MatchResult {
        let is_match = self.pattern.is_match(url);
        let effective_match = if self.negated { !is_match } else { is_match };

        if effective_match {
            if self.negated {
                MatchResult::matched()
            } else if let Some(captures) = self.extract_captures(url) {
                MatchResult::matched_with_captures(captures)
            } else {
                MatchResult::matched()
            }
        } else {
            MatchResult::not_matched()
        }
    }

    fn is_negated(&self) -> bool {
        self.negated
    }

    fn priority(&self) -> i32 {
        match self.wildcard_type {
            PathWildcardType::Single => 70,
            PathWildcardType::Double => 65,
            PathWildcardType::Triple => 60,
        }
    }

    fn matches_host_scope(&self, url: &str, host: &str) -> bool {
        !self.negated && pattern_host_scope_matches(&self.raw_pattern, url, host)
    }

    fn can_trigger_tls_auto_intercept(&self) -> bool {
        !self.negated && pattern_has_concrete_host_scope(&self.raw_pattern)
    }
}

pub fn is_path_wildcard_pattern(pattern: &str) -> bool {
    let clean = pattern.strip_prefix('!').unwrap_or(pattern);
    clean.starts_with('^')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_star_path_wildcard() {
        let matcher = PathWildcardMatcher::new("^example.com/api/*/info").unwrap();
        assert_eq!(matcher.wildcard_type(), &PathWildcardType::Single);

        let result = matcher.matches(
            "http://example.com/api/users/info",
            "example.com",
            "/api/users/info",
        );
        assert!(result.matched);
        assert_eq!(result.captures, Some(vec!["users".to_string()]));

        let result = matcher.matches(
            "http://example.com/api/products/info",
            "example.com",
            "/api/products/info",
        );
        assert!(result.matched);
        assert_eq!(result.captures, Some(vec!["products".to_string()]));

        let result = matcher.matches(
            "http://example.com/api/users/nested/info",
            "example.com",
            "/api/users/nested/info",
        );
        assert!(!result.matched);
    }

    #[test]
    fn test_double_star_path_wildcard() {
        let matcher = PathWildcardMatcher::new("^example.com/api/**").unwrap();
        assert_eq!(matcher.wildcard_type(), &PathWildcardType::Double);

        let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);
        assert_eq!(result.captures, Some(vec!["users".to_string()]));

        let result = matcher.matches(
            "http://example.com/api/users/123/details",
            "example.com",
            "/api/users/123/details",
        );
        assert!(result.matched);
        assert_eq!(result.captures, Some(vec!["users/123/details".to_string()]));

        let result = matcher.matches(
            "http://example.com/api/users?id=123",
            "example.com",
            "/api/users?id=123",
        );
        assert!(!result.matched);
    }

    #[test]
    fn test_triple_star_path_wildcard() {
        let matcher = PathWildcardMatcher::new("^example.com/api/***").unwrap();
        assert_eq!(matcher.wildcard_type(), &PathWildcardType::Triple);

        let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);

        let result = matcher.matches(
            "http://example.com/api/users?id=123",
            "example.com",
            "/api/users?id=123",
        );
        assert!(result.matched);
        assert_eq!(result.captures, Some(vec!["users?id=123".to_string()]));

        let result = matcher.matches(
            "http://example.com/api/a/b/c?x=1&y=2",
            "example.com",
            "/api/a/b/c?x=1&y=2",
        );
        assert!(result.matched);
    }

    #[test]
    fn test_single_star_no_slash_no_query() {
        let matcher = PathWildcardMatcher::new("^example.com/file-*").unwrap();

        let result = matcher.matches("http://example.com/file-abc", "example.com", "/file-abc");
        assert!(result.matched);
        assert_eq!(result.captures, Some(vec!["abc".to_string()]));

        let result = matcher.matches(
            "http://example.com/file-abc/nested",
            "example.com",
            "/file-abc/nested",
        );
        assert!(!result.matched);

        let result = matcher.matches(
            "http://example.com/file-abc?query=1",
            "example.com",
            "/file-abc?query=1",
        );
        assert!(!result.matched);
    }

    #[test]
    fn test_negated_path_wildcard() {
        let matcher = PathWildcardMatcher::new("!^example.com/api/*").unwrap();
        assert!(matcher.is_negated());

        let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
        assert!(!result.matched);

        let result = matcher.matches("http://example.com/other", "example.com", "/other");
        assert!(result.matched);
    }

    #[test]
    fn test_multiple_wildcards() {
        let matcher = PathWildcardMatcher::new("^example.com/*/action/*").unwrap();

        let result = matcher.matches(
            "http://example.com/users/action/delete",
            "example.com",
            "/users/action/delete",
        );
        assert!(result.matched);
        assert_eq!(
            result.captures,
            Some(vec!["users".to_string(), "delete".to_string()])
        );
    }

    #[test]
    fn test_priority_values() {
        let single = PathWildcardMatcher::new("^example.com/*").unwrap();
        assert_eq!(single.priority(), 70);

        let double = PathWildcardMatcher::new("^example.com/**").unwrap();
        assert_eq!(double.priority(), 65);

        let triple = PathWildcardMatcher::new("^example.com/***").unwrap();
        assert_eq!(triple.priority(), 60);
    }

    #[test]
    fn test_raw_pattern() {
        let pattern = "^example.com/api/*";
        let matcher = PathWildcardMatcher::new(pattern).unwrap();
        assert_eq!(matcher.raw_pattern(), pattern);
    }

    #[test]
    fn test_special_chars_escaped() {
        let matcher = PathWildcardMatcher::new("^example.com/path+test/*").unwrap();

        let result = matcher.matches(
            "http://example.com/path+test/value",
            "example.com",
            "/path+test/value",
        );
        assert!(result.matched);
    }

    #[test]
    fn test_is_path_wildcard_pattern() {
        assert!(is_path_wildcard_pattern("^example.com/*"));
        assert!(is_path_wildcard_pattern("!^example.com/*"));
        assert!(!is_path_wildcard_pattern("example.com/*"));
        assert!(!is_path_wildcard_pattern("*.example.com"));
    }

    #[test]
    fn test_capture_groups_count() {
        let single = PathWildcardMatcher::new("^example.com/*").unwrap();
        assert_eq!(single.capture_groups(), 1);

        let double = PathWildcardMatcher::new("^example.com/*/*").unwrap();
        assert_eq!(double.capture_groups(), 2);

        let triple = PathWildcardMatcher::new("^example.com/*/*/*").unwrap();
        assert_eq!(triple.capture_groups(), 3);
    }

    #[test]
    fn test_https_support() {
        let matcher = PathWildcardMatcher::new("^example.com/api/*").unwrap();

        let result = matcher.matches("https://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);

        let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);
    }

    #[test]
    fn test_explicit_https_path_wildcard_support() {
        let matcher = PathWildcardMatcher::new(
            "^https://cdn.example.com/obj/archi/obj/okrx-web/approvals-web/1.0.0.*/index.html",
        )
        .unwrap();

        let result = matcher.matches(
            "https://cdn.example.com/obj/archi/obj/okrx-web/approvals-web/1.0.0.3505/index.html",
            "cdn.example.com",
            "/obj/archi/obj/okrx-web/approvals-web/1.0.0.3505/index.html",
        );
        assert!(result.matched);
        assert_eq!(result.captures, Some(vec!["3505".to_string()]));

        let result = matcher.matches(
            "http://cdn.example.com/obj/archi/obj/okrx-web/approvals-web/1.0.0.3505/index.html",
            "cdn.example.com",
            "/obj/archi/obj/okrx-web/approvals-web/1.0.0.3505/index.html",
        );
        assert!(!result.matched);
    }

    #[test]
    fn test_explicit_http_path_wildcard_support() {
        let matcher = PathWildcardMatcher::new("^http://example.com/api/*").unwrap();

        let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);

        let result = matcher.matches("https://example.com/api/users", "example.com", "/api/users");
        assert!(!result.matched);
    }

    #[test]
    fn test_scheme_wildcard_path_wildcard_support() {
        let matcher = PathWildcardMatcher::new("^http*://example.com/api/*").unwrap();

        let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);

        let result = matcher.matches("https://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);

        let matcher = PathWildcardMatcher::new("^//example.com/api/*").unwrap();
        let result = matcher.matches("https://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);
    }

    #[test]
    fn test_complex_path_patterns() {
        let matcher = PathWildcardMatcher::new("^api.example.com/v1/*/items/*/details").unwrap();

        let result = matcher.matches(
            "http://api.example.com/v1/users/items/123/details",
            "api.example.com",
            "/v1/users/items/123/details",
        );
        assert!(result.matched);
        assert_eq!(
            result.captures,
            Some(vec!["users".to_string(), "123".to_string()])
        );
    }
}
