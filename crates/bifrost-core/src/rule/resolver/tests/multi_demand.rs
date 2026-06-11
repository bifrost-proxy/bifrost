use super::*;
use crate::rule::parser::RuleParser;

fn cache_key(ctx: &RequestContext) -> String {
    format!("{}|{}|{}", ctx.url, ctx.host, ctx.path)
}

fn assert_cached_candidate_count(resolver: &RulesResolver, ctx: &RequestContext, expected: usize) {
    let cached = resolver
        .cache
        .read()
        .peek(&cache_key(ctx))
        .expect("matcher candidates should be cached");
    assert_eq!(cached.len(), expected);
}

#[test]
fn test_cached_candidates_recheck_header_filters_per_request() {
    let parser = RuleParser::new();
    let mut rules = parser
        .parse_line(
            "multi-demand.local host://127.0.0.1:3000 includeFilter://h:x-feature-env=feature-web",
        )
        .unwrap();
    rules.extend(
        parser
            .parse_line(
                "multi-demand.local host://127.0.0.1:3001 includeFilter://h:x-feature-env=feature-mobile",
            )
            .unwrap(),
    );
    rules.extend(
        parser
            .parse_line("multi-demand.local host://127.0.0.1:3999")
            .unwrap(),
    );
    let resolver = RulesResolver::new(rules);

    let ctx_web = RequestContext::from_url("http://multi-demand.local/api")
        .with_header("x-feature-env", "feature-web");
    let web_result = resolver.resolve(&ctx_web);
    assert_eq!(web_result.len(), 1);
    assert_eq!(web_result.rules[0].resolved_value, "127.0.0.1:3000");
    assert_cached_candidate_count(&resolver, &ctx_web, 3);

    let ctx_mobile = RequestContext::from_url("http://multi-demand.local/api")
        .with_header("x-feature-env", "feature-mobile");
    let mobile_result = resolver.resolve(&ctx_mobile);
    assert_eq!(mobile_result.len(), 1);
    assert_eq!(mobile_result.rules[0].resolved_value, "127.0.0.1:3001");

    let ctx_fallback = RequestContext::from_url("http://multi-demand.local/api")
        .with_header("x-feature-env", "feature-unknown");
    let fallback_result = resolver.resolve(&ctx_fallback);
    assert_eq!(fallback_result.len(), 1);
    assert_eq!(fallback_result.rules[0].resolved_value, "127.0.0.1:3999");
}

#[test]
fn test_cached_candidates_recheck_method_filters_per_request() {
    let rules = vec![create_test_rule_with_filters(
        "method-cache.local",
        Protocol::Host,
        "127.0.0.1",
        vec![parse_filter("m:POST").unwrap()],
        vec![],
    )];
    let resolver = RulesResolver::new(rules);

    let post_ctx = RequestContext::from_url("http://method-cache.local/api").with_method("POST");
    assert_eq!(resolver.resolve(&post_ctx).len(), 1);
    assert_cached_candidate_count(&resolver, &post_ctx, 1);

    let get_ctx = RequestContext::from_url("http://method-cache.local/api").with_method("GET");
    assert!(resolver.resolve(&get_ctx).is_empty());
}

#[test]
fn test_cached_candidates_recheck_disabled_state_per_request() {
    let mut disabled_rule = create_test_rule("state-cache.local", Protocol::Host, "127.0.0.1");
    disabled_rule.line_props.disabled = true;
    let mut resolver = RulesResolver::new(vec![disabled_rule]);

    let ctx = RequestContext::from_url("http://state-cache.local/api");
    assert!(resolver.resolve(&ctx).is_empty());
    assert_cached_candidate_count(&resolver, &ctx, 1);

    resolver.rules[0].line_props.disabled = false;
    let enabled_result = resolver.resolve(&ctx);
    assert_eq!(enabled_result.len(), 1);
    assert_eq!(enabled_result.rules[0].resolved_value, "127.0.0.1");

    resolver.rules[0].line_props.disabled = true;
    assert!(resolver.resolve(&ctx).is_empty());
}

