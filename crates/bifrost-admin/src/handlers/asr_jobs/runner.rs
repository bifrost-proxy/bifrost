/// Start the ASR task scheduler if not already running.
/// This is called from the router at server startup so that scheduled tasks
/// execute even if no one visits the ASR Tasks page.
pub(crate) async fn ensure_scheduler_started() {
    let mut started = ASR_SCHEDULER_STARTED.lock().await;
    if *started {
        return;
    }
    *started = true;
    let recovery_tasks = recover_interrupted_task_runs_on_startup();
    start_external_device_event_watcher();
    for task in recovery_tasks {
        let task_id = task.id.clone();
        match spawn_directory_task_run_background(task) {
            Ok(()) => {
                tracing::warn!(
                    task_id = %task_id,
                    "re-enqueued interrupted ASR directory task on scheduler startup"
                );
            }
            Err(error) if error == "ASR task is already running" => {
                tracing::debug!(
                    task_id = %task_id,
                    "skipped interrupted ASR directory task recovery because it is already running"
                );
            }
            Err(error) => {
                tracing::warn!(
                    task_id = %task_id,
                    error = %error,
                    "failed to re-enqueue interrupted ASR directory task on scheduler startup"
                );
            }
        }
    }
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let now = now_ms();
            let due = load_tasks()
                .tasks
                .into_iter()
                .filter(|task| {
                    task.enabled
                        && task.next_run_at_ms.is_some_and(|next| next <= now)
                        && (!task.paused || task.next_run_at_ms.is_some())
                })
                .collect::<Vec<_>>();
            for task in due {
                let task_id = task.id.clone();
                let task = if task.paused {
                    match resume_temporary_paused_task_for_schedule(&task.id, now) {
                        Ok(Some(task)) => task,
                        Ok(None) => continue,
                        Err(error) => {
                            warn!(
                                task_id = %task_id,
                                error = %error,
                                "failed to auto-resume temporary paused ASR scheduled directory task"
                            );
                            continue;
                        }
                    }
                } else {
                    task
                };
                if let Err(error) = spawn_directory_task_run_background(task) {
                    if error != "ASR task is already running" {
                        warn!(
                            task_id = %task_id,
                            error = %error,
                            "failed to start due ASR scheduled directory task"
                        );
                    }
                }
            }
        }
    });
}

#[cfg(target_os = "macos")]
async fn sync_external_device_tasks(trigger: &'static str) {
    for task in load_tasks()
        .tasks
        .into_iter()
        .filter(task_allows_external_device_event_import)
    {
        sync_external_device_task(task, trigger).await;
    }
}

#[cfg(any(target_os = "macos", test))]
fn task_allows_external_device_event_import(task: &AsrDirectoryTask) -> bool {
    task.import_policy.enabled && !task.external_devices.is_empty()
}

#[cfg(target_os = "macos")]
async fn sync_external_device_task(task: AsrDirectoryTask, trigger: &'static str) {
    if RUNNING_TASKS.lock().unwrap().contains(&task.id) {
        return;
    }
    match start_external_import_background(task.clone(), trigger) {
        Ok(progress) => {
            tracing::info!(
                task_id = %task.id,
                run_id = %progress.run_id,
                trigger,
                "external device sync queued in background"
            );
        }
        Err(error) if error == "ASR external import is already running" => {}
        Err(error) => {
            tracing::warn!(
                task_id = %task.id,
                error = %error,
                trigger,
                "external device sync failed"
            );
        }
    }
}

#[cfg(target_os = "macos")]
fn start_external_device_event_watcher() {
    tokio::spawn(async {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut child = match Command::new("diskutil")
            .arg("activity")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                tracing::debug!(%error, "failed to start macOS Disk Arbitration activity watcher");
                return;
            }
        };
        let Some(stdout) = child.stdout.take() else {
            tracing::debug!("macOS Disk Arbitration activity watcher has no stdout");
            let _ = child.kill().await;
            return;
        };
        let mut lines = BufReader::new(stdout).lines();
        let mut last_trigger_ms = 0u64;
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.contains("DiskAppeared")
                && !line.contains("DescriptionChanged")
                && !line.contains("DiskDisappeared")
            {
                continue;
            }
            let now = now_ms();
            if now.saturating_sub(last_trigger_ms) < 2_000 {
                continue;
            }
            last_trigger_ms = now;
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                sync_external_device_tasks("macos_disk_arbitration").await;
            });
        }
        match child.wait().await {
            Ok(status) => {
                tracing::debug!(%status, "macOS Disk Arbitration activity watcher exited");
            }
            Err(error) => {
                tracing::debug!(%error, "macOS Disk Arbitration activity watcher wait failed");
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn start_external_device_event_watcher() {}

struct PendingBatchScan {
    pending: Vec<PathBuf>,
}

fn discover_and_prepare_pending_batch(
    task: &AsrDirectoryTask,
    files: &mut FileStore,
    attempted_keys: &HashSet<String>,
) -> Result<PendingBatchScan, String> {
    let discovered = discover_audio_files(&task.audio_dir, task.recursive)?;
    for path in &discovered {
        let key = source_key(path);
        files
            .files
            .entry(key)
            .or_insert_with(|| pending_record(&task.id, path));
    }
    apply_external_import_hashes_to_records(task, &discovered, files);
    apply_content_hash_dedupe(task, &discovered, files)?;
    save_file_store(&task.id, files)?;

    // Only process files that are truly pending or failed outright.
    // PartialSuccess files already have usable text/timeline output and should
    // be recovered via retry-chunks, not re-processed from scratch.
    let mut pending = discovered
        .iter()
        .filter(|path| {
            let key = source_key(path);
            !attempted_keys.contains(&key)
                && files
                    .files
                    .get(&key)
                    .map(|record| {
                        matches!(
                            record.status,
                            FileStatus::Pending | FileStatus::Processing
                        )
                            || (record.status == FileStatus::Failed
                                && !moss_failure_is_non_retryable_for_unchanged_source(
                                    task, path, record,
                                ))
                    })
                    .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();
    sort_pending_paths_by_source_time(files, &mut pending);
    Ok(PendingBatchScan { pending })
}

fn sort_pending_paths_by_source_time(files: &FileStore, pending: &mut [PathBuf]) {
    pending.sort_by(|left, right| {
        let left_key = source_key(left);
        let right_key = source_key(right);
        let left_record = files.files.get(&left_key);
        let right_record = files.files.get(&right_key);
        pending_source_time_ms(left_record)
            .cmp(&pending_source_time_ms(right_record))
            .then_with(|| pending_modified_time_ms(left_record).cmp(&pending_modified_time_ms(right_record)))
            .then_with(|| left.cmp(right))
    });
}

fn pending_source_time_ms(record: Option<&FileRecord>) -> u64 {
    record
        .and_then(|record| record.source_created_at_ms)
        .unwrap_or(u64::MAX)
}

fn pending_modified_time_ms(record: Option<&FileRecord>) -> u64 {
    record
        .and_then(|record| record.source_modified_ms)
        .unwrap_or(u64::MAX)
}

async fn run_directory_task(
    task: AsrDirectoryTask,
) -> Result<(AsrDirectoryTask, usize, usize), String> {
    let _guard = ASR_JOB_RUN_LOCK.lock().await;
    let _task_lock = TaskRunFileLock::acquire(&task.id)?;
    start_run_progress(&task.id, "background");
    update_run_progress(&task.id, |progress| {
        progress.max_concurrent_files = normalize_max_concurrent_files(task.max_concurrent_files);
        progress.effective_max_concurrent_files = effective_max_concurrent_files(&task);
        progress.active_file_count = 0;
    });
    if task_pause_requested(&task.id) {
        return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
    }
    let mut files = load_file_store(&task.id);
    let reset_count = reset_interrupted_processing_records(&task.id, &mut files);
    if reset_count > 0 {
        tracing::warn!(
            task_id = %task.id,
            reset_count,
            "reset interrupted ASR processing records before starting task run"
        );
    }
    let mut attempted_keys = HashSet::new();
    let mut pending_scan =
        discover_and_prepare_pending_batch(&task, &mut files, &attempted_keys)?;
    update_run_progress(&task.id, |progress| {
        progress.current_file_total = pending_scan.pending.len();
        progress.current_file_index = 0;
        progress.current_chunk_done = 0;
        progress.current_chunk_total = 0;
        progress.message = Some(format!(
            "discovered {} pending file(s)",
            pending_scan.pending.len()
        ));
    });

    if pending_scan.pending.is_empty() {
        refresh_task_daily_summaries(&task)?;
        let updated = match update_task_after_run(&task.id, None) {
            Ok(task) => task,
            Err(error) => {
                warn!(
                    task_id = %task.id, %error,
                    "failed to update task metadata after merge-only ASR run"
                );
                task
            }
        };
        spawn_daily_agent_original_files_after_refresh(&updated);
        return Ok((updated, 0, 0));
    }

    let resource_decision = crate::handlers::speech::acquire_speech_resource(
        crate::handlers::speech::directory_task_lease(&task.id, &task.model),
    );
    if !resource_decision.granted {
        update_run_progress(&task.id, |progress| {
            progress.stage = "paused".to_string();
            progress.stage_message = resource_decision.reason.clone();
            progress.message = Some(
                "directory task is waiting for realtime voice resources to become idle"
                    .to_string(),
            );
        });
        return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
    }
    let _resource_guard = SpeechResourceLeaseGuard {
        lease: resource_decision.lease,
    };

    let target = target_from_query(Some(&format!(
        "language={}&model={}&owner_module=directory_task&owner_id={}",
        urlencoding::encode(&task.language),
        urlencoding::encode(&task.model),
        urlencoding::encode(&task.id)
    )))?;
    tracing::info!(
        task_id = %task.id,
        model = %task.model,
        transcription_mode = task.transcription_mode.as_str(),
        language = %task.language,
        runtime_strategy = task.runtime_strategy.as_str(),
        pending_files = pending_scan.pending.len(),
        "starting ASR directory task run"
    );

    // Plan B (fork-per-chunk): resolve the `asr` binary and model directory
    // instead of starting a long-lived asr-server HTTP daemon. Each chunk
    // will fork a fresh `asr` CLI process, avoiding Metal/MLX state
    // degradation that makes Plan A ~2× slower on batch workloads.
    let asr_bin = target.install_dir().join("asr");
    let model_path = target.model_dir();
    if task.transcription_mode == AsrTranscriptionMode::MossJoint {
        ensure_moss_joint_runtime(&target.home, &task.id).await?;
    } else if !asr_bin.is_file() {
        // Attempt asset repair before giving up.
        crate::handlers::asr::run_initializer_silent_pub(target.clone())
            .await
            .map_err(|e| format!("ASR asset preparation failed: {e}"))?;
        if !asr_bin.is_file() {
            return Err(format!(
                "asr CLI binary not found at {} after asset repair",
                asr_bin.display()
            ));
        }
    }
    if task_uses_standard_runtime(&task) && !model_path.join("tokenizer.json").is_file() {
        return Err(format!(
            "ASR model not found at {} — run ASR initialization first",
            model_path.display()
        ));
    }

    let mut server_url = None::<String>;
    let mut startup_fallback_reason = None::<String>;
    let mut stop_task_server_after_use = false;
    if task_uses_task_lifetime_server(&task) {
        match start_task_managed_server(&task, &target, "task").await {
            Ok(server) => {
                server_url = Some(server.server_url);
                stop_task_server_after_use = server.stop_after_use;
            }
            Err(reason) if task.runtime_strategy == AsrRuntimeStrategy::Auto => {
                tracing::warn!(
                    task_id = %task.id,
                    %reason,
                    "ASR auto strategy falling back to fork_per_chunk during startup"
                );
                startup_fallback_reason = Some(reason);
            }
            Err(reason) => return Err(reason),
        }
    }
    let mut task_server_state = server_url.as_ref().map(|url| ServerRunnerState {
        server_url: url.clone(),
        baseline_rtf: None,
        baseline_samples: Vec::new(),
        server_failures: 0,
        force_fork_for_remaining: startup_fallback_reason.is_some(),
        restart_required: false,
        current_chunk_failure_reason: None,
        fallback_reason: startup_fallback_reason.clone(),
    });

    let pause_check = || {
        task_pause_requested(&task.id)
            || crate::handlers::speech::directory_task_should_yield_for_realtime(&task.id)
    };
    let mut processed_now = 0usize;
    let mut failed_now = 0usize;
    let loop_result: Result<(), String> = loop {
        let pending = std::mem::take(&mut pending_scan.pending);
        for path in &pending {
            attempted_keys.insert(source_key(path));
        }
        let (batch_processed, batch_failed) = match process_pending_files(
            &task,
            &target,
            &asr_bin,
            &model_path,
            server_url.as_deref(),
            startup_fallback_reason.as_deref(),
            &pending,
            processed_now,
            failed_now,
            &mut files,
            &pause_check,
            &mut task_server_state,
            &mut stop_task_server_after_use,
        )
        .await
        {
            Ok(result) => result,
            Err(error) => break Err(error),
        };
        processed_now += batch_processed;
        failed_now += batch_failed;

        files = load_file_store(&task.id);
        pending_scan =
            match discover_and_prepare_pending_batch(&task, &mut files, &attempted_keys) {
                Ok(scan) => scan,
                Err(error) => break Err(error),
            };
        update_run_progress(&task.id, |progress| {
            progress.current_file_index = 0;
            progress.current_file_total = pending_scan.pending.len();
            progress.current_chunk_done = 0;
            progress.current_chunk_total = 0;
            progress.processed_now = processed_now;
            progress.failed_now = failed_now;
            progress.message = if pending_scan.pending.is_empty() {
                Some("no newly appended pending files found".to_string())
            } else {
                Some(format!(
                    "discovered {} appended pending file(s)",
                    pending_scan.pending.len()
                ))
            };
        });
        if pending_scan.pending.is_empty() {
            break Ok(());
        }
        tracing::info!(
            task_id = %task.id,
            pending_files = pending_scan.pending.len(),
            "continuing ASR directory task run with appended files"
        );
    };

    if stop_task_server_after_use {
        tracing::info!(
            task_id = %task.id,
            runtime_strategy = task.runtime_strategy.as_str(),
            "stopping ASR managed server after task-scoped runtime strategy"
        );
        stop_managed_service_for_target(&target).await;
    }

    loop_result?;

    if let Err(error) = refresh_task_daily_summaries(&task) {
        warn!(task_id = %task.id, error = %error, "failed to generate daily summaries");
    }

    // update_task_after_run persists scheduling metadata (next_run_at_ms,
    // last_error).  If it fails, the per-file results in FileStore are
    // already safely persisted — so log a warning instead of discarding
    // the entire run result.
    let updated = match update_task_after_run(
        &task.id,
        (failed_now > 0).then(|| format!("{failed_now} file(s) failed")),
    ) {
        Ok(t) => t,
        Err(error) => {
            warn!(
                task_id = %task.id, %error,
                "failed to update task metadata after run; file results are safe"
            );
            task
        }
    };
    spawn_daily_agent_original_files_after_refresh(&updated);
    // Hook: trigger Daily Agent if configured
    maybe_enqueue_daily_agent_after_asr_run(&updated).await;

    Ok((updated, processed_now, failed_now))
}

struct SpeechResourceLeaseGuard {
    lease: Option<bifrost_asr::resources::ResourceLease>,
}

impl Drop for SpeechResourceLeaseGuard {
    fn drop(&mut self) {
        crate::handlers::speech::release_speech_resource(self.lease.as_ref());
    }
}

fn spawn_directory_task_run_background(task: AsrDirectoryTask) -> Result<(), String> {
    FORCE_PAUSED_TASKS.lock().unwrap().remove(&task.id);
    let running_guard = RunningTaskGuard::acquire(&task.id)
        .map_err(|_| "ASR task is already running".to_string())?;

    let task_id = task.id.clone();
    let task_clone = task.clone();
    tokio::spawn(async move {
        let running_guard = running_guard;
        let result = run_directory_task(task_clone.clone()).await;
        match &result {
            Ok((_updated, processed, failed)) => {
                finish_run_progress(
                    &task_clone.id,
                    "completed",
                    *processed,
                    *failed,
                    Some(format!(
                        "ASR directory task completed; processed {processed}, failed {failed}."
                    )),
                );
                tracing::info!(
                    task_id = %task_clone.id, processed = processed, failed = failed,
                    "ASR directory task completed"
                );
            }
            Err(error) if error == ASR_TASK_PAUSED_MESSAGE => {
                finish_run_progress(
                    &task_clone.id,
                    "paused",
                    0,
                    0,
                    Some("ASR directory task paused and released compute.".to_string()),
                );
                tracing::info!(
                    task_id = %task_clone.id,
                    "ASR directory task paused and released compute"
                );
            }
            Err(error) => {
                let _ = update_task_after_run(&task_clone.id, Some(error.clone()));
                finish_run_progress(
                    &task_clone.id,
                    "failed",
                    0,
                    0,
                    Some(error.clone()),
                );
                tracing::warn!(
                    task_id = %task_clone.id, error = %error,
                    "ASR directory task failed"
                );
            }
        }
        drop(running_guard);
        tracing::debug!(
            task_id = %task_id,
            "released ASR directory task running marker"
        );
    });

    Ok(())
}

fn refresh_task_daily_summaries(task: &AsrDirectoryTask) -> Result<Vec<PathBuf>, String> {
    let task_output_dir = text_output_dir(&bifrost_storage::data_dir()).join(&task.id);
    let paths = generate_daily_summaries(&task_output_dir, &task.name)?;
    tracing::info!(
        task_id = %task.id,
        daily_files = paths.len(),
        "refreshed ASR daily summary markdown files"
    );
    Ok(paths)
}

async fn start_task_managed_server(
    task: &AsrDirectoryTask,
    target: &AsrTarget,
    scope: &str,
) -> Result<TaskManagedServer, String> {
    let before = read_service_state(&bifrost_storage::data_dir());
    let response = start_managed_service(target.clone())
        .await
        .map_err(|response| {
            format!(
                "managed ASR server start failed: {}; detail={}",
                response.message,
                response.detail.unwrap_or_default()
            )
        })?;

    if !response.ready {
        return Err(format!(
            "managed ASR server not ready: {}; detail={}",
            response.message,
            response.detail.unwrap_or_default()
        ));
    }

    let after = read_service_state(&bifrost_storage::data_dir());
    let stop_after_use = !service_state_same_process(before.as_ref(), after.as_ref());
    tracing::info!(
        task_id = %task.id,
        runtime_strategy = task.runtime_strategy.as_str(),
        scope,
        server_url = %response.server_url,
        stop_after_use,
        "ASR runtime strategy acquired managed server"
    );

    Ok(TaskManagedServer {
        server_url: response.server_url,
        stop_after_use,
    })
}

fn service_state_same_process(
    before: Option<&AsrServiceState>,
    after: Option<&AsrServiceState>,
) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => before.pid == after.pid && before.port == after.port,
        _ => false,
    }
}

/// Process all pending files using fork-per-chunk (Plan B).
/// Each file (or chunk of a long file) forks a fresh `asr` CLI process for
/// inference, avoiding Metal/MLX state degradation.
#[allow(clippy::too_many_arguments)]
async fn process_pending_files(
    task: &AsrDirectoryTask,
    target: &AsrTarget,
    asr_bin: &Path,
    model_path: &Path,
    server_url: Option<&str>,
    startup_fallback_reason: Option<&str>,
    pending: &[PathBuf],
    processed_now_base: usize,
    failed_now_base: usize,
    files: &mut FileStore,
    pause_check: &(dyn Fn() -> bool + Send + Sync),
    task_server_state: &mut Option<ServerRunnerState>,
    stop_task_server_after_use: &mut bool,
) -> Result<(usize, usize), String> {
    let effective_concurrency = current_effective_max_concurrent_files(task);
    if effective_concurrency <= 1 {
        return process_pending_files_sequential(
            task,
            target,
            asr_bin,
            model_path,
            server_url,
            startup_fallback_reason,
            pending,
            processed_now_base,
            failed_now_base,
            files,
            pause_check,
            task_server_state,
            stop_task_server_after_use,
        )
        .await;
    }

    process_pending_files_parallel_fork(
        task,
        target,
        asr_bin,
        model_path,
        startup_fallback_reason,
        pending,
        processed_now_base,
        failed_now_base,
        pause_check,
    )
    .await
}

fn current_effective_max_concurrent_files(task: &AsrDirectoryTask) -> usize {
    find_task(&task.id)
        .as_ref()
        .map(effective_max_concurrent_files)
        .unwrap_or_else(|| effective_max_concurrent_files(task)) as usize
}

#[allow(clippy::too_many_arguments)]
async fn process_pending_files_parallel_fork(
    task: &AsrDirectoryTask,
    target: &AsrTarget,
    asr_bin: &Path,
    model_path: &Path,
    startup_fallback_reason: Option<&str>,
    pending: &[PathBuf],
    processed_now_base: usize,
    failed_now_base: usize,
    pause_check: &(dyn Fn() -> bool + Send + Sync),
) -> Result<(usize, usize), String> {
    let mut queue = pending.iter().cloned().collect::<VecDeque<_>>();
    let total_pending = pending.len();
    let mut processed_now = processed_now_base;
    let mut failed_now = failed_now_base;
    let mut completed_now = 0usize;
    let mut join_set = tokio::task::JoinSet::new();

    update_run_progress(&task.id, |progress| {
        progress.current_file_index = 0;
        progress.current_file_total = total_pending;
        progress.processed_now = processed_now;
        progress.failed_now = failed_now;
        progress.max_concurrent_files = normalize_max_concurrent_files(task.max_concurrent_files);
        progress.effective_max_concurrent_files = current_effective_max_concurrent_files(task) as u8;
        progress.active_file_count = 0;
        progress.stage = "asr".to_string();
        progress.stage_message = Some("parallel file transcription".to_string());
        progress.message = Some(format!(
            "processing {total_pending} pending file(s) with up to {} concurrent worker(s)",
            progress.effective_max_concurrent_files
        ));
    });

    while !queue.is_empty() || !join_set.is_empty() {
        if pause_check() {
            join_set.abort_all();
            update_run_progress(&task.id, |progress| {
                progress.active_file_count = 0;
                progress.message = Some("ASR directory task paused; aborting active workers".to_string());
            });
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }

        let effective = current_effective_max_concurrent_files(task).max(1);
        while join_set.len() < effective {
            let Some(path) = queue.pop_front() else {
                break;
            };
            let task = find_task(&task.id).unwrap_or_else(|| task.clone());
            let target = target.clone();
            let asr_bin = asr_bin.to_path_buf();
            let model_path = model_path.to_path_buf();
            let startup_fallback_reason = startup_fallback_reason.map(str::to_string);
            join_set.spawn(async move {
                tokio::task::spawn_blocking(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|error| {
                            format!("failed to create ASR file worker runtime: {error}")
                        })?;
                    runtime.block_on(async move {
                let key = source_key(&path);
                let mut latest_store = load_file_store(&task.id);
                let existing = latest_store.files.remove(&key);
                let mut file_store = FileStore {
                    version: TASK_STORE_VERSION,
                    files: BTreeMap::new(),
                };
                if let Some(record) = existing {
                    file_store.files.insert(key, record);
                }
                let mut file_server_state = None::<ServerRunnerState>;
                let mut stop_after_use = false;
                let worker_pause_check = || {
                    task_pause_requested(&task.id)
                        || crate::handlers::speech::directory_task_should_yield_for_realtime(
                            &task.id,
                        )
                };
                process_pending_files_sequential(
                    &task,
                    &target,
                    &asr_bin,
                    &model_path,
                    None,
                    startup_fallback_reason.as_deref(),
                    std::slice::from_ref(&path),
                    0,
                    0,
                    &mut file_store,
                    &worker_pause_check,
                    &mut file_server_state,
                    &mut stop_after_use,
                )
                .await
                    })
                })
                .await
                .map_err(|error| format!("ASR file worker blocking task failed: {error}"))?
            });
        }

        update_run_progress(&task.id, |progress| {
            progress.current_file_index = completed_now;
            progress.current_file_total = total_pending;
            progress.processed_now = processed_now;
            progress.failed_now = failed_now;
            progress.max_concurrent_files = find_task(&task.id)
                .map(|task| normalize_max_concurrent_files(task.max_concurrent_files))
                .unwrap_or_else(|| normalize_max_concurrent_files(task.max_concurrent_files));
            progress.effective_max_concurrent_files =
                current_effective_max_concurrent_files(task) as u8;
            progress.active_file_count = join_set.len();
            progress.message = Some(format!(
                "processed {completed_now}/{total_pending}; {} active, {} queued",
                join_set.len(),
                queue.len()
            ));
        });

        let result = tokio::select! {
            result = join_set.join_next() => result,
            _ = tokio::time::sleep(Duration::from_secs(2)), if !join_set.is_empty() => {
                continue;
            }
        };
        let Some(result) = result else {
            continue;
        };
        completed_now += 1;
        match result {
            Ok(Ok((processed, failed))) => {
                processed_now += processed;
                failed_now += failed;
            }
            Ok(Err(error)) if error == ASR_TASK_PAUSED_MESSAGE => {
                join_set.abort_all();
                update_run_progress(&task.id, |progress| {
                    progress.active_file_count = 0;
                    progress.message =
                        Some("ASR directory task paused; aborting active workers".to_string());
                });
                return Err(error);
            }
            Ok(Err(error)) => {
                failed_now += 1;
                tracing::warn!(
                    task_id = %task.id,
                    error = %error,
                    "parallel ASR file worker failed"
                );
            }
            Err(error) => {
                failed_now += 1;
                tracing::warn!(
                    task_id = %task.id,
                    error = %error,
                    "parallel ASR file worker panicked or was cancelled"
                );
            }
        }
    }

    update_run_progress(&task.id, |progress| {
        progress.current_file_index = completed_now;
        progress.current_file_total = total_pending;
        progress.processed_now = processed_now;
        progress.failed_now = failed_now;
        progress.active_file_count = 0;
        progress.message = Some(format!(
            "processed {processed_now} file(s), failed {failed_now}"
        ));
    });

    Ok((
        processed_now.saturating_sub(processed_now_base),
        failed_now.saturating_sub(failed_now_base),
    ))
}

