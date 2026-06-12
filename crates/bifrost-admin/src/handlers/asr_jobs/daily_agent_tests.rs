
fn assert_path_ends_with(path: impl AsRef<std::path::Path>, components: &[&str]) {
    let expected = components.iter().collect::<std::path::PathBuf>();
    assert!(
        path.as_ref().ends_with(&expected),
        "expected path `{}` to end with `{}`",
        path.as_ref().display(),
        expected.display()
    );
}

#[test]
fn daily_agent_default_config_has_report_and_tomorrow_todo_agents() {
    let config = AsrDailyAgentConfig::default();
    let agents = normalized_daily_agents(&config);
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0].id, DEFAULT_DAILY_AGENT_ID);
    assert_eq!(agents[0].output_dir, DEFAULT_DAILY_AGENT_OUTPUT_DIR);
    assert_eq!(agents[1].id, DEFAULT_TOMORROW_TODO_AGENT_ID);
    assert_eq!(agents[1].output_dir, DEFAULT_TOMORROW_TODO_OUTPUT_DIR);
    assert!(agents[1].im_delivery.enabled);
    assert_eq!(
        agents[1].im_delivery.channel.as_deref(),
        Some(DEFAULT_DAILY_AGENT_IM_CHANNEL)
    );
}

#[test]
fn daily_agent_legacy_config_is_upgraded_to_two_agents_without_losing_settings() {
    let legacy = AsrDailyAgentConfig {
        enabled: false,
        agent_id: "meeting_notes".to_string(),
        name: "meeting_notes".to_string(),
        runner: "codex_runner".to_string(),
        timeout_ms: 12345,
        trigger_policy: AsrDailyAgentTriggerPolicy::ManualOnly,
        session_key: Some("legacy-session".to_string()),
        instructions_source: AsrDailyAgentInstructionsSource::Custom,
        instructions: Some("legacy instructions".to_string()),
        im_delivery: AsrDailyAgentImDeliveryConfig::default(),
        output_dir: "meeting_notes".to_string(),
        agents: Vec::new(),
        terminology: Some("  Alpha 项目 = A  ".to_string()),
        report_sync_dir: Some("~/reports".to_string()),
        last_report_sync: None,
        last_run_at_ms: Some(11),
        last_status: Some("success".to_string()),
        last_error: None,
        last_run_id: Some("run-legacy".to_string()),
    };

    let normalized = normalize_daily_agent_config(&legacy);
    assert!(!normalized.enabled);
    assert_eq!(normalized.agents.len(), 2);
    assert_eq!(normalized.agents[0].id, "meeting_notes");
    assert_eq!(normalized.agents[0].runner, "codex_runner");
    assert_eq!(normalized.agents[0].trigger_policy, AsrDailyAgentTriggerPolicy::ManualOnly);
    assert_eq!(normalized.agents[0].instructions.as_deref(), Some("legacy instructions"));
    assert_eq!(normalized.agents[0].output_dir, "meeting_notes");
    assert_eq!(normalized.agents[0].last_run_id.as_deref(), Some("run-legacy"));
    assert_eq!(normalized.terminology.as_deref(), Some("Alpha 项目 = A"));
    assert_eq!(normalized.agents[1].id, DEFAULT_TOMORROW_TODO_AGENT_ID);
    assert!(normalized.agents[1].im_delivery.enabled);
}

#[test]
fn daily_agent_report_sync_dir_update_survives_task_normalization() {
    let mut config = AsrDailyAgentConfig::default();

    set_primary_daily_agent_report_sync_dir(
        &mut config,
        Some(" /tmp/bifrost-daily-agent-reports ".to_string()),
    );

    assert_eq!(
        config.report_sync_dir.as_deref(),
        Some("/tmp/bifrost-daily-agent-reports")
    );
    assert_eq!(
        config.agents[0].report_sync_dir.as_deref(),
        Some("/tmp/bifrost-daily-agent-reports")
    );
    assert_eq!(
        config.agents[1].report_sync_dir.as_deref(),
        Some("/tmp/bifrost-daily-agent-reports")
    );

    let normalized = normalize_daily_agent_config(&config);
    assert_eq!(
        normalized.report_sync_dir.as_deref(),
        Some("/tmp/bifrost-daily-agent-reports")
    );
    assert_eq!(
        normalized.agents[0].report_sync_dir.as_deref(),
        Some("/tmp/bifrost-daily-agent-reports")
    );
    assert_eq!(
        normalized.agents[1].report_sync_dir.as_deref(),
        Some("/tmp/bifrost-daily-agent-reports")
    );

    set_primary_daily_agent_report_sync_dir(&mut config, Some(" ".to_string()));
    let normalized = normalize_daily_agent_config(&config);
    assert_eq!(normalized.report_sync_dir, None);
    assert_eq!(normalized.agents[0].report_sync_dir, None);
    assert_eq!(normalized.agents[1].report_sync_dir, None);
}