#[test]
fn test_cached_candidates_recheck_exclude_filters_per_request_both_orders() {
    let rules = vec![
        create_test_rule_with_filters(
            "exclude-cache.local",
            Protocol::Host,
            "127.0.0.1",
            vec![],
            vec![parse_filter("h:x-block=true").unwrap()],
        ),
        create_test_rule("exclude-cache.local", Protocol::Host, "127.0.0.2"),
    ];
    let resolver = RulesResolver::new(rules.clone());

    let allowed_ctx = RequestContext::from_url("http://exclude-cache.local/api");
    let allowed_result = resolver.resolve(&allowed_ctx);
    assert_eq!(allowed_result.len(), 1);
    assert_eq!(allowed_result.rules[0].resolved_value, "127.0.0.1");
    assert_cached_candidate_count(&resolver, &allowed_ctx, 2);

    let blocked_ctx =
        RequestContext::from_url("http://exclude-cache.local/api").with_header("x-block", "true");
    let blocked_result = resolver.resolve(&blocked_ctx);
    assert_eq!(blocked_result.len(), 1);
    assert_eq!(blocked_result.rules[0].resolved_value, "127.0.0.2");
    assert_eq!(
        resolver.resolve(&allowed_ctx).rules[0].resolved_value,
        "127.0.0.1"
    );

    let reverse_resolver = RulesResolver::new(rules);
    let reverse_blocked = reverse_resolver.resolve(&blocked_ctx);
    assert_eq!(reverse_blocked.len(), 1);
    assert_eq!(reverse_blocked.rules[0].resolved_value, "127.0.0.2");
    assert_cached_candidate_count(&reverse_resolver, &blocked_ctx, 2);
    assert_eq!(
        reverse_resolver.resolve(&allowed_ctx).rules[0].resolved_value,
        "127.0.0.1"
    );
}

#[test]
fn test_cached_candidates_recheck_request_modify_filters_per_request() {
    let rules = vec![
        create_test_rule_with_filters(
            "multi-modify.local",
            Protocol::ReqHeaders,
            "X-Selected-Env=web",
            vec![parse_filter("h:x-feature-env=feature-web").unwrap()],
            vec![],
        ),
        create_test_rule_with_filters(
            "multi-modify.local",
            Protocol::ReqHeaders,
            "X-Selected-Env=mobile",
            vec![parse_filter("h:x-feature-env=feature-mobile").unwrap()],
            vec![],
        ),
    ];
    let resolver = RulesResolver::new(rules);

    let web_ctx = RequestContext::from_url("http://multi-modify.local/api")
        .with_header("x-feature-env", "feature-web");
    let web_result = resolver.resolve(&web_ctx);
    let web_rules = web_result.get_by_protocol(Protocol::ReqHeaders);
    assert_eq!(web_rules.len(), 1);
    assert_eq!(web_rules[0].resolved_value, "X-Selected-Env=web");
    assert_cached_candidate_count(&resolver, &web_ctx, 2);

    let mobile_ctx = RequestContext::from_url("http://multi-modify.local/api")
        .with_header("x-feature-env", "feature-mobile");
    let mobile_result = resolver.resolve(&mobile_ctx);
    let mobile_rules = mobile_result.get_by_protocol(Protocol::ReqHeaders);
    assert_eq!(mobile_rules.len(), 1);
    assert_eq!(mobile_rules[0].resolved_value, "X-Selected-Env=mobile");
}

#[test]
fn test_cached_candidates_recheck_response_modify_filters_per_request() {
    let rules = vec![
        create_test_rule_with_filters(
            "multi-modify.local",
            Protocol::ResHeaders,
            "X-Selected-Env=web",
            vec![parse_filter("h:x-feature-env=feature-web").unwrap()],
            vec![],
        ),
        create_test_rule_with_filters(
            "multi-modify.local",
            Protocol::ResHeaders,
            "X-Selected-Env=mobile",
            vec![parse_filter("h:x-feature-env=feature-mobile").unwrap()],
            vec![],
        ),
        create_test_rule_with_filters(
            "multi-modify.local",
            Protocol::ReplaceStatus,
            "201",
            vec![parse_filter("h:x-feature-env=feature-web").unwrap()],
            vec![],
        ),
        create_test_rule_with_filters(
            "multi-modify.local",
            Protocol::ReplaceStatus,
            "202",
            vec![parse_filter("h:x-feature-env=feature-mobile").unwrap()],
            vec![],
        ),
    ];
    let resolver = RulesResolver::new(rules);

    let web_ctx = RequestContext::from_url("http://multi-modify.local/api")
        .with_header("x-feature-env", "feature-web");
    let web_result = resolver.resolve(&web_ctx);
    assert_eq!(
        web_result.get_by_protocol(Protocol::ResHeaders)[0].resolved_value,
        "X-Selected-Env=web"
    );
    assert_eq!(
        web_result.get_by_protocol(Protocol::ReplaceStatus)[0].resolved_value,
        "201"
    );
    assert_cached_candidate_count(&resolver, &web_ctx, 4);

    let mobile_ctx = RequestContext::from_url("http://multi-modify.local/api")
        .with_header("x-feature-env", "feature-mobile");
    let mobile_result = resolver.resolve(&mobile_ctx);
    assert_eq!(
        mobile_result.get_by_protocol(Protocol::ResHeaders)[0].resolved_value,
        "X-Selected-Env=mobile"
    );
    assert_eq!(
        mobile_result.get_by_protocol(Protocol::ReplaceStatus)[0].resolved_value,
        "202"
    );
}