fn task_uses_standard_runtime(task: &AsrDirectoryTask) -> bool {
    task.transcription_mode == AsrTranscriptionMode::Standard
}

fn task_uses_external_diarization(task: &AsrDirectoryTask) -> bool {
    task.diarization.enabled && !task.transcription_mode.uses_native_speakers()
}

fn task_uses_task_lifetime_server(task: &AsrDirectoryTask) -> bool {
    task_uses_standard_runtime(task) && task.runtime_strategy.uses_task_lifetime_server()
}

fn task_uses_file_lifetime_server(task: &AsrDirectoryTask) -> bool {
    task_uses_standard_runtime(task) && task.runtime_strategy == AsrRuntimeStrategy::ReusePerFile
}

fn task_initial_processing_stage(task: &AsrDirectoryTask) -> (&'static str, Option<String>) {
    if task_uses_external_diarization(task) {
        (
            "normalize",
            Some(format!(
                "speaker diarization profile: {}",
                task.diarization.profile
            )),
        )
    } else {
        ("asr", None)
    }
}

fn task_external_diarization_profile(task: &AsrDirectoryTask) -> Option<String> {
    task_uses_external_diarization(task).then(|| task.diarization.profile.clone())
}

fn task_asr_stage_message(task: &AsrDirectoryTask) -> &'static str {
    if task.transcription_mode.uses_native_speakers() {
        "jointly transcribing audio with native speaker labels"
    } else if task.diarization.enabled {
        "transcribing diarized audio segments"
    } else {
        "transcribing audio"
    }
}

