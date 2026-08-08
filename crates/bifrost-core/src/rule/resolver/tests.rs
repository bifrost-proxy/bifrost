use super::*;
use crate::matcher::WildcardMatcher;
use crate::rule::filter::{parse_filter, LineProps};
use std::sync::Arc;

mod multi_demand;

fn create_test_rule(pattern: &str, protocol: Protocol, value: &str) -> Rule {
    let matcher = Arc::new(WildcardMatcher::new(pattern).unwrap());
    Rule::new(
        pattern.to_string(),
        matcher,
        protocol,
        value.to_string(),
        format!("{} {}://{}", pattern, protocol.to_str(), value),
    )
}

fn parse_test_rule(line: &str) -> Vec<Rule> {
    crate::rule::parser::RuleParser::new()
        .parse_line(line)
        .unwrap_or_else(|err| panic!("failed to parse test rule `{}`: {}", line, err))
}

fn create_test_rule_with_filters(
    pattern: &str,
    protocol: Protocol,
    value: &str,
    include_filters: Vec<Filter>,
    exclude_filters: Vec<Filter>,
) -> Rule {
    let matcher = Arc::new(WildcardMatcher::new(pattern).unwrap());
    Rule::new(
        pattern.to_string(),
        matcher,
        protocol,
        value.to_string(),
        format!("{} {}://{}", pattern, protocol.to_str(), value),
    )
    .with_include_filters(include_filters)
    .with_exclude_filters(exclude_filters)
}

fn create_test_context(url: &str, host: &str, path: &str) -> RequestContext {
    RequestContext::builder()
        .url(url)
        .host(host)
        .hostname(host)
        .path(path)
        .pathname(path)
        .build()
}

#[test]
fn test_request_context_new() {
    let ctx = create_test_context("http://example.com/path", "example.com", "/path");
    assert_eq!(ctx.url, "http://example.com/path");
    assert_eq!(ctx.host, "example.com");
    assert_eq!(ctx.path, "/path");
}

#[test]
fn test_resolved_rules_new() {
    let result = ResolvedRules::new();
    assert!(result.is_empty());
    assert_eq!(result.len(), 0);
}

#[test]
fn test_resolved_rules_add() {
    let mut result = ResolvedRules::new();
    let rule = create_test_rule("*.example.com", Protocol::Host, "127.0.0.1");
    let resolved = ResolvedRule::new_simple(rule, None, &HashMap::new());
    result.add(resolved);

    assert!(!result.is_empty());
    assert_eq!(result.len(), 1);
}

#[test]
fn header_templates_expand_after_authored_separators_are_parsed() {
    let ctx = create_test_context("http://example.com/path?a=1&b=2", "example.com", "/path");
    let rule = create_test_rule(
        "example.com",
        Protocol::ReqHeaders,
        "X-Full-Url=${url}&X-Mode=test",
    );
    let resolved = ResolvedRule::new(rule, None, &ctx, &HashMap::new());

    assert_eq!(
        resolved.header_pairs(),
        Some(
            [
                (
                    "X-Full-Url".to_string(),
                    "http://example.com/path?a=1&b=2".to_string(),
                ),
                ("X-Mode".to_string(), "test".to_string()),
            ]
            .as_slice()
        )
    );
}

#[test]
fn header_template_value_cannot_inject_an_extra_header() {
    let mut ctx = create_test_context("http://example.com/path", "example.com", "/path");
    ctx.req_headers
        .insert("source".to_string(), "safe&X-Injected=yes".to_string());
    let rule = create_test_rule(
        "example.com",
        Protocol::ReqHeaders,
        "X-Copied=${reqHeaders.source}",
    );
    let resolved = ResolvedRule::new(rule, None, &ctx, &HashMap::new());

    assert_eq!(
        resolved.header_pairs(),
        Some([("X-Copied".to_string(), "safe&X-Injected=yes".to_string())].as_slice())
    );
}

#[test]
fn cookie_and_trailer_templates_expand_after_authored_separators_are_parsed() {
    let mut ctx = create_test_context("http://example.com/path", "example.com", "/path");
    ctx.req_headers
        .insert("source".to_string(), "safe&injected=yes".to_string());

    for protocol in [
        Protocol::ReqCookies,
        Protocol::ResCookies,
        Protocol::Trailers,
    ] {
        let rule = create_test_rule(
            "example.com",
            protocol,
            "first=${reqHeaders.source}&second=two",
        );
        let resolved = ResolvedRule::new(rule, None, &ctx, &HashMap::new());
        assert_eq!(
            resolved.key_value_pairs(),
            Some(
                [
                    ("first".to_string(), "safe&injected=yes".to_string()),
                    ("second".to_string(), "two".to_string()),
                ]
                .as_slice()
            ),
            "protocol {protocol:?}"
        );
    }
}

#[test]
fn response_cookie_json_with_attributes_stays_structured() {
    let value = r#"{"sid":{"value":"abc","path":"/","httpOnly":true}}"#;
    let rule = create_test_rule("example.com", Protocol::ResCookies, value);
    let resolved = ResolvedRule::new_simple(rule, None, &HashMap::new());

    assert_eq!(resolved.resolved_value, value);
    assert_eq!(resolved.key_value_pairs(), None);
    assert_eq!(resolved.header_pairs(), None);

    let parenthesized = format!("({value})");
    let rule = create_test_rule("example.com", Protocol::ResCookies, &parenthesized);
    let resolved = ResolvedRule::new_simple(rule, None, &HashMap::new());
    assert_eq!(resolved.key_value_pairs(), None);
}

