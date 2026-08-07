use bifrost_core::{RuleParser, RulesResolver};

#[test]
fn host_scoped_breakpoint_rules_match_concrete_authority_and_port() {
    let rules = RuleParser::new()
        .parse_rules(
            "breakpoint-auto.local breakpoint://request\n127.0.0.1:8443 breakpoint://response\n^path-breakpoint.local/api/* breakpoint://response",
        )
        .unwrap();
    let resolver = RulesResolver::new(rules);

    assert!(resolver.has_breakpoint_rules_for_host("breakpoint-auto.local:443"));
    assert!(resolver.has_breakpoint_rules_for_host("127.0.0.1:8443"));
    assert!(!resolver.has_breakpoint_rules_for_host("127.0.0.1:443"));
    assert!(resolver.has_breakpoint_rules_for_host("path-breakpoint.local:443"));
    assert!(!resolver.has_breakpoint_rules_for_host("other.local:443"));
}

#[test]
fn broad_regex_and_non_breakpoint_rules_do_not_auto_intercept_tls() {
    let rules = RuleParser::new()
        .parse_rules(
            "* breakpoint://response\n/breakpoint-.*/ breakpoint://response\nheaders-only.local resHeaders://X-Test=value",
        )
        .unwrap();
    let resolver = RulesResolver::new(rules);

    assert!(!resolver.has_breakpoint_rules_for_host("broad.local:443"));
    assert!(!resolver.has_breakpoint_rules_for_host("headers-only.local:443"));
}
