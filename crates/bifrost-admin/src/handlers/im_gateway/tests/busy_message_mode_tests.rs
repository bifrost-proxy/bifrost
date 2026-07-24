use super::*;

fn runner_config(
    runner_id: &str,
    adapter: &str,
) -> crate::im_gateway::external_cli::ExternalCliGatewayConfig {
    let mut config = crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    config.runners.insert(
        runner_id.to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            enabled: true,
            adapter: adapter.to_string(),
            ..Default::default()
        },
    );
    config
}

#[test]
fn busy_default_mode_guides_external_runners_except_chatgpt_web() {
    for (runner_id, adapter) in [
        ("codex-main", "codex"),
        ("traex-main", "traex"),
        ("claude-main", "claude_code"),
    ] {
        let config = runner_config(runner_id, adapter);
        assert_eq!(
            busy_default_mode_for_agent_config(
                &bifrost_agent::AgentConfig {
                    runner: Some(bifrost_agent::AgentRunnerMode::Custom(
                        runner_id.to_string(),
                    )),
                    ..Default::default()
                },
                &config,
                Some("feishu-main"),
            ),
            BusyMessageDefaultMode::ExternalGuide,
            "adapter {adapter} should default to external guide",
        );
    }

    let config = runner_config("chatgpt-web", crate::im_gateway::chatgpt_web::ADAPTER_ID);
    assert_eq!(
        busy_default_mode_for_agent_config(
            &bifrost_agent::AgentConfig {
                runner: Some(bifrost_agent::AgentRunnerMode::Custom(
                    "chatgpt-web".to_string(),
                )),
                ..Default::default()
            },
            &config,
            Some("feishu-main"),
        ),
        BusyMessageDefaultMode::Queue
    );
}

#[test]
fn apply_busy_message_default_guides_messages_without_queueing() {
    let manager = SessionQueueManager::new();

    let result = apply_busy_message_default(
        &manager,
        "busy-default-guide",
        "继续看最新日志",
        BusyMessageDefaultMode::Guide,
    )
    .expect("guide default should be accepted");

    assert_eq!(result, BusyMessageDefaultResult::Guide { pending_count: 1 });
    assert_eq!(
        manager.guide_status("busy-default-guide"),
        vec!["继续看最新日志".to_string()]
    );
    assert!(manager.queue_status("busy-default-guide").is_empty());
}

#[test]
fn merge_pending_guide_messages_keeps_worker_guides_and_deduplicates_queue_guides() {
    let merged = merge_pending_guide_messages(
        &["worker-guide".to_string(), "shared-guide".to_string()],
        vec!["shared-guide".to_string(), "queue-guide".to_string()],
    );

    assert_eq!(
        merged,
        vec![
            "worker-guide".to_string(),
            "shared-guide".to_string(),
            "queue-guide".to_string()
        ]
    );
}

#[test]
fn apply_busy_message_default_queues_custom_runner_messages() {
    let manager = SessionQueueManager::new();

    let result = apply_busy_message_default(
        &manager,
        "busy-default-queue",
        "下一条给 ChatGPT Web",
        BusyMessageDefaultMode::Queue,
    )
    .expect("queue default should be accepted");

    match result {
        BusyMessageDefaultResult::Queue { items } => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].message, "下一条给 ChatGPT Web");
        }
        BusyMessageDefaultResult::Guide { .. } => panic!("custom runner default should queue"),
    }
    assert!(manager.guide_status("busy-default-queue").is_empty());
    assert_eq!(
        manager.pop_queue("busy-default-queue").as_deref(),
        Some("下一条给 ChatGPT Web")
    );
}

#[test]
fn codex_runner_metadata_resumes_queued_messages_after_current_run() {
    let mut request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "queued continuation".to_string(),
        operation: "chat".to_string(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("codex".to_string()),
        session_key: Some("im:provider-a:user-a".to_string()),
        runtime: "external_cli".to_string(),
        adapter: "codex".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: Default::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: true,
        skill_paths: Vec::new(),
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("threadId".to_string(), "thread-existing".to_string());

    apply_external_cli_resume_metadata(&mut request, &metadata);

    assert_eq!(
        request
            .params
            .get("threadId")
            .and_then(|value| value.as_str()),
        Some("thread-existing")
    );
}

#[test]
fn traex_runner_metadata_resumes_queued_messages_after_current_run() {
    let mut request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "queued continuation".to_string(),
        operation: "chat".to_string(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("traex".to_string()),
        session_key: Some("im:provider-a:user-a".to_string()),
        runtime: "external_cli".to_string(),
        adapter: crate::im_gateway::external_cli::TRAEX_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: Default::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: true,
        skill_paths: Vec::new(),
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("threadId".to_string(), "trae-thread-existing".to_string());

    apply_external_cli_resume_metadata(&mut request, &metadata);

    assert_eq!(
        request
            .params
            .get("threadId")
            .and_then(|value| value.as_str()),
        Some("trae-thread-existing")
    );
}

#[test]
fn claude_code_runner_metadata_resumes_queued_messages_after_current_run() {
    let mut request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "queued continuation".to_string(),
        operation: "chat".to_string(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("claude-code".to_string()),
        session_key: Some("im:provider-a:user-a".to_string()),
        runtime: "external_cli".to_string(),
        adapter: crate::im_gateway::external_cli::CLAUDE_CODE_ADAPTER.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: Default::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: true,
        skill_paths: Vec::new(),
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert(
        "threadId".to_string(),
        "claude-session-existing".to_string(),
    );

    apply_external_cli_resume_metadata(&mut request, &metadata);

    assert_eq!(
        request
            .params
            .get("threadId")
            .and_then(|value| value.as_str()),
        Some("claude-session-existing")
    );
}

#[test]
fn codex_runner_metadata_does_not_override_explicit_thread() {
    let mut request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "queued continuation".to_string(),
        operation: "chat".to_string(),
        params: serde_json::json!({ "threadId": "explicit-thread" }),
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("codex".to_string()),
        session_key: Some("im:provider-a:user-a".to_string()),
        runtime: "external_cli".to_string(),
        adapter: "codex".to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: Default::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: true,
        skill_paths: Vec::new(),
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("threadId".to_string(), "remembered-thread".to_string());

    apply_external_cli_resume_metadata(&mut request, &metadata);

    assert_eq!(
        request
            .params
            .get("threadId")
            .and_then(|value| value.as_str()),
        Some("explicit-thread")
    );
}

#[test]
fn chatgpt_web_metadata_resumes_persisted_conversation() {
    let mut request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        files: Vec::new(),
        message: "continue".to_string(),
        operation: "ask".to_string(),
        params: serde_json::Value::Null,
        provider_id: Some("provider-a".to_string()),
        runner_id: Some("chatgpt-web".to_string()),
        session_key: Some("im:provider-a:user-a".to_string()),
        runtime: "external_cli".to_string(),
        adapter: crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string(),
        work_dir: None,
        instructions: None,
        adapter_config: Default::default(),
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: true,
        skill_paths: Vec::new(),
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("conversationId".to_string(), "conv-existing".to_string());

    apply_external_cli_resume_metadata(&mut request, &metadata);

    assert_eq!(
        request
            .params
            .get("conversationId")
            .and_then(|value| value.as_str()),
        Some("conv-existing")
    );
}
