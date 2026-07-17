// ─── Official Daily Agent templates ─────────────────────────────────────────

const OFFICIAL_DAILY_RESEARCH_TEMPLATE_ID: &str = "daily-research";
const OFFICIAL_DAILY_RESEARCH_TEMPLATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
struct AsrDailyAgentTemplate {
    id: String,
    version: u32,
    name: String,
    description: String,
    official: bool,
    prompt_customizable: bool,
    execution_mode: String,
    agents: Vec<AsrDailyAgentItem>,
}

#[derive(Debug, Deserialize)]
struct ApplyAsrDailyAgentTemplateRequest {
    template_id: String,
    #[serde(default)]
    primary_runner: Option<String>,
    #[serde(default)]
    research_runner: Option<String>,
}

fn official_daily_agent_template_item(
    id: &str,
    runner: &str,
    instructions: &str,
    output_dir: &str,
    dependencies: Vec<AsrDailyAgentDependency>,
) -> AsrDailyAgentItem {
    AsrDailyAgentItem {
        id: id.to_string(),
        name: id.to_string(),
        enabled: true,
        runner: runner.to_string(),
        timeout_ms: DEFAULT_DAILY_AGENT_TIMEOUT_MS,
        trigger_policy: AsrDailyAgentTriggerPolicy::AfterAsrRun,
        session_key: None,
        instructions_source: AsrDailyAgentInstructionsSource::Custom,
        instructions: Some(instructions.to_string()),
        chatgpt_project_url: None,
        im_delivery: AsrDailyAgentImDeliveryConfig::default(),
        output_dir: output_dir.to_string(),
        dependencies,
        dependency_failure_policy: AsrDailyAgentDependencyFailurePolicy::Skip,
        research_fanout: None,
        report_sync_dir: None,
        last_report_sync: None,
        last_run_at_ms: None,
        last_status: None,
        last_error: None,
        last_run_id: None,
    }
}

fn official_daily_agent_dependency(agent_id: &str) -> AsrDailyAgentDependency {
    AsrDailyAgentDependency {
        agent_id: agent_id.to_string(),
        include_output: true,
    }
}

fn build_official_daily_research_template(
    primary_runner: &str,
    research_runner: &str,
) -> AsrDailyAgentTemplate {
    let mut fanout = official_daily_agent_template_item(
        "research_fanout",
        research_runner,
        "直接回答每个原始问题，优先使用一手、权威和可复查来源。若问题指定 GitHub 仓库，实际使用已连接的 GitHub Connector 检索并列出读取的文件。",
        "research_result",
        vec![official_daily_agent_dependency("research_dispatcher")],
    );
    fanout.research_fanout = Some(AsrDailyAgentResearchFanoutConfig {
        max_questions: 8,
        max_concurrency: default_research_fanout_max_concurrency(),
        chatgpt_interface_mode: default_research_chatgpt_interface_mode(),
        chatgpt_model: default_research_chatgpt_model(),
        chatgpt_project_url: None,
        allowed_runners: vec![research_runner.to_string()],
        context_profiles: DailyAgentBTreeMap::new(),
    });

    AsrDailyAgentTemplate {
        id: OFFICIAL_DAILY_RESEARCH_TEMPLATE_ID.to_string(),
        version: OFFICIAL_DAILY_RESEARCH_TEMPLATE_VERSION,
        name: "Daily Research".to_string(),
        description: "Generate a daily summary, extract research questions, dispatch independent research, and produce a linked digest.".to_string(),
        official: true,
        prompt_customizable: true,
        execution_mode: "serial_stages_parallel_research_fanout".to_string(),
        agents: vec![
            official_daily_agent_template_item(
                "daily_report",
                primary_runner,
                "根据当日输入生成简短日报，保留事实、待办和用户明确提出的问题。",
                "report",
                Vec::new(),
            ),
            official_daily_agent_template_item(
                "research_seed",
                primary_runner,
                "从上游日报原样提取需要独立研究的问题；保留原始问题、背景和原始片段，不评分。",
                "research_seed",
                vec![official_daily_agent_dependency("daily_report")],
            ),
            official_daily_agent_template_item(
                "research_dispatcher",
                primary_runner,
                "把上游问题转换为 JSON manifest 的 questions 数组；保留原始问题，并按用户自定义 Prompt 选择 Runner、GitHub 仓库和可选上下文。没有问题时输出空数组。",
                "research_manifest",
                vec![official_daily_agent_dependency("research_seed")],
            ),
            fanout,
            official_daily_agent_template_item(
                "research_digest",
                primary_runner,
                "汇总每个研究问题的原始问题、核心结论、不确定性和完整研究链接，不评分。",
                "research_digest",
                vec![official_daily_agent_dependency("research_fanout")],
            ),
        ],
    }
}

