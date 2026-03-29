use regex::Regex;

use super::{MatchResult, Matcher};

#[derive(Debug, Clone)]
enum PortMatcher {
    Exact(u16),
    Wildcard(Regex),
}

#[derive(Debug, Clone)]
enum ProtocolMatcher {
    Exact(String),
    HttpWildcard,
    WsWildcard,
    AnyProtocol,
}

pub struct DomainMatcher {
    domain: String,
    port: Option<PortMatcher>,
    path_pattern: Option<PathPattern>,
    protocol: Option<ProtocolMatcher>,
    negated: bool,
    raw_pattern: String,
}

#[derive(Debug, Clone)]
enum PathPattern {
    Exact(String),
    Prefix(String),
}

impl DomainMatcher {
    pub fn new(pattern: &str) -> Self {
        let (negated, clean_pattern) = Self::parse_negation(pattern);
        let (protocol, remaining) = Self::parse_protocol(clean_pattern);
        let (domain, port, path_pattern) = Self::parse_domain_port_path(remaining);

        Self {
            domain,
            port,
            path_pattern,
            protocol,
            negated,
            raw_pattern: pattern.to_string(),
        }
    }

    fn parse_negation(pattern: &str) -> (bool, &str) {
        if let Some(stripped) = pattern.strip_prefix('!') {
            (true, stripped)
        } else {
            (false, pattern)
        }
    }

    fn parse_protocol(pattern: &str) -> (Option<ProtocolMatcher>, &str) {
        if let Some(stripped) = pattern.strip_prefix("//") {
            (Some(ProtocolMatcher::AnyProtocol), stripped)
        } else if let Some(stripped) = pattern.strip_prefix("http*://") {
            (Some(ProtocolMatcher::HttpWildcard), stripped)
        } else if let Some(stripped) = pattern.strip_prefix("ws*://") {
            (Some(ProtocolMatcher::WsWildcard), stripped)
        } else if let Some(stripped) = pattern.strip_prefix("http://") {
            (Some(ProtocolMatcher::Exact("http".to_string())), stripped)
        } else if let Some(stripped) = pattern.strip_prefix("https://") {
            (Some(ProtocolMatcher::Exact("https".to_string())), stripped)
        } else if let Some(stripped) = pattern.strip_prefix("ws://") {
            (Some(ProtocolMatcher::Exact("ws".to_string())), stripped)
        } else if let Some(stripped) = pattern.strip_prefix("wss://") {
            (Some(ProtocolMatcher::Exact("wss".to_string())), stripped)
        } else if let Some(stripped) = pattern.strip_prefix("tunnel://") {
            (Some(ProtocolMatcher::Exact("tunnel".to_string())), stripped)
        } else {
            (None, pattern)
        }
    }

    fn parse_domain_port_path(pattern: &str) -> (String, Option<PortMatcher>, Option<PathPattern>) {
        let (domain_port, path) = if let Some(pos) = pattern.find('/') {
            let (dp, p) = pattern.split_at(pos);
            (dp, Some(p))
        } else {
            (pattern, None)
        };

        let (domain, port) = if let Some(colon_pos) = domain_port.rfind(':') {
            let port_str = &domain_port[colon_pos + 1..];
            if port_str.contains('*') {
                let port_matcher = Self::parse_port_wildcard(port_str);
                (domain_port[..colon_pos].to_string(), port_matcher)
            } else if let Ok(p) = port_str.parse::<u16>() {
                (
                    domain_port[..colon_pos].to_string(),
                    Some(PortMatcher::Exact(p)),
                )
            } else {
                (domain_port.to_string(), None)
            }
        } else {
            (domain_port.to_string(), None)
        };

        let path_pattern = path.map(|p| {
            if let Some(stripped) = p.strip_suffix('*') {
                PathPattern::Prefix(stripped.to_string())
            } else {
                PathPattern::Exact(p.to_string())
            }
        });

        (domain, port, path_pattern)
    }

    fn parse_port_wildcard(port_pattern: &str) -> Option<PortMatcher> {
        let mut regex_str = String::from("^");

        for c in port_pattern.chars() {
            if c == '*' {
                regex_str.push_str("[0-9]*");
            } else if c.is_ascii_digit() {
                regex_str.push(c);
            } else {
                return None;
            }
        }
        regex_str.push('$');

        Regex::new(&regex_str).ok().map(PortMatcher::Wildcard)
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }

    pub fn port(&self) -> Option<u16> {
        match &self.port {
            Some(PortMatcher::Exact(p)) => Some(*p),
            _ => None,
        }
    }

