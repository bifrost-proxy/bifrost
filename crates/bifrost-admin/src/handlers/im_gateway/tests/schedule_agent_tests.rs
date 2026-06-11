use super::{super::*, test_provider, EnvGuard};

#[test]
pub(super) fn schedule_chatgpt_web_initial_prompt_is_sent_as_first_message_only() {
    assert_eq!(
        schedule_external_runner_messages(
            crate::im_gateway::chatgpt_web::ADAPTER_ID,
            Some("INIT_MARKER"),
            false,
            "TASK_MARKER",
        ),
        vec!["INIT_MARKER".to_string(), "TASK_MARKER".to_string()]
    );
    assert_eq!(
        schedule_external_runner_messages(
            crate::im_gateway::chatgpt_web::ADAPTER_ID,
            Some("INIT_MARKER"),
            true,
            "TASK_MARKER",
        ),
        vec!["TASK_MARKER".to_string()]
    );
    assert_eq!(
        schedule_external_runner_messages("mock", Some("INIT_MARKER"), false, "TASK_MARKER"),
        vec!["TASK_MARKER".to_string()]
    );
}

#[test]
pub(super) fn schedule_agent_work_dir_prefers_schedule_then_inherited_default() {
    let mut agent_task = crate::im_gateway::types::ScheduleAgentTask {
        prompt: "TASK".to_string(),
        ..Default::default()
    };

    assert_eq!(
        schedule_agent_effective_work_dir(&agent_task, Some(" /repo/default ")).as_deref(),
        Some("/repo/default")
    );

    agent_task.work_dir = Some(" /repo/schedule ".to_string());
    assert_eq!(
        schedule_agent_effective_work_dir(&agent_task, Some("/repo/default")).as_deref(),
        Some("/repo/schedule")
    );
}

