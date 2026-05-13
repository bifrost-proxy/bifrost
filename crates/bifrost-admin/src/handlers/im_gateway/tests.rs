use super::*;
use crate::im_gateway::types::ImProviderType;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::sync::Mutex;

pub(super) struct EnvGuard {
    old_data_dir: Option<String>,
}

impl EnvGuard {
    pub(super) fn set_data_dir(data_dir: &std::path::Path) -> Self {
        let old_data_dir = std::env::var("BIFROST_DATA_DIR").ok();
        std::env::set_var("BIFROST_DATA_DIR", data_dir);
        Self { old_data_dir }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.old_data_dir.as_deref() {
            Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
            None => std::env::remove_var("BIFROST_DATA_DIR"),
        }
    }
}

#[test]
pub(super) fn online_notification_message_uses_provider_work_dir_override() {
    let mut provider = test_provider();
    provider.agent_config = Some(ImProviderAgentConfig {
        work_dir: Some("/custom/im-provider-workdir".to_string()),
        base_instructions: None,
        developer_instructions: None,
        user_instructions: None,
    });

    let message = build_online_notification_message(&provider);

    assert!(message.starts_with("你好，Bifrost 助手上线了"));
    assert!(message.contains("工作目录：/custom/im-provider-workdir"));
}

#[test]
pub(super) fn online_notification_message_falls_back_to_process_work_dir() {
    let cwd = std::env::current_dir()
        .expect("current dir")
        .display()
        .to_string();
    let provider = test_provider();

    let message = build_online_notification_message(&provider);

    assert!(message.starts_with("你好，Bifrost 助手上线了"));
    assert!(message.contains("工作目录："));
    assert!(message.contains(&cwd));
}

#[test]
pub(super) fn agent_reply_image_path_resolution_uses_work_dir_and_skips_remote_or_image_key() {
    let base = std::path::Path::new("/tmp/im-agent-workdir");

    assert_eq!(
        resolve_agent_reply_image_path("./chart.png", Some(base)).as_deref(),
        Some(std::path::Path::new("/tmp/im-agent-workdir/./chart.png"))
    );
    assert_eq!(
        resolve_agent_reply_image_path("/tmp/chart.png", Some(base)).as_deref(),
        Some(std::path::Path::new("/tmp/chart.png"))
    );
    assert_eq!(
        resolve_agent_reply_image_path("file:///tmp/chart.png", Some(base)).as_deref(),
        Some(std::path::Path::new("/tmp/chart.png"))
    );
    assert!(resolve_agent_reply_image_path("https://example.com/chart.png", Some(base)).is_none());
    assert!(resolve_agent_reply_image_path("img_v3_chart", Some(base)).is_none());
    assert!(resolve_agent_reply_image_path("./chart.png", None).is_none());
}

#[test]
pub(super) fn markdown_image_destination_strips_wrappers_and_title() {
    assert_eq!(markdown_image_destination("<./chart.png>"), "./chart.png");
    assert_eq!(
        markdown_image_destination("./chart.png \"Chart title\""),
        "./chart.png"
    );
    assert_eq!(
        markdown_image_destination("./chart.png 'Chart title'"),
        "./chart.png"
    );
}

#[test]
pub(super) fn agent_reply_target_uses_weixin_sender_instead_of_owner() {
    let mut provider = test_provider();
    provider.provider_type = ImProviderType::Weixin;
    provider.owner_open_id = Some("owner@im.wechat".to_string());
    let event = ImEvent {
        event_id: "evt-1".to_string(),
        provider_id: provider.id.clone(),
        provider_type: ImProviderType::Weixin,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("sender@im.wechat".to_string()),
            user_id: Some("sender@im.wechat".to_string()),
            message_id: Some("msg-1".to_string()),
        },
        message: None,
        received_at: 0,
        raw_digest: None,
    };

    assert_eq!(
        agent_reply_target_id(&provider, &event).as_deref(),
        Some("sender@im.wechat")
    );
}

