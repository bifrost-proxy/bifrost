// ─── Daily Agent API Handlers ────────────────────────────────────────────────

async fn get_daily_agent_config_response(task_id: &str) -> Response<BoxBody> {
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };

    let workspace = read_workspace_status(&task);
    let processed = load_daily_agent_processed_state(task_id);
    let report_index_status = build_daily_agent_report_index_status(task_id, &processed);

    let response = serde_json::json!({
        "task_id": task.id,
        "config": task.daily_agent,
        "workspace": workspace,
        "report_index_status": report_index_status,
        "last_run": {
            "run_id": task.daily_agent.last_run_id,
            "status": daily_agent_effective_last_status(&task),
            "error": task.daily_agent.last_error,
            "last_run_at_ms": task.daily_agent.last_run_at_ms,
        },
    });

    json_response(&response)
}

async fn put_daily_agent_config_response(
    task_id: &str,
    req: Request<Incoming>,
) -> Response<BoxBody> {
    let body = match req.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("read body: {e}")),
    };

    #[derive(Deserialize)]
    struct UpdateDailyAgentConfigRequest {
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        agents: Option<Vec<AsrDailyAgentItem>>,
        #[serde(default)]
        runner: Option<String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        trigger_policy: Option<AsrDailyAgentTriggerPolicy>,
        #[serde(default)]
        session_key: Option<String>,
        #[serde(default)]
        im_delivery: Option<UpdateImDeliveryConfig>,
        #[serde(default)]
        report_sync_dir: Option<String>,
    }

    #[derive(Deserialize)]
    struct UpdateImDeliveryConfig {
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        mode: Option<AsrDailyAgentImDeliveryMode>,
        #[serde(default)]
        send_policy: Option<AsrDailyAgentImSendPolicy>,
    }

    let update: UpdateDailyAgentConfigRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("invalid JSON: {e}")),
    };

    let _config_lock = DAILY_AGENT_TASK_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let mut store = load_tasks();
    let Some(task) = store.tasks.iter_mut().find(|t| t.id == task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };

    if let Some(enabled) = update.enabled {
        task.daily_agent.enabled = enabled;
    }
    if let Some(agents) = update.agents {
        task.daily_agent.agents = agents.into_iter().map(normalize_daily_agent_item).collect();
        if let Err(error) = validate_daily_agent_config(&task.daily_agent) {
            return error_response(StatusCode::BAD_REQUEST, &error);
        }
        let enabled = update.enabled.unwrap_or(task.daily_agent.enabled);
        normalize_daily_agent_config_in_place(&mut task.daily_agent);
        task.daily_agent.enabled = enabled;
    }
    if let Some(runner) = update.runner {
        task.daily_agent.runner = runner.trim().to_string();
    }
    if let Some(timeout_ms) = update.timeout_ms {
        task.daily_agent.timeout_ms = timeout_ms;
    }
    if let Some(trigger_policy) = update.trigger_policy {
        task.daily_agent.trigger_policy = trigger_policy;
    }
    if let Some(session_key) = update.session_key {
        task.daily_agent.session_key = Some(session_key).filter(|s| !s.trim().is_empty());
    }
    if let Some(im) = update.im_delivery {
        if let Some(enabled) = im.enabled {
            task.daily_agent.im_delivery.enabled = enabled;
        }
        if let Some(channel) = im.channel {
            task.daily_agent.im_delivery.channel =
                Some(channel.trim().to_string()).filter(|s| !s.is_empty());
        }
        if let Some(mode) = im.mode {
            task.daily_agent.im_delivery.mode = mode;
        }
        if let Some(send_policy) = im.send_policy {
            task.daily_agent.im_delivery.send_policy = send_policy;
        }
    }
    if let Some(report_sync_dir) = update.report_sync_dir {
        set_primary_daily_agent_report_sync_dir(&mut task.daily_agent, Some(report_sync_dir));
    }

    if let Err(error) = validate_daily_agent_config(&task.daily_agent) {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }

    task.updated_at_ms = now_ms();
    let updated_config = task.daily_agent.clone();

    if let Err(e) = save_tasks(&store) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }

    json_response(&serde_json::json!({
        "ok": true,
        "config": updated_config,
    }))
}

