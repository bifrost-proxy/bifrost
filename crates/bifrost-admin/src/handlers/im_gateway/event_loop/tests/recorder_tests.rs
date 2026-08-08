use super::*;

#[test]
fn ensure_external_cli_recorder_creates_metadata_once() {
    let temp_dir = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
    let session_key = "im-recorder-created";
    let mut session = bifrost_agent::AgentSession::new(session_key);
    let mut recorder = None;

    ensure_external_cli_session_recorder(
        &mut session,
        &mut recorder,
        session_key,
        &recorder_test_provider(),
        "codex",
        &recorder_test_request(session_key),
    );

    let path = recorder.as_ref().unwrap().file_path().to_path_buf();
    drop(recorder);
    let events = bifrost_agent::persistence::load_conversation_events(&path).unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["session_start", "title_updated", "run_state_changed"]
    );
}

#[test]
fn ensure_external_cli_recorder_rejects_an_unremovable_canonical_path() {
    let temp_dir = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
    let session_key = "im-recorder-invalid-path";
    let data_dir = bifrost_agent::config::agent_home_dir();
    let canonical = bifrost_agent::persistence::canonical_conversation_path(&data_dir, session_key);
    std::fs::create_dir_all(&canonical).unwrap();
    let mut session = bifrost_agent::AgentSession::new(session_key);
    let mut recorder = None;

    ensure_external_cli_session_recorder(
        &mut session,
        &mut recorder,
        session_key,
        &recorder_test_provider(),
        "codex",
        &recorder_test_request(session_key),
    );

    assert!(recorder.is_none());
}

#[cfg(unix)]
#[test]
fn ensure_external_cli_recorder_handles_metadata_write_failures() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().unwrap();
    let _guard = crate::handlers::im_gateway::tests::EnvGuard::set_data_dir(temp_dir.path());
    let session_key = "im-recorder-readonly";
    let data_dir = bifrost_agent::config::agent_home_dir();
    let parent = bifrost_agent::persistence::canonical_conversation_path(&data_dir, session_key)
        .parent()
        .unwrap()
        .to_path_buf();
    std::fs::create_dir_all(&parent).unwrap();
    let original_mode = std::fs::metadata(&parent).unwrap().permissions().mode();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).unwrap();
    let mut session = bifrost_agent::AgentSession::new(session_key);
    let mut recorder = None;

    ensure_external_cli_session_recorder(
        &mut session,
        &mut recorder,
        session_key,
        &recorder_test_provider(),
        "codex",
        &recorder_test_request(session_key),
    );

    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(original_mode)).unwrap();
    assert!(recorder.is_some());
    assert!(!recorder.unwrap().file_path().exists());
}

#[test]
fn external_cli_progress_runner_summary_uses_session_effort_override() {
    let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        message: "hello".to_string(),
        images: Vec::new(),
        files: Vec::new(),
        operation: "chat".to_string(),
        params: serde_json::Value::Null,
        provider_id: Some("feishu-main".to_string()),
        runner_id: Some("Traex".to_string()),
        session_key: Some("feishu-main:owner-open-id".to_string()),
        runtime: "external_cli".to_string(),
        adapter: crate::im_gateway::external_cli::TRAEX_ADAPTER.to_string(),
        work_dir: Some(std::path::PathBuf::from("/tmp/bifrost")),
        instructions: None,
        adapter_config: crate::im_gateway::external_cli::ExternalCliAdapterConfig {
            model: Some("GPT-5.5".to_string()),
            reasoning_effort: Some("high".to_string()),
            config_overrides: vec![
                "model_reasoning_effort=\"xhigh\"".to_string(),
                "model_provider=\"trae\"".to_string(),
            ],
            ..Default::default()
        },
        allow_work_dirs: Vec::new(),
        inject_bifrost_tools: false,
        skill_paths: Vec::new(),
    };
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("modelReasoningEffort".to_string(), "xhigh".to_string());

    let summary = external_cli_progress_runner_summary(
        "Traex",
        crate::im_gateway::external_cli::TRAEX_ADAPTER,
        &request,
        Some(&metadata),
    );

    assert_eq!(summary.model.as_deref(), Some("GPT-5.5"));
    assert_eq!(summary.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(summary.reasoning_source.as_deref(), Some("runner 配置"));
}

