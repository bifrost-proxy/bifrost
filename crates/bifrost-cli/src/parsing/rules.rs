use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bifrost_core::{Protocol, RequestContext, Rule, RulesResolver as CoreRulesResolver};
use bifrost_proxy::{
    ResolvedRules as ProxyResolvedRules, RuleValue, RulesResolver as ProxyRulesResolverTrait,
};
use parking_lot::RwLock;

use super::{parse_cors_config, parse_header_value, parse_replace_value, parse_res_cookies_value};

fn extract_inline_content(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.starts_with('(') && trimmed.ends_with(')') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        value
    }
}

fn insert_merge_leaf(
    target: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: &str,
) {
    target.insert(
        key.trim().to_string(),
        serde_json::Value::String(value.trim().to_string()),
    );
}

fn parse_merge_value(value: &str) -> Option<serde_json::Value> {
    if let Ok(json_value) = serde_json::from_str(value) {
        return Some(json_value);
    }

    let trimmed = value.trim();
    let form_candidate = if trimmed.starts_with('(') && trimmed.ends_with(')') && trimmed.len() >= 2
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    if form_candidate.contains('=') {
        let mut object = serde_json::Map::new();
        for pair in form_candidate.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                insert_merge_leaf(&mut object, k, v);
            }
        }
        if !object.is_empty() {
            return Some(serde_json::Value::Object(object));
        }
    }

    if let Some(params) = parse_header_value(value) {
        let mut object = serde_json::Map::new();
        for (k, v) in params {
            insert_merge_leaf(&mut object, &k, &v);
        }
        if !object.is_empty() {
            return Some(serde_json::Value::Object(object));
        }
    }

    let mut object = serde_json::Map::new();
    for line in value.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((key, raw_value)) = trimmed.split_once(':') {
            insert_merge_leaf(&mut object, key, raw_value);
        }
    }

    if object.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(object))
    }
}

fn parse_redirect_target(value: &str) -> (Option<u16>, String) {
    if let Some((status_part, location)) = value.split_once(':') {
        if status_part.len() == 3 && status_part.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(status) = status_part.parse::<u16>() {
                if (300..=399).contains(&status) && !location.is_empty() {
                    return (Some(status), location.to_string());
                }
            }
        }
    }

    (None, value.to_string())
}

fn parse_pac_proxy_target(value: &str) -> Option<String> {
    let upper = value.to_ascii_uppercase();
    if upper.contains("DIRECT") && !upper.contains("PROXY") {
        return None;
    }

    if let Some(proxy_pos) = upper.find("PROXY") {
        let after = &value[proxy_pos + 5..];
        let trimmed = after
            .trim_start_matches(|c: char| c.is_whitespace() || c == ':')
            .trim();
        if trimmed.is_empty() {
            return None;
        }
        let mut end = trimmed.len();
        for (idx, ch) in trimmed.char_indices() {
            if ch.is_whitespace() || ch == ';' || ch == '"' || ch == '\'' {
                end = idx;
                break;
            }
        }
        let host_port = trimmed[..end].trim();
        if host_port.is_empty() {
            None
        } else {
            Some(host_port.to_string())
        }
    } else {
        None
    }
}

pub fn parse_cli_rules(
    rules: &[String],
    rules_file: &Option<PathBuf>,
    values: &HashMap<String, String>,
) -> bifrost_core::Result<(Vec<Rule>, HashMap<String, String>)> {
    let mut all_rules = Vec::new();
    let mut merged_values = values.clone();

    let parser = bifrost_core::RuleParser::with_values(values.clone());

    for rule_str in rules {
        match parser.parse_rules(rule_str) {
            Ok(parsed) => all_rules.extend(parsed),
            Err(e) => {
                return Err(bifrost_core::BifrostError::Config(format!(
                    "Failed to parse rule '{}': {}",
                    rule_str, e
                )));
            }
        }
    }

    if let Some(file_path) = rules_file {
        let content = std::fs::read_to_string(file_path).map_err(|e| {
            bifrost_core::BifrostError::Config(format!(
                "Failed to read rules file '{}': {}",
                file_path.display(),
                e
            ))
        })?;
        let parser_with_file = bifrost_core::RuleParser::with_values(merged_values.clone());
        match parser_with_file.parse_rules_with_inline_values(&content) {
            Ok((parsed, inline_values)) => {
                all_rules.extend(parsed);
                for (k, v) in inline_values {
                    merged_values.entry(k).or_insert(v);
                }
            }
            Err(e) => {
                return Err(bifrost_core::BifrostError::Config(format!(
                    "Failed to parse rules file '{}': {}",
                    file_path.display(),
                    e
                )));
            }
        }
    }

    Ok((all_rules, merged_values))
}