fn effective_task_model(task: &AsrDirectoryTask) -> String {
    if task.transcription_mode == AsrTranscriptionMode::MossJoint {
        "MOSS-Transcribe-Diarize-MLX-8bit".to_string()
    } else {
        task.model.clone()
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_pending_files_sequential(
    task: &AsrDirectoryTask,
    target: &AsrTarget,
    asr_bin: &Path,
    model_path: &Path,
    server_url: Option<&str>,
    startup_fallback_reason: Option<&str>,
    pending: &[PathBuf],
    processed_now_base: usize,
    failed_now_base: usize,
    files: &mut FileStore,
    pause_check: &(dyn Fn() -> bool + Send + Sync),
    task_server_state: &mut Option<ServerRunnerState>,
    stop_task_server_after_use: &mut bool,
) -> Result<(usize, usize), String> {
    let mut processed_now = processed_now_base;
    let mut failed_now = failed_now_base;

    let total_pending = pending.len();

    for (file_index, path) in pending.iter().enumerate() {
        update_run_progress(&task.id, |progress| {
            progress.current_source_path = Some(path.clone());
            progress.current_file_index = file_index + 1;
            progress.current_file_total = total_pending;
            progress.current_chunk_done = 0;
            progress.current_chunk_total = 0;
            progress.processed_now = processed_now;
            progress.failed_now = failed_now;
            let (stage, stage_message) = task_initial_processing_stage(task);
            progress.stage = stage.to_string();
            progress.stage_message = stage_message;
            progress.message = Some(format!("processing {}", path.display()));
        });
        if pause_check() {
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }
        let key = source_key(path);
        let (partial_text_path, partial_metadata_path, partial_timeline_path) =
            bifrost_asr::artifacts::output_paths_in(
                &bifrost_storage::data_dir(),
                &task.id,
                path,
                &task.audio_dir,
            );
        let existing_content_hash = files
            .files
            .get(&key)
            .and_then(|record| record.content_hash.clone());
        let existing_hash_algorithm = files
            .files
            .get(&key)
            .and_then(|record| record.content_hash_algorithm.clone());
        let existing_memory_limit_hints = files
            .files
            .get(&key)
            .map(|record| record.memory_limit_hints.clone())
            .unwrap_or_default();
        let file_started_at_ms = now_ms();
        // Cache source audio metadata once per file. The end-to-end MOSS RTF
        // budget starts before this ffprobe call, so probing and normalization
        // cannot consume unbounded time outside the 0.5x guard.
        let source_info = inspect_source_audio(path);
        let mut record = file_record_from_info(&task.id, path, &source_info);
        record.memory_limit_hints = existing_memory_limit_hints.clone();
        record.runtime_strategy = task.runtime_strategy;
        record.fallback_reason = startup_fallback_reason.map(str::to_string);
        record.status = FileStatus::Processing;
        record.started_at_ms = Some(file_started_at_ms);
        record.progress_current = Some(0);
        record.progress_total = Some(total_pending);
        files.files.insert(key.clone(), record);
        save_file_store(&task.id, files)?;

        let normalize_result =
            normalize_to_temp(path, Some(pause_check), source_info.media_duration_ms).await;

        let (wav_path, _temp_dir_path) = match normalize_result {
            Ok(result) => result,
            Err(error) if error == ASR_TASK_PAUSED_MESSAGE => {
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = existing_memory_limit_hints.clone();
                record.status = FileStatus::Pending;
                record.started_at_ms = Some(file_started_at_ms);
                record.progress_current = Some(file_index);
                record.progress_total = Some(total_pending);
                files.files.insert(key, record);
                save_file_store(&task.id, files)?;
                return Err(error);
            }
            Err(error) => {
                // Mark the file as failed and continue with the next one.
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = existing_memory_limit_hints.clone();
                record.status = FileStatus::Failed;
                record.started_at_ms = Some(file_started_at_ms);
                record.error = Some(format!("normalize failed: {error}"));
                record.finished_at_ms = Some(now_ms());
                record.progress_current = Some(file_index + 1);
                record.progress_total = Some(total_pending);
                files.files.insert(key, record);
                failed_now += 1;
                save_file_store(&task.id, files)?;
                continue;
            }
        };

        if pause_check() {
            let mut record = file_record_from_info(&task.id, path, &source_info);
            record.memory_limit_hints = existing_memory_limit_hints.clone();
            record.status = FileStatus::Pending;
            record.started_at_ms = Some(file_started_at_ms);
            record.progress_current = Some(file_index);
            record.progress_total = Some(total_pending);
            files.files.insert(key, record);
            let _ = std::fs::remove_dir_all(&_temp_dir_path);
            save_file_store(&task.id, files)?;
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }

        if task_uses_external_diarization(task) {
            update_run_progress(&task.id, |progress| {
                progress.stage = "diarize".to_string();
                progress.stage_message = Some(format!(
                    "identifying speaker turns with {}",
                    task.diarization.profile
                ));
            });
            if !diarization_profile_ready(&task.diarization.profile) {
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = existing_memory_limit_hints.clone();
                record.runtime_strategy = task.runtime_strategy;
                record.status = FileStatus::Failed;
                record.started_at_ms = Some(file_started_at_ms);
                record.error = Some(format!(
                    "diarization_missing_assets: profile '{}' is not initialized",
                    task.diarization.profile
                ));
                record.finished_at_ms = Some(now_ms());
                record.progress_current = Some(file_index + 1);
                record.progress_total = Some(total_pending);
                files.files.insert(key.clone(), record);
                failed_now += 1;
                let _ = std::fs::remove_dir_all(&_temp_dir_path);
                save_file_store(&task.id, files)?;
                update_run_progress(&task.id, |progress| {
                    progress.processed_now = processed_now;
                    progress.failed_now = failed_now;
                    progress.stage = "failed".to_string();
                    progress.stage_message = Some("speaker diarization profile is not initialized".to_string());
                    progress.message = Some(format!(
                        "processed {processed_now} file(s), failed {failed_now}"
                    ));
                });
                continue;
            }
        }

        let mut file_server_url = server_url.map(str::to_string);
        let mut stop_file_server_after_use = false;
        if task_uses_file_lifetime_server(task) {
            match start_task_managed_server(task, target, "file").await {
                Ok(server) => {
                    file_server_url = Some(server.server_url);
                    stop_file_server_after_use = server.stop_after_use;
                }
                Err(error) => {
                    let mut record = file_record_from_info(&task.id, path, &source_info);
                    record.memory_limit_hints = existing_memory_limit_hints.clone();
                    record.runtime_strategy = task.runtime_strategy;
                    record.status = FileStatus::Failed;
                    record.started_at_ms = Some(file_started_at_ms);
                    record.error = Some(error);
                    record.finished_at_ms = Some(now_ms());
                    record.progress_current = Some(file_index + 1);
                    record.progress_total = Some(total_pending);
                    files.files.insert(key, record);
                    failed_now += 1;
                    let _ = std::fs::remove_dir_all(&_temp_dir_path);
                    save_file_store(&task.id, files)?;
                    continue;
                }
            }
        }

        // Construct a progress callback that updates FileStore on each chunk,
        // so the WebUI can show live chunk-level progress.
        let mut file_server_state = file_server_url.as_ref().map(|url| ServerRunnerState {
            server_url: url.clone(),
            baseline_rtf: None,
            baseline_samples: Vec::new(),
            server_failures: 0,
            force_fork_for_remaining: startup_fallback_reason.is_some(),
            restart_required: false,
            current_chunk_failure_reason: None,
            fallback_reason: startup_fallback_reason.map(str::to_string),
        });
        let chunk_progress_cb = {
            let task_id = task.id.clone();
            let key = key.clone();
            let path = path.clone();
            let runtime_strategy = task.runtime_strategy;
            move |chunk_done: usize, chunk_total: usize| {
                update_run_progress(&task_id, |progress| {
                    progress.current_source_path = Some(path.clone());
                    progress.current_chunk_done = chunk_done;
                    progress.current_chunk_total = chunk_total;
                    progress.message = Some(format!("processing chunk {chunk_done}/{chunk_total}"));
                });
                let mut progress_store = FileStore {
                    version: TASK_STORE_VERSION,
                    files: BTreeMap::new(),
                };
                let mut rec = load_file_store(&task_id)
                    .files
                    .remove(&key)
                    .unwrap_or_else(|| FileRecord {
                        task_id: task_id.clone(),
                        source_path: path.clone(),
                        source_size: None,
                        source_modified_ms: None,
                        source_created_at_ms: None,
                        source_created_at_source: None,
                        content_hash: None,
                        content_hash_algorithm: None,
                        duplicate_of_source_key: None,
                        transcript_alias: None,
                        media_duration_ms: None,
                        status: FileStatus::Processing,
                        output_text_path: None,
                        output_metadata_path: None,
                        output_timeline_path: None,
                        text_chars: 0,
                        error: None,
                        runtime_strategy,
                        chunk_metrics: Vec::new(),
                        fallback_reason: None,
                        started_at_ms: Some(now_ms()),
                        finished_at_ms: None,
                        progress_current: Some(chunk_done),
                        progress_total: Some(chunk_total),
                        failed_chunks: Vec::new(),
                        memory_limit_hints: Vec::new(),
                    });
                rec.progress_current = Some(chunk_done);
                rec.progress_total = Some(chunk_total);
                progress_store.files.insert(key.clone(), rec);
                let _ = save_file_store(&task_id, &progress_store);
                tracing::debug!(
                    task_id = %task_id,
                    chunk_done, chunk_total,
                    "chunk progress saved"
                );
            }
        };
        let chunk_metric_cb = {
            let task_id = task.id.clone();
            let key = key.clone();
            let path = path.clone();
            move |metric: AsrChunkMetric| {
                tracing::info!(
                    task_id = %task_id,
                    file = %path.display(),
                    chunk = metric.chunk_index,
                    offset_secs = metric.offset_secs,
                    duration_secs = metric.duration_secs,
                    runner = %metric.runner,
                    status = %metric.status,
                    elapsed_ms = metric.elapsed_ms,
                    rtf = metric.rtf,
                    text_chars = metric.text_chars,
                    fallback_reason = ?metric.fallback_reason,
                    server_url = ?metric.server_url,
                    error = ?metric.error,
                    "ASR chunk metric"
                );
                let mut metric_store = FileStore {
                    version: TASK_STORE_VERSION,
                    files: BTreeMap::new(),
                };
                if let Some(mut rec) = load_file_store(&task_id).files.remove(&key) {
                    rec.chunk_metrics.push(metric);
                    if rec.fallback_reason.is_none() {
                        rec.fallback_reason = rec
                            .chunk_metrics
                            .iter()
                            .find_map(|item| item.fallback_reason.clone());
                    }
                    metric_store.files.insert(key.clone(), rec);
                    let _ = save_file_store(&task_id, &metric_store);
                }
            }
        };

        let use_task_lifetime_server = task_uses_task_lifetime_server(task);
        let partial_artifact_context = PartialArtifactContext {
            task_id: task.id.clone(),
            file_key: key.clone(),
            task_name: task.name.clone(),
            model: task.model.clone(),
            language: task.language.clone(),
            runtime_strategy: task.runtime_strategy,
            source_path: path.clone(),
            source_info: source_info.clone(),
            diarization_profile: task_external_diarization_profile(task),
            speakers: Vec::new(),
            text_path: partial_text_path.clone(),
            metadata_path: partial_metadata_path.clone(),
            timeline_path: partial_timeline_path.clone(),
            started_at_ms: file_started_at_ms,
        };
        update_run_progress(&task.id, |progress| {
            progress.stage = "asr".to_string();
            progress.stage_message = Some(task_asr_stage_message(task).to_string());
        });
        let transcription_result = if use_task_lifetime_server {
            transcribe_file_for_task_with_wav(
                task,
                asr_bin,
                model_path,
                path,
                &wav_path,
                &source_info,
                TaskTranscribeHooks {
                    on_chunk_progress: Some(&chunk_progress_cb),
                    on_chunk_metric: Some(&chunk_metric_cb),
                    pause_check: Some(pause_check),
                    force_pause_task_id: Some(&task.id),
                    memory_limit_hints: &existing_memory_limit_hints,
                    server_url: file_server_url.as_deref(),
                    startup_fallback_reason,
                    server_state: Some(task_server_state),
                    managed_server_restart: Some(ManagedServerRestartContext {
                        task,
                        target,
                        scope: "task",
                        stop_after_use: &mut *stop_task_server_after_use,
                    }),
                    partial_artifacts: Some(partial_artifact_context.clone()),
                    moss_runtime: None,
                },
            )
            .await
        } else {
            transcribe_file_for_task_with_wav(
                task,
                asr_bin,
                model_path,
                path,
                &wav_path,
                &source_info,
                TaskTranscribeHooks {
                    on_chunk_progress: Some(&chunk_progress_cb),
                    on_chunk_metric: Some(&chunk_metric_cb),
                    pause_check: Some(pause_check),
                    force_pause_task_id: Some(&task.id),
                    memory_limit_hints: &existing_memory_limit_hints,
                    server_url: file_server_url.as_deref(),
                    startup_fallback_reason,
                    server_state: Some(&mut file_server_state),
                    managed_server_restart: Some(ManagedServerRestartContext {
                        task,
                        target,
                        scope: "file",
                        stop_after_use: &mut stop_file_server_after_use,
                    }),
                    partial_artifacts: Some(partial_artifact_context),
                    moss_runtime: None,
                },
            )
            .await
        };
        let fallback_reason_after_transcription = if use_task_lifetime_server {
            task_server_state
                .as_ref()
                .and_then(|state| state.fallback_reason.clone())
        } else {
            file_server_state
                .as_ref()
                .and_then(|state| state.fallback_reason.clone())
        };

        match transcription_result {
            Ok(output) => {
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = merge_memory_limit_hints(
                    existing_memory_limit_hints.clone(),
                    output.memory_limit_hints,
                );
                record.runtime_strategy = task.runtime_strategy;
                record.chunk_metrics = output.chunk_metrics;
                record.fallback_reason = output
                    .fallback_reason
                    .or_else(|| fallback_reason_after_transcription.clone());
                record.started_at_ms = Some(file_started_at_ms);
                // If some chunks failed, mark as partial_success instead of success.
                if output.failed_chunks.is_empty() {
                    record.status = FileStatus::Success;
                } else {
                    record.status = FileStatus::PartialSuccess;
                    record.failed_chunks = output.failed_chunks;
                    tracing::warn!(
                        file = %path.display(),
                        failed_count = record.failed_chunks.len(),
                        "file completed with partial failures — failed chunks recorded for later retry"
                    );
                }
                record.media_duration_ms = output.timeline.media_duration_ms;
                record.output_text_path = Some(output.text_path);
                record.output_metadata_path = Some(output.metadata_path);
                record.output_timeline_path = Some(output.timeline_path);
                record.content_hash = existing_content_hash.clone();
                record.content_hash_algorithm = existing_hash_algorithm.clone();
                record.text_chars = output.text.chars().count();
                record.finished_at_ms = Some(now_ms());
                record.progress_current = Some(file_index + 1);
                record.progress_total = Some(total_pending);
                files.files.insert(key.clone(), record.clone());
                index_completed_file_hash(task, &key, &record);
                processed_now += 1;
                update_run_progress(&task.id, |progress| {
                    progress.processed_now = processed_now;
                    progress.failed_now = failed_now;
                    progress.stage = "finalize".to_string();
                    progress.stage_message = Some("speaker timeline and ASR artifacts written".to_string());
                    progress.message = Some(format!(
                        "processed {processed_now} file(s), failed {failed_now}"
                    ));
                });
            }
            Err(error) if error == ASR_TASK_PAUSED_MESSAGE => {
                let latest_record = load_file_store(&task.id).files.get(&key).cloned();
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = existing_memory_limit_hints.clone();
                record.status = FileStatus::Pending;
                record.started_at_ms = Some(file_started_at_ms);
                record.progress_current = Some(file_index);
                record.progress_total = Some(total_pending);
                if let Some(existing) = latest_record.as_ref() {
                    preserve_partial_artifact_fields(&mut record, existing);
                }
                files.files.insert(key, record);
                let _ = std::fs::remove_dir_all(&_temp_dir_path);
                if stop_file_server_after_use {
                    tracing::info!(
                        task_id = %task.id,
                        file = %path.display(),
                        runtime_strategy = task.runtime_strategy.as_str(),
                        "stopping ASR managed server after file-scoped runtime strategy"
                    );
                    stop_managed_service_for_target(target).await;
                }
                save_file_store(&task.id, files)?;
                return Err(error);
            }
            Err(error) => {
                let latest_record = load_file_store(&task.id).files.get(&key).cloned();
                let mut record = file_record_from_info(&task.id, path, &source_info);
                record.memory_limit_hints = existing_memory_limit_hints.clone();
                record.runtime_strategy = task.runtime_strategy;
                record.fallback_reason = fallback_reason_after_transcription
                    .clone()
                    .or_else(|| startup_fallback_reason.map(str::to_string));
                record.status = FileStatus::Failed;
                record.started_at_ms = Some(file_started_at_ms);
                record.error = Some(error);
                record.finished_at_ms = Some(now_ms());
                record.progress_current = Some(file_index + 1);
                record.progress_total = Some(total_pending);
                if let Some(existing) = latest_record.as_ref() {
                    preserve_partial_artifact_fields(&mut record, existing);
                }
                files.files.insert(key, record);
                failed_now += 1;
                update_run_progress(&task.id, |progress| {
                    progress.processed_now = processed_now;
                    progress.failed_now = failed_now;
                    progress.message = Some(format!(
                        "processed {processed_now} file(s), failed {failed_now}"
                    ));
                });
            }
        }
        if stop_file_server_after_use {
            tracing::info!(
                task_id = %task.id,
                file = %path.display(),
                runtime_strategy = task.runtime_strategy.as_str(),
                "stopping ASR managed server after file-scoped runtime strategy"
            );
            stop_managed_service_for_target(target).await;
        }
        // Clean up the temp dir BEFORE propagating save errors, so that a
        // save_file_store failure does not leak the normalize temp directory.
        let _ = std::fs::remove_dir_all(&_temp_dir_path);
        save_file_store(&task.id, files)?;
    }

    Ok((
        processed_now.saturating_sub(processed_now_base),
        failed_now.saturating_sub(failed_now_base),
    ))
}

/// Transcribe a pre-normalized WAV file for a directory task using
/// fork-per-chunk (Plan B). The `wav_path` must already be a 16kHz mono PCM WAV.
async fn transcribe_file_for_task_with_wav(
    task: &AsrDirectoryTask,
    asr_bin: &Path,
    model_path: &Path,
    path: &Path,
    wav: &Path,
    source_info: &SourceAudioInfo,
    mut hooks: TaskTranscribeHooks<'_>,
) -> Result<TranscriptionOutput, String> {
    let temp = tempfile::tempdir().map_err(|error| format!("create temp dir: {error}"))?;

    // For long audio (> chunk threshold), split into fixed-duration chunks and
    // process each via a separate `asr` CLI fork.
    const CHUNK_DURATION_SECS: u64 = ASR_TASK_SEGMENT_MAX_MS / 1000;
    // Overlap between consecutive chunks avoids cutting words at boundaries.
    // The `dedupe_increment` function de-duplicates the overlapping text.
    const CHUNK_OVERLAP_SECS: u64 = 2;
    let duration_ms = source_info.media_duration_ms.unwrap_or(0);
    let duration_secs = duration_ms.div_ceil(1000);

    let mut diarized_timeline_segments = None::<Vec<TimelineSegment>>;
    let mut diarized_speakers = None::<Vec<TimelineSpeaker>>;
    let mut diarization_segments_for_manifest = None::<Vec<DiarizationSegment>>;
    let (
        result,
        failed_chunks,
        result_memory_limit_hints,
        result_chunk_metrics,
        result_fallback_reason,
    ) = if task.transcription_mode == AsrTranscriptionMode::MossJoint {
        if hooks.pause_check.is_some_and(|check| check()) {
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }
        let default_runtime;
        let runtime = if let Some(runtime) = hooks.moss_runtime {
            runtime
        } else {
            default_runtime = moss_runtime_paths(&bifrost_asr::runtime::fixed_asr_home());
            &default_runtime
        };
        validate_moss_audio_input(wav, duration_ms)?;
        let file_started_at_ms = hooks
            .partial_artifacts
            .as_ref()
            .map(|context| context.started_at_ms);
        let moss_started = Instant::now();
        let result = run_moss_joint_transcription(
            runtime,
            wav,
            duration_ms,
            &task.transcription_prompt,
            hooks.pause_check,
            file_started_at_ms,
        )
        .await;
        let elapsed_ms = moss_started
            .elapsed()
            .as_millis()
            .clamp(1, u128::from(u64::MAX)) as u64;
        let metric = chunk_metric(
            0,
            0,
            duration_secs.max(1),
            "moss_joint",
            &result,
            elapsed_ms,
            None,
            None,
        );
        let result = result?;
        if let Some(callback) = hooks.on_chunk_metric {
            callback(metric.clone());
        }
        (result, Vec::new(), Vec::new(), vec![metric], None)
    } else if task.diarization.enabled {
        let diarized = transcribe_diarized_segments_for_task(
            task,
            asr_bin,
            model_path,
            wav,
            temp.path(),
            hooks,
        )
        .await?;
        let result = WholeFileTranscription {
            text: diarized.text,
            segments: Vec::new(),
            structured: Default::default(),
        };
        diarized_timeline_segments = Some(diarized.timeline_segments);
        diarized_speakers = Some(diarized.speakers);
        diarization_segments_for_manifest = Some(diarized.diarization_segments);
        (
            result,
            diarized.failed_chunks,
            diarized.memory_limit_hints,
            diarized.chunk_metrics,
            diarized.fallback_reason,
        )
    } else if duration_ms > CHUNK_DURATION_SECS * 1000 {
        // Split into chunks and process each via fork-per-chunk.
        let partial_artifacts = hooks.partial_artifacts.clone();
        let chunked = transcribe_in_chunks(
            asr_bin,
            model_path,
            &task.language,
            wav,
            temp.path(),
            duration_secs,
            CHUNK_DURATION_SECS,
            CHUNK_OVERLAP_SECS,
            duration_ms,
            hooks.on_chunk_progress,
            hooks.pause_check,
            hooks.force_pause_task_id,
            &task.model,
            hooks.memory_limit_hints,
            task.runtime_strategy,
            hooks.server_url,
            hooks.startup_fallback_reason,
            hooks.server_state,
            hooks.managed_server_restart,
            hooks.on_chunk_metric,
            partial_artifacts.as_ref(),
        )
        .await?;
        (
            chunked.transcription,
            chunked.failed_chunks,
            chunked.memory_limit_hints,
            chunked.chunk_metrics,
            chunked.fallback_reason,
        )
    } else {
        if hooks.pause_check.is_some_and(|check| check()) {
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }
        let mut local_server_state;
        let server_state = if let Some(server_state) = hooks.server_state {
            server_state
        } else {
            local_server_state = hooks.server_url.map(|url| ServerRunnerState {
                server_url: url.to_string(),
                baseline_rtf: None,
                baseline_samples: Vec::new(),
                server_failures: 0,
                force_fork_for_remaining: hooks.startup_fallback_reason.is_some(),
                restart_required: false,
                current_chunk_failure_reason: None,
                fallback_reason: hooks.startup_fallback_reason.map(str::to_string),
            });
            &mut local_server_state
        };
        prepare_managed_server_for_chunk(
            task.runtime_strategy,
            server_state,
            hooks.managed_server_restart.as_mut(),
        )
        .await;
        let attempt = run_chunk_with_strategy(
            task.runtime_strategy,
            asr_bin,
            model_path,
            &task.language,
            wav,
            0,
            duration_secs.max(1),
            0,
            temp.path(),
            hooks.force_pause_task_id,
            server_state,
            hooks.managed_server_restart.as_mut(),
            None,
        )
        .await?;
        if let Some(cb) = hooks.on_chunk_metric {
            cb(attempt.metric.clone());
            for metric in &attempt.shadow_metrics {
                cb(metric.clone());
            }
        }
        let mut chunk_metrics = vec![attempt.metric.clone()];
        chunk_metrics.extend(attempt.shadow_metrics.clone());
        (
            attempt.result?,
            Vec::new(),
            Vec::new(),
            chunk_metrics,
            server_state
                .as_ref()
                .and_then(|state| state.fallback_reason.clone())
                .or_else(|| hooks.startup_fallback_reason.map(str::to_string)),
        )
    };

    let native_joint_segments = result.structured.segments.clone();
    let mut segments: Vec<TimelineSegment> =
        if let Some(diarized_segments) = diarized_timeline_segments.take() {
            diarized_segments
                .into_iter()
                .enumerate()
                .map(|(index, mut segment)| {
                    segment.index = index;
                    segment.absolute_start_ms = source_info
                        .source_created_at_ms
                        .map(|start| start.saturating_add(segment.audio_start_ms));
                    segment.absolute_end_ms = source_info
                        .source_created_at_ms
                        .map(|start| start.saturating_add(segment.audio_end_ms));
                    segment
                })
                .collect()
        } else if !native_joint_segments.is_empty() {
            native_joint_segments
                .into_iter()
                .enumerate()
                .filter_map(|(index, segment)| {
                    if segment.text.trim().is_empty() || segment.end_ms <= segment.start_ms {
                        return None;
                    }
                    Some(TimelineSegment {
                        index,
                        audio_start_ms: segment.start_ms,
                        audio_end_ms: segment.end_ms,
                        absolute_start_ms: source_info
                            .source_created_at_ms
                            .map(|start| start.saturating_add(segment.start_ms)),
                        absolute_end_ms: source_info
                            .source_created_at_ms
                            .map(|start| start.saturating_add(segment.end_ms)),
                        speaker: segment.normalized_speaker().map(str::to_string),
                        speaker_display_name: segment.normalized_speaker().map(str::to_string),
                        overlap: segment.overlap,
                        text: segment.text,
                    })
                })
                .collect()
        } else {
            result
                .segments
                .into_iter()
                .enumerate()
                .map(
                    |(index, (audio_start_ms, audio_end_ms, text))| TimelineSegment {
                        index,
                        audio_start_ms,
                        audio_end_ms,
                        absolute_start_ms: source_info
                            .source_created_at_ms
                            .map(|start| start.saturating_add(audio_start_ms)),
                        absolute_end_ms: source_info
                            .source_created_at_ms
                            .map(|start| start.saturating_add(audio_end_ms)),
                        speaker: None,
                        speaker_display_name: None,
                        overlap: false,
                        text,
                    },
                )
                .collect()
        };

    let text = result.text;

    // The native `asr` CLI returns plain text only (no per-word timestamps).
    // Synthesize a single segment spanning the entire file duration so that
    // the timeline/daily-summary pipeline has something to work with.
    if segments.is_empty() && !text.is_empty() {
        segments.push(TimelineSegment {
            index: 0,
            audio_start_ms: 0,
            audio_end_ms: duration_ms,
            absolute_start_ms: source_info.source_created_at_ms,
            absolute_end_ms: source_info
                .source_created_at_ms
                .map(|start| start.saturating_add(duration_ms)),
            speaker: None,
            speaker_display_name: None,
            overlap: false,
            text: text.clone(),
        });
    }
    let effective_model = effective_task_model(task);
    let mut timeline = TranscriptTimeline {
        task_id: task.id.clone(),
        task_name: task.name.clone(),
        source_path: path.to_path_buf(),
        source_size: source_info.source_size,
        source_modified_ms: source_info.source_modified_ms,
        source_created_at_ms: source_info.source_created_at_ms,
        source_created_at_source: source_info.source_created_at_source.clone(),
        media_duration_ms: source_info.media_duration_ms,
        model: effective_model.clone(),
        language: task.language.clone(),
        diarization_profile: None,
        speakers: Vec::new(),
        processed_at_ms: now_ms(),
        segments,
    };
    if task.transcription_mode.uses_native_speakers() {
        let speaker_ids = timeline
            .segments
            .iter()
            .filter_map(|segment| segment.speaker.clone())
            .collect::<std::collections::BTreeSet<_>>();
        timeline.diarization_profile = Some("moss_joint_native".to_string());
        timeline.speakers = speaker_ids
            .into_iter()
            .map(|id| TimelineSpeaker {
                display_name: id.clone(),
                id,
                mapped_profile_id: None,
                confidence: None,
                candidate_profile_id: None,
                candidate_display_name: None,
                candidate_confidence: None,
            })
            .collect();
    } else if let Some(diarization_segments) = diarization_segments_for_manifest.as_deref() {
        timeline.diarization_profile = Some(task.diarization.profile.clone());
        timeline.speakers = diarized_speakers.unwrap_or_default();
        write_diarization_manifest(
            task,
            &timeline,
            timeline.speakers.clone(),
            diarization_segments,
        )?;
    } else {
        apply_diarization_to_timeline(task, &mut timeline, wav)?;
    }
    let metadata = serde_json::json!({
        "task_id": task.id,
        "task_name": task.name,
        "source_path": path,
        "source_size": timeline.source_size,
        "source_modified_ms": timeline.source_modified_ms,
        "source_created_at_ms": timeline.source_created_at_ms,
        "source_created_at_source": timeline.source_created_at_source,
        "media_duration_ms": timeline.media_duration_ms,
        "model": effective_model,
        "transcription_mode": task.transcription_mode,
        "transcription_prompt_configured": !task.transcription_prompt.is_empty(),
        "language": task.language,
        "runtime_strategy": task.runtime_strategy,
        "fallback_reason": result_fallback_reason.clone(),
        "chunk_metrics": result_chunk_metrics.clone(),
        "processed_at_ms": timeline.processed_at_ms,
    });
    let artifact_paths = write_offline_subtitle_artifacts(OfflineSubtitleArtifactRequest {
        data_dir: &bifrost_storage::data_dir(),
        task_id: &task.id,
        source_path: path,
        audio_dir: &task.audio_dir,
        timeline: &timeline,
        fallback_text: &text,
        metadata,
    })?;
    Ok(TranscriptionOutput {
        text,
        text_path: artifact_paths.text_path,
        metadata_path: artifact_paths.metadata_path,
        timeline_path: artifact_paths.timeline_path,
        timeline,
        failed_chunks,
        memory_limit_hints: result_memory_limit_hints,
        chunk_metrics: result_chunk_metrics,
        fallback_reason: result_fallback_reason,
    })
}

#[allow(clippy::too_many_arguments)]
async fn transcribe_diarized_segments_for_task(
    task: &AsrDirectoryTask,
    asr_bin: &Path,
    model_path: &Path,
    wav: &Path,
    temp_dir: &Path,
    mut hooks: TaskTranscribeHooks<'_>,
) -> Result<DiarizedTranscriptionOutput, String> {
    if !diarization_profile_ready(&task.diarization.profile) {
        return Err(format!(
            "diarization_missing_assets: profile '{}' is not initialized",
            task.diarization.profile
        ));
    }
    let diarization_segments =
        run_sherpa_diarization(&task.diarization, &task.diarization.profile, wav)?;
    if diarization_segments.is_empty() {
        return Err("diarization_no_segments: sherpa-onnx returned no speaker segments".to_string());
    }

    let mut local_server_state;
    let server_state = if let Some(server_state) = hooks.server_state {
        server_state
    } else {
        local_server_state = hooks.server_url.map(|url| ServerRunnerState {
            server_url: url.to_string(),
            baseline_rtf: None,
            baseline_samples: Vec::new(),
            server_failures: 0,
            force_fork_for_remaining: hooks.startup_fallback_reason.is_some(),
            restart_required: false,
            current_chunk_failure_reason: None,
            fallback_reason: hooks.startup_fallback_reason.map(str::to_string),
        });
        &mut local_server_state
    };
    prepare_managed_server_for_chunk(
        task.runtime_strategy,
        server_state,
        hooks.managed_server_restart.as_mut(),
    )
    .await;

    let speakers = speakers_from_diarization_segments(&diarization_segments);
    if let Some(context) = hooks.partial_artifacts.as_mut() {
        context.speakers = speakers.clone();
    }
    let asr_units = plan_asr_units(&diarization_segments, &AsrUnitPlannerConfig::default());
    if asr_units.is_empty() {
        return Err("diarization_no_asr_units: diarization produced no transcribable ASR units"
            .to_string());
    }
    let total_units = asr_units.len();
    let mut all_text = String::new();
    let mut timeline_segments = Vec::new();
    let mut chunk_metrics = Vec::new();
    let failed_chunks = Vec::new();
    let mut fallback_reason = server_state
        .as_ref()
        .and_then(|state| state.fallback_reason.clone())
        .or_else(|| hooks.startup_fallback_reason.map(str::to_string));

    for (index, asr_unit) in asr_units.iter().enumerate() {
        if hooks.pause_check.is_some_and(|check| check()) {
            return Err(ASR_TASK_PAUSED_MESSAGE.to_string());
        }
        if let Some(callback) = hooks.on_chunk_progress {
            callback(index, total_units);
        }
        let segment_wav = temp_dir.join(format!("{}.wav", asr_unit.unit_id));
        ffmpeg_cut_wav_ms(
            wav,
            &segment_wav,
            asr_unit.source_start_ms,
            asr_unit.source_end_ms,
            hooks.pause_check,
        )
        .await?;
        if compute_wav_rms_energy(&segment_wav).is_some_and(|rms| rms < SILENCE_RMS_THRESHOLD) {
            continue;
        }
        let duration_ms = asr_unit
            .source_end_ms
            .saturating_sub(asr_unit.source_start_ms);
        let duration_secs = duration_ms.div_ceil(1000).max(1);
        let attempt = run_chunk_with_strategy(
            task.runtime_strategy,
            asr_bin,
            model_path,
            &task.language,
            &segment_wav,
            asr_unit.source_start_ms / 1000,
            duration_secs,
            index,
            temp_dir,
            hooks.force_pause_task_id,
            server_state,
            hooks.managed_server_restart.as_mut(),
            None,
        )
        .await?;
        if let Some(callback) = hooks.on_chunk_metric {
            callback(attempt.metric.clone());
            for metric in &attempt.shadow_metrics {
                callback(metric.clone());
            }
        }
        chunk_metrics.push(attempt.metric.clone());
        chunk_metrics.extend(attempt.shadow_metrics.clone());
        fallback_reason = server_state
            .as_ref()
            .and_then(|state| state.fallback_reason.clone())
            .or_else(|| fallback_reason.clone());

        let chunk_result = attempt.result?;
        append_diarized_segment_result(
            &mut all_text,
            &mut timeline_segments,
            asr_unit,
            chunk_result,
        );
        if let Some(context) = hooks.partial_artifacts.as_ref() {
            persist_partial_transcription_artifacts(
                context,
                DiarizedSegmentProgress {
                    text: all_text.clone(),
                    timeline_segments: timeline_segments.clone(),
                    chunk_metrics: chunk_metrics.clone(),
                    fallback_reason: fallback_reason.clone(),
                },
            )?;
        }
    }

    if let Some(callback) = hooks.on_chunk_progress {
        callback(total_units, total_units);
    }

    Ok(DiarizedTranscriptionOutput {
        text: all_text,
        timeline_segments,
        speakers,
        diarization_segments,
        failed_chunks,
        memory_limit_hints: hooks.memory_limit_hints.to_vec(),
        chunk_metrics,
        fallback_reason,
    })
}

fn append_diarized_segment_result(
    all_text: &mut String,
    timeline_segments: &mut Vec<TimelineSegment>,
    asr_unit: &AsrAudioUnit,
    chunk_result: WholeFileTranscription,
) {
    let chunk_text = chunk_result.text.trim();
    if chunk_text.is_empty() && chunk_result.segments.is_empty() {
        return;
    }
    if !chunk_text.is_empty() {
        if !all_text.is_empty() {
            all_text.push('\n');
        }
        all_text.push_str(chunk_text);
    }
    if chunk_result.segments.is_empty() {
        timeline_segments.push(TimelineSegment {
            index: timeline_segments.len(),
            audio_start_ms: asr_unit.source_start_ms,
            audio_end_ms: asr_unit.source_end_ms,
            absolute_start_ms: None,
            absolute_end_ms: None,
            speaker: asr_unit.speaker.clone(),
            speaker_display_name: asr_unit.speaker_display_name.clone(),
            overlap: asr_unit.overlap,
            text: chunk_text.to_string(),
        });
        return;
    }
    for (local_start_ms, local_end_ms, text) in chunk_result.segments {
        if text.trim().is_empty() {
            continue;
        }
        let audio_start_ms = asr_unit.source_start_ms.saturating_add(local_start_ms);
        let audio_end_ms = asr_unit
            .source_start_ms
            .saturating_add(local_end_ms)
            .min(asr_unit.source_end_ms);
        if audio_end_ms <= audio_start_ms {
            continue;
        }
        timeline_segments.push(TimelineSegment {
            index: timeline_segments.len(),
            audio_start_ms,
            audio_end_ms,
            absolute_start_ms: None,
            absolute_end_ms: None,
            speaker: asr_unit.speaker.clone(),
            speaker_display_name: asr_unit.speaker_display_name.clone(),
            overlap: asr_unit.overlap,
            text,
        });
    }
}

fn timeline_segments_from_plain_chunks(
    segments: &[(u64, u64, String)],
    context: &PartialArtifactContext,
) -> Vec<TimelineSegment> {
    segments
        .iter()
        .enumerate()
        .map(|(index, (audio_start_ms, audio_end_ms, text))| TimelineSegment {
            index,
            audio_start_ms: *audio_start_ms,
            audio_end_ms: *audio_end_ms,
            absolute_start_ms: context
                .source_info
                .source_created_at_ms
                .map(|start| start.saturating_add(*audio_start_ms)),
            absolute_end_ms: context
                .source_info
                .source_created_at_ms
                .map(|start| start.saturating_add(*audio_end_ms)),
            speaker: None,
            speaker_display_name: None,
            overlap: false,
            text: text.clone(),
        })
        .collect()
}

fn preserve_partial_artifact_fields(record: &mut FileRecord, existing: &FileRecord) {
    if existing.output_text_path.is_some() {
        record.output_text_path = existing.output_text_path.clone();
    }
    if existing.output_metadata_path.is_some() {
        record.output_metadata_path = existing.output_metadata_path.clone();
    }
    if existing.output_timeline_path.is_some() {
        record.output_timeline_path = existing.output_timeline_path.clone();
    }
    if existing.text_chars > 0 {
        record.text_chars = existing.text_chars;
    }
    if existing.media_duration_ms.is_some() {
        record.media_duration_ms = existing.media_duration_ms;
    }
    if !existing.chunk_metrics.is_empty() {
        record.chunk_metrics = existing.chunk_metrics.clone();
    }
    if record.fallback_reason.is_none() {
        record.fallback_reason = existing.fallback_reason.clone();
    }
}

fn persist_partial_transcription_artifacts(
    context: &PartialArtifactContext,
    progress: DiarizedSegmentProgress,
) -> Result<(), String> {
    let mut segments = progress.timeline_segments;
    for (index, segment) in segments.iter_mut().enumerate() {
        segment.index = index;
        if segment.absolute_start_ms.is_none() {
            segment.absolute_start_ms = context
                .source_info
                .source_created_at_ms
                .map(|start| start.saturating_add(segment.audio_start_ms));
        }
        if segment.absolute_end_ms.is_none() {
            segment.absolute_end_ms = context
                .source_info
                .source_created_at_ms
                .map(|start| start.saturating_add(segment.audio_end_ms));
        }
    }
    let timeline = TranscriptTimeline {
        task_id: context.task_id.clone(),
        task_name: context.task_name.clone(),
        source_path: context.source_path.clone(),
        source_size: context.source_info.source_size,
        source_modified_ms: context.source_info.source_modified_ms,
        source_created_at_ms: context.source_info.source_created_at_ms,
        source_created_at_source: context.source_info.source_created_at_source.clone(),
        media_duration_ms: context.source_info.media_duration_ms,
        model: context.model.clone(),
        language: context.language.clone(),
        diarization_profile: context.diarization_profile.clone(),
        speakers: context.speakers.clone(),
        processed_at_ms: now_ms(),
        segments,
    };
    let rendered = render_timeline_text(&timeline, &progress.text);
    atomic_text_write(&context.text_path, &rendered)?;
    atomic_json_write(&context.timeline_path, &timeline)?;
    let srt_path = bifrost_asr::artifacts::subtitle_path_from_timeline(&context.timeline_path, "srt");
    let vtt_path = bifrost_asr::artifacts::subtitle_path_from_timeline(&context.timeline_path, "vtt");
    atomic_text_write(&srt_path, &bifrost_asr::subtitle::render_srt(&timeline))?;
    atomic_text_write(&vtt_path, &bifrost_asr::subtitle::render_vtt(&timeline))?;

    let text_chars = rendered.chars().count();
    let chunk_metrics = progress.chunk_metrics.clone();
    let fallback_reason = progress.fallback_reason.clone();
    let mut metadata = serde_json::json!({
        "task_id": context.task_id,
        "task_name": context.task_name,
        "source_path": context.source_path,
        "source_size": timeline.source_size,
        "source_modified_ms": timeline.source_modified_ms,
        "source_created_at_ms": timeline.source_created_at_ms,
        "source_created_at_source": timeline.source_created_at_source,
        "media_duration_ms": timeline.media_duration_ms,
        "model": context.model,
        "language": context.language,
        "runtime_strategy": context.runtime_strategy,
        "fallback_reason": fallback_reason,
        "chunk_metrics": chunk_metrics,
        "processed_at_ms": timeline.processed_at_ms,
        "partial": true,
        "partial_started_at_ms": context.started_at_ms,
        "partial_segment_count": timeline.segments.len(),
        "text_path": context.text_path,
        "metadata_path": context.metadata_path,
        "timeline_path": context.timeline_path,
        "srt_path": srt_path,
        "vtt_path": vtt_path,
    });
    if let Some(profile) = &context.diarization_profile {
        metadata["diarization_profile"] = serde_json::Value::String(profile.clone());
    }
    atomic_json_write(&context.metadata_path, &metadata)?;

    let mut store = load_file_store(&context.task_id);
    if let Some(record) = store.files.get_mut(&context.file_key) {
        if matches!(record.status, FileStatus::Pending | FileStatus::Processing) {
            record.status = FileStatus::Processing;
        }
        record.output_text_path = Some(context.text_path.clone());
        record.output_metadata_path = Some(context.metadata_path.clone());
        record.output_timeline_path = Some(context.timeline_path.clone());
        record.media_duration_ms = timeline.media_duration_ms;
        record.text_chars = text_chars;
        record.chunk_metrics = chunk_metrics;
        if record.fallback_reason.is_none() {
            record.fallback_reason = fallback_reason;
        }
        record.started_at_ms.get_or_insert(context.started_at_ms);
    }
    save_file_store(&context.task_id, &store)
}

#[cfg(test)]
mod runner_tests {
    use super::*;
    use crate::asr_runtime::AsrServiceState;
    use std::path::PathBuf;

    fn sample_file_record(created: Option<u64>, modified: Option<u64>) -> FileRecord {
        FileRecord {
            task_id: "task".to_string(),
            source_path: PathBuf::from("/tmp/audio.wav"),
            source_size: None,
            source_modified_ms: modified,
            source_created_at_ms: created,
            source_created_at_source: None,
            content_hash: None,
            content_hash_algorithm: None,
            duplicate_of_source_key: None,
            transcript_alias: None,
            media_duration_ms: None,
            status: FileStatus::Pending,
            output_text_path: None,
            output_metadata_path: None,
            output_timeline_path: None,
            text_chars: 0,
            error: None,
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            chunk_metrics: Vec::new(),
            fallback_reason: None,
            started_at_ms: None,
            finished_at_ms: None,
            progress_current: None,
            progress_total: None,
            failed_chunks: Vec::new(),
            memory_limit_hints: Vec::new(),
        }
    }

    fn sample_state(pid: Option<u32>, port: u16) -> AsrServiceState {
        AsrServiceState {
            host: "127.0.0.1".to_string(),
            port,
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "chinese".to_string(),
            home: crate::handlers::asr::default_home(),
            pid,
            managed_by: "test".to_string(),
            owner_module: Some("test".to_string()),
            owner_id: None,
            started_at_ms: 1,
        }
    }

    #[test]
    fn pending_source_and_modified_time_ms_use_defaults_when_missing() {
        let record = sample_file_record(Some(123), Some(456));
        assert_eq!(pending_source_time_ms(Some(&record)), 123);
        assert_eq!(pending_modified_time_ms(Some(&record)), 456);

        assert_eq!(pending_source_time_ms(None), u64::MAX);
        assert_eq!(pending_modified_time_ms(None), u64::MAX);

        let missing_times = sample_file_record(None, None);
        assert_eq!(pending_source_time_ms(Some(&missing_times)), u64::MAX);
        assert_eq!(pending_modified_time_ms(Some(&missing_times)), u64::MAX);
    }

    #[test]
    fn service_state_same_process_requires_matching_pid_and_port() {
        let before = sample_state(Some(12345), 60_000);
        let same = sample_state(Some(12345), 60_000);
        let different_pid = sample_state(Some(12346), 60_000);
        let different_port = sample_state(Some(12345), 60_001);

        assert!(service_state_same_process(Some(&before), Some(&same)));
        assert!(!service_state_same_process(Some(&before), Some(&different_pid)));
        assert!(!service_state_same_process(Some(&before), Some(&different_port)));
        assert!(!service_state_same_process(Some(&before), None));
        assert!(!service_state_same_process(None, Some(&same)));
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;
    use crate::asr_streaming::WholeFileTranscription;
    use bifrost_asr::planner::AsrUnitKind;
    use std::path::PathBuf;

    /// Helper to build a minimal FileRecord with configurable timestamps.
    fn make_file_record(created: Option<u64>, modified: Option<u64>) -> FileRecord {
        FileRecord {
            task_id: "task".to_string(),
            source_path: PathBuf::from("/tmp/audio.wav"),
            source_size: None,
            source_modified_ms: modified,
            source_created_at_ms: created,
            source_created_at_source: None,
            content_hash: None,
            content_hash_algorithm: None,
            duplicate_of_source_key: None,
            transcript_alias: None,
            media_duration_ms: None,
            status: FileStatus::Pending,
            output_text_path: None,
            output_metadata_path: None,
            output_timeline_path: None,
            text_chars: 0,
            error: None,
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            chunk_metrics: Vec::new(),
            fallback_reason: None,
            started_at_ms: None,
            finished_at_ms: None,
            progress_current: None,
            progress_total: None,
            failed_chunks: Vec::new(),
            memory_limit_hints: Vec::new(),
        }
    }

    fn make_asr_unit(start_ms: u64, end_ms: u64) -> AsrAudioUnit {
        AsrAudioUnit {
            unit_id: "unit_00".to_string(),
            speaker: Some("speaker_00".to_string()),
            speaker_display_name: Some("用户A".to_string()),
            mapped_profile_id: None,
            confidence: Some(0.9),
            source_start_ms: start_ms,
            source_end_ms: end_ms,
            source_segment_ids: vec!["seg0".to_string()],
            overlap: false,
            unit_kind: AsrUnitKind::Segment,
        }
    }

    // --- pending_* helpers -------------------------------------------------

    #[test]
    fn pending_source_time_ms_and_modified_time_ms_cover_all_branches() {
        let record = make_file_record(Some(10), Some(20));
        assert_eq!(pending_source_time_ms(Some(&record)), 10);
        assert_eq!(pending_modified_time_ms(Some(&record)), 20);

        let missing_both = make_file_record(None, None);
        assert_eq!(pending_source_time_ms(Some(&missing_both)), u64::MAX);
        assert_eq!(pending_modified_time_ms(Some(&missing_both)), u64::MAX);

        assert_eq!(pending_source_time_ms(None), u64::MAX);
        assert_eq!(pending_modified_time_ms(None), u64::MAX);
    }

    #[test]
    fn service_state_same_process_covers_all_input_combinations() {
        use crate::asr_runtime::AsrServiceState;

        fn state(pid: Option<u32>, port: u16) -> AsrServiceState {
            AsrServiceState {
                host: "127.0.0.1".to_string(),
                port,
                model: "Qwen3-ASR-0.6B".to_string(),
                language: "chinese".to_string(),
                home: crate::handlers::asr::default_home(),
                pid,
                managed_by: "test".to_string(),
                owner_module: Some("test".to_string()),
                owner_id: None,
                started_at_ms: 1,
            }
        }

        let base = state(Some(100), 6_0000);
        let same = state(Some(100), 6_0000);
        let different_pid = state(Some(101), 6_0000);
        let different_port = state(Some(100), 6_0001);

        assert!(service_state_same_process(Some(&base), Some(&same)));
        assert!(!service_state_same_process(Some(&base), Some(&different_pid)));
        assert!(!service_state_same_process(Some(&base), Some(&different_port)));
        assert!(!service_state_same_process(Some(&base), None));
        assert!(!service_state_same_process(None, Some(&same)));
        assert!(!service_state_same_process(None, None));
    }

    // --- sort_pending_paths_by_source_time ---------------------------------

    #[test]
    fn sort_pending_paths_orders_by_created_then_modified_then_path() {
        let mut store = FileStore {
            version: 1,
            files: BTreeMap::new(),
        };

        let a = PathBuf::from("/audio/a.wav");
        let b = PathBuf::from("/audio/b.wav");
        let c = PathBuf::from("/audio/c.wav");

        let key_a = source_key(&a);
        let key_b = source_key(&b);
        let key_c = source_key(&c);

        store.files.insert(key_a.clone(), make_file_record(Some(30), Some(10)));
        store.files.insert(key_b.clone(), make_file_record(Some(20), Some(50)));
        store.files.insert(key_c.clone(), make_file_record(Some(20), Some(40)));

        let mut pending = vec![a.clone(), b.clone(), c.clone()];
        sort_pending_paths_by_source_time(&store, &mut pending);

        // Earliest created_at_ms wins (b and c before a), then modified_ms (c before b).
        assert_eq!(pending, vec![c, b, a]);
    }

    #[test]
    fn sort_pending_paths_uses_defaults_when_records_missing() {
        let store = FileStore {
            version: 1,
            files: BTreeMap::new(),
        };
        let p1 = PathBuf::from("/audio/x.wav");
        let p2 = PathBuf::from("/audio/y.wav");
        let mut pending = vec![p2.clone(), p1.clone()];

        sort_pending_paths_by_source_time(&store, &mut pending);

        // With no records, paths fall back to lexical ordering.
        assert_eq!(pending, vec![p1, p2]);
    }

    // --- append_diarized_segment_result ------------------------------------

    #[test]
    fn append_diarized_segment_result_noop_for_empty_text_and_segments() {
        let unit = make_asr_unit(0, 10_000);
        let mut all_text = String::new();
        let mut timeline = Vec::new();

        append_diarized_segment_result(
            &mut all_text,
            &mut timeline,
            &unit,
            WholeFileTranscription {
                text: "  ".to_string(),
                segments: Vec::new(),
                structured: Default::default(),
            },
        );

        assert!(all_text.is_empty());
        assert!(timeline.is_empty());
    }

    #[test]
    fn append_diarized_segment_result_appends_text_and_creates_single_segment() {
        let unit = make_asr_unit(1_000, 3_000);
        let mut all_text = String::new();
        let mut timeline = Vec::new();

        append_diarized_segment_result(
            &mut all_text,
            &mut timeline,
            &unit,
            WholeFileTranscription {
                text: " hello world ".to_string(),
                segments: Vec::new(),
                structured: Default::default(),
            },
        );

        assert_eq!(all_text, "hello world");
        assert_eq!(timeline.len(), 1);
        let seg = &timeline[0];
        assert_eq!(seg.index, 0);
        assert_eq!(seg.audio_start_ms, unit.source_start_ms);
        assert_eq!(seg.audio_end_ms, unit.source_end_ms);
        assert_eq!(seg.text, "hello world");
        assert_eq!(seg.speaker.as_deref(), Some("speaker_00"));
    }

    #[test]
    fn append_diarized_segment_result_appends_newline_between_chunks() {
        let unit = make_asr_unit(0, 10_000);
        let mut all_text = "first".to_string();
        let mut timeline = Vec::new();

        append_diarized_segment_result(
            &mut all_text,
            &mut timeline,
            &unit,
            WholeFileTranscription {
                text: "second".to_string(),
                segments: Vec::new(),
                structured: Default::default(),
            },
        );

        assert_eq!(all_text, "first\nsecond");
        assert_eq!(timeline.len(), 1);
    }

    #[test]
    fn append_diarized_segment_result_skips_blank_segment_text() {
        let unit = make_asr_unit(0, 10_000);
        let mut all_text = String::new();
        let mut timeline = Vec::new();

        append_diarized_segment_result(
            &mut all_text,
            &mut timeline,
            &unit,
            WholeFileTranscription {
                text: String::new(),
                segments: vec![(0, 500, "   ".to_string()), (500, 1_000, "ok".to_string())],
                structured: Default::default(),
            },
        );

        assert!(all_text.is_empty());
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0].audio_start_ms, 500);
        assert_eq!(timeline[0].audio_end_ms, 1_000);
        assert_eq!(timeline[0].text, "ok");
    }

    #[test]
    fn append_diarized_segment_result_clamps_segment_end_to_unit_range() {
        let unit = make_asr_unit(1_000, 2_000);
        let mut all_text = String::new();
        let mut timeline = Vec::new();

        append_diarized_segment_result(
            &mut all_text,
            &mut timeline,
            &unit,
            WholeFileTranscription {
                text: String::new(),
                segments: vec![(500, 2_500, "chunk".to_string())],
                structured: Default::default(),
            },
        );

        assert_eq!(timeline.len(), 1);
        let seg = &timeline[0];
        assert_eq!(seg.audio_start_ms, 1_500);
        assert_eq!(seg.audio_end_ms, 2_000);
    }

    #[test]
    fn append_diarized_segment_result_skips_non_positive_duration_segments() {
        let unit = make_asr_unit(1_000, 2_000);
        let mut all_text = String::new();
        let mut timeline = Vec::new();

        append_diarized_segment_result(
            &mut all_text,
            &mut timeline,
            &unit,
            WholeFileTranscription {
                text: String::new(),
                segments: vec![(1_000, 1_000, "zero".to_string())],
                structured: Default::default(),
            },
        );

        assert!(timeline.is_empty());
    }

    // --- timeline_segments_from_plain_chunks --------------------------------

    #[test]
    fn timeline_segments_from_plain_chunks_populates_indices_and_absolute_times() {
        let context = PartialArtifactContext {
            task_id: "task".to_string(),
            file_key: "key".to_string(),
            task_name: "Task".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "chinese".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            source_path: PathBuf::from("/tmp/audio.wav"),
            source_info: SourceAudioInfo {
                source_size: Some(123),
                source_modified_ms: Some(50),
                source_created_at_ms: Some(1_000),
                source_created_at_source: Some("test".to_string()),
                media_duration_ms: Some(10_000),
            },
            diarization_profile: None,
            speakers: Vec::new(),
            text_path: PathBuf::from("/tmp/text.txt"),
            metadata_path: PathBuf::from("/tmp/meta.json"),
            timeline_path: PathBuf::from("/tmp/timeline.json"),
            started_at_ms: 42,
        };

        let segments = timeline_segments_from_plain_chunks(
            &[(100, 200, "a".to_string()), (300, 450, "b".to_string())],
            &context,
        );

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].index, 0);
        assert_eq!(segments[1].index, 1);
        assert_eq!(segments[0].absolute_start_ms, Some(1_100));
        assert_eq!(segments[0].absolute_end_ms, Some(1_200));
        assert_eq!(segments[1].absolute_start_ms, Some(1_300));
        assert_eq!(segments[1].absolute_end_ms, Some(1_450));
        assert_eq!(segments[0].text, "a");
        assert_eq!(segments[1].text, "b");
    }

    #[test]
    fn timeline_segments_from_plain_chunks_handles_empty_input() {
        let context = PartialArtifactContext {
            task_id: "task".to_string(),
            file_key: "key".to_string(),
            task_name: "Task".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "chinese".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            source_path: PathBuf::from("/tmp/audio.wav"),
            source_info: SourceAudioInfo {
                source_size: None,
                source_modified_ms: None,
                source_created_at_ms: None,
                source_created_at_source: None,
                media_duration_ms: None,
            },
            diarization_profile: None,
            speakers: Vec::new(),
            text_path: PathBuf::from("/tmp/text.txt"),
            metadata_path: PathBuf::from("/tmp/meta.json"),
            timeline_path: PathBuf::from("/tmp/timeline.json"),
            started_at_ms: 0,
        };

        let segments = timeline_segments_from_plain_chunks(&[], &context);
        assert!(segments.is_empty());
    }

    // --- preserve_partial_artifact_fields ----------------------------------

    #[test]
    fn preserve_partial_artifact_fields_copies_artifacts_and_metadata() {
        let mut record = make_file_record(None, None);
        let existing = FileRecord {
            output_text_path: Some(PathBuf::from("/tmp/text.txt")),
            output_metadata_path: Some(PathBuf::from("/tmp/meta.json")),
            output_timeline_path: Some(PathBuf::from("/tmp/timeline.json")),
            text_chars: 42,
            media_duration_ms: Some(5_000),
            chunk_metrics: vec![AsrChunkMetric {
                chunk_index: 0,
                offset_secs: 0,
                duration_secs: 5,
                runner: "fork_per_chunk".to_string(),
                status: "ok".to_string(),
                elapsed_ms: 100,
                rtf: 0.1,
                text_chars: 3,
                text_sha1: "abc".to_string(),
                server_url: None,
                fallback_reason: Some("fallback".to_string()),
                error: None,
                recorded_at_ms: 1,
            }],
            fallback_reason: Some("fallback".to_string()),
            ..make_file_record(Some(1), Some(2))
        };

        preserve_partial_artifact_fields(&mut record, &existing);

        assert_eq!(record.output_text_path, existing.output_text_path);
        assert_eq!(record.output_metadata_path, existing.output_metadata_path);
        assert_eq!(record.output_timeline_path, existing.output_timeline_path);
        assert_eq!(record.text_chars, existing.text_chars);
        assert_eq!(record.media_duration_ms, existing.media_duration_ms);
        assert_eq!(record.chunk_metrics.len(), 1);
        assert_eq!(record.fallback_reason.as_deref(), Some("fallback"));
    }

    #[test]
    fn preserve_partial_artifact_fields_respects_existing_fallback_reason() {
        let mut record = make_file_record(None, None);
        record.fallback_reason = Some("keep".to_string());
        let existing = FileRecord {
            fallback_reason: Some("override".to_string()),
            text_chars: 5,
            media_duration_ms: Some(10),
            ..make_file_record(Some(1), Some(2))
        };

        preserve_partial_artifact_fields(&mut record, &existing);

        assert_eq!(record.fallback_reason.as_deref(), Some("keep"));
        assert_eq!(record.text_chars, 5);
        assert_eq!(record.media_duration_ms, Some(10));
    }

    // --- SpeechResourceLeaseGuard -------------------------------------------

    #[test]
    fn speech_resource_lease_guard_drop_with_none_lease_is_noop() {
        let guard = SpeechResourceLeaseGuard { lease: None };
        drop(guard);
    }

    #[test]
    fn speech_resource_lease_guard_drop_with_unknown_lease_is_noop() {
        let lease = bifrost_asr::resources::ResourceLease {
            lease_id: "unknown".to_string(),
            owner_module: "test".to_string(),
            owner_id: "id".to_string(),
            kind: bifrost_asr::resources::SpeechResourceKind::OfflineAsrServer,
            model_id: Some("model".to_string()),
            priority: 10,
            preemptible: true,
            acquired_at_ms: 0,
        };
        let guard = SpeechResourceLeaseGuard { lease: Some(lease) };
        drop(guard);
    }

    // --- transcribe_file_for_task_with_wav early error paths ----------------

    fn short_audio_source_info(duration_ms: u64) -> SourceAudioInfo {
        SourceAudioInfo {
            source_size: None,
            source_modified_ms: None,
            source_created_at_ms: None,
            source_created_at_source: None,
            media_duration_ms: Some(duration_ms),
        }
    }

    fn minimal_task(id: &str) -> AsrDirectoryTask {
        AsrDirectoryTask {
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
            id: id.to_string(),
            name: id.to_string(),
            audio_dir: PathBuf::from("/tmp"),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        }
    }

    #[tokio::test]
    async fn transcribe_file_for_task_with_wav_respects_pause_check_for_short_audio() {
        let task = minimal_task("pause-short");
        let source_info = short_audio_source_info(5_000);
        let pause = || true;
        let hooks = TaskTranscribeHooks {
            on_chunk_progress: None,
            on_chunk_metric: None,
            pause_check: Some(&pause),
            force_pause_task_id: Some(&task.id),
            memory_limit_hints: &[],
            server_url: None,
            startup_fallback_reason: None,
            server_state: None,
            managed_server_restart: None,
            partial_artifacts: None,
            moss_runtime: None,
        };

        let result = transcribe_file_for_task_with_wav(
            &task,
            Path::new("/asr/bin"),
            Path::new("/asr/model"),
            Path::new("/audio/source.wav"),
            Path::new("/audio/source.wav"),
            &source_info,
            hooks,
        )
        .await;

        assert!(matches!(result, Err(e) if e == ASR_TASK_PAUSED_MESSAGE));
    }

    #[tokio::test]
    async fn transcribe_file_for_task_with_wav_propagates_diarization_missing_assets() {
        let mut task = minimal_task("diarization-missing");
        task.diarization.enabled = true;
        task.diarization.profile = "missing-test-profile".to_string();

        let source_info = short_audio_source_info(5_000);
        let hooks = TaskTranscribeHooks {
            on_chunk_progress: None,
            on_chunk_metric: None,
            pause_check: None,
            force_pause_task_id: None,
            memory_limit_hints: &[],
            server_url: None,
            startup_fallback_reason: None,
            server_state: None,
            managed_server_restart: None,
            partial_artifacts: None,
            moss_runtime: None,
        };

        let result = transcribe_file_for_task_with_wav(
            &task,
            Path::new("/asr/bin"),
            Path::new("/asr/model"),
            Path::new("/audio/source.wav"),
            Path::new("/audio/source.wav"),
            &source_info,
            hooks,
        )
        .await;

        assert!(matches!(result, Err(e) if e.contains("diarization_missing_assets")));
    }

    #[tokio::test]
    async fn transcribe_diarized_segments_for_task_reports_missing_assets() {
        let mut task = minimal_task("diarized-missing");
        task.diarization.enabled = true;
        task.diarization.profile = "missing-test-profile".to_string();

        let hooks = TaskTranscribeHooks {
            on_chunk_progress: None,
            on_chunk_metric: None,
            pause_check: None,
            force_pause_task_id: None,
            memory_limit_hints: &[],
            server_url: None,
            startup_fallback_reason: None,
            server_state: None,
            managed_server_restart: None,
            partial_artifacts: None,
            moss_runtime: None,
        };

        let result = transcribe_diarized_segments_for_task(
            &task,
            Path::new("/asr/bin"),
            Path::new("/asr/model"),
            Path::new("/audio/source.wav"),
            Path::new("/tmp"),
            hooks,
        )
        .await;

        assert!(matches!(result, Err(e) if e.contains("diarization_missing_assets")));
    }

    // ---------------------------------------------------------------------
    // Additional small tests to exercise edge cases of helper functions.
    // These focus on pure logic and avoid touching external binaries.
    // ---------------------------------------------------------------------

    #[test]
    fn sort_pending_paths_prefers_present_created_time_over_missing() {
        let mut store = FileStore {
            version: 1,
            files: BTreeMap::new(),
        };
        let with_created = PathBuf::from("/audio/with_created.wav");
        let without_created = PathBuf::from("/audio/without_created.wav");

        let key_with = source_key(&with_created);
        let key_without = source_key(&without_created);

        store
            .files
            .insert(key_with.clone(), make_file_record(Some(5), Some(10)));
        store
            .files
            .insert(key_without.clone(), make_file_record(None, Some(1)));

        let mut pending = vec![without_created.clone(), with_created.clone()];
        sort_pending_paths_by_source_time(&store, &mut pending);

        assert_eq!(pending[0], with_created);
        assert_eq!(pending[1], without_created);
    }

    #[test]
    fn sort_pending_paths_uses_modified_time_when_created_equal() {
        let mut store = FileStore {
            version: 1,
            files: BTreeMap::new(),
        };
        let early = PathBuf::from("/audio/early.wav");
        let late = PathBuf::from("/audio/late.wav");

        let key_early = source_key(&early);
        let key_late = source_key(&late);

        store
            .files
            .insert(key_early.clone(), make_file_record(Some(10), Some(20)));
        store
            .files
            .insert(key_late.clone(), make_file_record(Some(10), Some(30)));

        let mut pending = vec![late.clone(), early.clone()];
        sort_pending_paths_by_source_time(&store, &mut pending);

        assert_eq!(pending, vec![early, late]);
    }

    #[test]
    fn sort_pending_paths_handles_mixed_existing_and_missing_records() {
        let mut store = FileStore {
            version: 1,
            files: BTreeMap::new(),
        };
        let known = PathBuf::from("/audio/known.wav");
        let unknown = PathBuf::from("/audio/unknown.wav");

        let key_known = source_key(&known);
        store
            .files
            .insert(key_known.clone(), make_file_record(Some(1), Some(1)));

        let mut pending = vec![unknown.clone(), known.clone()];
        sort_pending_paths_by_source_time(&store, &mut pending);

        assert_eq!(pending[0], known);
        assert_eq!(pending[1], unknown);
    }

    #[test]
    fn sort_pending_paths_breaks_full_ties_by_path_lexicographically() {
        let mut store = FileStore {
            version: 1,
            files: BTreeMap::new(),
        };
        let a = PathBuf::from("/audio/a.wav");
        let b = PathBuf::from("/audio/b.wav");

        let key_a = source_key(&a);
        let key_b = source_key(&b);
        let template = make_file_record(Some(42), Some(42));
        store.files.insert(key_a.clone(), template.clone());
        store.files.insert(key_b.clone(), template);

        let mut pending = vec![b.clone(), a.clone()];
        sort_pending_paths_by_source_time(&store, &mut pending);

        assert_eq!(pending, vec![a, b]);
    }

    #[test]
    fn append_diarized_segment_result_appends_after_existing_timeline_segments() {
        let unit = make_asr_unit(0, 1_000);
        let mut all_text = String::new();
        let mut timeline = vec![TimelineSegment {
            index: 0,
            audio_start_ms: 0,
            audio_end_ms: 100,
            absolute_start_ms: None,
            absolute_end_ms: None,
            speaker: Some("speaker_00".to_string()),
            speaker_display_name: Some("用户A".to_string()),
            overlap: false,
            text: "old".to_string(),
        }];

        append_diarized_segment_result(
            &mut all_text,
            &mut timeline,
            &unit,
            WholeFileTranscription {
                text: String::new(),
                segments: vec![(0, 500, "new".to_string())],
                structured: Default::default(),
            },
        );

        assert_eq!(timeline.len(), 2);
        assert_eq!(timeline[0].index, 0);
        assert_eq!(timeline[1].index, 1);
        assert_eq!(timeline[1].text, "new");
    }

    #[test]
    fn append_diarized_segment_result_keeps_segment_text_whitespace() {
        let unit = make_asr_unit(0, 1_000);
        let mut all_text = String::new();
        let mut timeline = Vec::new();

        append_diarized_segment_result(
            &mut all_text,
            &mut timeline,
            &unit,
            WholeFileTranscription {
                text: String::new(),
                segments: vec![(0, 500, "  padded  ".to_string())],
                structured: Default::default(),
            },
        );

        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0].text, "  padded  ");
    }

    #[test]
    fn append_diarized_segment_result_handles_multiple_segments_in_one_call() {
        let unit = make_asr_unit(0, 2_000);
        let mut all_text = String::new();
        let mut timeline = Vec::new();

        append_diarized_segment_result(
            &mut all_text,
            &mut timeline,
            &unit,
            WholeFileTranscription {
                text: String::new(),
                segments: vec![
                    (0, 500, "one".to_string()),
                    (500, 1_000, "two".to_string()),
                ],
                structured: Default::default(),
            },
        );

        assert_eq!(timeline.len(), 2);
        assert_eq!(timeline[0].text, "one");
        assert_eq!(timeline[1].text, "two");
    }

    #[test]
    fn append_diarized_segment_result_preserves_speaker_metadata_for_all_segments() {
        let unit = make_asr_unit(100, 900);
        let mut all_text = String::new();
        let mut timeline = Vec::new();

        append_diarized_segment_result(
            &mut all_text,
            &mut timeline,
            &unit,
            WholeFileTranscription {
                text: String::new(),
                segments: vec![(0, 200, "a".to_string()), (200, 400, "b".to_string())],
                structured: Default::default(),
            },
        );

        assert_eq!(timeline.len(), 2);
        for seg in &timeline {
            assert_eq!(seg.speaker.as_deref(), Some("speaker_00"));
            assert_eq!(seg.speaker_display_name.as_deref(), Some("用户A"));
        }
    }

    #[test]
    fn append_diarized_segment_result_indices_continue_across_multiple_calls() {
        let unit = make_asr_unit(0, 1_000);
        let mut all_text = String::new();
        let mut timeline = Vec::new();

        append_diarized_segment_result(
            &mut all_text,
            &mut timeline,
            &unit,
            WholeFileTranscription {
                text: String::new(),
                segments: vec![(0, 500, "one".to_string())],
                structured: Default::default(),
            },
        );
        append_diarized_segment_result(
            &mut all_text,
            &mut timeline,
            &unit,
            WholeFileTranscription {
                text: String::new(),
                segments: vec![(500, 1_000, "two".to_string())],
                structured: Default::default(),
            },
        );

        assert_eq!(timeline.len(), 2);
        assert_eq!(timeline[0].index, 0);
        assert_eq!(timeline[1].index, 1);
    }

    #[test]
    fn timeline_segments_from_plain_chunks_sets_absolute_times_only_when_created_at_present() {
        let mut context = PartialArtifactContext {
            task_id: "task".to_string(),
            file_key: "key".to_string(),
            task_name: "Task".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "chinese".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            source_path: PathBuf::from("/tmp/audio.wav"),
            source_info: SourceAudioInfo {
                source_size: None,
                source_modified_ms: None,
                source_created_at_ms: None,
                source_created_at_source: None,
                media_duration_ms: None,
            },
            diarization_profile: None,
            speakers: Vec::new(),
            text_path: PathBuf::from("/tmp/text.txt"),
            metadata_path: PathBuf::from("/tmp/meta.json"),
            timeline_path: PathBuf::from("/tmp/timeline.json"),
            started_at_ms: 0,
        };

        let segments_without_created = timeline_segments_from_plain_chunks(
            &[(0, 1_000, "a".to_string())],
            &context,
        );
        assert_eq!(segments_without_created[0].absolute_start_ms, None);
        assert_eq!(segments_without_created[0].absolute_end_ms, None);

        context.source_info.source_created_at_ms = Some(10_000);
        let segments_with_created = timeline_segments_from_plain_chunks(
            &[(0, 1_000, "a".to_string())],
            &context,
        );
        assert_eq!(segments_with_created[0].absolute_start_ms, Some(10_000));
        assert_eq!(segments_with_created[0].absolute_end_ms, Some(11_000));
    }

    #[test]
    fn preserve_partial_artifact_fields_does_not_copy_when_existing_has_no_artifacts() {
        let mut record = make_file_record(None, None);
        let existing = make_file_record(Some(1), Some(2));

        preserve_partial_artifact_fields(&mut record, &existing);

        assert!(record.output_text_path.is_none());
        assert!(record.output_metadata_path.is_none());
        assert!(record.output_timeline_path.is_none());
        assert!(record.chunk_metrics.is_empty());
    }

    #[test]
    fn pending_time_helpers_accept_large_values() {
        let record = make_file_record(Some(u64::MAX - 1), Some(u64::MAX - 2));
        assert_eq!(pending_source_time_ms(Some(&record)), u64::MAX - 1);
        assert_eq!(pending_modified_time_ms(Some(&record)), u64::MAX - 2);
    }

    #[test]
    fn speech_resource_lease_guard_is_send_and_sync_safe_to_move() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SpeechResourceLeaseGuard>();
    }

    #[test]
    fn minimal_task_uses_expected_defaults() {
        let task = minimal_task("defaults");
        assert!(task.enabled);
        assert!(!task.paused);
        assert_eq!(task.runtime_strategy, AsrRuntimeStrategy::ForkPerChunk);
    }

    #[test]
    fn make_file_record_starts_with_pending_status_and_zero_text_chars() {
        let record = make_file_record(Some(123), Some(456));
        assert!(matches!(record.status, FileStatus::Pending));
        assert_eq!(record.text_chars, 0);
        assert!(record.output_text_path.is_none());
        assert_eq!(record.source_created_at_ms, Some(123));
        assert_eq!(record.source_modified_ms, Some(456));
    }

    #[test]
    fn make_file_record_respects_created_and_modified_inputs() {
        let record = make_file_record(Some(1), Some(2));
        assert_eq!(record.source_created_at_ms, Some(1));
        assert_eq!(record.source_modified_ms, Some(2));

        let record_none = make_file_record(None, None);
        assert_eq!(record_none.source_created_at_ms, None);
        assert_eq!(record_none.source_modified_ms, None);
    }

    #[test]
    fn make_asr_unit_initializes_speaker_and_unit_kind() {
        let unit = make_asr_unit(100, 200);
        assert_eq!(unit.source_start_ms, 100);
        assert_eq!(unit.source_end_ms, 200);
        assert_eq!(unit.unit_kind, AsrUnitKind::Segment);
        assert_eq!(unit.speaker.as_deref(), Some("speaker_00"));
        assert_eq!(unit.speaker_display_name.as_deref(), Some("用户A"));
    }

    #[test]
    fn short_audio_source_info_sets_only_media_duration() {
        let info = short_audio_source_info(1_234);
        assert_eq!(info.media_duration_ms, Some(1_234));
        assert!(info.source_size.is_none());
        assert!(info.source_modified_ms.is_none());
        assert!(info.source_created_at_ms.is_none());
    }

    #[test]
    fn short_audio_source_info_zero_duration_is_allowed() {
        let info = short_audio_source_info(0);
        assert_eq!(info.media_duration_ms, Some(0));
    }

    #[test]
    fn minimal_task_sets_id_and_name_consistently() {
        let task = minimal_task("example-task");
        assert_eq!(task.id, "example-task");
        assert_eq!(task.name, "example-task");
        assert_eq!(task.audio_dir, PathBuf::from("/tmp"));
    }

    #[test]
    fn minimal_task_has_empty_external_devices_and_default_import_policy() {
        let task = minimal_task("policy");
        assert!(task.external_devices.is_empty());
        assert!(!task.import_policy.enabled);
        assert!(task.import_policy.content_hash_dedupe_enabled);
        assert!(task.import_policy.auto_run_after_import);
    }
}

