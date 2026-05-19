pub async fn handle_asr_tasks(req: Request<Incoming>, path: &str) -> Response<BoxBody> {
    ensure_scheduler_started().await;

    match (req.method(), path) {
        (&Method::GET, "/api/asr/tasks") => list_tasks_response(),
        (&Method::POST, "/api/asr/tasks") => create_task_response(req).await,
        (&Method::GET, _) if path.starts_with("/api/asr/tasks/") && path.contains("/daily-agent") => {
            let parts = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            match parts.as_slice() {
                [task_id, "daily-agent"] => get_daily_agent_config_response(task_id).await,
                [task_id, "daily-agent", "agents"] => {
                    get_daily_agent_instructions_response(task_id).await
                }
                [task_id, "daily-agent", "runs"] => get_daily_agent_runs_response(task_id),
                _ => error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found"),
            }
        }
        (&Method::PUT, _) if path.starts_with("/api/asr/tasks/") && path.contains("/daily-agent") => {
            let parts = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            match parts.as_slice() {
                [task_id, "daily-agent"] => put_daily_agent_config_response(task_id, req).await,
                [task_id, "daily-agent", "agents"] => {
                    put_daily_agent_instructions_response(task_id, req).await
                }
                _ => error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found"),
            }
        }
        (&Method::POST, _) if path.starts_with("/api/asr/tasks/") && path.contains("/daily-agent") => {
            let parts = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            match parts.as_slice() {
                [task_id, "daily-agent", "run"] => post_daily_agent_run_response(task_id, req).await,
                [task_id, "daily-agent", "send"] => post_daily_agent_send_response(task_id).await,
                _ => error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found"),
            }
        }
        (&Method::GET, _) if path.starts_with("/api/asr/tasks/") && path.contains("/daily") => {
            let parts = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            match parts.as_slice() {
                [task_id, "daily"] => list_task_daily_documents_response(task_id),
                [task_id, "daily", date] => get_task_daily_document_response(task_id, date),
                _ => error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found"),
            }
        }
        (&Method::GET, _) if path.starts_with("/api/asr/tasks/") && path.ends_with("/source") => {
            let parts = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/source")
                .trim_end_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            if parts.len() != 3 || parts[1] != "files" {
                return error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found");
            }
            let range = req
                .headers()
                .get("range")
                .and_then(|value| value.to_str().ok());
            get_task_file_source_response(parts[0], parts[2], range)
        }
        (&Method::GET, _) if path.starts_with("/api/asr/tasks/") && path.ends_with("/timeline") => {
            let parts = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/timeline")
                .trim_end_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            if parts.len() != 3 || parts[1] != "files" {
                return error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found");
            }
            get_task_file_timeline_response(parts[0], parts[2])
        }
        (&Method::GET, _) if path.starts_with("/api/asr/tasks/") => {
            let Some(id) = path
                .strip_prefix("/api/asr/tasks/")
                .filter(|id| !id.contains('/'))
            else {
                return error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found");
            };
            get_task_response(id)
        }
        (&Method::DELETE, _) if path.starts_with("/api/asr/tasks/") => {
            let Some(id) = path
                .strip_prefix("/api/asr/tasks/")
                .filter(|id| !id.contains('/'))
            else {
                return error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found");
            };
            delete_task_response(id)
        }
        (&Method::POST, _) if path.starts_with("/api/asr/tasks/") && path.ends_with("/run") => {
            let id = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/run")
                .trim_end_matches('/');
            run_task_response(id).await
        }
        (&Method::POST, _) if path.starts_with("/api/asr/tasks/") && path.ends_with("/pause") => {
            let id = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/pause")
                .trim_end_matches('/');
            let force = req
                .uri()
                .query()
                .is_some_and(|query| query_flag_enabled(query, "force"));
            pause_task_response(id, force)
        }
        (&Method::POST, _) if path.starts_with("/api/asr/tasks/") && path.ends_with("/resume") => {
            let id = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/resume")
                .trim_end_matches('/');
            resume_task_response(id).await
        }
        // POST /api/asr/tasks/{task_id}/retry-failed-chunks
        (&Method::POST, _)
            if path.starts_with("/api/asr/tasks/") && path.ends_with("/retry-failed-chunks") =>
        {
            let id = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/retry-failed-chunks")
                .trim_end_matches('/');
            retry_all_failed_chunks_response(id).await
        }
        // POST /api/asr/tasks/{task_id}/files/{file_key}/retry-chunks
        (&Method::POST, _)
            if path.starts_with("/api/asr/tasks/") && path.ends_with("/retry-chunks") =>
        {
            let parts = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/retry-chunks")
                .trim_end_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            if parts.len() != 3 || parts[1] != "files" {
                return error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found");
            }
            retry_failed_chunks_response(parts[0], parts[2]).await
        }
        (&Method::GET, _) | (&Method::POST, _) | (&Method::DELETE, _) => {
            error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found")
        }
        _ => method_not_allowed(),
    }
}

