use super::{
    is_path_wildcard_pattern, DomainMatcher, IpMatcher, Matcher, PathWildcardMatcher, RegexMatcher,
    WildcardMatcher,
};

#[derive(Debug, Clone, PartialEq)]
pub enum PatternType {
    Regex,
    PathWildcard,
    Wildcard,
    Domain,
    Ip,
}

pub fn parse_pattern(pattern: &str) -> Result<Box<dyn Matcher>, PatternParseError> {
    let pattern_type = detect_pattern_type(pattern);

    match pattern_type {
        PatternType::Regex => {
            let matcher = RegexMatcher::new(pattern)
                .map_err(|e| PatternParseError::InvalidRegex(e.to_string()))?;
            Ok(Box::new(matcher))
        }
        PatternType::Ip => {
            let matcher =
                IpMatcher::new(pattern).map_err(|e| PatternParseError::InvalidIp(e.to_string()))?;
            Ok(Box::new(matcher))
        }
        PatternType::PathWildcard => {
            let matcher = PathWildcardMatcher::new(pattern)
                .map_err(|e| PatternParseError::InvalidPathWildcard(e.to_string()))?;
            Ok(Box::new(matcher))
        }
        PatternType::Wildcard => {
            let matcher = WildcardMatcher::new(pattern)
                .map_err(|e| PatternParseError::InvalidWildcard(e.to_string()))?;
            Ok(Box::new(matcher))
        }
        PatternType::Domain => {
            let matcher = DomainMatcher::new(pattern);
            Ok(Box::new(matcher))
        }
    }
}

pub fn detect_pattern_type(pattern: &str) -> PatternType {
    let clean_pattern = pattern.strip_prefix('!').unwrap_or(pattern);

    if is_regex_pattern(clean_pattern) {
        return PatternType::Regex;
    }

    if is_path_wildcard_pattern(pattern) {
        return PatternType::PathWildcard;
    }

    if is_ip_pattern(clean_pattern) {
        return PatternType::Ip;
    }

    if is_wildcard_pattern(clean_pattern) {
        return PatternType::Wildcard;
    }

    PatternType::Domain
}

fn is_regex_pattern(pattern: &str) -> bool {
    if pattern.starts_with('/')
        && pattern.len() > 1
        && (pattern.ends_with('/') || pattern.ends_with("/i"))
    {
        return true;
    }
    false
}

fn is_ip_pattern(pattern: &str) -> bool {
    if pattern.starts_with("http://")
        || pattern.starts_with("https://")
        || pattern.starts_with("ws://")
        || pattern.starts_with("wss://")
    {
        return false;
    }

    let clean = pattern;

    if clean.contains('/') && !clean.starts_with('/') {
        let parts: Vec<&str> = clean.splitn(2, '/').collect();
        if parts.len() == 2 {
            let ip_part = parts[0];
            let prefix_part = parts[1];
            let ip_for_check = ip_part.split(':').next().unwrap_or(ip_part);
            if is_ipv4(ip_for_check) && prefix_part.parse::<u8>().is_ok() {
                return true;
            }
        }
    }

    let host_part = clean.split('/').next().unwrap_or(clean);

    if is_ipv6(host_part) {
        return true;
    }

    let host_without_port = if host_part.contains(':') {
        let colon_count = host_part.matches(':').count();
        if colon_count == 1 {
            let parts: Vec<&str> = host_part.splitn(2, ':').collect();
            if parts[1].parse::<u16>().is_ok() {
                return false;
            }
            parts[0]
        } else {
            host_part
        }
    } else {
        host_part
    };

    is_ipv4(host_without_port)
}

fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return false;
    }

    parts.iter().all(|part| {
        part.parse::<u8>().is_ok()
            || (part.contains('/') && {
                let sub_parts: Vec<&str> = part.split('/').collect();
                sub_parts.len() == 2
                    && sub_parts[0].parse::<u8>().is_ok()
                    && sub_parts[1].parse::<u8>().is_ok()
            })
    })
}