#[test]
fn test_resolved_rules_get_by_protocol() {
    let mut result = ResolvedRules::new();

    let rule1 = create_test_rule("*.example.com", Protocol::Host, "127.0.0.1");
    let rule2 = create_test_rule("*.api.com", Protocol::Proxy, "proxy:8080");

    result.add(ResolvedRule::new_simple(rule1, None, &HashMap::new()));
    result.add(ResolvedRule::new_simple(rule2, None, &HashMap::new()));

    let host_rules = result.get_by_protocol(Protocol::Host);
    assert_eq!(host_rules.len(), 1);

    let proxy_rules = result.get_by_protocol(Protocol::Proxy);
    assert_eq!(proxy_rules.len(), 1);
}

#[test]
fn test_resolved_rules_has_protocol() {
    let mut result = ResolvedRules::new();
    let rule = create_test_rule("*.example.com", Protocol::Host, "127.0.0.1");
    result.add(ResolvedRule::new_simple(rule, None, &HashMap::new()));

    assert!(result.has_protocol(Protocol::Host));
    assert!(!result.has_protocol(Protocol::Proxy));
}

#[test]
fn test_rules_resolver_new() {
    let rules = vec![
        create_test_rule("*.example.com", Protocol::Host, "127.0.0.1"),
        create_test_rule("example.com", Protocol::Host, "127.0.0.2"),
    ];
    let resolver = RulesResolver::new(rules);
    assert_eq!(resolver.rule_count(), 2);
}

#[test]
fn test_rules_resolver_priority_sorting() {
    let rules = vec![
        create_test_rule("*.example.com", Protocol::Host, "127.0.0.1"),
        create_test_rule("example.com", Protocol::Host, "127.0.0.2"),
    ];
    let resolver = RulesResolver::new(rules);
    assert!(resolver.rules()[0].priority() >= resolver.rules()[1].priority());
}

#[test]
fn test_rules_resolver_resolve() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::Host,
        "127.0.0.1",
    )];
    let resolver = RulesResolver::new(rules);

    let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
    assert!(result.has_protocol(Protocol::Host));
}

#[test]
fn test_rules_resolver_no_match() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::Host,
        "127.0.0.1",
    )];
    let resolver = RulesResolver::new(rules);

    let ctx = create_test_context("http://www.other.com/path", "www.other.com", "/path");

    let result = resolver.resolve(&ctx);
    assert!(result.is_empty());
}

#[test]
fn test_rules_resolver_with_values() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::Host,
        "${target}",
    )];

    let mut values = HashMap::new();
    values.insert("target".to_string(), "127.0.0.1".to_string());

    let resolver = RulesResolver::new(rules).with_values(values);

    let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
    assert_eq!(result.rules[0].resolved_value, "127.0.0.1");
}

#[test]
fn test_rules_resolver_with_value_ref() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::ReqHeaders,
        "{authHeaders}",
    )];

    let mut values = HashMap::new();
    values.insert(
        "authHeaders".to_string(),
        "X-Auth-Token: secret-12345".to_string(),
    );

    let resolver = RulesResolver::new(rules).with_values(values);

    let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
    assert_eq!(result.rules[0].resolved_value, "X-Auth-Token: secret-12345");
}

#[test]
fn test_value_ref_with_parsed_rules() {
    use crate::parse_rules;

    let rules = parse_rules("test.local reqHeaders://{customHeaders}").unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].value, "{customHeaders}");

    let mut values = HashMap::new();
    values.insert(
        "customHeaders".to_string(),
        "X-Custom-Token: secret-12345".to_string(),
    );

    let resolver = RulesResolver::new(rules).with_values(values);

    let ctx = RequestContext::from_url("http://test.local/api");

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
    assert_eq!(
        result.rules[0].resolved_value,
        "X-Custom-Token: secret-12345"
    );
}

#[test]
fn test_bp_remote_url_is_preserved_as_script_reference() {
    let rule = create_test_rule(
        "bp.example.com",
        Protocol::Bp,
        "http://127.0.0.1:18080/parser.js?sha256=abc",
    );
    let resolver = RulesResolver::new(vec![rule]);
    let ctx = RequestContext::from_url("http://bp.example.com/api");

    let result = resolver.resolve(&ctx);

    assert_eq!(
        result.rules[0].resolved_value,
        "http://127.0.0.1:18080/parser.js?sha256=abc"
    );
}

#[test]
fn test_bp_value_ref_expands_profile_without_remote_fetch() {
    let rule = create_test_rule("bp.example.com", Protocol::Bp, "{order_bp}");
    let mut values = HashMap::new();
    values.insert(
        "order_bp".to_string(),
        "build_in_bp?psm=foo.bar.order&idlSource=bam".to_string(),
    );
    let resolver = RulesResolver::new(vec![rule]).with_values(values);
    let ctx = RequestContext::from_url("http://bp.example.com/api");

    let result = resolver.resolve(&ctx);

    assert_eq!(
        result.rules[0].resolved_value,
        "build_in_bp?psm=foo.bar.order&idlSource=bam"
    );
}