pub struct DynamicRulesResolver {
    inner: RwLock<CoreRulesResolver>,
    cli_rules: Vec<Rule>,
}

impl DynamicRulesResolver {
    pub fn new(
        cli_rules: Vec<Rule>,
        stored_rules: Vec<Rule>,
        values: HashMap<String, String>,
    ) -> Self {
        let mut all_rules = cli_rules.clone();
        all_rules.extend(stored_rules);

        let inner = CoreRulesResolver::new(all_rules).with_values(values);
        Self {
            inner: RwLock::new(inner),
            cli_rules,
        }
    }

    pub fn update_stored_rules(&self, stored_rules: Vec<Rule>, values: HashMap<String, String>) {
        let stored_count = stored_rules.len();
        let mut all_rules = self.cli_rules.clone();
        all_rules.extend(stored_rules);

        let new_resolver = CoreRulesResolver::new(all_rules).with_values(values);
        let mut inner = self.inner.write();
        *inner = new_resolver;

        tracing::info!(
            target: "bifrost_cli::rules",
            cli_count = self.cli_rules.len(),
            stored_count = stored_count,
            "rules resolver updated with new stored rules"
        );
    }

    pub fn cli_rules(&self) -> &[Rule] {
        &self.cli_rules
    }

    pub fn get_tls_rule_patterns(&self) -> (Vec<String>, Vec<String>) {
        let inner = self.inner.read();
        let mut intercept_patterns = Vec::new();
        let mut passthrough_patterns = Vec::new();

        for rule in inner.rules() {
            if rule.is_disabled() {
                continue;
            }
            match rule.protocol {
                Protocol::TlsIntercept => {
                    intercept_patterns.push(rule.pattern.clone());
                }
                Protocol::TlsPassthrough => {
                    passthrough_patterns.push(rule.pattern.clone());
                }
                _ => {}
            }
        }

        (intercept_patterns, passthrough_patterns)
    }
}

impl ProxyRulesResolverTrait for DynamicRulesResolver {
    fn values(&self) -> std::collections::HashMap<String, String> {
        let inner = self.inner.read();
        inner.values().clone()
    }

    fn resolve_with_context(
        &self,
        url: &str,
        method: &str,
        req_headers: &std::collections::HashMap<String, String>,
        req_cookies: &std::collections::HashMap<String, String>,
    ) -> ProxyResolvedRules {
        let inner = self.inner.read();
        resolve_rules_impl(&inner, url, method, req_headers, req_cookies)
    }

    fn has_response_rules_for_host(&self, host: &str) -> bool {
        let inner = self.inner.read();
        inner.has_response_rules_for_host(host)
    }
}

fn resolve_rules_impl(
    resolver: &CoreRulesResolver,
    url: &str,
    method: &str,
    req_headers: &std::collections::HashMap<String, String>,
    req_cookies: &std::collections::HashMap<String, String>,
) -> ProxyResolvedRules {
    let mut ctx = RequestContext::from_url(url);
    ctx.method = method.to_string();
    ctx.client_ip = "127.0.0.1".to_string();
    ctx.req_headers = req_headers.clone();
    ctx.req_cookies = req_cookies.clone();

    let core_result = resolver.resolve(&ctx);

    if core_result.rules.is_empty() {
        tracing::debug!(
            target: "bifrost_proxy::rules",
            url = %url,
            "no rules matched"
        );
    } else {
        tracing::info!(
            target: "bifrost_proxy::rules",
            url = %url,
            matched_count = core_result.rules.len(),
            "rules matched for request"
        );
        for (idx, resolved) in core_result.rules.iter().enumerate() {
            let rule = &resolved.rule;
            tracing::info!(
                target: "bifrost_proxy::rules",
                rule_index = idx + 1,
                pattern = %rule.pattern,
                protocol = %rule.protocol.to_str(),
                value = %resolved.resolved_value,
                raw = %rule.raw,
                file = rule.file.as_deref().unwrap_or("<cli>"),
                line = rule.line.unwrap_or(0),
                disabled = rule.is_disabled(),
                "matched rule detail"
            );
        }
    }

    let mut result = convert_core_result_to_proxy(&core_result);
    result.values = resolver.values().clone();
    result
}

