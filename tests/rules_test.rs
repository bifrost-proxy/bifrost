use bifrost_core::protocol::{protocol_aliases, Protocol, ALL_PROTOCOLS, MULTI_MATCH_PROTOCOLS};
use bifrost_core::rule::{parse_line, parse_rules, RuleParser};
use bifrost_core::{DomainMatcher, MatchResult, Matcher, RegexMatcher, WildcardMatcher};

#[test]
fn test_all_protocols() {
    assert_eq!(ALL_PROTOCOLS.len(), 72, "Should have exactly 72 protocols");

    let protocol_names = [
        "host",
        "xhost",
        "http",
        "https",
        "ws",
        "wss",
        "proxy",
        "http3",
        "pac",
        "decode",
        "redirect",
        "file",
        "tpl",
        "rawfile",
        "skip",
        "referer",
        "auth",
        "ua",
        "urlParams",
        "params",
        "resMerge",
        "replaceStatus",
        "statusCode",
        "method",
        "cache",
        "attachment",
        "resScript",
        "reqDelay",
        "resDelay",
        "headerReplace",
        "reqSpeed",
        "resSpeed",
        "reqType",
        "resType",
        "reqCharset",
        "resCharset",
        "reqCookies",
        "resCookies",
        "forwardedFor",
        "responseFor",
        "reqCors",
        "resCors",
        "reqHeaders",
        "resHeaders",
        "trailers",
        "reqPrepend",
        "resPrepend",
        "reqBody",
        "resBody",
        "reqAppend",
        "resAppend",
        "urlReplace",
        "reqReplace",
        "resReplace",
        "cssAppend",
        "htmlAppend",
        "jsAppend",
        "cssBody",
        "htmlBody",
        "jsBody",
        "cssPrepend",
        "htmlPrepend",
        "jsPrepend",
        "dns",
        "tlsIntercept",
        "tlsPassthrough",
        "tlsOptions",
        "sniCallback",
        "passthrough",
        "tunnel",
        "delete",
        "reqScript",
    ];

    for name in &protocol_names {
        let protocol = Protocol::parse(name);
        assert!(
            protocol.is_some(),
            "Protocol '{}' should be parseable",
            name
        );
    }

    assert_eq!(
        protocol_names.len(),
        72,
        "Test should cover all 72 protocols"
    );
}

#[test]
fn test_protocol_categories() {
    assert!(Protocol::ReqHeaders.is_req_protocol());
    assert!(!Protocol::ReqHeaders.is_res_protocol());

    assert!(Protocol::ResHeaders.is_res_protocol());
    assert!(!Protocol::ResHeaders.is_req_protocol());

    assert!(Protocol::Host.is_req_protocol());
    assert!(Protocol::Proxy.is_req_protocol());
}

#[test]
fn test_protocol_aliases() {
    let aliases = protocol_aliases();

    assert_eq!(aliases.get("hosts"), Some(&"host"));
    assert_eq!(aliases.get("status"), Some(&"statusCode"));
    assert_eq!(aliases.get("download"), Some(&"attachment"));
    assert_eq!(aliases.get("h3"), Some(&"http3"));
    assert_eq!(aliases.get("html"), Some(&"htmlAppend"));
    assert_eq!(aliases.get("js"), Some(&"jsAppend"));
    assert_eq!(aliases.get("css"), Some(&"cssAppend"));
}

#[test]
fn test_multi_match_protocols() {
    assert!(MULTI_MATCH_PROTOCOLS.contains(&Protocol::ReqHeaders));
    assert!(MULTI_MATCH_PROTOCOLS.contains(&Protocol::ResHeaders));
    assert!(MULTI_MATCH_PROTOCOLS.contains(&Protocol::ReqCookies));
    assert!(MULTI_MATCH_PROTOCOLS.contains(&Protocol::Delete));

    assert!(!MULTI_MATCH_PROTOCOLS.contains(&Protocol::Host));
    assert!(!MULTI_MATCH_PROTOCOLS.contains(&Protocol::Proxy));
}

#[test]
fn test_regex_matching() {
    let matcher = RegexMatcher::new("/^https?://api\\.example\\.com/v\\d+/.*$/").unwrap();

    let result = matcher.matches(
        "https://api.example.com/v1/users",
        "api.example.com",
        "/v1/users",
    );
    assert!(result.matched);

    let result = matcher.matches(
        "http://api.example.com/v2/data",
        "api.example.com",
        "/v2/data",
    );
    assert!(result.matched);

    let result = matcher.matches(
        "https://other.example.com/v1/users",
        "other.example.com",
        "/v1/users",
    );
    assert!(!result.matched);
}