#[test]
fn test_rules_resolver_add_rule() {
    let mut resolver = RulesResolver::new(vec![]);
    assert_eq!(resolver.rule_count(), 0);

    resolver.add_rule(create_test_rule(
        "*.example.com",
        Protocol::Host,
        "127.0.0.1",
    ));
    assert_eq!(resolver.rule_count(), 1);
}

#[test]
fn test_rules_resolver_set_value() {
    let mut resolver = RulesResolver::new(vec![]);
    resolver.set_value("key".to_string(), "value".to_string());
    assert_eq!(resolver.values.get("key"), Some(&"value".to_string()));
}

#[test]
fn test_rules_resolver_cache() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::Host,
        "127.0.0.1",
    )];
    let resolver = RulesResolver::new(rules);

    let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

    let result1 = resolver.resolve(&ctx);
    let result2 = resolver.resolve(&ctx);

    assert_eq!(result1.len(), result2.len());
}

#[test]
fn test_rules_resolver_disable_cache() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::Host,
        "127.0.0.1",
    )];
    let resolver = RulesResolver::new(rules).disable_cache();

    let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
}

#[test]
fn test_rules_resolver_clear_cache() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::Host,
        "127.0.0.1",
    )];
    let resolver = RulesResolver::new(rules);

    let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

    let _ = resolver.resolve(&ctx);
    resolver.clear_cache();

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
}

#[test]
fn test_lru_cache_eviction() {
    let mut cache = LruCache::new(2);
    cache.insert(
        "key1".to_string(),
        vec![CandidateMatch {
            rule_index: 1,
            captures: None,
            is_negated: false,
        }],
    );
    cache.insert(
        "key2".to_string(),
        vec![CandidateMatch {
            rule_index: 2,
            captures: None,
            is_negated: false,
        }],
    );
    let _ = cache.get("key1");
    cache.insert(
        "key3".to_string(),
        vec![CandidateMatch {
            rule_index: 3,
            captures: None,
            is_negated: false,
        }],
    );

    assert!(cache.get("key1").is_some());
    assert!(cache.get("key2").is_none());
    assert!(cache.get("key3").is_some());
}

#[test]
fn test_multi_match_protocol() {
    let rules = vec![
        create_test_rule("*.example.com", Protocol::ReqHeaders, "header1=value1"),
        create_test_rule("*.example.com", Protocol::ReqHeaders, "header2=value2"),
    ];
    let resolver = RulesResolver::new(rules);

    let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 2);
}

#[test]
fn test_single_match_protocol() {
    let rules = vec![
        create_test_rule("*.example.com", Protocol::Host, "127.0.0.1"),
        create_test_rule("*.example.com", Protocol::Host, "127.0.0.2"),
    ];
    let resolver = RulesResolver::new(rules);

    let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
}

#[test]
fn test_builtin_variable_expansion() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::Host,
        "host-${hostname}",
    )];
    let resolver = RulesResolver::new(rules);

    let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
    assert_eq!(result.rules[0].resolved_value, "host-www.example.com");
}

#[test]
fn test_url_variable_expansion() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::ResBody,
        "${url}",
    )];
    let resolver = RulesResolver::new(rules);

    let ctx = create_test_context(
        "http://www.example.com/api/test",
        "www.example.com",
        "/api/test",
    );

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
    assert_eq!(
        result.rules[0].resolved_value,
        "http://www.example.com/api/test"
    );
}

#[test]
fn test_path_variable_expansion() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::ResBody,
        "path=${path}",
    )];
    let resolver = RulesResolver::new(rules);

    let ctx = create_test_context(
        "http://www.example.com/api/test?foo=bar",
        "www.example.com",
        "/api/test?foo=bar",
    );

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
    assert_eq!(result.rules[0].resolved_value, "path=/api/test?foo=bar");
}

#[test]
fn test_method_variable_expansion() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::ResBody,
        "method=${method}",
    )];
    let resolver = RulesResolver::new(rules);

    let ctx = RequestContext::builder()
        .url("http://www.example.com/api")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/api")
        .pathname("/api")
        .method("POST")
        .build();

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
    assert_eq!(result.rules[0].resolved_value, "method=POST");
}

#[test]
fn test_client_ip_variable_expansion() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::ResBody,
        "client=${clientIp}",
    )];
    let resolver = RulesResolver::new(rules);

    let ctx = RequestContext::builder()
        .url("http://www.example.com/api")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/api")
        .pathname("/api")
        .client_ip("192.168.1.100")
        .build();

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
    assert_eq!(result.rules[0].resolved_value, "client=192.168.1.100");
}

#[test]
fn test_header_variable_expansion() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::ResBody,
        "auth=${reqHeaders.authorization}",
    )];
    let resolver = RulesResolver::new(rules);

    let mut headers = HashMap::new();
    headers.insert("authorization".to_string(), "Bearer token123".to_string());

    let ctx = RequestContext::builder()
        .url("http://www.example.com/api")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/api")
        .pathname("/api")
        .req_headers(headers)
        .build();

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
    assert_eq!(result.rules[0].resolved_value, "auth=Bearer token123");
}

#[test]
fn test_cookie_variable_expansion() {
    let rules = vec![create_test_rule(
        "*.example.com",
        Protocol::ResBody,
        "session=${reqCookies.session_id}",
    )];
    let resolver = RulesResolver::new(rules);

    let mut cookies = HashMap::new();
    cookies.insert("session_id".to_string(), "abc123".to_string());

    let ctx = RequestContext::builder()
        .url("http://www.example.com/api")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/api")
        .pathname("/api")
        .req_cookies(cookies)
        .build();

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);
    assert_eq!(result.rules[0].resolved_value, "session=abc123");
}

