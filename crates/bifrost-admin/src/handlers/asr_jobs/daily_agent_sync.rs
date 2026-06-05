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

fn daily_agent_report_sync_target_dir(task: &AsrDirectoryTask) -> Result<PathBuf, String> {
    let root_dir = configured_daily_agent_report_sync_dir(task)
        .ok_or_else(|| "Daily Agent report sync directory is not configured".to_string())?;

    if root_dir.exists() && !root_dir.is_dir() {
        return Err(format!(
            "Daily Agent report sync target is not a directory: {}",
            root_dir.display()
        ));
    }

    let agent_dir = normalize_daily_agent_token(&task.daily_agent.agent_id);
    let agent_dir = if agent_dir.is_empty() {
        normalize_daily_agent_token(&task.daily_agent.output_dir)
    } else {
        agent_dir
    };

    Ok(root_dir.join(agent_dir))
}

fn sync_daily_agent_report_files(
    task: &AsrDirectoryTask,
    report_paths: &[String],
) -> Result<AsrDailyAgentReportSyncResult, String> {
    let target_dir = daily_agent_report_sync_target_dir(task)?;

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

fn sync_all_daily_agent_reports_by_agent(
    task: &AsrDirectoryTask,
) -> Result<(AsrDailyAgentReportSyncResult, Vec<(AsrDirectoryTask, AsrDailyAgentReportSyncResult)>), String> {
    let root_dir = configured_daily_agent_report_sync_dir(task)
        .ok_or_else(|| "Daily Agent report sync directory is not configured".to_string())?;

    if root_dir.exists() && !root_dir.is_dir() {
        return Err(format!(
            "Daily Agent report sync target is not a directory: {}",
            root_dir.display()
        ));
    }
    std::fs::create_dir_all(&root_dir)
        .map_err(|error| format!("create report sync directory {}: {error}", root_dir.display()))?;

    let reports = list_daily_agent_report_files(&task.id);
    let mut grouped_reports: BTreeMap<String, (AsrDirectoryTask, Vec<String>)> = BTreeMap::new();
    for report_path in reports {
        let output_dir = agent_output_dir_from_report_path(task, &report_path);
        let agent = agent_for_output_dir(task, &output_dir);
        let agent_task = task_for_daily_agent(task, &agent);
        grouped_reports
            .entry(agent.id.clone())
            .or_insert_with(|| (agent_task, Vec::new()))
            .1
            .push(report_path.to_string_lossy().to_string());
    }

    let mut aggregate = AsrDailyAgentReportSyncResult {
        target_dir: root_dir.to_string_lossy().to_string(),
        total_files: 0,
        synced_at_ms: now_ms(),
        ..Default::default()
    };
    let mut per_agent = Vec::new();

    for (_, (agent_task, agent_reports)) in grouped_reports {
        let result = sync_daily_agent_report_files(&agent_task, &agent_reports)?;
        aggregate.total_files += result.total_files;
        aggregate.copied_files += result.copied_files;
        aggregate.skipped_files += result.skipped_files;
        aggregate.failed_files += result.failed_files;
        aggregate.errors.extend(result.errors.clone());
        per_agent.push((agent_task, result));
    }

    Ok((aggregate, per_agent))
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