fn convert_core_result_to_proxy(core_result: &bifrost_core::ResolvedRules) -> ProxyResolvedRules {
    let mut result = ProxyResolvedRules::default();

    for resolved_rule in &core_result.rules {
        let protocol = resolved_rule.rule.protocol;
        let value = &resolved_rule.resolved_value;
        let pattern = &resolved_rule.rule.pattern;

        result.rules.push(RuleValue {
            pattern: pattern.clone(),
            protocol,
            value: value.clone(),
            options: HashMap::new(),
            rule_name: resolved_rule.rule.file.clone(),
            raw: Some(resolved_rule.rule.raw.clone()),
            line: resolved_rule.rule.line,
        });

        match protocol {
            Protocol::Host
            | Protocol::XHost
            | Protocol::Http
            | Protocol::Https
            | Protocol::Ws
            | Protocol::Wss => {
                if !result.ignored.host {
                    result.host = Some(value.to_string());
                    result.host_protocol = Some(protocol);
                }
            }
            Protocol::Redirect => {
                let (status, location) = parse_redirect_target(value);
                result.redirect = Some(location);
                result.redirect_status = status;
            }
            Protocol::ReqHeaders => {
                if let Some(headers) = parse_header_value(value) {
                    for (k, v) in headers {
                        let key_lower = k.to_lowercase();
                        if !result
                            .req_headers
                            .iter()
                            .any(|(existing, _)| existing.to_lowercase() == key_lower)
                        {
                            result.req_headers.push((k, v));
                        }
                    }
                }
            }
            Protocol::ResHeaders => {
                if let Some(headers) = parse_header_value(value) {
                    for (k, v) in headers {
                        let key_lower = k.to_lowercase();
                        if !result
                            .res_headers
                            .iter()
                            .any(|(existing, _)| existing.to_lowercase() == key_lower)
                        {
                            result.res_headers.push((k, v));
                        }
                    }
                }
            }
            Protocol::StatusCode => {
                if let Ok(code) = value.parse::<u16>() {
                    result.status_code = Some(code);
                }
            }
            Protocol::ReplaceStatus => {
                if let Ok(code) = value.parse::<u16>() {
                    result.replace_status = Some(code);
                }
            }
            Protocol::ResBody => {
                let content = extract_inline_content(value);
                result.res_body = Some(bytes::Bytes::from(content.to_string()));
            }
            Protocol::ReqBody => {
                let content = extract_inline_content(value);
                result.req_body = Some(bytes::Bytes::from(content.to_string()));
            }
            Protocol::Proxy => {
                result.proxy = Some(value.to_string());
            }
            Protocol::Http3 => {
                result.upstream_http3 = true;
            }
            Protocol::Pac => {
                if let Some(target) = parse_pac_proxy_target(value) {
                    result.host = Some(target);
                    result.host_protocol = Some(Protocol::Host);
                }
            }
            Protocol::ReqCors => {
                let cors = parse_cors_config(value);
                result.req_cors = cors;
            }
            Protocol::ResCors => {
                let cors = parse_cors_config(value);
                result.res_cors = cors;
            }
            Protocol::File => {
                result.mock_file = Some(value.to_string());
            }
            Protocol::Tpl => {
                result.mock_template = Some(value.to_string());
            }
            Protocol::RawFile => {
                result.mock_rawfile = Some(value.to_string());
            }
            Protocol::Ua => {
                result.ua = Some(value.to_string());
            }
            Protocol::Referer => {
                result.referer = Some(value.to_string());
            }
            Protocol::Method => {
                result.method = Some(value.to_string());
            }
            Protocol::ReqDelay => {
                if let Ok(delay) = value.parse::<u64>() {
                    result.req_delay = Some(delay);
                }
            }
            Protocol::ResDelay => {
                if let Ok(delay) = value.parse::<u64>() {
                    result.res_delay = Some(delay);
                }
            }
            Protocol::ReqCookies => {
                if let Some(cookies) = parse_header_value(value) {
                    for (k, v) in cookies {
                        result.req_cookies.push((k, v));
                    }
                }
            }
            Protocol::ForwardedFor => {
                result
                    .req_headers
                    .push(("x-forwarded-for".to_string(), value.to_string()));
            }
            Protocol::ResCookies => {
                let parsed_cookies = parse_res_cookies_value(value);
                result.res_cookies.extend(parsed_cookies);
            }
            Protocol::ResponseFor => {
                result
                    .res_headers
                    .push(("x-bifrost-response-for".to_string(), value.to_string()));
            }
            Protocol::ReqPrepend => {
                let content = extract_inline_content(value);
                result.req_prepend = Some(bytes::Bytes::from(content.to_string()));
            }
            Protocol::ReqAppend => {
                let content = extract_inline_content(value);
                result.req_append = Some(bytes::Bytes::from(content.to_string()));
            }
            Protocol::ResPrepend => {
                let content = extract_inline_content(value);
                result.res_prepend = Some(bytes::Bytes::from(content.to_string()));
            }
            Protocol::ResAppend => {
                let content = extract_inline_content(value);
                result.res_append = Some(bytes::Bytes::from(content.to_string()));
            }
            Protocol::ReqReplace => {
                let parsed = parse_replace_value(value);
                result.req_replace.extend(parsed.string_rules);
                result.req_replace_regex.extend(parsed.regex_rules);
            }
            Protocol::ResReplace => {
                let parsed = parse_replace_value(value);
                result.res_replace.extend(parsed.string_rules);
                result.res_replace_regex.extend(parsed.regex_rules);
            }
            Protocol::Params => {
                if let Some(json_value) = parse_merge_value(value) {
                    result.req_merge = Some(json_value);
                }
            }
            Protocol::ResMerge => {
                if let Some(json_value) = parse_merge_value(value) {
                    result.res_merge = Some(json_value);
                }
            }
            Protocol::UrlParams => {
                if let Some(params) = parse_header_value(value) {
                    for (k, v) in params {
                        if v.is_empty() {
                            result.delete_url_params.push(k);
                        } else {
                            result.url_params.push((k, v));
                        }
                    }
                }
            }
            Protocol::UrlReplace => {
                let parsed = parse_replace_value(value);
                result.url_replace.extend(parsed.string_rules);
                result.url_replace_regex.extend(parsed.regex_rules);
            }
            Protocol::ReqType => {
                result.req_type = Some(value.to_string());
            }
            Protocol::ReqCharset => {
                result.req_charset = Some(value.to_string());
            }
            Protocol::ResType => {
                result.res_type = Some(value.to_string());
            }
            Protocol::ResCharset => {
                result.res_charset = Some(value.to_string());
            }
            Protocol::Cache => {
                result.cache = Some(value.to_string());
            }
            Protocol::Attachment => {
                result.attachment = Some(value.to_string());
            }
            Protocol::HtmlAppend => {
                result.html_append = Some(value.to_string());
            }
            Protocol::HtmlPrepend => {
                result.html_prepend = Some(value.to_string());
            }
            Protocol::HtmlBody => {
                result.html_body = Some(value.to_string());
            }
            Protocol::JsAppend => {
                result.js_append = Some(value.to_string());
            }
            Protocol::JsPrepend => {
                result.js_prepend = Some(value.to_string());
            }
            Protocol::JsBody => {
                result.js_body = Some(value.to_string());
            }
            Protocol::CssAppend => {
                result.css_append = Some(value.to_string());
            }
            Protocol::CssPrepend => {
                result.css_prepend = Some(value.to_string());
            }
            Protocol::CssBody => {
                result.css_body = Some(value.to_string());
            }
            Protocol::ReqSpeed => {
                if let Ok(speed) = value.parse::<u64>() {
                    result.req_speed = Some(speed.saturating_mul(1024));
                }
            }
            Protocol::ResSpeed => {
                if let Ok(speed) = value.parse::<u64>() {
                    result.res_speed = Some(speed.saturating_mul(1024));
                }
            }
            Protocol::Trailers => {
                if let Some(headers) = parse_header_value(value) {
                    for (k, v) in headers {
                        result.trailers.push((k, v));
                    }
                }
            }
            Protocol::Dns => {
                result.dns_servers.push(value.to_string());
            }
            Protocol::TlsIntercept => {
                result.tls_intercept = Some(true);
            }
            Protocol::TlsPassthrough => {
                result.tls_intercept = Some(false);
            }
            Protocol::TlsOptions => {
                result.tls_options = Some(value.to_string());
            }
            Protocol::SniCallback => {
                result.sni_callback = Some(value.to_string());
            }
            Protocol::Passthrough => {
                result.ignored.host = true;
            }
            Protocol::Tunnel => {
                result.host = Some(value.to_string());
                result.host_protocol = Some(Protocol::Tunnel);
            }
            Protocol::ReqScript => {
                result.req_scripts.push(value.to_string());
            }
            Protocol::ResScript => {
                result.res_scripts.push(value.to_string());
            }
            Protocol::Decode => {
                result.decode_scripts.push(value.to_string());
            }
            Protocol::Auth => {
                result.auth = Some(value.to_string());
            }
            Protocol::Delete => {
                let parsed = parse_delete_value(value);
                result.delete_req_headers.extend(parsed.req_headers);
                result.delete_res_headers.extend(parsed.res_headers);
                result.delete_url_params.extend(parsed.url_params);
            }
            Protocol::HeaderReplace => {
                if let Some(rules) = parse_header_replace_value(value) {
                    result.header_replace.extend(rules);
                }
            }
            _ => {}
        }
    }

    result
}

