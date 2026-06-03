use hyper::Uri;
use regex::Regex;
use tracing::debug;
use url::Url;

use crate::server::ResolvedRules;
use crate::utils::logging::RequestContext;

lazy_static::lazy_static! {
    static ref MULTI_SLASH_PATH_REGEX: Regex = Regex::new(r"/{2,}").unwrap();
}

fn normalize_replaced_path(path: &str) -> String {
    if path.is_empty() {
        return path.to_string();
    }

    let collapsed = MULTI_SLASH_PATH_REGEX.replace_all(path, "/").to_string();
    if !collapsed.starts_with("/") {
        // Deleting a prefix like `/api/` from `/api/demo` would otherwise
        // strip the leading slash and yield a relative-looking path. Always
        // re-anchor to an absolute path for HTTP request-URIs.
        format!("/{}", collapsed)
    } else {
        collapsed
    }
}

pub fn apply_url_params(uri: &Uri, rules: &ResolvedRules) -> Uri {
    if rules.url_params.is_empty() && rules.delete_url_params.is_empty() {
        return uri.clone();
    }

    let uri_str = uri.to_string();
    let base_url = if uri_str.starts_with('/') {
        format!("http://localhost{}", uri_str)
    } else if !uri_str.contains("://") {
        format!("http://{}", uri_str)
    } else {
        uri_str
    };

    let Ok(mut url) = Url::parse(&base_url) else {
        return uri.clone();
    };

    let mut query_pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();

    if !rules.delete_url_params.is_empty() {
        query_pairs.retain(|(key, _)| !rules.delete_url_params.iter().any(|delete| delete == key));
    }

    for (key, value) in &rules.url_params {
        query_pairs.retain(|(existing_key, _)| existing_key != key);
        query_pairs.push((key.clone(), value.clone()));
    }

    if query_pairs.is_empty() {
        url.set_query(None);
    } else {
        let query = query_pairs
            .into_iter()
            .map(|(key, value)| {
                format!(
                    "{}={}",
                    urlencoding::encode(&key),
                    urlencoding::encode(&value)
                )
            })
            .collect::<Vec<_>>()
            .join("&");
        url.set_query(Some(&query));
    }

    let new_uri_str = if uri.to_string().starts_with('/') {
        match url.query() {
            Some(query) if !query.is_empty() => format!("{}?{}", url.path(), query),
            _ => url.path().to_string(),
        }
    } else {
        url.to_string()
    };

    new_uri_str.parse().unwrap_or_else(|_| uri.clone())
}