#[test]
fn external_cli_progress_runner_summary_reads_weekly_usage_metadata() {
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("codexWeeklyUsedPercent".to_string(), "140".to_string());
    metadata.insert("codexWeeklyWindowMinutes".to_string(), "10080".to_string());
    metadata.insert("codexWeeklyResetsAt".to_string(), "1784490086".to_string());

    let usage = external_cli_weekly_usage_from_metadata(&metadata)
        .expect("valid weekly metadata should be rendered");

    assert_eq!(usage.used_percent, 100);
    assert_eq!(usage.window_minutes, 10_080);
    assert_eq!(usage.resets_at, Some(1_784_490_086));
    metadata.remove("codexWeeklyWindowMinutes");
    assert!(external_cli_weekly_usage_from_metadata(&metadata).is_none());
}

#[test]
fn external_cli_progress_session_id_flows_from_live_event_to_feishu_card() {
    let request = recorder_test_request("feishu-main:owner-open-id");
    let event = crate::im_gateway::external_cli::parse_progress_events(
        r#"{"type":"thread.started","thread_id":"thread-live-card"}"#,
    )
    .pop()
    .expect("thread started event");
    let mut metadata = std::collections::BTreeMap::new();
    assert!(
        crate::im_gateway::external_cli::merge_external_cli_progress_metadata(
            "codex",
            &event,
            &mut metadata,
        )
    );
    let runner = external_cli_progress_runner_summary("codex", "codex", &request, Some(&metadata));
    let mut snapshot =
        crate::im_gateway::progress_card::ImAgentProgressSnapshot::new("s1", "codex task");
    snapshot.runner = Some(runner);

    let card = crate::im_gateway::progress_card::build_feishu_progress_card(&snapshot, true);
    let serialized = serde_json::to_string(&card).expect("serialize progress card");

    assert!(
        serialized.contains("Runner：`codex` · Adapter：`codex` · Session ID：`thread-live-card`")
    );
}

fn external_cli_result_with_status(
    status: crate::im_gateway::external_cli::ExternalCliRunStatus,
) -> crate::im_gateway::external_cli::ExternalCliRunResult {
    crate::im_gateway::external_cli::ExternalCliRunResult {
        run_id: "run-timeout".to_string(),
        session_key: Some("s1".to_string()),
        runtime: "external_cli".to_string(),
        adapter: "traex".to_string(),
        status,
        exit_code: None,
        response: "early agent message".to_string(),
        responses: Vec::new(),
        started_at: 1,
        finished_at: 181_000,
        duration_ms: 180_999,
        artifacts: crate::im_gateway::external_cli::ExternalCliRunArtifacts {
            run_dir: String::new(),
            prompt: String::new(),
            command_snapshot: String::new(),
            stdout: String::new(),
            stderr: String::new(),
            normalized_events: String::new(),
            last_message: String::new(),
        },
        events: Vec::new(),
        metadata: std::collections::BTreeMap::new(),
    }
}

#[test]
fn timed_out_external_cli_result_reports_failure_reply() {
    let result = external_cli_result_with_status(
        crate::im_gateway::external_cli::ExternalCliRunStatus::TimedOut,
    );

    let reply = external_cli_non_success_reply(&result);

    assert!(reply.contains("timed out after 180 seconds"));
    assert!(reply.contains("run-timeout"));
    assert!(!reply.contains("early agent message"));
}

#[tokio::test]
async fn external_runner_small_branches_keep_safe_defaults() {
    assert_eq!(
        external_cli_adapter_label(crate::im_gateway::chatgpt_web::ADAPTER_ID),
        "ChatGPT Web"
    );
    assert_eq!(external_cli_adapter_label("traex"), "Runner");

    let mut without_message = group_test_event("provider", "missing", "", false, 1);
    without_message.message = None;
    maybe_stop_external_cli_for_event(&without_message, "unrelated").await;

    let non_stop = group_test_event("provider", "continue", "continue", false, 2);
    maybe_stop_external_cli_for_event(&non_stop, "unrelated").await;

    let other_session = group_test_event("provider", "stop", "/stop", false, 3);
    maybe_stop_external_cli_for_event(&other_session, "unrelated").await;
}
