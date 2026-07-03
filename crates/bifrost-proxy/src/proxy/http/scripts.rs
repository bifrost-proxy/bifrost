use std::collections::HashMap;
use std::sync::Arc;

use bifrost_admin::AdminState;
use bifrost_script::{MatchedRuleInfo, RequestData, ResponseData, ScriptContext, ScriptType};
use bytes::Bytes;
use hyper::header::{HeaderName, HeaderValue};

use crate::server::ResolvedRules;
use crate::transform::{compress_body, try_decompress_body_with_limit, ContentInjectionResult};
use crate::utils::logging::RequestContext;

pub(in crate::proxy::http) fn build_matched_rules_info(
    resolved_rules: &ResolvedRules,
) -> Vec<MatchedRuleInfo> {
    resolved_rules
        .rules
        .iter()
        .map(|r| MatchedRuleInfo {
            pattern: r.pattern.clone(),
            protocol: r.protocol.to_string(),
            value: r.value.clone(),
        })
        .collect()
}

pub(in crate::proxy::http) fn parse_url_parts(url: &str) -> (String, String, String) {
    if let Ok(parsed) = url::Url::parse(url) {
        let host = parsed.host_str().unwrap_or("").to_string();
        let path = parsed.path().to_string();
        let protocol = parsed.scheme().to_string();
        (host, path, protocol)
    } else {
        ("".to_string(), url.to_string(), "http".to_string())
    }
}

pub(in crate::proxy::http) fn headers_to_hashmap(
    headers: &[(String, String)],
) -> HashMap<String, String> {
    header_pairs_to_hashmap(headers)
}

pub(in crate::proxy::http) fn header_pairs_to_hashmap(
    headers: &[(String, String)],
) -> HashMap<String, String> {
    let mut map = HashMap::with_capacity(headers.len());
    for (key, value) in headers {
        map.insert(key.clone(), value.clone());
    }
    map
}

pub(in crate::proxy::http) fn header_map_to_hashmap(
    headers: &hyper::HeaderMap,
) -> HashMap<String, String> {
    let mut map = HashMap::with_capacity(headers.len());
    for (key, value) in headers {
        map.insert(key.to_string(), value.to_str().unwrap_or("").to_string());
    }
    map
}

fn lower_header_map(headers: &HashMap<String, String>) -> HashMap<String, (String, String)> {
    headers
        .iter()
        .map(|(key, value)| (key.to_ascii_lowercase(), (key.clone(), value.clone())))
        .collect()
}

pub(in crate::proxy::http) fn apply_script_headers_to_header_map(
    original_headers: &hyper::HeaderMap,
    original_script_headers: &HashMap<String, String>,
    updated_script_headers: &HashMap<String, String>,
) -> hyper::HeaderMap {
    let original_script_headers = lower_header_map(original_script_headers);
    let updated_script_headers = lower_header_map(updated_script_headers);
    let mut result = hyper::HeaderMap::new();
    let mut rewritten = std::collections::HashSet::new();

    for (name, value) in original_headers {
        let lower_name = name.as_str().to_ascii_lowercase();
        match updated_script_headers.get(&lower_name) {
            Some((_, updated_value))
                if original_script_headers
                    .get(&lower_name)
                    .map(|(_, original_value)| original_value == updated_value)
                    .unwrap_or(false) =>
            {
                result.append(name.clone(), value.clone());
            }
            Some((updated_name, updated_value)) if rewritten.insert(lower_name) => {
                if let (Ok(name), Ok(value)) = (
                    HeaderName::from_bytes(updated_name.as_bytes()),
                    HeaderValue::from_str(updated_value),
                ) {
                    result.insert(name, value);
                }
            }
            Some(_) => {}
            None => {}
        }
    }

    for (lower_name, (updated_name, updated_value)) in &updated_script_headers {
        if original_script_headers.contains_key(lower_name) || rewritten.contains(lower_name) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(updated_name.as_bytes()),
            HeaderValue::from_str(updated_value),
        ) {
            result.insert(name, value);
        }
    }

    result
}

pub(in crate::proxy::http) fn body_to_script_string(
    body: &Bytes,
    content_encoding: Option<&str>,
    max_decompress_output_bytes: usize,
) -> Option<String> {
    if body.is_empty() {
        return None;
    }

    let content_encoding =
        content_encoding.filter(|encoding| !encoding.eq_ignore_ascii_case("identity"));
    let decoded = if let Some(content_encoding) = content_encoding {
        try_decompress_body_with_limit(body.as_ref(), content_encoding, max_decompress_output_bytes)
            .ok()?
    } else {
        body.to_vec()
    };

    String::from_utf8(decoded).ok()
}