#[test]
fn test_include_filter_method() {
    let include_filters = vec![parse_filter("m:GET").unwrap()];
    let rules = vec![create_test_rule_with_filters(
        "*.example.com",
        Protocol::Host,
        "127.0.0.1",
        include_filters,
        vec![],
    )];
    let resolver = RulesResolver::new(rules);

    let ctx_get = RequestContext::builder()
        .url("http://www.example.com/api")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/api")
        .pathname("/api")
        .method("GET")
        .build();

    let result = resolver.resolve(&ctx_get);
    assert_eq!(result.len(), 1);

    let ctx_post = RequestContext::builder()
        .url("http://www.example.com/api")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/api")
        .pathname("/api")
        .method("POST")
        .build();

    let result = resolver.resolve(&ctx_post);
    assert!(result.is_empty());
}

#[test]
fn test_exclude_filter_path() {
    let exclude_filters = vec![parse_filter("/admin/").unwrap()];
    let rules = vec![create_test_rule_with_filters(
        "*.example.com",
        Protocol::Host,
        "127.0.0.1",
        vec![],
        exclude_filters,
    )];
    let resolver = RulesResolver::new(rules);

    let ctx_api = RequestContext::builder()
        .url("http://www.example.com/api")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/api")
        .pathname("/api")
        .build();

    let result = resolver.resolve(&ctx_api);
    assert_eq!(result.len(), 1);

    let ctx_admin = RequestContext::builder()
        .url("http://www.example.com/admin/users")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/admin/users")
        .pathname("/admin/users")
        .build();

    let result = resolver.resolve(&ctx_admin);
    assert!(result.is_empty());
}

#[test]
fn test_exclude_filter_whistle_style_wildcard_url() {
    let exclude_filters = vec![parse_filter("*/alice/*").unwrap()];
    let rules = vec![create_test_rule_with_filters(
        "*.doubao.com",
        Protocol::Http,
        "localhost:8080",
        vec![],
        exclude_filters,
    )];
    let resolver = RulesResolver::new(rules);

    let excluded = resolver.resolve(&RequestContext::from_url(
        "https://www.doubao.com/alice/commerce/sale/subscription/entry/config/?from=test",
    ));
    assert!(excluded.is_empty());

    let allowed = resolver.resolve(&RequestContext::from_url(
        "https://www.doubao.com/bob/commerce/sale/subscription/entry/config/?from=test",
    ));
    assert_eq!(allowed.len(), 1);
    assert_eq!(allowed.rules[0].resolved_value, "localhost:8080");
}

#[test]
fn test_exclude_filter_whistle_style_wildcard_path_prefix() {
    let exclude_filters = vec![parse_filter("*/api").unwrap()];
    let rules = vec![create_test_rule_with_filters(
        "www.example.com",
        Protocol::Http,
        "localhost:5173",
        vec![],
        exclude_filters,
    )];
    let resolver = RulesResolver::new(rules);

    assert!(resolver
        .resolve(&RequestContext::from_url(
            "https://www.example.com/api/users"
        ))
        .is_empty());
    assert!(resolver
        .resolve(&RequestContext::from_url(
            "https://www.example.com/api?debug=1"
        ))
        .is_empty());

    let allowed = resolver.resolve(&RequestContext::from_url(
        "https://www.example.com/apiary/users",
    ));
    assert_eq!(allowed.len(), 1);
}

#[test]
fn test_long_exclude_filter_chain_uses_regular_prefix_matching() {
    let excludes = [
        "/account/page/cooperate/qianchuan",
        "/garrmodlistv3",
        "/vmok-modules",
        "/qc_main_mono",
        "/qcServiceWorker",
        "/promotion-v2",
        "/product",
        "/nbs",
        "/finance",
        "/creative_config",
        "/check_login",
        "/brand_main",
        "/brand",
        "/aweme",
        "/apps",
        "/api",
        "/ad",
        "/account",
        "/_AMapService",
        "/zhitui",
        "/uni-prom",
        "/trident_v2",
        "/star",
        "/promotion",
        "/notify",
        "/mp",
        "/magic",
        "/home",
        "/doris/ad_v2",
        "/doris-report",
        "/docs",
        "/dataV2",
        "/creative-preview-apps",
        "/creation",
        "/createV2",
        "/copilot",
        "/cg_trade",
        "/cf/ml",
        "/brand_shop_window",
        "/brand_pre_sales",
        "/brand_pre_review",
        "/brand_inquiry_tool",
        "/bp",
        "/arcosite-api",
        "/alpha_sw",
        "/ttwid",
        "/webcast",
        "/uni-creation",
        "/support",
        "/sta_statement",
        "/sta_invoice",
        "/sta_deposit",
        "/sso",
        "/site",
        "/selfcreative",
        "/rule",
        "/risk-control",
        "/refund",
        "/passport",
        "/openapi",
        "/ocic",
        "/mobile",
        "/marketing",
        "/jarvis",
        "/im-linkchat",
        "/help",
        "/growth",
        "/fund_report",
        "/fund_refund",
        "/fund_recharge",
        "/ecp",
        "/e_adv",
        "/draft",
        "/doushop",
        "/credit",
        "/community_security",
        "/cognition",
        "/cip_invoice",
        "/cg_project/backend/open",
        "/cg_detain",
        "/cg_deposit/backend",
        "/cg_cont",
        "/cg_charge",
        "/cg_cert_center/back_end",
        "/cg_cert",
        "/cc-external",
        "/brand_node",
        "/brand_fe",
        "/board-next",
        "/board",
        "/app",
        "/advocacy",
        "/adstyle",
        "/account_security",
        "/account_info_help",
        "/login",
        "/tools",
        "/creative",
        "/star-pages",
    ];
    let filter_chain = excludes
        .iter()
        .map(|path| format!("excludeFilter://{}", path))
        .collect::<Vec<_>>()
        .join(" ");
    let rule_text = format!("qianchuan.jinritemai.com 10.37.102.138:8080 {filter_chain}");
    let rules = crate::parse_rules(&rule_text).unwrap();
    assert_eq!(rules[0].exclude_filters.len(), excludes.len());
    let resolver = RulesResolver::new(rules);

    for (idx, path) in excludes.iter().enumerate() {
        let url = format!("https://qianchuan.jinritemai.com{}-suffix?case={idx}", path);
        let result = resolver.resolve(&RequestContext::from_url(&url));
        assert!(
            result.is_empty(),
            "excludeFilter://{path} should exclude prefix URL {url}"
        );
    }

    let result = resolver.resolve(&RequestContext::from_url(
        "https://qianchuan.jinritemai.com/not-listed/path?case=allowed",
    ));
    assert_eq!(result.len(), 1);
    assert_eq!(result.rules[0].resolved_value, "10.37.102.138:8080");
}