#[cfg(test)]
mod coverage_boost_v2 {
    use super::*;
    use crate::asr_runtime::AsrServiceState;
    use crate::test_env::BifrostDataDirGuard;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn simple_file_record(created: Option<u64>, modified: Option<u64>) -> FileRecord {
        FileRecord {
            task_id: "task".to_string(),
            source_path: PathBuf::from("/tmp/audio.wav"),
            source_size: None,
            source_modified_ms: modified,
            source_created_at_ms: created,
            source_created_at_source: None,
            content_hash: None,
            content_hash_algorithm: None,
            duplicate_of_source_key: None,
            transcript_alias: None,
            media_duration_ms: None,
            status: FileStatus::Pending,
            output_text_path: None,
            output_metadata_path: None,
            output_timeline_path: None,
            text_chars: 0,
            error: None,
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            chunk_metrics: Vec::new(),
            fallback_reason: None,
            started_at_ms: None,
            finished_at_ms: None,
            progress_current: None,
            progress_total: None,
            failed_chunks: Vec::new(),
            memory_limit_hints: Vec::new(),
        }
    }

    fn simple_state(pid: Option<u32>, port: u16) -> AsrServiceState {
        AsrServiceState {
            host: "127.0.0.1".to_string(),
            port,
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "chinese".to_string(),
            home: crate::handlers::asr::default_home(),
            pid,
            managed_by: "test".to_string(),
            owner_module: Some("test".to_string()),
            owner_id: None,
            started_at_ms: 1,
        }
    }

