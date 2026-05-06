use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use bifrost_admin::AdminState;
use bifrost_core::text::truncate_bytes_with_suffix;
use bifrost_script::{MatchedRuleInfo, RequestData, ResponseData, ScriptContext, ScriptType};
use bytes::Bytes;
use tracing::warn;

use crate::server::ResolvedRules;
use crate::utils::logging::RequestContext;

pub(super) fn build_matched_rules_info(resolved_rules: &ResolvedRules) -> Vec<MatchedRuleInfo> {
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

fn is_builtin_decoder(name: &str) -> bool {
    matches!(name.trim(), "utf8" | "default")
}

fn builtin_decode_utf8(input: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(input).to_string().into_bytes()
}

#[derive(Debug, Clone)]
pub(super) struct DecodeForStorageResult {
    pub(super) output: Bytes,
    pub(super) results: Vec<bifrost_script::ScriptExecutionResult>,
}

fn truncate_string(s: String, max_len: usize) -> String {
    if s.len() <= max_len {
        return s;
    }
    truncate_bytes_with_suffix(&s, max_len, "…(truncated)")
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
pub(super) async fn apply_decode_scripts_for_storage(
    admin_state: &Option<Arc<AdminState>>,
    script_names: &[String],
    phase: &str,
    ctx: &RequestContext,
    resolved_rules: &ResolvedRules,
    request_data: &RequestData,
    response_data: &ResponseData,
    values: &HashMap<String, String>,
    body_bytes: Bytes,
) -> DecodeForStorageResult {
    if script_names.is_empty() || body_bytes.is_empty() {
        return DecodeForStorageResult {
            output: body_bytes,
            results: vec![],
        };
    }

    let state = match admin_state {
        Some(s) => s,
        None => {
            return DecodeForStorageResult {
                output: body_bytes,
                results: vec![],
            }
        }
    };

    let manager = match &state.script_manager {
        Some(m) => m,
        None => {
            return DecodeForStorageResult {
                output: body_bytes,
                results: vec![],
            }
        }
    };

    let cfg = if let Some(cm) = state.config_manager.as_ref() {
        Some(cm.config().await)
    } else {
        None
    };

    // 性能保护：decode 的输入过大时直接跳过（落库仍然保存原始内容）
    let max_decode_input_bytes = cfg
        .as_ref()
        .map(|c| c.sandbox.limits.max_decode_input_bytes)
        .unwrap_or(2 * 1024 * 1024);
    if body_bytes.len() > max_decode_input_bytes {
        let body_len = body_bytes.len();
        warn!(
            "[{}] [DECODE] skip decode ({} bytes > {} limit)",
            ctx.id_str(),
            body_len,
            max_decode_input_bytes
        );
        return DecodeForStorageResult {
            output: body_bytes,
            results: vec![bifrost_script::ScriptExecutionResult {
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
        };
    }

    let matched_rules = build_matched_rules_info(resolved_rules);
    let mut current = body_bytes.to_vec();
    let mut results: Vec<bifrost_script::ScriptExecutionResult> = Vec::new();

    let mgr = manager.read().await;
    for script_name in script_names {
        let script_name = script_name.trim();
        if script_name.is_empty() {
            continue;
        }

        // 内置解码器（与 WebSocket decode 行为保持一致）：decode://utf8 / decode://default
        if is_builtin_decoder(script_name) {
            let start = Instant::now();
            current = builtin_decode_utf8(&current);
            // 避免把大体积内容塞进 record，decode_output.data 做截断。
            let data_preview = truncate_string(String::from_utf8_lossy(&current).to_string(), 4096);
            results.push(bifrost_script::ScriptExecutionResult {
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

        let script_ctx = ScriptContext {
            request_id: ctx.id_str().to_string(),
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
            mgr.engine()
                .execute_decode_script_with_config(
                    script_name,
                    phase,
                    request_data,
                    req_bytes,
                    response_data,
                    res_bytes,
                    &script_ctx,
                    cfg,
                )
                .await
        } else {
            mgr.engine()
                .execute_decode_script(
                    script_name,
                    phase,
                    request_data,
                    req_bytes,
                    response_data,
                    res_bytes,
                    &script_ctx,
                )
                .await
        };

        match result {
            Ok((out, logs)) => {
                let mut data_preview = out.data.clone();
                let mut msg_preview = out.msg.clone();
                data_preview = truncate_string(data_preview, 4096);
                msg_preview = truncate_string(msg_preview, 4096);

                let success = out.code == "0";
                let err = if success {
                    None
                } else {
                    Some(format!("decode 输出 code != 0: {}", msg_preview))
                };

                results.push(bifrost_script::ScriptExecutionResult {
                    script_name: script_name.to_string(),
                    script_type: ScriptType::Decode,
                    success,
                    error: err,
                    duration_ms: start.elapsed().as_millis() as u64,
                    logs: limit_script_logs(logs, 100),
                    request_modifications: None,
                    response_modifications: None,
                    decode_output: Some(bifrost_script::DecodeOutput {
                        code: out.code,
                        data: data_preview,
                        msg: msg_preview,
                    }),
                });

                if success {
                    current = out.data.into_bytes();
                } else {
                    // decode 失败：不覆盖当前内容（避免丢失可读数据）；终止链路。
                    break;
                }
            }
            Err(e) => {
                results.push(bifrost_script::ScriptExecutionResult {
                    script_name: script_name.to_string(),
                    script_type: ScriptType::Decode,
                    success: false,
                    error: Some(format!("decode 脚本执行失败: {}", e)),
                    duration_ms: start.elapsed().as_millis() as u64,
                    logs: vec![],
                    request_modifications: None,
                    response_modifications: None,
                    decode_output: None,
                });
                // decode 失败：不覆盖当前内容；终止链路。
                break;
            }
        }
    }

    DecodeForStorageResult {
        output: Bytes::from(current),
        results,
    }
}

pub(super) fn parse_url_parts(url: &str) -> (String, String, String) {
    if let Ok(parsed) = url::Url::parse(url) {
        let host = parsed.host_str().unwrap_or("").to_string();
        let path = parsed.path().to_string();
        let protocol = parsed.scheme().to_string();
        (host, path, protocol)
    } else {
        ("".to_string(), url.to_string(), "http".to_string())
    }
}

pub(super) async fn get_values_from_state(
    admin_state: &Option<Arc<AdminState>>,
) -> HashMap<String, String> {
    use bifrost_core::ValueStore;
    if let Some(state) = admin_state {
        if let Some(values_storage) = &state.values_storage {
            let storage = values_storage.read();
            return storage.as_hashmap();
        }
    }
    HashMap::new()
}
