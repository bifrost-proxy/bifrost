use std::collections::HashMap;
use std::time::Instant;

use bifrost_core::{Protocol, ResolvedRules};
use bifrost_script::{
    MatchedRuleInfo, RequestData, ResponseData, ScriptContext, ScriptExecutionResult, ScriptType,
};
use bytes::Bytes;

use crate::state::SharedAdminState;

#[derive(Debug, Clone, Default)]
pub(crate) struct ReplayScriptRules {
    pub req_scripts: Vec<String>,
    pub res_scripts: Vec<String>,
    pub decode_scripts: Vec<String>,
    pub bp_scripts: Vec<String>,
}

pub(crate) fn collect_script_rules(resolved_rules: &ResolvedRules) -> ReplayScriptRules {
    let mut scripts = ReplayScriptRules::default();

    for rule in &resolved_rules.rules {
        let value = rule.resolved_value.trim();
        if value.is_empty() {
            continue;
        }

        match rule.rule.protocol {
            Protocol::ReqScript => scripts.req_scripts.push(value.to_string()),
            Protocol::ResScript => scripts.res_scripts.push(value.to_string()),
            Protocol::Decode => scripts.decode_scripts.push(value.to_string()),
            Protocol::Bp => scripts.bp_scripts.push(value.to_string()),
            _ => {}
        }
    }

    scripts
}

pub(crate) fn matched_rules_info(resolved_rules: &ResolvedRules) -> Vec<MatchedRuleInfo> {
    resolved_rules
        .rules
        .iter()
        .map(|rule| MatchedRuleInfo {
            pattern: rule.rule.pattern.clone(),
            protocol: rule.rule.protocol.to_str().to_string(),
            value: rule.resolved_value.clone(),
        })
        .collect()
}

pub(crate) fn parse_url_parts(url: &str) -> (String, String, String) {
    if let Ok(parsed) = url::Url::parse(url) {
        (
            parsed.host_str().unwrap_or("").to_string(),
            parsed.path().to_string(),
            parsed.scheme().to_string(),
        )
    } else {
        ("".to_string(), url.to_string(), "http".to_string())
    }
}

pub(crate) fn header_pairs_to_hashmap(headers: &[(String, String)]) -> HashMap<String, String> {
    let mut map = HashMap::with_capacity(headers.len());
    for (key, value) in headers {
        map.insert(key.clone(), value.clone());
    }
    map
}