#[test]
fn daily_agent_terminology_survives_normalization_and_agent_task_projection() {
    let mut config = AsrDailyAgentConfig {
        terminology: Some("  Jennie = 内部项目代号\nASR = 自动语音识别  ".to_string()),
        ..Default::default()
    };

    let normalized = normalize_daily_agent_config(&config);
    assert_eq!(
        normalized.terminology.as_deref(),
        Some("Jennie = 内部项目代号\nASR = 自动语音识别")
    );
    assert_eq!(normalized.agents.len(), 2);

    let task = AsrDirectoryTask {
        id: "daily-agent-terms-config-task".to_string(),
        name: "Daily Agent Terms Config Task".to_string(),
        audio_dir: PathBuf::new(),
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: normalized.clone(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    let tomorrow_task = task_for_daily_agent(&task, &normalized.agents[1]);
    assert_eq!(
        tomorrow_task.daily_agent.terminology.as_deref(),
        normalized.terminology.as_deref()
    );

    config.terminology = Some("   ".to_string());
    assert_eq!(normalize_daily_agent_config(&config).terminology, None);
}

#[test]
fn daily_agent_validation_rejects_non_english_tokens_and_duplicates() {
    let mut config = AsrDailyAgentConfig::default();
    config.agents[0].name = "中文".to_string();
    assert!(validate_daily_agent_config(&config)
        .unwrap_err()
        .contains("name must use English"));

    let mut config = AsrDailyAgentConfig::default();
    config.agents[1].output_dir = config.agents[0].output_dir.clone();
    assert!(validate_daily_agent_config(&config)
        .unwrap_err()
        .contains("Duplicate Daily Agent output_dir"));

    let mut config = AsrDailyAgentConfig::default();
    config.agents[1].id = config.agents[0].id.clone();
    assert!(validate_daily_agent_config(&config)
        .unwrap_err()
        .contains("Duplicate Daily Agent id"));
}

#[test]
fn daily_agent_workspace_creates_per_agent_instruction_and_output_dirs() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let mut task = AsrDirectoryTask {
        id: "daily-agent-multi-workspace-task".to_string(),
        name: "Daily Agent Multi Workspace Task".to_string(),
        audio_dir: temp.path().join("audio"),
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    task.daily_agent.terminology = Some("Jennie = 内部项目代号\nQwen3-ASR = 语音模型".to_string());

    ensure_asr_daily_workspace(&task).unwrap();
    let daily_dir = daily_dir_for_task(&task.id);

    std::fs::write(daily_dir.join("2026-05-22.md"), "daily source").unwrap();
    let status = ensure_asr_daily_workspace(&task).unwrap();

    assert!(daily_dir.join("agents/daily_report/input").is_dir());
    assert!(daily_dir.join("agents/daily_report/output/report").is_dir());
    assert!(daily_dir.join("agents/tomorrow_todo/input").is_dir());
    assert!(daily_dir.join("agents/tomorrow_todo/output/tomorrow_todo").is_dir());
    assert_eq!(
        std::fs::read_to_string(daily_dir.join("agents/daily_report/input/2026-05-22.md"))
            .unwrap(),
        "daily source"
    );
    assert_eq!(
        std::fs::read_to_string(daily_dir.join("agents/tomorrow_todo/input/2026-05-22.md"))
            .unwrap(),
        "daily source"
    );

    std::fs::write(daily_dir.join("2026-05-22.md"), "daily sourcf").unwrap();
    ensure_asr_daily_workspace(&task).unwrap();
    assert_eq!(
        std::fs::read_to_string(daily_dir.join("agents/daily_report/input/2026-05-22.md"))
            .unwrap(),
        "daily sourcf"
    );
    assert_eq!(
        std::fs::read_to_string(daily_dir.join("agents/tomorrow_todo/input/2026-05-22.md"))
            .unwrap(),
        "daily sourcf"
    );

    assert!(daily_dir.join("agents/daily_report/AGENTS.md").is_file());
    assert!(daily_dir.join("agents/tomorrow_todo/AGENTS.md").is_file());
    assert_eq!(
        std::fs::read_to_string(daily_dir.join("agents/daily_report/TERMS.md")).unwrap(),
        "Jennie = 内部项目代号\nQwen3-ASR = 语音模型\n"
    );
    assert_eq!(
        std::fs::read_to_string(daily_dir.join("agents/tomorrow_todo/TERMS.md")).unwrap(),
        "Jennie = 内部项目代号\nQwen3-ASR = 语音模型\n"
    );
    assert!(std::fs::read_to_string(daily_dir.join("agents/daily_report/AGENTS.md"))
        .unwrap()
        .contains("`TERMS.md`"));
    assert!(std::fs::read_to_string(daily_dir.join("agents/tomorrow_todo/AGENTS.md"))
        .unwrap()
        .contains("明日 To Do List"));
    assert_eq!(status.agents.len(), 2);
}

#[test]
fn daily_agent_source_copy_current_check_compares_content() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("2026-05-22.md");
    let target = temp.path().join("agent/input/2026-05-22.md");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();

    std::fs::write(&source, "same daily source").unwrap();
    assert!(!daily_agent_source_copy_is_current(&source, &target).unwrap());

    std::fs::write(&target, "same daily source").unwrap();
    assert!(daily_agent_source_copy_is_current(&source, &target).unwrap());

    std::fs::write(&target, "same daily sourcf").unwrap();
    assert!(!daily_agent_source_copy_is_current(&source, &target).unwrap());
}

#[test]
fn daily_agent_workspace_migrates_legacy_instruction_paths() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let task = AsrDirectoryTask {
        id: "daily-agent-legacy-instructions-task".to_string(),
        name: "Daily Agent Legacy Instructions Task".to_string(),
        audio_dir: temp.path().join("audio"),
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    let daily_dir = daily_dir_for_task(&task.id);
    let daily_report_path = daily_dir.join("agents/daily_report/AGENTS.md");
    let tomorrow_todo_path = daily_dir.join("agents/tomorrow_todo/AGENTS.md");
    std::fs::create_dir_all(daily_report_path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(tomorrow_todo_path.parent().unwrap()).unwrap();
    std::fs::write(
        &daily_report_path,
        "keep custom\n- 原始按日转写文件位于当前目录，命名通常为 `YYYY-MM-DD.md`。\n- 每日报告输出到当前目录下的 `report/` 文件夹，命名为 `YYYY-MM-DD-report.md`。\n",
    )
    .unwrap();
    std::fs::write(
        &tomorrow_todo_path,
        "keep todo\n- 源文件是当前目录根部的 `YYYY-MM-DD.md`。\n- 默认输出目录是 `./tomorrow_todo/`。\n",
    )
    .unwrap();

    ensure_asr_daily_workspace(&task).unwrap();

    let daily_report = std::fs::read_to_string(daily_report_path).unwrap();
    assert!(daily_report.contains("keep custom"));
    assert!(daily_report.contains("`input/YYYY-MM-DD.md`"));
    assert!(daily_report.contains("`./output/report`"));
    assert!(!daily_report.contains("当前目录，命名通常为 `YYYY-MM-DD.md`"));

    let tomorrow_todo = std::fs::read_to_string(tomorrow_todo_path).unwrap();
    assert!(tomorrow_todo.contains("keep todo"));
    assert!(tomorrow_todo.contains("`input/YYYY-MM-DD.md`"));
    assert!(tomorrow_todo.contains("`./output/tomorrow_todo/`"));
    assert!(!tomorrow_todo.contains("当前目录根部的 `YYYY-MM-DD.md`"));
}

#[test]
fn daily_agent_processed_state_keys_do_not_collide_for_same_date() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let mut task = AsrDirectoryTask {
        id: "daily-agent-multi-records-task".to_string(),
        name: "Daily Agent Multi Records Task".to_string(),
        audio_dir: temp.path().join("audio"),
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    ensure_asr_daily_workspace(&task).unwrap();
    let daily_dir = daily_dir_for_task(&task.id);
    std::fs::write(daily_dir.join("2026-05-22.md"), "source").unwrap();
    std::fs::write(
        daily_dir.join("agents/daily_report/output/report/2026-05-22-report.md"),
        "# report",
    )
    .unwrap();
    std::fs::write(
        daily_dir.join("agents/tomorrow_todo/output/tomorrow_todo/2026-05-22-report.md"),
        "# todo",
    )
    .unwrap();

    let mut processed = AsrDailyAgentProcessedState::default();
    for agent in normalized_daily_agents(&task.daily_agent) {
        let agent_task = task_for_daily_agent(&task, &agent);
        let report_path = daily_agent_output_dir(&agent_task).join("2026-05-22-report.md");
        processed.documents.insert(
            daily_agent_processed_key(&agent_task, "2026-05-22"),
            AsrDailyAgentProcessedDocument {
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                output_dir: agent.output_dir.clone(),
                date: "2026-05-22".to_string(),
                source_sha256: "hash".to_string(),
                source_len_bytes: 6,
                processed_at_ms: 100,
                runner: agent.runner.clone(),
                report_path: Some(report_path.to_string_lossy().to_string()),
                last_run_id: format!("run-{}", agent.id),
            },
        );
    }

    let records = build_daily_agent_records_for_task(&task, &processed);
    task.daily_agent.agents.reverse();
    let reversed_records = build_daily_agent_records_for_task(&task, &processed);

    assert_eq!(records.len(), 2);
    assert!(records.iter().any(|record| record.agent_id == "daily_report"));
    assert!(records.iter().any(|record| record.agent_id == "tomorrow_todo"));
    assert_eq!(reversed_records.len(), 2);
}

#[test]
fn daily_agent_default_template_keeps_knowledge_modules_inside_report() {
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("## 报告内知识沉淀模块"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("## 知识沉淀输出要求"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD
        .contains("这份指南会作为当前 Daily Agent 工作目录中的 `AGENTS.md` 写入"));
    assert!(
        DEFAULT_ASR_DAILY_AGENTS_MD.contains("下面所有规则都是运行指令，不是注释、示例或可忽略说明")
    );
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("不能省略会影响输出契约的核心模块"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("同一份 `{{report_dir}}YYYY-MM-DD-report.md`"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("### 长期想法与效率方案"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("### 方向决策与判断"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("### 跨天待办追踪"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("资料搜索结果"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("可行性分析"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("方案草案"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("不是装饰性标题"));
    assert!(!DEFAULT_ASR_DAILY_AGENTS_MD.contains("`knowledge/"));
}

#[test]
fn daily_agent_prompt_uses_file_list_for_file_capable_runners() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let audio_dir = temp.path().join("audio");
    std::fs::create_dir_all(&audio_dir).unwrap();
    let mut task = AsrDirectoryTask {
        id: "daily-agent-prompt-task".to_string(),
        name: "Daily Agent Prompt Task".to_string(),
        audio_dir,
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    task.daily_agent.runner = "codex-runner".to_string();
    task.daily_agent.terminology = Some("Jennie = 内部项目代号\nBeta 客户 = 测试客户".to_string());
    ensure_asr_daily_workspace(&task).unwrap();
    std::fs::write(
        daily_dir_for_task(&task.id).join("2026-05-19.md"),
        "今日新增转写内容",
    )
    .unwrap();

    let plan = build_daily_agent_change_plan(&task, "test", None, false).unwrap();
    let codex_prompt = build_daily_agent_prompt(&task, &plan, "codex", false).unwrap();
    assert!(codex_prompt.contains("2026-05-19.md"));
    assert!(codex_prompt.contains("change_kind=NewFile"));
    assert!(codex_prompt.contains("专有名词文件 `TERMS.md`"));
    assert!(!codex_prompt.contains("Jennie = 内部项目代号"));
    assert!(!codex_prompt.contains("AGENTS.md 内容"));
    assert!(!codex_prompt.contains("变更文件内容"));

    let chatgpt_first = build_daily_agent_prompt(&task, &plan, "chatgpt_web", true).unwrap();
    assert!(chatgpt_first.starts_with("## 专有名词配置（每次运行动态注入）"));
    assert!(chatgpt_first.contains("Jennie = 内部项目代号"));
    assert!(chatgpt_first.contains("AGENTS.md 内容"));
    assert!(chatgpt_first.contains("今日新增转写内容"));

    let chatgpt_next = build_daily_agent_prompt(&task, &plan, "chatgpt_web", false).unwrap();
    assert!(chatgpt_next.starts_with("## 专有名词配置（每次运行动态注入）"));
    assert!(chatgpt_next.contains("Jennie = 内部项目代号"));
    assert!(!chatgpt_next.contains("AGENTS.md 内容"));
    assert!(chatgpt_next.contains("后续轮次"));
    assert!(chatgpt_next.contains("今日新增转写内容"));
}

#[test]
fn daily_agent_change_plan_filters_to_requested_date() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let audio_dir = temp.path().join("audio");
    std::fs::create_dir_all(&audio_dir).unwrap();
    let task = AsrDirectoryTask {
        id: "daily-agent-date-filter-task".to_string(),
        name: "Daily Agent Date Filter Task".to_string(),
        audio_dir,
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    ensure_asr_daily_workspace(&task).unwrap();
    let daily_dir = daily_dir_for_task(&task.id);
    std::fs::write(daily_dir.join("2026-05-18.md"), "older daily doc").unwrap();
    std::fs::write(daily_dir.join("2026-05-19.md"), "selected daily doc").unwrap();

    let plan = build_daily_agent_change_plan(&task, "manual", Some("2026-05-19"), false).unwrap();

    assert!(!plan.skipped);
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.entries[0].date, "2026-05-19");
    assert_path_ends_with(
        &plan.entries[0].report_target,
        &[
            "daily-agent-date-filter-task",
            ".daily",
            "agents",
            "daily_report",
            "output",
            "report",
            "2026-05-19-report.md",
        ],
    );
}

#[tokio::test]
async fn reuse_server_failure_records_chunk_and_schedules_restart() {
    let temp = TempDir::new().unwrap();
    let chunk_path = temp.path().join("chunk.wav");
    std::fs::write(&chunk_path, make_wav(&[500i16; 16_000])).unwrap();

    let mut server_state = Some(ServerRunnerState {
        server_url: "test-error:connection refused by watchdog".to_string(),
        baseline_rtf: None,
        baseline_samples: Vec::new(),
        server_failures: 0,
        force_fork_for_remaining: false,
        restart_required: false,
        current_chunk_failure_reason: None,
        fallback_reason: None,
    });

    let first = run_chunk_with_strategy(
        AsrRuntimeStrategy::ReuseServer,
        Path::new("/nonexistent/asr"),
        Path::new("/nonexistent/model"),
        "chinese",
        &chunk_path,
        0,
        1,
        0,
        temp.path(),
        None,
        &mut server_state,
        None,
        None,
    )
    .await
    .unwrap();

    let fallback_reason = server_state
        .as_ref()
        .and_then(|state| state.fallback_reason.as_deref())
        .map(str::to_string)
        .unwrap();
    assert!(server_state
        .as_ref()
        .is_some_and(|state| !state.force_fork_for_remaining && state.restart_required));
    assert!(fallback_reason.contains("reuse_server strategy transport failure"));
    assert_eq!(first.metric.runner, "fork_per_chunk");
    assert_eq!(first.metric.status, "error");
    assert_eq!(
        first.metric.fallback_reason.as_deref(),
        Some(fallback_reason.as_str())
    );
    assert_eq!(first.shadow_metrics.len(), 1);
    assert_eq!(first.shadow_metrics[0].runner, "reuse_server");
    assert_eq!(first.shadow_metrics[0].status, "error");

    if let Some(state) = server_state.as_mut() {
        state.server_url = "test-ok:restarted-server".to_string();
        state.restart_required = false;
        state.current_chunk_failure_reason = None;
    }
    let second = run_chunk_with_strategy(
        AsrRuntimeStrategy::ReuseServer,
        Path::new("/nonexistent/asr"),
        Path::new("/nonexistent/model"),
        "chinese",
        &chunk_path,
        1,
        1,
        1,
        temp.path(),
        None,
        &mut server_state,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(second.metric.runner, "reuse_server");
    assert_eq!(second.metric.status, "ok");
    assert_eq!(second.metric.server_url.as_deref(), Some("test-ok:restarted-server"));
    assert_eq!(second.metric.fallback_reason.as_deref(), None);
    assert!(second.shadow_metrics.is_empty());
}

#[tokio::test]
async fn reuse_server_failure_threshold_forces_remaining_fork_isolation() {
    let temp = TempDir::new().unwrap();
    let chunk_path = temp.path().join("chunk.wav");
    std::fs::write(&chunk_path, make_wav(&[500i16; 16_000])).unwrap();

    let mut server_state = Some(ServerRunnerState {
        server_url: "test-error:connection refused by watchdog".to_string(),
        baseline_rtf: None,
        baseline_samples: Vec::new(),
        server_failures: max_server_failures_for_strategy(AsrRuntimeStrategy::ReuseServer) - 1,
        force_fork_for_remaining: false,
        restart_required: false,
        current_chunk_failure_reason: None,
        fallback_reason: None,
    });

    let attempt = run_chunk_with_strategy(
        AsrRuntimeStrategy::ReuseServer,
        Path::new("/nonexistent/asr"),
        Path::new("/nonexistent/model"),
        "chinese",
        &chunk_path,
        0,
        1,
        0,
        temp.path(),
        None,
        &mut server_state,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(attempt.metric.runner, "fork_per_chunk");
    assert_eq!(attempt.metric.status, "error");
    assert_eq!(attempt.shadow_metrics.len(), 1);
    assert_eq!(attempt.shadow_metrics[0].runner, "reuse_server");
    assert_eq!(attempt.shadow_metrics[0].status, "error");
    let state = server_state.as_ref().unwrap();
    assert!(state.force_fork_for_remaining);
    assert!(!state.restart_required);
    assert!(state
        .fallback_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("switching remaining chunks to fork_per_chunk isolation")));
}

#[tokio::test]
async fn restart_failure_records_current_chunk_and_keeps_retry_pending() {
    let temp = TempDir::new().unwrap();
    let chunk_path = temp.path().join("chunk.wav");
    std::fs::write(&chunk_path, make_wav(&[500i16; 16_000])).unwrap();

    let mut server_state = Some(ServerRunnerState {
        server_url: "test-ok:must-not-be-called-after-restart-failure".to_string(),
        baseline_rtf: None,
        baseline_samples: Vec::new(),
        server_failures: 0,
        force_fork_for_remaining: false,
        restart_required: true,
        current_chunk_failure_reason: Some(
            "reuse_per_file strategy managed ASR server restart failed; recording current chunk as failed"
                .to_string(),
        ),
        fallback_reason: None,
    });

    let attempt = run_chunk_with_strategy(
        AsrRuntimeStrategy::ReusePerFile,
        Path::new("/nonexistent/asr"),
        Path::new("/nonexistent/model"),
        "chinese",
        &chunk_path,
        0,
        1,
        0,
        temp.path(),
        None,
        &mut server_state,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(attempt.metric.runner, "reuse_server");
    assert_eq!(attempt.metric.status, "error");
    assert!(attempt.shadow_metrics.is_empty());
    assert!(attempt
        .metric
        .fallback_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("managed ASR server restart failed")));
    let state = server_state.as_ref().unwrap();
    assert!(state.restart_required);
    assert!(!state.force_fork_for_remaining);
    assert!(state.current_chunk_failure_reason.is_none());
}

#[test]
fn daily_agent_report_gate_requires_report_before_processed_state() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let audio_dir = temp.path().join("audio");
    std::fs::create_dir_all(&audio_dir).unwrap();
    let task = AsrDirectoryTask {
        id: "daily-agent-report-gate-task".to_string(),
        name: "Daily Agent Report Gate Task".to_string(),
        audio_dir,
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    ensure_asr_daily_workspace(&task).unwrap();
    std::fs::write(daily_dir_for_task(&task.id).join("2026-05-19.md"), "text").unwrap();

    let plan = build_daily_agent_change_plan(&task, "test", None, false).unwrap();
    let (generated, missing) = collect_report_outputs_for_plan(&plan);
    assert!(generated.is_empty());
    assert_eq!(missing.len(), 1);

    std::fs::write(&missing[0], "# report").unwrap();
    let (generated, missing) = collect_report_outputs_for_plan(&plan);
    assert_eq!(generated.len(), 1);
    assert!(missing.is_empty());
}

#[test]
fn daily_agent_report_detail_path_is_date_scoped() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());

    let report_path =
        daily_agent_report_path_for_date("daily-agent-report-task", "2026-05-14").unwrap();
    assert_path_ends_with(
        &report_path,
        &[
            "daily-agent-report-task",
            ".daily",
            "report",
            "2026-05-14-report.md",
        ],
    );
    assert!(daily_agent_report_path_for_date("daily-agent-report-task", "../secret").is_err());
    assert!(daily_agent_report_path_for_date("daily-agent-report-task", "2026-02-31").is_err());
}

#[test]
fn daily_agent_report_detail_uses_processed_state_report_path() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let task_id = "daily-agent-state-report-task";
    let legacy_report_dir = text_output_dir(temp.path()).join(task_id).join("daily/report");
    std::fs::create_dir_all(&legacy_report_dir).unwrap();
    let legacy_report_path = legacy_report_dir.join("2026-05-20-report.md");
    std::fs::write(&legacy_report_path, "# state report").unwrap();

    let mut processed = AsrDailyAgentProcessedState::default();
    processed.documents.insert(
        "2026-05-20".to_string(),
        AsrDailyAgentProcessedDocument {
            agent_id: DEFAULT_DAILY_AGENT_ID.to_string(),
            agent_name: DEFAULT_DAILY_AGENT_NAME.to_string(),
            output_dir: DEFAULT_DAILY_AGENT_OUTPUT_DIR.to_string(),
            date: "2026-05-20".to_string(),
            source_sha256: "abc123".to_string(),
            source_len_bytes: 42,
            processed_at_ms: 100,
            runner: "web".to_string(),
            report_path: Some(legacy_report_path.to_string_lossy().to_string()),
            last_run_id: "run-1".to_string(),
        },
    );
    atomic_json_write(&daily_agent_processed_state_path(task_id), &processed).unwrap();

    let detail_path = daily_agent_report_path_for_date(task_id, "2026-05-20").unwrap();
    assert_eq!(detail_path, legacy_report_path);
}

#[test]
fn daily_agent_records_include_existing_report_directory_without_processed_state() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let task_id = "daily-agent-records-report-dir-task";
    let daily_dir = daily_dir_for_task(task_id);
    let report_dir = daily_dir.join("Report");
    std::fs::create_dir_all(&report_dir).unwrap();
    std::fs::write(daily_dir.join("2026-05-14.md"), "source text").unwrap();
    std::fs::write(report_dir.join("2026-05-14-report.md"), "# report").unwrap();
    std::fs::write(report_dir.join("notes.md"), "# ignore").unwrap();

    let processed = load_daily_agent_processed_state(task_id);
    let records = build_daily_agent_records(task_id, &processed);

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].date, "2026-05-14");
    assert_eq!(records[0].runner, "filesystem");
    assert_eq!(records[0].last_run_id, "filesystem-scan");
    assert!(records[0].source_len_bytes > 0);
    assert_path_ends_with(
        records[0].report_path.as_deref().unwrap(),
        &[".daily", "Report", "2026-05-14-report.md"],
    );

    let detail_path = daily_agent_report_path_for_date(task_id, "2026-05-14").unwrap();
    assert_path_ends_with(
        &detail_path,
        &[".daily", "Report", "2026-05-14-report.md"],
    );
}

#[test]
fn daily_agent_records_for_task_use_configured_runner_for_unindexed_reports() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let task_id = "daily-agent-records-configured-runner-task";
    let daily_dir = daily_dir_for_task(task_id);
    let report_dir = daily_dir.join("report");
    std::fs::create_dir_all(&report_dir).unwrap();
    std::fs::write(daily_dir.join("2026-05-18.md"), "source text").unwrap();
    std::fs::write(report_dir.join("2026-05-18-report.md"), "# report").unwrap();

    let mut task = AsrDirectoryTask {
        id: task_id.to_string(),
        name: "Daily Agent Records Configured Runner Task".to_string(),
        audio_dir: temp.path().join("audio"),
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    task.daily_agent.runner = "web".to_string();

    let processed = load_daily_agent_processed_state(task_id);
    let records = build_daily_agent_records_for_task(&task, &processed);

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].date, "2026-05-18");
    assert_eq!(records[0].runner, "web");
    assert_eq!(records[0].last_run_id, "filesystem-scan");
}

#[test]
fn daily_agent_report_index_status_marks_unindexed_reports_without_backfill() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let task_id = "daily-agent-report-index-task";
    let daily_dir = daily_dir_for_task(task_id);
    let report_dir = daily_dir.join("report");
    std::fs::create_dir_all(&report_dir).unwrap();
    std::fs::write(daily_dir.join("2026-05-14.md"), "indexed source").unwrap();
    std::fs::write(daily_dir.join("2026-05-15.md"), "external source").unwrap();
    std::fs::write(report_dir.join("2026-05-14-report.md"), "# indexed report").unwrap();
    std::fs::write(report_dir.join("2026-05-15-report.md"), "# external report").unwrap();

    let mut processed = AsrDailyAgentProcessedState::default();
    processed.documents.insert(
        "2026-05-14".to_string(),
        AsrDailyAgentProcessedDocument {
            agent_id: DEFAULT_DAILY_AGENT_ID.to_string(),
            agent_name: DEFAULT_DAILY_AGENT_NAME.to_string(),
            output_dir: DEFAULT_DAILY_AGENT_OUTPUT_DIR.to_string(),
            date: "2026-05-14".to_string(),
            source_sha256: "abc123".to_string(),
            source_len_bytes: 14,
            processed_at_ms: 100,
            runner: "web".to_string(),
            report_path: Some(
                report_dir
                    .join("2026-05-14-report.md")
                    .to_string_lossy()
                    .to_string(),
            ),
            last_run_id: "run-1".to_string(),
        },
    );

    let state_path = daily_agent_processed_state_path(task_id);
    assert!(!state_path.exists());

    let status = build_daily_agent_report_index_status(task_id, &processed);

    assert_eq!(status.report_files, 2);
    assert_eq!(status.processed_documents, 1);
    assert_eq!(status.indexed_reports, 1);
    assert_eq!(status.unindexed_reports, 1);
    assert_eq!(status.processed_missing_report, 0);
    assert_eq!(status.unindexed_dates, vec!["2026-05-15".to_string()]);
    assert!(
        !state_path.exists(),
        "report index status must not backfill processed state"
    );
}

#[test]
fn daily_agent_report_sync_copies_reports_into_agent_subdirectories() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let task_id = "daily-agent-report-sync-task";
    let sync_dir = temp.path().join("icloud-sync");
    std::fs::create_dir_all(&sync_dir).unwrap();

    let mut task = AsrDirectoryTask {
        id: task_id.to_string(),
        name: "Daily Agent Report Sync Task".to_string(),
        audio_dir: temp.path().join("audio"),
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    set_primary_daily_agent_report_sync_dir(
        &mut task.daily_agent,
        Some(sync_dir.to_string_lossy().to_string()),
    );
    ensure_asr_daily_workspace(&task).unwrap();

    let daily_report_agent = normalized_daily_agents(&task.daily_agent)
        .into_iter()
        .find(|agent| agent.id == "daily_report")
        .unwrap();
    let daily_report_task = task_for_daily_agent(&task, &daily_report_agent);
    let daily_report_path = daily_agent_output_dir(&daily_report_task).join("2026-05-14-report.md");
    std::fs::write(&daily_report_path, "report one").unwrap();
    std::fs::create_dir_all(sync_dir.join("daily_report")).unwrap();
    std::fs::write(sync_dir.join("daily_report/2026-05-14-report.md"), "report one").unwrap();

    let tomorrow_agent = normalized_daily_agents(&task.daily_agent)
        .into_iter()
        .find(|agent| agent.id == "tomorrow_todo")
        .unwrap();
    let tomorrow_task = task_for_daily_agent(&task, &tomorrow_agent);
    let tomorrow_report_path = daily_agent_output_dir(&tomorrow_task).join("2026-05-14-report.md");
    std::fs::write(&tomorrow_report_path, "todo one").unwrap();

    let daily_result = sync_daily_agent_report_files(
        &daily_report_task,
        &[daily_report_path.to_string_lossy().to_string()],
    )
    .unwrap();
    let tomorrow_result = sync_daily_agent_report_files(
        &tomorrow_task,
        &[tomorrow_report_path.to_string_lossy().to_string()],
    )
    .unwrap();

    assert_eq!(daily_result.total_files, 1);
    assert_eq!(daily_result.copied_files, 1);
    assert_eq!(daily_result.skipped_files, 0);
    assert_eq!(daily_result.failed_files, 0);
    assert_eq!(
        daily_result.target_dir,
        sync_dir.join("daily_report").to_string_lossy()
    );
    assert_eq!(tomorrow_result.total_files, 1);
    assert_eq!(tomorrow_result.copied_files, 1);
    assert_eq!(tomorrow_result.skipped_files, 0);
    assert_eq!(tomorrow_result.failed_files, 0);
    assert_eq!(
        tomorrow_result.target_dir,
        sync_dir.join("tomorrow_todo").to_string_lossy()
    );
    assert_eq!(
        std::fs::read_to_string(sync_dir.join("daily_report/2026-05-14-report.md")).unwrap(),
        "report one"
    );
    assert_eq!(
        std::fs::read_to_string(sync_dir.join("tomorrow_todo/2026-05-14-report.md")).unwrap(),
        "todo one"
    );
    assert!(!sync_dir.join("2026-05-14-report.md").exists());
}

#[cfg(unix)]
#[test]
fn daily_agent_report_sync_overwrites_unreadable_target_without_reading_target_hash() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let task_id = "daily-agent-report-sync-unreadable-target-task";
    let sync_dir = temp.path().join("icloud-sync");
    std::fs::create_dir_all(&sync_dir).unwrap();

    let mut task = AsrDirectoryTask {
        id: task_id.to_string(),
        name: "Daily Agent Report Sync Unreadable Target Task".to_string(),
        audio_dir: temp.path().join("audio"),
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    set_primary_daily_agent_report_sync_dir(
        &mut task.daily_agent,
        Some(sync_dir.to_string_lossy().to_string()),
    );
    ensure_asr_daily_workspace(&task).unwrap();

    let daily_report_agent = normalized_daily_agents(&task.daily_agent)
        .into_iter()
        .find(|agent| agent.id == "daily_report")
        .unwrap();
    let daily_report_task = task_for_daily_agent(&task, &daily_report_agent);
    let daily_report_path = daily_agent_output_dir(&daily_report_task).join("2026-05-14-report.md");
    std::fs::write(&daily_report_path, "fresh report").unwrap();

    let target_dir = sync_dir.join("daily_report");
    std::fs::create_dir_all(&target_dir).unwrap();
    let target_path = target_dir.join("2026-05-14-report.md");
    std::fs::write(&target_path, "stale report").unwrap();
    std::fs::set_permissions(&target_path, std::fs::Permissions::from_mode(0o000)).unwrap();

    let result = sync_daily_agent_report_files(
        &daily_report_task,
        &[daily_report_path.to_string_lossy().to_string()],
    )
    .unwrap();

    assert_eq!(result.total_files, 1);
    assert_eq!(result.copied_files, 1);
    assert_eq!(result.skipped_files, 0);
    assert_eq!(result.failed_files, 0);
    assert_eq!(std::fs::read_to_string(&target_path).unwrap(), "fresh report");
}

#[test]
fn daily_agent_report_sync_auto_after_generation_uses_isolated_copy_path() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let task_id = "daily-agent-auto-sync-task";
    let sync_dir = temp.path().join("icloud-sync");
    std::fs::create_dir_all(&sync_dir).unwrap();

    let mut task = test_directory_task(task_id, temp.path().join("audio"));
    set_primary_daily_agent_report_sync_dir(
        &mut task.daily_agent,
        Some(sync_dir.to_string_lossy().to_string()),
    );
    ensure_asr_daily_workspace(&task).unwrap();
    save_tasks(&TaskStore {
        version: TASK_STORE_VERSION,
        tasks: vec![task.clone()],
    })
    .unwrap();

    let daily_report_agent = normalized_daily_agents(&task.daily_agent)
        .into_iter()
        .find(|agent| agent.id == "daily_report")
        .unwrap();
    let daily_report_task = task_for_daily_agent(&task, &daily_report_agent);
    let daily_report_path = daily_agent_output_dir(&daily_report_task).join("2026-05-14-report.md");
    std::fs::write(&daily_report_path, "fresh auto report").unwrap();

    let target_dir = sync_dir.join("daily_report");
    std::fs::create_dir_all(&target_dir).unwrap();
    let target_path = target_dir.join("2026-05-14-report.md");
    std::fs::write(&target_path, "stale auto report").unwrap();

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(sync_daily_agent_reports_after_generation(
            &daily_report_task,
            &[daily_report_path.to_string_lossy().to_string()],
        ))
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(&target_path).unwrap(),
        "fresh auto report"
    );
    let store = load_tasks();
    let stored_task = store.tasks.iter().find(|task| task.id == task_id).unwrap();
    let stored_agent = normalized_daily_agents(&stored_task.daily_agent)
        .into_iter()
        .find(|agent| agent.id == "daily_report")
        .unwrap();
    let sync = stored_agent.last_report_sync.unwrap();
    assert_eq!(sync.target_dir, target_dir.to_string_lossy());
    assert_eq!(sync.total_files, 1);
    assert_eq!(sync.copied_files, 1);
    assert_eq!(sync.skipped_files, 0);
    assert_eq!(sync.failed_files, 0);
}