fn query_param(query: &str, key: &str) -> Option<String> {
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.to_string())
}

fn selected_daily_agent(task: &AsrDirectoryTask, agent_id: Option<&str>) -> Option<AsrDailyAgentItem> {
    let agents = normalized_daily_agents(&task.daily_agent);
    if let Some(agent_id) = agent_id {
        return agents.into_iter().find(|agent| agent.id == agent_id);
    }
    agents.into_iter().next()
}

async fn get_daily_agent_instructions_response(
    task_id: &str,
    req: Request<Incoming>,
) -> Response<BoxBody> {
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    let query = req.uri().query().unwrap_or("");
    let agent_id = query_param(query, "agent_id");
    let Some(agent) = selected_daily_agent(&task, agent_id.as_deref()) else {
        return error_response(StatusCode::NOT_FOUND, "Daily Agent not found");
    };
    let agent_task = task_for_daily_agent(&task, &agent);

    let agents_path = daily_agent_instructions_path(&agent_task);

    let content = if agents_path.exists() {
        std::fs::read_to_string(&agents_path).unwrap_or_default()
    } else if let Some(instructions) = &agent_task.daily_agent.instructions {
        instructions.clone()
    } else {
        daily_agent_instruction_content(&agent_task)
    };

    json_response(&serde_json::json!({
        "task_id": task_id,
        "agent_id": agent.id,
        "agent_name": agent.name,
        "content": content,
        "source": if agents_path.exists() { "file" } else { "default" },
    }))
}

async fn put_daily_agent_instructions_response(
    task_id: &str,
    req: Request<Incoming>,
) -> Response<BoxBody> {
    let query = req.uri().query().unwrap_or("").to_string();
    let body = match req.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("read body: {e}")),
    };

    #[derive(Deserialize)]
    struct UpdateInstructionsRequest {
        content: String,
    }

    let update: UpdateInstructionsRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("invalid JSON: {e}")),
    };

    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    let agent_id = query_param(&query, "agent_id");
    let Some(agent) = selected_daily_agent(&task, agent_id.as_deref()) else {
        return error_response(StatusCode::NOT_FOUND, "Daily Agent not found");
    };
    let agent_task = task_for_daily_agent(&task, &agent);
    let _ = ensure_asr_daily_workspace(&task);

    let daily_dir = daily_dir_for_task(task_id);
    let agents_path = daily_agent_instructions_path(&agent_task);
    if let Err(e) = std::fs::write(&agents_path, update.content.as_bytes()) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("write Daily Agent instructions: {e}"),
        );
    }

    let _config_lock = DAILY_AGENT_TASK_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let mut store = load_tasks();
    if let Some(task) = store.tasks.iter_mut().find(|t| t.id == task_id) {
        let agent_id = agent.id.clone();
        sync_daily_agent_item_status(&mut task.daily_agent, &agent_id, |item| {
            item.instructions_source = AsrDailyAgentInstructionsSource::Custom;
            item.instructions = Some(update.content.clone());
        });
        mirror_daily_agent_legacy_status(&mut task.daily_agent, &agent_id);
        task.updated_at_ms = now_ms();
    }
    if let Err(e) = save_tasks(&store) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }

    let git_commit_hash = try_git_commit(&daily_dir, "update ASR daily agent instructions");

    json_response(&serde_json::json!({
        "ok": true,
        "git_commit": git_commit_hash,
    }))
}

