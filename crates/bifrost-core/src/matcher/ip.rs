use std::net::IpAddr;
use std::str::FromStr;

use ipnet::IpNet;

use super::{MatchResult, Matcher};

pub struct IpMatcher {
    ip_net: IpNet,
    negated: bool,
    raw_pattern: String,
    is_exact: bool,
}

impl IpMatcher {
    pub fn new(pattern: &str) -> Result<Self, IpMatcherError> {
        let (negated, clean_pattern) = Self::parse_negation(pattern);
        let (ip_net, is_exact) = Self::parse_ip_pattern(clean_pattern)?;

        Ok(Self {
            ip_net,
            negated,
            raw_pattern: pattern.to_string(),
            is_exact,
        })
    }

    fn parse_negation(pattern: &str) -> (bool, &str) {
        if let Some(stripped) = pattern.strip_prefix('!') {
            (true, stripped)
        } else {
            (false, pattern)
        }
    }

    fn parse_ip_pattern(pattern: &str) -> Result<(IpNet, bool), IpMatcherError> {
        if pattern.contains('/') {
            let ip_net = pattern
                .parse::<IpNet>()
                .map_err(|_| IpMatcherError::InvalidCidr(pattern.to_string()))?;
            Ok((ip_net, false))
        } else {
            let ip = pattern
                .parse::<IpAddr>()
                .map_err(|_| IpMatcherError::InvalidIp(pattern.to_string()))?;

            let ip_net = match ip {
                IpAddr::V4(v4) => IpNet::from_str(&format!("{}/32", v4))
                    .map_err(|_| IpMatcherError::InvalidIp(pattern.to_string()))?,
                IpAddr::V6(v6) => IpNet::from_str(&format!("{}/128", v6))
                    .map_err(|_| IpMatcherError::InvalidIp(pattern.to_string()))?,
            };
            Ok((ip_net, true))
        }
    }

    pub fn raw_pattern(&self) -> &str {
        &self.raw_pattern
    }

    pub fn is_exact(&self) -> bool {
        self.is_exact
    }

    pub fn network(&self) -> &IpNet {
        &self.ip_net
    }

    fn extract_ip_from_host(host: &str) -> Option<IpAddr> {
        if host.starts_with('[') {
            if let Some(end) = host.find(']') {
                let ipv6_str = &host[1..end];
                return ipv6_str.parse::<IpAddr>().ok();
            }
        }

        if host.contains(':') && !host.starts_with('[') {
            let colon_count = host.matches(':').count();
            if colon_count == 1 {
                let clean_host = host.split(':').next().unwrap_or(host);
                return clean_host.parse::<IpAddr>().ok();
            } else {
                return host.parse::<IpAddr>().ok();
            }
        }

        host.parse::<IpAddr>().ok()
    }
}

impl Matcher for IpMatcher {
    fn matches(&self, _url: &str, host: &str, _path: &str) -> MatchResult {
        let ip = match Self::extract_ip_from_host(host) {
            Some(ip) => ip,
            None => {
                return if self.negated {
                    MatchResult::matched()
                } else {
                    MatchResult::not_matched()
                }
            }
        };

        let is_match = self.ip_net.contains(&ip);
        let effective_match = if self.negated { !is_match } else { is_match };

        if effective_match {
            MatchResult::matched()
        } else {
            MatchResult::not_matched()
        }
    }

    fn is_negated(&self) -> bool {
        self.negated
    }

    fn priority(&self) -> i32 {
        if self.is_exact {
            95
        } else {
            let prefix_len = self.ip_net.prefix_len();
            70 + (prefix_len as i32 / 4)
        }
    }

    fn can_trigger_tls_auto_intercept(&self) -> bool {
        !self.negated
    }
}

#[derive(Debug, Clone)]
pub enum IpMatcherError {
    InvalidIp(String),
    InvalidCidr(String),
}

impl std::fmt::Display for IpMatcherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpMatcherError::InvalidIp(s) => write!(f, "Invalid IP address: {}", s),
            IpMatcherError::InvalidCidr(s) => write!(f, "Invalid CIDR notation: {}", s),
        }
    }
}

impl std::error::Error for IpMatcherError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_ipv4() {
        let matcher = IpMatcher::new("192.168.1.1").unwrap();
        assert!(matcher.is_exact());