#[test]
fn daily_agent_report_sync_requires_configured_directory() {
    let temp = TempDir::new().unwrap();
    let task = AsrDirectoryTask {
        id: "daily-agent-report-sync-missing-dir-task".to_string(),
        name: "Daily Agent Report Sync Missing Dir Task".to_string(),
        audio_dir: temp.path().join("audio"),
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };

    let error = sync_daily_agent_report_files(&task, &[]).unwrap_err();
    assert!(error.contains("report sync directory is not configured"));
}

#[test]
fn daily_agent_watch_summary_counts_processed_pending_and_report_only_documents() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let task_id = "daily-agent-watch-summary-task";
    let daily_dir = daily_dir_for_task(task_id);
    let report_dir = daily_dir.join("report");
    std::fs::create_dir_all(&report_dir).unwrap();

    let processed_source = daily_dir.join("2026-05-20.md");
    let pending_source = daily_dir.join("2026-05-21.md");
    std::fs::write(&processed_source, "processed source").unwrap();
    std::fs::write(&pending_source, "pending source v2").unwrap();
    let processed_report = report_dir.join("2026-05-20-report.md");
    let report_only = report_dir.join("2026-05-19-report.md");
    std::fs::write(&processed_report, "# processed report").unwrap();
    std::fs::write(&report_only, "# report only").unwrap();

    let mut processed = AsrDailyAgentProcessedState::default();
    processed.documents.insert(
        "2026-05-20".to_string(),
        AsrDailyAgentProcessedDocument {
            agent_id: DEFAULT_DAILY_AGENT_ID.to_string(),
            agent_name: DEFAULT_DAILY_AGENT_NAME.to_string(),
            output_dir: DEFAULT_DAILY_AGENT_OUTPUT_DIR.to_string(),
            date: "2026-05-20".to_string(),
            source_sha256: compute_sha256(&processed_source).unwrap(),
            source_len_bytes: source_size(&processed_source).unwrap(),
            processed_at_ms: 100,
            runner: "codex".to_string(),
            report_path: Some(processed_report.to_string_lossy().to_string()),
            last_run_id: "run-processed".to_string(),
        },
    );
    processed.documents.insert(
        "2026-05-21".to_string(),
        AsrDailyAgentProcessedDocument {
            agent_id: DEFAULT_DAILY_AGENT_ID.to_string(),
            agent_name: DEFAULT_DAILY_AGENT_NAME.to_string(),
            output_dir: DEFAULT_DAILY_AGENT_OUTPUT_DIR.to_string(),
            date: "2026-05-21".to_string(),
            source_sha256: "old-hash".to_string(),
            source_len_bytes: 4,
            processed_at_ms: 90,
            runner: "codex".to_string(),
            report_path: Some(
                report_dir
                    .join("2026-05-21-report.md")
                    .to_string_lossy()
                    .to_string(),
            ),
            last_run_id: "run-old".to_string(),
        },
    );
    save_daily_agent_processed_state(task_id, &processed).unwrap();

    let mut task = test_directory_task(task_id, temp.path().join("audio"));
    task.daily_agent.enabled = true;
    task.daily_agent.runner = "codex".to_string();

    let summary = task_watch_daily_agent(&task, 8);

    assert_eq!(summary.daily_files, 2);
    assert_eq!(summary.processed_documents, 1);
    assert_eq!(summary.pending_documents, 3);
    assert_eq!(summary.report_files, 2);
    assert_eq!(summary.indexed_reports, 1);
    assert_eq!(summary.unindexed_reports, 1);
    assert!(summary
        .recent_documents
        .iter()
        .any(|document| document.date == "2026-05-21" && document.status == "pending"));
    assert!(summary
        .recent_documents
        .iter()
        .any(|document| document.date == "2026-05-20" && document.status == "processed"));
    assert!(summary
        .recent_documents
        .iter()
        .any(|document| document.date == "2026-05-19" && document.status == "report_only"));
}

