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
        "config": {
            "enabled": task.daily_agent.enabled,
            "runner": task.daily_agent.runner,
            "timeout_ms": task.daily_agent.timeout_ms,
            "trigger_policy": task.daily_agent.trigger_policy,
            "session_key": task.daily_agent.session_key,
            "instructions_source": task.daily_agent.instructions_source,
            "im_delivery": task.daily_agent.im_delivery,
        },
        "workspace": workspace,
        "report_index_status": report_index_status,
        "last_run": {
            "run_id": task.daily_agent.last_run_id,
            "status": task.daily_agent.last_status,
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
        runner: Option<String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        trigger_policy: Option<AsrDailyAgentTriggerPolicy>,
        #[serde(default)]
        session_key: Option<String>,
        #[serde(default)]
        im_delivery: Option<UpdateImDeliveryConfig>,
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

    if task.daily_agent.enabled && !daily_agent_runner_ready(task) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "必须选择 Runner 或关闭 Daily Agent",
        );
    }
    if task.daily_agent.im_delivery.enabled && daily_agent_im_channel(task).is_none() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "必须选择 IM Channel 或关闭发送",
        );
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

async fn get_daily_agent_instructions_response(task_id: &str) -> Response<BoxBody> {
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };

    let daily_dir = daily_dir_for_task(task_id);
    let agents_path = daily_dir.join("AGENTS.md");

    let content = if agents_path.exists() {
        std::fs::read_to_string(&agents_path).unwrap_or_default()
    } else if let Some(instructions) = &task.daily_agent.instructions {
        instructions.clone()
    } else {
        DEFAULT_ASR_DAILY_AGENTS_MD
            .replace("{{task_name}}", &task.name)
            .replace("{{daily_dir}}", ".")
            .replace("{{report_dir}}", "./report/")
    };

    json_response(&serde_json::json!({
        "task_id": task_id,
        "content": content,
        "source": if agents_path.exists() { "file" } else { "default" },
    }))
}

async fn put_daily_agent_instructions_response(
    task_id: &str,
    req: Request<Incoming>,
) -> Response<BoxBody> {
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
    let _ = ensure_asr_daily_workspace(&task);

    let daily_dir = daily_dir_for_task(task_id);
    let agents_path = daily_dir.join("AGENTS.md");
    if let Err(e) = std::fs::write(&agents_path, update.content.as_bytes()) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("write AGENTS.md: {e}"),
        );
    }

    let _config_lock = DAILY_AGENT_TASK_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let mut store = load_tasks();
    if let Some(task) = store.tasks.iter_mut().find(|t| t.id == task_id) {
        task.daily_agent.instructions_source = AsrDailyAgentInstructionsSource::Custom;
        task.daily_agent.instructions = Some(update.content);
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
    if !daily_agent_runner_ready(&task) {
        return error_response(StatusCode::BAD_REQUEST, "Daily Agent runner not configured");
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

    let task_clone = task.clone();
    let date_clone = date.clone();

    tokio::spawn(async move {
        run_daily_agent(&task_clone, "manual", date_clone.as_deref(), force).await;
    });

    json_response_with_status(
        StatusCode::ACCEPTED,
        &serde_json::json!({
            "status": "queued",
            "message": "Daily Agent run queued",
            "force": force,
            "date": date,
        }),
    )
}

async fn post_daily_agent_send_response(task_id: &str) -> Response<BoxBody> {
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };

    if !task.daily_agent.im_delivery.enabled {
        return error_response(StatusCode::BAD_REQUEST, "IM delivery is not enabled");
    }
    if daily_agent_im_channel(&task).is_none() {
        return error_response(StatusCode::BAD_REQUEST, "IM channel not configured");
    }

    let mut reports: Vec<String> = Vec::new();

    for path in list_daily_agent_report_files(task_id) {
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

fn get_daily_agent_runs_response(task_id: &str) -> Response<BoxBody> {
    let Some(_task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };

    let processed = load_daily_agent_processed_state(task_id);
    let documents = build_daily_agent_records(task_id, &processed);

    json_response(&serde_json::json!({
        "task_id": task_id,
        "processed_documents": documents,
    }))
}

fn build_daily_agent_records(
    task_id: &str,
    processed: &AsrDailyAgentProcessedState,
) -> Vec<AsrDailyAgentProcessedDocument> {
    let mut documents = processed.documents.clone();

    for report_path in list_daily_agent_report_files(task_id) {
        let Some(date) = daily_agent_report_date_from_path(&report_path) else {
            continue;
        };
        let report_path_string = report_path.to_string_lossy().to_string();
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

        let daily_source_path = daily_dir_for_task(task_id).join(format!("{date}.md"));
        let (source_sha256, source_len_bytes) = if daily_source_path.exists() {
            (
                compute_sha256(&daily_source_path).unwrap_or_default(),
                source_size(&daily_source_path).unwrap_or_default(),
            )
        } else {
            (String::new(), 0)
        };

        documents.insert(
            date.clone(),
            AsrDailyAgentProcessedDocument {
                date,
                source_sha256,
                source_len_bytes,
                processed_at_ms: source_modified_ms(&report_path).unwrap_or_default(),
                runner: "filesystem".to_string(),
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

fn daily_agent_report_path_for_date(task_id: &str, date: &str) -> Result<PathBuf, String> {
    if NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        return Err("ASR Daily Agent report date must use YYYY-MM-DD".to_string());
    }

    for path in list_daily_agent_report_files(task_id) {
        if daily_agent_report_date_from_path(&path).as_deref() == Some(date) {
            return Ok(path);
        }
    }

    Ok(daily_dir_for_task(task_id)
        .join("report")
        .join(format!("{date}-report.md")))
}

fn get_daily_agent_report_response(task_id: &str, date: &str) -> Response<BoxBody> {
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };

    let report_path = match daily_agent_report_path_for_date(task_id, date) {
        Ok(path) => path,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    if !report_path.exists() {
        return error_response(StatusCode::NOT_FOUND, "ASR Daily Agent report not found");
    }

    let processed = load_daily_agent_processed_state(task_id);
    let processed_document = processed.documents.get(date);
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