    fn short_audio_source_info(duration_ms: u64) -> SourceAudioInfo {
        SourceAudioInfo {
            source_size: None,
            source_modified_ms: None,
            source_created_at_ms: None,
            source_created_at_source: None,
            media_duration_ms: Some(duration_ms),
        }
    }

    fn minimal_task(id: &str) -> AsrDirectoryTask {
        AsrDirectoryTask {
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
            id: id.to_string(),
            name: id.to_string(),
            audio_dir: PathBuf::from("/tmp"),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: default_task_schedule(),
            language: "chinese".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        }
    }

    fn make_file_record(created: Option<u64>, modified: Option<u64>) -> FileRecord {
        simple_file_record(created, modified)
    }

    fn make_asr_unit(start_ms: u64, end_ms: u64) -> AsrAudioUnit {
        AsrAudioUnit {
            unit_id: "unit_00".to_string(),
            speaker: Some("speaker_00".to_string()),
            speaker_display_name: Some("用户A".to_string()),
            mapped_profile_id: None,
            confidence: Some(0.9),
            source_start_ms: start_ms,
            source_end_ms: end_ms,
            source_segment_ids: vec!["seg0".to_string()],
            overlap: false,
            unit_kind: bifrost_asr::planner::AsrUnitKind::Segment,
        }
    }