pub fn apply_url_replace(
    uri: &Uri,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Uri {
    if rules.url_replace.is_empty() && rules.url_replace_regex.is_empty() {
        return uri.clone();
    }

    let mut path = uri.path().to_string();
    let query = uri.query().map(|q| format!("?{}", q)).unwrap_or_default();

    for (from, to) in &rules.url_replace {
        if path.contains(from.as_str()) {
            path = normalize_replaced_path(&path.replace(from.as_str(), to.as_str()));
            if verbose_logging {
                debug!("[{}] [URL_REPLACE] {} -> {}", ctx.id_str(), from, to);
            }
        }
    }

    for rule in &rules.url_replace_regex {
        let updated = if rule.global {
            rule.pattern
                .replace_all(&path, rule.replacement.as_str())
                .to_string()
        } else {
            rule.pattern
                .replace(&path, rule.replacement.as_str())
                .to_string()
        };

        if updated != path {
            if verbose_logging {
                debug!(
                    "[{}] [URL_REPLACE_REGEX] {} -> {}",
                    ctx.id_str(),
                    rule.pattern.as_str(),
                    rule.replacement
                );
            }
            path = normalize_replaced_path(&updated);
        }
    }

    // Re-assemble the URI while preserving scheme + authority when the
    // original is an absolute form (proxy-style target). Using only
    // `format!("{}{}", path, query)` would drop scheme/host, so downstream
    // `extract_host_port` can no longer find the target.
    let uri_str = uri.to_string();
    let new_uri_str = if uri_str.starts_with("/") {
        format!("{}{}", path, query)
    } else {
        let scheme = uri.scheme_str().unwrap_or("http");
        match uri.authority() {
            Some(authority) => format!("{}://{}{}{}", scheme, authority.as_str(), path, query),
            None => format!("{}{}", path, query),
        }
    };
    new_uri_str.parse().unwrap_or_else(|_| uri.clone())
}

pub fn apply_url_rules(
    uri: &Uri,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Uri {
    let uri = apply_url_params(uri, rules);
    apply_url_replace(&uri, rules, verbose_logging, ctx)
}

pub fn extract_path_from_pattern(pattern: &str) -> Option<String> {
    let s = pattern
        .strip_prefix("http://")
        .or_else(|| pattern.strip_prefix("https://"))
        .or_else(|| pattern.strip_prefix("ws://"))
        .or_else(|| pattern.strip_prefix("wss://"))
        .unwrap_or(pattern);

    s.find('/')
        .map(|pos| s[pos..].to_string())
        .filter(|p| p != "/")
}

pub fn find_host_rule_source_path(
    rules: &[crate::server::RuleValue],
    host_protocol: bifrost_core::Protocol,
    host_rule: &str,
) -> Option<String> {
    use bifrost_core::Protocol;
    rules.iter().find_map(|rule| {
        if matches!(
            rule.protocol,
            Protocol::Host
                | Protocol::XHost
                | Protocol::Http
                | Protocol::Https
                | Protocol::Ws
                | Protocol::Wss
        ) && rule.protocol == host_protocol
            && rule.value == host_rule
        {
            extract_path_from_pattern(&rule.pattern)
        } else {
            None
        }
    })
}

pub fn host_rule_uses_exact_target_path(
    rules: &[crate::server::RuleValue],
    host_protocol: bifrost_core::Protocol,
    host_rule: &str,
) -> bool {
    use bifrost_core::Protocol;
    rules.iter().any(|rule| {
        matches!(
            rule.protocol,
            Protocol::Host
                | Protocol::XHost
                | Protocol::Http
                | Protocol::Https
                | Protocol::Ws
                | Protocol::Wss
        ) && rule.protocol == host_protocol
            && rule.value == host_rule
            && rule.pattern.trim_start_matches('!').starts_with('^')
    })
}

pub fn rewrite_path_with_prefix(
    request_path: &str,
    source_path: Option<&str>,
    target_path: &str,
) -> String {
    if let Some(source) = source_path {
        let source_trimmed = source.trim_end_matches('/');
        if let Some(remaining) = request_path.strip_prefix(source_trimmed) {
            let target = target_path.trim_end_matches('/');
            if remaining.is_empty() {
                if target_path.ends_with('/') {
                    format!("{}/", target)
                } else {
                    target.to_string()
                }
            } else if remaining.starts_with('/') || remaining.starts_with('?') {
                format!("{}{}", target, remaining)
            } else {
                format!("{}/{}", target, remaining)
            }
        } else {
            request_path.to_string()
        }
    } else {
        let target = target_path.trim_end_matches('/');
        if request_path == "/" {
            format!("{}/", target)
        } else if request_path.starts_with('/') {
            format!("{}{}", target, request_path)
        } else {
            format!("{}/{}", target, request_path)
        }
    }
}

pub fn extract_target_path_from_host_rule(host_rule: &str) -> Option<String> {
    let mut s = host_rule.trim();
    if s.is_empty() {
        return None;
    }
    for prefix in [
        "http://", "https://", "ws://", "wss://", "host://", "xhost://", "proxy://", "pac://",
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
            break;
        }
    }
    let uri: Uri = format!("http://{}", s).parse().ok()?;
    uri.path_and_query()
        .map(|pq| pq.as_str().to_string())
        .filter(|pq| pq != "/")
}

pub fn build_redirect_uri(base_uri: &Uri, redirect_target: &str) -> Option<String> {
    if redirect_target.starts_with("http://") || redirect_target.starts_with("https://") {
        return Some(redirect_target.to_string());
    }

    let scheme = base_uri.scheme_str().unwrap_or("http");
    let host = base_uri.host()?;
    let port = base_uri.port_u16();

    let host_port = if let Some(p) = port {
        if (scheme == "http" && p == 80) || (scheme == "https" && p == 443) {
            host.to_string()
        } else {
            format!("{}:{}", host, p)
        }
    } else {
        host.to_string()
    };

    if redirect_target.starts_with('/') {
        Some(format!("{}://{}{}", scheme, host_port, redirect_target))
    } else {
        let path = base_uri.path();
        let base_path = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        Some(format!(
            "{}://{}{}/{}",
            scheme, host_port, base_path, redirect_target
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::Protocol;

    fn mock_ctx() -> RequestContext {
        RequestContext::new()
    }

    #[test]
    fn test_apply_url_params_empty() {
        let uri: Uri = "/api/test".parse().unwrap();
        let rules = ResolvedRules::default();

        let result = apply_url_params(&uri, &rules);
        assert_eq!(result.to_string(), "/api/test");
    }

    #[test]
    fn test_apply_url_params_add_new() {
        let uri: Uri = "/api/test".parse().unwrap();
        let rules = ResolvedRules {
            url_params: vec![("key".to_string(), "value".to_string())],
            ..Default::default()
        };

        let result = apply_url_params(&uri, &rules);
        assert!(result.to_string().contains("key=value"));
    }

    #[test]
    fn test_apply_url_params_with_existing_query() {
        let uri: Uri = "/api/test?existing=1".parse().unwrap();
        let rules = ResolvedRules {
            url_params: vec![("new".to_string(), "2".to_string())],
            ..Default::default()
        };

        let result = apply_url_params(&uri, &rules);
        let result_str = result.to_string();
        assert!(result_str.contains("existing=1"));
        assert!(result_str.contains("new=2"));
    }

    #[test]
    fn test_apply_url_replace_path() {
        let uri: Uri = "/api/v1/users".parse().unwrap();
        let rules = ResolvedRules {
            url_replace: vec![("/v1/".to_string(), "/v2/".to_string())],
            ..Default::default()
        };

        let result = apply_url_replace(&uri, &rules, false, &mock_ctx());
        assert_eq!(result.path(), "/api/v2/users");
    }

    #[test]
    fn test_apply_url_replace_multiple() {
        let uri: Uri = "/old/path/old".parse().unwrap();
        let rules = ResolvedRules {
            url_replace: vec![("old".to_string(), "new".to_string())],
            ..Default::default()
        };

        let result = apply_url_replace(&uri, &rules, false, &mock_ctx());
        assert_eq!(result.path(), "/new/path/new");
    }

    #[test]
    fn test_apply_url_replace_preserves_query() {
        let uri: Uri = "/api/v1/users?page=1".parse().unwrap();
        let rules = ResolvedRules {
            url_replace: vec![("/v1/".to_string(), "/v2/".to_string())],
            ..Default::default()
        };

        let result = apply_url_replace(&uri, &rules, false, &mock_ctx());
        assert_eq!(result.path(), "/api/v2/users");
        assert_eq!(result.query(), Some("page=1"));
    }

    #[test]
    fn test_apply_url_replace_regex_normalizes_double_slashes() {
        let uri: Uri = "/legacy/v1/users".parse().unwrap();
        let rules = ResolvedRules {
            url_replace_regex: vec![crate::server::RegexReplace {
                pattern: Regex::new(r"/legacy/v\d+/").unwrap(),
                replacement: "/api/v99/".to_string(),
                global: false,
            }],
            ..Default::default()
        };

        let result = apply_url_replace(&uri, &rules, false, &mock_ctx());
        assert_eq!(result.path(), "/api/v99/users");
    }

    #[test]
    fn test_apply_url_replace_regex_normalizes_replacement_wrapped_with_slashes() {
        let uri: Uri = "/api/users".parse().unwrap();
        let rules = ResolvedRules {
            url_replace_regex: vec![crate::server::RegexReplace {
                pattern: Regex::new(r"/api/").unwrap(),
                replacement: "/edge/".to_string(),
                global: false,
            }],
            ..Default::default()
        };

        let result = apply_url_replace(&uri, &rules, false, &mock_ctx());
        assert_eq!(result.path(), "/edge/users");
    }

    #[test]
    fn test_build_redirect_uri_absolute() {
        let base: Uri = "http://example.com/path".parse().unwrap();
        let result = build_redirect_uri(&base, "https://other.com/new");
        assert_eq!(result, Some("https://other.com/new".to_string()));
    }

    #[test]
    fn test_build_redirect_uri_absolute_path() {
        let base: Uri = "http://example.com/old/path".parse().unwrap();
        let result = build_redirect_uri(&base, "/new/path");
        assert_eq!(result, Some("http://example.com/new/path".to_string()));
    }

    #[test]
    fn test_build_redirect_uri_relative() {
        let base: Uri = "http://example.com/old/path".parse().unwrap();
        let result = build_redirect_uri(&base, "newfile.html");
        assert_eq!(
            result,
            Some("http://example.com/old/newfile.html".to_string())
        );
    }

    #[test]
    fn test_build_redirect_uri_with_port() {
        let base: Uri = "http://example.com:8080/path".parse().unwrap();
        let result = build_redirect_uri(&base, "/new");
        assert_eq!(result, Some("http://example.com:8080/new".to_string()));
    }

    #[test]
    fn test_build_redirect_uri_default_port() {
        let base: Uri = "http://example.com:80/path".parse().unwrap();
        let result = build_redirect_uri(&base, "/new");
        assert_eq!(result, Some("http://example.com/new".to_string()));
    }

    #[test]
    fn test_extract_path_from_pattern_https_with_path() {
        assert_eq!(
            extract_path_from_pattern("https://example.com/labor_cost/static/"),
            Some("/labor_cost/static/".to_string())
        );
    }

    #[test]
    fn test_extract_path_from_pattern_http_with_path() {
        assert_eq!(
            extract_path_from_pattern("http://example.com/api/v1"),
            Some("/api/v1".to_string())
        );
    }

    #[test]
    fn test_extract_path_from_pattern_domain_with_path() {
        assert_eq!(
            extract_path_from_pattern("example.com/api/"),
            Some("/api/".to_string())
        );
    }

    #[test]
    fn test_extract_path_from_pattern_domain_only() {
        assert_eq!(extract_path_from_pattern("example.com"), None);
    }

    #[test]
    fn test_extract_path_from_pattern_https_root_path() {
        assert_eq!(extract_path_from_pattern("https://example.com/"), None);
    }

    #[test]
    fn test_find_host_rule_source_path_uses_selected_rule_not_later_host_rule() {
        use crate::server::RuleValue;
        use std::collections::HashMap;

        let rules = vec![
            RuleValue {
                pattern: "https://example.com/labor_cost/static/".to_string(),
                protocol: bifrost_core::Protocol::Http,
                value: "http://127.0.0.1:9000/labor_cost/static/".to_string(),
                options: HashMap::new(),
                rule_name: None,
                raw: None,
                line: None,
                auto_tls_intercept: false,
            },
            RuleValue {
                pattern: "https://example.com/other/".to_string(),
                protocol: bifrost_core::Protocol::Http,
                value: "http://127.0.0.1:9000/other/".to_string(),
                options: HashMap::new(),
                rule_name: None,
                raw: None,
                line: None,
                auto_tls_intercept: false,
            },
            RuleValue {
                pattern: "https://example.com/labor_cost/static/".to_string(),
                protocol: bifrost_core::Protocol::Https,
                value: "https://127.0.0.1:9443/labor_cost/static/".to_string(),
                options: HashMap::new(),
                rule_name: None,
                raw: None,
                line: None,
                auto_tls_intercept: false,
            },
        ];

        let path = find_host_rule_source_path(
            &rules,
            bifrost_core::Protocol::Http,
            "http://127.0.0.1:9000/labor_cost/static/",
        );
        assert_eq!(path, Some("/labor_cost/static/".to_string()));
    }

    #[test]
    fn test_rewrite_path_same_source_target() {
        let result = rewrite_path_with_prefix(
            "/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg",
            Some("/labor_cost/static/"),
            "/labor_cost/static/",
        );
        assert_eq!(
            result,
            "/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg"
        );
    }

    #[test]
    fn test_rewrite_path_different_source_target() {
        let result = rewrite_path_with_prefix("/old/api/users", Some("/old/api/"), "/new/api/");
        assert_eq!(result, "/new/api/users");
    }

    #[test]
    fn test_rewrite_path_with_query_string() {
        let result = rewrite_path_with_prefix(
            "/labor_cost/static/file.svg?v=1",
            Some("/labor_cost/static/"),
            "/labor_cost/static/",
        );
        assert_eq!(result, "/labor_cost/static/file.svg?v=1");
    }

    #[test]
    fn test_rewrite_path_source_without_trailing_slash() {
        let result = rewrite_path_with_prefix("/api/v1/users", Some("/api/v1"), "/api/v2");
        assert_eq!(result, "/api/v2/users");
    }

    #[test]
    fn test_rewrite_path_no_source_path() {
        let result = rewrite_path_with_prefix("/users", None, "/api/");
        assert_eq!(result, "/api/users");
    }

    #[test]
    fn test_rewrite_path_no_source_path_root_request() {
        let result = rewrite_path_with_prefix("/", None, "/api/");
        assert_eq!(result, "/api/");
    }

    #[test]
    fn test_host_rule_uses_exact_target_path_for_path_wildcard() {
        let rules = vec![crate::server::RuleValue {
            protocol: Protocol::Http,
            value: "127.0.0.1:8999/approvals".to_string(),
            pattern:
                "^https://cdn-tos-cn.bytedance.net/obj/archi/obj/okrx-web/approvals-web/1.0.0.*/index.html"
                    .to_string(),
            options: std::collections::HashMap::new(),
            line: Some(1),
            raw: None,
            rule_name: None,
            auto_tls_intercept: false,
        }];

        assert!(host_rule_uses_exact_target_path(
            &rules,
            Protocol::Http,
            "127.0.0.1:8999/approvals",
        ));
    }

    #[test]
    fn test_rewrite_path_exact_match() {
        let result = rewrite_path_with_prefix(
            "/labor_cost/static/",
            Some("/labor_cost/static/"),
            "/labor_cost/static/",
        );
        assert_eq!(result, "/labor_cost/static/");
    }

    #[test]
    fn test_rewrite_path_exact_match_preserves_target_without_trailing_slash() {
        let result = rewrite_path_with_prefix(
            "/labor_cost/static/__webpack_hmr",
            Some("/labor_cost/static/__webpack_hmr"),
            "/__webpack_hmr",
        );
        assert_eq!(result, "/__webpack_hmr");
    }

    #[test]
    fn test_rewrite_path_source_no_match_fallback() {
        let result = rewrite_path_with_prefix("/other/path/file.txt", Some("/api/"), "/new/");
        assert_eq!(result, "/other/path/file.txt");
    }

    #[test]
    fn test_extract_target_path_from_host_rule_with_port_and_path() {
        assert_eq!(
            extract_target_path_from_host_rule("localhost:9000/labor_cost/static/"),
            Some("/labor_cost/static/".to_string())
        );
    }

    #[test]
    fn test_extract_target_path_from_host_rule_no_path() {
        assert_eq!(extract_target_path_from_host_rule("localhost:9000"), None);
    }

    #[test]
    fn test_extract_target_path_from_host_rule_with_scheme() {
        assert_eq!(
            extract_target_path_from_host_rule("http://localhost:9000/api/"),
            Some("/api/".to_string())
        );
    }

    #[test]
    fn test_apply_url_replace_preserves_scheme_and_authority_on_absolute_uri() {
        // Regression: absolute-form proxy URIs must retain scheme + authority
        // after URL replace; otherwise downstream `extract_host_port` cannot
        // locate the target. See `www.example.com urlReplace://api/=`.
        let uri: Uri = "http://www.example.com/api/demo".parse().unwrap();
        let rules = ResolvedRules {
            url_replace: vec![("api/".to_string(), "".to_string())],
            ..Default::default()
        };

        let result = apply_url_replace(&uri, &rules, false, &mock_ctx());
        assert_eq!(result.scheme_str(), Some("http"));
        assert_eq!(result.host(), Some("www.example.com"));
        assert_eq!(result.path(), "/demo");
    }

    #[test]
    fn test_apply_url_replace_absolute_https_preserves_query() {
        let uri: Uri = "https://www.example.com/api/demo?x=1&y=2".parse().unwrap();
        let rules = ResolvedRules {
            url_replace: vec![("api/".to_string(), "".to_string())],
            ..Default::default()
        };

        let result = apply_url_replace(&uri, &rules, false, &mock_ctx());
        assert_eq!(result.scheme_str(), Some("https"));
        assert_eq!(result.host(), Some("www.example.com"));
        assert_eq!(result.path(), "/demo");
        assert_eq!(result.query(), Some("x=1&y=2"));
    }

    #[test]
    fn test_apply_url_replace_delete_prefix_with_leading_slash() {
        // Regression: deleting `/api/` from `/api/demo` must not strip the
        // leading `/` (would yield the relative-looking path `demo`).
        let uri: Uri = "/api/demo".parse().unwrap();
        let rules = ResolvedRules {
            url_replace: vec![("/api/".to_string(), "".to_string())],
            ..Default::default()
        };

        let result = apply_url_replace(&uri, &rules, false, &mock_ctx());
        assert_eq!(result.path(), "/demo");
    }
}
