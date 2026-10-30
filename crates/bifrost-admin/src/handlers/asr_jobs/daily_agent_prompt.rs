// ─── Daily Agent Prompt ──────────────────────────────────────────────────────

fn daily_agent_prompt_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn build_daily_agent_prompt(
    task: &AsrDirectoryTask,
    plan: &AsrDailyAgentChangePlan,
    _adapter: &str,
    _first_turn: bool,
) -> Result<String, String> {
    let work_dir = daily_agent_work_dir(task);
    let instructions_path = daily_agent_instructions_path(task);
    let instructions_file = daily_agent_prompt_path(
        instructions_path
            .strip_prefix(&work_dir)
            .unwrap_or_else(|_| Path::new("AGENTS.md")),
    );
    let changed_entries: Vec<_> = plan
        .entries
        .iter()
        .filter(|e| e.change_kind != DailyAgentChangeKind::Unchanged)
        .collect();

    let terms_path = daily_agent_terms_path(task);
    let terms_file = daily_agent_prompt_path(
        terms_path
            .strip_prefix(&work_dir)
            .unwrap_or_else(|_| Path::new(DAILY_AGENT_TERMS_FILENAME)),
    );
    let terms_content = read_daily_agent_terms_content(task);
    let processed = load_daily_agent_processed_state(&task.id);
    let mut upstream_artifacts = Vec::new();
    for dependency in task
        .daily_agent
        .dependencies
        .iter()
        .filter(|dependency| dependency.include_output)
    {
        for entry in &changed_entries {
            let processed_key = format!("{}:{}", dependency.agent_id, entry.date);
            let upstream_document = processed.documents.get(&processed_key).or_else(|| {
                (dependency.agent_id == DEFAULT_DAILY_AGENT_ID)
                    .then(|| processed.documents.get(&entry.date))
                    .flatten()
            });
            if upstream_document
                .is_none_or(|document| document.source_sha256 != entry.source_sha256)
            {
                continue;
            }
            let path = daily_agent_upstream_input_dir(task, &dependency.agent_id)
                .join(format!("{}-report.md", entry.date));
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let relative_path = path
                .strip_prefix(&work_dir)
                .map(daily_agent_prompt_path)
                .unwrap_or_else(|_| daily_agent_prompt_path(&path));
            upstream_artifacts.push((
                dependency.agent_id.clone(),
                entry.date.clone(),
                relative_path,
                content,
            ));
        }
    }

    let mut prompt = String::new();

    if terms_content.is_some() {
        prompt.push_str(&format!(
                "请根据当前目录 `{instructions_file}` 和专有名词文件 `{terms_file}`，检查并处理以下变更文件：\n\n"
            ));
    } else {
        prompt.push_str(&format!(
            "请根据当前目录 `{instructions_file}`，检查并处理以下变更文件：\n\n"
        ));
    }

    for entry in &changed_entries {
        let source_path = daily_agent_source_copy_path(task, &entry.date)
            .strip_prefix(&work_dir)
            .map(daily_agent_prompt_path)
            .unwrap_or_else(|_| entry.source_path.clone());
        let report_path = PathBuf::from(&entry.report_target)
            .strip_prefix(&work_dir)
            .map(daily_agent_prompt_path)
            .unwrap_or_else(|_| entry.report_target.clone());
        prompt.push_str(&format!(
            "- source={}, change_kind={:?}, source_sha256={}, report={}\n",
            source_path,
            entry.change_kind,
            &entry.source_sha256[..8],
            report_path
        ));
    }

    if !upstream_artifacts.is_empty() {
        prompt.push_str("\n本轮可用的上游 Agent 产物：\n\n");
        for (agent_id, date, relative_path, _) in &upstream_artifacts {
            prompt.push_str(&format!(
                "- agent={agent_id}, date={date}, path={relative_path}\n"
            ));
        }
        prompt.push_str("请把这些文件作为本轮输入依据，不要修改上游文件。\n");
    }
    prompt.push_str("\n只刷新这些日期对应的 report。不要修改原始 YYYY-MM-DD.md。\n");

    Ok(prompt)
}

fn read_daily_agent_terms_content(task: &AsrDirectoryTask) -> Option<String> {
    let terms_path = daily_agent_terms_path(task);
    if let Ok(content) = std::fs::read_to_string(&terms_path) {
        return normalize_daily_agent_terminology(Some(content));
    }
    normalize_daily_agent_terminology(task.daily_agent.terminology.clone())
}