        let result = matcher.matches("http://192.168.1.1/path", "192.168.1.1", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://192.168.1.2/path", "192.168.1.2", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_exact_ipv4_with_port() {
        let matcher = IpMatcher::new("192.168.1.1").unwrap();

        let result = matcher.matches("http://192.168.1.1:8080/path", "192.168.1.1:8080", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_cidr_ipv4_24() {
        let matcher = IpMatcher::new("192.168.1.0/24").unwrap();
        assert!(!matcher.is_exact());

        let result = matcher.matches("http://192.168.1.1/path", "192.168.1.1", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://192.168.1.254/path", "192.168.1.254", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://192.168.2.1/path", "192.168.2.1", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_cidr_ipv4_16() {
        let matcher = IpMatcher::new("192.168.0.0/16").unwrap();

        let result = matcher.matches("http://192.168.1.1/path", "192.168.1.1", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://192.168.255.255/path", "192.168.255.255", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://192.169.0.1/path", "192.169.0.1", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_cidr_ipv4_8() {
        let matcher = IpMatcher::new("10.0.0.0/8").unwrap();

        let result = matcher.matches("http://10.0.0.1/path", "10.0.0.1", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://10.255.255.255/path", "10.255.255.255", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://11.0.0.1/path", "11.0.0.1", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_exact_ipv6() {
        let matcher = IpMatcher::new("::1").unwrap();
        assert!(matcher.is_exact());

        let result = matcher.matches("http://[::1]/path", "::1", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://[::1]/path", "[::1]", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_cidr_ipv6() {
        let matcher = IpMatcher::new("2001:db8::/32").unwrap();

        let result = matcher.matches("http://[2001:db8::1]/path", "2001:db8::1", "/path");
        assert!(result.matched);

        let result = matcher.matches(
            "http://[2001:db8:ffff::1]/path",
            "2001:db8:ffff::1",
            "/path",
        );
        assert!(result.matched);

        let result = matcher.matches("http://[2001:db9::1]/path", "2001:db9::1", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_negated_exact_ip() {
        let matcher = IpMatcher::new("!192.168.1.1").unwrap();
        assert!(matcher.is_negated());

        let result = matcher.matches("http://192.168.1.1/path", "192.168.1.1", "/path");
        assert!(!result.matched);

        let result = matcher.matches("http://192.168.1.2/path", "192.168.1.2", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_negated_cidr() {
        let matcher = IpMatcher::new("!192.168.0.0/16").unwrap();
        assert!(matcher.is_negated());

        let result = matcher.matches("http://192.168.1.1/path", "192.168.1.1", "/path");
        assert!(!result.matched);

        let result = matcher.matches("http://10.0.0.1/path", "10.0.0.1", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_localhost_ipv4() {
        let matcher = IpMatcher::new("127.0.0.0/8").unwrap();

        let result = matcher.matches("http://127.0.0.1/path", "127.0.0.1", "/path");
        assert!(result.matched);

        let result = matcher.matches("http://127.0.0.2/path", "127.0.0.2", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_non_ip_host() {
        let matcher = IpMatcher::new("192.168.1.1").unwrap();

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(!result.matched);
    }

    #[test]
    fn test_negated_non_ip_host() {
        let matcher = IpMatcher::new("!192.168.1.1").unwrap();

        let result = matcher.matches("http://example.com/path", "example.com", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_priority_exact() {
        let matcher = IpMatcher::new("192.168.1.1").unwrap();
        assert_eq!(matcher.priority(), 95);
    }

    #[test]
    fn test_priority_cidr_24() {
        let matcher = IpMatcher::new("192.168.1.0/24").unwrap();
        assert_eq!(matcher.priority(), 76);
    }

    #[test]
    fn test_priority_cidr_16() {
        let matcher = IpMatcher::new("192.168.0.0/16").unwrap();
        assert_eq!(matcher.priority(), 74);
    }

    #[test]
    fn test_priority_cidr_8() {
        let matcher = IpMatcher::new("10.0.0.0/8").unwrap();
        assert_eq!(matcher.priority(), 72);
    }

    #[test]
    fn test_raw_pattern() {
        let pattern = "192.168.0.0/16";
        let matcher = IpMatcher::new(pattern).unwrap();
        assert_eq!(matcher.raw_pattern(), pattern);
    }

    #[test]
    fn test_invalid_ip() {
        let result = IpMatcher::new("invalid.ip");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_cidr() {
        let result = IpMatcher::new("192.168.1.0/33");
        assert!(result.is_err());
    }

    #[test]
    fn test_private_networks() {
        let class_a = IpMatcher::new("10.0.0.0/8").unwrap();
        assert!(class_a.matches("", "10.1.2.3", "").matched);

        let class_b = IpMatcher::new("172.16.0.0/12").unwrap();
        assert!(class_b.matches("", "172.16.0.1", "").matched);
        assert!(class_b.matches("", "172.31.255.255", "").matched);
        assert!(!class_b.matches("", "172.32.0.1", "").matched);

        let class_c = IpMatcher::new("192.168.0.0/16").unwrap();
        assert!(class_c.matches("", "192.168.1.1", "").matched);
    }

    #[test]
    fn test_ipv6_with_brackets() {
        let matcher = IpMatcher::new("::1").unwrap();

        let result = matcher.matches("http://[::1]:8080/path", "[::1]:8080", "/path");
        assert!(result.matched);
    }

    #[test]
    fn test_network_accessor() {
        let matcher = IpMatcher::new("192.168.0.0/16").unwrap();
        assert_eq!(matcher.network().prefix_len(), 16);
    }
}