#[test]
fn test_combined_include_exclude_filters() {
    let include_filters = vec![parse_filter("m:GET,POST").unwrap()];
    let exclude_filters = vec![parse_filter("/health/").unwrap()];
    let rules = vec![create_test_rule_with_filters(
        "*.example.com",
        Protocol::Host,
        "127.0.0.1",
        include_filters,
        exclude_filters,
    )];
    let resolver = RulesResolver::new(rules);

    let ctx = RequestContext::builder()
        .url("http://www.example.com/api")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/api")
        .pathname("/api")
        .method("GET")
        .build();

    let result = resolver.resolve(&ctx);
    assert_eq!(result.len(), 1);

    let ctx_health = RequestContext::builder()
        .url("http://www.example.com/health/")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/health/")
        .pathname("/health/")
        .method("GET")
        .build();

    let result = resolver.resolve(&ctx_health);
    assert!(result.is_empty());
}

#[test]
fn test_include_filter_header_exists() {
    let include_filters = vec![parse_filter("h:X-Custom-Header").unwrap()];
    let rules = vec![create_test_rule_with_filters(
        "*.example.com",
        Protocol::Host,
        "127.0.0.1",
        include_filters,
        vec![],
    )];
    let resolver = RulesResolver::new(rules).disable_cache();

    let ctx_with_header = RequestContext::builder()
        .url("http://www.example.com/api")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/api")
        .pathname("/api")
        .header("X-Custom-Header", "value")
        .build();

    let result = resolver.resolve(&ctx_with_header);
    assert_eq!(result.len(), 1);

    let ctx_without_header = RequestContext::builder()
        .url("http://www.example.com/api")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/api")
        .pathname("/api")
        .build();

    let result = resolver.resolve(&ctx_without_header);
    assert!(result.is_empty());
}

#[test]
fn test_include_filter_client_ip() {
    let include_filters = vec![parse_filter("i:192.168.0.0/16").unwrap()];
    let rules = vec![create_test_rule_with_filters(
        "*.example.com",
        Protocol::Host,
        "127.0.0.1",
        include_filters,
        vec![],
    )];
    let resolver = RulesResolver::new(rules).disable_cache();

    let ctx_match = RequestContext::builder()
        .url("http://www.example.com/api")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/api")
        .pathname("/api")
        .client_ip("192.168.1.100")
        .build();

    let result = resolver.resolve(&ctx_match);
    assert_eq!(result.len(), 1);

    let ctx_no_match = RequestContext::builder()
        .url("http://www.example.com/api")
        .host("www.example.com")
        .hostname("www.example.com")
        .path("/api")
        .pathname("/api")
        .client_ip("10.0.0.1")
        .build();

    let result = resolver.resolve(&ctx_no_match);
    assert!(result.is_empty());
}

#[test]
fn test_disabled_rule() {
    let matcher = Arc::new(WildcardMatcher::new("*.example.com").unwrap());
    let rule = Rule::new(
        "*.example.com".to_string(),
        matcher,
        Protocol::Host,
        "127.0.0.1".to_string(),
        "*.example.com host://127.0.0.1".to_string(),
    )
    .with_line_props(LineProps {
        important: false,
        disabled: true,
    });

    let resolver = RulesResolver::new(vec![rule]);

    let ctx = create_test_context("http://www.example.com/path", "www.example.com", "/path");

    let result = resolver.resolve(&ctx);
    assert!(result.is_empty());
}