async fn post_daily_agent_run_response(
    task_id: &str,
    req: Request<Incoming>,
) -> Response<BoxBody> {
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };

    if !task.daily_agent.enabled {
        return error_response(StatusCode::BAD_REQUEST, "Daily Agent is not enabled");
    }

    {
        let running = DAILY_AGENT_RUNNING_TASKS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if running.contains(task_id) {
            return json_response_with_status(
                StatusCode::ACCEPTED,
                &serde_json::json!({
                    "status": "already_running",
                    "message": "Daily Agent run is already in progress",
                }),
            );
        }
    }

    let query = req.uri().query().unwrap_or("");
    let force = query_flag_enabled(query, "force");
    let date: Option<String> = url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "date")
        .map(|(_, v)| v.to_string());
    let agent_id = query_param(query, "agent_id");
    if let Some(agent_id) = agent_id.as_deref() {
        if selected_daily_agent(&task, Some(agent_id)).is_none() {
            return error_response(StatusCode::NOT_FOUND, "Daily Agent not found");
        }
    }

    let task_clone = task.clone();
    let date_clone = date.clone();
    let agent_id_clone = agent_id.clone();

    tokio::spawn(async move {
        if let Some(agent_id) = agent_id_clone.as_deref() {
            if let Some(agent) = selected_daily_agent(&task_clone, Some(agent_id)) {
                let agent_task = task_for_daily_agent(&task_clone, &agent);
                run_daily_agent(&agent_task, "manual", date_clone.as_deref(), force).await;
            }
        } else {
            run_daily_agents(&task_clone, "manual", date_clone.as_deref(), force).await;
        }
    });

    json_response_with_status(
        StatusCode::ACCEPTED,
        &serde_json::json!({
            "status": "queued",
            "message": "Daily Agent run queued",
            "force": force,
            "date": date,
            "agent_id": agent_id,
        }),
    )
}

async fn post_daily_agent_send_response(task_id: &str, req: Request<Incoming>) -> Response<BoxBody> {
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    let query = req.uri().query().unwrap_or("");
    let agent_id = query_param(query, "agent_id");
    let Some(agent) = selected_daily_agent(&task, agent_id.as_deref()) else {
        return error_response(StatusCode::NOT_FOUND, "Daily Agent not found");
    };
    let task = task_for_daily_agent(&task, &agent);

    if !task.daily_agent.im_delivery.enabled {
        return error_response(StatusCode::BAD_REQUEST, "IM delivery is not enabled");
    }
    if daily_agent_im_channel(&task).is_none() {
        return error_response(StatusCode::BAD_REQUEST, "IM channel not configured");
    }

    let mut reports: Vec<String> = Vec::new();

    for path in list_daily_agent_report_files(task_id) {
        if agent_output_dir_from_report_path(&task, &path) != task.daily_agent.output_dir {
            continue;
        }
        reports.push(path.to_string_lossy().to_string());
    }

    reports.sort();
    let recent_reports: Vec<String> = reports.into_iter().rev().take(1).collect();

    if recent_reports.is_empty() {
        return error_response(StatusCode::NOT_FOUND, "no reports found to send");
    }

    let content = build_im_content_for_reports(&task, &recent_reports);
    match send_daily_agent_im_message(&task, &content, "manual_send", recent_reports.len()).await {
        Ok(()) => json_response(&serde_json::json!({
            "ok": true,
            "sent_reports": recent_reports,
        })),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn post_daily_agent_sync_response(task_id: &str) -> Response<BoxBody> {
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    let agent = normalized_daily_agents(&task.daily_agent)
        .into_iter()
        .next()
        .unwrap_or_else(|| daily_agent_item_from_legacy(&task.daily_agent));
    let task = task_for_daily_agent(&task, &agent);

    let sync_result = match sync_all_daily_agent_reports(&task) {
        Ok(result) => result,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };

    if let Err(error) = update_daily_agent_report_sync_status(&task, sync_result.clone()) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }

    if sync_result.failed_files > 0 {
        return json_response_with_status(
            StatusCode::INTERNAL_SERVER_ERROR,
            &serde_json::json!({
                "ok": false,
                "sync": sync_result,
            }),
        );
    }

    json_response(&serde_json::json!({
        "ok": true,
        "sync": sync_result,
    }))
}

fn get_daily_agent_runs_response(task_id: &str) -> Response<BoxBody> {
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };

    let processed = load_daily_agent_processed_state(task_id);
    let documents = build_daily_agent_records_for_task(&task, &processed);

    json_response(&serde_json::json!({
        "task_id": task_id,
        "processed_documents": documents,
    }))
}

