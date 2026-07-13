use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use bifrost_admin::AdminState;
use bifrost_core::text::truncate_bytes_with_suffix;
use bifrost_script::{RequestData, ResponseData, ScriptContext, ScriptType};
use bytes::Bytes;
use tracing::warn;

use crate::server::ResolvedRules;
use crate::utils::logging::RequestContext;

use super::super::scripts::build_matched_rules_info;

fn is_builtin_decoder(name: &str) -> bool {
    matches!(name.trim(), "utf8" | "default")
}

fn builtin_decode_utf8(input: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(input).to_string().into_bytes()
}

#[derive(Debug, Clone)]
pub(in crate::proxy::http) struct DecodeForStorageResult {
    pub(in crate::proxy::http) output: Bytes,
    pub(in crate::proxy::http) results: Vec<bifrost_script::ScriptExecutionResult>,
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
pub(in crate::proxy::http) async fn apply_decode_scripts_for_storage(
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

        if script_name == "bp" {
            if resolved_rules.bp_scripts.is_empty() {
                results.push(bifrost_script::ScriptExecutionResult {
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

            for parser_ref in &resolved_rules.bp_scripts {
                let parser_ref = parser_ref.trim();
                if parser_ref.is_empty() {
                    continue;
                }

                let script_ctx = ScriptContext {
                    request_id: ctx.id_str().to_string(),
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
                    mgr.execute_parser_script_with_config(
                        parser_ref,
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
                        .execute_parser_script(
                            parser_ref,
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
                        let success = out.code == "0";
                        let data_preview = truncate_string(out.data.clone(), 4096);
                        let msg_preview = truncate_string(out.msg.clone(), 4096);
                        let err = if success {
                            None
                        } else {
                            Some(format!("bp parser 输出 code != 0: {}", msg_preview))
                        };
                        results.push(bifrost_script::ScriptExecutionResult {
                            script_name: parser_ref.to_string(),
                            script_type: ScriptType::Parser,
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
                            break;
                        }
                    }
                    Err(e) => {
                        results.push(bifrost_script::ScriptExecutionResult {
                            script_name: parser_ref.to_string(),
                            script_type: ScriptType::Parser,
                            success: false,
                            error: Some(format!("bp parser 执行失败: {}", e)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use bifrost_admin::{AdminState, ScriptManager, SharedScriptManager, SharedValuesStorage};
    use bifrost_storage::ValuesStorage;
    use bytes::Bytes;
    use parking_lot::RwLock as ParkingRwLock;
    use rand::random;
    use tokio::sync::RwLock as TokioRwLock;

    fn temp_path(prefix: &str) -> std::path::PathBuf {
        let mut base = std::env::temp_dir();
        base.push(format!("bifrost-proxy-tests-{prefix}-{}", random::<u64>()));
        base
    }

    fn make_admin_state_with_script_manager() -> Arc<AdminState> {
        let mut state = AdminState::new(0);
        let scripts_dir = temp_path("scripts");
        std::fs::create_dir_all(&scripts_dir).expect("create scripts dir");
        let manager = ScriptManager::new(scripts_dir);
        let shared: SharedScriptManager = Arc::new(TokioRwLock::new(manager));
        state.script_manager = Some(shared);
        Arc::new(state)
    }

    fn make_admin_state_with_values(values: &[(&str, &str)]) -> Arc<AdminState> {
        let mut state = AdminState::new(0);
        let values_dir = temp_path("values");
        let mut storage = ValuesStorage::with_dir(values_dir).expect("values storage");
        for (k, v) in values {
            storage.set_value(k, v).expect("set value");
        }
        let shared: SharedValuesStorage = Arc::new(ParkingRwLock::new(storage));
        state.values_storage = Some(shared);
        Arc::new(state)
    }

    fn make_request_context() -> RequestContext {
        RequestContext::new()
    }

    fn make_script_io() -> (RequestData, ResponseData) {
        (RequestData::default(), ResponseData::default())
    }

    #[test]
    fn is_builtin_decoder_recognizes_utf8_and_default() {
        assert!(is_builtin_decoder("utf8"));
        assert!(is_builtin_decoder(" default "));
        assert!(!is_builtin_decoder("UTF8"));
        assert!(!is_builtin_decoder("other"));
    }

    #[test]
    fn builtin_decode_utf8_handles_invalid_utf8() {
        let input = vec![0xff, b'a'];
        let decoded = builtin_decode_utf8(&input);
        let s = String::from_utf8(decoded).unwrap();
        assert_eq!(s, String::from_utf8_lossy(&input));
    }

    #[test]
    fn truncate_string_returns_original_when_short() {
        let s = "hello".to_string();
        assert_eq!(truncate_string(s.clone(), 10), s);
    }

    #[test]
    fn truncate_string_truncates_with_suffix_and_keeps_utf8_boundary() {
        let s = "ab前cd".to_string();
        let truncated = truncate_string(s.clone(), 3);
        assert!(truncated.starts_with("ab"));
        assert!(truncated.ends_with("…(truncated)"));
        assert!(!truncated.contains('前'));
        assert_eq!(
            truncate_string("abcdef".to_string(), 3),
            "abc…(truncated)".to_string()
        );
    }

    fn mk_log(message: &str) -> bifrost_script::ScriptLogEntry {
        bifrost_script::ScriptLogEntry {
            timestamp: 1,
            level: bifrost_script::ScriptLogLevel::Info,
            message: message.to_string(),
            args: None,
        }
    }

    #[test]
    fn limit_script_logs_keeps_all_when_within_limit() {
        let logs = vec![mk_log("a"), mk_log("b")];
        let out = limit_script_logs(logs.clone(), 10);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].message, "a");
        assert_eq!(out[1].message, "b");
    }

    #[test]
    fn limit_script_logs_keeps_tail_when_exceeding_limit() {
        let logs = vec![mk_log("l1"), mk_log("l2"), mk_log("l3"), mk_log("l4")];
        let out = limit_script_logs(logs, 2);
        let messages: Vec<_> = out.iter().map(|l| l.message.as_str()).collect();
        assert_eq!(messages, vec!["l3", "l4"]);
    }

    #[tokio::test]
    async fn apply_decode_scripts_returns_original_when_no_scripts_or_body() {
        let admin_state: Option<Arc<AdminState>> = None;
        let ctx = make_request_context();
        let resolved = ResolvedRules::default();
        let (req, res) = make_script_io();
        let values = HashMap::new();
        let body = Bytes::from_static(b"hello");

        let result = apply_decode_scripts_for_storage(
            &admin_state,
            &[],
            "request",
            &ctx,
            &resolved,
            &req,
            &res,
            &values,
            body.clone(),
        )
        .await;

        assert_eq!(result.output, body);
        assert!(result.results.is_empty());

        let empty_body = Bytes::new();
        let result = apply_decode_scripts_for_storage(
            &admin_state,
            &["utf8".to_string()],
            "request",
            &ctx,
            &resolved,
            &req,
            &res,
            &values,
            empty_body.clone(),
        )
        .await;
        assert_eq!(result.output, empty_body);
        assert!(result.results.is_empty());
    }

    #[tokio::test]
    async fn apply_decode_scripts_returns_original_when_admin_state_or_manager_missing() {
        let ctx = make_request_context();
        let resolved = ResolvedRules::default();
        let (req, res) = make_script_io();
        let values = HashMap::new();
        let body = Bytes::from_static(b"body");
        let scripts = ["custom".to_string()];

        // admin_state = None
        let admin_none: Option<Arc<AdminState>> = None;
        let result = apply_decode_scripts_for_storage(
            &admin_none,
            &scripts,
            "response",
            &ctx,
            &resolved,
            &req,
            &res,
            &values,
            body.clone(),
        )
        .await;
        assert_eq!(result.output, body);
        assert!(result.results.is_empty());

        // admin_state present but script_manager is None
        let state = Arc::new(AdminState::new(0));
        let admin_some = Some(state);
        let result = apply_decode_scripts_for_storage(
            &admin_some,
            &scripts,
            "response",
            &ctx,
            &resolved,
            &req,
            &res,
            &values,
            body.clone(),
        )
        .await;
        assert_eq!(result.output, body);
        assert!(result.results.is_empty());
    }

    #[tokio::test]
    async fn apply_decode_scripts_skips_large_input_with_marker_result() {
        let admin = make_admin_state_with_script_manager();
        let admin_state = Some(admin);
        let ctx = make_request_context();
        let resolved = ResolvedRules::default();
        let (req, res) = make_script_io();
        let values = HashMap::new();
        let large = Bytes::from(vec![b'a'; 2 * 1024 * 1024 + 1]);

        let result = apply_decode_scripts_for_storage(
            &admin_state,
            &["utf8".to_string()],
            "request",
            &ctx,
            &resolved,
            &req,
            &res,
            &values,
            large.clone(),
        )
        .await;

        assert_eq!(result.output, large);
        assert_eq!(result.results.len(), 1);
        let r = &result.results[0];
        assert_eq!(r.script_name, "__bifrost_skip__");
        assert_eq!(r.script_type, ScriptType::Decode);
        assert!(!r.success);
        assert!(r
            .error
            .as_ref()
            .expect("error message")
            .contains("decode 输入过大"));
    }

    #[tokio::test]
    async fn apply_decode_scripts_bp_without_parser_scripts_produces_error() {
        let admin = make_admin_state_with_script_manager();
        let admin_state = Some(admin);
        let ctx = make_request_context();
        let resolved = ResolvedRules {
            bp_scripts: Vec::new(),
            ..Default::default()
        };
        let (req, res) = make_script_io();
        let values = HashMap::new();
        let body = Bytes::from_static(b"body");

        let result = apply_decode_scripts_for_storage(
            &admin_state,
            &["bp".to_string()],
            "request",
            &ctx,
            &resolved,
            &req,
            &res,
            &values,
            body.clone(),
        )
        .await;

        assert_eq!(result.output, body);
        assert_eq!(result.results.len(), 1);
        let r = &result.results[0];
        assert_eq!(r.script_name, "bp");
        assert_eq!(r.script_type, ScriptType::Parser);
        assert!(!r.success);
        assert!(r
            .error
            .as_ref()
            .expect("error message")
            .contains("decode://bp requires at least one bp:// parser script"));
        assert!(r.decode_output.is_none());
    }

    #[tokio::test]
    async fn get_values_from_state_handles_none_and_some() {
        let none_state: Option<Arc<AdminState>> = None;
        let values = get_values_from_state(&none_state).await;
        assert!(values.is_empty());

        let state = make_admin_state_with_values(&[("k1", "v1"), ("k2", "v2")]);
        let some_state = Some(state);
        let values = get_values_from_state(&some_state).await;
        assert_eq!(values.get("k1"), Some(&"v1".to_string()));
        assert_eq!(values.get("k2"), Some(&"v2".to_string()));
    }

    #[tokio::test]
    async fn apply_decode_scripts_runs_builtin_utf8_and_default() {
        let admin = make_admin_state_with_script_manager();
        let admin_state = Some(admin);
        let ctx = make_request_context();
        let resolved = ResolvedRules::default();
        let (req, res) = make_script_io();
        let values = HashMap::new();
        let body = Bytes::from_static(b"hello");

        let result_utf8 = apply_decode_scripts_for_storage(
            &admin_state,
            &["utf8".to_string()],
            "response",
            &ctx,
            &resolved,
            &req,
            &res,
            &values,
            body.clone(),
        )
        .await;

        assert_eq!(result_utf8.output, body);
        assert_eq!(result_utf8.results.len(), 1);
        let r = &result_utf8.results[0];
        assert_eq!(r.script_name, "utf8");
        assert!(r.success);
        assert_eq!(r.script_type, ScriptType::Decode);
        assert!(r.decode_output.as_ref().is_some());
        assert!(r.decode_output.as_ref().unwrap().data.contains("hello"));

        let result_default = apply_decode_scripts_for_storage(
            &admin_state,
            &["default".to_string()],
            "response",
            &ctx,
            &resolved,
            &req,
            &res,
            &values,
            body.clone(),
        )
        .await;

        assert_eq!(result_default.output, body);
        assert_eq!(result_default.results.len(), 1);
        let r = &result_default.results[0];
        assert_eq!(r.script_name, "default");
        assert!(r.success);
        assert_eq!(r.script_type, ScriptType::Decode);
    }

    #[tokio::test]
    async fn coverage_90_custom_decode_scripts_cover_success_failure_and_missing_paths() {
        let admin = make_admin_state_with_script_manager();
        {
            let manager = admin.script_manager.as_ref().unwrap().read().await;
            manager.init().await.unwrap();
            manager
                .engine()
                .save_script(
                    ScriptType::Decode,
                    "decode-success",
                    r#"ctx.output = { code: "0", data: "decoded-success", msg: "" };"#,
                )
                .await
                .unwrap();
            manager
                .engine()
                .save_script(
                    ScriptType::Decode,
                    "decode-failure",
                    r#"ctx.output = { code: "1", data: "ignored", msg: "expected failure" };"#,
                )
                .await
                .unwrap();
        }
        let (req, res) = make_script_io();
        let scripts = vec![
            " ".to_string(),
            "decode-success".to_string(),
            "decode-failure".to_string(),
            "not-reached".to_string(),
        ];
        let result = apply_decode_scripts_for_storage(
            &Some(admin.clone()),
            &scripts,
            "response",
            &make_request_context(),
            &ResolvedRules::default(),
            &req,
            &res,
            &HashMap::new(),
            Bytes::from_static(b"original"),
        )
        .await;
        assert_eq!(result.output, Bytes::from_static(b"decoded-success"));
        assert_eq!(result.results.len(), 2);
        assert!(result.results[0].success);
        assert!(!result.results[1].success);

        let missing = apply_decode_scripts_for_storage(
            &Some(admin),
            &["missing-script".to_string()],
            "request",
            &make_request_context(),
            &ResolvedRules::default(),
            &req,
            &res,
            &HashMap::new(),
            Bytes::from_static(b"original"),
        )
        .await;
        assert_eq!(missing.output, Bytes::from_static(b"original"));
        assert_eq!(missing.results.len(), 1);
        assert!(!missing.results[0].success);
        assert!(missing.results[0].decode_output.is_none());
    }

    #[tokio::test]
    async fn coverage_90_bp_parser_scripts_cover_success_failure_and_missing_paths() {
        let admin = make_admin_state_with_script_manager();
        {
            let manager = admin.script_manager.as_ref().unwrap().read().await;
            manager.init().await.unwrap();
            manager
                .engine()
                .save_script(
                    ScriptType::Parser,
                    "parser-success",
                    r#"ctx.output = { code: "0", data: "parser-output", msg: "" };"#,
                )
                .await
                .unwrap();
            manager
                .engine()
                .save_script(
                    ScriptType::Parser,
                    "parser-failure",
                    r#"ctx.output = { code: "2", data: "ignored", msg: "parser rejected" };"#,
                )
                .await
                .unwrap();
        }
        let (req, res) = make_script_io();
        let rules = ResolvedRules {
            bp_scripts: vec![" ".into(), "parser-success".into(), "parser-failure".into()],
            ..Default::default()
        };
        let result = apply_decode_scripts_for_storage(
            &Some(admin.clone()),
            &["bp".to_string()],
            "request",
            &make_request_context(),
            &rules,
            &req,
            &res,
            &HashMap::new(),
            Bytes::from_static(b"original"),
        )
        .await;
        assert_eq!(result.output, Bytes::from_static(b"parser-output"));
        assert_eq!(result.results.len(), 2);
        assert!(result.results[0].success);
        assert!(!result.results[1].success);

        let missing_rules = ResolvedRules {
            bp_scripts: vec!["missing-parser".into()],
            ..Default::default()
        };
        let missing = apply_decode_scripts_for_storage(
            &Some(admin),
            &["bp".to_string()],
            "response",
            &make_request_context(),
            &missing_rules,
            &req,
            &res,
            &HashMap::new(),
            Bytes::from_static(b"original"),
        )
        .await;
        assert_eq!(missing.results.len(), 1);
        assert!(!missing.results[0].success);
    }
}