    #[test]
    fn task_allows_external_device_event_import_requires_enabled_and_devices() {
        let mut task = AsrDirectoryTask {
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
            id: "t".to_string(),
            name: "t".to_string(),
            audio_dir: PathBuf::from("/tmp"),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: default_task_schedule(),
            language: "zh".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 0,
            updated_at_ms: 0,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy::default(),
        };
        assert!(!task_allows_external_device_event_import(&task));
        task.import_policy.enabled = true;
        task.external_devices.push(AsrExternalDeviceBinding::default());
        assert!(task_allows_external_device_event_import(&task));
    }

    #[test]
    fn task_allows_external_device_event_import_false_when_no_devices() {
        let task = AsrDirectoryTask {
            transcription_mode: AsrTranscriptionMode::Standard,
            transcription_prompt: String::new(),
            id: "t2".to_string(),
            name: "t2".to_string(),
            audio_dir: PathBuf::from("/tmp"),
            recursive: true,
            enabled: true,
            paused: false,
            paused_at_ms: None,
            schedule: default_task_schedule(),
            language: "zh".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            max_concurrent_files: default_max_concurrent_files(),
            diarization: AsrDiarizationConfig::default(),
            created_at_ms: 0,
            updated_at_ms: 0,
            last_run_at_ms: None,
            next_run_at_ms: None,
            last_error: None,
            daily_agent: AsrDailyAgentConfig::default(),
            external_devices: Vec::new(),
            import_policy: AsrExternalImportPolicy { enabled: true, ..AsrExternalImportPolicy::default() },
        };
        assert!(!task_allows_external_device_event_import(&task));
    }

