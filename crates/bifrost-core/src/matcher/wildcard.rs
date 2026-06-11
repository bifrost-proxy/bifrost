use regex::Regex;

use super::{pattern_has_concrete_host_scope, MatchResult, Matcher};

#[derive(Debug, Clone, PartialEq)]
pub enum WildcardType {
    Prefix,
    Suffix,
    Contains,
    DomainWildcard,
    PathWildcard,
    Mixed,
}

pub struct WildcardMatcher {
    pattern: Regex,
    negated: bool,
    raw_pattern: String,
    wildcard_type: WildcardType,
    capture_groups: usize,
}

impl WildcardMatcher {
    pub fn new(pattern: &str) -> Result<Self, regex::Error> {
        let (negated, clean_pattern) = Self::parse_negation(pattern);
        let (scheme_regex, body) = Self::split_scheme(clean_pattern);
        let wildcard_type = Self::detect_type(body);
        let (regex_pattern, capture_groups) = Self::to_regex(body, &wildcard_type, scheme_regex);
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

    /// Splits an optional URL-scheme prefix off the pattern.
    ///
    /// Returns `(scheme_regex, body)` where `scheme_regex` is a regex fragment that
    /// matches the scheme + `://`, and `body` is the host/path portion the wildcard
    /// machinery operates on. Stripping the scheme here (rather than leaving it in
    /// `body`) keeps the wildcard regex correct: it stops the `/` inside `://` from
    /// being treated as a path separator (which corrupted `*` semantics) and lets
    /// scheme wildcards like `http*://` / `ws*://` / `//` work with wildcard hosts.
    ///
    /// A leading `$` is the domain-wildcard marker, not a scheme, so it stays in
    /// `body` and the default scheme regex is used.
    fn split_scheme(pattern: &str) -> (&'static str, &str) {
        if let Some(rest) = pattern.strip_prefix("http*://") {
            ("https?://", rest)
        } else if let Some(rest) = pattern.strip_prefix("https://") {
            ("https://", rest)
        } else if let Some(rest) = pattern.strip_prefix("http://") {
            ("http://", rest)
        } else if let Some(rest) = pattern.strip_prefix("ws*://") {
            ("wss?://", rest)
        } else if let Some(rest) = pattern.strip_prefix("wss://") {
            ("wss://", rest)
        } else if let Some(rest) = pattern.strip_prefix("ws://") {
            ("ws://", rest)
        } else if let Some(rest) = pattern.strip_prefix("//") {
            (r"[a-zA-Z][a-zA-Z0-9+.\-]*://", rest)
        } else {
            // No scheme prefix (including the `$` domain-wildcard marker): match
            // http/https by default, mirroring the historical behavior.
            ("https?://", pattern)
        }
    }

    fn detect_type(pattern: &str) -> WildcardType {
        if pattern.starts_with('$') {
            return WildcardType::DomainWildcard;
        }

        let has_prefix_star = pattern.starts_with('*');
        let has_suffix_star = pattern.ends_with('*');
        let has_path_wildcard = pattern.contains("/*") || pattern.contains("*/");

        let inner_stars = pattern
            .trim_start_matches('*')
            .trim_end_matches('*')
            .contains('*');

        if has_path_wildcard {
            WildcardType::PathWildcard
        } else if has_prefix_star && has_suffix_star {
            WildcardType::Contains
        } else if has_prefix_star {
            WildcardType::Prefix
        } else if has_suffix_star {
            WildcardType::Suffix
        } else if inner_stars {
            WildcardType::Mixed
        } else {
            WildcardType::Suffix
        }
    }

    fn to_regex(body: &str, wildcard_type: &WildcardType, scheme: &str) -> (String, usize) {
        let (escaped, capture_groups) = Self::pattern_to_regex(body, wildcard_type);
        let has_explicit_path = body.contains('/');
        let is_root_path = body
            .split_once('/')
            .is_some_and(|(_, path)| path.is_empty());

        let host_wildcard_regex = |escaped: &str| {
            if is_root_path {
                let host = escaped.trim_end_matches('/');
                format!("^{}{}(?:/.*)?$", scheme, host)
            } else if has_explicit_path && escaped.ends_with('/') {
                format!("^{}{}.*$", scheme, escaped)
            } else {
                format!("^{}{}(/.*)?$", scheme, escaped)
            }
        };

        let regex = match wildcard_type {
            WildcardType::DomainWildcard => {
                let domain_pattern = escaped.replace("__DOLLAR__", "");
                format!("^{}{}(/.*)?$", scheme, domain_pattern)
            }
            WildcardType::Prefix
            | WildcardType::Suffix
            | WildcardType::Contains
            | WildcardType::Mixed => host_wildcard_regex(&escaped),
            WildcardType::PathWildcard => {
                format!("^{}{}$", scheme, escaped)
            }
        };

        (regex, capture_groups)
    }

    fn pattern_to_regex(pattern: &str, wildcard_type: &WildcardType) -> (String, usize) {
        let mut result = String::with_capacity(pattern.len() * 2);
        let special_chars = ['.', '+', '^', '(', ')', '[', ']', '{', '}', '|', '\\'];
        let mut in_path = false;
        let mut chars = pattern.chars().peekable();
        let mut capture_count = 0;

        while let Some(c) = chars.next() {
            match c {
                '*' => {
                    capture_count += 1;
                    if in_path {
                        result.push_str("(.*)");
                    } else {
                        let is_double = chars.peek() == Some(&'*');
                        if is_double {
                            chars.next();
                            result.push_str("([^/?]*)");
                        } else {
                            match wildcard_type {
                                WildcardType::DomainWildcard | WildcardType::PathWildcard => {
                                    result.push_str("([^/?.]*)");
                                }
                                WildcardType::Prefix => {
                                    result.push_str("([^/?.]*)");
                                }
                                _ => {
                                    result.push_str("([^/?]*)");
                                }
                            }
                        }
                    }
                }
                '?' => {
                    result.push('.');
                }
                '$' => {
                    result.push_str("__DOLLAR__");
                }
                '/' => {
                    in_path = true;
                    result.push(c);
                }
                _ if special_chars.contains(&c) => {
                    result.push('\\');
                    result.push(c);
                }
                _ => {
                    result.push(c);
                }
            }
        }
        (result, capture_count)
    }

    fn extract_captures(&self, url: &str) -> Option<Vec<String>> {
        self.pattern.captures(url).map(|caps| {
            (1..=self.capture_groups)
                .filter_map(|i| caps.get(i).map(|m| m.as_str().to_string()))
                .collect()
        })
    }

    pub fn capture_groups(&self) -> usize {
        self.capture_groups
    }

    pub fn raw_pattern(&self) -> &str {
        &self.raw_pattern
    }

    pub fn wildcard_type(&self) -> &WildcardType {
        &self.wildcard_type
    }
}

impl Matcher for WildcardMatcher {
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
            WildcardType::DomainWildcard => 50,
            WildcardType::Contains => 40,
            WildcardType::Mixed => 45,
            WildcardType::PathWildcard => 60,
            WildcardType::Prefix => 55,
            WildcardType::Suffix => 55,
        }
    }

    fn can_trigger_tls_auto_intercept(&self) -> bool {
        !self.negated && pattern_has_concrete_host_scope(&self.raw_pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prefix_wildcard() {
        let matcher = WildcardMatcher::new("*.example.com").unwrap();
        assert_eq!(matcher.wildcard_type(), &WildcardType::Prefix);

        let result = matcher.matches("http://www.example.com", "www.example.com", "/");
        assert!(result.matched);

        let result = matcher.matches("https://api.example.com", "api.example.com", "/");
        assert!(result.matched);

        let result = matcher.matches("http://example.com", "example.com", "/");
        assert!(!result.matched);
    }

    #[test]
    fn test_prefix_wildcard_subdomain() {
        let matcher = WildcardMatcher::new("*.test.example.com").unwrap();

        let result = matcher.matches("http://api.test.example.com", "api.test.example.com", "/");
        assert!(result.matched);

        let result = matcher.matches("http://test.example.com", "test.example.com", "/");
        assert!(!result.matched);
    }

    #[test]
    fn test_prefix_wildcard_with_root_path_matches_subpaths() {
        let matcher = WildcardMatcher::new("*.qq.com/").unwrap();
        assert_eq!(matcher.wildcard_type(), &WildcardType::Prefix);

        let result = matcher.matches("https://www.qq.com", "www.qq.com", "/");
        assert!(result.matched);

        let result = matcher.matches("https://www.qq.com/", "www.qq.com", "/");
        assert!(result.matched);

        let result = matcher.matches(
            "https://news.qq.com/rain/a/20260428A07D9U00",
            "news.qq.com",
            "/rain/a/20260428A07D9U00",
        );
        assert!(result.matched);

        let result = matcher.matches("https://a.b.qq.com/rain", "a.b.qq.com", "/rain");
        assert!(!result.matched);

        let result = matcher.matches("https://news.example.com/rain", "news.example.com", "/rain");
        assert!(!result.matched);
    }

    #[test]
    fn test_suffix_wildcard() {
        let matcher = WildcardMatcher::new("example.*").unwrap();
        assert_eq!(matcher.wildcard_type(), &WildcardType::Suffix);

        let result = matcher.matches("http://example.com", "example.com", "/");
        assert!(result.matched);

        let result = matcher.matches("http://example.org", "example.org", "/");
        assert!(result.matched);

        let result = matcher.matches("http://example.co.uk", "example.co.uk", "/");
        assert!(result.matched);
    }

    #[test]
    fn test_contains_wildcard() {
        let matcher = WildcardMatcher::new("*example*").unwrap();
        assert_eq!(matcher.wildcard_type(), &WildcardType::Contains);

        let result = matcher.matches("http://www.example.com/path", "www.example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://myexample.org", "myexample.org", "/");
        assert!(result.matched);

        let result = matcher.matches("http://test.com", "test.com", "/");
        assert!(!result.matched);
    }

    #[test]
    fn test_domain_wildcard() {
        let matcher = WildcardMatcher::new("$example.com").unwrap();
        assert_eq!(matcher.wildcard_type(), &WildcardType::DomainWildcard);

        let result = matcher.matches("http://example.com", "example.com", "/");
        assert!(result.matched);

        let result = matcher.matches("https://example.com", "example.com", "/");
        assert!(result.matched);

        let result = matcher.matches("http://example.com/api/test", "example.com", "/api/test");
        assert!(result.matched);
    }

    #[test]
    fn test_domain_wildcard_with_star() {
        let matcher = WildcardMatcher::new("$*.example.com").unwrap();

        let result = matcher.matches("http://api.example.com/path", "api.example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("https://www.example.com", "www.example.com", "/");
        assert!(result.matched);
    }

    #[test]
    fn test_path_wildcard() {
        let matcher = WildcardMatcher::new("example.com/api/*").unwrap();
        assert_eq!(matcher.wildcard_type(), &WildcardType::PathWildcard);

        let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);

        let result = matcher.matches(
            "http://example.com/api/products/123",
            "example.com",
            "/api/products/123",
        );
        assert!(result.matched);

        let result = matcher.matches("http://example.com/other", "example.com", "/other");
        assert!(!result.matched);
    }

    #[test]
    fn test_path_wildcard_nested() {
        let matcher = WildcardMatcher::new("example.com/api/*/details").unwrap();

        let result = matcher.matches(
            "http://example.com/api/users/details",
            "example.com",
            "/api/users/details",
        );
        assert!(result.matched);

        let result = matcher.matches(
            "http://example.com/api/products/details",
            "example.com",
            "/api/products/details",
        );
        assert!(result.matched);
    }

    #[test]
    fn test_negated_wildcard() {
        let matcher = WildcardMatcher::new("!*.example.com").unwrap();
        assert!(matcher.is_negated());

        let result = matcher.matches("http://www.example.com", "www.example.com", "/");
        assert!(!result.matched);

        let result = matcher.matches("http://other.com", "other.com", "/");
        assert!(result.matched);
    }

    #[test]
    fn test_negated_contains() {
        let matcher = WildcardMatcher::new("!*internal*").unwrap();
        assert!(matcher.is_negated());

        let result = matcher.matches("http://internal.company.com", "internal.company.com", "/");
        assert!(!result.matched);

        let result = matcher.matches("http://public.company.com", "public.company.com", "/");
        assert!(result.matched);
    }

    #[test]
    fn test_with_protocol_http() {
        let matcher = WildcardMatcher::new("http://*.example.com").unwrap();

        let result = matcher.matches("http://www.example.com", "www.example.com", "/");
        assert!(result.matched);
    }

    #[test]
    fn test_with_protocol_https() {
        let matcher = WildcardMatcher::new("https://*.example.com").unwrap();

        let result = matcher.matches("https://api.example.com", "api.example.com", "/");
        assert!(result.matched);
    }

    // Regression: an explicit scheme prefix on a wildcard host must keep the
    // single-level `*` semantics (it previously spanned `.` like `**`).
    #[test]
    fn test_explicit_scheme_keeps_single_level() {
        let matcher = WildcardMatcher::new("http://*.example.com").unwrap();
        assert!(
            matcher
                .matches("http://www.example.com", "www.example.com", "/")
                .matched
        );
        // http only
        assert!(
            !matcher
                .matches("https://www.example.com", "www.example.com", "/")
                .matched
        );
        // single level: must NOT match a multi-level subdomain
        assert!(
            !matcher
                .matches("http://a.b.example.com", "a.b.example.com", "/")
                .matched
        );
    }

    // Regression: `http*://` (and `ws*://`) scheme wildcards must work with a
    // wildcard host. Previously the `*/` inside `http*:/` was misdetected as a
    // path wildcard and the rule never matched anything.
    #[test]
    fn test_scheme_wildcard_http_star() {
        let matcher = WildcardMatcher::new("http*://*.example.com").unwrap();
        assert!(
            matcher
                .matches("http://www.example.com", "www.example.com", "/")
                .matched
        );
        assert!(
            matcher
                .matches("https://api.example.com", "api.example.com", "/")
                .matched
        );
        // single-level only
        assert!(
            !matcher
                .matches("http://a.b.example.com", "a.b.example.com", "/")
                .matched
        );
        // not ws
        assert!(
            !matcher
                .matches("ws://www.example.com", "www.example.com", "/")
                .matched
        );
    }

    #[test]
    fn test_scheme_wildcard_ws_star() {
        let matcher = WildcardMatcher::new("ws*://*.example.com").unwrap();
        assert!(
            matcher
                .matches("ws://chat.example.com", "chat.example.com", "/")
                .matched
        );
        assert!(
            matcher
                .matches("wss://chat.example.com", "chat.example.com", "/")
                .matched
        );
        assert!(
            !matcher
                .matches("http://chat.example.com", "chat.example.com", "/")
                .matched
        );
    }

    // Regression: `//` (any scheme) + wildcard host must stay scoped to the host,
    // not over-match every host as it did before.
    #[test]
    fn test_scheme_any_does_not_overmatch() {
        let matcher = WildcardMatcher::new("//*.example.com").unwrap();
        assert!(
            matcher
                .matches("http://www.example.com", "www.example.com", "/")
                .matched
        );
        assert!(
            matcher
                .matches("ws://www.example.com", "www.example.com", "/")
                .matched
        );
        // must NOT hijack unrelated hosts
        assert!(
            !matcher
                .matches(
                    "http://totally.unrelated.test",
                    "totally.unrelated.test",
                    "/"
                )
                .matched
        );
        // single level
        assert!(
            !matcher
                .matches("http://a.b.example.com", "a.b.example.com", "/")
                .matched
        );
    }

    // Bare wildcard host (no scheme) keeps matching http+https, single level.
    #[test]
    fn test_bare_wildcard_unchanged() {
        let matcher = WildcardMatcher::new("*.example.com").unwrap();
        assert!(
            matcher
                .matches("http://www.example.com", "www.example.com", "/")
                .matched
        );
        assert!(
            matcher
                .matches("https://www.example.com", "www.example.com", "/")
                .matched
        );
        assert!(
            !matcher
                .matches("http://a.b.example.com", "a.b.example.com", "/")
                .matched
        );
        assert!(
            !matcher
                .matches("http://example.com", "example.com", "/")
                .matched
        );
    }

    #[test]
    fn test_priority_values() {
        let domain = WildcardMatcher::new("$example.com").unwrap();
        assert_eq!(domain.priority(), 50);

        let contains = WildcardMatcher::new("*example*").unwrap();
        assert_eq!(contains.priority(), 40);

        let path = WildcardMatcher::new("example.com/*").unwrap();
        assert_eq!(path.priority(), 60);

        let prefix = WildcardMatcher::new("*.example.com").unwrap();
        assert_eq!(prefix.priority(), 55);
    }

    #[test]
    fn test_raw_pattern() {
        let pattern = "*.example.com";
        let matcher = WildcardMatcher::new(pattern).unwrap();
        assert_eq!(matcher.raw_pattern(), pattern);
    }

    #[test]
    fn test_special_chars_escaped() {
        let matcher = WildcardMatcher::new("example.com/path+test").unwrap();
        let result = matcher.matches("http://example.com/path+test", "example.com", "/path+test");
        assert!(result.matched);
    }

    #[test]
    fn test_question_mark_wildcard() {
        let matcher = WildcardMatcher::new("example?.com").unwrap();

        let result = matcher.matches("http://example1.com", "example1.com", "/");
        assert!(result.matched);

        let result = matcher.matches("http://exampleA.com", "exampleA.com", "/");
        assert!(result.matched);
    }

    #[test]
    fn test_complex_wildcard_pattern() {
        let matcher = WildcardMatcher::new("*.example.*/api/*").unwrap();

        let result = matcher.matches(
            "http://www.example.com/api/users",
            "www.example.com",
            "/api/users",
        );
        assert!(result.matched);

        let result = matcher.matches(
            "https://api.example.org/api/products",
            "api.example.org",
            "/api/products",
        );
        assert!(result.matched);
    }

    #[test]
    fn test_multiple_subdomain_levels() {
        let matcher = WildcardMatcher::new("*.*.example.com").unwrap();

        let result = matcher.matches("http://a.b.example.com", "a.b.example.com", "/");
        assert!(result.matched);
    }

    #[test]
    fn test_empty_path() {
        let matcher = WildcardMatcher::new("*.example.com").unwrap();

        let result = matcher.matches("http://www.example.com", "www.example.com", "");
        assert!(result.matched);
    }

    #[test]
    fn test_single_star_no_dot_match() {
        let matcher = WildcardMatcher::new("*.example.com").unwrap();

        let result = matcher.matches("http://www.example.com", "www.example.com", "/");
        assert!(result.matched);

        let result = matcher.matches("http://api.example.com", "api.example.com", "/");
        assert!(result.matched);

        let result = matcher.matches("http://a.b.example.com", "a.b.example.com", "/");
        assert!(!result.matched);
    }

    #[test]
    fn test_double_star_with_dot_match() {
        let matcher = WildcardMatcher::new("**.example.com").unwrap();

        let result = matcher.matches("http://www.example.com", "www.example.com", "/");
        assert!(result.matched);

        let result = matcher.matches("http://api.example.com", "api.example.com", "/");
        assert!(result.matched);

        let result = matcher.matches("http://a.b.example.com", "a.b.example.com", "/");
        assert!(result.matched);

        let result = matcher.matches("http://a.b.c.example.com", "a.b.c.example.com", "/");
        assert!(result.matched);
    }

    #[test]
    fn test_domain_dollar_single_star() {
        let matcher = WildcardMatcher::new("$*.example.com").unwrap();

        let result = matcher.matches("http://www.example.com/path", "www.example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://a.b.example.com/path", "a.b.example.com", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_domain_dollar_double_star() {
        let matcher = WildcardMatcher::new("$**.example.com").unwrap();

        let result = matcher.matches("http://www.example.com/path", "www.example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://a.b.example.com/path", "a.b.example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://x.y.z.example.com/", "x.y.z.example.com", "/");
        assert!(result.matched);
    }

    #[test]
    fn test_capture_groups_prefix() {
        let matcher = WildcardMatcher::new("*.example.com").unwrap();
        assert_eq!(matcher.capture_groups(), 1);

        let result = matcher.matches("http://www.example.com/", "www.example.com", "/");
        assert!(result.matched);
        assert_eq!(result.captures, Some(vec!["www".to_string()]));

        let result = matcher.matches("http://api.example.com/", "api.example.com", "/");
        assert!(result.matched);
        assert_eq!(result.captures, Some(vec!["api".to_string()]));
    }

    #[test]
    fn test_capture_groups_suffix() {
        let matcher = WildcardMatcher::new("example.*").unwrap();
        assert_eq!(matcher.capture_groups(), 1);

        let result = matcher.matches("http://example.com/", "example.com", "/");
        assert!(result.matched);
        assert_eq!(result.captures, Some(vec!["com".to_string()]));

        let result = matcher.matches("http://example.org/", "example.org", "/");
        assert!(result.matched);
        assert_eq!(result.captures, Some(vec!["org".to_string()]));
    }

    #[test]
    fn test_capture_groups_contains() {
        let matcher = WildcardMatcher::new("*example*").unwrap();
        assert_eq!(matcher.capture_groups(), 2);

        let result = matcher.matches("http://myexample.com/", "myexample.com", "/");
        assert!(result.matched);
        assert_eq!(
            result.captures,
            Some(vec!["my".to_string(), ".com".to_string()])
        );
    }

    #[test]
    fn test_capture_groups_path() {
        let matcher = WildcardMatcher::new("example.com/*").unwrap();
        assert_eq!(matcher.capture_groups(), 1);

        let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);
        assert_eq!(result.captures, Some(vec!["api/users".to_string()]));
    }

    #[test]
    fn test_capture_groups_multiple() {
        let matcher = WildcardMatcher::new("*.*.example.com").unwrap();
        assert_eq!(matcher.capture_groups(), 2);

        let result = matcher.matches("http://a.b.example.com/", "a.b.example.com", "/");
        assert!(result.matched);
        assert_eq!(
            result.captures,
            Some(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn test_capture_groups_negated() {
        let matcher = WildcardMatcher::new("!*.example.com").unwrap();

        let result = matcher.matches("http://other.com/", "other.com", "/");
        assert!(result.matched);
        assert!(result.captures.is_none());
    }

    #[test]
    fn test_capture_groups_double_star() {
        let matcher = WildcardMatcher::new("**.example.com").unwrap();
        assert_eq!(matcher.capture_groups(), 1);

        let result = matcher.matches("http://a.b.c.example.com/", "a.b.c.example.com", "/");
        assert!(result.matched);
        assert_eq!(result.captures, Some(vec!["a.b.c".to_string()]));
    }
}