fn official_daily_agent_templates() -> Vec<AsrDailyAgentTemplate> {
    vec![build_official_daily_research_template("Codex", "Codex")]
}

fn apply_official_daily_agent_template(
    config: &AsrDailyAgentConfig,
    template_id: &str,
    primary_runner: Option<&str>,
    research_runner: Option<&str>,
) -> Result<AsrDailyAgentConfig, String> {
    if template_id.trim() != OFFICIAL_DAILY_RESEARCH_TEMPLATE_ID {
        return Err(format!("unknown Daily Agent template '{template_id}'"));
    }
    let fallback_runner = normalized_daily_agents(config)
        .into_iter()
        .find_map(|agent| {
            let runner = agent.runner.trim().to_string();
            (!runner.is_empty()).then_some(runner)
        })
        .unwrap_or_else(default_daily_agent_runner);
    let primary_runner = primary_runner
        .map(str::trim)
        .filter(|runner| !runner.is_empty())
        .unwrap_or(&fallback_runner);
    let research_runner = research_runner
        .map(str::trim)
        .filter(|runner| !runner.is_empty())
        .unwrap_or(primary_runner);
    let template = build_official_daily_research_template(primary_runner, research_runner);

    let preserved_terminology = config.terminology.clone();
    let preserved_report_sync_dir = config.report_sync_dir.clone().or_else(|| {
        normalized_daily_agents(config)
            .into_iter()
            .find_map(|agent| agent.report_sync_dir)
    });
    let mut next = config.clone();
    next.enabled = true;
    next.agents = template.agents;
    next.last_run_at_ms = None;
    next.last_status = None;
    next.last_error = None;
    next.last_run_id = None;
    normalize_daily_agent_config_in_place(&mut next);
    next.terminology = normalize_daily_agent_terminology(preserved_terminology);
    set_primary_daily_agent_report_sync_dir(&mut next, preserved_report_sync_dir);
    validate_daily_agent_config(&next)?;
    Ok(next)
}

fn write_official_daily_agent_template_instructions(
    task: &AsrDirectoryTask,
) -> Result<(), String> {
    ensure_asr_daily_workspace(task)?;
    sync_configured_daily_agent_instructions(task)
}

fn get_daily_agent_templates_response() -> Response<BoxBody> {
    json_response(&serde_json::json!({
        "templates": official_daily_agent_templates(),
    }))
}

async fn post_apply_daily_agent_template_response(
    task_id: &str,
    req: Request<Incoming>,
) -> Response<BoxBody> {
    let body = match req.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("read body: {error}"))
        }
    };
    let request: ApplyAsrDailyAgentTemplateRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("invalid JSON: {error}"))
        }
    };

    let _config_lock = DAILY_AGENT_TASK_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut store = load_tasks();
    let Some(task) = store.tasks.iter_mut().find(|task| task.id == task_id) else {
        return error_response(StatusCode::NOT_FOUND, "ASR task not found");
    };
    let next = match apply_official_daily_agent_template(
        &task.daily_agent,
        &request.template_id,
        request.primary_runner.as_deref(),
        request.research_runner.as_deref(),
    ) {
        Ok(next) => next,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    task.daily_agent = next;
    task.updated_at_ms = now_ms();
    if let Err(error) = write_official_daily_agent_template_instructions(task) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    let updated_config = task.daily_agent.clone();
    if let Err(error) = save_tasks(&store) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }

    json_response(&serde_json::json!({
        "ok": true,
        "template_id": request.template_id,
        "config": updated_config,
    }))
}
