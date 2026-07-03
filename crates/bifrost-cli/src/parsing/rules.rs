use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bifrost_core::{Protocol, RequestContext, Rule, RulesResolver as CoreRulesResolver};
use bifrost_proxy::{
    DevtoolsInjectMode, DevtoolsMode, DevtoolsRule, ResolvedRules as ProxyResolvedRules, RuleValue,
    RulesResolver as ProxyRulesResolverTrait,
};
use bifrost_script::{parse_pac_decision, PacDecision, PacEngine, PacEngineConfig, PacProxyScheme};
use parking_lot::RwLock;
use url::Url;

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

fn parse_url_params_value(value: &str) -> Option<Vec<(String, String)>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let content = if trimmed.starts_with('(') && trimmed.ends_with(')') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    let mut params = Vec::new();
    for part in content.split(['\n', ',', '&']).map(str::trim) {
        if part.is_empty() {
            continue;
        }

        let split = part.split_once('=').or_else(|| part.split_once(':'));
        if let Some((key, value)) = split {
            let key = key.trim();
            if !key.is_empty() {
                params.push((key.to_string(), value.trim().to_string()));
            }
        }
    }

    if params.is_empty() {
        None
    } else {
        Some(params)
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

fn normalize_pac_proxy_url(scheme: PacProxyScheme, host_port: &str) -> Option<String> {
    let proxy_scheme = scheme.as_proxy_url_scheme()?;
    if host_port.contains("://") {
        Some(host_port.to_string())
    } else {
        Some(format!("{}://{}", proxy_scheme, host_port))
    }
}

fn parse_inline_pac_decision(value: &str) -> Option<PacDecision> {
    let trimmed = value.trim();
    let decision_start = trimmed.trim_start_matches('(').trim_start();
    let first_token = decision_start
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches('"')
        .trim_matches('\'');
    if !matches!(
        first_token.to_ascii_uppercase().as_str(),
        "DIRECT" | "PROXY" | "HTTP" | "HTTPS" | "SOCKS" | "SOCKS5"
    ) {
        return None;
    }
    parse_pac_decision(trimmed).ok()
}

fn parse_devtools_rule(value: &str) -> DevtoolsRule {
    let mut rule = DevtoolsRule {
        raw_value: value.to_string(),
        ..Default::default()
    };

    for part in value.split([',', '&']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, raw_value) = part.split_once('=').unwrap_or((part, "true"));
        let key = key.trim();
        let raw_value = raw_value.trim();
        match key {
            "mode" if raw_value.eq_ignore_ascii_case("control") => {
                rule.mode = DevtoolsMode::Control;
            }
            "mode" if raw_value.eq_ignore_ascii_case("read") => {
                rule.mode = DevtoolsMode::Read;
            }
            "inject" if raw_value.eq_ignore_ascii_case("bridge") => {
                rule.inject = DevtoolsInjectMode::Bridge;
            }
            "inject" if raw_value.eq_ignore_ascii_case("off") => {
                rule.inject = DevtoolsInjectMode::Off;
            }
            "inject" if raw_value.eq_ignore_ascii_case("auto") => {
                rule.inject = DevtoolsInjectMode::Auto;
            }
            "deny" => {
                rule.deny = matches!(
                    raw_value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                );
            }
            "evaluate_allowlist" => {
                rule.evaluate_allowlist = parse_devtools_evaluate_allowlist(raw_value);
            }
            _ => {}
        }
    }

    rule
}

fn parse_devtools_evaluate_allowlist(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(trimmed);
    inner
        .split('|')
        .map(|item| item.trim().trim_matches('"').trim_matches('\''))
        .filter(|item| !item.is_empty())
        .map(|item| item.replace("\\\\", "\\"))
        .collect()
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

    fn resolve_with_response_context(
        &self,
        url: &str,
        method: &str,
        req_headers: &std::collections::HashMap<String, String>,
        req_cookies: &std::collections::HashMap<String, String>,
        res_status: u16,
        res_headers: &std::collections::HashMap<String, String>,
    ) -> ProxyResolvedRules {
        let inner = self.inner.read();
        resolve_rules_with_response_impl(
            &inner,
            url,
            method,
            req_headers,
            req_cookies,
            res_status,
            res_headers,
        )
    }

    fn has_response_rules_for_host(&self, host: &str) -> bool {
        let inner = self.inner.read();
        inner.has_response_rules_for_host(host)
    }

    fn has_tls_auto_intercept_route_rules_for_host(&self, host: &str) -> bool {
        let inner = self.inner.read();
        inner.has_tls_auto_intercept_route_rules_for_host(host)
    }
}