    #[test]
    fn pending_time_helpers_handle_zero_and_max_values() {
        let record = simple_file_record(Some(0), Some(u64::MAX));
        assert_eq!(pending_source_time_ms(Some(&record)), 0);
        assert_eq!(pending_modified_time_ms(Some(&record)), u64::MAX);

        assert_eq!(pending_source_time_ms(None), u64::MAX);
        assert_eq!(pending_modified_time_ms(None), u64::MAX);
    }

    #[test]
    fn service_state_same_process_false_for_mismatched_pid_or_port() {
        let base = simple_state(Some(1000), 60_000);
        let other_pid = simple_state(Some(1001), 60_000);
        let other_port = simple_state(Some(1000), 60_001);

        assert!(service_state_same_process(Some(&base), Some(&base)));
        assert!(!service_state_same_process(Some(&base), Some(&other_pid)));
        assert!(!service_state_same_process(Some(&base), Some(&other_port)));
        assert!(!service_state_same_process(Some(&base), None));
        assert!(!service_state_same_process(None, Some(&base)));
    }

    #[test]
    fn sort_pending_paths_uses_source_times_then_path() {
        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        let a = PathBuf::from("/audio/a.wav");
        let b = PathBuf::from("/audio/b.wav");
        let c = PathBuf::from("/audio/c.wav");

        store
            .files
            .insert(source_key(&a), simple_file_record(Some(5), Some(30)));
        store
            .files
            .insert(source_key(&b), simple_file_record(Some(5), Some(20)));
        store
            .files
            .insert(source_key(&c), simple_file_record(Some(1), Some(40)));

        let mut pending = vec![b.clone(), c.clone(), a.clone()];
        sort_pending_paths_by_source_time(&store, &mut pending);
        assert_eq!(pending, vec![c, b, a]);
    }

