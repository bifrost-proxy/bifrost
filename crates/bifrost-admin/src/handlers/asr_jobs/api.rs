pub async fn handle_asr_tasks(req: Request<Incoming>, path: &str) -> Response<BoxBody> {
    ensure_scheduler_started().await;

    if path.starts_with("/api/asr/diarization")
        || path.starts_with("/api/asr/speaker-profiles")
        || (path.starts_with("/api/asr/tasks/")
            && (path.ends_with("/diarization") || path.contains("/speakers/")))
    {
        return handle_diarization_api(req, path).await;
    }

    match (req.method(), path) {
        (&Method::GET, "/api/asr/external-volumes") => {
            json_response(&serde_json::json!({ "volumes": list_external_volumes() }))
        }
        (&Method::GET, "/api/asr/tasks/-/watch") => list_task_watch_response(),
        (&Method::GET, "/api/asr/tasks") => list_tasks_response(),
        (&Method::POST, "/api/asr/tasks") => create_task_response(req).await,
        (&Method::PATCH, _) if path.starts_with("/api/asr/tasks/") => {
            let Some(id) = path
                .strip_prefix("/api/asr/tasks/")
                .filter(|id| !id.contains('/'))
            else {
                return error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found");
            };
            update_task_response(id, req).await
        }
        (&Method::GET, _) if path.starts_with("/api/asr/tasks/") && path.ends_with("/external-import") => {
            let id = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/external-import")
                .trim_end_matches('/');
            get_external_import_response(id)
        }
        (&Method::PUT, _) if path.starts_with("/api/asr/tasks/") && path.ends_with("/external-import") => {
            let id = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/external-import")
                .trim_end_matches('/');
            put_external_import_response(id, req).await
        }
        (&Method::POST, _) if path.starts_with("/api/asr/tasks/") && path.ends_with("/external-import/run") => {
            let id = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/external-import/run")
                .trim_end_matches('/');
            run_external_import_response(id).await
        }
        (&Method::GET, _) if path.starts_with("/api/asr/tasks/") && path.contains("/daily-agent") => {
            let parts = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            match parts.as_slice() {
                [task_id, "daily-agent"] => get_daily_agent_config_response(task_id).await,
                [task_id, "daily-agent", "agents"] => {
                    get_daily_agent_instructions_response(task_id, req).await
                }
                [task_id, "daily-agent", "runs"] => get_daily_agent_runs_response(task_id),
                [task_id, "daily-agent", "reports", date] => {
                    get_daily_agent_report_response(task_id, date, req)
                }
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
                [task_id, "daily-agent", "send"] => post_daily_agent_send_response(task_id, req).await,
                [task_id, "daily-agent", "sync"] => post_daily_agent_sync_response(task_id).await,
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
        (&Method::GET, _) if path.starts_with("/api/asr/tasks/") && path.contains("/artifacts") => {
            let parts = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches('/')
                .split('/')
                .collect::<Vec<_>>();
            match parts.as_slice() {
                [task_id, "files", file_key, "artifacts"] => {
                    get_task_file_artifacts_response(task_id, file_key, None)
                }
                [task_id, "files", file_key, "artifacts", format] => {
                    get_task_file_artifacts_response(task_id, file_key, Some(format))
                }
                _ => error_response(StatusCode::NOT_FOUND, "ASR task endpoint not found"),
            }
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
        (&Method::GET, _) if path.starts_with("/api/asr/tasks/") && path.ends_with("/watch") => {
            let id = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/watch")
                .trim_end_matches('/');
            get_task_watch_response(id)
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
            delete_task_response(id, req.uri().query())
        }
        (&Method::POST, _)
            if path.starts_with("/api/asr/tasks/") && path.ends_with("/cleanup-source-audio") =>
        {
            let id = path
                .trim_start_matches("/api/asr/tasks/")
                .trim_end_matches("/cleanup-source-audio")
                .trim_end_matches('/');
            cleanup_task_source_audio_response(id)
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
            let query = req.uri().query().unwrap_or_default();
            let force = query_flag_enabled(query, "force");
            let pause_mode = match pause_mode_from_query(query) {
                Ok(mode) => mode,
                Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
            };
            pause_task_response(id, force, pause_mode)
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

fn query_param_value(query: &str, key: &str) -> Option<String> {
    url::form_urlencoded::parse(query.as_bytes()).find_map(|(name, value)| {
        (name == key).then(|| value.into_owned())
    })
}

fn pause_mode_from_query(query: &str) -> Result<AsrTaskPauseMode, &'static str> {
    let Some(value) = query_param_value(query, "mode") else {
        return Ok(AsrTaskPauseMode::LongTerm);
    };
    AsrTaskPauseMode::from_query(&value)
        .ok_or("invalid pause mode; use temporary or long_term")
}

fn normalize_task_diarization_config(mut config: AsrDiarizationConfig) -> AsrDiarizationConfig {
    if config.enabled && config.known_speaker_count.is_none() && config.max_speakers.is_none() {
        config.max_speakers = Some(bifrost_asr::profiles::DEFAULT_AUTO_MAX_SPEAKERS);
    }
    config
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
    if let Err(error) = ensure_task_audio_dir(&create.audio_dir) {
        return error_response(
            StatusCode::BAD_REQUEST,
            &error,
        );
    }

    let now = now_ms();
    let schedule = create.schedule.unwrap_or_else(default_task_schedule);
    if let Err(error) = schedule.validate() {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }
    let enabled = create.enabled.unwrap_or(true);
    let external_devices = match normalize_bindings(create.external_devices.unwrap_or_default()) {
        Ok(bindings) => bindings,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    let mut import_policy = create.import_policy.unwrap_or_default();
    if !external_devices.is_empty() {
        import_policy.enabled = true;
    }
    if let Err(error) = validate_import_policy(&import_policy) {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }
    let next_run_at_ms = enabled
        .then(|| schedule.initial_next_run_at_ms(now))
        .flatten();
    let daily_agent = normalize_daily_agent_config(&create.daily_agent.unwrap_or_default());
    if let Err(error) = validate_daily_agent_config(&daily_agent) {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }
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
        model: create
            .model
            .unwrap_or_else(|| bifrost_asr::runtime::DEFAULT_ASR_MODEL.to_string()),
        runtime_strategy: create.runtime_strategy.unwrap_or_default(),
        diarization: normalize_task_diarization_config(create.diarization.unwrap_or_else(
            bifrost_asr::profiles::AsrDiarizationConfig::speaker_aware_default,
        )),
        created_at_ms: now,
        updated_at_ms: now,
        last_run_at_ms: None,
        next_run_at_ms,
        last_error: None,
        daily_agent,
        external_devices,
        import_policy,
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

async fn update_task_response(id: &str, req: Request<Incoming>) -> Response<BoxBody> {
    let body = match req.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read ASR task update body: {error}"),
            );
        }
    };
    let update: UpdateTaskRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid ASR task update JSON: {error}"),
            );
        }
    };
    match update_task_config(id, update) {
        Ok(task) => json_response(&task_with_summary(task)),
        Err((status, error)) => error_response(status, &error),
    }
}

fn update_task_config(
    id: &str,
    update: UpdateTaskRequest,
) -> Result<AsrDirectoryTask, (StatusCode, String)> {
    let running = RUNNING_TASKS.lock().unwrap().contains(id);
    let high_risk = update.audio_dir.is_some()
        || update.recursive.is_some()
        || update.language.is_some()
        || update.model.is_some()
        || update.runtime_strategy.is_some()
        || update.diarization.is_some()
        || update.external_devices.is_some()
        || update.import_policy.is_some();
    if running && high_risk {
        return Err((
            StatusCode::CONFLICT,
            "task_running: pause or wait for the ASR task before changing data source, model, runtime, or external import configuration".to_string(),
        ));
    }
    let mut store = load_tasks();
    let now = now_ms();
    let Some(task) = store.tasks.iter_mut().find(|task| task.id == id) else {
        return Err((StatusCode::NOT_FOUND, "ASR task not found".to_string()));
    };
    if let Some(audio_dir) = update.audio_dir {
        ensure_task_audio_dir(&audio_dir).map_err(|error| (StatusCode::BAD_REQUEST, error))?;
        task.audio_dir = audio_dir;
    }
    if let Some(name) = update.name {
        let name = name.trim();
        if !name.is_empty() {
            task.name = name.to_string();
        }
    }
    if let Some(recursive) = update.recursive {
        task.recursive = recursive;
    }
    if let Some(enabled) = update.enabled {
        task.enabled = enabled;
    }
    if let Some(paused) = update.paused {
        task.paused = paused;
        task.paused_at_ms = paused.then_some(now);
    }
    if let Some(schedule) = update.schedule {
        if let Err(error) = schedule.validate() {
            return Err((StatusCode::BAD_REQUEST, error));
        }
        task.schedule = schedule;
    }
    if let Some(language) = update.language {
        task.language = language;
    }
    if let Some(model) = update.model {
        task.model = model;
    }
    if let Some(runtime_strategy) = update.runtime_strategy {
        task.runtime_strategy = runtime_strategy;
    }
    if let Some(diarization) = update.diarization {
        task.diarization = normalize_task_diarization_config(diarization);
    }
    if let Some(daily_agent) = update.daily_agent {
        task.daily_agent = normalize_daily_agent_config(&daily_agent);
    }
    if let Some(bindings) = update.external_devices {
        task.external_devices =
            normalize_bindings(bindings).map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    }
    if let Some(policy) = update.import_policy {
        validate_import_policy(&policy).map_err(|error| (StatusCode::BAD_REQUEST, error))?;
        task.import_policy = policy;
    }
    task.updated_at_ms = now;
    task.next_run_at_ms = task
        .enabled
        .then_some(())
        .filter(|_| !task.paused)
        .and_then(|_| task.schedule.next_run_at_ms(now.saturating_add(60_000), false));
    let updated = task.clone();
    save_tasks(&store).map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    Ok(updated)
}

fn ensure_task_audio_dir(audio_dir: &Path) -> Result<(), String> {
    if audio_dir.exists() && !audio_dir.is_dir() {
        return Err(format!(
            "ASR task audio_dir must be a directory: {}",
            audio_dir.display()
        ));
    }
    std::fs::create_dir_all(audio_dir).map_err(|error| {
        format!(
            "Failed to create ASR task audio_dir {}: {error}",
            audio_dir.display()
        )
    })
}

fn list_tasks_response() -> Response<BoxBody> {
    let tasks = load_tasks()
        .tasks
        .into_iter()
        .map(task_with_list_summary)
        .collect::<Vec<_>>();
    json_response(&serde_json::json!({ "tasks": tasks }))
}

fn get_task_response(id: &str) -> Response<BoxBody> {
    match find_task(id) {
        Some(task) => json_response(&task_detail(task)),
        None => error_response(StatusCode::NOT_FOUND, "ASR task not found"),
    }
}

fn list_task_watch_response() -> Response<BoxBody> {
    let tasks = load_tasks()
        .tasks
        .into_iter()
        .map(|task| task_watch_snapshot(task, false))
        .collect::<Vec<_>>();
    json_response(&TaskWatchListResponse {
        tasks,
        updated_at_ms: now_ms(),
    })
}

fn get_task_watch_response(id: &str) -> Response<BoxBody> {
    match find_task(id) {
        Some(task) => json_response(&task_watch_snapshot(task, true)),
        None => error_response(StatusCode::NOT_FOUND, "ASR task not found"),
    }
}

fn get_external_import_response(id: &str) -> Response<BoxBody> {
    let Some(task) = find_task(id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    let volumes = list_external_volumes();
    let store = load_external_import_store(id);
    let devices = task
        .external_devices
        .iter()
        .cloned()
        .map(|binding| {
            let volume = volumes
                .iter()
                .find(|volume| external_volume_matches(&binding, volume));
            let state = store.devices.get(&binding.name);
            AsrExternalDeviceStatus {
                binding,
                connected: volume.is_some(),
                mount_path: volume.map(|volume| volume.mount_path.clone()),
                status: state
                    .and_then(|state| state.last_status.clone())
                    .unwrap_or_else(|| if volume.is_some() { "ready" } else { "not_connected" }.to_string()),
                last_error: state.and_then(|state| state.last_error.clone()),
            }
        })
        .collect::<Vec<_>>();
    json_response(&ExternalImportStatusResponse {
        policy: task.import_policy,
        devices,
        runs: store.runs,
        current_run: normalize_external_import_progress(id),
    })
}

async fn put_external_import_response(id: &str, req: Request<Incoming>) -> Response<BoxBody> {
    let body = match req.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read external import body: {error}"),
            );
        }
    };
    let update: ExternalImportUpdateRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid external import JSON: {error}"),
            );
        }
    };
    let task_update = UpdateTaskRequest {
        name: None,
        audio_dir: None,
        recursive: None,
        enabled: None,
        paused: None,
        schedule: None,
        language: None,
        model: None,
        runtime_strategy: None,
        diarization: None,
        daily_agent: None,
        external_devices: update.external_devices,
        import_policy: update.import_policy,
    };
    match update_task_config(id, task_update) {
        Ok(task) => json_response(&task_with_summary(task)),
        Err((status, error)) => error_response(status, &error),
    }
}