#[test]
fn test_cached_candidates_recheck_direct_proxy_filters_per_request() {
    let rules = vec![
        create_test_rule_with_filters(
            "multi-direct.local",
            Protocol::Redirect,
            "302:https://web.example.test/",
            vec![parse_filter("h:x-feature-env=feature-web").unwrap()],
            vec![],
        ),
        create_test_rule_with_filters(
            "multi-direct.local",
            Protocol::Redirect,
            "302:https://mobile.example.test/",
            vec![parse_filter("h:x-feature-env=feature-mobile").unwrap()],
            vec![],
        ),
        create_test_rule_with_filters(
            "multi-status.local",
            Protocol::StatusCode,
            "201",
            vec![parse_filter("h:x-feature-env=feature-web").unwrap()],
            vec![],
        ),
        create_test_rule_with_filters(
            "multi-status.local",
            Protocol::StatusCode,
            "202",
            vec![parse_filter("h:x-feature-env=feature-mobile").unwrap()],
            vec![],
        ),
    ];
    let resolver = RulesResolver::new(rules);

    let web_redirect_ctx = RequestContext::from_url("http://multi-direct.local/api")
        .with_header("x-feature-env", "feature-web");
    let web_redirect = resolver.resolve(&web_redirect_ctx);
    assert_eq!(
        web_redirect.get_by_protocol(Protocol::Redirect)[0].resolved_value,
        "302:https://web.example.test/"
    );
    assert_cached_candidate_count(&resolver, &web_redirect_ctx, 2);

    let mobile_redirect_ctx = RequestContext::from_url("http://multi-direct.local/api")
        .with_header("x-feature-env", "feature-mobile");
    let mobile_redirect = resolver.resolve(&mobile_redirect_ctx);
    assert_eq!(
        mobile_redirect.get_by_protocol(Protocol::Redirect)[0].resolved_value,
        "302:https://mobile.example.test/"
    );

    let web_status_ctx = RequestContext::from_url("http://multi-status.local/api")
        .with_header("x-feature-env", "feature-web");
    let web_status = resolver.resolve(&web_status_ctx);
    assert_eq!(
        web_status.get_by_protocol(Protocol::StatusCode)[0].resolved_value,
        "201"
    );
    assert_cached_candidate_count(&resolver, &web_status_ctx, 2);

    let mobile_status_ctx = RequestContext::from_url("http://multi-status.local/api")
        .with_header("x-feature-env", "feature-mobile");
    let mobile_status = resolver.resolve(&mobile_status_ctx);
    assert_eq!(
        mobile_status.get_by_protocol(Protocol::StatusCode)[0].resolved_value,
        "202"
    );
}

#[test]
fn test_cached_candidates_recheck_skip_filters_per_request() {
    let parser = RuleParser::new();
    let mut rules = parser
        .parse_line("skip-cache.local resHeaders://X-Skip-Target=selected")
        .unwrap();
    rules.extend(
        parser
            .parse_line(
                "skip-cache.local skip://operation=resHeaders://X-Skip-Target=selected includeFilter://h:x-skip=true",
            )
            .unwrap(),
    );
    let resolver = RulesResolver::new(rules);

    let skipped_ctx =
        RequestContext::from_url("http://skip-cache.local/api").with_header("x-skip", "true");
    assert!(resolver.resolve(&skipped_ctx).is_empty());
    assert_cached_candidate_count(&resolver, &skipped_ctx, 2);

    let selected_ctx = RequestContext::from_url("http://skip-cache.local/api");
    let selected = resolver.resolve(&selected_ctx);
    let selected_rules = selected.get_by_protocol(Protocol::ResHeaders);
    assert_eq!(selected_rules.len(), 1);
    assert_eq!(selected_rules[0].resolved_value, "X-Skip-Target=selected");

    assert!(resolver.resolve(&skipped_ctx).is_empty());
}