#[test]
fn daily_agent_records_preserve_processed_metadata_and_repair_missing_report_path() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let task_id = "daily-agent-records-merge-task";
    let report_dir = daily_dir_for_task(task_id).join("report");
    std::fs::create_dir_all(&report_dir).unwrap();
    std::fs::write(report_dir.join("2026-05-15-report.md"), "# report").unwrap();

    let mut processed = AsrDailyAgentProcessedState::default();
    processed.documents.insert(
        "2026-05-15".to_string(),
        AsrDailyAgentProcessedDocument {
            agent_id: DEFAULT_DAILY_AGENT_ID.to_string(),
            agent_name: DEFAULT_DAILY_AGENT_NAME.to_string(),
            output_dir: DEFAULT_DAILY_AGENT_OUTPUT_DIR.to_string(),
            date: "2026-05-15".to_string(),
            source_sha256: "abc123".to_string(),
            source_len_bytes: 42,
            processed_at_ms: 100,
            runner: "web".to_string(),
            report_path: None,
            last_run_id: "run-1".to_string(),
        },
    );

    let records = build_daily_agent_records(task_id, &processed);

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].runner, "web");
    assert_eq!(records[0].last_run_id, "run-1");
    assert_eq!(records[0].processed_at_ms, 100);
    assert_eq!(records[0].source_sha256, "abc123");
    assert_path_ends_with(
        records[0].report_path.as_deref().unwrap(),
        &[".daily", "report", "2026-05-15-report.md"],
    );
}

