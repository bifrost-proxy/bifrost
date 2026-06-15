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
    fn matches_host_scope(&self, url: &str, host: &str) -> bool {
        self.matches_host(url, host)
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

pub(crate) fn pattern_host_scope_matches(pattern: &str, url: &str, host: &str) -> bool {
    let mut clean = pattern.strip_prefix('!').unwrap_or(pattern);
    clean = clean.strip_prefix('^').unwrap_or(clean);

    let (scheme_match, body) = if let Some(rest) = clean.strip_prefix("http://") {
        (url.starts_with("http://"), rest)
    } else if let Some(rest) = clean.strip_prefix("https://") {
        (url.starts_with("https://"), rest)
    } else if let Some(rest) = clean.strip_prefix("http*://") {
        (
            url.starts_with("http://") || url.starts_with("https://"),
            rest,
        )
    } else if let Some(rest) = clean.strip_prefix("ws://") {
        (url.starts_with("ws://"), rest)
    } else if let Some(rest) = clean.strip_prefix("wss://") {
        (url.starts_with("wss://"), rest)
    } else if let Some(rest) = clean.strip_prefix("ws*://") {
        (url.starts_with("ws://") || url.starts_with("wss://"), rest)
    } else if let Some(rest) = clean.strip_prefix("tunnel://") {
        (url.starts_with("tunnel://"), rest)
    } else if let Some(rest) = clean.strip_prefix("//") {
        (url.contains("://"), rest)
    } else {
        (true, clean)
    };

    if !scheme_match || body.starts_with('/') {
        return false;
    }

    let host_pattern = body.split('/').next().unwrap_or(body);
    let host_pattern = host_pattern.strip_prefix('$').unwrap_or(host_pattern);
    if host_pattern.is_empty() {
        return false;
    }

    let (host_pattern, port_pattern) = split_host_port_pattern(host_pattern);
    if host_pattern.is_empty()
        || !host_pattern.chars().any(|c| c.is_ascii_alphanumeric())
        || !port_scope_matches(port_pattern, url, host)
    {
        return false;
    }

    let host = strip_host_port(host);
    wildcard_host_matches(host_pattern, host)
}

fn split_host_port_pattern(host: &str) -> (&str, Option<&str>) {
    if host.starts_with('[') {
        if let Some(end) = host.find(']') {
            let rest = &host[end + 1..];
            if let Some(port) = rest.strip_prefix(':') {
                return (&host[..=end], Some(port));
            }
        }
        return (host, None);
    }

    if host.matches(':').count() == 1 {
        let (candidate_host, candidate_port) = host.split_once(':').unwrap_or((host, ""));
        if candidate_port
            .chars()
            .all(|c| c.is_ascii_digit() || c == '*')
        {
            return (candidate_host, Some(candidate_port));
        }
    }

    (host, None)
}

fn strip_host_port(host: &str) -> &str {
    if host.starts_with('[') {
        return host.find(']').map(|end| &host[..=end]).unwrap_or(host);
    }

    if host.matches(':').count() == 1 {
        let (candidate_host, candidate_port) = host.split_once(':').unwrap_or((host, ""));
        if candidate_port.chars().all(|c| c.is_ascii_digit()) {
            return candidate_host;
        }
    }

    host
}

fn port_scope_matches(port_pattern: Option<&str>, url: &str, host: &str) -> bool {
    let Some(port_pattern) = port_pattern else {
        return true;
    };
    let Some(port) = host
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .or_else(|| default_port_for_url(url))
    else {
        return false;
    };

    wildcard_text_matches(port_pattern, &port.to_string(), true)
}

fn default_port_for_url(url: &str) -> Option<u16> {
    if url.starts_with("https://") || url.starts_with("wss://") {
        Some(443)
    } else if url.starts_with("http://") || url.starts_with("ws://") {
        Some(80)
    } else {
        None
    }
}

fn wildcard_host_matches(pattern: &str, host: &str) -> bool {
    wildcard_text_matches(pattern, host, false)
}

fn wildcard_text_matches(pattern: &str, value: &str, ascii_case_sensitive: bool) -> bool {
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            c => regex.push_str(&::regex::escape(&c.to_string())),
        }
    }
    regex.push('$');

    let regex = if ascii_case_sensitive {
        regex
    } else {
        format!("(?i:{regex})")
    };

    ::regex::Regex::new(&regex)
        .map(|compiled| compiled.is_match(value))
        .unwrap_or(false)
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

    #[test]
    fn pattern_has_concrete_host_scope_plain_host() {
        assert!(pattern_has_concrete_host_scope("example.com"));
        assert!(pattern_has_concrete_host_scope("example.com/api/v1"));
    }

    #[test]
    fn pattern_has_concrete_host_scope_path_only_is_false() {
        assert!(!pattern_has_concrete_host_scope("/api/v1"));
        assert!(!pattern_has_concrete_host_scope("/"));
    }

    #[test]
    fn pattern_has_concrete_host_scope_strips_schemes() {
        assert!(pattern_has_concrete_host_scope("http://example.com/x"));
        assert!(pattern_has_concrete_host_scope("https://example.com/x"));
        assert!(pattern_has_concrete_host_scope("http*://example.com"));
        assert!(pattern_has_concrete_host_scope("ws://example.com"));
        assert!(pattern_has_concrete_host_scope("wss://example.com"));
        assert!(pattern_has_concrete_host_scope("ws*://example.com"));
        assert!(pattern_has_concrete_host_scope("tunnel://example.com"));
        assert!(pattern_has_concrete_host_scope("//example.com/path"));
    }

    #[test]
    fn pattern_has_concrete_host_scope_strips_negation_and_anchor() {
        assert!(pattern_has_concrete_host_scope("!example.com"));
        assert!(pattern_has_concrete_host_scope("^example.com"));
        assert!(pattern_has_concrete_host_scope("!^https://example.com"));
        // negation/anchor in front of a path-only pattern still resolves false
        assert!(!pattern_has_concrete_host_scope("!/api"));
    }

    #[test]
    fn pattern_has_concrete_host_scope_handles_ports() {
        // numeric port stripped, host remains concrete
        assert!(pattern_has_concrete_host_scope("example.com:8080"));
        // wildcard port stripped
        assert!(pattern_has_concrete_host_scope("example.com:*"));
        // non-numeric "port" keeps full host string, still concrete
        assert!(pattern_has_concrete_host_scope("example.com:abc"));
    }

    #[test]
    fn pattern_has_concrete_host_scope_handles_ipv6() {
        assert!(pattern_has_concrete_host_scope("[::1]:8080"));
        assert!(pattern_has_concrete_host_scope("[2001:db8::1]"));
    }

    #[test]
    fn pattern_has_concrete_host_scope_var_prefix_and_empty() {
        // leading '$' is stripped; remaining host is concrete
        assert!(pattern_has_concrete_host_scope("$example.com"));
        // empty after stripping → false
        assert!(!pattern_has_concrete_host_scope(""));
        assert!(!pattern_has_concrete_host_scope("$"));
        // host with no alphanumeric chars → false
        assert!(!pattern_has_concrete_host_scope("$:*"));
    }

    struct DummyMatcher {
        result: bool,
    }

    impl Matcher for DummyMatcher {
        fn matches(&self, _url: &str, _host: &str, _path: &str) -> MatchResult {
            if self.result {
                MatchResult::matched()
            } else {
                MatchResult::not_matched()
            }
        }
        fn is_negated(&self) -> bool {
            false
        }
        fn priority(&self) -> i32 {
            0
        }
    }

    #[test]
    fn matcher_default_matches_host_delegates_to_matches() {
        let m = DummyMatcher { result: true };
        assert!(m.matches_host("http://x/", "x"));
        let m = DummyMatcher { result: false };
        assert!(!m.matches_host("http://x/", "x"));
    }

    #[test]
    fn matcher_default_can_trigger_tls_auto_intercept_is_false() {
        let m = DummyMatcher { result: true };
        assert!(!m.can_trigger_tls_auto_intercept());
    }

    #[test]
    fn pattern_host_scope_matches_plain_domain_with_path() {
        assert!(pattern_host_scope_matches(
            "qianchuan.jinritemai.com/app/account-center",
            "https://qianchuan.jinritemai.com",
            "qianchuan.jinritemai.com"
        ));
        assert!(!pattern_host_scope_matches(
            "qianchuan.jinritemai.com/app/account-center",
            "https://other.example.com",
            "other.example.com"
        ));
    }

    #[test]
    fn pattern_host_scope_matches_respects_explicit_scheme() {
        assert!(!pattern_host_scope_matches(
            "http://example.com/api",
            "https://example.com",
            "example.com"
        ));
        assert!(pattern_host_scope_matches(
            "http*://example.com/api",
            "https://example.com",
            "example.com"
        ));
        assert!(pattern_host_scope_matches(
            "wss://example.com/socket",
            "wss://example.com",
            "example.com"
        ));
    }

    #[test]
    fn pattern_host_scope_matches_wildcard_and_ports() {
        assert!(pattern_host_scope_matches(
            "*.example.com/api",
            "https://api.example.com",
            "api.example.com"
        ));
        assert!(pattern_host_scope_matches(
            "example.com:443/api",
            "https://example.com",
            "example.com"
        ));
        assert!(!pattern_host_scope_matches(
            "example.com:8443/api",
            "https://example.com",
            "example.com"
        ));
    }
}