    #[test]
    fn timeline_segments_from_plain_chunks_produces_sequential_indices() {
        let context = PartialArtifactContext {
            task_id: "t".to_string(),
            file_key: "k".to_string(),
            task_name: "Task".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "zh".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            source_path: PathBuf::from("/tmp/audio.wav"),
            source_info: SourceAudioInfo {
                source_size: None,
                source_modified_ms: None,
                source_created_at_ms: Some(1_000),
                source_created_at_source: Some("test".to_string()),
                media_duration_ms: Some(5_000),
            },
            diarization_profile: None,
            speakers: Vec::new(),
            text_path: PathBuf::from("/tmp/text.txt"),
            metadata_path: PathBuf::from("/tmp/meta.json"),
            timeline_path: PathBuf::from("/tmp/timeline.json"),
            started_at_ms: 0,
        };

        let segments = timeline_segments_from_plain_chunks(
            &[(0, 500, "a".to_string()), (500, 1000, "b".to_string())],
            &context,
        );
        assert_eq!(segments[0].index, 0);
        assert_eq!(segments[1].index, 1);
        assert_eq!(segments[0].absolute_start_ms, Some(1_000));
        assert_eq!(segments[1].absolute_start_ms, Some(1_500));
    }

    #[test]
    fn preserve_partial_artifact_fields_leaves_missing_fields_unchanged() {
        let mut record = simple_file_record(None, None);
        let existing = simple_file_record(Some(1), Some(2));

        preserve_partial_artifact_fields(&mut record, &existing);

        assert!(record.output_text_path.is_none());
        assert!(record.output_metadata_path.is_none());
        assert!(record.output_timeline_path.is_none());
        assert_eq!(record.text_chars, 0);
        assert!(record.media_duration_ms.is_none());
    }

    #[test]
    fn preserve_partial_artifact_fields_prefers_existing_artifacts() {
        let mut record = simple_file_record(None, None);
        let existing = FileRecord {
            output_text_path: Some(PathBuf::from("/tmp/text.txt")),
            output_metadata_path: Some(PathBuf::from("/tmp/meta.json")),
            output_timeline_path: Some(PathBuf::from("/tmp/timeline.json")),
            text_chars: 10,
            media_duration_ms: Some(1_000),
            chunk_metrics: vec![AsrChunkMetric {
                chunk_index: 0,
                offset_secs: 0,
                duration_secs: 1,
                runner: "r".to_string(),
                status: "ok".to_string(),
                elapsed_ms: 10,
                rtf: 0.1,
                text_chars: 3,
                text_sha1: "abc".to_string(),
                server_url: None,
                fallback_reason: Some("fallback".to_string()),
                error: None,
                recorded_at_ms: 1,
            }],
            fallback_reason: Some("fallback".to_string()),
            ..simple_file_record(Some(1), Some(2))
        };

        preserve_partial_artifact_fields(&mut record, &existing);
        assert_eq!(record.output_text_path, existing.output_text_path);
        assert_eq!(record.text_chars, 10);
        assert_eq!(record.media_duration_ms, Some(1_000));
        assert_eq!(record.fallback_reason.as_deref(), Some("fallback"));
        assert_eq!(record.chunk_metrics.len(), 1);
    }

    #[test]
    fn speech_resource_lease_guard_can_wrap_none_and_some() {
        let guard_none = SpeechResourceLeaseGuard { lease: None };
        drop(guard_none);

        let lease = bifrost_asr::resources::ResourceLease {
            lease_id: "id".to_string(),
            owner_module: "test".to_string(),
            owner_id: "owner".to_string(),
            kind: bifrost_asr::resources::SpeechResourceKind::OfflineAsrServer,
            model_id: Some("model".to_string()),
            priority: 1,
            preemptible: true,
            acquired_at_ms: 0,
        };
        let guard_some = SpeechResourceLeaseGuard { lease: Some(lease) };
        drop(guard_some);
    }

    #[test]
    fn minimal_task_helper_sets_basic_defaults() {
        let task = minimal_task("v2-task");
        assert_eq!(task.id, "v2-task");
        assert_eq!(task.name, "v2-task");
        assert!(task.enabled);
        assert!(!task.paused);
        assert_eq!(task.runtime_strategy, AsrRuntimeStrategy::ForkPerChunk);
    }

    #[test]
    fn short_audio_source_info_preserves_duration() {
        let info = short_audio_source_info(1234);
        assert_eq!(info.media_duration_ms, Some(1234));
        assert!(info.source_size.is_none());
    }

    #[test]
    fn short_audio_source_info_accepts_zero_duration() {
        let info = short_audio_source_info(0);
        assert_eq!(info.media_duration_ms, Some(0));
    }

    #[test]
    fn make_file_record_uses_created_and_modified_inputs() {
        let record = make_file_record(Some(10), Some(20));
        assert_eq!(record.source_created_at_ms, Some(10));
        assert_eq!(record.source_modified_ms, Some(20));
        assert!(matches!(record.status, FileStatus::Pending));
    }

    #[test]
    fn make_asr_unit_carries_speaker_metadata() {
        let unit = make_asr_unit(100, 200);
        assert_eq!(unit.source_start_ms, 100);
        assert_eq!(unit.source_end_ms, 200);
        assert_eq!(unit.unit_kind, bifrost_asr::planner::AsrUnitKind::Segment);
        assert_eq!(unit.speaker.as_deref(), Some("speaker_00"));
    }

    #[test]
    fn pending_time_helpers_support_large_values() {
        let record = simple_file_record(Some(u64::MAX - 1), Some(u64::MAX - 2));
        assert_eq!(pending_source_time_ms(Some(&record)), u64::MAX - 1);
        assert_eq!(pending_modified_time_ms(Some(&record)), u64::MAX - 2);
    }

    #[test]
    fn service_state_same_process_false_when_either_side_missing() {
        let state = simple_state(Some(1), 10_000);
        assert!(!service_state_same_process(Some(&state), None));
        assert!(!service_state_same_process(None, Some(&state)));
        assert!(!service_state_same_process(None, None));
    }

    #[test]
    fn timeline_segments_from_plain_chunks_empty_input_returns_empty_vec() {
        let context = PartialArtifactContext {
            task_id: "t".to_string(),
            file_key: "k".to_string(),
            task_name: "Task".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "zh".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            source_path: PathBuf::from("/tmp/audio.wav"),
            source_info: SourceAudioInfo {
                source_size: None,
                source_modified_ms: None,
                source_created_at_ms: None,
                source_created_at_source: None,
                media_duration_ms: None,
            },
            diarization_profile: None,
            speakers: Vec::new(),
            text_path: PathBuf::from("/tmp/text.txt"),
            metadata_path: PathBuf::from("/tmp/meta.json"),
            timeline_path: PathBuf::from("/tmp/timeline.json"),
            started_at_ms: 0,
        };
        let segments = timeline_segments_from_plain_chunks(&[], &context);
        assert!(segments.is_empty());
    }

    #[test]
    fn preserve_partial_artifact_fields_keeps_existing_fallback_reason() {
        let mut record = simple_file_record(None, None);
        record.fallback_reason = Some("keep".to_string());
        let existing = simple_file_record(Some(1), Some(2));

        preserve_partial_artifact_fields(&mut record, &existing);
        assert_eq!(record.fallback_reason.as_deref(), Some("keep"));
    }

    #[test]
    fn speech_resource_lease_guard_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SpeechResourceLeaseGuard>();
    }

    #[test]
    fn persist_partial_transcription_artifacts_updates_pending_record_and_artifacts() {
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());

        let task_id = "persist_task".to_string();
        let file_key = "audio.wav".to_string();

        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        store
            .files
            .insert(file_key.clone(), simple_file_record(Some(1_000), Some(2_000)));
        save_file_store(&task_id, &store).expect("save file store");

        let context = PartialArtifactContext {
            task_id: task_id.clone(),
            file_key: file_key.clone(),
            task_name: "Task".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "zh".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            source_path: dir.path().join("audio.wav"),
            source_info: SourceAudioInfo {
                source_size: Some(123),
                source_modified_ms: Some(2_000),
                source_created_at_ms: Some(1_000),
                source_created_at_source: Some("test".to_string()),
                media_duration_ms: Some(5_000),
            },
            diarization_profile: Some("default".to_string()),
            speakers: Vec::new(),
            text_path: dir.path().join("text.txt"),
            metadata_path: dir.path().join("meta.json"),
            timeline_path: dir.path().join("timeline.json"),
            started_at_ms: 42,
        };

        let segments = vec![TimelineSegment {
            index: 0,
            audio_start_ms: 0,
            audio_end_ms: 1_000,
            absolute_start_ms: None,
            absolute_end_ms: None,
            speaker: None,
            speaker_display_name: None,
            overlap: false,
            text: "hello".to_string(),
        }];
        let metrics = vec![AsrChunkMetric {
            chunk_index: 0,
            offset_secs: 0,
            duration_secs: 1,
            runner: "test".to_string(),
            status: "ok".to_string(),
            elapsed_ms: 10,
            rtf: 0.1,
            text_chars: 5,
            text_sha1: "abc".to_string(),
            server_url: None,
            fallback_reason: None,
            error: None,
            recorded_at_ms: 1,
        }];
        let progress = DiarizedSegmentProgress {
            text: "hello".to_string(),
            timeline_segments: segments,
            chunk_metrics: metrics.clone(),
            fallback_reason: Some("fallback".to_string()),
        };

        persist_partial_transcription_artifacts(&context, progress).expect("persist");

        let updated_store = load_file_store(&task_id);
        let updated = updated_store.files.get(&file_key).expect("record");
        assert_eq!(updated.status, FileStatus::Processing);
        assert_eq!(updated.output_text_path.as_ref(), Some(&context.text_path));
        assert_eq!(updated.output_metadata_path.as_ref(), Some(&context.metadata_path));
        assert_eq!(updated.output_timeline_path.as_ref(), Some(&context.timeline_path));
        assert_eq!(updated.media_duration_ms, context.source_info.media_duration_ms);
        // `text_chars` is the char count of the *rendered* timeline text
        // (segments with timestamp/speaker formatting), not the raw segment
        // text. Tie the assertion to the artifact actually written to disk so
        // it stays correct regardless of timezone-dependent timestamp width.
        let rendered_text = std::fs::read_to_string(&context.text_path).expect("read text artifact");
        assert_eq!(updated.text_chars, rendered_text.chars().count());
        assert_eq!(updated.chunk_metrics.len(), metrics.len());
        assert_eq!(updated.fallback_reason.as_deref(), Some("fallback"));
        assert_eq!(updated.started_at_ms, Some(context.started_at_ms));

        assert!(context.text_path.is_file());
        assert!(context.metadata_path.is_file());
        assert!(context.timeline_path.is_file());
    }

    #[test]
    fn persist_partial_transcription_artifacts_preserves_success_status() {
        let dir = tempdir().expect("tempdir");
        let _guard = BifrostDataDirGuard::set(dir.path());

        let task_id = "persist_success".to_string();
        let file_key = "audio.wav".to_string();

        let mut record = simple_file_record(Some(1), Some(2));
        record.status = FileStatus::Success;

        let mut store = FileStore {
            version: TASK_STORE_VERSION,
            files: BTreeMap::new(),
        };
        store.files.insert(file_key.clone(), record);
        save_file_store(&task_id, &store).expect("save file store");

        let context = PartialArtifactContext {
            task_id: task_id.clone(),
            file_key: file_key.clone(),
            task_name: "Task".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "zh".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            source_path: dir.path().join("audio.wav"),
            source_info: SourceAudioInfo {
                source_size: None,
                source_modified_ms: None,
                source_created_at_ms: None,
                source_created_at_source: None,
                media_duration_ms: None,
            },
            diarization_profile: None,
            speakers: Vec::new(),
            text_path: dir.path().join("text.txt"),
            metadata_path: dir.path().join("meta.json"),
            timeline_path: dir.path().join("timeline.json"),
            started_at_ms: 0,
        };

        let progress = DiarizedSegmentProgress {
            text: String::new(),
            timeline_segments: Vec::new(),
            chunk_metrics: Vec::new(),
            fallback_reason: None,
        };

        persist_partial_transcription_artifacts(&context, progress).expect("persist");

        let updated_store = load_file_store(&task_id);
        let updated = updated_store.files.get(&file_key).expect("record");
        assert_eq!(updated.status, FileStatus::Success);
    }

    #[test]
    fn timeline_segments_from_plain_chunks_uses_context_created_at_for_absolute_times_v2() {
        let context = PartialArtifactContext {
            task_id: "t".to_string(),
            file_key: "k".to_string(),
            task_name: "Task".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "zh".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            source_path: PathBuf::from("/tmp/audio.wav"),
            source_info: SourceAudioInfo {
                source_size: None,
                source_modified_ms: None,
                source_created_at_ms: Some(10_000),
                source_created_at_source: None,
                media_duration_ms: None,
            },
            diarization_profile: None,
            speakers: Vec::new(),
            text_path: PathBuf::from("/tmp/text.txt"),
            metadata_path: PathBuf::from("/tmp/meta.json"),
            timeline_path: PathBuf::from("/tmp/timeline.json"),
            started_at_ms: 0,
        };

        let segments = timeline_segments_from_plain_chunks(
            &[(250, 750, "hi".to_string())],
            &context,
        );

        assert_eq!(segments[0].absolute_start_ms, Some(10_250));
        assert_eq!(segments[0].absolute_end_ms, Some(10_750));
    }

    #[test]
    fn timeline_segments_from_plain_chunks_leaves_absolute_times_none_when_created_missing_v2() {
        let context = PartialArtifactContext {
            task_id: "t".to_string(),
            file_key: "k".to_string(),
            task_name: "Task".to_string(),
            model: "Qwen3-ASR-0.6B".to_string(),
            language: "zh".to_string(),
            runtime_strategy: AsrRuntimeStrategy::ForkPerChunk,
            source_path: PathBuf::from("/tmp/audio.wav"),
            source_info: SourceAudioInfo {
                source_size: None,
                source_modified_ms: None,
                source_created_at_ms: None,
                source_created_at_source: None,
                media_duration_ms: None,
            },
            diarization_profile: None,
            speakers: Vec::new(),
            text_path: PathBuf::from("/tmp/text.txt"),
            metadata_path: PathBuf::from("/tmp/meta.json"),
            timeline_path: PathBuf::from("/tmp/timeline.json"),
            started_at_ms: 0,
        };

        let segments = timeline_segments_from_plain_chunks(
            &[(0, 500, "hi".to_string())],
            &context,
        );

        assert!(segments[0].absolute_start_ms.is_none());
        assert!(segments[0].absolute_end_ms.is_none());
    }

    #[test]
    fn pending_source_time_ms_prefers_created_timestamp_over_default() {
        let record = simple_file_record(Some(123), None);
        assert_eq!(pending_source_time_ms(Some(&record)), 123);
    }

    #[test]
    fn pending_modified_time_ms_prefers_modified_timestamp_over_default() {
        let record = simple_file_record(None, Some(456));
        assert_eq!(pending_modified_time_ms(Some(&record)), 456);
    }
}
