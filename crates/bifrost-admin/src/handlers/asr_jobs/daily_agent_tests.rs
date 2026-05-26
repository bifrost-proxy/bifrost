#[test]
fn daily_agent_default_template_keeps_knowledge_modules_inside_report() {
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("## 报告内知识沉淀模块"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("## 知识沉淀输出要求"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("同一份 `{{report_dir}}YYYY-MM-DD-report.md`"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("### 长期想法与效率方案"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("### 方向决策与判断"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("### 跨天待办追踪"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("资料搜索结果"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("可行性分析"));
    assert!(DEFAULT_ASR_DAILY_AGENTS_MD.contains("方案草案"));
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
    assert!(!codex_prompt.contains("AGENTS.md 内容"));
    assert!(!codex_prompt.contains("变更文件内容"));

    let chatgpt_first = build_daily_agent_prompt(&task, &plan, "chatgpt_web", true).unwrap();
    assert!(chatgpt_first.contains("AGENTS.md 内容"));
    assert!(chatgpt_first.contains("今日新增转写内容"));

    let chatgpt_next = build_daily_agent_prompt(&task, &plan, "chatgpt_web", false).unwrap();
    assert!(!chatgpt_next.contains("AGENTS.md 内容"));
    assert!(chatgpt_next.contains("后续轮次"));
    assert!(chatgpt_next.contains("今日新增转写内容"));
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
    assert!(report_path.ends_with("daily-agent-report-task/.daily/report/2026-05-14-report.md"));
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
    assert!(records[0]
        .report_path
        .as_deref()
        .unwrap()
        .ends_with(".daily/Report/2026-05-14-report.md"));

    let detail_path = daily_agent_report_path_for_date(task_id, "2026-05-14").unwrap();
    assert!(detail_path.ends_with(".daily/Report/2026-05-14-report.md"));
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
fn daily_agent_report_sync_copies_reports_and_skips_identical_targets() {
    let _lock = TEST_DATA_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().unwrap();
    let _guard = EnvGuard::set_data_dir(temp.path());
    let task_id = "daily-agent-report-sync-task";
    let daily_dir = daily_dir_for_task(task_id);
    let report_dir = daily_dir.join("report");
    let sync_dir = temp.path().join("icloud-sync");
    std::fs::create_dir_all(&report_dir).unwrap();
    std::fs::create_dir_all(&sync_dir).unwrap();
    std::fs::write(report_dir.join("2026-05-14-report.md"), "report one").unwrap();
    std::fs::write(report_dir.join("2026-05-15-report.md"), "report two").unwrap();
    std::fs::write(sync_dir.join("2026-05-14-report.md"), "report one").unwrap();

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
        created_at_ms: 1,
        updated_at_ms: 1,
        last_run_at_ms: None,
        next_run_at_ms: Some(1),
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    };
    task.daily_agent.report_sync_dir = Some(sync_dir.to_string_lossy().to_string());

    let reports: Vec<String> = list_daily_agent_report_files(task_id)
        .into_iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    let result = sync_daily_agent_report_files(&task, &reports).unwrap();

    assert_eq!(result.total_files, 2);
    assert_eq!(result.copied_files, 1);
    assert_eq!(result.skipped_files, 1);
    assert_eq!(result.failed_files, 0);
    assert_eq!(
        std::fs::read_to_string(sync_dir.join("2026-05-15-report.md")).unwrap(),
        "report two"
    );
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
    assert_eq!(summary.pending_documents, 1);
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
    assert!(records[0]
        .report_path
        .as_deref()
        .unwrap()
        .ends_with(".daily/report/2026-05-15-report.md"));
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
fn daily_agent_after_asr_run_requires_no_pending_files() {
    let clean = TaskSummary {
        discovered: 1,
        processed: 1,
        pending: 0,
        failed: 0,
        partial_success: 0,
        failed_chunk_count: 0,
        deleted_after_processing: 0,
        audio_source_bytes: 0,
        audio_source_file_count: 0,
        cleanable_source_bytes: 0,
        cleanable_source_file_count: 0,
        running: true,
    };
    assert!(daily_agent_asr_completion_ready(&clean));

    let mut pending = clean.clone();
    pending.pending = 1;
    assert!(!daily_agent_asr_completion_ready(&pending));

    let mut failed = clean.clone();
    failed.failed = 1;
    assert!(!daily_agent_asr_completion_ready(&failed));

    let mut partial = clean.clone();
    partial.partial_success = 1;
    assert!(!daily_agent_asr_completion_ready(&partial));

    let mut failed_chunks = clean;
    failed_chunks.failed_chunk_count = 1;
    assert!(!daily_agent_asr_completion_ready(&failed_chunks));
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