pub(in crate::proxy::http) fn script_string_to_body(
    body: &str,
    content_encoding: Option<&str>,
) -> ContentInjectionResult {
    let content_encoding =
        content_encoding.filter(|encoding| !encoding.eq_ignore_ascii_case("identity"));
    if let Some(content_encoding) = content_encoding {
        match compress_body(body.as_bytes(), content_encoding) {
            Ok(compressed) => ContentInjectionResult {
                body: Bytes::from(compressed),
                content_encoding: Some(content_encoding.to_string()),
            },
            Err(_) => ContentInjectionResult {
                body: Bytes::from(body.to_string()),
                content_encoding: None,
            },
        }
    } else {
        ContentInjectionResult {
            body: Bytes::from(body.to_string()),
            content_encoding: None,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::proxy::http) async fn execute_request_scripts(
    admin_state: &Option<Arc<AdminState>>,
    script_names: &[String],
    ctx: &RequestContext,
    resolved_rules: &ResolvedRules,
    url: &str,
    method: &mut String,
    headers: &mut HashMap<String, String>,
    body: &mut Option<String>,
    values: &HashMap<String, String>,
) -> Vec<bifrost_script::ScriptExecutionResult> {
    if script_names.is_empty() {
        return vec![];
    }

    let state = match admin_state {
        Some(s) => s,
        None => return vec![],
    };

    let manager = match &state.script_manager {
        Some(m) => m,
        None => return vec![],
    };

    let cfg = if let Some(cm) = state.config_manager.as_ref() {
        Some(cm.config().await)
    } else {
        None
    };
    let matched_rules = build_matched_rules_info(resolved_rules);
    let (host, path, protocol) = parse_url_parts(url);

    let mut request_data = RequestData {
        url: url.to_string(),
        method: method.clone(),
        host,
        path,
        protocol,
        client_ip: ctx.client_ip.clone(),
        client_app: ctx.client_app.clone(),
        headers: headers.clone(),
        body: body.clone(),
    };

    let script_ctx = ScriptContext {
        request_id: ctx.id_str().to_string(),
        script_name: script_names.first().cloned().unwrap_or_default(),
        script_type: ScriptType::Request,
        values: values.clone(),
        matched_rules,
    };

    let mgr = manager.read().await;
    let results = if let Some(ref cfg) = cfg {
        mgr.execute_request_scripts_with_config(script_names, &mut request_data, &script_ctx, cfg)
            .await
    } else {
        mgr.execute_request_scripts(script_names, &mut request_data, &script_ctx)
            .await
    };

    if results.iter().any(|r| r.success) {
        *method = request_data.method;
        *headers = request_data.headers;
        *body = request_data.body;
    }

    results
}

#[allow(clippy::too_many_arguments)]
pub(in crate::proxy::http) async fn execute_response_scripts(
    admin_state: &Option<Arc<AdminState>>,
    script_names: &[String],
    ctx: &RequestContext,
    resolved_rules: &ResolvedRules,
    request_url: &str,
    request_method: &str,
    request_headers: &HashMap<String, String>,
    request_body: Option<String>,
    status: &mut u16,
    status_text: &mut String,
    headers: &mut HashMap<String, String>,
    body: &mut Option<String>,
    values: &HashMap<String, String>,
) -> Vec<bifrost_script::ScriptExecutionResult> {
    if script_names.is_empty() {
        return vec![];
    }

    let state = match admin_state {
        Some(s) => s,
        None => return vec![],
    };

    let manager = match &state.script_manager {
        Some(m) => m,
        None => return vec![],
    };

    let cfg = if let Some(cm) = state.config_manager.as_ref() {
        Some(cm.config().await)
    } else {
        None
    };
    let matched_rules = build_matched_rules_info(resolved_rules);
    let (host, path, protocol) = parse_url_parts(request_url);

    let mut response_data = ResponseData {
        status: *status,
        status_text: status_text.clone(),
        headers: headers.clone(),
        body: body.clone(),
        request: RequestData {
            url: request_url.to_string(),
            method: request_method.to_string(),
            host,
            path,
            protocol,
            client_ip: ctx.client_ip.clone(),
            client_app: ctx.client_app.clone(),
            headers: request_headers.clone(),
            body: request_body,
        },
    };

    let script_ctx = ScriptContext {
        request_id: ctx.id_str().to_string(),
        script_name: script_names.first().cloned().unwrap_or_default(),
        script_type: ScriptType::Response,
        values: values.clone(),
        matched_rules,
    };

    let mgr = manager.read().await;
    let results = if let Some(ref cfg) = cfg {
        mgr.execute_response_scripts_with_config(script_names, &mut response_data, &script_ctx, cfg)
            .await
    } else {
        mgr.execute_response_scripts(script_names, &mut response_data, &script_ctx)
            .await
    };

    if results.iter().any(|r| r.success) {
        *status = response_data.status;
        *status_text = response_data.status_text;
        *headers = response_data.headers;
        *body = response_data.body;
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_script_headers_preserves_unchanged_multi_value_headers() {
        let mut original = hyper::HeaderMap::new();
        original.append("set-cookie", "a=1; Path=/".parse().unwrap());
        original.append("set-cookie", "b=2; Path=/".parse().unwrap());
        original.insert("content-type", "text/plain".parse().unwrap());

        let original_script_headers = header_map_to_hashmap(&original);
        let mut updated_script_headers = original_script_headers.clone();
        updated_script_headers.insert("x-script".to_string(), "ran".to_string());

        let result = apply_script_headers_to_header_map(
            &original,
            &original_script_headers,
            &updated_script_headers,
        );

        let cookies: Vec<_> = result
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap().to_string())
            .collect();
        assert_eq!(cookies, vec!["a=1; Path=/", "b=2; Path=/"]);
        assert_eq!(result.get("x-script").unwrap(), "ran");
    }

    #[test]
    fn apply_script_headers_rewrites_touched_multi_value_header_once() {
        let mut original = hyper::HeaderMap::new();
        original.append("set-cookie", "a=1; Path=/".parse().unwrap());
        original.append("set-cookie", "b=2; Path=/".parse().unwrap());

        let original_script_headers = header_map_to_hashmap(&original);
        let mut updated_script_headers = original_script_headers.clone();
        updated_script_headers.insert("set-cookie".to_string(), "c=3; Path=/".to_string());

        let result = apply_script_headers_to_header_map(
            &original,
            &original_script_headers,
            &updated_script_headers,
        );

        let cookies: Vec<_> = result
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap().to_string())
            .collect();
        assert_eq!(cookies, vec!["c=3; Path=/"]);
    }
}