#[test]
fn daily_agent_records_are_returned_newest_date_first() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let task_id = "daily-agent-records-sort-task";
    let mut processed = AsrDailyAgentProcessedState::default();

    for (date, processed_at_ms) in [
        ("2026-05-14", 100),
        ("2026-05-16", 300),
        ("2026-05-15", 200),
    ] {
        processed.documents.insert(
            date.to_string(),
            AsrDailyAgentProcessedDocument {
                agent_id: DEFAULT_DAILY_AGENT_ID.to_string(),
                agent_name: DEFAULT_DAILY_AGENT_NAME.to_string(),
                output_dir: DEFAULT_DAILY_AGENT_OUTPUT_DIR.to_string(),
                date: date.to_string(),
                source_sha256: format!("hash-{date}"),
                source_len_bytes: 42,
                processed_at_ms,
                runner: "web".to_string(),
                report_path: None,
                last_run_id: format!("run-{date}"),
            },
        );
    }

    let records = build_daily_agent_records(task_id, &processed);

    assert_eq!(
        records
            .iter()
            .map(|record| record.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-05-16", "2026-05-15", "2026-05-14"]
    );
}

#[test]
fn daily_agent_runner_is_single_required_value() {
    let mut task = AsrDirectoryTask {
        id: "daily-agent-ready-task".to_string(),
        name: "Daily Agent Ready Task".to_string(),
        audio_dir: PathBuf::from("/tmp"),
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    task.daily_agent.runner = String::new();
    assert!(!daily_agent_runner_ready(&task));

    task.daily_agent.runner = "bifrost_agent".to_string();
    assert!(daily_agent_runner_ready(&task));

    task.daily_agent.runner = "codex-runner".to_string();
    assert!(daily_agent_runner_ready(&task));
    assert_eq!(daily_agent_external_runner_id(&task), Some("codex-runner"));
}

#[test]
fn daily_agent_after_asr_run_requires_changed_daily_markdown() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let audio_dir = temp.path().join("audio");
    std::fs::create_dir_all(&audio_dir).unwrap();
    let task = test_directory_task("daily-agent-asr-completion-task", audio_dir);
    ensure_asr_daily_workspace(&task).unwrap();
    let agents = normalized_daily_agents(&task.daily_agent);

    assert!(!daily_agent_has_changed_daily_markdown(&task, &agents).unwrap());

    let daily_path = daily_dir_for_task(&task.id).join("2026-06-03.md");
    std::fs::write(&daily_path, "initial daily transcript").unwrap();
    assert!(daily_agent_has_changed_daily_markdown(&task, &agents).unwrap());

    let source_sha256 = compute_sha256(&daily_path).unwrap();
    let source_len_bytes = std::fs::metadata(&daily_path).unwrap().len();
    let mut processed = AsrDailyAgentProcessedState::default();
    for agent in &agents {
        let agent_task = task_for_daily_agent(&task, agent);
        processed.documents.insert(
            daily_agent_processed_key(&agent_task, "2026-06-03"),
            AsrDailyAgentProcessedDocument {
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                output_dir: agent.output_dir.clone(),
                date: "2026-06-03".to_string(),
                source_sha256: source_sha256.clone(),
                source_len_bytes,
                processed_at_ms: 1,
                runner: agent.runner.clone(),
                report_path: None,
                last_run_id: "previous-run".to_string(),
            },
        );
    }
    save_daily_agent_processed_state(&task.id, &processed).unwrap();
    assert!(!daily_agent_has_changed_daily_markdown(&task, &agents).unwrap());

    std::fs::write(&daily_path, "initial daily transcript\nnew appended text").unwrap();
    assert!(daily_agent_has_changed_daily_markdown(&task, &agents).unwrap());
}

