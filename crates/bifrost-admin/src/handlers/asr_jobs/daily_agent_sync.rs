// ─── Daily Agent Report Sync ─────────────────────────────────────────────────

fn expand_daily_agent_sync_dir(path: &str) -> PathBuf {
    let trimmed = path.trim();
    if trimmed == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(trimmed));
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(trimmed)
}

fn configured_daily_agent_report_sync_dir(task: &AsrDirectoryTask) -> Option<PathBuf> {
    task.daily_agent
        .report_sync_dir
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(expand_daily_agent_sync_dir)
}

fn sync_daily_agent_report_files(
    task: &AsrDirectoryTask,
    report_paths: &[String],
) -> Result<AsrDailyAgentReportSyncResult, String> {
    let target_dir = configured_daily_agent_report_sync_dir(task)
        .ok_or_else(|| "Daily Agent report sync directory is not configured".to_string())?;

    if target_dir.exists() && !target_dir.is_dir() {
        return Err(format!(
            "Daily Agent report sync target is not a directory: {}",
            target_dir.display()
        ));
    }
    std::fs::create_dir_all(&target_dir)
        .map_err(|error| format!("create report sync directory {}: {error}", target_dir.display()))?;

    let mut result = AsrDailyAgentReportSyncResult {
        target_dir: target_dir.to_string_lossy().to_string(),
        total_files: report_paths.len(),
        synced_at_ms: now_ms(),
        ..Default::default()
    };

    for report_path in report_paths {
        let source = PathBuf::from(report_path);
        let Some(file_name) = source.file_name() else {
            result.failed_files += 1;
            result.errors.push(format!("invalid report path: {report_path}"));
            continue;
        };
        if !source.is_file() {
            result.failed_files += 1;
            result
                .errors
                .push(format!("report does not exist: {}", source.display()));
            continue;
        }

        let target = target_dir.join(file_name);
        if target.exists() {
            let same_file = source
                .canonicalize()
                .ok()
                .zip(target.canonicalize().ok())
                .is_some_and(|(left, right)| left == right);
            let same_content = compute_sha256(&source)
                .ok()
                .zip(compute_sha256(&target).ok())
                .is_some_and(|(left, right)| left == right);
            if same_file || same_content {
                result.skipped_files += 1;
                continue;
            }
        }

        match std::fs::copy(&source, &target) {
            Ok(_) => {
                result.copied_files += 1;
            }
            Err(error) => {
                result.failed_files += 1;
                result.errors.push(format!(
                    "copy {} to {}: {error}",
                    source.display(),
                    target.display()
                ));
            }
        }
    }

    tracing::info!(
        task_id = %task.id,
        target_dir = %result.target_dir,
        total_files = result.total_files,
        copied_files = result.copied_files,
        skipped_files = result.skipped_files,
        failed_files = result.failed_files,
        "synced ASR daily agent reports"
    );

    Ok(result)
}

fn sync_all_daily_agent_reports(task: &AsrDirectoryTask) -> Result<AsrDailyAgentReportSyncResult, String> {
    let reports: Vec<String> = list_daily_agent_report_files(&task.id)
        .into_iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    sync_daily_agent_report_files(task, &reports)
}

fn update_daily_agent_report_sync_status(
    source_task: &AsrDirectoryTask,
    result: AsrDailyAgentReportSyncResult,
) -> Result<(), String> {
    let task_id = source_task.id.as_str();
    let agent_id = source_task.daily_agent.agent_id.clone();
    let _config_lock = DAILY_AGENT_TASK_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut store = load_tasks();
    let Some(task) = store.tasks.iter_mut().find(|t| t.id == task_id) else {
        return Err(format!("ASR task '{task_id}' not found"));
    };
    sync_daily_agent_item_status(&mut task.daily_agent, &agent_id, |agent| {
        agent.last_report_sync = Some(result.clone());
    });
    mirror_daily_agent_legacy_status(&mut task.daily_agent, &agent_id);
    task.updated_at_ms = now_ms();
    save_tasks(&store)
}