async fn run_external_import_response(id: &str) -> Response<BoxBody> {
    let Some(task) = find_task(id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    match start_external_import_background(task.clone(), "manual_api") {
        Ok(progress) => json_response_with_status(
            StatusCode::ACCEPTED,
            &serde_json::json!({
                "task": task_with_summary(task),
                "imported": 0,
                "running": true,
                "progress": progress,
                "message": "External import started in background.",
            }),
        ),
        Err(error) if error == "ASR external import is already running" => {
            json_response_with_status(
                StatusCode::ACCEPTED,
                &serde_json::json!({
                    "task": task_with_summary(task),
                    "imported": 0,
                    "running": true,
                    "progress": normalize_external_import_progress(id),
                    "message": "External import is already running.",
                }),
            )
        }
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
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

fn get_task_file_artifacts_response(
    task_id: &str,
    file_key: &str,
    format: Option<&str>,
) -> Response<BoxBody> {
    let files = load_file_store(task_id);
    let Some(record) = files.files.get(file_key) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task file not found");
    };
    let artifacts = record_artifact_paths(record);
    let Some(format) = format else {
        return json_response(&serde_json::json!({ "artifacts": artifacts }));
    };
    let Some(path) = artifacts
        .iter()
        .find_map(|artifact| {
            (artifact
                .get("format")
                .and_then(|value| value.as_str())
                .map(|artifact_format| artifact_format == canonical_artifact_format(format))
                .unwrap_or(false))
                .then(|| artifact.get("path").and_then(|value| value.as_str()))
                .flatten()
        })
        .map(PathBuf::from)
    else {
        return error_response(StatusCode::NOT_FOUND, "ASR task file artifact not found");
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("read artifact {}: {error}", path.display()),
            );
        }
    };
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", artifact_content_type(format))
        .body(full_body(bytes))
        .unwrap()
}