#[test]
pub(super) fn local_image_fallback_removes_markdown_image_syntax() {
    let fallback = local_image_fallback_markdown("chart", "./missing.png");

    assert_eq!(fallback, "[chart 未能上传]");
    assert!(!fallback.contains("!["));
    assert!(!fallback.contains("./missing.png"));

    let fallback = local_image_fallback_markdown(" ", "/tmp/chart.png");
    assert_eq!(fallback, "[图片 未能上传]");
}

#[test]
pub(super) fn local_markdown_image_candidate_filters_remote_and_existing_keys() {
    assert!(is_local_markdown_image_candidate("./chart.png"));
    assert!(is_local_markdown_image_candidate("/tmp/chart.png"));
    assert!(is_local_markdown_image_candidate("file:///tmp/chart.png"));
    assert!(!is_local_markdown_image_candidate(
        "https://example.com/chart.png"
    ));
    assert!(!is_local_markdown_image_candidate(
        "http://example.com/chart.png"
    ));
    assert!(!is_local_markdown_image_candidate("img_v3_chart"));
    assert!(!is_local_markdown_image_candidate(" "));
}

#[test]
pub(super) fn agent_reply_target_keeps_feishu_owner_boundary() {
    let mut provider = test_provider();
    provider.owner_open_id = Some("owner-ou".to_string());
    let event = ImEvent {
        event_id: "evt-1".to_string(),
        provider_id: provider.id.clone(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("chat-1".to_string()),
            user_id: Some("sender-ou".to_string()),
            message_id: Some("msg-1".to_string()),
        },
        message: None,
        received_at: 0,
        raw_digest: None,
    };

    assert_eq!(
        agent_reply_target_id(&provider, &event).as_deref(),
        Some("owner-ou")
    );
}

#[test]
pub(super) fn start_notice_is_plain_weixin_only_without_progress_card() {
    let mut provider = test_provider();
    provider.provider_type = ImProviderType::Weixin;

    assert!(should_send_plain_im_task_start_notice(&provider, false));
    assert!(!should_send_plain_im_task_start_notice(&provider, true));

    provider.provider_type = ImProviderType::Feishu;
    assert!(!should_send_plain_im_task_start_notice(&provider, false));
}