fn build_daily_agent_records_for_task(
    task: &AsrDirectoryTask,
    processed: &AsrDailyAgentProcessedState,
) -> Vec<AsrDailyAgentProcessedDocument> {
    let fallback_runner = task.daily_agent.runner.trim();
    build_daily_agent_records_with_fallback_runner(
        task,
        processed,
        (!fallback_runner.is_empty()).then_some(fallback_runner),
    )
}

#[cfg(test)]
fn build_daily_agent_records(
    task_id: &str,
    processed: &AsrDailyAgentProcessedState,
) -> Vec<AsrDailyAgentProcessedDocument> {
    let legacy_only = processed
        .documents
        .keys()
        .all(|key| !key.contains(':'));
    let task = find_task(task_id).unwrap_or_else(|| AsrDirectoryTask {
        id: task_id.to_string(),
        name: task_id.to_string(),
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
        daily_agent: if legacy_only {
            AsrDailyAgentConfig {
                agents: Vec::new(),
                ..Default::default()
            }
        } else {
            AsrDailyAgentConfig::default()
        },
        external_devices: Vec::new(),
        import_policy: AsrExternalImportPolicy::default(),
    });
    build_daily_agent_records_with_fallback_runner(&task, processed, None)
}

fn build_daily_agent_records_with_fallback_runner(
    task: &AsrDirectoryTask,
    processed: &AsrDailyAgentProcessedState,
    fallback_runner: Option<&str>,
) -> Vec<AsrDailyAgentProcessedDocument> {
    let mut documents = processed.documents.clone();

    for report_path in list_daily_agent_report_files(&task.id) {
        let Some(date) = daily_agent_report_date_from_path(&report_path) else {
            continue;
        };
        let output_dir = agent_output_dir_from_report_path(task, &report_path);
        let agent = agent_for_output_dir(task, &output_dir);
        let key = format!("{}:{date}", agent.id);
        let report_path_string = report_path.to_string_lossy().to_string();
        if agent.id == DEFAULT_DAILY_AGENT_ID {
            if let Some(document) = documents.get_mut(&date) {
                if document
                    .report_path
                    .as_ref()
                    .map(|path| !Path::new(path).exists())
                    .unwrap_or(true)
                {
                    document.report_path = Some(report_path_string);
                }
                continue;
            }
        }
        if let Some(document) = documents.get_mut(&key) {
            if document
                .report_path
                .as_ref()
                .map(|path| !Path::new(path).exists())
                .unwrap_or(true)
            {
                document.report_path = Some(report_path_string);
            }
            continue;
        }

        let daily_source_path = daily_dir_for_task(&task.id).join(format!("{date}.md"));
        let (source_sha256, source_len_bytes) = if daily_source_path.exists() {
            (
                compute_sha256(&daily_source_path).unwrap_or_default(),
                source_size(&daily_source_path).unwrap_or_default(),
            )
        } else {
            (String::new(), 0)
        };

        documents.insert(
            key,
            AsrDailyAgentProcessedDocument {
                agent_id: agent.id,
                agent_name: agent.name,
                output_dir,
                date,
                source_sha256,
                source_len_bytes,
                processed_at_ms: source_modified_ms(&report_path).unwrap_or_default(),
                runner: fallback_runner.unwrap_or("filesystem").to_string(),
                report_path: Some(report_path_string),
                last_run_id: "filesystem-scan".to_string(),
            },
        );
    }

    let mut records: Vec<_> = documents.into_values().collect();
    records.sort_by(|a, b| {
        b.date
            .cmp(&a.date)
            .then_with(|| b.processed_at_ms.cmp(&a.processed_at_ms))
    });
    records
}