#[test]
fn test_has_response_rules_for_host_allows_explicit_domain_and_ip_scope() {
    let mut rules = Vec::new();
    rules.extend(parse_test_rule(
        "tls-auto-domain.local resHeaders://X-Auto-Tls=domain",
    ));
    rules.extend(parse_test_rule("127.0.0.1 resHeaders://X-Auto-Tls=ip"));

    let resolver = RulesResolver::new(rules);

    assert!(resolver.has_response_rules_for_host("tls-auto-domain.local"));
    assert!(resolver.has_response_rules_for_host("127.0.0.1"));
    assert!(!resolver.has_response_rules_for_host("other.local"));
}

#[test]
fn test_has_response_rules_for_host_allows_host_scoped_wildcards() {
    let mut rules = Vec::new();
    rules.extend(parse_test_rule(
        "*.tls-auto.local resHeaders://X-Auto-Tls=wildcard",
    ));
    rules.extend(parse_test_rule(
        "^path.tls-auto.local/api/* resHeaders://X-Auto-Tls=path",
    ));

    let resolver = RulesResolver::new(rules);

    assert!(resolver.has_response_rules_for_host("api.tls-auto.local"));
    assert!(resolver.has_response_rules_for_host("path.tls-auto.local"));
    assert!(!resolver.has_response_rules_for_host("tls-auto.local"));
}

#[test]
fn test_has_response_rules_for_host_rejects_pure_regex_and_wildcards() {
    let mut rules = Vec::new();
    rules.extend(parse_test_rule("* resHeaders://X-Auto-Tls=wildcard"));
    rules.extend(parse_test_rule(
        "*/api/* resHeaders://X-Auto-Tls=path-wildcard",
    ));
    rules.extend(parse_test_rule(
        "/regex-auto\\.local/ resHeaders://X-Auto-Tls=regex",
    ));

    let resolver = RulesResolver::new(rules);

    assert!(!resolver.has_response_rules_for_host("regex-auto.local"));
    assert!(!resolver.has_response_rules_for_host("wildcard-auto.local"));
}

#[test]
fn test_has_tls_auto_intercept_route_rules_for_host_allows_plain_domain_routes() {
    let mut rules = Vec::new();
    rules.extend(parse_test_rule(
        "qianchuan.jinritemai.com/app/account-center https://10.37.102.138:8081",
    ));
    rules.extend(parse_test_rule(
        "qianchuan.jinritemai.com/app https://qianchuan.jinritemai.com/app",
    ));
    rules.extend(parse_test_rule(
        "qianchuan.jinritemai.com https://10.37.102.138:8080",
    ));

    let resolver = RulesResolver::new(rules);

    assert!(resolver.has_tls_auto_intercept_route_rules_for_host("qianchuan.jinritemai.com"));
    assert!(!resolver.has_tls_auto_intercept_route_rules_for_host("other.jinritemai.com"));
}

#[test]
fn test_has_tls_auto_intercept_route_rules_for_host_respects_scheme_scope() {
    let mut rules = Vec::new();
    rules.extend(parse_test_rule(
        "http://scheme-route.local/app http://127.0.0.1:8080",
    ));

    let resolver = RulesResolver::new(rules);

    assert!(!resolver.has_tls_auto_intercept_route_rules_for_host("scheme-route.local"));
}

#[test]
fn test_has_tls_auto_intercept_route_rules_for_host_rejects_proxy_only_and_broad_patterns() {
    let mut rules = Vec::new();
    rules.extend(parse_test_rule("proxy-route.local proxy://127.0.0.1:8888"));
    rules.extend(parse_test_rule(
        "proxy-route.local/app proxy://127.0.0.1:8889",
    ));
    rules.extend(parse_test_rule("* https://127.0.0.1:9443"));
    rules.extend(parse_test_rule("*/account/* https://127.0.0.1:9444"));
    rules.extend(parse_test_rule(
        "/regex-route\\.local/ https://127.0.0.1:9445",
    ));

    let resolver = RulesResolver::new(rules);

    assert!(!resolver.has_tls_auto_intercept_route_rules_for_host("proxy-route.local"));
    assert!(!resolver.has_tls_auto_intercept_route_rules_for_host("regex-route.local"));
    assert!(!resolver.has_tls_auto_intercept_route_rules_for_host("broad-route.local"));
}

#[test]
fn test_important_priority_ordering() {
    let matcher1 = Arc::new(WildcardMatcher::new("*.example.com").unwrap());
    let rule1 = Rule::new(
        "*.example.com".to_string(),
        matcher1,
        Protocol::Host,
        "127.0.0.1".to_string(),
        "*.example.com host://127.0.0.1".to_string(),
    );

    let matcher2 = Arc::new(WildcardMatcher::new("*.example.com").unwrap());
    let rule2 = Rule::new(
        "*.example.com".to_string(),
        matcher2,
        Protocol::Host,
        "127.0.0.2".to_string(),
        "*.example.com host://127.0.0.2".to_string(),
    )
    .with_line_props(LineProps {
        important: true,
        disabled: false,
    });

    let resolver = RulesResolver::new(vec![rule1, rule2]);

    assert!(resolver.rules()[0].line_props.important);
    assert!(resolver.rules()[0].priority() > resolver.rules()[1].priority());
}

#[test]
fn test_path_wildcard_double_star_matching() {
    use crate::matcher::PathWildcardMatcher;

    let pattern = "^path-double.local/api/**";
    let matcher = Arc::new(PathWildcardMatcher::new(pattern).unwrap());

    let rule = Rule::new(
        pattern.to_string(),
        matcher,
        Protocol::Host,
        "127.0.0.1:3000".to_string(),
        format!("{} host://127.0.0.1:3000", pattern),
    );

    let resolver = RulesResolver::new(vec![rule]);
    let ctx = RequestContext::from_url("http://path-double.local/api/users");
    let result = resolver.resolve(&ctx);

    assert_eq!(result.len(), 1, "Should match the path wildcard rule");
    assert_eq!(result.rules[0].resolved_value, "127.0.0.1:3000");
}

