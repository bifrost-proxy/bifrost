// ─── Daily Agent Records And Report Discovery ───────────────────────────────

fn daily_agent_report_dirs_for_task(task_id: &str) -> Vec<PathBuf> {
    let daily_dir = daily_dir_for_task(task_id);
    let mut exact_lower = Vec::new();
    let mut case_compat = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&daily_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name == "report" {
                exact_lower.push(path);
            } else if name.eq_ignore_ascii_case("report") {
                case_compat.push(path);
            }
        }
    }

    exact_lower.sort();
    case_compat.sort();
    let mut dirs = exact_lower;
    dirs.extend(case_compat);
    if dirs.is_empty() {
        dirs.push(daily_dir.join("report"));
    }
    dirs
}

fn daily_agent_report_date_from_path(path: &Path) -> Option<String> {
    let filename = path.file_name()?.to_str()?;
    let date = filename.strip_suffix("-report.md")?;
    if NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok() {
        Some(date.to_string())
    } else {
        None
    }
}

fn list_daily_agent_report_files(task_id: &str) -> Vec<PathBuf> {
    let mut reports = Vec::new();
    for report_dir in daily_agent_report_dirs_for_task(task_id) {
        let Ok(entries) = std::fs::read_dir(&report_dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() && daily_agent_report_date_from_path(&path).is_some() {
                reports.push(path);
            }
        }
    }
    reports.sort();
    reports
}

fn build_daily_agent_report_index_status(
    task_id: &str,
    processed: &AsrDailyAgentProcessedState,
) -> AsrDailyAgentReportIndexStatus {
    let report_files = list_daily_agent_report_files(task_id);
    let report_dates: HashSet<String> = report_files
        .iter()
        .filter_map(|path| daily_agent_report_date_from_path(path))
        .collect();

    let mut unindexed_dates: Vec<String> = report_dates
        .iter()
        .filter(|date| !processed.documents.contains_key(*date))
        .cloned()
        .collect();
    unindexed_dates.sort();

    let processed_missing_report = processed
        .documents
        .keys()
        .filter(|date| !report_dates.contains(*date))
        .count();

    AsrDailyAgentReportIndexStatus {
        report_files: report_dates.len(),
        processed_documents: processed.documents.len(),
        indexed_reports: report_dates.len().saturating_sub(unindexed_dates.len()),
        unindexed_reports: unindexed_dates.len(),
        processed_missing_report,
        unindexed_dates,
    }
}

fn task_watch_daily_agent(task: &AsrDirectoryTask, limit: usize) -> TaskWatchDailyAgent {
    let processed = load_daily_agent_processed_state(&task.id);
    let report_index = build_daily_agent_report_index_status(&task.id, &processed);
    let daily_dir = daily_dir_for_task(&task.id);
    let mut source_dates = HashSet::new();
    let mut recent_documents = Vec::new();
    let mut processed_documents = 0usize;
    let mut pending_documents = 0usize;

    if let Ok(entries) = std::fs::read_dir(&daily_dir) {
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if filename.starts_with('.') || filename == "AGENTS.md" || !filename.ends_with(".md") {
                continue;
            }
            let date = filename.trim_end_matches(".md");
            if !is_valid_date_format(date) {
                continue;
            }

            source_dates.insert(date.to_string());
            let source_len_bytes = source_size(&path).unwrap_or_default();
            let source_sha256 = compute_sha256(&path).unwrap_or_default();
            let processed_document = processed.documents.get(date);
            let report_path = processed_document
                .and_then(|document| document.report_path.clone())
                .or_else(|| {
                    daily_agent_report_path_for_date(&task.id, date)
                        .ok()
                        .map(|path| path.to_string_lossy().to_string())
                });
            let report_exists = report_path
                .as_deref()
                .map(|path| Path::new(path).exists())
                .unwrap_or(false);

            let change_kind = match processed_document {
                None => "new_file",
                Some(document) if document.source_sha256 == source_sha256 && report_exists => {
                    "unchanged"
                }
                Some(document) if document.source_sha256 == source_sha256 => "missing_report",
                Some(document) if source_len_bytes > document.source_len_bytes => "appended",
                Some(_) => "rewritten",
            };
            let status = if change_kind == "unchanged" {
                processed_documents += 1;
                "processed"
            } else {
                pending_documents += 1;
                "pending"
            };

            recent_documents.push(TaskWatchDailyAgentDocument {
                date: date.to_string(),
                status: status.to_string(),
                change_kind: change_kind.to_string(),
                source_path: Some(path),
                report_path,
                source_len_bytes: Some(source_len_bytes),
                processed_at_ms: processed_document.map(|document| document.processed_at_ms),
                runner: processed_document.map(|document| document.runner.clone()),
                last_run_id: processed_document.map(|document| document.last_run_id.clone()),
            });
        }
    }

    for report_path in list_daily_agent_report_files(&task.id) {
        let Some(date) = daily_agent_report_date_from_path(&report_path) else {
            continue;
        };
        if source_dates.contains(&date) {
            continue;
        }
        recent_documents.push(TaskWatchDailyAgentDocument {
            date,
            status: "report_only".to_string(),
            change_kind: "report_only".to_string(),
            source_path: None,
            report_path: Some(report_path.to_string_lossy().to_string()),
            source_len_bytes: None,
            processed_at_ms: source_modified_ms(&report_path),
            runner: Some(task.daily_agent.runner.clone()),
            last_run_id: None,
        });
    }

    recent_documents.sort_by(|left, right| {
        right
            .date
            .cmp(&left.date)
            .then_with(|| right.processed_at_ms.cmp(&left.processed_at_ms))
    });
    recent_documents.truncate(limit);

    let status = if !task.daily_agent.enabled {
        Some("disabled".to_string())
    } else {
        daily_agent_effective_last_status(task).or_else(|| {
            if pending_documents > 0 {
                Some("pending".to_string())
            } else {
                Some("idle".to_string())
            }
        })
    };

    TaskWatchDailyAgent {
        enabled: task.daily_agent.enabled,
        runner: task.daily_agent.runner.clone(),
        status,
        last_run_id: task.daily_agent.last_run_id.clone(),
        last_error: task.daily_agent.last_error.clone(),
        last_run_at_ms: task.daily_agent.last_run_at_ms,
        daily_files: source_dates.len(),
        processed_documents,
        pending_documents,
        report_files: report_index.report_files,
        indexed_reports: report_index.indexed_reports,
        unindexed_reports: report_index.unindexed_reports,
        processed_missing_report: report_index.processed_missing_report,
        recent_documents,
    }
}