    pub fn has_port_pattern(&self) -> bool {
        self.port.is_some()
    }

    pub fn raw_pattern(&self) -> &str {
        &self.raw_pattern
    }

    fn matches_domain(&self, url: &str, host: &str) -> bool {
        let (check_host, check_port) = Self::split_host_port(host);

        if !self.domain.eq_ignore_ascii_case(check_host) {
            return false;
        }

        match &self.port {
            Some(PortMatcher::Exact(expected_port)) => match check_port {
                Some(p) => p == *expected_port,
                None => *expected_port == 80 || *expected_port == 443,
            },
            Some(PortMatcher::Wildcard(regex)) => match check_port {
                Some(p) => regex.is_match(&p.to_string()),
                None => {
                    let default_port = if url.starts_with("https://") || url.starts_with("wss://") {
                        443
                    } else {
                        80
                    };
                    regex.is_match(&default_port.to_string())
                }
            },
            None => true,
        }
    }

    fn split_host_port(host: &str) -> (&str, Option<u16>) {
        if let Some(colon_pos) = host.rfind(':') {
            let potential_port = &host[colon_pos + 1..];
            if let Ok(p) = potential_port.parse::<u16>() {
                return (&host[..colon_pos], Some(p));
            }
        }
        (host, None)
    }

    fn matches_path(&self, path: &str) -> bool {
        match &self.path_pattern {
            None => true,
            Some(PathPattern::Exact(expected)) => {
                if expected.ends_with('/') {
                    path == expected || path.starts_with(expected)
                } else {
                    path == expected
                        || path.starts_with(&format!("{}?", expected))
                        || path.starts_with(&format!("{}/", expected))
                }
            }
            Some(PathPattern::Prefix(prefix)) => path.starts_with(prefix),
        }
    }

    fn matches_protocol(&self, url: &str) -> bool {
        match &self.protocol {
            None => true,
            Some(ProtocolMatcher::Exact(proto)) => {
                let expected_prefix = format!("{}://", proto);
                url.starts_with(&expected_prefix)
            }
            Some(ProtocolMatcher::HttpWildcard) => {
                url.starts_with("http://") || url.starts_with("https://")
            }
            Some(ProtocolMatcher::WsWildcard) => {
                url.starts_with("ws://") || url.starts_with("wss://")
            }
            Some(ProtocolMatcher::AnyProtocol) => {
                url.starts_with("http://")
                    || url.starts_with("https://")
                    || url.starts_with("ws://")
                    || url.starts_with("wss://")
                    || url.starts_with("tunnel://")
            }
        }
    }
}

impl Matcher for DomainMatcher {
    fn matches(&self, url: &str, host: &str, path: &str) -> MatchResult {
        let protocol_match = self.matches_protocol(url);
        let domain_match = self.matches_domain(url, host);
        let path_match = self.matches_path(path);

        let is_match = protocol_match && domain_match && path_match;
        let effective_match = if self.negated { !is_match } else { is_match };

        if effective_match {
            MatchResult::matched()
        } else {
            MatchResult::not_matched()
        }
    }

    fn matches_host(&self, url: &str, host: &str) -> bool {
        if self.negated {
            return false;
        }
        self.matches_protocol(url) && self.matches_domain(url, host)
    }

    fn is_negated(&self) -> bool {
        self.negated
    }