fn processed_report_path_for_agent_date(task_id: &str, agent_id: &str, date: &str) -> Option<PathBuf> {
    let processed = load_daily_agent_processed_state(task_id);
    let report_path = processed
        .documents
        .get(&format!("{agent_id}:{date}"))
        .or_else(|| (agent_id == DEFAULT_DAILY_AGENT_ID).then(|| processed.documents.get(date)).flatten())?
        .report_path
        .as_deref()?;
    let path = PathBuf::from(report_path);
    if daily_agent_report_date_from_path(&path).as_deref() != Some(date) || !path.is_file() {
        return None;
    }

    let task_output_dir = text_output_dir(&bifrost_storage::data_dir()).join(task_id);
    let Ok(canonical_task_dir) = task_output_dir.canonicalize() else {
        return None;
    };
    let Ok(canonical_report_path) = path.canonicalize() else {
        return None;
    };
    if canonical_report_path.starts_with(canonical_task_dir) {
        Some(path)
    } else {
        None
    }
}

#[cfg(test)]
fn daily_agent_report_path_for_date(task_id: &str, date: &str) -> Result<PathBuf, String> {
    daily_agent_report_path_for_agent_date(task_id, DEFAULT_DAILY_AGENT_ID, date)
}

fn daily_agent_report_path_for_agent_date(task_id: &str, agent_id: &str, date: &str) -> Result<PathBuf, String> {
    if NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        return Err("ASR Daily Agent report date must use YYYY-MM-DD".to_string());
    }

    if let Some(path) = processed_report_path_for_agent_date(task_id, agent_id, date) {
        return Ok(path);
    }

    let task = find_task(task_id);
    let output_dir = task
        .as_ref()
        .and_then(|task| normalized_daily_agents(&task.daily_agent).into_iter().find(|agent| agent.id == agent_id))
        .map(|agent| agent.output_dir)
        .unwrap_or_else(default_daily_agent_output_dir);

    for path in list_daily_agent_report_files(task_id) {
        let same_date = daily_agent_report_date_from_path(&path).as_deref() == Some(date);
        let same_agent = task
            .as_ref()
            .map(|task| agent_output_dir_from_report_path(task, &path) == output_dir)
            .unwrap_or(true);
        if same_date && same_agent {
            return Ok(path);
        }
    }

    Ok(daily_dir_for_task(task_id)
        .join(output_dir)
        .join(format!("{date}-report.md")))
}

fn get_daily_agent_report_response(task_id: &str, date: &str, req: Request<Incoming>) -> Response<BoxBody> {
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    let agent_id = query_param(req.uri().query().unwrap_or(""), "agent_id")
        .unwrap_or_else(default_daily_agent_id);

    let report_path = match daily_agent_report_path_for_agent_date(task_id, &agent_id, date) {
        Ok(path) => path,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    if !report_path.exists() {
        return error_response(StatusCode::NOT_FOUND, "ASR Daily Agent report not found");
    }

    let processed = load_daily_agent_processed_state(task_id);
    let processed_document = processed
        .documents
        .values()
        .find(|document| document.date == date && document.report_path.as_deref() == Some(report_path.to_string_lossy().as_ref()))
        .or_else(|| processed.documents.get(&format!("{}:{date}", DEFAULT_DAILY_AGENT_ID)))
        .or_else(|| processed.documents.get(date));
    let content = match std::fs::read_to_string(&report_path) {
        Ok(content) => content,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("read ASR Daily Agent report {}: {error}", report_path.display()),
            );
        }
    };

    json_response(&serde_json::json!({
        "task_id": task.id,
        "task_name": task.name,
        "agent_id": processed_document.map(|document| document.agent_id.as_str()).unwrap_or(DEFAULT_DAILY_AGENT_ID),
        "agent_name": processed_document.map(|document| document.agent_name.as_str()).unwrap_or(DEFAULT_DAILY_AGENT_NAME),
        "output_dir": processed_document.map(|document| document.output_dir.as_str()).unwrap_or(DEFAULT_DAILY_AGENT_OUTPUT_DIR),
        "date": date,
        "path": report_path.to_string_lossy(),
        "size": source_size(&report_path),
        "modified_ms": source_modified_ms(&report_path),
        "content": content,
        "processed_at_ms": processed_document.map(|document| document.processed_at_ms),
        "runner": processed_document.map(|document| document.runner.as_str()),
        "last_run_id": processed_document.map(|document| document.last_run_id.as_str()),
    }))
}