#[test]
pub(super) fn schedule_patch_merges_agent_adapter_config_without_dropping_prompt() {
    let mut schedule = ImSchedule {
        id: "schedule-patch".to_string(),
        name: "Schedule Patch".to_string(),
        enabled: true,
        message_channel: Some(crate::im_gateway::types::ImMessageChannelBinding {
            provider_id: "feishu-main".to_string(),
            target_id: "target-main".to_string(),
            target_mode: crate::im_gateway::types::MessageTargetMode::ConfiguredTarget,
        }),
        trigger: crate::im_gateway::types::ScheduleTrigger::Interval { every_ms: 60_000 },
        task_type: crate::im_gateway::types::ScheduleTaskType::Agent,
        script: Default::default(),
        agent: Some(crate::im_gateway::types::ScheduleAgentTask {
            prompt: "Original prompt".to_string(),
            runner_id: Some("codex".to_string()),
            session_key: Some("stable-session".to_string()),
            adapter_config: Some(crate::im_gateway::external_cli::ExternalCliAdapterConfig {
                model: Some("gpt-old".to_string()),
                sandbox: Some("read-only".to_string()),
                env: std::collections::BTreeMap::from([(
                    "BASE_ENV".to_string(),
                    "runner".to_string(),
                )]),
                ..Default::default()
            }),
            ..Default::default()
        }),
        timeout_ms: 10_000,
        max_output_bytes: 1024,
        concurrency_policy: Default::default(),
        retry: Default::default(),
        next_run_at: None,
        last_run_at: None,
        created_at: 0,
        updated_at: 0,
    };

    apply_schedule_patch(
        &mut schedule,
        &serde_json::json!({
            "agent": {
                "adapter_config": {
                    "model": "gpt-new",
                    "reasoningEffort": "high",
                    "dangerouslyBypassHookTrust": true,
                    "strictConfig": true,
                    "oss": true,
                    "localProvider": "ollama",
                    "outputSchema": "/tmp/schema.json",
                    "color": "never"
                }
            }
        }),
    );

    let agent = schedule.agent.expect("agent retained");
    assert_eq!(agent.prompt, "Original prompt");
    assert_eq!(agent.runner_id.as_deref(), Some("codex"));
    assert_eq!(agent.session_key.as_deref(), Some("stable-session"));
    let adapter_config = agent.adapter_config.expect("adapter config updated");
    assert_eq!(adapter_config.model.as_deref(), Some("gpt-new"));
    assert_eq!(adapter_config.sandbox.as_deref(), Some("read-only"));
    assert_eq!(
        adapter_config.env.get("BASE_ENV").map(String::as_str),
        Some("runner")
    );
    assert_eq!(adapter_config.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(adapter_config.dangerously_bypass_hook_trust, Some(true));
    assert_eq!(adapter_config.strict_config, Some(true));
    assert_eq!(adapter_config.oss, Some(true));
    assert_eq!(adapter_config.local_provider.as_deref(), Some("ollama"));
    assert_eq!(
        adapter_config.output_schema.as_deref(),
        Some("/tmp/schema.json")
    );
    assert_eq!(adapter_config.color.as_deref(), Some("never"));
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn schedule_chatgpt_web_session_exists_requires_non_empty_conversation() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let sessions_path = bifrost_agent::config::agent_home_dir()
        .join("im_gateway")
        .join("chatgpt_web")
        .join("sessions.json");
    tokio::fs::create_dir_all(sessions_path.parent().expect("sessions parent"))
        .await
        .expect("create sessions parent");
    tokio::fs::write(
        &sessions_path,
        r#"{"schedule:empty":"  ","schedule:ready":"conv-ready"}"#,
    )
    .await
    .expect("write sessions map");

    assert!(crate::im_gateway::chatgpt_web::session_conversation_exists("schedule:ready").await);
    assert!(!crate::im_gateway::chatgpt_web::session_conversation_exists("schedule:empty").await);
    assert!(!crate::im_gateway::chatgpt_web::session_conversation_exists("schedule:missing").await);
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn schedule_agent_can_run_selected_external_runner_with_initial_prompt() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());

    let provider = test_provider();
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");
    service
        .target_store
        .add(ImTarget {
            id: "target-main".to_string(),
            provider_id: provider.id.clone(),
            display_name: "Target Main".to_string(),
            receive_id_type: "open_id".to_string(),
            receive_id: "ou-target".to_string(),
            default_msg_type: "text".to_string(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        })
        .expect("add target");

    let mut external_cli_config =
        crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    external_cli_config.runners.insert(
        "chatgpt-test".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            enabled: false,
            adapter: "mock".to_string(),
            instructions: None,
            adapter_config: crate::im_gateway::external_cli::ExternalCliAdapterConfig {
                executable: Some("sh".to_string()),
                args: vec![
                    "-c".to_string(),
                    "input=$(cat); if printf '%s' \"$input\" | grep -q INIT_MARKER && printf '%s' \"$input\" | grep -q TASK_MARKER; then printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"SCHEDULE_RUNNER_OK\"}'; else printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"SCHEDULE_RUNNER_MISSING_PROMPT\"}'; fi".to_string(),
                ],
                ..Default::default()
            },
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
            delivery_mode: crate::im_gateway::external_cli::ExternalCliDeliveryMode::FinalReply,
        },
    );
    service
        .external_cli_config_store
        .save(external_cli_config)
        .expect("save external config");

    let schedule = ImSchedule {
        id: "schedule-runner".to_string(),
        name: "Schedule Runner".to_string(),
        enabled: true,
        message_channel: Some(crate::im_gateway::types::ImMessageChannelBinding {
            provider_id: provider.id.clone(),
            target_id: "target-main".to_string(),
            target_mode: crate::im_gateway::types::MessageTargetMode::ConfiguredTarget,
        }),
        trigger: crate::im_gateway::types::ScheduleTrigger::Interval { every_ms: 60_000 },
        task_type: crate::im_gateway::types::ScheduleTaskType::Agent,
        script: Default::default(),
        agent: Some(crate::im_gateway::types::ScheduleAgentTask {
            prompt: "TASK_MARKER".to_string(),
            runner_id: Some("chatgpt-test".to_string()),
            initial_prompt: Some("INIT_MARKER".to_string()),
            session_key: Some("schedule-runner-test".to_string()),
            work_dir: None,
            system_prompt: None,
            adapter_config: None,
            conversation_ref: None,
        }),
        timeout_ms: 10_000,
        max_output_bytes: 1024,
        concurrency_policy: Default::default(),
        retry: Default::default(),
        next_run_at: None,
        last_run_at: None,
        created_at: 0,
        updated_at: 0,
    };

    let run = execute_schedule_once(
        &service,
        &schedule,
        "run-selected-runner".to_string(),
        crate::im_gateway::types::TriggerSource::ManualRun,
    )
    .await;

    assert_eq!(run.status, crate::im_gateway::types::TaskRunStatus::Success);
    assert_eq!(run.runner_id.as_deref(), Some("chatgpt-test"));
    assert_eq!(run.provider_id.as_deref(), Some("feishu-main"));
    assert_eq!(run.target_id.as_deref(), Some("target-main"));
    assert!(run
        .input_preview
        .as_deref()
        .unwrap_or_default()
        .contains("INIT_MARKER"));
    assert!(run
        .input_preview
        .as_deref()
        .unwrap_or_default()
        .contains("TASK_MARKER"));
    assert_eq!(
        run.agent_final_response.as_deref(),
        Some("SCHEDULE_RUNNER_OK")
    );
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn schedule_agent_adapter_config_overrides_runner_without_dropping_command() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());

    let provider = test_provider();
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");
    service
        .target_store
        .add(ImTarget {
            id: "target-main".to_string(),
            provider_id: provider.id.clone(),
            display_name: "Target Main".to_string(),
            receive_id_type: "open_id".to_string(),
            receive_id: "ou-target".to_string(),
            default_msg_type: "text".to_string(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        })
        .expect("add target");

    let mut external_cli_config =
        crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    external_cli_config.runners.insert(
        "codex-override-test".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            enabled: false,
            adapter: "mock".to_string(),
            instructions: None,
            adapter_config: crate::im_gateway::external_cli::ExternalCliAdapterConfig {
                executable: Some("sh".to_string()),
                args: vec![
                    "-c".to_string(),
                    "cat >/dev/null; if [ \"$MODEL_OVERRIDE\" = \"gpt-schedule\" ] && [ \"$BASE_ENV\" = \"runner\" ]; then printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"OVERRIDE_OK\"}'; else printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"OVERRIDE_MISSING\"}'; fi".to_string(),
                ],
                env: std::collections::BTreeMap::from([(
                    "BASE_ENV".to_string(),
                    "runner".to_string(),
                )]),
                ..Default::default()
            },
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
            delivery_mode: crate::im_gateway::external_cli::ExternalCliDeliveryMode::FinalReply,
        },
    );
    service
        .external_cli_config_store
        .save(external_cli_config)
        .expect("save external config");

    let schedule = ImSchedule {
        id: "schedule-adapter-override".to_string(),
        name: "Schedule Adapter Override".to_string(),
        enabled: true,
        message_channel: Some(crate::im_gateway::types::ImMessageChannelBinding {
            provider_id: provider.id.clone(),
            target_id: "target-main".to_string(),
            target_mode: crate::im_gateway::types::MessageTargetMode::ConfiguredTarget,
        }),
        trigger: crate::im_gateway::types::ScheduleTrigger::Interval { every_ms: 60_000 },
        task_type: crate::im_gateway::types::ScheduleTaskType::Agent,
        script: Default::default(),
        agent: Some(crate::im_gateway::types::ScheduleAgentTask {
            prompt: "TASK_MARKER".to_string(),
            runner_id: Some("codex-override-test".to_string()),
            initial_prompt: None,
            session_key: Some("schedule-adapter-override".to_string()),
            work_dir: None,
            system_prompt: None,
            adapter_config: Some(crate::im_gateway::external_cli::ExternalCliAdapterConfig {
                model: Some("gpt-schedule".to_string()),
                env: std::collections::BTreeMap::from([(
                    "MODEL_OVERRIDE".to_string(),
                    "gpt-schedule".to_string(),
                )]),
                ..Default::default()
            }),
            conversation_ref: None,
        }),
        timeout_ms: 10_000,
        max_output_bytes: 1024,
        concurrency_policy: Default::default(),
        retry: Default::default(),
        next_run_at: None,
        last_run_at: None,
        created_at: 0,
        updated_at: 0,
    };

    let run = execute_schedule_once(
        &service,
        &schedule,
        "run-adapter-override".to_string(),
        crate::im_gateway::types::TriggerSource::ManualRun,
    )
    .await;

    assert_eq!(run.status, crate::im_gateway::types::TaskRunStatus::Success);
    assert_eq!(run.agent_final_response.as_deref(), Some("OVERRIDE_OK"));
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn schedule_external_runner_executes_from_configured_work_dir() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());
    let work_dir = temp_dir.path().join("runner-workdir");
    std::fs::create_dir_all(&work_dir).expect("create runner workdir");
    std::fs::write(work_dir.join("expected.marker"), b"ok").expect("write marker");

    let provider = test_provider();
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");
    service
        .target_store
        .add(ImTarget {
            id: "target-main".to_string(),
            provider_id: provider.id.clone(),
            display_name: "Target Main".to_string(),
            receive_id_type: "open_id".to_string(),
            receive_id: "ou-target".to_string(),
            default_msg_type: "text".to_string(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        })
        .expect("add target");

    let mut external_cli_config =
        crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    external_cli_config.runners.insert(
        "codex-workdir-test".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            enabled: false,
            adapter: "mock".to_string(),
            instructions: None,
            adapter_config: crate::im_gateway::external_cli::ExternalCliAdapterConfig {
                executable: Some(if cfg!(windows) {
                    "powershell.exe".to_string()
                } else {
                    "sh".to_string()
                }),
                args: if cfg!(windows) {
                    vec![
                        "-NoProfile".to_string(),
                        "-Command".to_string(),
                        "$input | Out-Null; if (Test-Path -LiteralPath '.\\expected.marker') { [Console]::Out.WriteLine('{\"type\":\"assistant_final\",\"content\":\"WORKDIR_OK\"}') } else { [Console]::Out.WriteLine('{\"type\":\"assistant_final\",\"content\":\"WORKDIR_MISMATCH\"}') }".to_string(),
                    ]
                } else {
                    vec![
                        "-c".to_string(),
                        "cat >/dev/null; if [ -f ./expected.marker ]; then printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"WORKDIR_OK\"}'; else printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"WORKDIR_MISMATCH\"}'; fi".to_string(),
                    ]
                },
                env: Default::default(),
                ..Default::default()
            },
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
            delivery_mode: crate::im_gateway::external_cli::ExternalCliDeliveryMode::FinalReply,
        },
    );
    service
        .external_cli_config_store
        .save(external_cli_config)
        .expect("save external config");

    let schedule = ImSchedule {
        id: "schedule-workdir".to_string(),
        name: "Schedule Workdir".to_string(),
        enabled: true,
        message_channel: Some(crate::im_gateway::types::ImMessageChannelBinding {
            provider_id: provider.id.clone(),
            target_id: "target-main".to_string(),
            target_mode: crate::im_gateway::types::MessageTargetMode::ConfiguredTarget,
        }),
        trigger: crate::im_gateway::types::ScheduleTrigger::Interval { every_ms: 60_000 },
        task_type: crate::im_gateway::types::ScheduleTaskType::Agent,
        script: Default::default(),
        agent: Some(crate::im_gateway::types::ScheduleAgentTask {
            prompt: "TASK_MARKER".to_string(),
            runner_id: Some("codex-workdir-test".to_string()),
            initial_prompt: None,
            session_key: Some("schedule-workdir".to_string()),
            work_dir: Some(work_dir.display().to_string()),
            system_prompt: None,
            adapter_config: None,
            conversation_ref: None,
        }),
        timeout_ms: 10_000,
        max_output_bytes: 1024,
        concurrency_policy: Default::default(),
        retry: Default::default(),
        next_run_at: None,
        last_run_at: None,
        created_at: 0,
        updated_at: 0,
    };

    let run = execute_schedule_once(
        &service,
        &schedule,
        "run-workdir".to_string(),
        crate::im_gateway::types::TriggerSource::ManualRun,
    )
    .await;

    assert_eq!(run.status, crate::im_gateway::types::TaskRunStatus::Success);
    assert_eq!(run.agent_final_response.as_deref(), Some("WORKDIR_OK"));
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn schedule_agent_persists_codex_thread_id_for_next_run() {
    let temp_dir = tempfile::tempdir().expect("temp data dir");
    let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
    let service = ImGatewayService::new(temp_dir.path());

    let provider = test_provider();
    service
        .provider_store
        .add(provider.clone())
        .expect("add provider");
    service
        .target_store
        .add(ImTarget {
            id: "target-main".to_string(),
            provider_id: provider.id.clone(),
            display_name: "Target Main".to_string(),
            receive_id_type: "open_id".to_string(),
            receive_id: "ou-target".to_string(),
            default_msg_type: "text".to_string(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        })
        .expect("add target");

    let mut external_cli_config =
        crate::im_gateway::external_cli::ExternalCliGatewayConfig::default();
    external_cli_config.runners.insert(
        "codex-thread-test".to_string(),
        crate::im_gateway::external_cli::ExternalCliAgentSettings {
            enabled: false,
            adapter: "codex".to_string(),
            instructions: None,
            adapter_config: crate::im_gateway::external_cli::ExternalCliAdapterConfig {
                executable: Some("sh".to_string()),
                args: vec![
                    "-c".to_string(),
                    "cat >/dev/null; printf '%s\n' '{\"type\":\"thread.started\",\"thread_id\":\"thread-schedule-1\"}' '{\"type\":\"assistant_final\",\"content\":\"THREAD_OK\"}'".to_string(),
                ],
                ..Default::default()
            },
            inject_bifrost_tools: false,
            skill_paths: Vec::new(),
            delivery_mode: crate::im_gateway::external_cli::ExternalCliDeliveryMode::FinalReply,
        },
    );
    service
        .external_cli_config_store
        .save(external_cli_config)
        .expect("save external config");

    let schedule = ImSchedule {
        id: "schedule-codex-thread".to_string(),
        name: "Schedule Codex Thread".to_string(),
        enabled: true,
        message_channel: Some(crate::im_gateway::types::ImMessageChannelBinding {
            provider_id: provider.id.clone(),
            target_id: "target-main".to_string(),
            target_mode: crate::im_gateway::types::MessageTargetMode::ConfiguredTarget,
        }),
        trigger: crate::im_gateway::types::ScheduleTrigger::Interval { every_ms: 60_000 },
        task_type: crate::im_gateway::types::ScheduleTaskType::Agent,
        script: Default::default(),
        agent: Some(crate::im_gateway::types::ScheduleAgentTask {
            prompt: "TASK_MARKER".to_string(),
            runner_id: Some("codex-thread-test".to_string()),
            initial_prompt: None,
            session_key: Some("schedule-codex-thread".to_string()),
            work_dir: None,
            system_prompt: None,
            adapter_config: None,
            conversation_ref: None,
        }),
        timeout_ms: 10_000,
        max_output_bytes: 1024,
        concurrency_policy: Default::default(),
        retry: Default::default(),
        next_run_at: None,
        last_run_at: None,
        created_at: 0,
        updated_at: 0,
    };
    service
        .schedule_store
        .add(schedule.clone())
        .expect("store schedule");

    let run = execute_schedule_once(
        &service,
        &schedule,
        "run-codex-thread".to_string(),
        crate::im_gateway::types::TriggerSource::ManualRun,
    )
    .await;

    assert_eq!(run.status, crate::im_gateway::types::TaskRunStatus::Success);
    assert_eq!(run.agent_final_response.as_deref(), Some("THREAD_OK"));
    let stored = service
        .schedule_store
        .get("schedule-codex-thread")
        .expect("stored schedule");
    let conversation_ref = stored
        .agent
        .expect("agent task")
        .conversation_ref
        .expect("conversation ref");
    assert_eq!(conversation_ref.adapter, "codex");
    assert_eq!(
        conversation_ref.thread_id.as_deref(),
        Some("thread-schedule-1")
    );
}

#[test]
pub(super) fn schedule_external_result_extracts_chatgpt_conversation_id() {
    let result = crate::im_gateway::external_cli::ExternalCliRunResult {
        run_id: "run-1".to_string(),
        session_key: Some("schedule:one".to_string()),
        runtime: "external_cli".to_string(),
        adapter: crate::im_gateway::chatgpt_web::ADAPTER_ID.to_string(),
        status: crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded,
        exit_code: Some(0),
        response: "ok".to_string(),
        responses: vec!["ok".to_string()],
        started_at: 1,
        finished_at: 2,
        duration_ms: 1,
        artifacts: crate::im_gateway::external_cli::ExternalCliRunArtifacts {
            run_dir: "".to_string(),
            prompt: "".to_string(),
            command_snapshot: "".to_string(),
            stdout: "".to_string(),
            stderr: "".to_string(),
            normalized_events: "".to_string(),
            last_message: "".to_string(),
        },
        events: Vec::new(),
        metadata: std::collections::BTreeMap::from([(
            "conversationId".to_string(),
            "conv-schedule-1".to_string(),
        )]),
    };

    let conversation_ref = schedule_conversation_ref_from_external_result(
        crate::im_gateway::chatgpt_web::ADAPTER_ID,
        &result,
    )
    .expect("conversation ref");

    assert_eq!(
        conversation_ref.conversation_id.as_deref(),
        Some("conv-schedule-1")
    );
    assert_eq!(
        conversation_ref.adapter,
        crate::im_gateway::chatgpt_web::ADAPTER_ID
    );
}
