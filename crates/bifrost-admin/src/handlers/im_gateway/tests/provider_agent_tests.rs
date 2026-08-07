use super::*;

async fn spawn_new_group_feishu_server() -> (
    String,
    Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake Feishu server");
    let address = listener.local_addr().expect("fake Feishu address");
    let creates = Arc::new(AtomicUsize::new(0));
    let server_creates = Arc::clone(&creates);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let creates = Arc::clone(&server_creates);
            tokio::spawn(async move {
                let handler = service_fn(move |request: Request<Incoming>| {
                    let creates = Arc::clone(&creates);
                    async move {
                        let path = request.uri().path();
                        let body = if path.ends_with("/auth/v3/tenant_access_token/internal") {
                            r#"{"code":0,"tenant_access_token":"token","expire":7200}"#
                        } else if path.ends_with("/im/v1/chats") {
                            creates.fetch_add(1, Ordering::SeqCst);
                            r#"{"code":0,"data":{"chat_id":"oc_created","name":"发布群"}}"#
                        } else {
                            r#"{"code":0,"data":{"message_id":"om_reply"}}"#
                        };
                        Ok::<_, std::convert::Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from_static(body.as_bytes())))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), handler)
                    .await;
            });
        }
    });
    (format!("http://{address}/open-apis"), creates, task)
}

async fn spawn_new_group_feishu_response_server(
    create_body: &'static str,
    message_body: &'static str,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake Feishu response server");
    let address = listener.local_addr().expect("fake Feishu address");
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let handler = service_fn(move |request: Request<Incoming>| async move {
                    let path = request.uri().path();
                    let body = if path.ends_with("/auth/v3/tenant_access_token/internal") {
                        r#"{"code":0,"tenant_access_token":"token","expire":7200}"#
                    } else if path.ends_with("/im/v1/chats") {
                        create_body
                    } else {
                        message_body
                    };
                    Ok::<_, std::convert::Infallible>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "application/json")
                            .body(Full::new(Bytes::from_static(body.as_bytes())))
                            .unwrap(),
                    )
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), handler)
                    .await;
            });
        }
    });
    (format!("http://{address}/open-apis"), task)
}

fn new_group_event(
    provider: &ImProviderConfig,
    user_id: Option<&str>,
    message_id: &str,
) -> ImEvent {
    ImEvent {
        event_id: format!("event-{message_id}"),
        provider_id: provider.id.clone(),
        provider_type: provider.provider_type,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("oc_source".to_string()),
            user_id: user_id.map(str::to_string),
            message_id: Some(message_id.to_string()),
            ..Default::default()
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: "/new 发布群".to_string(),
            ..Default::default()
        }),
        received_at: now_ms(),
        raw_digest: None,
    }
}

#[tokio::test]
pub(super) async fn im_new_group_handler_enforces_owner_and_persists_success_for_replay() {
    use std::sync::atomic::Ordering;
    let temp = tempfile::tempdir().expect("temp data dir");
    let service = ImGatewayService::new(temp.path());
    let (base_url, creates, server) = spawn_new_group_feishu_server().await;
    let mut provider = test_provider();
    provider.owner_open_id = Some("ou_owner".to_string());
    provider.base_url = Some(base_url);
    provider.secret_ref = Some("secret".to_string());
    let client =
        ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
    let owner_event = new_group_event(&provider, Some("ou_owner"), "om-new");

    assert!(
        handle_im_new_group_command(
            "/new 发布群",
            &client,
            &provider,
            &owner_event,
            &service.group_context_store,
            &service.message_log_store,
        )
        .await
    );
    assert!(
        handle_im_new_group_command(
            "/new 发布群",
            &client,
            &provider,
            &owner_event,
            &service.group_context_store,
            &service.message_log_store,
        )
        .await
    );
    assert_eq!(creates.load(Ordering::SeqCst), 1);
    assert!(service.message_log_store.list().iter().any(|log| log
        .content
        .as_deref()
        .is_some_and(|text| text.contains("未重复创建"))));

    let denied = new_group_event(&provider, Some("ou_other"), "om-denied");
    assert!(
        handle_im_new_group_command(
            "/new 越权群",
            &client,
            &provider,
            &denied,
            &service.group_context_store,
            &service.message_log_store,
        )
        .await
    );
    let missing_sender = new_group_event(&provider, None, "om-missing-sender");
    assert!(
        handle_im_new_group_command(
            "/new 缺发送者",
            &client,
            &provider,
            &missing_sender,
            &service.group_context_store,
            &service.message_log_store,
        )
        .await
    );
    let mut no_owner = provider.clone();
    no_owner.owner_open_id = None;
    assert!(
        handle_im_new_group_command(
            "/new 无 owner",
            &client,
            &no_owner,
            &owner_event,
            &service.group_context_store,
            &service.message_log_store,
        )
        .await
    );
    assert!(
        handle_im_new_group_command(
            "/new",
            &client,
            &provider,
            &owner_event,
            &service.group_context_store,
            &service.message_log_store,
        )
        .await
    );
    let mut weixin = provider.clone();
    weixin.provider_type = ImProviderType::Weixin;
    assert!(
        !handle_im_new_group_command(
            "/new ignored",
            &client,
            &weixin,
            &owner_event,
            &service.group_context_store,
            &service.message_log_store,
        )
        .await
    );
    server.abort();
}