#[test]
fn daily_agent_after_asr_run_ignores_failed_files_when_daily_markdown_changed() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let audio_dir = temp.path().join("audio");
    std::fs::create_dir_all(&audio_dir).unwrap();
    let task = test_directory_task("daily-agent-failed-files-with-daily-changes", audio_dir.clone());
    ensure_asr_daily_workspace(&task).unwrap();
    std::fs::write(
        daily_dir_for_task(&task.id).join("2026-06-03.md"),
        "daily markdown was refreshed even though some audio files failed",
    )
    .unwrap();

    let source_path = audio_dir.join("normalize-failed.wav");
    std::fs::write(&source_path, b"audio").unwrap();
    let mut record = file_record_from_info(
        &task.id,
        &source_path,
        &SourceAudioInfo {
            source_size: Some(5),
            source_modified_ms: Some(1),
            source_created_at_ms: None,
            source_created_at_source: None,
            media_duration_ms: Some(1_000),
        },
    );
    record.status = FileStatus::Failed;
    record.error = Some("normalize failed: unsupported audio".to_string());
    let no_asr_units_path = audio_dir.join("no-asr-units.wav");
    std::fs::write(&no_asr_units_path, b"audio").unwrap();
    let mut no_asr_units = file_record_from_info(
        &task.id,
        &no_asr_units_path,
        &SourceAudioInfo {
            source_size: Some(5),
            source_modified_ms: Some(1),
            source_created_at_ms: None,
            source_created_at_source: None,
            media_duration_ms: Some(1_000),
        },
    );
    no_asr_units.status = FileStatus::Failed;
    no_asr_units.error = Some(
        "diarization_no_asr_units: diarization produced no transcribable ASR units".to_string(),
    );
    save_file_store(
        &task.id,
        &FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::from([
                (source_key(&source_path), record),
                (source_key(&no_asr_units_path), no_asr_units),
            ]),
        },
    )
    .unwrap();

    let agents = normalized_daily_agents(&task.daily_agent);
    assert!(daily_agent_has_changed_daily_markdown(&task, &agents).unwrap());
}

