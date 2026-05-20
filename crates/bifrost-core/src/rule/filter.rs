use regex::Regex;
use std::net::IpAddr;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub enum Filter {
    Method(Vec<String>),
    StatusCode(StatusCodeRange),
    Path(PathMatcher),
    HeaderExists(String),
    HeaderMatch {
        name: String,
        pattern: Regex,
        is_request: bool,
    },
    ClientIp(IpMatcher),
    Body(Regex),
    Url(UrlMatcher),
    Custom(String, String),
}

#[derive(Debug, Clone)]
pub enum StatusCodeRange {
    Single(u16),
    Range(u16, u16),
    List(Vec<u16>),
}

impl StatusCodeRange {
    pub fn matches(&self, status: u16) -> bool {
        match self {
            StatusCodeRange::Single(s) => *s == status,
            StatusCodeRange::Range(start, end) => status >= *start && status <= *end,
            StatusCodeRange::List(list) => list.contains(&status),
        }
    }
}

#[derive(Debug, Clone)]
pub enum PathMatcher {
    Exact(String),
    Prefix(String),
    Contains(String),
    Regex(Regex),
}

impl PathMatcher {
    pub fn matches(&self, path: &str) -> bool {
        match self {
            PathMatcher::Exact(s) => path == s,
            PathMatcher::Prefix(s) => path.starts_with(s),
            PathMatcher::Contains(s) => path.contains(s),
            PathMatcher::Regex(r) => r.is_match(path),
        }
    }
}

#[derive(Debug, Clone)]
pub enum UrlMatcher {
    Contains(String),
    HostPath { host: String, path: Option<String> },
}

impl UrlMatcher {
    pub fn matches(&self, url: &str, host: &str, path: &str) -> bool {
        match self {
            UrlMatcher::Contains(s) => url.contains(s.as_str()),
            UrlMatcher::HostPath {
                host: filter_host,
                path: filter_path,
            } => {
                let host_match =
                    host == filter_host || host.starts_with(&format!("{}:", filter_host));
                if !host_match {
                    return false;
                }
                if let Some(fp) = filter_path {
                    path.starts_with(fp)
                } else {
                    true
                }
            }
        }
    }
}

pub fn parse_url_filter(s: &str) -> Option<UrlMatcher> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(pos) = s.find('/') {
        let host = &s[..pos];
        let path = &s[pos..];
        if !host.is_empty() {
            return Some(UrlMatcher::HostPath {
                host: host.to_string(),
                path: Some(path.to_string()),
            });
        }
    }
    Some(UrlMatcher::HostPath {
        host: s.to_string(),
        path: None,
    })
}

#[derive(Debug, Clone)]
pub enum IpMatcher {
    Exact(IpAddr),
    Cidr { addr: IpAddr, prefix_len: u8 },
}

