// ─── Daily Agent Records And Report Discovery ───────────────────────────────

fn daily_agent_report_dirs_for_task(task_id: &str) -> Vec<PathBuf> {
    let daily_dir = daily_dir_for_task(task_id);
    let mut exact_lower = Vec::new();
    let mut configured = Vec::new();
    let mut case_compat = Vec::new();
    if let Some(task) = find_task(task_id) {
        for agent in normalized_daily_agents(&task.daily_agent) {
            let agent_task = task_for_daily_agent(&task, &agent);
            configured.push(daily_agent_output_dir(&agent_task));
            configured.push(daily_agent_work_dir(&agent_task).join(&agent.output_dir));
            configured.push(daily_dir.join(agent.output_dir));
        }
    }
    if let Ok(entries) = std::fs::read_dir(&daily_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name == "agents" || name.starts_with('.') {
                continue;
            }
            if name == "report" {
                exact_lower.push(path);
            } else if name.eq_ignore_ascii_case("report") {
                case_compat.push(path);
            } else if name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
            {
                configured.push(path);
            }
        }
    }

    configured.sort();
    configured.dedup();
    exact_lower.sort();
    case_compat.sort();
    let mut dirs = configured;
    dirs.extend(exact_lower);
    dirs.extend(case_compat);
    dirs.sort();
    dirs.dedup();
    if dirs.is_empty() {
        dirs.push(daily_dir.join("report"));
    }
    dirs
}

fn agent_output_dir_from_report_path(task: &AsrDirectoryTask, path: &Path) -> String {
    let daily_dir = daily_dir_for_task(&task.id);
    let relative = path
        .parent()
        .and_then(|parent| parent.strip_prefix(&daily_dir).ok())
        .map(Path::to_path_buf);
    if let Some(relative) = relative {
        let parts: Vec<_> = relative
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .collect();
        if parts.first() == Some(&"agents") && parts.get(2) == Some(&"output") && parts.len() >= 4
        {
            return parts[3].to_string();
        }
        if parts.first() == Some(&"agents") && parts.len() >= 3 {
            return parts[2].to_string();
        }
        if let Some(output_dir) = parts.first() {
            return (*output_dir).to_string();
        }
    }
    default_daily_agent_output_dir()
}

fn agent_for_output_dir(task: &AsrDirectoryTask, output_dir: &str) -> AsrDailyAgentItem {
    normalized_daily_agents(&task.daily_agent)
        .into_iter()
        .find(|agent| agent.output_dir == output_dir)
        .unwrap_or_else(|| AsrDailyAgentItem {
            id: output_dir.to_string(),
            name: output_dir.to_string(),
            output_dir: output_dir.to_string(),
            ..daily_agent_item_from_legacy(&task.daily_agent)
        })
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
    let task = find_task(task_id);
    let report_keys: HashSet<String> = report_files
        .iter()
        .filter_map(|path| {
            let date = daily_agent_report_date_from_path(path)?;
            let output_dir = task
                .as_ref()
                .map(|task| agent_output_dir_from_report_path(task, path))
                .unwrap_or_else(default_daily_agent_output_dir);
            if output_dir == DEFAULT_DAILY_AGENT_OUTPUT_DIR {
                Some(date)
            } else {
                Some(format!("{output_dir}:{date}"))
            }
        })
        .collect();

    let mut unindexed_dates: Vec<String> = report_keys
        .iter()
        .filter(|key| {
            !processed
                .documents
                .values()
                .any(|document| {
                    document.date == **key
                        || format!("{}:{}", document.output_dir, document.date) == **key
                })
        })
        .cloned()
        .collect();
    unindexed_dates.sort();

    let processed_missing_report = processed
        .documents
        .values()
        .filter(|document| {
            !report_keys.contains(&document.date)
                && !report_keys.contains(&format!("{}:{}", document.output_dir, document.date))
        })
        .count();

    AsrDailyAgentReportIndexStatus {
        report_files: report_keys.len(),
        processed_documents: processed.documents.len(),
        indexed_reports: report_keys.len().saturating_sub(unindexed_dates.len()),
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
    let mut processed_keys = HashSet::new();
    let mut pending_keys = HashSet::new();

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
            let date = filename.trim_end_matches(".md").to_string();
            if !is_valid_date_format(&date) {
                continue;
            }

            source_dates.insert(date.clone());
            let source_len_bytes = source_size(&path).unwrap_or_default();
            let source_sha256 = compute_sha256(&path).unwrap_or_default();
            for agent in normalized_daily_agents(&task.daily_agent) {
            let agent_task = task_for_daily_agent(task, &agent);
            let processed_key = daily_agent_processed_key(&agent_task, &date);
            let processed_document = processed
                .documents
                .get(&processed_key)
                .or_else(|| (agent.id == DEFAULT_DAILY_AGENT_ID).then(|| processed.documents.get(&date)).flatten());
            let report_path = processed_document
                .and_then(|document| document.report_path.clone())
                .or_else(|| {
                    daily_agent_report_path_for_agent_date(&task.id, &agent.id, &date)
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
                processed_keys.insert(format!("{}:{}", agent.id, date));
                "processed"
            } else {
                pending_keys.insert(format!("{}:{}", agent.id, date));
                "pending"
            };

            recent_documents.push(TaskWatchDailyAgentDocument {
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                output_dir: agent.output_dir.clone(),
                date: date.clone(),
                status: status.to_string(),
                change_kind: change_kind.to_string(),
                source_path: Some(path.clone()),
                report_path,
                source_len_bytes: Some(source_len_bytes),
                processed_at_ms: processed_document.map(|document| document.processed_at_ms),
                runner: processed_document.map(|document| document.runner.clone()),
                last_run_id: processed_document.map(|document| document.last_run_id.clone()),
            });
            }
        }
    }

    for report_path in list_daily_agent_report_files(&task.id) {
        let Some(date) = daily_agent_report_date_from_path(&report_path) else {
            continue;
        };
        let output_dir = agent_output_dir_from_report_path(task, &report_path);
        let agent = agent_for_output_dir(task, &output_dir);
        if source_dates.contains(&date) {
            continue;
        }
        recent_documents.push(TaskWatchDailyAgentDocument {
            agent_id: agent.id.clone(),
            agent_name: agent.name.clone(),
            output_dir: agent.output_dir.clone(),
            date,
            status: "report_only".to_string(),
            change_kind: "report_only".to_string(),
            source_path: None,
            report_path: Some(report_path.to_string_lossy().to_string()),
            source_len_bytes: None,
            processed_at_ms: source_modified_ms(&report_path),
            runner: Some(agent.runner.clone()),
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
            if !pending_keys.is_empty() {
                Some("pending".to_string())
            } else {
                Some("idle".to_string())
            }
        })
    };

    TaskWatchDailyAgent {
        enabled: task.daily_agent.enabled,
        runner: task.daily_agent.runner.clone(),
        agents: normalized_daily_agents(&task.daily_agent),
        status,
        last_run_id: task.daily_agent.last_run_id.clone(),
        last_error: task.daily_agent.last_error.clone(),
        last_run_at_ms: task.daily_agent.last_run_at_ms,
        daily_files: source_dates.len(),
        processed_documents: processed_keys.len(),
        pending_documents: pending_keys.len(),
        report_files: report_index.report_files,
        indexed_reports: report_index.indexed_reports,
        unindexed_reports: report_index.unindexed_reports,
        processed_missing_report: report_index.processed_missing_report,
        recent_documents,
    }
}