#[test]
fn test_path_wildcard_via_rule_parser() {
    use crate::rule::parser::RuleParser;

    let rule_text = "^path-double.local/api/** http://127.0.0.1:3000";
    let parser = RuleParser::new();
    let rules = parser.parse_line(rule_text).expect("Failed to parse rule");

    assert_eq!(rules.len(), 1, "Should parse one rule");

    let resolver = RulesResolver::new(rules);
    let ctx = RequestContext::from_url("http://path-double.local/api/users");
    let result = resolver.resolve(&ctx);

    assert_eq!(result.len(), 1, "Should match the path wildcard rule");
}

#[test]
fn test_host_rule_matches_request_with_explicit_port() {
    use crate::rule::parser::RuleParser;

    let parser = RuleParser::new();
    let rules = parser
        .parse_line("127.0.0.1 reqHeaders://X-UI-Rule=alpha")
        .expect("Failed to parse rule");

    let resolver = RulesResolver::new(rules);
    let ctx = RequestContext::from_url("http://127.0.0.1:18084/rules-check");
    let result = resolver.resolve(&ctx);

    assert_eq!(
        result.len(),
        1,
        "Host-only rule should match requests with an explicit port"
    );
    assert_eq!(result.rules[0].resolved_value, "X-UI-Rule=alpha");
}

#[test]
fn test_negated_rule_does_not_block_other_patterns() {
    use crate::rule::parser::RuleParser;

    // 否定规则不应该阻止不匹配的其他规则
    let parser = RuleParser::new();
    let mut rules = parser
        .parse_line("!^path-negate.local/api/* http://127.0.0.1:3000")
        .unwrap();
    rules.extend(
        parser
            .parse_line("^path-double.local/api/** http://127.0.0.1:3000")
            .unwrap(),
    );

    let resolver = RulesResolver::new(rules);

    // 请求 path-double.local，不应该被 path-negate 的否定规则影响
    let ctx = RequestContext::from_url("http://path-double.local/api/users");
    let result = resolver.resolve(&ctx);

    assert_eq!(result.len(), 1, "Should match path-double rule");
    assert_eq!(
        result.rules[0].rule.pattern, "^path-double.local/api/**",
        "Should match the correct rule"
    );
}

#[test]
fn test_negated_rule_blocks_matching_pattern() {
    use crate::rule::parser::RuleParser;

    // 否定规则应该阻止匹配的同协议规则
    let parser = RuleParser::new();
    let mut rules = parser
        .parse_line("!^path-negate.local/api/* http://127.0.0.1:3000")
        .unwrap();
    rules.extend(
        parser
            .parse_line("^path-negate.local/api/** http://127.0.0.1:3000")
            .unwrap(),
    );

    let resolver = RulesResolver::new(rules);

    // 请求 path-negate.local，应该被否定规则阻止
    let ctx = RequestContext::from_url("http://path-negate.local/api/users");
    let result = resolver.resolve(&ctx);

    assert_eq!(result.len(), 0, "Should be blocked by the negated rule");
}

#[test]
fn test_response_filter_deferred_to_response_phase() {
    use crate::rule::parser::RuleParser;

    let parser = RuleParser::new();
    let rules = parser
        .parse_line("a.test host://127.0.0.1:18181 replaceStatus://217 includeFilter://s:500")
        .unwrap();
    let resolver = RulesResolver::new(rules);

    let ctx = RequestContext::from_url("http://a.test/");
    let req = resolver.resolve(&ctx);
    assert!(
        req.rules.iter().any(|r| r.rule.protocol == Protocol::Host),
        "host routing rule must be included at request phase"
    );

    let mut ctx500 = RequestContext::from_url("http://a.test/");
    ctx500.set_response(500, HashMap::new());
    let res500 = resolver.resolve_uncached(&ctx500);
    assert!(
        res500
            .rules
            .iter()
            .any(|r| r.rule.protocol == Protocol::ReplaceStatus),
        "replaceStatus must apply when the response status matches s:500"
    );

    let mut ctx200 = RequestContext::from_url("http://a.test/");
    ctx200.set_response(200, HashMap::new());
    let res200 = resolver.resolve_uncached(&ctx200);
    assert!(
        !res200
            .rules
            .iter()
            .any(|r| r.rule.protocol == Protocol::ReplaceStatus),
        "replaceStatus must not apply when the response status does not match s:500"
    );
}

#[test]
fn test_multiple_different_protocols_all_match() {
    use crate::rule::parser::RuleParser;

    let parser = RuleParser::new();
    let rules = parser
        .parse_line("test.local http://127.0.0.1:3000 resBody://{test-body}")
        .unwrap();

    assert_eq!(
        rules.len(),
        2,
        "Should create 2 rules for different protocols"
    );
    assert_eq!(rules[0].protocol, Protocol::Http);
    assert_eq!(rules[1].protocol, Protocol::ResBody);

    let resolver = RulesResolver::new(rules);
    let ctx = RequestContext::from_url("http://test.local/path");
    let result = resolver.resolve(&ctx);

    assert_eq!(result.len(), 2, "Both Http and ResBody rules should match");
    assert!(result.has_protocol(Protocol::Http));
    assert!(result.has_protocol(Protocol::ResBody));
}