#[test]
fn test_regex_case_insensitive() {
    let matcher = RegexMatcher::new("/example\\.com/i").unwrap();

    let result = matcher.matches("https://EXAMPLE.COM/path", "EXAMPLE.COM", "/path");
    assert!(result.matched);

    let result = matcher.matches("https://Example.Com/path", "Example.Com", "/path");
    assert!(result.matched);
}

#[test]
fn test_regex_with_captures() {
    let matcher = RegexMatcher::new("/api/(\\w+)/(\\d+)/").unwrap();

    let result = matcher.matches(
        "http://example.com/api/users/123",
        "example.com",
        "/api/users/123",
    );
    assert!(result.matched);
    assert!(result.captures.is_some());

    let caps = result.captures.unwrap();
    assert_eq!(caps[0], "users");
    assert_eq!(caps[1], "123");
}

#[test]
fn test_wildcard_matching() {
    let rules = parse_line("*.example.com host://127.0.0.1").unwrap();
    assert_eq!(rules.len(), 1);

    let matcher = &rules[0].matcher;
    let result = matcher.matches("http://www.example.com/path", "www.example.com", "/path");
    assert!(result.matched);

    let result = matcher.matches("http://api.example.com/path", "api.example.com", "/path");
    assert!(result.matched);
}

#[test]
fn test_wildcard_double_star() {
    let rules = parse_line("**.example.com host://127.0.0.1").unwrap();
    let matcher = &rules[0].matcher;

    let result = matcher.matches("http://www.example.com/path", "www.example.com", "/path");
    assert!(result.matched);

    let result = matcher.matches(
        "http://api.sub.example.com/path",
        "api.sub.example.com",
        "/path",
    );
    assert!(result.matched);
}

#[test]
fn test_wildcard_path_matching() {
    let rules = parse_line("example.com/api/* host://127.0.0.1").unwrap();
    let matcher = &rules[0].matcher;

    let result = matcher.matches("http://example.com/api/users", "example.com", "/api/users");
    assert!(result.matched);

    let result = matcher.matches("http://example.com/api/data", "example.com", "/api/data");
    assert!(result.matched);
}

#[test]
fn test_rule_priority() {
    let rules_text = r#"
*.example.com host://wildcard.local
www.example.com host://specific.local
example.com host://exact.local
"#;

    let rules = parse_rules(rules_text).unwrap();
    assert_eq!(rules.len(), 3);
}

#[test]
fn test_exact_domain_priority() {
    let exact_rule = parse_line("example.com host://127.0.0.1").unwrap();
    let wildcard_rule = parse_line("*.example.com host://127.0.0.1").unwrap();

    assert!(exact_rule[0].priority() >= wildcard_rule[0].priority());
}

#[test]
fn test_cidr_matching() {
    let rules = parse_line("192.168.0.0/16 proxy://proxy.local:8080").unwrap();
    let matcher = &rules[0].matcher;

    let result = matcher.matches("http://192.168.1.1/", "192.168.1.1", "/");
    assert!(result.matched);

    let result = matcher.matches("http://192.168.255.255/", "192.168.255.255", "/");
    assert!(result.matched);
}

#[test]
fn test_ip_exact_matching() {
    let rules = parse_line("192.168.1.100 delete://").unwrap();
    let matcher = &rules[0].matcher;

    let result = matcher.matches("http://192.168.1.100/", "192.168.1.100", "/");
    assert!(result.matched);
}

#[test]
fn test_parse_simple_rule() {
    let rules = parse_line("example.com host://127.0.0.1").unwrap();

    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].pattern, "example.com");
    assert_eq!(rules[0].protocol, Protocol::Host);
    assert_eq!(rules[0].value, "127.0.0.1");
}

#[test]
fn test_parse_multi_protocol_rule() {
    let rules =
        parse_line("example.com host://127.0.0.1 reqHeaders://{test=1} reqDelay://1000").unwrap();

    assert_eq!(rules.len(), 3);
    assert_eq!(rules[0].protocol, Protocol::Host);
    assert_eq!(rules[1].protocol, Protocol::ReqHeaders);
    assert_eq!(rules[2].protocol, Protocol::ReqDelay);
}

#[test]
fn test_parse_rule_with_inline_values() {
    let rules = parse_line("example.com reqHeaders://{Authorization=Bearer token123}").unwrap();

    assert_eq!(rules.len(), 1);
    assert!(rules[0].value.contains("Authorization"));
}

#[test]
fn test_parse_rules_multiline() {
    let text = r#"
# Comment line
example.com host://127.0.0.1
*.api.com proxy://proxy.local:8080
192.168.1.1 delete://
"#;

    let rules = parse_rules(text).unwrap();
    assert_eq!(rules.len(), 3);
}