struct ParsedDeleteValue {
    req_headers: Vec<String>,
    res_headers: Vec<String>,
    url_params: Vec<String>,
}

fn parse_delete_value(value: &str) -> ParsedDeleteValue {
    let mut result = ParsedDeleteValue {
        req_headers: Vec::new(),
        res_headers: Vec::new(),
        url_params: Vec::new(),
    };

    for part in value.split('|') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        if let Some(header) = part.strip_prefix("reqHeaders.") {
            result.req_headers.push(header.to_string());
        } else if let Some(header) = part.strip_prefix("resHeaders.") {
            result.res_headers.push(header.to_string());
        } else if let Some(header) = part.strip_prefix("req.") {
            result.req_headers.push(header.to_string());
        } else if let Some(header) = part.strip_prefix("res.") {
            result.res_headers.push(header.to_string());
        } else if let Some(param) = part.strip_prefix("urlParams.") {
            result.url_params.push(param.to_string());
        } else {
            result.req_headers.push(part.to_string());
            result.res_headers.push(part.to_string());
        }
    }

    result
}

fn parse_header_replace_value(value: &str) -> Option<Vec<bifrost_proxy::HeaderReplaceRule>> {
    use bifrost_proxy::{HeaderReplaceRule, HeaderReplaceTarget};

    let mut rules = Vec::new();

    for part in value.split('|') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let (target, rest) = if let Some(rest) = part.strip_prefix("req.") {
            (HeaderReplaceTarget::Request, rest)
        } else if let Some(rest) = part.strip_prefix("res.") {
            (HeaderReplaceTarget::Response, rest)
        } else {
            continue;
        };

        let colon_pos = rest.find(':')?;
        let header_name = rest[..colon_pos].to_string();
        let pattern_replacement = &rest[colon_pos + 1..];

        let eq_pos = pattern_replacement.find('=')?;
        let pattern = pattern_replacement[..eq_pos].to_string();
        let replacement = pattern_replacement[eq_pos + 1..].to_string();

        rules.push(HeaderReplaceRule {
            target,
            header_name,
            pattern,
            replacement,
        });
    }

    if rules.is_empty() {
        None
    } else {
        Some(rules)
    }
}

pub type SharedDynamicRulesResolver = Arc<DynamicRulesResolver>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http3_rule_enables_upstream_http3_flag() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser.parse_rules("example.com http3://").unwrap();
        let resolver = CoreRulesResolver::new(rules);

        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert!(resolved.upstream_http3);
    }

    #[test]
    fn test_h3_alias_enables_upstream_http3_flag() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser.parse_rules("example.com h3://").unwrap();
        let resolver = CoreRulesResolver::new(rules);

        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert!(resolved.upstream_http3);
    }

    #[test]
    fn test_delete_rule_supports_reqheaders_and_resheaders_prefixes() {
        let parsed =
            parse_delete_value("reqHeaders.X-Debug|resHeaders.X-Echo-Server|urlParams.trace");

        assert_eq!(parsed.req_headers, vec!["X-Debug"]);
        assert_eq!(parsed.res_headers, vec!["X-Echo-Server"]);
        assert_eq!(parsed.url_params, vec!["trace"]);
    }
}