impl IpMatcher {
    pub fn matches(&self, ip: &str) -> bool {
        let Ok(ip_addr) = IpAddr::from_str(ip) else {
            return false;
        };

        match self {
            IpMatcher::Exact(addr) => ip_addr == *addr,
            IpMatcher::Cidr { addr, prefix_len } => match (addr, &ip_addr) {
                (IpAddr::V4(cidr_addr), IpAddr::V4(target_addr)) => {
                    let cidr_bits = u32::from(*cidr_addr);
                    let target_bits = u32::from(*target_addr);
                    let mask = if *prefix_len == 0 {
                        0
                    } else {
                        !0u32 << (32 - prefix_len)
                    };
                    (cidr_bits & mask) == (target_bits & mask)
                }
                (IpAddr::V6(cidr_addr), IpAddr::V6(target_addr)) => {
                    let cidr_bits = u128::from(*cidr_addr);
                    let target_bits = u128::from(*target_addr);
                    let mask = if *prefix_len == 0 {
                        0
                    } else {
                        !0u128 << (128 - prefix_len)
                    };
                    (cidr_bits & mask) == (target_bits & mask)
                }
                _ => false,
            },
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LineProps {
    pub important: bool,
    pub disabled: bool,
}

impl LineProps {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_important(mut self, important: bool) -> Self {
        self.important = important;
        self
    }

    pub fn with_disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

pub fn parse_filter(filter_str: &str) -> Option<Filter> {
    let filter_str = filter_str.trim();

    if filter_str.starts_with("m:") || filter_str.starts_with("M:") {
        let methods: Vec<String> = filter_str[2..]
            .split(',')
            .map(|s| s.trim().to_uppercase())
            .collect();
        return Some(Filter::Method(methods));
    }

    if filter_str.starts_with("s:") || filter_str.starts_with("S:") {
        let status_str = &filter_str[2..];
        if let Some(status_range) = parse_status_code_range(status_str) {
            return Some(Filter::StatusCode(status_range));
        }
    }

    if filter_str.starts_with("h:") || filter_str.starts_with("H:") {
        let header_filter = filter_str[2..].trim();
        if let Some((name, pattern)) = parse_header_match(header_filter) {
            return Some(Filter::HeaderMatch {
                name,
                pattern,
                is_request: true,
            });
        }
        return Some(Filter::HeaderExists(header_filter.to_string()));
    }

    if filter_str.starts_with("reqH:") || filter_str.starts_with("reqh:") {
        if let Some((name, pattern)) = parse_header_match(&filter_str[5..]) {
            return Some(Filter::HeaderMatch {
                name,
                pattern,
                is_request: true,
            });
        }
    }

    if filter_str.starts_with("resH:") || filter_str.starts_with("resh:") {
        if let Some((name, pattern)) = parse_header_match(&filter_str[5..]) {
            return Some(Filter::HeaderMatch {
                name,
                pattern,
                is_request: false,
            });
        }
    }

    if filter_str.starts_with("i:") || filter_str.starts_with("I:") {
        if let Some(ip_matcher) = parse_ip_matcher(&filter_str[2..]) {
            return Some(Filter::ClientIp(ip_matcher));
        }
    }

    if filter_str.starts_with("b:") || filter_str.starts_with("B:") {
        let body_pattern = &filter_str[2..];
        if let Some(regex) = parse_regex_pattern(body_pattern) {
            return Some(Filter::Body(regex));
        }
    }

    if filter_str.starts_with('/') && filter_str.ends_with('/') && filter_str.len() > 2 {
        let pattern = &filter_str[1..filter_str.len() - 1];
        if let Ok(regex) = Regex::new(pattern) {
            return Some(Filter::Path(PathMatcher::Regex(regex)));
        }
    }

    if filter_str.starts_with('/') {
        return Some(Filter::Path(PathMatcher::Prefix(filter_str.to_string())));
    }

    if looks_like_url_pattern(filter_str) {
        if let Some(url_matcher) = parse_url_filter(filter_str) {
            return Some(Filter::Url(url_matcher));
        }
    }

    None
}

fn parse_status_code_range(s: &str) -> Option<StatusCodeRange> {
    if s.contains('-') {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() == 2 {
            if let (Ok(start), Ok(end)) = (parts[0].parse::<u16>(), parts[1].parse::<u16>()) {
                return Some(StatusCodeRange::Range(start, end));
            }
        }
    } else if s.contains(',') {
        let codes: Result<Vec<u16>, _> = s.split(',').map(|p| p.trim().parse::<u16>()).collect();
        if let Ok(codes) = codes {
            return Some(StatusCodeRange::List(codes));
        }
    } else if let Ok(code) = s.parse::<u16>() {
        return Some(StatusCodeRange::Single(code));
    }
    None
}

fn parse_header_match(s: &str) -> Option<(String, Regex)> {
    if let Some(eq_pos) = s.find('=') {
        let name = s[..eq_pos].trim().to_string();
        let pattern_str = s[eq_pos + 1..].trim();

        let regex = parse_regex_pattern(pattern_str)
            .or_else(|| Regex::new(&regex::escape(pattern_str)).ok())?;

        return Some((name, regex));
    }
    None
}

fn parse_regex_pattern(s: &str) -> Option<Regex> {
    if s.starts_with('/') && s.ends_with('/') && s.len() > 2 {
        let pattern = &s[1..s.len() - 1];
        return Regex::new(pattern).ok();
    }
    None
}

fn looks_like_url_pattern(s: &str) -> bool {
    s.contains('.')
}

fn parse_ip_matcher(s: &str) -> Option<IpMatcher> {
    if s.contains('/') {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() == 2 {
            if let (Ok(addr), Ok(prefix_len)) = (IpAddr::from_str(parts[0]), parts[1].parse::<u8>())
            {
                return Some(IpMatcher::Cidr { addr, prefix_len });
            }
        }
    } else if let Ok(addr) = IpAddr::from_str(s) {
        return Some(IpMatcher::Exact(addr));
    }
    None
}

pub fn parse_line_props(value: &str) -> LineProps {
    let mut props = LineProps::new();
    for part in value.split(',') {
        let part = part.trim().to_lowercase();
        match part.as_str() {
            "important" => props.important = true,
            "disabled" => props.disabled = true,
            _ => {}
        }
    }
    props
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_method_filter() {
        let filter = parse_filter("m:GET").unwrap();
        if let Filter::Method(methods) = filter {
            assert_eq!(methods, vec!["GET"]);
        } else {
            panic!("Expected Method filter");
        }

        let filter = parse_filter("m:GET,POST,PUT").unwrap();
        if let Filter::Method(methods) = filter {
            assert_eq!(methods, vec!["GET", "POST", "PUT"]);
        } else {
            panic!("Expected Method filter");
        }
    }

    #[test]
    fn test_parse_status_code_filter() {
        let filter = parse_filter("s:200").unwrap();
        if let Filter::StatusCode(range) = filter {
            assert!(range.matches(200));
            assert!(!range.matches(201));
        } else {
            panic!("Expected StatusCode filter");
        }

        let filter = parse_filter("s:200-299").unwrap();
        if let Filter::StatusCode(range) = filter {
            assert!(range.matches(200));
            assert!(range.matches(250));
            assert!(range.matches(299));
            assert!(!range.matches(300));
        } else {
            panic!("Expected StatusCode filter");
        }

        let filter = parse_filter("s:200,404,500").unwrap();
        if let Filter::StatusCode(range) = filter {
            assert!(range.matches(200));
            assert!(range.matches(404));
            assert!(range.matches(500));
            assert!(!range.matches(201));
        } else {
            panic!("Expected StatusCode filter");
        }
    }

    #[test]
    fn test_parse_header_exists_filter() {
        let filter = parse_filter("h:X-Custom-Header").unwrap();
        if let Filter::HeaderExists(name) = filter {
            assert_eq!(name, "X-Custom-Header");
        } else {
            panic!("Expected HeaderExists filter");
        }
    }

    #[test]
    fn test_parse_header_match_filter() {
        let filter = parse_filter("reqH:Content-Type=/json/").unwrap();
        if let Filter::HeaderMatch {
            name,
            pattern,
            is_request,
        } = filter
        {
            assert_eq!(name, "Content-Type");
            assert!(pattern.is_match("application/json"));
            assert!(is_request);
        } else {
            panic!("Expected HeaderMatch filter");
        }
    }

    #[test]
    fn test_parse_ip_filter() {
        let filter = parse_filter("i:127.0.0.1").unwrap();
        if let Filter::ClientIp(matcher) = filter {
            assert!(matcher.matches("127.0.0.1"));
            assert!(!matcher.matches("127.0.0.2"));
        } else {
            panic!("Expected ClientIp filter");
        }

        let filter = parse_filter("i:192.168.0.0/16").unwrap();
        if let Filter::ClientIp(matcher) = filter {
            assert!(matcher.matches("192.168.1.1"));
            assert!(matcher.matches("192.168.255.255"));
            assert!(!matcher.matches("192.169.0.1"));
        } else {
            panic!("Expected ClientIp filter");
        }
    }

    #[test]
    fn test_parse_path_filter() {
        let filter = parse_filter("/api/").unwrap();
        if let Filter::Path(matcher) = filter {
            assert!(matcher.matches("/api/users"));
            assert!(matcher.matches("/v1/api/data"));
            assert!(!matcher.matches("/home"));
        } else {
            panic!("Expected Path filter");
        }

        let filter = parse_filter("/^/api/v\\d+/").unwrap();
        if let Filter::Path(PathMatcher::Regex(regex)) = filter {
            assert!(regex.is_match("/api/v1"));
            assert!(regex.is_match("/api/v2"));
            assert!(!regex.is_match("/api/users"));
        } else {
            panic!("Expected Path Regex filter");
        }
    }

    #[test]
    fn test_parse_plain_path_filter_matches_prefix() {
        let filter = parse_filter("/account").unwrap();
        if let Filter::Path(matcher) = filter {
            assert!(matcher.matches("/account"));
            assert!(matcher.matches("/account/profile"));
            assert!(matcher.matches("/account?tab=security"));
            assert!(matcher.matches("/account-center/account-assistant"));
            assert!(!matcher.matches("/v1/account"));
        } else {
            panic!("Expected Path filter");
        }
    }

    #[test]
    fn test_parse_body_filter() {
        let filter = parse_filter("b:/error/").unwrap();
        if let Filter::Body(regex) = filter {
            assert!(regex.is_match("error occurred"));
            assert!(!regex.is_match("success"));
        } else {
            panic!("Expected Body filter");
        }
    }

    #[test]
    fn test_parse_line_props() {
        let props = parse_line_props("important");
        assert!(props.important);
        assert!(!props.disabled);

        let props = parse_line_props("important,disabled");
        assert!(props.important);
        assert!(props.disabled);

        let props = parse_line_props("IMPORTANT");
        assert!(props.important);
    }

    #[test]
    fn test_line_props_builder() {
        let props = LineProps::new().with_important(true).with_disabled(false);
        assert!(props.important);
        assert!(!props.disabled);
    }

    #[test]
    fn test_parse_url_filter_domain_only() {
        let filter = parse_filter("m.bifrost.local").unwrap();
        if let Filter::Url(matcher) = filter {
            assert!(matcher.matches("https://m.bifrost.local/page", "m.bifrost.local", "/page"));
            assert!(!matcher.matches(
                "https://other.example.com/page",
                "other.example.com",
                "/page"
            ));
        } else {
            panic!("Expected Url filter");
        }
    }

    #[test]
    fn test_parse_url_filter_domain_with_path() {
        let filter = parse_filter("m.bifrost.local/api").unwrap();
        if let Filter::Url(matcher) = filter {
            assert!(matcher.matches(
                "https://m.bifrost.local/api/v1",
                "m.bifrost.local",
                "/api/v1"
            ));
            assert!(matcher.matches(
                "https://m.bifrost.local/api?debug=1",
                "m.bifrost.local",
                "/api?debug=1"
            ));
            assert!(matcher.matches(
                "https://m.bifrost.local/apiary",
                "m.bifrost.local",
                "/apiary"
            ));
            assert!(!matcher.matches("https://m.bifrost.local/page", "m.bifrost.local", "/page"));
            assert!(!matcher.matches("https://other.example.com/api", "other.example.com", "/api"));
        } else {
            panic!("Expected Url filter");
        }
    }

    #[test]
    fn test_parse_url_filter_domain_with_deep_path() {
        let filter = parse_filter("m.bifrost.local/mira/api").unwrap();
        if let Filter::Url(matcher) = filter {
            assert!(matcher.matches(
                "https://m.bifrost.local/mira/api/endpoint",
                "m.bifrost.local",
                "/mira/api/endpoint"
            ));
            assert!(!matcher.matches("https://m.bifrost.local/other", "m.bifrost.local", "/other"));
        } else {
            panic!("Expected Url filter");
        }
    }
}
