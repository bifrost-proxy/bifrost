use super::*;

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

#[tokio::test]
pub(super) async fn request_agent_stop_stops_external_runner_by_session_key() {
    let temp_dir = tempfile::tempdir().expect("temp runs root");
    let runs_root = temp_dir.path().to_path_buf();
    let manager = bifrost_agent::AgentSessionManager::new(3600);
    let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(&runs_root);
    let session_key = "external-stop-status-deadlock";
    let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
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
            executable: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                "sleep 2; printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"too late\"}'"
                    .to_string(),
            ],
            timeout_secs: Some(10),
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };

    let handle = tokio::spawn(async move { runtime.run(request).await.unwrap() });
    for _ in 0..50 {
        if std::fs::read_dir(&runs_root)
            .expect("read runs root")
            .next()
            .is_some()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    assert!(request_agent_stop_with_runs_root(&manager, session_key, &runs_root).await);
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
    let text = build_im_status_text(
        Some(&detail),
        &status_context_from_agent_runner(Some(&bifrost_agent::AgentRunnerMode::Custom(
            "codex".to_string(),
        ))),
        None,
    );

    assert!(text.contains("Agent 类型: External Runner Agent"));
    assert!(text.contains("Runner 类型: codex"));
    assert!(text.contains("Runner ID: codex"));
    assert!(text.contains("外部会话: Codex threadId=thread-status-123"));
    assert!(text.contains("历史对话轮次: 2"));
    assert!(text.contains("API 累计 token: 38.6K"));
    assert!(text.contains("压缩次数: 2"));
}

#[test]
pub(super) fn im_status_text_uses_resolved_default_work_dir_when_session_has_no_override() {
    let manager = bifrost_agent::AgentSessionManager::new(3600);
    let session = manager
        .try_take_session_with_work_dir("status-default-workdir", None)
        .expect("session should be available");
    manager.return_session(session);

    let detail = manager
        .get_session_detail("status-default-workdir")
        .expect("detail");
    assert!(detail.work_dir.is_none());

    let current_dir = std::env::current_dir()
        .expect("current dir")
        .display()
        .to_string();
    let text = build_im_status_text(
        Some(&detail),
        &status_context_from_agent_runner(None),
        Some(current_dir.as_str()),
    );
    let api_text = build_agent_api_status_text(
        Some(&detail),
        &bifrost_agent::config::AgentConfig::default(),
    );

    assert!(text.contains(&format!("工作路径: {current_dir}")));
    assert!(api_text.contains(&format!("工作路径: {current_dir}")));
    assert!(!text.contains("工作路径: N/A"));
    assert!(!api_text.contains("工作路径: N/A"));
}