#[tokio::test]
pub(super) async fn im_new_group_handler_reports_storage_api_and_welcome_failures() {
    async fn run_case(
        create_body: &'static str,
        message_body: &'static str,
        mutate_store: impl FnOnce(&ImGroupContextStore),
        expected: &str,
        blank_message_id: bool,
    ) {
        let temp = tempfile::tempdir().expect("temp data dir");
        let service = ImGatewayService::new(temp.path());
        mutate_store(&service.group_context_store);
        let (base_url, server) =
            spawn_new_group_feishu_response_server(create_body, message_body).await;
        let mut provider = test_provider();
        provider.owner_open_id = Some("ou_owner".to_string());
        provider.base_url = Some(base_url);
        provider.secret_ref = Some("secret".to_string());
        let client =
            ImProviderClient::Feishu(Arc::new(crate::im_gateway::feishu::FeishuProvider::new()));
        let mut event = new_group_event(&provider, Some("ou_owner"), "om-error");
        if blank_message_id {
            event.source.message_id = Some(" ".to_string());
        }

        assert!(
            handle_im_new_group_command(
                "/new 失败路径群",
                &client,
                &provider,
                &event,
                &service.group_context_store,
                &service.message_log_store,
            )
            .await
        );
        assert!(
            service.message_log_store.list().iter().any(|log| log
                .content
                .as_deref()
                .is_some_and(|text| text.contains(expected))),
            "missing expected reply fragment: {expected}"
        );
        server.abort();
    }

    run_case(
        r#"{"code":0,"data":{"chat_id":"oc_created","name":"失败路径群"}}"#,
        r#"{"code":0,"data":{"message_id":"om_reply"}}"#,
        |store| {
            let connection = rusqlite::Connection::open(store.file_path()).unwrap();
            connection
                .execute("DROP TABLE im_feishu_new_groups", [])
                .unwrap();
        },
        "读取建群幂等记录失败",
        false,
    )
    .await;

    run_case(
        r#"{"code":999,"msg":"forbidden"}"#,
        r#"{"code":0,"data":{"message_id":"om_reply"}}"#,
        |_| {},
        "创建飞书群失败",
        true,
    )
    .await;

    run_case(
        r#"{"code":0,"data":{"chat_id":"oc_created","name":"失败路径群"}}"#,
        r#"{"code":0,"data":{"message_id":"om_reply"}}"#,
        |store| {
            let connection = rusqlite::Connection::open(store.file_path()).unwrap();
            connection
                .execute_batch(
                    "CREATE TRIGGER reject_new_group_insert
                     BEFORE INSERT ON im_feishu_new_groups
                     BEGIN SELECT RAISE(ABORT, 'reject insert'); END;",
                )
                .unwrap();
        },
        "保存幂等记录失败",
        false,
    )
    .await;

    run_case(
        r#"{"code":0,"data":{"chat_id":"oc_created","name":"失败路径群"}}"#,
        r#"{"code":999,"msg":"send denied"}"#,
        |_| {},
        "欢迎消息发送失败",
        false,
    )
    .await;

    let weixin = ImProviderClient::Weixin(Arc::new(WeixinProvider::new()));
    let error = weixin
        .create_feishu_group_chat(&test_provider(), "群", "ou_owner", "uuid")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("only supported by Feishu"));
}