#[test]
fn daily_agent_after_asr_run_checks_all_agents_for_pending_markdown_changes() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let audio_dir = temp.path().join("audio");
    std::fs::create_dir_all(&audio_dir).unwrap();
    let task = test_directory_task("daily-agent-any-agent-changed-task", audio_dir);
    ensure_asr_daily_workspace(&task).unwrap();
    let agents = normalized_daily_agents(&task.daily_agent);
    let daily_path = daily_dir_for_task(&task.id).join("2026-06-03.md");
    std::fs::write(&daily_path, "shared daily transcript").unwrap();
    let source_sha256 = compute_sha256(&daily_path).unwrap();
    let source_len_bytes = std::fs::metadata(&daily_path).unwrap().len();

    let primary_agent = &agents[0];
    let primary_task = task_for_daily_agent(&task, primary_agent);
    let mut processed = AsrDailyAgentProcessedState::default();
    processed.documents.insert(
        daily_agent_processed_key(&primary_task, "2026-06-03"),
        AsrDailyAgentProcessedDocument {
            agent_id: primary_agent.id.clone(),
            agent_name: primary_agent.name.clone(),
            output_dir: primary_agent.output_dir.clone(),
            date: "2026-06-03".to_string(),
            source_sha256,
            source_len_bytes,
            processed_at_ms: 1,
            runner: primary_agent.runner.clone(),
            report_path: None,
            last_run_id: "primary-processed".to_string(),
        },
    );
    save_daily_agent_processed_state(&task.id, &processed).unwrap();

    assert!(daily_agent_has_changed_daily_markdown(&task, &agents).unwrap());
}