fn is_ipv6(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }

    let s = s.strip_prefix('[').unwrap_or(s);
    let s = s.strip_suffix(']').unwrap_or(s);

    if s == "::1" || s.starts_with("::") || s.ends_with("::") {
        return true;
    }

    let has_colons = s.matches(':').count() >= 2;
    let all_hex = s
        .split(':')
        .all(|part| part.is_empty() || part.chars().all(|c| c.is_ascii_hexdigit()));

    has_colons && all_hex
}

fn is_wildcard_pattern(pattern: &str) -> bool {
    let clean = pattern
        .strip_prefix("http://")
        .or_else(|| pattern.strip_prefix("https://"))
        .or_else(|| pattern.strip_prefix("http*://"))
        .or_else(|| pattern.strip_prefix("ws*://"))
        .or_else(|| pattern.strip_prefix("//"))
        .unwrap_or(pattern);

    if clean.starts_with('$') {
        return true;
    }

    let domain_part = clean.split('/').next().unwrap_or(clean);
    if let Some(colon_pos) = domain_part.rfind(':') {
        let host = &domain_part[..colon_pos];
        let port = &domain_part[colon_pos + 1..];

        if port.contains('*') && !host.contains('*') && !host.contains('?') {
            return false;
        }
    }

    clean.contains('*') || clean.contains('?')
}

#[derive(Debug, Clone)]
pub enum PatternParseError {
    InvalidRegex(String),
    InvalidWildcard(String),
    InvalidPathWildcard(String),
    InvalidIp(String),
}

impl std::fmt::Display for PatternParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatternParseError::InvalidRegex(s) => write!(f, "Invalid regex pattern: {}", s),
            PatternParseError::InvalidWildcard(s) => write!(f, "Invalid wildcard pattern: {}", s),
            PatternParseError::InvalidPathWildcard(s) => {
                write!(f, "Invalid path wildcard pattern: {}", s)
            }
            PatternParseError::InvalidIp(s) => write!(f, "Invalid IP pattern: {}", s),
        }
    }
}

