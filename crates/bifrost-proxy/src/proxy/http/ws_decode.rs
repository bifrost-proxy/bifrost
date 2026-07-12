use std::collections::HashMap;
use std::sync::Arc;

use bifrost_admin::{AdminState, FrameDirection, FrameType};
use bifrost_script::{MatchedRuleInfo, RequestData, ResponseData, ScriptContext, ScriptType};
use bytes::Bytes;
use tracing::warn;

use crate::protocol::parse_permessage_deflate_config;
use crate::server::ResolvedRules;
use crate::utils::logging::RequestContext;

#[derive(Debug, Clone, Default)]
pub struct WsHandshakeMeta {
    pub negotiated_protocol: Option<String>,
    pub negotiated_extensions: Option<String>,
}

fn build_matched_rules_info(resolved_rules: &ResolvedRules) -> Vec<MatchedRuleInfo> {
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

fn parse_url_parts(url: &str) -> (String, String, String) {
    if let Ok(parsed) = url::Url::parse(url) {
        let host = parsed.host_str().unwrap_or("").to_string();
        let path = parsed.path().to_string();
        let protocol = parsed.scheme().to_string();
        (host, path, protocol)
    } else {
        ("".to_string(), url.to_string(), "http".to_string())
    }
}

async fn get_values_from_state(admin_state: &Option<Arc<AdminState>>) -> HashMap<String, String> {
    use bifrost_core::ValueStore;
    if let Some(state) = admin_state {
        if let Some(values_storage) = &state.values_storage {
            let storage = values_storage.read();
            return storage.as_hashmap();
        }
    }
    HashMap::new()
}

fn is_builtin_decoder(name: &str) -> bool {
    matches!(name, "utf8" | "default")
}

fn builtin_decode_utf8(input: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(input).to_string().into_bytes()
}

#[allow(clippy::too_many_arguments)]
pub async fn decode_ws_payload_for_storage(
    admin_state: &Option<Arc<AdminState>>,
    script_names: &[String],
    ctx: &RequestContext,
    resolved_rules: &ResolvedRules,
    request_url: &str,
    request_method: &str,
    request_headers: &[(String, String)],
    ws_meta: &WsHandshakeMeta,
    direction: FrameDirection,
    frame_type: FrameType,
    payload_bytes: &[u8],
) -> Option<Vec<u8>> {
    if script_names.is_empty() || payload_bytes.is_empty() {
        return None;
    }

    let state = admin_state.as_ref()?;
    let manager = state.script_manager.as_ref()?;
    let cfg = if let Some(cm) = state.config_manager.as_ref() {
        Some(cm.config().await)
    } else {
        None
    };

    const MAX_DECODE_INPUT_BYTES: usize = 2 * 1024 * 1024;
    if payload_bytes.len() > MAX_DECODE_INPUT_BYTES {
        warn!(
            "[{}] [DECODE][WS] skip decode ({} bytes > {} limit)",
            ctx.id_str(),
            payload_bytes.len(),
            MAX_DECODE_INPUT_BYTES
        );
        return None;
    }

    let mut values = resolved_rules.values.clone();
    for (k, v) in get_values_from_state(admin_state).await {
        values.entry(k).or_insert(v);
    }
    values.insert(
        "ws_direction".to_string(),
        format!("{:?}", direction).to_lowercase(),
    );
    values.insert(
        "ws_frame_type".to_string(),
        format!("{:?}", frame_type).to_lowercase(),
    );
    values.insert(
        "ws_payload_size".to_string(),
        payload_bytes.len().to_string(),
    );

    if let Some(ref proto) = ws_meta.negotiated_protocol {
        values.insert("ws_subprotocol".to_string(), proto.clone());
    }
    if let Some(ref ext) = ws_meta.negotiated_extensions {
        values.insert("ws_extensions".to_string(), ext.clone());
        if let Some(cfg) = parse_permessage_deflate_config(ext) {
            values.insert(
                "ws_permessage_deflate".to_string(),
                cfg.enabled().to_string(),
            );
            values.insert(
                "ws_client_no_context_takeover".to_string(),
                cfg.client_no_context_takeover.to_string(),
            );
            values.insert(
                "ws_server_no_context_takeover".to_string(),
                cfg.server_no_context_takeover.to_string(),
            );
            if let Some(bits) = cfg.client_max_window_bits {
                values.insert("ws_client_max_window_bits".to_string(), bits.to_string());
            }
            if let Some(bits) = cfg.server_max_window_bits {
                values.insert("ws_server_max_window_bits".to_string(), bits.to_string());
            }
        } else {
            values.insert("ws_permessage_deflate".to_string(), "false".to_string());
        }
    }

    if let Ok(parsed) = url::Url::parse(request_url) {
        if let Some(h) = parsed.host_str() {
            values.insert("ws_target_host".to_string(), h.to_string());
        }
        if let Some(p) = parsed.port_or_known_default() {
            values.insert("ws_target_port".to_string(), p.to_string());
        }
        let tls = matches!(parsed.scheme(), "wss" | "https");
        values.insert("ws_is_tls".to_string(), tls.to_string());
    }

    let matched_rules = build_matched_rules_info(resolved_rules);
    let (host, path, protocol) = parse_url_parts(request_url);

    let request_data = RequestData {
        url: request_url.to_string(),
        method: request_method.to_string(),
        host,
        path,
        protocol,
        client_ip: ctx.client_ip.clone(),
        client_app: ctx.client_app.clone(),
        headers: request_headers.iter().cloned().collect(),
        body: None,
    };

    let mut response_headers = HashMap::new();
    if let Some(ref proto) = ws_meta.negotiated_protocol {
        response_headers.insert("Sec-WebSocket-Protocol".to_string(), proto.clone());
    }
    if let Some(ref ext) = ws_meta.negotiated_extensions {
        response_headers.insert("Sec-WebSocket-Extensions".to_string(), ext.clone());
    }

    // 附加基础元信息，方便脚本侧判断握手类型。
    response_headers.insert("Upgrade".to_string(), "websocket".to_string());
    response_headers.insert("Connection".to_string(), "Upgrade".to_string());

    let response_data = ResponseData {
        status: 101,
        status_text: "Switching Protocols".to_string(),
        headers: response_headers,
        body: None,
        request: request_data.clone(),
    };

    let phase = match direction {
        FrameDirection::Send => "websocket_send",
        FrameDirection::Receive => "websocket_recv",
    };

    let mut current = payload_bytes.to_vec();
    let mgr = manager.read().await;

    let mut applied = false;
    for script_name in script_names {
        let script_name = script_name.trim();
        if script_name.is_empty() || is_builtin_decoder(script_name) {
            current = builtin_decode_utf8(&current);
            applied = true;
            continue;
        }

        let script_ctx = ScriptContext {
            request_id: ctx.id_str().to_string(),
            script_name: script_name.to_string(),
            script_type: ScriptType::Decode,
            values: values.clone(),
            matched_rules: matched_rules.clone(),
        };

        let exec = if let Some(ref cfg) = cfg {
            mgr.engine()
                .execute_decode_script_with_config(
                    script_name,
                    phase,
                    &request_data,
                    if matches!(direction, FrameDirection::Send) {
                        &current
                    } else {
                        &[]
                    },
                    &response_data,
                    if matches!(direction, FrameDirection::Receive) {
                        &current
                    } else {
                        &[]
                    },
                    &script_ctx,
                    cfg,
                )
                .await
        } else {
            mgr.engine()
                .execute_decode_script(
                    script_name,
                    phase,
                    &request_data,
                    if matches!(direction, FrameDirection::Send) {
                        &current
                    } else {
                        &[]
                    },
                    &response_data,
                    if matches!(direction, FrameDirection::Receive) {
                        &current
                    } else {
                        &[]
                    },
                    &script_ctx,
                )
                .await
        };

        if let Ok((out, _logs)) = exec {
            if out.code == "0" {
                current = Bytes::from(out.data).to_vec();
                applied = true;
            } else {
                current = Bytes::from(out.msg).to_vec();
                applied = true;
                break;
            }
        }
    }

    if applied {
        Some(current)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use bifrost_admin::{
        AdminState, FrameDirection, FrameType, ScriptManager, SharedScriptManager,
        SharedValuesStorage,
    };
    use bifrost_core::Protocol;
    use bifrost_storage::ValuesStorage;
    use parking_lot::RwLock as ParkingRwLock;
    use rand::random;
    use tokio::sync::RwLock as TokioRwLock;

    fn temp_path(prefix: &str) -> std::path::PathBuf {
        let mut base = std::env::temp_dir();
        base.push(format!(
            "bifrost-proxy-tests-ws-{prefix}-{}",
            random::<u64>()
        ));
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

    fn make_ctx() -> RequestContext {
        RequestContext::new()
    }

    #[test]
    fn parse_url_parts_handles_valid_and_invalid_urls() {
        let (host, path, protocol) = parse_url_parts("ws://example.com/chat?token=1");
        assert_eq!(host, "example.com");
        assert_eq!(path, "/chat");
        assert_eq!(protocol, "ws");

        let (host, path, protocol) = parse_url_parts("not a url");
        assert_eq!(host, "");
        assert_eq!(path, "not a url");
        assert_eq!(protocol, "http");
    }

    #[test]
    fn is_builtin_decoder_matches_exact_names_only() {
        assert!(is_builtin_decoder("utf8"));
        assert!(is_builtin_decoder("default"));
        assert!(!is_builtin_decoder(" utf8"));
        assert!(!is_builtin_decoder("other"));
    }

    #[test]
    fn builtin_decode_utf8_roundtrips_and_is_lossy() {
        let ascii = b"hello";
        assert_eq!(builtin_decode_utf8(ascii), b"hello".to_vec());

        let invalid = vec![0xff, b'a'];
        let decoded = String::from_utf8(builtin_decode_utf8(&invalid)).unwrap();
        assert_eq!(decoded, String::from_utf8_lossy(&invalid));
    }

    #[test]
    fn build_matched_rules_info_copies_fields() {
        use crate::server::RuleValue;

        let rule = RuleValue {
            pattern: "example.test".to_string(),
            protocol: Protocol::Http,
            value: "foo".to_string(),
            options: HashMap::new(),
            rule_name: None,
            raw: None,
            line: None,
            auto_tls_intercept: false,
        };
        let resolved = ResolvedRules {
            rules: vec![rule.clone()],
            ..Default::default()
        };

        let infos = build_matched_rules_info(&resolved);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].pattern, rule.pattern);
        assert_eq!(infos[0].value, rule.value);
        assert_eq!(infos[0].protocol, rule.protocol.to_string());
    }

    #[tokio::test]
    async fn get_values_from_state_handles_none_and_some() {
        let none_state: Option<Arc<AdminState>> = None;
        let values = get_values_from_state(&none_state).await;
        assert!(values.is_empty());

        let state = make_admin_state_with_values(&[("vk", "vv")]);
        let some_state = Some(state);
        let values = get_values_from_state(&some_state).await;
        assert_eq!(values.get("vk"), Some(&"vv".to_string()));
    }

    #[tokio::test]
    async fn decode_ws_payload_returns_none_for_empty_inputs_and_missing_state() {
        let ctx = make_ctx();
        let resolved = ResolvedRules::default();
        let meta = WsHandshakeMeta::default();

        // No scripts
        let out = decode_ws_payload_for_storage(
            &None,
            &[],
            &ctx,
            &resolved,
            "ws://example.com/ws",
            "GET",
            &[],
            &meta,
            FrameDirection::Send,
            FrameType::Text,
            b"some payload",
        )
        .await;
        assert!(out.is_none());

        // Empty payload
        let out = decode_ws_payload_for_storage(
            &None,
            &["utf8".to_string()],
            &ctx,
            &resolved,
            "ws://example.com/ws",
            "GET",
            &[],
            &meta,
            FrameDirection::Send,
            FrameType::Text,
            &[],
        )
        .await;
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn decode_ws_payload_skips_large_payload_with_marker() {
        let admin = make_admin_state_with_script_manager();
        let admin_state = Some(admin);
        let ctx = make_ctx();
        let resolved = ResolvedRules::default();
        let meta = WsHandshakeMeta::default();
        let headers = vec![("Host".to_string(), "example.com".to_string())];

        let big = vec![b'x'; 2 * 1024 * 1024 + 1];
        let out = decode_ws_payload_for_storage(
            &admin_state,
            &["utf8".to_string()],
            &ctx,
            &resolved,
            "ws://example.com/ws",
            "GET",
            &headers,
            &meta,
            FrameDirection::Send,
            FrameType::Text,
            &big,
        )
        .await;
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn decode_ws_payload_applies_builtin_utf8_decoder() {
        let admin = make_admin_state_with_script_manager();
        let admin_state = Some(admin);
        let ctx = make_ctx();
        let resolved = ResolvedRules::default();
        let meta = WsHandshakeMeta::default();
        let headers = vec![("Host".to_string(), "example.com".to_string())];

        let payload = vec![0xff, b'a'];
        let out = decode_ws_payload_for_storage(
            &admin_state,
            &["utf8".to_string()],
            &ctx,
            &resolved,
            "ws://example.com/ws",
            "GET",
            &headers,
            &meta,
            FrameDirection::Send,
            FrameType::Text,
            &payload,
        )
        .await;

        let result = out.expect("expected some output");
        assert_eq!(result, builtin_decode_utf8(&payload));
    }

    #[tokio::test]
    async fn decode_ws_payload_covers_metadata_receive_and_failed_script_paths() {
        let admin = make_admin_state_with_script_manager();
        let admin_state = Some(admin);
        let ctx = make_ctx();
        let mut resolved = ResolvedRules::default();
        resolved
            .values
            .insert("existing".to_string(), "rule".to_string());
        let meta = WsHandshakeMeta {
            negotiated_protocol: Some("chat.v1".to_string()),
            negotiated_extensions: Some(
                "permessage-deflate; client_no_context_takeover; server_no_context_takeover; client_max_window_bits=12; server_max_window_bits=13"
                    .to_string(),
            ),
        };
        let headers = vec![("Authorization".to_string(), "Bearer test".to_string())];

        let out = decode_ws_payload_for_storage(
            &admin_state,
            &["missing-script".to_string()],
            &ctx,
            &resolved,
            "wss://example.com:9443/socket?token=1",
            "GET",
            &headers,
            &meta,
            FrameDirection::Receive,
            FrameType::Binary,
            b"payload",
        )
        .await;
        assert!(out.is_none());

        let out = decode_ws_payload_for_storage(
            &admin_state,
            &[
                " ".to_string(),
                "missing-script".to_string(),
                "default".to_string(),
            ],
            &ctx,
            &resolved,
            "not a websocket url",
            "POST",
            &headers,
            &WsHandshakeMeta {
                negotiated_protocol: None,
                negotiated_extensions: Some("unsupported-extension".to_string()),
            },
            FrameDirection::Send,
            FrameType::Text,
            b"payload",
        )
        .await;
        assert_eq!(out.unwrap(), b"payload".to_vec());
    }

    #[tokio::test]
    async fn coverage_90_custom_scripts_cover_configured_send_receive_and_error_outputs() {
        let harness = bifrost_admin::test_support::TestAdminState::builder()
            .port(19610)
            .build();
        let manager = ScriptManager::new(harness.data_dir().join("ws-decode-scripts"));
        manager.init().await.unwrap();
        manager
            .engine()
            .save_script(
                ScriptType::Decode,
                "ws-success",
                r#"ctx.output = { code: "0", data: "decoded-ws", msg: "" };"#,
            )
            .await
            .unwrap();
        manager
            .engine()
            .save_script(
                ScriptType::Decode,
                "ws-error",
                r#"ctx.output = { code: "9", data: "ignored", msg: "decode-rejected" };"#,
            )
            .await
            .unwrap();
        let state = Arc::new(
            AdminState::new(19610)
                .with_config_manager_shared(harness.config_manager.clone())
                .with_script_manager(manager),
        );
        let admin = Some(state);
        let ctx = make_ctx();
        let rules = ResolvedRules {
            values: HashMap::from([("rule-value".to_string(), "present".to_string())]),
            ..Default::default()
        };
        let meta = WsHandshakeMeta {
            negotiated_protocol: Some("chat.v1".to_string()),
            negotiated_extensions: Some("permessage-deflate".to_string()),
        };
        for direction in [FrameDirection::Send, FrameDirection::Receive] {
            let decoded = decode_ws_payload_for_storage(
                &admin,
                &["ws-success".to_string()],
                &ctx,
                &rules,
                "wss://example.test/socket",
                "GET",
                &[("x-test".to_string(), "yes".to_string())],
                &meta,
                direction,
                FrameType::Binary,
                b"wire-payload",
            )
            .await;
            assert_eq!(decoded.as_deref(), Some(b"decoded-ws".as_slice()));
        }
        let rejected = decode_ws_payload_for_storage(
            &admin,
            &["ws-error".to_string(), "ws-success".to_string()],
            &ctx,
            &rules,
            "ws://example.test/socket",
            "POST",
            &[],
            &WsHandshakeMeta::default(),
            FrameDirection::Receive,
            FrameType::Text,
            b"wire-payload",
        )
        .await;
        assert_eq!(rejected.as_deref(), Some(b"decode-rejected".as_slice()));
    }
}