pub(super) struct TestChatCompletionMock {
    port: u16,
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl TestChatCompletionMock {
    pub(super) async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock chat server");
        let port = listener.local_addr().expect("mock local addr").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_server = Arc::clone(&requests);

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let io = TokioIo::new(stream);
                let requests = Arc::clone(&requests_for_server);
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| {
                        let requests = Arc::clone(&requests);
                        async move {
                            let body_bytes = req
                                .into_body()
                                .collect()
                                .await
                                .map(|body| body.to_bytes())
                                .unwrap_or_else(|_| Bytes::new());
                            let body: serde_json::Value =
                                serde_json::from_slice(&body_bytes).unwrap_or_default();
                            requests.lock().expect("requests lock").push(body);
                            let response = serde_json::json!({
                                "choices": [{
                                    "message": {
                                        "role": "assistant",
                                        "content": "IM_PROVIDER_CONFIG_OK"
                                    },
                                    "finish_reason": "stop"
                                }],
                                "usage": {
                                    "prompt_tokens": 10,
                                    "completion_tokens": 4,
                                    "total_tokens": 14
                                }
                            });
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(StatusCode::OK)
                                    .header("Content-Type", "application/json")
                                    .body(Full::new(Bytes::from(response.to_string())))
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        Self { port, requests }
    }

    pub(super) fn url(&self) -> String {
        format!("http://127.0.0.1:{}/chat/completions", self.port)
    }
}

pub(super) fn request_messages_contain(body: &serde_json::Value, needle: &str) -> bool {
    body.get("messages")
        .and_then(|messages| messages.as_array())
        .map(|messages| {
            messages.iter().any(|message| {
                let Some(content) = message.get("content") else {
                    return false;
                };
                if let Some(text) = content.as_str() {
                    return text.contains(needle);
                }
                content
                    .as_array()
                    .map(|parts| {
                        parts.iter().any(|part| {
                            part.get("text")
                                .and_then(|value| value.as_str())
                                .map(|text| text.contains(needle))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

pub(super) fn request_contains_image_url(body: &serde_json::Value) -> bool {
    request_image_url_count(body) > 0
}

pub(super) fn request_image_url_count(body: &serde_json::Value) -> usize {
    body.get("messages")
        .and_then(|messages| messages.as_array())
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    message
                        .get("content")
                        .and_then(|content| content.as_array())
                        .map(|parts| {
                            parts
                                .iter()
                                .filter(|part| {
                                    part.get("type").and_then(|value| value.as_str())
                                        == Some("image_url")
                                        && part
                                            .pointer("/image_url/url")
                                            .and_then(|value| value.as_str())
                                            .is_some_and(|url| {
                                                url.starts_with("data:image/png;base64,")
                                            })
                                })
                                .count()
                        })
                        .unwrap_or(0)
                })
                .sum()
        })
        .unwrap_or(0)
}

pub(super) fn request_message_role(body: &serde_json::Value, idx: usize) -> Option<&str> {
    body.get("messages")?
        .as_array()?
        .get(idx)?
        .get("role")?
        .as_str()
}

pub(super) fn test_provider() -> ImProviderConfig {
    ImProviderConfig {
        id: "feishu-main".to_string(),
        provider_type: ImProviderType::Feishu,
        display_name: "Feishu Main".to_string(),
        enabled: true,
        base_url: None,
        app_id: Some("cli_xxx".to_string()),
        secret_ref: None,
        owner_open_id: None,
        event_connection_enabled: true,
        event_types: Vec::new(),
        agent_config: None,
        created_at: 0,
        updated_at: 0,
    }
}

#[test]
pub(super) fn send_message_request_resolves_owner_target_from_provider() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let service = ImGatewayService::new(temp_dir.path());
    let mut provider = test_provider();
    provider.owner_open_id = Some("ou-owner".to_string());
    service
        .provider_store
        .add(provider)
        .expect("provider should be saved");

    let body = SendMessageRequest {
        provider_id: Some("feishu-main".to_string()),
        target_id: Some("__owner__".to_string()),
        msg_type: "text".to_string(),
        content: serde_json::json!("hello"),
        text: None,
        card: None,
        image: None,
        rich_card: None,
    };

    let resolved =
        resolve_send_message_request(&service, &body).expect("owner target should resolve");

    assert_eq!(resolved.provider.id, "feishu-main");
    assert_eq!(resolved.target.provider_id, "feishu-main");
    assert_eq!(resolved.target.receive_id_type, "open_id");
    assert_eq!(resolved.target.receive_id, "ou-owner");
    assert_eq!(resolved.log_target_id, "__owner__");
    assert_eq!(resolved.log_target_name, "Owner");
    assert_eq!(resolved.content, serde_json::json!("hello"));
}

#[test]
pub(super) fn send_message_request_rejects_owner_without_provider() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let service = ImGatewayService::new(temp_dir.path());
    let body = SendMessageRequest {
        provider_id: None,
        target_id: Some("__owner__".to_string()),
        msg_type: "text".to_string(),
        content: serde_json::json!("hello"),
        text: None,
        card: None,
        image: None,
        rich_card: None,
    };

    let error = resolve_send_message_request(&service, &body)
        .expect_err("owner send should require provider");

    assert_eq!(error.0, StatusCode::BAD_REQUEST);
    assert!(error.1.contains("provider_id is required"));
}

#[test]
pub(super) fn send_message_request_accepts_image_key_payload() {
    let body = SendMessageRequest {
        provider_id: Some("feishu-main".to_string()),
        target_id: Some("__owner__".to_string()),
        msg_type: "image".to_string(),
        content: serde_json::Value::Null,
        text: None,
        card: None,
        image: Some(SendImageRequest {
            image_key: Some("img_v3_key".to_string()),
            data_base64: None,
            file_name: None,
            mime_type: None,
            image_type: default_feishu_image_type(),
        }),
        rich_card: None,
    };

    let content = normalized_send_content(&body).expect("image content");
    assert_eq!(content["image_key"], "img_v3_key");
    assert_eq!(content["image_type"], "message");
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn rich_card_builder_uses_image_key_and_markdown() {
    let provider = test_provider();
    let feishu = crate::im_gateway::feishu::FeishuProvider::new();
    let rich_card = SendRichCardRequest {
        title: Some("Deploy report".to_string()),
        text: Some("**Done** with chart".to_string()),
        image_key: Some("img_v3_chart".to_string()),
        image: None,
        image_alt: Some("Chart".to_string()),
    };

    let card = build_rich_card_content(&feishu, &provider, &rich_card)
        .await
        .expect("rich card");

    assert_eq!(card["header"]["title"]["content"], "Deploy report");
    assert_eq!(card["elements"][0]["tag"], "img");
    assert_eq!(card["elements"][0]["img_key"], "img_v3_chart");
    assert_eq!(card["elements"][1]["tag"], "markdown");
    assert_eq!(card["elements"][1]["content"], "**Done** with chart");
}

#[test]
pub(super) fn provider_agent_config_patch_sets_and_clears_overrides() {
    let mut provider = test_provider();

    apply_provider_patch(
        &mut provider,
        &serde_json::json!({
            "agent_config": {
                "work_dir": " /tmp/bifrost-im ",
                "base_instructions": " Provider prompt "
            }
        }),
    );

    let agent_config = provider.agent_config.as_ref().expect("agent_config");
    assert_eq!(agent_config.work_dir.as_deref(), Some("/tmp/bifrost-im"));
    assert_eq!(
        agent_config.base_instructions.as_deref(),
        Some("Provider prompt")
    );

    apply_provider_patch(
        &mut provider,
        &serde_json::json!({
            "agent_config": {
                "work_dir": null,
                "base_instructions": ""
            }
        }),
    );

    assert!(provider.agent_config.is_none());
}

#[test]
pub(super) fn provider_create_payload_maps_app_secret_without_exposing_it() {
    let provider = parse_provider_create_payload(serde_json::json!({
        "id": "feishu-main",
        "provider_type": "feishu",
        "display_name": "Feishu Main",
        "enabled": true,
        "app_id": "cli_xxx",
        "app_secret": "sk_test_secret",
        "event_connection_enabled": true,
        "event_types": []
    }))
    .expect("provider create payload should parse");

    assert_eq!(provider.secret_ref.as_deref(), Some("sk_test_secret"));
    assert_eq!(provider.display_name, "Feishu Main");

    let safe = sanitize_provider(&provider);
    assert_eq!(
        safe.get("secret_configured").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert!(safe.get("secret_ref").is_none());
    assert!(safe.get("app_secret").is_none());
    assert!(!safe.to_string().contains("sk_test_secret"));
}

#[test]
pub(super) fn provider_create_payload_defaults_missing_display_name_to_id() {
    let provider = parse_provider_create_payload(serde_json::json!({
        "id": "feishu-main",
        "provider_type": "feishu",
        "enabled": true,
        "app_id": "cli_xxx",
        "app_secret": "sk_test_secret",
        "event_connection_enabled": true,
        "event_types": []
    }))
    .expect("provider create payload should default display_name");

    assert_eq!(provider.display_name, "feishu-main");
    assert_eq!(provider.secret_ref.as_deref(), Some("sk_test_secret"));
}

#[test]
pub(super) fn provider_agent_config_overrides_base_agent_config() {
    let base = crate::im_gateway::agent::ImAgentConfig {
        work_dir: Some("/global".to_string()),
        base_instructions: Some("global prompt".to_string()),
        developer_instructions: Some("global developer".to_string()),
        user_instructions: Some("global user".to_string()),
        ..Default::default()
    };

    let mut provider = test_provider();
    provider.agent_config = Some(ImProviderAgentConfig {
        work_dir: Some("/provider".to_string()),
        base_instructions: Some("provider prompt".to_string()),
        developer_instructions: Some("provider developer".to_string()),
        user_instructions: Some("provider user".to_string()),
    });

    let effective = effective_agent_config_for_provider(&base, &provider);
    assert_eq!(effective.work_dir.as_deref(), Some("/provider"));
    assert_eq!(
        effective.base_instructions.as_deref(),
        Some("provider prompt")
    );
    assert_eq!(
        effective.developer_instructions.as_deref(),
        Some("provider developer")
    );
    assert_eq!(
        effective.user_instructions.as_deref(),
        Some("provider user")
    );
}

#[test]
pub(super) fn provider_switch_workdir_persists_provider_agent_override() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let store = Arc::new(ImProviderStore::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.id = "persist-workdir-provider".to_string();
    provider.agent_config = Some(ImProviderAgentConfig {
        work_dir: Some("/old".to_string()),
        base_instructions: Some("keep provider prompt".to_string()),
        developer_instructions: None,
        user_instructions: None,
    });
    store.add(provider).expect("add provider");

    persist_provider_agent_work_dir(&store, "persist-workdir-provider", " /new/workdir ");

    let updated = store.get("persist-workdir-provider").expect("provider");
    let agent_config = updated.agent_config.expect("agent_config");
    assert_eq!(agent_config.work_dir.as_deref(), Some("/new/workdir"));
    assert_eq!(
        agent_config.base_instructions.as_deref(),
        Some("keep provider prompt")
    );
}

#[test]
pub(super) fn agent_api_status_detail_applies_work_dir_for_fresh_status_session() {
    let manager = bifrost_agent::AgentSessionManager::new(3600);

    let detail = resolve_agent_api_status_detail(
        &manager,
        "status-fresh-workdir",
        Some("/tmp/bifrost-status-workdir".to_string()),
    )
    .expect("requested work_dir should create status detail");

    assert_eq!(
        detail.work_dir.as_deref(),
        Some("/tmp/bifrost-status-workdir")
    );
    assert_eq!(detail.message_count, 0);
}

#[test]
pub(super) fn agent_api_status_detail_overrides_existing_idle_session_work_dir() {
    let manager = bifrost_agent::AgentSessionManager::new(3600);
    let session = manager
        .try_take_session_with_work_dir("status-existing-workdir", Some("/tmp/old".to_string()))
        .expect("initial session should be available");
    manager.return_session(session);

    let detail = resolve_agent_api_status_detail(
        &manager,
        "status-existing-workdir",
        Some("/tmp/new".to_string()),
    )
    .expect("existing status detail should remain available");

    assert_eq!(detail.work_dir.as_deref(), Some("/tmp/new"));
}

#[test]
pub(super) fn agent_api_status_detail_keeps_new_session_text_when_no_work_dir_requested() {
    let manager = bifrost_agent::AgentSessionManager::new(3600);

    let detail = resolve_agent_api_status_detail(&manager, "status-no-workdir", None);

    assert!(detail.is_none());
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn im_event_loop_uses_provider_agent_config_for_agent_chat() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let mock = TestChatCompletionMock::start().await;
    let service = ImGatewayService::new(temp_dir.path());

    let mut base_config = service.agent_config_store.load();
    base_config.enabled = true;
    base_config.model = Some("mock-model".to_string());
    base_config.model_provider = Some("mock".to_string());
    base_config.work_dir = Some(std::env::current_dir().unwrap().display().to_string());
    base_config.base_instructions = Some("GLOBAL_BASE_SHOULD_NOT_APPEAR".to_string());
    base_config.developer_instructions = Some("GLOBAL_DEV_SHOULD_NOT_APPEAR".to_string());
    base_config.user_instructions = Some("GLOBAL_USER_SHOULD_NOT_APPEAR".to_string());
    base_config.max_turn_iterations = Some(1);
    base_config.model_providers.insert(
        "mock".to_string(),
        bifrost_agent::config::ModelProviderConfig {
            name: Some("Mock".to_string()),
            base_url: Some(mock.url()),
            env_key: None,
            api_key: None,
            http_headers: Some(HashMap::from([(
                "Authorization".to_string(),
                "Bearer test".to_string(),
            )])),
            env_http_headers: None,
            request_max_retries: None,
            stream_idle_timeout_ms: None,
            stream_max_retries: None,
        },
    );
    service
        .agent_config_store
        .save(&base_config)
        .expect("save base agent config");

    let mut provider = test_provider();
    provider.id = "new-im-provider-config".to_string();
    provider.owner_open_id = Some("owner-open-id".to_string());
    provider.base_url = Some("http://127.0.0.1:9".to_string());
    let mut provider_in_store = provider.clone();
    provider_in_store.agent_config = Some(ImProviderAgentConfig {
        work_dir: Some(std::env::current_dir().unwrap().display().to_string()),
        base_instructions: Some("IM_PROVIDER_BASE_OK: answer IM_PROVIDER_CONFIG_OK".to_string()),
        developer_instructions: Some("IM_PROVIDER_DEV_OK".to_string()),
        user_instructions: Some("IM_PROVIDER_USER_OK".to_string()),
    });
    service
        .provider_store
        .add(provider_in_store)
        .expect("add current provider config to store");

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_event_loop(
        rx,
        ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider())),
        provider.clone(),
        Arc::clone(&service.event_store),
        Arc::clone(&service.message_log_store),
        Arc::clone(&service.route_store),
        Arc::clone(&service.provider_store),
        Arc::clone(&service.agent_config_store),
        Arc::clone(&service.agent_client),
        Arc::clone(&service.agent_tools),
        Arc::clone(&service.schedule_store),
        Arc::clone(&service.scheduler),
        Arc::clone(&service.target_store),
        Arc::clone(&service.connection_manager),
        Arc::clone(&service.agent_session_manager),
        Arc::clone(&service.queue_manager),
        Arc::clone(&service.progress_registry),
    ));

    tx.send(ImEvent {
        event_id: "evt-im-provider-agent-config".to_string(),
        provider_id: provider.id.clone(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("chat-id".to_string()),
            user_id: Some("owner-open-id".to_string()),
            message_id: None,
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: "IM_PROVIDER_CHAT_MARKER 请只回复 IM_PROVIDER_CONFIG_OK".to_string(),
            mentions: Vec::new(),
            images: Vec::new(),
            raw_type: Some("text".to_string()),
        }),
        received_at: now_ms(),
        raw_digest: None,
    })
    .expect("send IM event");
    drop(tx);

    tokio::time::timeout(std::time::Duration::from_secs(10), handle)
        .await
        .expect("event loop timed out")
        .expect("event loop task panicked");

    let requests = mock.requests.lock().expect("requests lock");
    let request = requests.first().expect("mock received chat request");
    assert_eq!(request_message_role(request, 0), Some("system"));
    assert_eq!(request_message_role(request, 1), Some("developer"));
    assert_eq!(request_message_role(request, 2), Some("user"));
    assert!(request_messages_contain(request, "IM_PROVIDER_BASE_OK"));
    assert!(request_messages_contain(request, "IM_PROVIDER_DEV_OK"));
    assert!(request_messages_contain(request, "IM_PROVIDER_USER_OK"));
    assert!(request_messages_contain(request, "IM_PROVIDER_CHAT_MARKER"));
    assert!(!request_messages_contain(
        request,
        "GLOBAL_BASE_SHOULD_NOT_APPEAR"
    ));
    assert!(!request_messages_contain(
        request,
        "GLOBAL_DEV_SHOULD_NOT_APPEAR"
    ));
    assert!(!request_messages_contain(
        request,
        "GLOBAL_USER_SHOULD_NOT_APPEAR"
    ));
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn im_event_loop_forwards_image_attachment_to_agent_chat() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let mock = TestChatCompletionMock::start().await;
    let service = ImGatewayService::new(temp_dir.path());

    let mut base_config = service.agent_config_store.load();
    base_config.enabled = true;
    base_config.model = Some("mock-vision-model".to_string());
    base_config.model_provider = Some("mock".to_string());
    base_config.work_dir = Some(std::env::current_dir().unwrap().display().to_string());
    base_config.max_turn_iterations = Some(1);
    base_config.model_providers.insert(
        "mock".to_string(),
        bifrost_agent::config::ModelProviderConfig {
            name: Some("Mock".to_string()),
            base_url: Some(mock.url()),
            env_key: None,
            api_key: None,
            http_headers: Some(HashMap::from([(
                "Authorization".to_string(),
                "Bearer test".to_string(),
            )])),
            env_http_headers: None,
            request_max_retries: None,
            stream_idle_timeout_ms: None,
            stream_max_retries: None,
        },
    );
    service
        .agent_config_store
        .save(&base_config)
        .expect("save base agent config");

    let mut provider = test_provider();
    provider.id = "image-provider".to_string();
    provider.owner_open_id = Some("owner-open-id".to_string());
    provider.base_url = Some("http://127.0.0.1:9".to_string());
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(run_event_loop(
        rx,
        ImProviderClient::Feishu(Arc::clone(service.connection_manager.feishu_provider())),
        provider.clone(),
        Arc::clone(&service.event_store),
        Arc::clone(&service.message_log_store),
        Arc::clone(&service.route_store),
        Arc::clone(&service.provider_store),
        Arc::clone(&service.agent_config_store),
        Arc::clone(&service.agent_client),
        Arc::clone(&service.agent_tools),
        Arc::clone(&service.schedule_store),
        Arc::clone(&service.scheduler),
        Arc::clone(&service.target_store),
        Arc::clone(&service.connection_manager),
        Arc::clone(&service.agent_session_manager),
        Arc::clone(&service.queue_manager),
        Arc::clone(&service.progress_registry),
    ));

    tx.send(ImEvent {
        event_id: "evt-im-image-agent-chat".to_string(),
        provider_id: provider.id.clone(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("chat-id".to_string()),
            user_id: Some("owner-open-id".to_string()),
            message_id: Some("om-image".to_string()),
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: "".to_string(),
            mentions: Vec::new(),
            images: (0..7)
                .map(|idx| crate::im_gateway::types::ImImageAttachment {
                    file_key: format!("img-unit-{idx}"),
                    source: crate::im_gateway::types::ImImageSource::MessageResource,
                    mime_type: Some("image/png".to_string()),
                    data_base64: Some("iVBORw0KGgo=".to_string()),
                    download_url: None,
                    encrypted_query_param: None,
                    aes_key: None,
                })
                .collect(),
            raw_type: Some("image".to_string()),
        }),
        received_at: now_ms(),
        raw_digest: None,
    })
    .expect("send IM image event");
    drop(tx);

    tokio::time::timeout(std::time::Duration::from_secs(10), handle)
        .await
        .expect("event loop timed out")
        .expect("event loop task panicked");

    let requests = mock.requests.lock().expect("requests lock");
    let request = requests.first().expect("mock received chat request");
    assert!(request_messages_contain(request, IMAGE_ONLY_AGENT_PROMPT));
    assert!(request_contains_image_url(request));
    assert_eq!(
        request_image_url_count(request),
        MAX_AGENT_IMAGES_PER_MESSAGE
    );
}