impl std::error::Error for PatternParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_regex_pattern() {
        assert_eq!(detect_pattern_type("/example\\.com/"), PatternType::Regex);
        assert_eq!(detect_pattern_type("/test/i"), PatternType::Regex);
        assert_eq!(detect_pattern_type("!/pattern/"), PatternType::Regex);
        assert_eq!(detect_pattern_type("!/pattern/i"), PatternType::Regex);
    }

    #[test]
    fn test_detect_ip_pattern() {
        assert_eq!(detect_pattern_type("192.168.1.1"), PatternType::Ip);
        assert_eq!(detect_pattern_type("192.168.0.0/16"), PatternType::Ip);
        assert_eq!(detect_pattern_type("10.0.0.0/8"), PatternType::Ip);
        assert_eq!(detect_pattern_type("!192.168.1.1"), PatternType::Ip);
        assert_eq!(detect_pattern_type("::1"), PatternType::Ip);
    }

    #[test]
    fn test_detect_wildcard_pattern() {
        assert_eq!(detect_pattern_type("*.example.com"), PatternType::Wildcard);
        assert_eq!(detect_pattern_type("example.*"), PatternType::Wildcard);
        assert_eq!(detect_pattern_type("*example*"), PatternType::Wildcard);
        assert_eq!(detect_pattern_type("$example.com"), PatternType::Wildcard);
        assert_eq!(detect_pattern_type("example.com/*"), PatternType::Wildcard);
        assert_eq!(detect_pattern_type("!*.example.com"), PatternType::Wildcard);
        assert_eq!(detect_pattern_type("example?.com"), PatternType::Wildcard);
    }

    #[test]
    fn test_detect_path_wildcard_pattern() {
        assert_eq!(
            detect_pattern_type("^example.com/*"),
            PatternType::PathWildcard
        );
        assert_eq!(
            detect_pattern_type("^example.com/**"),
            PatternType::PathWildcard
        );
        assert_eq!(
            detect_pattern_type("^example.com/***"),
            PatternType::PathWildcard
        );
        assert_eq!(
            detect_pattern_type("!^example.com/*"),
            PatternType::PathWildcard
        );
        assert_eq!(
            detect_pattern_type("^api.example.com/v1/*/info"),
            PatternType::PathWildcard
        );
    }

    #[test]
    fn test_detect_domain_pattern() {
        assert_eq!(detect_pattern_type("example.com"), PatternType::Domain);
        assert_eq!(detect_pattern_type("example.com:8080"), PatternType::Domain);
        assert_eq!(
            detect_pattern_type("example.com/api/users"),
            PatternType::Domain
        );
        assert_eq!(
            detect_pattern_type("http://example.com"),
            PatternType::Domain
        );
        assert_eq!(
            detect_pattern_type("https://example.com"),
            PatternType::Domain
        );
        assert_eq!(detect_pattern_type("!example.com"), PatternType::Domain);
    }

    #[test]
    fn test_parse_regex_pattern() {
        let matcher = parse_pattern("/example\\.com/").unwrap();
        assert_eq!(matcher.priority(), 80);

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_parse_ip_pattern() {
        let matcher = parse_pattern("192.168.1.1").unwrap();
        assert_eq!(matcher.priority(), 95);

        let result = matcher.matches("http://192.168.1.1/path", "192.168.1.1", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_parse_cidr_pattern() {
        let matcher = parse_pattern("192.168.0.0/16").unwrap();

        let result = matcher.matches("http://192.168.1.1/path", "192.168.1.1", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://10.0.0.1/path", "10.0.0.1", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_parse_wildcard_pattern() {
        let matcher = parse_pattern("*.example.com").unwrap();

        let result = matcher.matches("http://www.example.com", "www.example.com", "/");
        assert!(result.matched);

        let result = matcher.matches("http://example.com", "example.com", "/");
        assert!(!result.matched);
    }

    #[test]
    fn test_parse_wildcard_root_path_pattern() {
        assert_eq!(detect_pattern_type("*.qq.com/"), PatternType::Wildcard);

        let matcher = parse_pattern("*.qq.com/").unwrap();

        let result = matcher.matches("https://www.qq.com", "www.qq.com", "/");
        assert!(result.matched);

        let result = matcher.matches(
            "https://news.qq.com/rain/a/20260428A07D9U00",
            "news.qq.com",
            "/rain/a/20260428A07D9U00",
        );
        assert!(result.matched);

        let result = matcher.matches("https://a.b.qq.com/rain", "a.b.qq.com", "/rain");
        assert!(!result.matched);
    }

    #[test]
    fn test_parse_path_wildcard_pattern() {
        let matcher = parse_pattern("^example.com/api/*").unwrap();
        assert_eq!(matcher.priority(), 70);

        let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);

        let result = matcher.matches(
            "http://example.com/api/users/nested",
            "example.com",
            "/api/users/nested",
        );
        assert!(!result.matched);
    }

    #[test]
    fn test_parse_path_wildcard_double_star() {
        let matcher = parse_pattern("^example.com/api/**").unwrap();

        let result = matcher.matches(
            "http://example.com/api/users/123/details",
            "example.com",
            "/api/users/123/details",
        );
        assert!(result.matched);

        let result = matcher.matches(
            "http://example.com/api/users?id=123",
            "example.com",
            "/api/users?id=123",
        );
        assert!(!result.matched);
    }

    #[test]
    fn test_parse_path_wildcard_triple_star() {
        let matcher = parse_pattern("^example.com/api/***").unwrap();

        let result = matcher.matches(
            "http://example.com/api/users?id=123",
            "example.com",
            "/api/users?id=123",
        );
        assert!(result.matched);
    }

    #[test]
    fn test_parse_domain_pattern() {
        let matcher = parse_pattern("example.com").unwrap();
        assert_eq!(matcher.priority(), 100);

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_parse_negated_patterns() {
        let regex_matcher = parse_pattern("!/test/").unwrap();
        assert!(regex_matcher.is_negated());

        let ip_matcher = parse_pattern("!192.168.1.1").unwrap();
        assert!(ip_matcher.is_negated());

        let wildcard_matcher = parse_pattern("!*.example.com").unwrap();
        assert!(wildcard_matcher.is_negated());

        let domain_matcher = parse_pattern("!example.com").unwrap();
        assert!(domain_matcher.is_negated());
    }

    #[test]
    fn test_parse_invalid_regex() {
        let result = parse_pattern("/[invalid/");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_complex_patterns() {
        let https_domain = parse_pattern("https://example.com:8443/api/*").unwrap();
        let result = https_domain.matches(
            "https://example.com:8443/api/users",
            "example.com:8443",
            "/api/users",
        );
        assert!(result.matched);

        let dollar_wildcard = parse_pattern("$example.com").unwrap();
        let result = dollar_wildcard.matches("http://example.com/path", "example.com", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_full_url_pattern_with_exact_path() {
        let matcher = parse_pattern("https://full-match.local:443/api/v1").unwrap();
        let result = matcher.matches(
            "https://full-match.local:443/api/v1",
            "full-match.local:443",
            "/api/v1",
        );
        assert!(result.matched, "Exact path should match");

        let result = matcher.matches(
            "https://full-match.local:443/api/v1/test",
            "full-match.local:443",
            "/api/v1/test",
        );
        assert!(
            result.matched,
            "Subpath should also match (prefix matching)"
        );

        let result = matcher.matches(
            "https://full-match.local:443/api/v2",
            "full-match.local:443",
            "/api/v2",
        );
        assert!(!result.matched, "Different path should not match");
    }

    #[test]
    fn test_priority_ordering() {
        let domain = parse_pattern("example.com").unwrap();
        let ip = parse_pattern("192.168.1.1").unwrap();
        let regex = parse_pattern("/test/").unwrap();
        let wildcard = parse_pattern("*.example.com").unwrap();

        assert!(domain.priority() > ip.priority());
        assert!(ip.priority() > regex.priority());
        assert!(regex.priority() > wildcard.priority());
    }

    #[test]
    fn test_is_ipv4() {
        assert!(is_ipv4("192.168.1.1"));
        assert!(is_ipv4("10.0.0.1"));
        assert!(is_ipv4("255.255.255.255"));
        assert!(is_ipv4("0.0.0.0"));

        assert!(!is_ipv4("example.com"));
        assert!(!is_ipv4("192.168.1"));
        assert!(!is_ipv4("192.168.1.1.1"));
        assert!(!is_ipv4("256.1.1.1"));
    }

    #[test]
    fn test_is_ipv6() {
        assert!(is_ipv6("::1"));
        assert!(is_ipv6("2001:db8::1"));
        assert!(is_ipv6("fe80::1"));
        assert!(is_ipv6("[::1]"));

        assert!(!is_ipv6("example.com"));
        assert!(!is_ipv6("192.168.1.1"));
    }

    #[test]
    fn test_domain_with_path_not_wildcard() {
        let pattern_type = detect_pattern_type("example.com/api/users");
        assert_eq!(pattern_type, PatternType::Domain);
    }

    #[test]
    fn test_http_prefix_handling() {
        assert_eq!(
            detect_pattern_type("http://example.com"),
            PatternType::Domain
        );
        assert_eq!(
            detect_pattern_type("https://example.com"),
            PatternType::Domain
        );
        assert_eq!(
            detect_pattern_type("http://*.example.com"),
            PatternType::Wildcard
        );
        assert_eq!(
            detect_pattern_type("https://*.example.com"),
            PatternType::Wildcard
        );
    }

    #[test]
    fn test_parse_pattern_returns_correct_type() {
        let regex = parse_pattern("/test/").unwrap();
        let ip = parse_pattern("192.168.1.1").unwrap();
        let wildcard = parse_pattern("*.example.com").unwrap();
        let domain = parse_pattern("example.com").unwrap();

        assert_eq!(regex.priority(), 80);
        assert_eq!(ip.priority(), 95);
        assert_eq!(wildcard.priority(), 55);
        assert_eq!(domain.priority(), 100);
    }

    #[test]
    fn test_edge_cases() {
        let single_char = parse_pattern("a");
        assert!(single_char.is_ok());

        let just_slash = detect_pattern_type("/");
        assert_eq!(just_slash, PatternType::Domain);

        let empty_negation = detect_pattern_type("!");
        assert_eq!(empty_negation, PatternType::Domain);
    }
}
