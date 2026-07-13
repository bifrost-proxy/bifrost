// ─── Daily Agent research fan-out ───────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AsrDailyResearchManifest {
    questions: Vec<AsrDailyResearchQuestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AsrDailyResearchQuestion {
    id: String,
    original_question: String,
    #[serde(default)]
    source_excerpt: String,
    #[serde(default)]
    background: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runner: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    github_repositories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    research_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AsrDailyResearchChildResult {
    question_id: String,
    original_question: String,
    runner: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    github_repositories: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    github_connector_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_profile: Option<String>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    full_report_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

struct AsrDailyResearchExecution {
    run_id: String,
    response: String,
    adapter: String,
    metadata: DailyAgentBTreeMap<String, String>,
}

fn daily_research_runner_adapter(runner_id: &str) -> Option<String> {
    let store = crate::im_gateway::external_cli::ExternalCliConfigStore::new(
        &bifrost_storage::data_dir(),
    );
    let config = store.load();
    config
        .runners
        .get(runner_id)
        .map(|runner| runner.adapter.trim().to_string())
}

fn enforce_daily_research_chatgpt_surface(
    config: &mut crate::im_gateway::external_cli::ExternalCliAdapterConfig,
    fanout: &AsrDailyAgentResearchFanoutConfig,
) {
    let mut chatgpt = config
        .extra
        .remove("chatgpt")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    chatgpt.insert(
        "interfaceMode".to_string(),
        serde_json::Value::String(fanout.chatgpt_interface_mode.clone()),
    );
    chatgpt.insert(
        "model".to_string(),
        serde_json::Value::String(fanout.chatgpt_model.clone()),
    );
    if let Some(project_url) = fanout.chatgpt_project_url.as_deref() {
        chatgpt.insert(
            "projectUrl".to_string(),
            serde_json::Value::String(project_url.to_string()),
        );
    }
    config
        .extra
        .insert("chatgpt".to_string(), serde_json::Value::Object(chatgpt));
}

fn parse_daily_research_manifest(content: &str) -> Result<AsrDailyResearchManifest, String> {
    let trimmed = content.trim();
    let mut candidates = Vec::new();
    if !trimmed.is_empty() {
        candidates.push(trimmed);
    }

    let mut remainder = trimmed;
    while let Some(start) = remainder.find("```json") {
        let after = &remainder[start + "```json".len()..];
        let Some(end) = after.find("```") else {
            break;
        };
        candidates.push(after[..end].trim());
        remainder = &after[end + 3..];
    }

    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start < end {
            candidates.push(&trimmed[start..=end]);
        }
    }

    for candidate in candidates {
        let Ok(manifest) = serde_json::from_str::<AsrDailyResearchManifest>(candidate) else {
            continue;
        };
        if !manifest.questions.is_empty() {
            return Ok(manifest);
        }
    }
    Err("research manifest must contain a non-empty JSON questions array".to_string())
}

fn validate_daily_research_manifest(
    manifest: &AsrDailyResearchManifest,
    fanout: &AsrDailyAgentResearchFanoutConfig,
    default_runner: &str,
) -> Result<(), String> {
    if manifest.questions.len() > fanout.max_questions {
        return Err(format!(
            "research manifest contains {} questions, exceeding max_questions {}",
            manifest.questions.len(),
            fanout.max_questions
        ));
    }
    let mut seen = HashSet::new();
    for question in &manifest.questions {
        if question.id.trim().len() > 128 || !is_valid_daily_agent_token(question.id.trim()) {
            return Err(format!(
                "research question id '{}' must use English letters, numbers, '_' or '-'",
                question.id
            ));
        }
        if !seen.insert(question.id.trim()) {
            return Err(format!("duplicate research question id '{}'", question.id));
        }
        if question.original_question.trim().is_empty() {
            return Err(format!(
                "research question '{}' must preserve original_question",
                question.id
            ));
        }
        if question.original_question.chars().count() > 20_000
            || question.source_excerpt.chars().count() > 100_000
            || question.background.chars().count() > 50_000
            || question
                .research_prompt
                .as_deref()
                .is_some_and(|value| value.chars().count() > 50_000)
        {
            return Err(format!(
                "research question '{}' exceeds the configured prompt field limits",
                question.id
            ));
        }
        for repository in &question.github_repositories {
            let repository = repository.trim();
            let valid = !repository.is_empty()
                && repository.len() <= 200
                && repository.split('/').all(|part| {
                    !part.is_empty()
                        && part != "."
                        && part != ".."
                        && part
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
                });
            if !valid {
                return Err(format!(
                    "research question '{}' contains invalid GitHub repository '{}'",
                    question.id, repository
                ));
            }
        }
        let runner = question.runner.as_deref().unwrap_or(default_runner).trim();
        let allowed = runner == default_runner
            || fanout
                .allowed_runners
                .iter()
                .any(|allowed| allowed.trim() == runner);
        if !allowed {
            return Err(format!(
                "research question '{}' selected runner '{}' outside the configured allowlist",
                question.id, runner
            ));
        }
        if let Some(profile) = question.context_profile.as_deref() {
            if !fanout.context_profiles.contains_key(profile.trim()) {
                return Err(format!(
                    "research question '{}' selected unknown context profile '{}'",
                    question.id, profile
                ));
            }
        }
    }
    Ok(())
}

fn load_daily_research_manifest_for_date(
    task: &AsrDirectoryTask,
    date: &str,
) -> Result<AsrDailyResearchManifest, String> {
    let mut errors = Vec::new();
    for dependency in task
        .daily_agent
        .dependencies
        .iter()
        .filter(|dependency| dependency.include_output)
    {
        let path = daily_agent_upstream_input_dir(task, &dependency.agent_id)
            .join(format!("{date}-report.md"));
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        match parse_daily_research_manifest(&content) {
            Ok(manifest) => return Ok(manifest),
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    Err(if errors.is_empty() {
        format!("no research manifest dependency output found for {date}")
    } else {
        format!(
            "no valid research manifest dependency output found for {date}: {}",
            errors.join("; ")
        )
    })
}

fn build_daily_research_context_prompt(
    question: &AsrDailyResearchQuestion,
    profile: &AsrDailyAgentResearchContextProfile,
) -> String {
    format!(
        "你是研究上下文收集器。只收集回答原始问题所需的可核事实，不替代最终研究。\n\n原始问题：\n{}\n\n提出问题时的背景：\n{}\n\n原始片段：\n{}\n\n上下文配置要求：\n{}\n\n请输出：数据口径、查到的事实、缺失数据、引用的本地文件或查询来源。",
        question.original_question.trim(),
        question.background.trim(),
        question.source_excerpt.trim(),
        profile.instructions.as_deref().unwrap_or("按当前仓库的真实数据和说明文件核验")
    )
}

fn build_daily_research_child_prompt(
    task: &AsrDirectoryTask,
    question: &AsrDailyResearchQuestion,
    context: Option<&str>,
    research_runner_can_read_context_repo: bool,
) -> String {
    let base_instructions = std::fs::read_to_string(daily_agent_instructions_path(task))
        .unwrap_or_else(|_| daily_agent_instruction_content(task));
    let mut prompt = format!(
        "你正在独立研究一个问题。必须直接回答原始问题，不要给问题打优先级分数，也不要把它改写成另一个问题。\n\n## 原始问题\n{}\n\n## 提出问题时的背景\n{}\n\n## 原始片段\n{}\n\n## 本研究 Agent 的要求\n{}\n",
        question.original_question.trim(),
        question.background.trim(),
        question.source_excerpt.trim(),
        base_instructions.trim()
    );
    if let Some(research_prompt) = question.research_prompt.as_deref() {
        prompt.push_str("\n## 单题补充要求\n");
        prompt.push_str(research_prompt.trim());
        prompt.push('\n');
    }
    if !question.github_repositories.is_empty() {
        prompt.push_str("\n## 必须使用的 GitHub Connector 仓库\n");
        for repository in &question.github_repositories {
            prompt.push_str(&format!("- `{}`\n", repository.trim()));
        }
        prompt.push_str(
            "请通过 ChatGPT 已连接的 GitHub Connector 实际检索这些仓库。报告必须列出实际读取的仓库文件路径；如果仓库不可见、未授权或尚未索引，明确输出 `GITHUB_CONNECTOR_STATUS: unavailable`，不要凭空推测仓库内容。成功读取时输出 `GITHUB_CONNECTOR_STATUS: verified`。\n",
        );
    }
    if let Some(context) = context {
        prompt.push_str("\n## 已核验的上下文事实\n");
        prompt.push_str(context.trim());
        prompt.push('\n');
    } else if research_runner_can_read_context_repo {
        prompt.push_str("\n你可以直接读取当前工作目录中的真实数据；先核验事实，再完成研究。\n");
    }
    prompt.push_str(
        "\n最终输出 Markdown，必须包含：`## 原始问题`、`## 核心结论`、`## 事实与证据`、`## 推断与不确定性`、`## 对原始问题的直接回答`。保留原始问题全文。",
    );
    prompt
}

async fn run_daily_research_child(
    task: &AsrDirectoryTask,
    runner_id: &str,
    prompt: String,
    work_dir: &Path,
    session_key: &str,
) -> Result<AsrDailyResearchExecution, String> {
    let config_store =
        crate::im_gateway::external_cli::ExternalCliConfigStore::new(&bifrost_storage::data_dir());
    let config = config_store.load();
    if !config.runners.contains_key(runner_id) {
        return Err(format!("research child runner '{runner_id}' not found"));
    }
    let effective = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
        &config,
        None,
        Some(runner_id),
    );
    if !effective.settings.enabled {
        return Err(format!("research child runner '{runner_id}' is disabled"));
    }
    let adapter = effective.settings.adapter.as_str();
    let request_session_key = (adapter != "chatgpt_web").then(|| session_key.to_string());
    if adapter == "chatgpt_web" {
        crate::im_gateway::chatgpt_web::clear_session_conversation(session_key).await;
    }
    let operation = if adapter == "codex" { "run" } else { "send" };
    let mut adapter_config = daily_agent_external_runner_adapter_config(
        adapter,
        &effective.settings.adapter_config,
        task.daily_agent.timeout_ms,
    );
    if adapter == "chatgpt_web" {
        let fanout = task
            .daily_agent
            .research_fanout
            .as_ref()
            .ok_or_else(|| "research fan-out config is missing".to_string())?;
        enforce_daily_research_chatgpt_surface(&mut adapter_config, fanout);
    }
    let agent_work_dir = daily_agent_work_dir(task);
    let daily_dir = daily_dir_for_task(&task.id);
    let mut allow_work_dirs = vec![
        work_dir.to_string_lossy().to_string(),
        agent_work_dir.to_string_lossy().to_string(),
        daily_dir.to_string_lossy().to_string(),
    ];
    allow_work_dirs.sort();
    allow_work_dirs.dedup();
    let request = crate::im_gateway::external_cli::ExternalCliRunRequest {
        images: Vec::new(),
        message: prompt,
        operation: operation.to_string(),
        params: serde_json::json!({}),
        provider_id: None,
        runner_id: Some(runner_id.to_string()),
        session_key: request_session_key,
        runtime: "external_cli".to_string(),
        adapter: effective.settings.adapter.clone(),
        work_dir: Some(work_dir.to_path_buf()),
        instructions: None,
        adapter_config,
        allow_work_dirs,
        inject_bifrost_tools: effective.settings.inject_bifrost_tools,
        skill_paths: effective.settings.skill_paths.clone(),
    };
    let runtime = crate::im_gateway::external_cli::ExternalCliRuntime::new(
        bifrost_storage::data_dir().join("im_gateway/runs"),
    );
    let result = tokio::time::timeout(
        Duration::from_millis(task.daily_agent.timeout_ms),
        runtime.run(request),
    )
    .await
    .map_err(|_| {
        format!(
            "research child run timed out after {}ms",
            task.daily_agent.timeout_ms
        )
    })?
    .map_err(|error| format!("research child runner failed: {error}"))?;
    if result.status != crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded {
        return Err(format!(
            "research child runner returned {:?}: {}",
            result.status, result.response
        ));
    }
    Ok(AsrDailyResearchExecution {
        run_id: result.run_id,
        response: result.response,
        adapter: result.adapter,
        metadata: result.metadata,
    })
}

fn daily_research_conversation_link(
    execution: &AsrDailyResearchExecution,
) -> (Option<String>, Option<String>) {
    let conversation_id = metadata_value(
        &execution.metadata,
        &["conversationId", "conversation_id"],
    );
    let link = (execution.adapter == "chatgpt_web")
        .then(|| {
            conversation_id
                .as_deref()
                .map(|id| format!("https://chatgpt.com/c/{id}"))
        })
        .flatten();
    (conversation_id, link)
}

fn daily_research_github_connector_status(
    question: &AsrDailyResearchQuestion,
    response: &str,
) -> Option<&'static str> {
    if question.github_repositories.is_empty() {
        return None;
    }
    let marker = response.lines().find_map(|line| {
        let line = line
            .trim()
            .trim_matches(|ch| matches!(ch, '`' | '*' | '-' | ' '));
        line.strip_prefix("GITHUB_CONNECTOR_STATUS:")
            .map(str::trim)
    });
    Some(match marker {
        Some(value) if value.eq_ignore_ascii_case("verified") => "verified",
        Some(value) if value.eq_ignore_ascii_case("unavailable") => "unavailable",
        _ => "missing",
    })
}

fn render_daily_research_index(
    date: &str,
    results: &[AsrDailyResearchChildResult],
) -> String {
    let mut report = format!("# {date} 独立研究结果\n\n");
    report.push_str("每一项均保留原始问题，并由独立研究会话处理。\n\n");
    for result in results {
        report.push_str(&format!("## {}\n\n", result.original_question.trim()));
        report.push_str(&format!("- 状态：{}\n", result.status));
        report.push_str(&format!("- Runner：{}\n", result.runner));
        if !result.github_repositories.is_empty() {
            report.push_str(&format!(
                "- GitHub 仓库：{}\n",
                result.github_repositories.join(", ")
            ));
        }
        if let Some(status) = result.github_connector_status.as_deref() {
            report.push_str(&format!("- GitHub Connector：{status}\n"));
        }
        if let Some(link) = result.full_report_link.as_deref() {
            report.push_str(&format!("- 完整研究：[打开独立 ChatGPT 会话]({link})\n"));
        } else if let Some(path) = result.result_path.as_deref() {
            let display_name = Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("research-report.md");
            report.push_str(&format!("- 完整研究文件：`{display_name}`\n"));
        }
        if let Some(error) = result.error.as_deref() {
            report.push_str(&format!("- 错误：{error}\n"));
        }
        if let Some(path) = result.result_path.as_deref() {
            if let Ok(content) = std::fs::read_to_string(path) {
                report.push('\n');
                report.push_str(content.trim());
                report.push('\n');
            }
        }
        report.push_str("\n---\n\n");
    }
    report
}

async fn run_daily_agent_research_fanout(
    task: &AsrDirectoryTask,
    plan: &AsrDailyAgentChangePlan,
) -> Result<(), String> {
    let fanout = task
        .daily_agent
        .research_fanout
        .as_ref()
        .ok_or_else(|| "research fan-out config is missing".to_string())?;
    let agent_work_dir = daily_agent_work_dir(task);
    for entry in plan
        .entries
        .iter()
        .filter(|entry| entry.change_kind != DailyAgentChangeKind::Unchanged)
    {
        let manifest = load_daily_research_manifest_for_date(task, &entry.date)?;
        validate_daily_research_manifest(&manifest, fanout, &task.daily_agent.runner)?;
        let report_path = PathBuf::from(&entry.report_target);
        let output_dir = report_path
            .parent()
            .ok_or_else(|| format!("invalid research report path: {}", report_path.display()))?;
        let child_dir = output_dir.join(&entry.date);
        std::fs::create_dir_all(&child_dir)
            .map_err(|error| format!("create research output dir failed: {error}"))?;
        atomic_json_write(&child_dir.join("manifest.json"), &manifest)?;

        let mut results = Vec::new();
        let mut success_count = 0usize;
        for question in &manifest.questions {
            let question_id = question.id.trim();
            let final_runner = question
                .runner
                .as_deref()
                .unwrap_or(&task.daily_agent.runner)
                .trim()
                .to_string();
            let result_path = child_dir.join(format!("{question_id}.md"));
            let context_path = child_dir.join(format!("{question_id}-context.md"));
            let metadata_path = child_dir.join(format!("{question_id}.json"));
            let context_profile = question
                .context_profile
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .and_then(|key| fanout.context_profiles.get(key).map(|profile| (key, profile)));
            let final_adapter = daily_research_runner_adapter(&final_runner)
                .unwrap_or_else(|| final_runner.clone());
            let context_adapter = context_profile
                .and_then(|(_, profile)| daily_research_runner_adapter(profile.runner.trim()));
            let direct_context_access = context_profile.is_some_and(|(_, profile)| {
                profile.runner.trim() == final_runner && final_adapter != "chatgpt_web"
            });
            let final_work_dir = if direct_context_access {
                PathBuf::from(context_profile.expect("checked context profile").1.work_dir.trim())
            } else {
                agent_work_dir.clone()
            };

            let execution = async {
                if !final_work_dir.is_dir() {
                    return Err(format!(
                        "research work_dir does not exist: {}",
                        final_work_dir.display()
                    ));
                }
                let mut context_content = None;
                if let Some((profile_key, profile)) = context_profile {
                    if !direct_context_access {
                        if context_adapter.as_deref() == Some("chatgpt_web") {
                            return Err(format!(
                                "context profile '{profile_key}' must use a local file-capable runner, not ChatGPT Web"
                            ));
                        }
                        let context_work_dir = PathBuf::from(profile.work_dir.trim());
                        if !context_work_dir.is_dir() {
                            return Err(format!(
                                "context profile '{profile_key}' work_dir does not exist: {}",
                                context_work_dir.display()
                            ));
                        }
                        let context_prompt = build_daily_research_context_prompt(question, profile);
                        let context_session = format!(
                            "asr-research:{}:{}:{}:{}:context",
                            task.id, task.daily_agent.agent_id, entry.date, question_id
                        );
                        let context_execution = run_daily_research_child(
                            task,
                            profile.runner.trim(),
                            context_prompt,
                            &context_work_dir,
                            &context_session,
                        )
                        .await?;
                        std::fs::write(&context_path, context_execution.response.trim())
                            .map_err(|error| format!("write context result failed: {error}"))?;
                        context_content = Some(context_execution.response);
                    }
                }
                let prompt = build_daily_research_child_prompt(
                    task,
                    question,
                    context_content.as_deref(),
                    direct_context_access,
                );
                let child_session = format!(
                    "asr-research:{}:{}:{}:{}:research",
                    task.id, task.daily_agent.agent_id, entry.date, question_id
                );
                run_daily_research_child(
                    task,
                    &final_runner,
                    prompt,
                    &final_work_dir,
                    &child_session,
                )
                .await
            }
            .await;

            let child_result = match execution {
                Ok(execution) => {
                    std::fs::write(&result_path, execution.response.trim())
                        .map_err(|error| format!("write research result failed: {error}"))?;
                    let (conversation_id, full_report_link) =
                        daily_research_conversation_link(&execution);
                    let connector_status =
                        daily_research_github_connector_status(question, &execution.response);
                    let status = match connector_status {
                        Some("verified") | None => "success",
                        Some("unavailable") if context_path.is_file() => {
                            "success_with_local_context"
                        }
                        Some("unavailable") => "github_connector_unavailable",
                        Some(_) => "github_connector_unverified",
                    };
                    success_count += 1;
                    AsrDailyResearchChildResult {
                        question_id: question_id.to_string(),
                        original_question: question.original_question.clone(),
                        runner: final_runner.clone(),
                        github_repositories: question.github_repositories.clone(),
                        github_connector_status: connector_status.map(str::to_string),
                        context_profile: question.context_profile.clone(),
                        status: status.to_string(),
                        run_id: Some(execution.run_id),
                        conversation_id,
                        full_report_link,
                        result_path: Some(result_path.to_string_lossy().to_string()),
                        context_path: context_path
                            .is_file()
                            .then(|| context_path.to_string_lossy().to_string()),
                        error: None,
                    }
                }
                Err(error) => AsrDailyResearchChildResult {
                    question_id: question_id.to_string(),
                    original_question: question.original_question.clone(),
                    runner: final_runner.clone(),
                    github_repositories: question.github_repositories.clone(),
                    github_connector_status: None,
                    context_profile: question.context_profile.clone(),
                    status: "failed".to_string(),
                    run_id: None,
                    conversation_id: None,
                    full_report_link: None,
                    result_path: None,
                    context_path: context_path
                        .is_file()
                        .then(|| context_path.to_string_lossy().to_string()),
                    error: Some(error),
                },
            };
            atomic_json_write(&metadata_path, &child_result)?;
            results.push(child_result);
        }

        let report = render_daily_research_index(&entry.date, &results);
        if let Some(parent) = report_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create research report dir failed: {error}"))?;
        }
        std::fs::write(&report_path, report)
            .map_err(|error| format!("write research index report failed: {error}"))?;
        if success_count == 0 {
            return Err(format!(
                "all {} research child runs failed for {}",
                results.len(), entry.date
            ));
        }
    }
    Ok(())
}