fn record_artifact_paths(record: &FileRecord) -> Vec<serde_json::Value> {
    let mut artifacts = Vec::new();
    if let Some(path) = &record.output_text_path {
        push_artifact(&mut artifacts, "txt", path);
    }
    if let Some(path) = &record.output_metadata_path {
        push_artifact(&mut artifacts, "metadata_json", path);
    }
    if let Some(path) = &record.output_timeline_path {
        push_artifact(&mut artifacts, "timeline_json", path);
        let srt_path = bifrost_asr::artifacts::subtitle_path_from_timeline(path, "srt");
        if srt_path.is_file() {
            push_artifact(&mut artifacts, "srt", &srt_path);
        }
        let vtt_path = bifrost_asr::artifacts::subtitle_path_from_timeline(path, "vtt");
        if vtt_path.is_file() {
            push_artifact(&mut artifacts, "vtt", &vtt_path);
        }
    }
    artifacts
}

fn push_artifact(artifacts: &mut Vec<serde_json::Value>, format: &str, path: &Path) {
    artifacts.push(serde_json::json!({
        "format": format,
        "path": path.to_string_lossy().to_string(),
        "exists": path.is_file(),
    }));
}

fn artifact_content_type(format: &str) -> &'static str {
    match canonical_artifact_format(format) {
        "vtt" => "text/vtt; charset=utf-8",
        "srt" | "txt" => "text/plain; charset=utf-8",
        "timeline_json" | "metadata_json" => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn canonical_artifact_format(format: &str) -> &str {
    match format {
        "text" => "txt",
        "metadata" | "json" => "metadata_json",
        "timeline" => "timeline_json",
        other => other,
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

fn delete_task_response(id: &str, query: Option<&str>) -> Response<BoxBody> {
    let mut store = load_tasks();
    let Some(index) = store.tasks.iter().position(|task| task.id == id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    if RUNNING_TASKS.lock().unwrap().contains(id) {
        return error_response(StatusCode::CONFLICT, "task_running");
    }
    let confirm_name = query
        .and_then(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .find_map(|(key, value)| (key == "confirm_name").then(|| value.into_owned()))
        })
        .unwrap_or_default();
    if confirm_name != store.tasks[index].name {
        return error_response(StatusCode::BAD_REQUEST, "task_delete_confirmation_required");
    }
    store.tasks.remove(index);
    match save_tasks(&store) {
        Ok(()) => {
            BULK_CHUNK_RETRY_JOBS.lock().unwrap().remove(id);
            json_response(&serde_json::json!({ "ok": true }))
        }
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn cleanup_task_source_audio_response(id: &str) -> Response<BoxBody> {
    let Some(task) = find_task(id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    if RUNNING_TASKS.lock().unwrap().contains(id) {
        return json_response_with_status(
            StatusCode::CONFLICT,
            &serde_json::json!({
                "message": "ASR task is running; pause or wait for it to finish before cleaning source audio",
                "running": true,
            }),
        );
    }
    let bulk_retry_active = bulk_chunk_retry_state(id)
        .map(|retry| matches!(retry.status, BulkChunkRetryStatus::Queued | BulkChunkRetryStatus::Running))
        .unwrap_or(false);
    if bulk_retry_active {
        return json_response_with_status(
            StatusCode::CONFLICT,
            &serde_json::json!({
                "message": "ASR failed-chunk retry is running; wait for it to finish before cleaning source audio",
                "running": true,
            }),
        );
    }

    json_response(&cleanup_task_source_audio(&task))
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
            task: task_with_control_summary(task),
            processed_now: 0,
            failed_now: 0,
            message: "ASR directory task started in background.".to_string(),
        }),
        Err(response) => *response,
    }
}

fn pause_task_response(id: &str, force: bool, mode: AsrTaskPauseMode) -> Response<BoxBody> {
    match update_task_paused_with_mode(id, true, mode) {
        Ok(task) => {
            let running = RUNNING_TASKS.lock().unwrap().contains(id);
            if force {
                FORCE_PAUSED_TASKS.lock().unwrap().insert(id.to_string());
            }
            json_response(&serde_json::json!({
                "task": task_with_control_summary(task),
                "paused": true,
                "running": running,
                "force": force,
                "pause_mode": mode.as_str(),
                "message": if running {
                    if force {
                        "ASR task force-pause requested. The running ASR process will be aborted promptly to release compute."
                    } else {
                        "ASR task pause requested. It will release compute after the current file or chunk boundary."
                    }
                } else if mode == AsrTaskPauseMode::Temporary {
                    "ASR task temporarily paused. It will resume automatically at the next scheduled run."
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
            "task": task_with_control_summary(task),
            "paused": false,
            "running": true,
            "message": "ASR task resume requested. The current run will continue at the next pause checkpoint.",
        }));
    }

    match spawn_directory_task_run(task.clone()) {
        Ok(()) => json_response(&serde_json::json!({
            "task": task_with_control_summary(task),
            "paused": false,
            "running": true,
            "message": "ASR task resumed and queued for background processing.",
        })),
        Err(response) => *response,
    }
}

fn spawn_directory_task_run(task: AsrDirectoryTask) -> Result<(), Box<Response<BoxBody>>> {
    match spawn_directory_task_run_background(task) {
        Ok(()) => Ok(()),
        Err(error) if error == "ASR task is already running" => {
            Err(Box::new(json_response_with_status(
                StatusCode::CONFLICT,
                &serde_json::json!({
                    "message": "ASR task is already running",
                    "running": true,
                }),
            )))
        }
        Err(error) => Err(Box::new(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &error,
        ))),
    }
}