#[test]
fn daily_agent_effective_status_marks_stale_running_as_interrupted() {
    let task_id = "daily-agent-stale-running-task";
    let mut task = AsrDirectoryTask {
        id: task_id.to_string(),
        name: "Daily Agent Stale Running Task".to_string(),
        audio_dir: PathBuf::from("/tmp"),
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    task.daily_agent.last_status = Some("running".to_string());

    DAILY_AGENT_RUNNING_TASKS
        .lock()
        .unwrap()
        .remove(task_id);
    assert_eq!(
        daily_agent_effective_last_status(&task).as_deref(),
        Some("interrupted")
    );

    DAILY_AGENT_RUNNING_TASKS
        .lock()
        .unwrap()
        .insert(task_id.to_string());
    assert_eq!(
        daily_agent_effective_last_status(&task).as_deref(),
        Some("running")
    );
    DAILY_AGENT_RUNNING_TASKS
        .lock()
        .unwrap()
        .remove(task_id);
}

#[test]
fn daily_agent_effective_status_uses_latest_agent_status() {
    let task_id = "daily-agent-latest-status-task";
    let mut task = AsrDirectoryTask {
        id: task_id.to_string(),
        name: "Daily Agent Latest Status Task".to_string(),
        audio_dir: PathBuf::from("/tmp"),
        recursive: true,
        enabled: true,
        paused: false,
        paused_at_ms: None,
        schedule: AsrTaskSchedule::Hourly { minute: 0 },
        language: "chinese".to_string(),
        model: "Qwen3-ASR-1.7B".to_string(),
        runtime_strategy: AsrRuntimeStrategy::ReusePerFile,
        diarization: AsrDiarizationConfig::default(),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    task.daily_agent.last_status = Some("success".to_string());
    task.daily_agent.agents[0].last_status = Some("success".to_string());
    task.daily_agent.agents[0].last_run_at_ms = Some(10);
    task.daily_agent.agents[1].last_status = Some("running".to_string());
    task.daily_agent.agents[1].last_run_at_ms = Some(20);

    DAILY_AGENT_RUNNING_TASKS.lock().unwrap().remove(task_id);
    assert_eq!(
        daily_agent_effective_last_status(&task).as_deref(),
        Some("interrupted")
    );

    DAILY_AGENT_RUNNING_TASKS.lock().unwrap().insert(task_id.to_string());
    assert_eq!(
        daily_agent_effective_last_status(&task).as_deref(),
        Some("running")
    );
    DAILY_AGENT_RUNNING_TASKS.lock().unwrap().remove(task_id);
}

#[test]
fn daily_agent_im_self_call_uses_admin_prefix() {
    assert_eq!(
        daily_agent_im_send_url(9900),
        "http://127.0.0.1:9900/_bifrost/api/im-gateway/messages/send"
    );
}

#[test]
fn daily_agent_im_self_call_discovers_runtime_port() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    std::fs::write(
        temp.path().join("runtime.json"),
        r#"{"pid":1234,"port":18896,"host":"127.0.0.1"}"#,
    )
    .unwrap();

    assert_eq!(discover_admin_port(), 18896);
}

#[test]
fn daily_agent_im_splits_full_report_without_summary_fallback() {
    let content = "明".repeat(DAILY_AGENT_IM_TEXT_CHUNK_CHARS + 3);
    let chunks = split_daily_agent_im_content(&content);

    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].chars().count(), DAILY_AGENT_IM_TEXT_CHUNK_CHARS);
    assert_eq!(chunks[1], "明明明");
    assert!(decorate_daily_agent_im_chunk(&chunks[0], 0, chunks.len())
        .starts_with("ASR Daily Agent Report 1/2\n\n"));
}