    fn priority(&self) -> i32 {
        let mut priority = 100;

        if self.protocol.is_some() {
            priority += 5;
        }

        if self.port.is_some() {
            priority += 10;
        }

        if self.path_pattern.is_some() {
            priority += match &self.path_pattern {
                Some(PathPattern::Exact(_)) => 15,
                Some(PathPattern::Prefix(_)) => 10,
                None => 0,
            };
        }

        priority
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_domain() {
        let matcher = DomainMatcher::new("example.com");

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("https://example.com", "example.com", "/");
        assert!(result.matched);

        let result = matcher.matches("http://other.com", "other.com", "/");
        assert!(!result.matched);
    }

    #[test]
    fn test_domain_case_insensitive() {
        let matcher = DomainMatcher::new("Example.COM");

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://EXAMPLE.COM/path", "EXAMPLE.COM", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_domain_with_port() {
        let matcher = DomainMatcher::new("example.com:8080");
        assert_eq!(matcher.port(), Some(8080));

        let result = matcher.matches("http://example.com:8080/path", "example.com:8080", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(!result.matched);

        let result = matcher.matches("http://example.com:9090/path", "example.com:9090", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_domain_default_ports() {
        let matcher80 = DomainMatcher::new("example.com:80");
        let result = matcher80.matches("http://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let matcher443 = DomainMatcher::new("example.com:443");
        let result = matcher443.matches("https://example.com/path", "example.com", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_domain_with_exact_path() {
        let matcher = DomainMatcher::new("example.com/api/users");

        let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);

        let result = matcher.matches(
            "http://example.com/api/users?id=1",
            "example.com",
            "/api/users?id=1",
        );
        assert!(result.matched);

        let result = matcher.matches(
            "http://example.com/api/products",
            "example.com",
            "/api/products",
        );
        assert!(!result.matched);
    }

    #[test]
    fn test_domain_with_path_subpath_match() {
        let matcher = DomainMatcher::new("example.com/api");

        let result = matcher.matches("http://example.com/api", "example.com", "/api");
        assert!(result.matched);

        let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);

        let result = matcher.matches(
            "http://example.com/api/users/123",
            "example.com",
            "/api/users/123",
        );
        assert!(result.matched);

        let result = matcher.matches("http://example.com/api?q=1", "example.com", "/api?q=1");
        assert!(result.matched);

        let result = matcher.matches("http://example.com/other", "example.com", "/other");
        assert!(!result.matched);

        let result = matcher.matches("http://example.com/apitest", "example.com", "/apitest");
        assert!(!result.matched);
    }

    #[test]
    fn test_domain_with_path_prefix() {
        let matcher = DomainMatcher::new("example.com/api/*");

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
    fn test_domain_with_protocol_http() {
        let matcher = DomainMatcher::new("http://example.com");

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("https://example.com/path", "example.com", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_domain_with_protocol_https() {
        let matcher = DomainMatcher::new("https://example.com");

        let result = matcher.matches("https://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_negated_domain() {
        let matcher = DomainMatcher::new("!example.com");
        assert!(matcher.is_negated());

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(!result.matched);

        let result = matcher.matches("http://other.com/path", "other.com", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_negated_domain_with_path() {
        let matcher = DomainMatcher::new("!example.com/api/*");
        assert!(matcher.is_negated());

        let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
        assert!(!result.matched);

        let result = matcher.matches("http://example.com/other", "example.com", "/other");
        assert!(result.matched);
    }

    #[test]
    fn test_full_url_pattern() {
        let matcher = DomainMatcher::new("https://example.com:8443/api/*");

        let result = matcher.matches(
            "https://example.com:8443/api/users",
            "example.com:8443",
            "/api/users",
        );
        assert!(result.matched);

        let result = matcher.matches(
            "http://example.com:8443/api/users",
            "example.com:8443",
            "/api/users",
        );
        assert!(!result.matched);
    }

    #[test]
    fn test_priority_basic() {
        let basic = DomainMatcher::new("example.com");
        assert_eq!(basic.priority(), 100);
    }

    #[test]
    fn test_priority_with_protocol() {
        let with_protocol = DomainMatcher::new("https://example.com");
        assert_eq!(with_protocol.priority(), 105);
    }

    #[test]
    fn test_priority_with_port() {
        let with_port = DomainMatcher::new("example.com:8080");
        assert_eq!(with_port.priority(), 110);
    }

    #[test]
    fn test_priority_with_exact_path() {
        let with_path = DomainMatcher::new("example.com/api/users");
        assert_eq!(with_path.priority(), 115);
    }

    #[test]
    fn test_priority_with_path_prefix() {
        let with_prefix = DomainMatcher::new("example.com/api/*");
        assert_eq!(with_prefix.priority(), 110);
    }

    #[test]
    fn test_priority_full() {
        let full = DomainMatcher::new("https://example.com:8443/api/users");
        assert_eq!(full.priority(), 130);
    }

    #[test]
    fn test_raw_pattern() {
        let pattern = "https://example.com:8080/api/*";
        let matcher = DomainMatcher::new(pattern);
        assert_eq!(matcher.raw_pattern(), pattern);
    }

    #[test]
    fn test_domain_accessor() {
        let matcher = DomainMatcher::new("example.com:8080/api/*");
        assert_eq!(matcher.domain(), "example.com");
    }

    #[test]
    fn test_subdomain() {
        let matcher = DomainMatcher::new("api.example.com");

        let result = matcher.matches("http://api.example.com/path", "api.example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://www.example.com/path", "www.example.com", "/path");
        assert!(!result.matched);

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_root_path() {
        let matcher = DomainMatcher::new("example.com/");

        let result = matcher.matches("http://example.com/", "example.com", "/");
        assert!(result.matched);
    }

    #[test]
    fn test_path_with_query() {
        let matcher = DomainMatcher::new("example.com/search");

        let result = matcher.matches(
            "http://example.com/search?q=test",
            "example.com",
            "/search?q=test",
        );
        assert!(result.matched);
    }

    #[test]
    fn test_port_wildcard_simple() {
        let matcher = DomainMatcher::new("example.com:8*8");
        assert!(matcher.has_port_pattern());

        let result = matcher.matches("http://example.com:88/path", "example.com:88", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com:808/path", "example.com:808", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com:8888/path", "example.com:8888", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com:8008/path", "example.com:8008", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com:80/path", "example.com:80", "/path");
        assert!(!result.matched);

        let result = matcher.matches("http://example.com:9999/path", "example.com:9999", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_port_wildcard_prefix() {
        let matcher = DomainMatcher::new("example.com:8*");

        let result = matcher.matches("http://example.com:80/path", "example.com:80", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com:8080/path", "example.com:8080", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com:8888/path", "example.com:8888", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com:9000/path", "example.com:9000", "/path");
        assert!(!result.matched);

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(result.matched, "HTTP default port 80 matches 8*");

        let result = matcher.matches("https://example.com/path", "example.com", "/path");
        assert!(!result.matched, "HTTPS default port 443 does not match 8*");
    }

    #[test]
    fn test_port_wildcard_suffix() {
        let matcher = DomainMatcher::new("example.com:*80");

        let result = matcher.matches("http://example.com:80/path", "example.com:80", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com:180/path", "example.com:180", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com:8080/path", "example.com:8080", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com:8081/path", "example.com:8081", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_port_wildcard_all() {
        let matcher = DomainMatcher::new("example.com:*");

        let result = matcher.matches("http://example.com:80/path", "example.com:80", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com:443/path", "example.com:443", "/path");
        assert!(result.matched);

        let result = matcher.matches(
            "http://example.com:12345/path",
            "example.com:12345",
            "/path",
        );
        assert!(result.matched);

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(result.matched, "Should match default port 80 via wildcard");

        let result = matcher.matches("https://example.com/path", "example.com", "/path");
        assert!(result.matched, "Should match default port 443 via wildcard");
    }

    #[test]
    fn test_port_wildcard_with_path() {
        let matcher = DomainMatcher::new("example.com:8*/api/*");
        assert!(matcher.has_port_pattern());

        let result = matcher.matches(
            "http://example.com:8080/api/users",
            "example.com:8080",
            "/api/users",
        );
        assert!(result.matched);

        let result = matcher.matches(
            "http://example.com:9090/api/users",
            "example.com:9090",
            "/api/users",
        );
        assert!(!result.matched);
    }

    #[test]
    fn test_protocol_ws() {
        let matcher = DomainMatcher::new("ws://example.com/socket");

        let result = matcher.matches("ws://example.com/socket", "example.com", "/socket");
        assert!(result.matched);

        let result = matcher.matches("wss://example.com/socket", "example.com", "/socket");
        assert!(!result.matched);

        let result = matcher.matches("http://example.com/socket", "example.com", "/socket");
        assert!(!result.matched);
    }

    #[test]
    fn test_protocol_wss() {
        let matcher = DomainMatcher::new("wss://example.com/socket");

        let result = matcher.matches("wss://example.com/socket", "example.com", "/socket");
        assert!(result.matched);

        let result = matcher.matches("ws://example.com/socket", "example.com", "/socket");
        assert!(!result.matched);
    }

    #[test]
    fn test_protocol_tunnel() {
        let matcher = DomainMatcher::new("tunnel://example.com");

        let result = matcher.matches("tunnel://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_protocol_http_wildcard() {
        let matcher = DomainMatcher::new("http*://example.com");

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("https://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("ws://example.com/path", "example.com", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_protocol_ws_wildcard() {
        let matcher = DomainMatcher::new("ws*://example.com");

        let result = matcher.matches("ws://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("wss://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_protocol_any() {
        let matcher = DomainMatcher::new("//example.com");

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("https://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("ws://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("wss://example.com/path", "example.com", "/path");
        assert!(result.matched);

        let result = matcher.matches("tunnel://example.com/path", "example.com", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_protocol_any_with_path() {
        let matcher = DomainMatcher::new("//example.com/api/*");

        let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);

        let result = matcher.matches("wss://example.com/api/users", "example.com", "/api/users");
        assert!(result.matched);
    }
}