/// Response-phase re-resolution: rebuilds the request context, attaches the upstream
/// response (status + lowercased headers) and resolves WITHOUT the cache, so
/// response-dependent filters (`s:`/`resH:`) are evaluated against the real response.
fn resolve_rules_with_response_impl(
    resolver: &CoreRulesResolver,
    url: &str,
    method: &str,
    req_headers: &std::collections::HashMap<String, String>,
    req_cookies: &std::collections::HashMap<String, String>,
    res_status: u16,
    res_headers: &std::collections::HashMap<String, String>,
) -> ProxyResolvedRules {
    let mut ctx = RequestContext::from_url(url);
    ctx.method = method.to_string();
    ctx.client_ip = "127.0.0.1".to_string();
    ctx.req_headers = req_headers.clone();
    ctx.req_cookies = req_cookies.clone();
    let res_headers_lower: std::collections::HashMap<String, String> = res_headers
        .iter()
        .map(|(k, v)| (k.to_lowercase(), v.clone()))
        .collect();
    ctx.set_response(res_status, res_headers_lower);

    let core_result = resolver.resolve_uncached(&ctx);
    let mut result = convert_core_result_to_proxy(&core_result);
    apply_pac_rules(resolver, &core_result, &mut result, &ctx, true);
    result.values = resolver.values().clone();
    result
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
        tracing::debug!(
            target: "bifrost_proxy::rules",
            url = %url,
            matched_count = core_result.rules.len(),
            "rules matched for request"
        );
        for (idx, resolved) in core_result.rules.iter().enumerate() {
            let rule = &resolved.rule;
            tracing::trace!(
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
    apply_pac_rules(resolver, &core_result, &mut result, &ctx, false);
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
            auto_tls_intercept: resolved_rule.rule.matcher.can_trigger_tls_auto_intercept(),
        });

        match protocol {
            Protocol::Host
            | Protocol::XHost
            | Protocol::Http
            | Protocol::Https
            | Protocol::Ws
            | Protocol::Wss
                if should_update_route_target(&result, protocol) =>
            {
                result.host = Some(value.to_string());
                result.host_protocol = Some(protocol);
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
            Protocol::Pac => {}
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
                result.forwarded_for = Some(value.to_string());
            }
            Protocol::ResCookies => {
                let parsed_cookies = parse_res_cookies_value(value);
                result.res_cookies.extend(parsed_cookies);
            }
            Protocol::ResponseFor => {
                result.response_for = Some(value.to_string());
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
                if let Some(params) = parse_url_params_value(value) {
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
            Protocol::UpstreamUnsafeSsl => {
                result.upstream_unsafe_ssl = parse_rule_bool(value, true);
            }
            Protocol::SniCallback => {
                result.sni_callback = Some(value.to_string());
            }
            Protocol::DevTools => {
                result.devtools = Some(parse_devtools_rule(value));
            }
            Protocol::Passthrough if !result.ignored.host && result.host.is_none() => {
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
            Protocol::Bp => {
                result.bp_scripts.push(value.to_string());
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

fn apply_pac_rules(
    resolver: &CoreRulesResolver,
    initial_result: &bifrost_core::ResolvedRules,
    result: &mut ProxyResolvedRules,
    original_ctx: &RequestContext,
    response_phase: bool,
) {
    let final_url =
        build_final_url_for_pac(original_ctx, result).unwrap_or_else(|| original_ctx.url.clone());
    let mut pac_ctx = RequestContext::from_url(&final_url);
    pac_ctx.method = original_ctx.method.clone();
    pac_ctx.client_ip = original_ctx.client_ip.clone();
    pac_ctx.req_headers = original_ctx.req_headers.clone();
    pac_ctx.req_cookies = original_ctx.req_cookies.clone();
    if response_phase {
        if let (Some(status), Some(headers)) =
            (original_ctx.status_code, original_ctx.res_headers.clone())
        {
            pac_ctx.set_response(status, headers);
        }
    }

    let pac_matches = if final_url == original_ctx.url {
        initial_result.clone()
    } else if response_phase {
        resolver.resolve_uncached(&pac_ctx)
    } else {
        resolver.resolve(&pac_ctx)
    };

    let Some(pac_rule) = pac_matches
        .rules
        .iter()
        .find(|resolved| resolved.rule.protocol == Protocol::Pac)
    else {
        return;
    };

    let decision = match parse_inline_pac_decision(&pac_rule.resolved_value) {
        Some(decision) => Ok(decision),
        None => {
            let engine = PacEngine::new(PacEngineConfig::default());
            engine.evaluate(
                &pac_rule.resolved_value,
                &pac_ctx.url,
                pac_ctx.hostname.as_str(),
            )
        }
    };

    match decision {
        Ok(PacDecision::Direct) => {
            result.proxy = None;
        }
        Ok(PacDecision::Proxy { scheme, host_port }) => {
            if let Some(proxy_url) = normalize_pac_proxy_url(scheme, &host_port) {
                result.proxy = Some(proxy_url);
            } else {
                set_pac_fail_closed_response(
                    result,
                    format!("unsupported PAC proxy scheme for {host_port}"),
                );
                tracing::warn!(
                    target: "bifrost_cli::rules",
                    pac_result = %host_port,
                    "PAC returned an unsupported proxy scheme"
                );
            }
        }
        Err(err) => {
            set_pac_fail_closed_response(result, err.to_string());
            tracing::warn!(
                target: "bifrost_cli::rules",
                url = %pac_ctx.url,
                error = %err,
                "PAC evaluation failed"
            );
        }
    }
}

fn set_pac_fail_closed_response(result: &mut ProxyResolvedRules, message: impl Into<String>) {
    let message = message.into();
    result.proxy = None;
    result.status_code = Some(502);
    result.res_body = Some(bytes::Bytes::from(format!(
        "Bifrost PAC evaluation failed: {message}\n"
    )));
    result.res_headers.push((
        "Content-Type".to_string(),
        "text/plain; charset=utf-8".to_string(),
    ));
    result.res_headers.push((
        "X-Bifrost-Error".to_string(),
        "PAC_EVALUATION_FAILED".to_string(),
    ));
}

fn build_final_url_for_pac(ctx: &RequestContext, result: &ProxyResolvedRules) -> Option<String> {
    if result.ignored.host {
        return Some(ctx.url.clone());
    }

    let Some(host_rule) = result.host.as_deref() else {
        return Some(ctx.url.clone());
    };
    let protocol = result.host_protocol.unwrap_or(Protocol::Host);
    if !matches!(
        protocol,
        Protocol::Host
            | Protocol::XHost
            | Protocol::Http
            | Protocol::Https
            | Protocol::Ws
            | Protocol::Wss
            | Protocol::Tunnel
    ) {
        return Some(ctx.url.clone());
    }

    let original = Url::parse(&ctx.url).ok()?;
    let (target_scheme, target_authority_and_path) =
        split_route_target(protocol, host_rule, original.scheme());
    let target_url = if target_authority_and_path.contains("://") {
        Url::parse(&target_authority_and_path).ok()?
    } else {
        Url::parse(&format!(
            "{}://{}",
            target_scheme, target_authority_and_path
        ))
        .ok()?
    };

    let target_path = target_url.path();
    let source_path = source_path_from_rules(result);
    let final_path = if (target_path.is_empty() || target_path == "/") && source_path.is_none() {
        original.path().to_string()
    } else {
        rewrite_path_with_prefix_for_pac(original.path(), source_path, target_path)
    };

    let mut final_url = target_url;
    final_url.set_path(&final_path);
    final_url.set_query(original.query());
    Some(final_url.to_string())
}

fn split_route_target(
    protocol: Protocol,
    host_rule: &str,
    original_scheme: &str,
) -> (&'static str, String) {
    let default_scheme = match protocol {
        Protocol::Http => "http",
        Protocol::Https => "https",
        Protocol::Ws => "ws",
        Protocol::Wss => "wss",
        _ if original_scheme == "https" || original_scheme == "wss" => "https",
        _ => "http",
    };
    (default_scheme, host_rule.to_string())
}

fn source_path_from_rules(result: &ProxyResolvedRules) -> Option<&str> {
    let host = result.host.as_ref()?;
    let protocol = result.host_protocol?;
    result.rules.iter().find_map(|rule| {
        if rule.protocol == protocol && rule.value == *host {
            extract_path_from_pattern_for_pac(&rule.pattern)
        } else {
            None
        }
    })
}

fn extract_path_from_pattern_for_pac(pattern: &str) -> Option<&str> {
    let pattern = pattern.trim_start_matches('!');
    let without_scheme = pattern
        .strip_prefix("http://")
        .or_else(|| pattern.strip_prefix("https://"))
        .or_else(|| pattern.strip_prefix("ws://"))
        .or_else(|| pattern.strip_prefix("wss://"))
        .unwrap_or(pattern);
    without_scheme.find('/').map(|idx| &without_scheme[idx..])
}

fn rewrite_path_with_prefix_for_pac(
    original_path: &str,
    source_path: Option<&str>,
    target_path: &str,
) -> String {
    if let Some(source_path) = source_path {
        let source = source_path.trim_end_matches('/');
        if let Some(remaining) = original_path.strip_prefix(source) {
            let target = target_path.trim_end_matches('/');
            if remaining.is_empty() {
                if target_path.ends_with('/') {
                    format!("{}/", target)
                } else {
                    target.to_string()
                }
            } else if remaining.starts_with('/') {
                format!("{}{}", target, remaining)
            } else {
                format!("{}/{}", target, remaining)
            }
        } else {
            original_path.to_string()
        }
    } else if target_path == "/" {
        original_path.to_string()
    } else {
        target_path.to_string()
    }
}

fn should_update_route_target(result: &ProxyResolvedRules, protocol: Protocol) -> bool {
    if result.ignored.host {
        return false;
    }

    if result.host.is_none() {
        return true;
    }

    matches!(
        (result.host_protocol, protocol),
        (Some(Protocol::Host), Protocol::XHost)
    )
}

fn parse_rule_bool(value: &str, default_when_empty: bool) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return default_when_empty;
    }

    matches!(
        value.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on" | "allow" | "allowed" | "enable" | "enabled"
    )
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
    fn test_pac_value_ref_proxy_maps_to_upstream_proxy_not_host() {
        let rules_text = r#"
```pac
function FindProxyForURL(url, host) {
  return "PROXY proxy.example:8080";
}
```
example.com pac://{pac}
"#;
        let parser = bifrost_core::RuleParser::new();
        let (rules, values) = parser.parse_rules_with_inline_values(rules_text).unwrap();
        let resolver = CoreRulesResolver::new(rules).with_values(values);

        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(resolved.proxy.as_deref(), Some("http://proxy.example:8080"));
        assert_eq!(resolved.host, None);
    }

    #[test]
    fn test_pac_inline_proxy_decision_maps_to_upstream_proxy() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com pac://(PROXY 127.0.0.1:3000)")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);

        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/test",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(resolved.proxy.as_deref(), Some("http://127.0.0.1:3000"));
        assert_eq!(resolved.host, None);
    }

    #[test]
    fn test_pac_direct_clears_explicit_proxy() {
        let rules_text = r#"
```pac
function FindProxyForURL(url, host) {
  return "DIRECT";
}
```
example.com proxy://http://proxy.example:8080
example.com pac://{pac}
"#;
        let parser = bifrost_core::RuleParser::new();
        let (rules, values) = parser.parse_rules_with_inline_values(rules_text).unwrap();
        let resolver = CoreRulesResolver::new(rules).with_values(values);

        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(resolved.proxy, None);
    }

    #[test]
    fn test_pac_eval_error_fails_closed() {
        let rules_text = r#"
```pac
function NotFindProxyForURL(url, host) {
  return "DIRECT";
}
```
example.com proxy://http://proxy.example:8080
example.com pac://{pac}
"#;
        let parser = bifrost_core::RuleParser::new();
        let (rules, values) = parser.parse_rules_with_inline_values(rules_text).unwrap();
        let resolver = CoreRulesResolver::new(rules).with_values(values);

        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(resolved.proxy, None);
        assert_eq!(resolved.status_code, Some(502));
        assert!(String::from_utf8_lossy(resolved.res_body.as_ref().unwrap())
            .contains("Bifrost PAC evaluation failed"));
        assert!(resolved.res_headers.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("X-Bifrost-Error") && v == "PAC_EVALUATION_FAILED"
        }));
    }

    #[test]
    fn test_pac_unsupported_proxy_scheme_fails_closed() {
        let rules_text = r#"
```pac
function FindProxyForURL(url, host) {
  return "SOCKS5 proxy.example:1080";
}
```
example.com pac://{pac}
"#;
        let parser = bifrost_core::RuleParser::new();
        let (rules, values) = parser.parse_rules_with_inline_values(rules_text).unwrap();
        let resolver = CoreRulesResolver::new(rules).with_values(values);

        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(resolved.proxy, None);
        assert_eq!(resolved.status_code, Some(502));
        assert!(String::from_utf8_lossy(resolved.res_body.as_ref().unwrap())
            .contains("unsupported PAC proxy scheme"));
    }

    #[test]
    fn test_pac_uses_rewritten_final_url_when_rule_is_split() {
        let rules_text = r#"
```pac
function FindProxyForURL(url, host) {
  if (url === "https://www.example.com/path") {
    return "PROXY proxy.example:8080";
  }
  return "DIRECT";
}
```
www.example.com/api www.example.com
www.example.com/path pac://{pac}
"#;
        let parser = bifrost_core::RuleParser::new();
        let (rules, values) = parser.parse_rules_with_inline_values(rules_text).unwrap();
        let resolver = CoreRulesResolver::new(rules).with_values(values);

        let resolved = resolve_rules_impl(
            &resolver,
            "https://www.example.com/api/path",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(resolved.proxy.as_deref(), Some("http://proxy.example:8080"));
    }

    #[test]
    fn test_pac_local_file_script_maps_to_proxy() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pac_path = temp_dir.path().join("proxy.pac");
        std::fs::write(
            &pac_path,
            r#"
function FindProxyForURL(url, host) {
  return "PROXY file-proxy.example:8080";
}
"#,
        )
        .unwrap();
        let rules_text = format!("example.com pac://{}", pac_path.display());
        let parser = bifrost_core::RuleParser::new();
        let rules = parser.parse_rules(&rules_text).unwrap();
        let resolver = CoreRulesResolver::new(rules);

        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(
            resolved.proxy.as_deref(),
            Some("http://file-proxy.example:8080")
        );
    }

    #[test]
    fn test_pac_remote_script_maps_to_proxy() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf);
            let body = r#"
function FindProxyForURL(url, host) {
  return "PROXY remote-proxy.example:8080";
}
"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-ns-proxy-autoconfig\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        let rules_text = format!("example.com pac://http://{}/proxy.pac", addr);
        let parser = bifrost_core::RuleParser::new();
        let rules = parser.parse_rules(&rules_text).unwrap();
        let resolver = CoreRulesResolver::new(rules);

        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        handle.join().unwrap();
        assert_eq!(
            resolved.proxy.as_deref(),
            Some("http://remote-proxy.example:8080")
        );
    }

    #[test]
    fn test_single_rewrite_and_pac_rule_does_not_recurse_on_final_url() {
        let rules_text = r#"
```pac
function FindProxyForURL(url, host) {
  return "PROXY proxy.example:8080";
}
```
www.example.com/api www.example.com pac://{pac}
"#;
        let parser = bifrost_core::RuleParser::new();
        let (rules, values) = parser.parse_rules_with_inline_values(rules_text).unwrap();
        let resolver = CoreRulesResolver::new(rules).with_values(values);

        let resolved = resolve_rules_impl(
            &resolver,
            "https://www.example.com/api/path",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(resolved.proxy, None);
        assert_eq!(resolved.host.as_deref(), Some("www.example.com"));
    }

    #[test]
    fn test_delete_rule_supports_reqheaders_and_resheaders_prefixes() {
        let parsed =
            parse_delete_value("reqHeaders.X-Debug|resHeaders.X-Echo-Server|urlParams.trace");

        assert_eq!(parsed.req_headers, vec!["X-Debug"]);
        assert_eq!(parsed.res_headers, vec!["X-Echo-Server"]);
        assert_eq!(parsed.url_params, vec!["trace"]);
    }

    #[test]
    fn test_later_reqheaders_rule_should_override_earlier_same_header() {
        let rules_text = r#"
`https://bifrost.local/` reqHeaders://{ppe}
`https://bifrost.local/api/v1/` reqHeaders://{ppe2}
```ppe
x-tt-env: ppe_next_agent_new
x-use-ppe: 1
```
```ppe2
x-tt-env: ppe_fix_disabled_skill_loading
x-use-ppe: 1
```
"#;
        let parser = bifrost_core::RuleParser::new();
        let (rules, values) = parser.parse_rules_with_inline_values(rules_text).unwrap();

        let resolver = CoreRulesResolver::new(rules).with_values(values);

        let resolved = resolve_rules_impl(
            &resolver,
            "https://bifrost.local/api/v1/oncall/system/env_info",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        let x_tt_env = resolved
            .req_headers
            .iter()
            .find(|(k, _)| k.to_lowercase() == "x-tt-env")
            .map(|(_, v)| v.as_str());

        assert_eq!(
            x_tt_env,
            Some("ppe_fix_disabled_skill_loading"),
            "Later reqHeaders rule with more specific path should override earlier rule's same-name header. \
             Got {:?}, expected 'ppe_fix_disabled_skill_loading'. \
             Current req_headers: {:?}",
            x_tt_env,
            resolved.req_headers
        );
    }

    #[test]
    fn test_reqheaders_markdown_value_skips_hash_comment_lines() {
        let rules_text = r#"
https://app.example.test/api/ https://app.example.test/api/
https://app.example.test/api/ reqHeaders://{env_block}
```env_block
X-Test-Env:test_env
#comment_marker
X-Test-Flag:1
## X-Ignored-Env: ignored comment
```

https://app.example.test http://localhost:8000/
wss://app.example.test/ ws://localhost:8000/
"#;
        let parser = bifrost_core::RuleParser::new();
        let (rules, values) = parser.parse_rules_with_inline_values(rules_text).unwrap();
        let resolver = CoreRulesResolver::new(rules).with_values(values);

        let resolved = resolve_rules_impl(
            &resolver,
            "https://app.example.test/api/v1/ping",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert!(resolved.ignored.host);
        assert_eq!(resolved.host, None);
        assert_eq!(
            resolved.req_headers,
            vec![
                ("X-Test-Env".to_string(), "test_env".to_string()),
                ("X-Test-Flag".to_string(), "1".to_string()),
            ]
        );
        assert!(
            !resolved
                .req_headers
                .iter()
                .any(|(name, _)| name.starts_with('#')),
            "hash-prefixed comment lines must not become request headers: {:?}",
            resolved.req_headers
        );
    }

    #[test]
    fn test_reqheaders_json_object_is_converted_to_proxy_headers() {
        let rules_text = r#"https://nextoncall.bytedance.net/api/nextagent/ reqHeaders://{"x-tt-env":"ppe_next_agent_new","x-use-ppe":"1","x-tt-env-fe":"dev"}"#;
        let parser = bifrost_core::RuleParser::new();
        let rules = parser.parse_rules(rules_text).unwrap();
        let resolver = CoreRulesResolver::new(rules);

        let resolved = resolve_rules_impl(
            &resolver,
            "https://nextoncall.bytedance.net/api/nextagent/v1/memory/items",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert!(resolved
            .req_headers
            .iter()
            .any(|(key, value)| key == "x-tt-env" && value == "ppe_next_agent_new"));
        assert!(resolved
            .req_headers
            .iter()
            .any(|(key, value)| key == "x-use-ppe" && value == "1"));
        assert!(resolved
            .req_headers
            .iter()
            .any(|(key, value)| key == "x-tt-env-fe" && value == "dev"));
    }

    #[test]
    fn test_merge_host_first_match_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com host://target1:8080\nexample.com host://target2:9090")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.host.as_deref(), Some("target1:8080"));
    }

    #[test]
    fn test_merge_host_passthrough_blocks_host() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com passthrough://\nexample.com host://target1:8080")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(resolved.ignored.host);
        assert_eq!(resolved.host, None);
    }

    #[test]
    fn test_merge_passthrough_does_not_override_higher_priority_https_route() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "qianchuan.jinritemai.com/app/account-center https://10.37.102.138:8081\n\
                 qianchuan.jinritemai.com/app https://qianchuan.jinritemai.com/app\n\
                 qianchuan.jinritemai.com https://10.37.102.138:8080",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "https://qianchuan.jinritemai.com/app/account-center",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert!(!resolved.ignored.host);
        assert_eq!(resolved.host.as_deref(), Some("10.37.102.138:8081"));
        assert_eq!(resolved.host_protocol, Some(Protocol::Https));
    }

    #[test]
    fn test_merge_xhost_first_match_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com xhost://target1:8080\nexample.com xhost://target2:9090")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.host.as_deref(), Some("target1:8080"));
    }

    #[test]
    fn test_merge_http_first_match_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com http://target1:8080\nexample.com http://target2:9090")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.host.as_deref(), Some("target1:8080"));
    }

    #[test]
    fn test_merge_https_first_match_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com https://target1:8080\nexample.com https://target2:9090")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.host.as_deref(), Some("target1:8080"));
    }

    #[test]
    fn test_merge_ws_first_match_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com ws://target1:8080\nexample.com ws://target2:9090")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "ws://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.host.as_deref(), Some("target1:8080"));
    }

    #[test]
    fn test_merge_wss_first_match_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com wss://target1:8080\nexample.com wss://target2:9090")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "wss://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.host.as_deref(), Some("target1:8080"));
    }

    #[test]
    fn test_merge_tunnel_assigns_host() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com tunnel://target1:8080")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "tunnel://example.com:443",
            "CONNECT",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.host.as_deref(), Some("target1:8080"));
        assert_eq!(resolved.host_protocol, Some(Protocol::Tunnel));
    }

    #[test]
    fn test_merge_keeps_first_route_target_across_protocols() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "qianchuan.jinritemai.com/ad qianchuan.jinritemai.com/ad\n\
                 qianchuan.jinritemai.com https://10.37.102.138:8080",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "https://qianchuan.jinritemai.com/ad/api/v1/account/user/info?gfversion=unknown",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(
            resolved.host.as_deref(),
            Some("qianchuan.jinritemai.com/ad")
        );
        assert_eq!(resolved.host_protocol, Some(Protocol::Host));
        assert_eq!(resolved.rules.len(), 2);
    }

    #[test]
    fn test_merge_keeps_domain_fallback_route_when_path_rule_misses() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "qianchuan.jinritemai.com/ad qianchuan.jinritemai.com/ad\n\
                 qianchuan.jinritemai.com https://10.37.102.138:8080",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "https://qianchuan.jinritemai.com/other/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(resolved.host.as_deref(), Some("10.37.102.138:8080"));
        assert_eq!(resolved.host_protocol, Some(Protocol::Https));
        assert_eq!(resolved.rules.len(), 1);
    }

    #[test]
    fn test_merge_keeps_xhost_priority_over_host() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com host://host-target\nexample.com xhost://xhost-target")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(resolved.host.as_deref(), Some("xhost-target"));
        assert_eq!(resolved.host_protocol, Some(Protocol::XHost));
        assert_eq!(resolved.rules.len(), 2);
    }

    #[test]
    fn test_merge_file_non_multi_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com file://(content_a)\nexample.com file://(content_b)")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.mock_file.as_deref(), Some("(content_a)"));
    }

    #[test]
    fn test_merge_tpl_non_multi_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com tpl://(tpl_a)\nexample.com tpl://(tpl_b)")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.mock_template.as_deref(), Some("(tpl_a)"));
    }

    #[test]
    fn test_merge_rawfile_non_multi_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com rawfile://(raw_a)\nexample.com rawfile://(raw_b)")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.mock_rawfile.as_deref(), Some("(raw_a)"));
    }

    #[test]
    fn test_merge_redirect_non_multi_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com redirect://(http://target-a.com)\nexample.com redirect://(http://target-b.com)",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.redirect.as_deref(), Some("http://target-a.com"));
    }

    #[test]
    fn test_merge_status_code_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com statusCode://200\nexample.com statusCode://404")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.status_code, Some(200));
    }

    #[test]
    fn test_merge_replace_status_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com replaceStatus://201\nexample.com replaceStatus://404")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.replace_status, Some(201));
    }

    #[test]
    fn test_merge_method_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com method://POST\nexample.com method://PUT")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.method.as_deref(), Some("POST"));
    }

    #[test]
    fn test_merge_ua_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com ua://Agent1\nexample.com ua://Agent2")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.ua.as_deref(), Some("Agent1"));
    }

    #[test]
    fn test_merge_referer_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com referer://ref1.example.com\nexample.com referer://ref2.example.com",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.referer.as_deref(), Some("ref1.example.com"));
    }

    #[test]
    fn test_merge_proxy_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com proxy://socks5://proxy1:1080\nexample.com proxy://socks5://proxy2:1081",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.proxy.as_deref(), Some("socks5://proxy1:1080"));
    }

    #[test]
    fn test_merge_auth_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com auth://user1:pass1\nexample.com auth://user2:pass2")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.auth.as_deref(), Some("user1:pass1"));
    }

    #[test]
    fn test_merge_req_delay_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com reqDelay://1000\nexample.com reqDelay://2000")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.req_delay, Some(1000));
    }

    #[test]
    fn test_merge_res_delay_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com resDelay://500\nexample.com resDelay://1000")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.res_delay, Some(500));
    }

    #[test]
    fn test_merge_req_speed_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com reqSpeed://100\nexample.com reqSpeed://200")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.req_speed, Some(100 * 1024));
    }

    #[test]
    fn test_merge_res_speed_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com resSpeed://50\nexample.com resSpeed://100")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.res_speed, Some(50 * 1024));
    }

    #[test]
    fn test_merge_req_type_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com reqType://application/json\nexample.com reqType://text/xml")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.req_type.as_deref(), Some("application/json"));
    }

    #[test]
    fn test_merge_res_type_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com resType://application/json\nexample.com resType://text/html")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.res_type.as_deref(), Some("application/json"));
    }

    #[test]
    fn test_merge_req_charset_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com reqCharset://utf-8\nexample.com reqCharset://gbk")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.req_charset.as_deref(), Some("utf-8"));
    }

    #[test]
    fn test_merge_res_charset_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com resCharset://utf-8\nexample.com resCharset://gbk")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.res_charset.as_deref(), Some("utf-8"));
    }

    #[test]
    fn test_merge_cache_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com cache://no-cache\nexample.com cache://max-age=3600")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.cache.as_deref(), Some("no-cache"));
    }

    #[test]
    fn test_merge_attachment_single_match() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com attachment://file_a.zip\nexample.com attachment://file_b.zip")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.attachment.as_deref(), Some("file_a.zip"));
    }

    #[test]
    fn test_merge_http3_flag() {
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
    fn test_merge_res_body_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com resBody://(body_first)\nexample.com resBody://(body_last)")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            resolved
                .res_body
                .as_ref()
                .map(|b| std::str::from_utf8(b).unwrap()),
            Some("body_last")
        );
    }

    #[test]
    fn test_merge_req_body_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com reqBody://(body_first)\nexample.com reqBody://(body_last)")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            resolved
                .req_body
                .as_ref()
                .map(|b| std::str::from_utf8(b).unwrap()),
            Some("body_last")
        );
    }

    #[test]
    fn test_merge_req_prepend_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com reqPrepend://(prepend_first)\nexample.com reqPrepend://(prepend_last)",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            resolved
                .req_prepend
                .as_ref()
                .map(|b| std::str::from_utf8(b).unwrap()),
            Some("prepend_last")
        );
    }

    #[test]
    fn test_merge_req_append_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com reqAppend://(append_first)\nexample.com reqAppend://(append_last)",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            resolved
                .req_append
                .as_ref()
                .map(|b| std::str::from_utf8(b).unwrap()),
            Some("append_last")
        );
    }

    #[test]
    fn test_merge_res_prepend_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com resPrepend://(prepend_first)\nexample.com resPrepend://(prepend_last)",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            resolved
                .res_prepend
                .as_ref()
                .map(|b| std::str::from_utf8(b).unwrap()),
            Some("prepend_last")
        );
    }

    #[test]
    fn test_merge_res_append_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com resAppend://(append_first)\nexample.com resAppend://(append_last)",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            resolved
                .res_append
                .as_ref()
                .map(|b| std::str::from_utf8(b).unwrap()),
            Some("append_last")
        );
    }

    #[test]
    fn test_merge_res_cors_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com resCors://http://origin-a.com\nexample.com resCors://http://origin-b.com",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            resolved.res_cors.origin.as_deref(),
            Some("http://origin-b.com")
        );
    }

    #[test]
    fn test_merge_req_cors_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com reqCors://http://origin-a.com\nexample.com reqCors://http://origin-b.com",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            resolved.req_cors.origin.as_deref(),
            Some("http://origin-b.com")
        );
    }

    #[test]
    fn test_merge_req_replace_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com reqReplace://old1=new1\nexample.com reqReplace://old2=new2")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.req_replace.len(), 2);
        assert!(resolved
            .req_replace
            .iter()
            .any(|(o, n)| o == "old1" && n == "new1"));
        assert!(resolved
            .req_replace
            .iter()
            .any(|(o, n)| o == "old2" && n == "new2"));
    }

    #[test]
    fn test_merge_res_replace_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com resReplace://old1=new1\nexample.com resReplace://old2=new2")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.res_replace.len(), 2);
    }

    #[test]
    fn test_merge_url_replace_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com urlReplace://old_path=new_path\nexample.com urlReplace://old_query=new_query",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.url_replace.len(), 2);
    }

    #[test]
    fn test_merge_req_script_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com reqScript://script1.js\nexample.com reqScript://script2.js")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.req_scripts.len(), 2);
        assert_eq!(resolved.req_scripts[0], "script1.js");
        assert_eq!(resolved.req_scripts[1], "script2.js");
    }

    #[test]
    fn test_merge_res_script_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com resScript://script1.js\nexample.com resScript://script2.js")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.res_scripts.len(), 2);
        assert_eq!(resolved.res_scripts[0], "script1.js");
        assert_eq!(resolved.res_scripts[1], "script2.js");
    }

    #[test]
    fn test_merge_decode_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com decode://gzip\nexample.com decode://br")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.decode_scripts.len(), 2);
        assert_eq!(resolved.decode_scripts[0], "gzip");
        assert_eq!(resolved.decode_scripts[1], "br");
    }

    #[test]
    fn test_bp_parser_script_accumulates_with_decode_bp() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com bp://team/parser decode://bp")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.bp_scripts, vec!["team/parser".to_string()]);
        assert_eq!(resolved.decode_scripts, vec!["bp".to_string()]);
    }

    #[test]
    fn test_bp_remote_script_reference_is_not_pre_fetched_by_rule_resolver() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com bp://http://127.0.0.1:18080/parser.js?sha256=abc decode://bp")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            resolved.bp_scripts,
            vec!["http://127.0.0.1:18080/parser.js?sha256=abc".to_string()]
        );
    }

    #[test]
    fn test_merge_dns_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser.parse_rules("example.com dns://8.8.8.8").unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.dns_servers.len(), 1);
        assert_eq!(resolved.dns_servers[0], "8.8.8.8");
    }

    #[test]
    fn test_merge_delete_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com delete://reqHeaders.X-Debug|resHeaders.X-Server\nexample.com delete://urlParams.trace",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(resolved.delete_req_headers.contains(&"X-Debug".to_string()));
        assert!(resolved
            .delete_res_headers
            .contains(&"X-Server".to_string()));
        assert!(resolved.delete_url_params.contains(&"trace".to_string()));
    }

    #[test]
    fn test_merge_header_replace_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com headerReplace://req.X-Token:old=new\nexample.com headerReplace://res.X-Server:apache=nginx",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.header_replace.len(), 2);
    }

    #[test]
    fn test_merge_req_cookies_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com reqCookies://session=abc\nexample.com reqCookies://token=xyz")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(resolved.req_cookies.len() >= 2);
        assert!(resolved.req_cookies.iter().any(|(k, _)| k == "session"));
        assert!(resolved.req_cookies.iter().any(|(k, _)| k == "token"));
    }

    #[test]
    fn test_merge_res_cookies_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com resCookies://session=abc\nexample.com resCookies://token=xyz")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(resolved.res_cookies.len() >= 2);
    }

    #[test]
    fn test_merge_url_params_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com urlParams://(key_a:val_a)\nexample.com urlParams://(key_b:val_b)",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(resolved.url_params.len() >= 2);
        assert!(resolved.url_params.iter().any(|(k, _)| k == "key_a"));
        assert!(resolved.url_params.iter().any(|(k, _)| k == "key_b"));
    }

    #[test]
    fn test_url_params_ampersand_value_splits_pairs() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com urlParams://key1=value1&key2=value2")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(
            resolved.url_params,
            vec![
                ("key1".to_string(), "value1".to_string()),
                ("key2".to_string(), "value2".to_string())
            ]
        );
    }

    #[test]
    fn test_url_params_mixed_delimiters_and_delete() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com urlParams://(key_a:val_a,key_b=val_b&remove_me=)")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );

        assert!(resolved
            .url_params
            .contains(&("key_a".to_string(), "val_a".to_string())));
        assert!(resolved
            .url_params
            .contains(&("key_b".to_string(), "val_b".to_string())));
        assert_eq!(resolved.delete_url_params, vec!["remove_me".to_string()]);
    }

    #[test]
    fn test_merge_trailers_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com trailers://X-Checksum=abc\nexample.com trailers://X-Audit=123",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.trailers.len(), 2);
    }

    #[test]
    fn test_merge_html_append_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com htmlAppend://(html_first)\nexample.com htmlAppend://(html_last)",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.html_append.as_deref(), Some("html_last"));
    }

    #[test]
    fn test_merge_html_prepend_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com htmlPrepend://(html_first)\nexample.com htmlPrepend://(html_last)",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.html_prepend.as_deref(), Some("html_last"));
    }

    #[test]
    fn test_merge_html_body_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com htmlBody://(html_first)\nexample.com htmlBody://(html_last)")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.html_body.as_deref(), Some("html_last"));
    }

    #[test]
    fn test_merge_js_append_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com jsAppend://(js_first)\nexample.com jsAppend://(js_last)")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.js_append.as_deref(), Some("js_last"));
    }

    #[test]
    fn test_merge_js_prepend_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com jsPrepend://(js_first)\nexample.com jsPrepend://(js_last)")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.js_prepend.as_deref(), Some("js_last"));
    }

    #[test]
    fn test_merge_js_body_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com jsBody://(js_first)\nexample.com jsBody://(js_last)")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.js_body.as_deref(), Some("js_last"));
    }

    #[test]
    fn test_merge_css_append_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com cssAppend://(css_first)\nexample.com cssAppend://(css_last)")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.css_append.as_deref(), Some("css_last"));
    }

    #[test]
    fn test_merge_css_prepend_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com cssPrepend://(css_first)\nexample.com cssPrepend://(css_last)",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.css_prepend.as_deref(), Some("css_last"));
    }

    #[test]
    fn test_merge_css_body_last_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com cssBody://(css_first)\nexample.com cssBody://(css_last)")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.css_body.as_deref(), Some("css_last"));
    }

    #[test]
    fn test_merge_forwarded_for_pushes_to_req_headers() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com forwardedFor://192.168.1.1")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.forwarded_for.as_deref(), Some("192.168.1.1"));
    }

    #[test]
    fn test_merge_response_for_pushes_to_res_headers() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com responseFor://test-response")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.response_for.as_deref(), Some("test-response"));
    }

    #[test]
    fn test_merge_passthrough_sets_ignored_host() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser.parse_rules("example.com passthrough://").unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(resolved.ignored.host);
    }

    #[test]
    fn test_merge_tls_intercept() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser.parse_rules("example.com tlsIntercept://").unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.tls_intercept, Some(true));
    }

    #[test]
    fn test_merge_tls_passthrough() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser.parse_rules("example.com tlsPassthrough://").unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.tls_intercept, Some(false));
    }

    #[test]
    fn test_merge_tls_options() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com tlsOptions://tls1.3")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.tls_options.as_deref(), Some("tls1.3"));
    }

    #[test]
    fn test_merge_upstream_unsafe_ssl() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com upstreamUnsafeSsl://true")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(resolved.upstream_unsafe_ssl);
    }

    #[test]
    fn test_merge_sni_callback() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com sniCallback://custom_sni_handler")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "https://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.sni_callback.as_deref(), Some("custom_sni_handler"));
    }

    #[test]
    fn test_devtools_evaluate_allowlist_parses_regex() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                r#"example.com devtools://mode=control,evaluate_allowlist=["^document\\.title$"]"#,
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        let devtools = resolved.devtools.expect("devtools rule");
        assert_eq!(devtools.mode, DevtoolsMode::Control);
        assert_eq!(
            devtools.evaluate_allowlist,
            vec![r#"^document\.title$"#.to_string()]
        );
        assert_eq!(
            parse_devtools_evaluate_allowlist(r#"["^document\\.title$"]"#),
            vec![r#"^document\.title$"#.to_string()]
        );
    }

    #[test]
    fn test_devtools_evaluate_allowlist_omitted_allows_any_expression() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com devtools://mode=control")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(!resolved
            .devtools
            .expect("devtools rule")
            .raw_value
            .is_empty());
        assert!(parse_devtools_evaluate_allowlist("").is_empty());
    }

    #[test]
    fn test_merge_forward_and_modify_coexist() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com host://target:8080\nexample.com reqHeaders://X-Custom=hello\nexample.com resHeaders://X-Response=world\nexample.com reqCookies://session=abc",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.host.as_deref(), Some("target:8080"));
        assert!(resolved
            .req_headers
            .iter()
            .any(|(k, v)| k.to_lowercase() == "x-custom" && v == "hello"));
        assert!(resolved
            .res_headers
            .iter()
            .any(|(k, v)| k.to_lowercase() == "x-response" && v == "world"));
        assert!(resolved.req_cookies.iter().any(|(k, _)| k == "session"));
    }

    #[test]
    fn test_merge_multiple_accumulate_protocols() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com reqReplace://foo=bar\nexample.com resReplace://baz=qux\nexample.com urlReplace://old=new\nexample.com reqScript://s1.js\nexample.com resScript://s2.js",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(resolved.req_replace.len(), 1);
        assert_eq!(resolved.res_replace.len(), 1);
        assert_eq!(resolved.url_replace.len(), 1);
        assert_eq!(resolved.req_scripts.len(), 1);
        assert_eq!(resolved.res_scripts.len(), 1);
    }

    #[test]
    fn test_merge_redirect_with_status_code() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules("example.com redirect://301:http://new-location.com")
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            resolved.redirect.as_deref(),
            Some("http://new-location.com")
        );
        assert_eq!(resolved.redirect_status, Some(301));
    }

    #[test]
    fn test_merge_reqheaders_different_keys_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com reqHeaders://X-Header-A=val-a\nexample.com reqHeaders://X-Header-B=val-b",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(resolved
            .req_headers
            .iter()
            .any(|(k, v)| k.to_lowercase() == "x-header-a" && v == "val-a"));
        assert!(resolved
            .req_headers
            .iter()
            .any(|(k, v)| k.to_lowercase() == "x-header-b" && v == "val-b"));
    }

    #[test]
    fn test_merge_resheaders_different_keys_accumulate() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com resHeaders://X-Header-A=val-a\nexample.com resHeaders://X-Header-B=val-b",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(resolved
            .res_headers
            .iter()
            .any(|(k, v)| k.to_lowercase() == "x-header-a" && v == "val-a"));
        assert!(resolved
            .res_headers
            .iter()
            .any(|(k, v)| k.to_lowercase() == "x-header-b" && v == "val-b"));
    }

    #[test]
    fn test_merge_reqheaders_same_key_first_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com reqHeaders://X-Same=first\nexample.com reqHeaders://X-Same=second",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        let val = resolved
            .req_headers
            .iter()
            .find(|(k, _)| k.to_lowercase() == "x-same")
            .map(|(_, v)| v.as_str());
        assert_eq!(val, Some("first"));
    }

    #[test]
    fn test_merge_resheaders_same_key_first_wins() {
        let parser = bifrost_core::RuleParser::new();
        let rules = parser
            .parse_rules(
                "example.com resHeaders://X-Same=first\nexample.com resHeaders://X-Same=second",
            )
            .unwrap();
        let resolver = CoreRulesResolver::new(rules);
        let resolved = resolve_rules_impl(
            &resolver,
            "http://example.com/api",
            "GET",
            &HashMap::new(),
            &HashMap::new(),
        );
        let val = resolved
            .res_headers
            .iter()
            .find(|(k, _)| k.to_lowercase() == "x-same")
            .map(|(_, v)| v.as_str());
        assert_eq!(val, Some("first"));
    }
}