#[test]
fn test_rules_resolver_skip_by_operation_allows_fallback_rule() {
    use crate::rule::parser::RuleParser;

    let parser = RuleParser::new();
    let rules = parser
            .parse_line(
                "skip-operation.local http://127.0.0.1:3000 resHeaders://`X-Skip-Op:first` resHeaders://`X-Skip-Op:second` skip://operation=resHeaders://`X-Skip-Op:first`",
            )
            .unwrap();
    let resolver = RulesResolver::new(rules);

    let ctx = RequestContext::from_url("http://skip-operation.local/test");
    let result = resolver.resolve(&ctx);
    let header_rules = result.get_by_protocol(Protocol::ResHeaders);

    assert_eq!(header_rules.len(), 1);
    assert_eq!(header_rules[0].resolved_value, "X-Skip-Op:second");
}

#[test]
fn test_rules_resolver_skip_by_pattern_allows_fallback_rule() {
    use crate::rule::parser::RuleParser;

    let parser = RuleParser::new();
    let mut rules = parser
            .parse_line("skip-pattern.local/api/blocked http://127.0.0.1:3000 resHeaders://`X-Skip-Pattern:blocked`")
            .unwrap();
    rules.extend(
            parser
                .parse_line("skip-pattern.local/api http://127.0.0.1:3000 resHeaders://`X-Skip-Pattern:fallback`")
                .unwrap(),
        );
    rules.extend(
        parser
            .parse_line(
                "skip-pattern.local/api/blocked skip://pattern=skip-pattern.local/api/blocked",
            )
            .unwrap(),
    );
    let resolver = RulesResolver::new(rules);

    let ctx = RequestContext::from_url("http://skip-pattern.local/api/blocked");
    let result = resolver.resolve(&ctx);
    let header_rules = result.get_by_protocol(Protocol::ResHeaders);

    assert_eq!(header_rules.len(), 1);
    assert_eq!(header_rules[0].resolved_value, "X-Skip-Pattern:fallback");
}

#[test]
fn test_path_specific_rule_takes_priority_over_domain_only_rule() {
    use crate::rule::parser::RuleParser;

    let parser = RuleParser::new();

    let mut rules = parser
        .parse_line("qianchuan.jinritemai.com/ad/api/ qianchuan.jinritemai.com/ad/")
        .unwrap();
    rules.extend(
        parser
            .parse_line("qianchuan.jinritemai.com localhost:8080")
            .unwrap(),
    );

    assert!(!rules.is_empty());

    let resolver = RulesResolver::new(rules);

    // With full path: more specific rule should win
    let ctx = RequestContext::from_url("https://qianchuan.jinritemai.com/ad/api/test");
    let result = resolver.resolve(&ctx);

    let host_rules = result.get_by_protocol(Protocol::Host);
    assert_eq!(host_rules.len(), 1);
    assert_eq!(host_rules[0].resolved_value, "qianchuan.jinritemai.com/ad/");
    assert_eq!(
        host_rules[0].rule.pattern,
        "qianchuan.jinritemai.com/ad/api/"
    );
}

#[test]
fn test_domain_only_rule_matches_connect_without_path() {
    use crate::rule::parser::RuleParser;

    let parser = RuleParser::new();

    let mut rules = parser
        .parse_line("qianchuan.jinritemai.com/ad/api/ qianchuan.jinritemai.com/ad/")
        .unwrap();
    rules.extend(
        parser
            .parse_line("qianchuan.jinritemai.com localhost:8080")
            .unwrap(),
    );

    let resolver = RulesResolver::new(rules);

    // CONNECT phase: URL has no path — only domain-only rules match
    let ctx = RequestContext::from_url("https://qianchuan.jinritemai.com:443");
    let result = resolver.resolve(&ctx);

    let host_rules = result.get_by_protocol(Protocol::Host);
    assert_eq!(host_rules.len(), 1);
    assert_eq!(host_rules[0].resolved_value, "localhost:8080");
}

#[test]
fn test_header_filter_not_stale_across_requests() {
    // Regression: matcher candidates are cached by URL shape, but filters are
    // request scoped and must be evaluated for every request.
    let rules = crate::rule::parser::parse_rules(
        "hc.test host://127.0.0.1:9 includeFilter://h:x-tag=match",
    )
    .unwrap();
    let resolver = RulesResolver::new(rules);

    let mut ctx_match = RequestContext::from_url("http://hc.test/");
    ctx_match.method = "GET".to_string();
    ctx_match
        .req_headers
        .insert("x-tag".to_string(), "match".to_string());
    assert_eq!(
        resolver.resolve(&ctx_match).rules.len(),
        1,
        "header value matches the filter -> rule applies"
    );

    let mut ctx_nomatch = RequestContext::from_url("http://hc.test/");
    ctx_nomatch.method = "GET".to_string();
    ctx_nomatch
        .req_headers
        .insert("x-tag".to_string(), "nope".to_string());
    assert_eq!(
        resolver.resolve(&ctx_nomatch).rules.len(),
        0,
        "different header value must not reuse the cached match"
    );

    assert_eq!(resolver.resolve(&ctx_match).rules.len(), 1);
}