#[test]
fn test_parse_rules_continuation() {
    let text = r#"example.com \
host://127.0.0.1 \
reqHeaders://{test=1}"#;

    let rules = parse_rules(text).unwrap();
    assert_eq!(rules.len(), 2);
}

#[test]
fn test_rule_parser_with_values() {
    let mut parser = RuleParser::new();
    parser.set_value("target".to_string(), "127.0.0.1".to_string());
    parser.set_value("port".to_string(), "8080".to_string());

    let rules = parser
        .parse_line("example.com host://${target}:${port}")
        .unwrap();

    assert_eq!(rules.len(), 1);
}

#[test]
fn test_parse_regex_rule() {
    let rules = parse_line("/example.com/i host://127.0.0.1").unwrap();

    assert_eq!(rules.len(), 1);
    assert!(rules[0].pattern.starts_with('/'));
}

#[test]
fn test_parse_regex_case_insensitive() {
    let rules = parse_line(r"/example\.com/i host://127.0.0.1").unwrap();

    assert_eq!(rules.len(), 1);
    assert!(rules[0].pattern.ends_with("/i"));
}

#[test]
fn test_invalid_rule_no_protocol() {
    let result = parse_line("example.com");
    assert!(result.is_err());
}

#[test]
fn test_empty_line() {
    let rules = parse_line("").unwrap();
    assert!(rules.is_empty());
}

#[test]
fn test_comment_line() {
    let rules = parse_line("# This is a comment").unwrap();
    assert!(rules.is_empty());
}

#[test]
fn test_domain_matcher_with_protocol() {
    let matcher = DomainMatcher::new("http://example.com");

    let result = matcher.matches("http://example.com/path", "example.com", "/path");
    assert!(result.matched);

    let result = matcher.matches("https://example.com/path", "example.com", "/path");
    assert!(!result.matched);
}

#[test]
fn test_domain_matcher_any_protocol() {
    let matcher = DomainMatcher::new("example.com");

    let result = matcher.matches("http://example.com/path", "example.com", "/path");
    assert!(result.matched);

    let result = matcher.matches("https://example.com/path", "example.com", "/path");
    assert!(result.matched);
}

#[test]
fn test_rule_line_numbers() {
    let text = "line1.com host://1\nline2.com host://2\nline3.com host://3";
    let rules = parse_rules(text).unwrap();

    assert_eq!(rules[0].line, Some(1));
    assert_eq!(rules[1].line, Some(2));
    assert_eq!(rules[2].line, Some(3));
}

#[test]
fn test_protocol_to_str() {
    assert_eq!(Protocol::Host.to_str(), "host");
    assert_eq!(Protocol::Proxy.to_str(), "proxy");
    assert_eq!(Protocol::ReqHeaders.to_str(), "reqHeaders");
    assert_eq!(Protocol::ResHeaders.to_str(), "resHeaders");
}

#[test]
fn test_protocol_from_str_roundtrip() {
    for protocol in ALL_PROTOCOLS.iter() {
        let name = protocol.to_str();
        let parsed = Protocol::parse(name);
        assert_eq!(
            parsed,
            Some(*protocol),
            "Roundtrip failed for {:?}",
            protocol
        );
    }
}

#[test]
fn test_protocol_is_multi_match() {
    assert!(!Protocol::Host.is_multi_match());
    assert!(!Protocol::Proxy.is_multi_match());
    assert!(!Protocol::StatusCode.is_multi_match());
    assert!(Protocol::ReqHeaders.is_multi_match());
    assert!(Protocol::Delete.is_multi_match());
}

#[test]
fn test_wildcard_matcher_creation() {
    let matcher = WildcardMatcher::new("*.example.com").unwrap();
    let result = matcher.matches("http://www.example.com", "www.example.com", "/");
    assert!(result.matched);

    let result = matcher.matches("http://api.example.com", "api.example.com", "/");
    assert!(result.matched);
}

#[test]
fn test_regex_matcher_invalid_pattern() {
    let result = RegexMatcher::new("/[invalid/");
    assert!(result.is_err());
}

#[test]
fn test_matcher_priority() {
    let exact = DomainMatcher::new("example.com");
    let wildcard = WildcardMatcher::new("*.example.com").unwrap();

    assert!(exact.priority() >= wildcard.priority());
}

#[test]
fn test_match_result_creation() {
    let matched = MatchResult::matched();
    assert!(matched.matched);
    assert!(matched.captures.is_none());

    let not_matched = MatchResult::not_matched();
    assert!(!not_matched.matched);

    let with_caps = MatchResult::matched_with_captures(vec!["a".to_string(), "b".to_string()]);
    assert!(with_caps.matched);
    assert_eq!(with_caps.captures.unwrap().len(), 2);
}