fn query_flag_enabled(query: &str, key: &str) -> bool {
    url::form_urlencoded::parse(query.as_bytes()).any(|(name, value)| {
        name == key && matches!(value.as_ref(), "" | "1" | "true" | "yes" | "on")
    })
}

async fn create_task_response(req: Request<Incoming>) -> Response<BoxBody> {
    let body = match req.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read ASR task body: {error}"),
            );
        }
    };
    let create: CreateTaskRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid ASR task JSON: {error}"),
            );
        }
    };
    if !create.audio_dir.is_dir() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "ASR task audio_dir must be an existing directory",
        );
    }

    let now = now_ms();
    let schedule = create.schedule.unwrap_or_else(default_task_schedule);
    if let Err(error) = schedule.validate() {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }
    let enabled = create.enabled.unwrap_or(true);
    let next_run_at_ms = enabled
        .then(|| schedule.initial_next_run_at_ms(now))
        .flatten();
    let task = AsrDirectoryTask {
        id: uuid::Uuid::new_v4().as_simple().to_string(),
        name: create
            .name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "ASR directory task".to_string()),
        audio_dir: create.audio_dir,
        recursive: create.recursive.unwrap_or(true),
        enabled,
        paused: false,
        paused_at_ms: None,
        schedule,
        language: create.language.unwrap_or_else(|| "chinese".to_string()),
        model: create.model.unwrap_or_else(|| "Qwen3-ASR-1.7B".to_string()),
        runtime_strategy: create.runtime_strategy.unwrap_or_default(),
        created_at_ms: now,
        updated_at_ms: now,
        last_run_at_ms: None,
        next_run_at_ms,
        last_error: None,
        daily_agent: AsrDailyAgentConfig::default(),
    };

    if let Err(error) = ensure_asr_daily_workspace(&task) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to initialize ASR daily workspace: {error}"),
        );
    }

    match add_task(task.clone()) {
        Ok(()) => json_response_with_status(StatusCode::CREATED, &task_with_summary(task)),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn list_tasks_response() -> Response<BoxBody> {
    let tasks = load_tasks()
        .tasks
        .into_iter()
        .map(task_with_summary)
        .collect::<Vec<_>>();
    json_response(&serde_json::json!({ "tasks": tasks }))
}

fn get_task_response(id: &str) -> Response<BoxBody> {
    match find_task(id) {
        Some(task) => json_response(&task_detail(task)),
        None => error_response(StatusCode::NOT_FOUND, "ASR task not found"),
    }
}

fn list_task_daily_documents_response(task_id: &str) -> Response<BoxBody> {
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    match list_daily_documents_for_task(&bifrost_storage::data_dir(), &task.id, &task.name) {
        Ok(documents) => json_response(&serde_json::json!({ "documents": documents })),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn get_task_daily_document_response(task_id: &str, date: &str) -> Response<BoxBody> {
    let Some(task) = find_task(task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    match read_daily_document_for_task(&bifrost_storage::data_dir(), &task.id, &task.name, date) {
        Ok(Some(document)) => json_response(&document),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "ASR daily document not found"),
        Err(error) if error.contains("YYYY-MM-DD") => {
            error_response(StatusCode::BAD_REQUEST, &error)
        }
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn get_task_file_timeline_response(task_id: &str, file_key: &str) -> Response<BoxBody> {
    let files = load_file_store(task_id);
    let Some(record) = files.files.get(file_key) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task file not found");
    };
    let Some(path) = &record.output_timeline_path else {
        return error_response(StatusCode::NOT_FOUND, "ASR task file timeline not found");
    };
    match std::fs::read_to_string(path)
        .map_err(|error| format!("read timeline {}: {error}", path.display()))
        .and_then(|content| {
            serde_json::from_str::<TranscriptTimeline>(&content)
                .map_err(|error| format!("parse timeline {}: {error}", path.display()))
        }) {
        Ok(mut timeline) => {
            normalize_timeline_segments(&mut timeline);
            json_response(&timeline)
        }
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn get_task_file_source_response(
    task_id: &str,
    file_key: &str,
    range_header: Option<&str>,
) -> Response<BoxBody> {
    let files = load_file_store(task_id);
    let Some(record) = files.files.get(file_key) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task file not found");
    };
    let path = &record.source_path;
    source_audio_response(path, range_header)
}

fn delete_task_response(id: &str) -> Response<BoxBody> {
    let mut store = load_tasks();
    let before = store.tasks.len();
    store.tasks.retain(|task| task.id != id);
    if before == store.tasks.len() {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    }
    match save_tasks(&store) {
        Ok(()) => {
            BULK_CHUNK_RETRY_JOBS.lock().unwrap().remove(id);
            json_response(&serde_json::json!({ "ok": true }))
        }
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

async fn run_task_response(id: &str) -> Response<BoxBody> {
    let task = match load_tasks().tasks.into_iter().find(|task| task.id == id) {
        Some(task) => task,
        None => return error_response(StatusCode::NOT_FOUND, "ASR task not found"),
    };
    if task.paused {
        return json_response_with_status(
            StatusCode::CONFLICT,
            &serde_json::json!({
                "message": "ASR task is paused; resume it before starting a run",
                "paused": true,
                "task": task_with_summary(task),
            }),
        );
    }

    match spawn_directory_task_run(task.clone()) {
        Ok(()) => json_response(&RunTaskResponse {
            task: task_with_summary(task),
            processed_now: 0,
            failed_now: 0,
            message: "ASR directory task started in background.".to_string(),
        }),
        Err(response) => *response,
    }
}

fn pause_task_response(id: &str, force: bool) -> Response<BoxBody> {
    match update_task_paused(id, true) {
        Ok(task) => {
            let running = RUNNING_TASKS.lock().unwrap().contains(id);
            if force {
                FORCE_PAUSED_TASKS.lock().unwrap().insert(id.to_string());
            }
            json_response(&serde_json::json!({
                "task": task_with_summary(task),
                "paused": true,
                "running": running,
                "force": force,
                "message": if running {
                    if force {
                        "ASR task force-pause requested. The running ASR process will be aborted promptly to release compute."
                    } else {
                        "ASR task pause requested. It will release compute after the current file or chunk boundary."
                    }
                } else {
                    "ASR task paused. It will not run until resumed."
                },
            }))
        }
        Err(error) if error.contains("not found") => {
            error_response(StatusCode::NOT_FOUND, "ASR task not found")
        }
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

async fn resume_task_response(id: &str) -> Response<BoxBody> {
    let task = match update_task_paused(id, false) {
        Ok(task) => task,
        Err(error) if error.contains("not found") => {
            return error_response(StatusCode::NOT_FOUND, "ASR task not found");
        }
        Err(error) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    FORCE_PAUSED_TASKS.lock().unwrap().remove(id);

    if RUNNING_TASKS.lock().unwrap().contains(id) {
        return json_response(&serde_json::json!({
            "task": task_with_summary(task),
            "paused": false,
            "running": true,
            "message": "ASR task resume requested. The current run will continue at the next pause checkpoint.",
        }));
    }

    let summary = summarize_task(&task);
    if summary.pending == 0 && summary.failed == 0 {
        return json_response(&serde_json::json!({
            "task": TaskWithSummary {
                bulk_retry: bulk_chunk_retry_state(&task.id),
                task,
                summary,
            },
            "paused": false,
            "running": false,
            "message": "ASR task resumed. No pending or failed files need processing.",
        }));
    }

    match spawn_directory_task_run(task.clone()) {
        Ok(()) => json_response(&serde_json::json!({
            "task": task_with_summary(task),
            "paused": false,
            "running": true,
            "message": "ASR task resumed and started in background.",
        })),
        Err(response) => *response,
    }
}

fn spawn_directory_task_run(task: AsrDirectoryTask) -> Result<(), Box<Response<BoxBody>>> {
    FORCE_PAUSED_TASKS.lock().unwrap().remove(&task.id);
    // Prevent duplicate runs: check and mark as running atomically.
    {
        let mut running = RUNNING_TASKS.lock().unwrap();
        if running.contains(&task.id) {
            return Err(Box::new(json_response_with_status(
                StatusCode::CONFLICT,
                &serde_json::json!({
                    "message": "ASR task is already running",
                    "running": true,
                }),
            )));
        }
        running.insert(task.id.clone());
    }

    // Spawn the task in background so the HTTP response returns immediately.
    let task_id = task.id.clone();
    let task_clone = task.clone();
    tokio::spawn(async move {
        let result = run_directory_task(task_clone.clone()).await;
        match &result {
            Ok((_updated, processed, failed)) => {
                tracing::info!(
                    task_id = %task_clone.id, processed = processed, failed = failed,
                    "ASR directory task completed"
                );
            }
            Err(error) if error == ASR_TASK_PAUSED_MESSAGE => {
                tracing::info!(
                    task_id = %task_clone.id,
                    "ASR directory task paused and released compute"
                );
            }
            Err(error) => {
                let _ = update_task_after_run(&task_clone.id, Some(error.clone()));
                tracing::warn!(
                    task_id = %task_clone.id, error = %error,
                    "ASR directory task failed"
                );
            }
        }
        RUNNING_TASKS.lock().unwrap().remove(&task_id);
    });

    Ok(())
}