pub(crate) fn hashmap_to_header_pairs(headers: HashMap<String, String>) -> Vec<(String, String)> {
    headers.into_iter().collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_replay_request_scripts(
    admin_state: &SharedAdminState,
    script_names: &[String],
    resolved_rules: &ResolvedRules,
    replay_id: &str,
    url: &str,
    method: &mut String,
    headers: &mut Vec<(String, String)>,
    body: &mut Option<Bytes>,
    values: &HashMap<String, String>,
) -> Vec<ScriptExecutionResult> {
    if script_names.is_empty() {
        return Vec::new();
    }

    let Some(manager) = admin_state.script_manager.as_ref() else {
        return Vec::new();
    };

    let (host, path, protocol) = parse_url_parts(url);
    let mut request_data = RequestData {
        url: url.to_string(),
        method: method.clone(),
        host,
        path,
        protocol,
        client_ip: "127.0.0.1".to_string(),
        client_app: Some("Bifrost Replay".to_string()),
        headers: header_pairs_to_hashmap(headers),
        body: body
            .as_ref()
            .map(|bytes| String::from_utf8_lossy(bytes).to_string()),
    };

    let ctx = ScriptContext {
        request_id: replay_id.to_string(),
        script_name: script_names.first().cloned().unwrap_or_default(),
        script_type: ScriptType::Request,
        values: values.clone(),
        matched_rules: matched_rules_info(resolved_rules),
    };

    let cfg = if let Some(config_manager) = admin_state.config_manager.as_ref() {
        Some(config_manager.config().await)
    } else {
        None
    };

    let manager = manager.read().await;
    let results = if let Some(ref cfg) = cfg {
        manager
            .execute_request_scripts_with_config(script_names, &mut request_data, &ctx, cfg)
            .await
    } else {
        manager
            .execute_request_scripts(script_names, &mut request_data, &ctx)
            .await
    };

    if results.iter().any(|result| result.success) {
        *method = request_data.method;
        *headers = hashmap_to_header_pairs(request_data.headers);
        if let Some(updated_body) = request_data.body {
            *body = Some(Bytes::from(updated_body));
        }
    }

    results
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_replay_response_scripts(
    admin_state: &SharedAdminState,
    script_names: &[String],
    resolved_rules: &ResolvedRules,
    replay_id: &str,
    request_url: &str,
    request_method: &str,
    request_headers: &[(String, String)],
    status: &mut u16,
    headers: &mut Vec<(String, String)>,
    body: &mut Option<String>,
    values: &HashMap<String, String>,
) -> Vec<ScriptExecutionResult> {
    if script_names.is_empty() {
        return Vec::new();
    }

    let Some(manager) = admin_state.script_manager.as_ref() else {
        return Vec::new();
    };

    let (host, path, protocol) = parse_url_parts(request_url);
    let request_data = RequestData {
        url: request_url.to_string(),
        method: request_method.to_string(),
        host,
        path,
        protocol,
        client_ip: "127.0.0.1".to_string(),
        client_app: Some("Bifrost Replay".to_string()),
        headers: header_pairs_to_hashmap(request_headers),
        body: None,
    };
    let mut response_data = ResponseData {
        status: *status,
        status_text: hyper::StatusCode::from_u16(*status)
            .ok()
            .and_then(|status| status.canonical_reason().map(|reason| reason.to_string()))
            .unwrap_or_default(),
        headers: header_pairs_to_hashmap(headers),
        body: body.clone(),
        request: request_data,
    };

    let ctx = ScriptContext {
        request_id: replay_id.to_string(),
        script_name: script_names.first().cloned().unwrap_or_default(),
        script_type: ScriptType::Response,
        values: values.clone(),
        matched_rules: matched_rules_info(resolved_rules),
    };

    let cfg = if let Some(config_manager) = admin_state.config_manager.as_ref() {
        Some(config_manager.config().await)
    } else {
        None
    };

    let manager = manager.read().await;
    let results = if let Some(ref cfg) = cfg {
        manager
            .execute_response_scripts_with_config(script_names, &mut response_data, &ctx, cfg)
            .await
    } else {
        manager
            .execute_response_scripts(script_names, &mut response_data, &ctx)
            .await
    };

    if results.iter().any(|result| result.success) {
        *status = response_data.status;
        *headers = hashmap_to_header_pairs(response_data.headers);
        *body = response_data.body;
    }

    results
}

fn is_builtin_decoder(name: &str) -> bool {
    matches!(name.trim(), "utf8" | "default")
}

fn builtin_decode_utf8(input: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(input).to_string().into_bytes()
}

fn truncate_string(value: String, max_len: usize) -> String {
    if value.len() <= max_len {
        return value;
    }
    bifrost_core::text::truncate_bytes_with_suffix(&value, max_len, "…(truncated)")
}

fn limit_script_logs(
    mut logs: Vec<bifrost_script::ScriptLogEntry>,
    max_len: usize,
) -> Vec<bifrost_script::ScriptLogEntry> {
    if logs.len() <= max_len {
        return logs;
    }
    let start = logs.len().saturating_sub(max_len);
    logs.drain(0..start);
    logs
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_replay_decode_scripts(
    admin_state: &SharedAdminState,
    script_names: &[String],
    bp_scripts: &[String],
    phase: &str,
    replay_id: &str,
    resolved_rules: &ResolvedRules,
    request_data: &RequestData,
    response_data: &ResponseData,
    values: &HashMap<String, String>,
    body_bytes: Bytes,
) -> (Bytes, Vec<ScriptExecutionResult>) {
    if script_names.is_empty() || body_bytes.is_empty() {
        return (body_bytes, Vec::new());
    }

    let Some(manager) = admin_state.script_manager.as_ref() else {
        return (body_bytes, Vec::new());
    };

    let cfg = if let Some(config_manager) = admin_state.config_manager.as_ref() {
        Some(config_manager.config().await)
    } else {
        None
    };
    let max_decode_input_bytes = cfg
        .as_ref()
        .map(|config| config.sandbox.limits.max_decode_input_bytes)
        .unwrap_or(2 * 1024 * 1024);
    if body_bytes.len() > max_decode_input_bytes {
        let body_len = body_bytes.len();
        return (
            body_bytes,
            vec![ScriptExecutionResult {
                script_name: "__bifrost_skip__".to_string(),
                script_type: ScriptType::Decode,
                success: false,
                error: Some(format!(
                    "decode 输入过大，已跳过（{} bytes > {} limit）",
                    body_len, max_decode_input_bytes
                )),
                duration_ms: 0,
                logs: vec![],
                request_modifications: None,
                response_modifications: None,
                decode_output: None,
            }],
        );
    }

    let matched_rules = matched_rules_info(resolved_rules);
    let mut current = body_bytes.to_vec();
    let mut results = Vec::new();
    let manager = manager.read().await;

    for script_name in script_names {
        let script_name = script_name.trim();
        if script_name.is_empty() {
            continue;
        }

        if script_name == "bp" {
            if bp_scripts.is_empty() {
                results.push(ScriptExecutionResult {
                    script_name: "bp".to_string(),
                    script_type: ScriptType::Parser,
                    success: false,
                    error: Some(
                        "decode://bp requires at least one bp:// parser script".to_string(),
                    ),
                    duration_ms: 0,
                    logs: vec![],
                    request_modifications: None,
                    response_modifications: None,
                    decode_output: None,
                });
                break;
            }

            for parser_ref in bp_scripts {
                let parser_ref = parser_ref.trim();
                if parser_ref.is_empty() {
                    continue;
                }

                let ctx = ScriptContext {
                    request_id: replay_id.to_string(),
                    script_name: parser_ref.to_string(),
                    script_type: ScriptType::Parser,
                    values: values.clone(),
                    matched_rules: matched_rules.clone(),
                };
                let (req_bytes, res_bytes) = if phase.eq_ignore_ascii_case("request") {
                    (current.as_slice(), &[][..])
                } else {
                    (&[][..], current.as_slice())
                };

                let start = Instant::now();
                let result = if let Some(ref cfg) = cfg {
                    manager
                        .execute_parser_script_with_config(
                            parser_ref,
                            phase,
                            request_data,
                            req_bytes,
                            response_data,
                            res_bytes,
                            &ctx,
                            cfg,
                        )
                        .await
                } else {
                    manager
                        .engine()
                        .execute_parser_script(
                            parser_ref,
                            phase,
                            request_data,
                            req_bytes,
                            response_data,
                            res_bytes,
                            &ctx,
                        )
                        .await
                };

                match result {
                    Ok((output, logs)) => {
                        let success = output.code == "0";
                        let data_preview = truncate_string(output.data.clone(), 4096);
                        let msg_preview = truncate_string(output.msg.clone(), 4096);
                        results.push(ScriptExecutionResult {
                            script_name: parser_ref.to_string(),
                            script_type: ScriptType::Parser,
                            success,
                            error: if success {
                                None
                            } else {
                                Some(format!("bp parser 输出 code != 0: {}", msg_preview))
                            },
                            duration_ms: start.elapsed().as_millis() as u64,
                            logs: limit_script_logs(logs, 100),
                            request_modifications: None,
                            response_modifications: None,
                            decode_output: Some(bifrost_script::DecodeOutput {
                                code: output.code,
                                data: data_preview,
                                msg: msg_preview,
                            }),
                        });
                        if success {
                            current = output.data.into_bytes();
                        } else {
                            break;
                        }
                    }
                    Err(error) => {
                        results.push(ScriptExecutionResult {
                            script_name: parser_ref.to_string(),
                            script_type: ScriptType::Parser,
                            success: false,
                            error: Some(format!("bp parser 执行失败: {}", error)),
                            duration_ms: start.elapsed().as_millis() as u64,
                            logs: vec![],
                            request_modifications: None,
                            response_modifications: None,
                            decode_output: None,
                        });
                        break;
                    }
                }
            }
            continue;
        }

        if is_builtin_decoder(script_name) {
            let start = Instant::now();
            current = builtin_decode_utf8(&current);
            let data_preview = truncate_string(String::from_utf8_lossy(&current).to_string(), 4096);
            results.push(ScriptExecutionResult {
                script_name: script_name.to_string(),
                script_type: ScriptType::Decode,
                success: true,
                error: None,
                duration_ms: start.elapsed().as_millis() as u64,
                logs: vec![],
                request_modifications: None,
                response_modifications: None,
                decode_output: Some(bifrost_script::DecodeOutput {
                    code: "0".to_string(),
                    data: data_preview,
                    msg: "".to_string(),
                }),
            });
            continue;
        }

        let ctx = ScriptContext {
            request_id: replay_id.to_string(),
            script_name: script_name.to_string(),
            script_type: ScriptType::Decode,
            values: values.clone(),
            matched_rules: matched_rules.clone(),
        };
        let (req_bytes, res_bytes) = if phase.eq_ignore_ascii_case("request") {
            (current.as_slice(), &[][..])
        } else {
            (&[][..], current.as_slice())
        };

        let start = Instant::now();
        let result = if let Some(ref cfg) = cfg {
            manager
                .engine()
                .execute_decode_script_with_config(
                    script_name,
                    phase,
                    request_data,
                    req_bytes,
                    response_data,
                    res_bytes,
                    &ctx,
                    cfg,
                )
                .await
        } else {
            manager
                .engine()
                .execute_decode_script(
                    script_name,
                    phase,
                    request_data,
                    req_bytes,
                    response_data,
                    res_bytes,
                    &ctx,
                )
                .await
        };

        match result {
            Ok((output, logs)) => {
                let success = output.code == "0";
                let data_preview = truncate_string(output.data.clone(), 4096);
                let msg_preview = truncate_string(output.msg.clone(), 4096);
                results.push(ScriptExecutionResult {
                    script_name: script_name.to_string(),
                    script_type: ScriptType::Decode,
                    success,
                    error: if success {
                        None
                    } else {
                        Some(format!("decode 输出 code != 0: {}", msg_preview))
                    },
                    duration_ms: start.elapsed().as_millis() as u64,
                    logs: limit_script_logs(logs, 100),
                    request_modifications: None,
                    response_modifications: None,
                    decode_output: Some(bifrost_script::DecodeOutput {
                        code: output.code,
                        data: data_preview,
                        msg: msg_preview,
                    }),
                });
                if success {
                    current = output.data.into_bytes();
                } else {
                    break;
                }
            }
            Err(error) => {
                results.push(ScriptExecutionResult {
                    script_name: script_name.to_string(),
                    script_type: ScriptType::Decode,
                    success: false,
                    error: Some(format!("decode 脚本执行失败: {}", error)),
                    duration_ms: start.elapsed().as_millis() as u64,
                    logs: vec![],
                    request_modifications: None,
                    response_modifications: None,
                    decode_output: None,
                });
                break;
            }
        }
    }

    (Bytes::from(current), results)
}

pub(crate) fn request_data_from_replay(
    url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Option<&Bytes>,
) -> RequestData {
    let (host, path, protocol) = parse_url_parts(url);
    RequestData {
        url: url.to_string(),
        method: method.to_string(),
        host,
        path,
        protocol,
        client_ip: "127.0.0.1".to_string(),
        client_app: Some("Bifrost Replay".to_string()),
        headers: header_pairs_to_hashmap(headers),
        body: body.map(|bytes| String::from_utf8_lossy(bytes).to_string()),
    }
}

pub(crate) fn response_data_from_replay(
    status: u16,
    headers: &[(String, String)],
    body: Option<&str>,
    request: RequestData,
) -> ResponseData {
    ResponseData {
        status,
        status_text: hyper::StatusCode::from_u16(status)
            .ok()
            .and_then(|status| status.canonical_reason().map(|reason| reason.to_string()))
            .unwrap_or_default(),
        headers: header_pairs_to_hashmap(headers),
        body: body.map(|body| body.to_string()),
        request,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::{RequestContext, RuleParser, RulesResolver};

    fn resolve_for(url: &str, rules_text: &str) -> ResolvedRules {
        let rules = RuleParser::new().parse_rules(rules_text).unwrap();
        let resolver = RulesResolver::new(rules);
        resolver.resolve(&RequestContext::from_url(url))
    }

    #[test]
    fn collect_script_rules_groups_replay_script_protocols() {
        let resolved = resolve_for(
            "http://127.0.0.1/replay",
            "\
127.0.0.1 reqScript://req_one
127.0.0.1 resScript://res_one
127.0.0.1 decode://bp
127.0.0.1 bp://parser_one
127.0.0.1 reqHeaders://X-Ignored=yes
",
        );

        let scripts = collect_script_rules(&resolved);

        assert_eq!(scripts.req_scripts, vec!["req_one"]);
        assert_eq!(scripts.res_scripts, vec!["res_one"]);
        assert_eq!(scripts.decode_scripts, vec!["bp"]);
        assert_eq!(scripts.bp_scripts, vec!["parser_one"]);
    }

    #[test]
    fn parse_url_parts_handles_absolute_and_fallback_urls() {
        assert_eq!(
            parse_url_parts("https://example.com:8443/replay/path?q=1"),
            (
                "example.com".to_string(),
                "/replay/path".to_string(),
                "https".to_string()
            )
        );
        assert_eq!(
            parse_url_parts("/relative/path"),
            (
                "".to_string(),
                "/relative/path".to_string(),
                "http".to_string()
            )
        );
    }

    #[test]
    fn header_helpers_round_trip_pairs_and_request_body() {
        let headers = vec![
            ("X-One".to_string(), "1".to_string()),
            ("X-Two".to_string(), "2".to_string()),
        ];
        let map = header_pairs_to_hashmap(&headers);
        assert_eq!(map.get("X-One"), Some(&"1".to_string()));
        assert_eq!(map.get("X-Two"), Some(&"2".to_string()));

        let request = request_data_from_replay(
            "http://example.com/replay",
            "POST",
            &hashmap_to_header_pairs(map),
            Some(&Bytes::from_static(b"hello")),
        );

        assert_eq!(request.host, "example.com");
        assert_eq!(request.path, "/replay");
        assert_eq!(request.protocol, "http");
        assert_eq!(request.body, Some("hello".to_string()));
    }
}