#[test]
pub(super) fn provider_agent_config_patch_sets_and_clears_overrides() {
    let mut provider = test_provider();

    apply_provider_patch(
        &mut provider,
        &serde_json::json!({
            "agent_config": {
                "runner": "codex",
                "work_dir": " /tmp/bifrost-im ",
                "base_instructions": " Provider prompt "
            }
        }),
    );

    let agent_config = provider.agent_config.as_ref().expect("agent_config");
    assert_eq!(
        agent_config.runner,
        Some(bifrost_agent::AgentRunnerMode::Custom("codex".to_string()))
    );
    assert_eq!(agent_config.work_dir.as_deref(), Some("/tmp/bifrost-im"));
    assert_eq!(
        agent_config.base_instructions.as_deref(),
        Some("Provider prompt")
    );

    apply_provider_patch(
        &mut provider,
        &serde_json::json!({
            "agent_config": {
                "runner": null,
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
pub(super) fn provider_create_payload_forces_feishu_base_url() {
    let provider = parse_provider_create_payload(serde_json::json!({
        "id": "feishu-main",
        "provider_type": "feishu",
        "display_name": "Feishu Main",
        "enabled": true,
        "app_id": "cli_xxx",
        "app_secret": "sk_test_secret",
        "base_url": "https://evil.example/open-apis",
        "event_connection_enabled": true,
        "event_types": []
    }))
    .expect("provider create payload should parse");

    assert_eq!(
        provider.base_url.as_deref(),
        Some("https://open.feishu.cn/open-apis")
    );
}

#[test]
pub(super) fn feishu_setup_pending_sessions_survive_service_restart() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let service = ImGatewayService::new(temp_dir.path());
    service.feishu_setup_pending.write().insert(
        "fas_restore".to_string(),
        PendingFeishuSetup {
            device_code: "device-code".to_string(),
            interval_seconds: 5,
            expires_at_ms: now_ms() + 60_000,
            app_id: Some("cli_restored".to_string()),
            app_secret: Some("secret".to_string()),
            owner_open_id: Some("ou_owner".to_string()),
            brand: FeishuSetupBrand::Feishu,
            provider_payload: Some(serde_json::json!({
                "id": "feishu-main",
                "provider_type": "feishu",
                "display_name": "Feishu Main",
                "enabled": true,
                "event_connection_enabled": true,
                "event_types": ["message.receive"],
                "agent_config": {
                    "runner": "traex"
                }
            })),
            created_provider_id: None,
            auto_connect: false,
        },
    );
    save_pending_feishu_setups(&service);

    let restored = ImGatewayService::new(temp_dir.path());
    let pending = restored
        .feishu_setup_pending
        .read()
        .get("fas_restore")
        .cloned()
        .expect("pending setup should be restored");

    assert_eq!(pending.device_code, "device-code");
    assert_eq!(pending.app_id.as_deref(), Some("cli_restored"));
    assert_eq!(pending.app_secret.as_deref(), Some("secret"));
    assert_eq!(pending.owner_open_id.as_deref(), Some("ou_owner"));
    assert_eq!(pending.brand, FeishuSetupBrand::Feishu);
    assert!(pending.provider_payload.is_some());
    assert!(pending.created_provider_id.is_none());
}

#[tokio::test]
pub(super) async fn feishu_setup_confirmed_session_creates_provider_from_draft() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let service = ImGatewayService::new(temp_dir.path());
    service.feishu_setup_pending.write().insert(
        "fas_confirmed".to_string(),
        PendingFeishuSetup {
            device_code: "device-code".to_string(),
            interval_seconds: 5,
            expires_at_ms: now_ms() + 60_000,
            app_id: Some("cli_confirmed".to_string()),
            app_secret: Some("secret".to_string()),
            owner_open_id: Some("ou_owner".to_string()),
            brand: FeishuSetupBrand::Feishu,
            provider_payload: Some(serde_json::json!({
                "id": "feishu-main",
                "provider_type": "feishu",
                "display_name": "Feishu Main",
                "enabled": false,
                "base_url": "https://evil.example/open-apis",
                "event_connection_enabled": false,
                "event_types": [],
                "agent_config": {
                    "runner": "traex"
                }
            })),
            created_provider_id: None,
            auto_connect: false,
        },
    );

    poll_and_complete_feishu_setup_session(&service, "fas_confirmed")
        .await
        .expect("confirmed setup should create provider from draft");

    let provider = service
        .provider_store
        .get("feishu-main")
        .expect("provider should be created");
    assert_eq!(provider.app_id.as_deref(), Some("cli_confirmed"));
    assert_eq!(provider.secret_ref.as_deref(), Some("secret"));
    assert_eq!(provider.owner_open_id.as_deref(), Some("ou_owner"));
    assert!(provider.enabled);
    assert!(provider.event_connection_enabled);
    assert_eq!(provider.event_types, vec!["message.receive"]);
    assert_eq!(
        provider.base_url.as_deref(),
        Some("https://open.feishu.cn/open-apis")
    );
    assert_eq!(
        provider
            .agent_config
            .as_ref()
            .and_then(|config| config.runner.as_ref()),
        Some(&bifrost_agent::AgentRunnerMode::Custom("traex".to_string()))
    );

    let pending = service
        .feishu_setup_pending
        .read()
        .get("fas_confirmed")
        .cloned()
        .expect("pending setup remains available for CLI status");
    assert_eq!(pending.created_provider_id.as_deref(), Some("feishu-main"));
}

#[test]
pub(super) fn provider_create_payload_preserves_lark_fixed_base_url() {
    let provider = parse_provider_create_payload(serde_json::json!({
        "id": "lark-main",
        "provider_type": "feishu",
        "display_name": "Lark Main",
        "enabled": true,
        "app_id": "cli_xxx",
        "app_secret": "sk_test_secret",
        "base_url": "https://open.larksuite.com",
        "event_connection_enabled": true,
        "event_types": []
    }))
    .expect("provider create payload should parse");

    assert_eq!(
        provider.base_url.as_deref(),
        Some("https://open.larksuite.com/open-apis")
    );
}

#[test]
pub(super) fn provider_patch_forces_feishu_base_url() {
    let mut provider = parse_provider_create_payload(serde_json::json!({
        "id": "feishu-main",
        "provider_type": "feishu",
        "display_name": "Feishu Main",
        "enabled": true,
        "app_id": "cli_xxx",
        "app_secret": "sk_test_secret",
        "base_url": "https://open.feishu.cn/open-apis",
        "event_connection_enabled": true,
        "event_types": []
    }))
    .expect("provider create payload should parse");

    apply_provider_patch(
        &mut provider,
        &serde_json::json!({
            "base_url": "https://evil.example/open-apis"
        }),
    );

    assert_eq!(
        provider.base_url.as_deref(),
        Some("https://open.feishu.cn/open-apis")
    );
}

#[test]
pub(super) fn provider_store_normalizes_legacy_feishu_base_url_on_read() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let store = Arc::new(ImProviderStore::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.id = "legacy-feishu-provider".to_string();
    provider.base_url = Some("https://evil.example/open-apis".to_string());
    store.add(provider).expect("add provider");

    let loaded = store.get("legacy-feishu-provider").expect("provider");

    assert_eq!(
        loaded.base_url.as_deref(),
        Some("https://open.feishu.cn/open-apis")
    );
    let raw = std::fs::read_to_string(
        temp_dir
            .path()
            .join("admin")
            .join("im_gateway_providers.json"),
    )
    .expect("read provider store");
    assert!(!raw.contains("https://evil.example/open-apis"));
    assert!(raw.contains("https://open.feishu.cn/open-apis"));
}

#[test]
pub(super) fn provider_create_payload_forces_weixin_base_url() {
    let provider = parse_provider_create_payload(serde_json::json!({
        "id": "weixin-main",
        "provider_type": "weixin",
        "display_name": "Weixin Main",
        "enabled": true,
        "base_url": "https://evil.example",
        "event_connection_enabled": true,
        "event_types": []
    }))
    .expect("provider create payload should parse");

    assert_eq!(
        provider.base_url.as_deref(),
        Some("https://ilinkai.weixin.qq.com")
    );
}

#[test]
pub(super) fn feishu_setup_brand_selects_expected_domains() {
    assert_eq!(parse_feishu_setup_brand(None), FeishuSetupBrand::Feishu);
    assert_eq!(
        parse_feishu_setup_brand(Some("lark")),
        FeishuSetupBrand::Lark
    );
    assert_eq!(
        FeishuSetupBrand::Feishu.open_base(),
        "https://open.feishu.cn"
    );
    assert_eq!(
        FeishuSetupBrand::Lark.provider_base_url(),
        "https://open.larksuite.com/open-apis"
    );
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
        runner: Some(bifrost_agent::AgentRunnerMode::Custom("codex".to_string())),
        work_dir: Some("/provider".to_string()),
        base_instructions: Some("provider prompt".to_string()),
        developer_instructions: Some("provider developer".to_string()),
        user_instructions: Some("provider user".to_string()),
    });

    let effective = effective_agent_config_for_provider(&base, &provider);
    assert_eq!(
        effective.runner,
        Some(bifrost_agent::AgentRunnerMode::Custom("codex".to_string()))
    );
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
pub(super) fn model_commands_resolve_the_group_selected_runner_first() {
    let temp = tempfile::tempdir().unwrap();
    let store = ImGroupContextStore::new(temp.path());
    let event = ImEvent {
        event_id: "group-runner-event".to_string(),
        provider_id: "feishu-main".to_string(),
        provider_type: ImProviderType::Feishu,
        event_type: "message.receive".to_string(),
        source: crate::im_gateway::types::ImEventSource {
            chat_id: Some("oc_group".to_string()),
            chat_type: Some("group".to_string()),
            user_id: Some("ou_user".to_string()),
            message_id: Some("group-runner-message".to_string()),
            ..Default::default()
        },
        message: Some(crate::im_gateway::types::ImEventMessage {
            text: "hello".to_string(),
            ..Default::default()
        }),
        received_at: 1,
        raw_digest: None,
    };
    store.record_event(&event, "event").unwrap();
    let session_key =
        crate::im_gateway::group_context::build_group_session_key("feishu-main", "oc_group");
    store
        .set_runner_id_by_session(&session_key, "traex-group")
        .unwrap();
    let agent_config = crate::im_gateway::agent::ImAgentConfig {
        runner: Some(bifrost_agent::AgentRunnerMode::Custom(
            "codex-provider".to_string(),
        )),
        ..Default::default()
    };

    assert_eq!(
        configured_runner_id_for_im_session(&store, &session_key, &agent_config).as_deref(),
        Some("traex-group")
    );
    assert_eq!(
        configured_runner_id_for_im_session(&store, "missing-session", &agent_config).as_deref(),
        Some("codex-provider")
    );
    assert!(configured_runner_id_for_im_session(
        &store,
        "missing-session",
        &crate::im_gateway::agent::ImAgentConfig {
            runner: None,
            ..Default::default()
        },
    )
    .is_none());
}

#[test]
pub(super) fn provider_agent_work_dir_resolves_global_default_directory() {
    let base = crate::im_gateway::agent::ImAgentConfig {
        work_dir: None,
        ..Default::default()
    };
    let provider = test_provider();

    let effective_work_dir =
        effective_agent_work_dir_for_provider(&base, &provider).expect("resolved work_dir");
    let current_dir = std::env::current_dir().expect("current dir");

    assert_eq!(effective_work_dir, current_dir);
}

#[test]
pub(super) fn agent_config_response_includes_resolved_work_dir() {
    let response = agent_config_response(crate::im_gateway::agent::ImAgentConfig {
        work_dir: None,
        ..Default::default()
    });
    let current_dir = std::env::current_dir()
        .expect("current dir")
        .display()
        .to_string();

    assert_eq!(
        response
            .get("resolved_work_dir")
            .and_then(|value| value.as_str()),
        Some(current_dir.as_str())
    );
    assert!(response.get("work_dir").is_none_or(|value| value.is_null()));
}

#[test]
pub(super) fn provider_switch_workdir_persists_provider_agent_override() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let store = Arc::new(ImProviderStore::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.id = "persist-workdir-provider".to_string();
    provider.agent_config = Some(ImProviderAgentConfig {
        runner: None,
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
pub(super) fn im_cwd_command_parses_existing_absolute_directory() {
    let work_dir = tempfile::tempdir().expect("work dir");
    let command = format!("/cwd \"{}\"", work_dir.path().display());

    let parsed = parse_im_cwd_command(&command)
        .expect("cwd command")
        .expect("valid cwd");

    assert_eq!(parsed, std::fs::canonicalize(work_dir.path()).unwrap());
}

#[test]
pub(super) fn im_cwd_command_rejects_invalid_paths() {
    assert!(parse_im_cwd_command("/cwdish /tmp").is_none());
    assert!(parse_im_cwd_command("请看 /cwd /tmp").is_none());
    assert_eq!(
        parse_im_cwd_command("/cwd")
            .expect("cwd command")
            .expect_err("missing path"),
        "用法: /cwd <绝对路径>"
    );
    assert!(parse_im_cwd_command("/cwd relative/path")
        .expect("cwd command")
        .expect_err("relative path")
        .contains("请使用绝对路径"));
    let missing_dir = std::env::temp_dir().join("definitely-not-exist-bifrost-cwd-test");
    let _ = std::fs::remove_dir_all(&missing_dir);
    assert!(
        parse_im_cwd_command(&format!("/cwd \"{}\"", missing_dir.display()))
            .expect("cwd command")
            .expect_err("missing path")
            .contains("路径不存在")
    );

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let file_path = temp_dir.path().join("file.txt");
    std::fs::write(&file_path, "not a directory").expect("write file");
    assert!(
        parse_im_cwd_command(&format!("/cwd {}", file_path.display()))
            .expect("cwd command")
            .expect_err("file path")
            .contains("不是目录")
    );
}

#[test]
pub(super) fn im_new_group_command_parses_name_and_rejects_invalid_forms() {
    assert_eq!(
        parse_im_new_group_command(" /new 发布 项目群 "),
        Some(Ok("发布 项目群".to_string()))
    );
    assert_eq!(
        parse_im_new_group_command("/new"),
        Some(Err("用法: /new <群名>".to_string()))
    );
    assert_eq!(
        parse_im_new_group_command("/new   "),
        Some(Err("用法: /new <群名>".to_string()))
    );
    assert_eq!(
        parse_im_new_group_command(&format!("/new {}", "群".repeat(61))),
        Some(Err("群名不能超过 60 个字符。".to_string()))
    );
    assert_eq!(
        parse_im_new_group_command(&format!("/new {}", "群".repeat(60))),
        Some(Ok("群".repeat(60)))
    );
    assert!(parse_im_new_group_command("/new-project").is_none());
    assert!(parse_im_new_group_command("请 /new 新群").is_none());
}

#[test]
pub(super) fn im_help_for_external_cli_runner_only_lists_supported_commands() {
    let help = build_im_startup_help_for_runner(
        &ImHelpRunnerKind::External {
            adapter: crate::im_gateway::external_cli::TRAEX_ADAPTER.to_string(),
        },
        ImProviderType::Feishu,
    );

    assert!(help.contains("IM 通道命令（所有 Runner）:"));
    assert!(help.contains("/help"));
    assert!(help.contains("/status"));
    assert!(help.contains("/cwd <绝对路径>"));
    assert!(help.contains("/runner [Runner]"));
    assert!(help.contains("/new <群名>"));
    assert!(help.contains("仅 Provider owner"));
    assert!(help.contains("/clear"));
    assert!(help.contains("/reset"));
    assert!(help.contains("/q <消息>"));
    assert!(help.contains("/rq <序号>"));
    assert!(help.contains("/stop"));
    assert!(help.contains("Traex Runner 命令:"));
    assert!(help.contains("/models"));
    assert!(help.contains("/model [模型]"));
    assert!(help.contains("/efforts"));
    assert!(help.contains("/effort [级别]"));
    assert!(!help.contains("/g <引导内容>"));
    assert!(help.contains("普通后续消息默认按引导处理，使用 /q 才排队"));
    assert!(!help.contains("/remember"));
    assert!(!help.contains("/memories"));
    assert!(!help.contains("/forget"));
    assert!(!help.contains("/goal"));
    assert!(!help.contains("/compact"));
}

#[test]
pub(super) fn im_help_for_codex_runner_lists_fast_command() {
    let help = build_im_startup_help_for_runner(
        &ImHelpRunnerKind::External {
            adapter: "codex".to_string(),
        },
        ImProviderType::Feishu,
    );

    assert!(help.contains("Codex Runner 命令:"));
    assert!(help.contains("/fast [on|off|status]"));
}

#[test]
pub(super) fn im_help_for_unsupported_external_runner_omits_unsupported_commands() {
    let help = build_im_startup_help_for_runner(
        &ImHelpRunnerKind::External {
            adapter: "chatgpt_web".to_string(),
        },
        ImProviderType::Weixin,
    );

    assert!(help.contains("IM 通道命令（所有 Runner）:"));
    assert!(!help.contains("/models"));
    assert!(!help.contains("/model [模型]"));
    assert!(!help.contains("/new <群名>"));
    assert!(!help.contains("/efforts"));
    assert!(!help.contains("/effort [级别]"));
    assert!(!help.contains("/fast [on|off|status]"));
    assert!(!help.contains("/remember"));
    assert!(!help.contains("/goal"));
    assert!(!help.contains("/g <引导内容>"));
}

#[test]
pub(super) fn im_runner_command_lists_configured_external_runners() {
    let mut config = crate::im_gateway::external_cli::ExternalCliGatewayConfig {
        default_runner_id: "Codex".to_string(),
        ..Default::default()
    };
    config.runners.insert(
        "Traex".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            adapter: "traex".to_string(),
            ..Default::default()
        },
    );

    assert_eq!(
        parse_im_runner_command("/runner"),
        Some(ImRunnerCommand::List)
    );
    assert_eq!(
        parse_im_runner_command("/Runner Traex"),
        Some(ImRunnerCommand::Switch("Traex".to_string()))
    );
    let legacy_alias = ["Tree", "X"].concat();
    let legacy_selection =
        resolve_im_runner_selection(&config, &legacy_alias).expect("legacy Traex alias");
    assert!(matches!(
        legacy_selection.runner,
        bifrost_agent::AgentRunnerMode::Custom(ref runner_id) if runner_id == "Traex"
    ));
    assert_eq!(parse_im_runner_command("/runnerish"), None);

    let runner_list = format_im_runner_list(&config);
    assert!(!runner_list.contains("Bifrost Agent"));
    assert!(runner_list.contains("Codex"));
    assert!(runner_list.contains("Traex"));
}

#[test]
pub(super) fn im_runner_command_rejects_unknown_runner() {
    let config = crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();

    let error = resolve_im_runner_selection(&config, "missing").expect_err("missing runner");

    assert!(error.contains("找不到 Runner"));
    assert!(error.contains("missing"));
    assert!(error.contains("Codex"));
}

#[test]
pub(super) fn im_cwd_command_persists_provider_and_reinitializes_idle_session() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let store = Arc::new(ImProviderStore::new(temp_dir.path()));
    let mut provider = test_provider();
    provider.id = "im-cwd-provider".to_string();
    provider.agent_config = Some(ImProviderAgentConfig {
        runner: None,
        work_dir: Some("/old/workdir".to_string()),
        base_instructions: Some("keep provider prompt".to_string()),
        developer_instructions: None,
        user_instructions: None,
    });
    store.add(provider).expect("add provider");

    let manager = Arc::new(bifrost_agent::AgentSessionManager::new(3600));
    let mut session = manager
        .try_take_session_with_work_dir("feishu-main:ou_owner", Some("/old/workdir".to_string()))
        .expect("session");
    session
        .history
        .push(bifrost_agent::ChatMessage::user("old message"));
    manager.return_session(session);

    let new_work_dir = tempfile::tempdir().expect("new work dir");
    let reply = apply_im_cwd_switch(
        &store,
        &Arc::new(ImGroupContextStore::new(temp_dir.path())),
        &manager,
        "im-cwd-provider",
        "feishu-main:ou_owner",
        new_work_dir.path(),
    )
    .expect("apply cwd");

    let canonical = std::fs::canonicalize(new_work_dir.path()).unwrap();
    let canonical_str = canonical.display().to_string();
    assert!(reply.contains(&canonical_str));
    let updated = store.get("im-cwd-provider").expect("provider");
    let agent_config = updated.agent_config.expect("agent config");
    assert_eq!(
        agent_config.work_dir.as_deref(),
        Some(canonical_str.as_str())
    );
    assert_eq!(
        agent_config.base_instructions.as_deref(),
        Some("keep provider prompt")
    );

    let detail = manager
        .get_session_detail("feishu-main:ou_owner")
        .expect("session detail");
    assert_eq!(detail.work_dir.as_deref(), Some(canonical_str.as_str()));
    assert_eq!(detail.message_count, 0);
}

#[tokio::test]
pub(super) async fn request_agent_stop_stops_external_runner_by_session_key() {
    let temp_dir = tempfile::tempdir().expect("temp runs root");
    let runs_root = temp_dir.path().to_path_buf();
    let manager = bifrost_agent::AgentSessionManager::new(3600);
    let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(&runs_root);
    let session_key = "external-stop-status-deadlock";
    let (executable, args) = fake_external_runner_sleep_command();
    let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "stop by shared helper".to_string(),
        operation: "ask".to_string(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("web".to_string()),
        session_key: Some(session_key.to_string()),
        runtime: "external_cli".to_string(),
        adapter: "mock".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: crate::im_gateway::external_cli::ExternalCliAdapterConfig {
            executable: Some(executable),
            args,
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let handle = tokio::spawn(async move { runtime.run(request).await.unwrap() });
    let mut stop_requested = false;
    for _ in 0..250 {
        if request_agent_stop_with_runs_root(&manager, session_key, &runs_root).await {
            stop_requested = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(stop_requested);

    let result = handle.await.expect("join external run");

    assert_eq!(
        result.status,
        crate::im_gateway::external_cli::ExternalCliRunStatus::Stopped
    );
}

#[test]
pub(super) fn im_status_text_formats_metrics_and_runner_metadata() {
    let manager = bifrost_agent::AgentSessionManager::new(3600);
    let mut session = manager
        .try_take_session_with_work_dir(
            "status-runner-metadata",
            Some("/tmp/status-runner".to_string()),
        )
        .expect("session should be available");
    session.mark_external_runner_runtime("codex", "codex");
    session.remember_external_conversation_ref(None, Some("thread-status-123".to_string()));
    session
        .history
        .push(bifrost_agent::ChatMessage::user("first"));
    session
        .history
        .push(bifrost_agent::ChatMessage::assistant("answer"));
    session
        .history
        .push(bifrost_agent::ChatMessage::user("second"));
    session.total_tokens_used = Some(38_634);
    session.compaction_count = 2;
    manager.return_session(session);

    let detail = manager
        .get_session_detail("status-runner-metadata")
        .expect("detail");
    let mut context = status_context_from_agent_runner(Some(
        &bifrost_agent::AgentRunnerMode::Custom("codex".to_string()),
    ));
    context.model = Some("trae-model".to_string());
    context.model_provider = Some("runner config".to_string());
    let text = build_im_status_text(Some(&detail), &context, None);

    assert!(text.contains("Agent 类型: External Runner Agent"));
    assert!(text.contains("Runner 类型: codex"));
    assert!(text.contains("Runner ID: codex"));
    assert!(text.contains("模型: trae-model（runner config）"));
    assert!(text.contains("外部会话: Codex threadId=thread-status-123"));
    assert!(text.contains("历史对话轮次: 2"));
    assert!(text.contains("API 累计 token: 38.6K"));
    assert!(text.contains("显式压缩次数: 2"));
    assert!(text.contains("上下文管理: 按 token/context budget 与 compaction 管理"));
    assert!(text.contains("常规请求使用完整 history：3 条"));
}
