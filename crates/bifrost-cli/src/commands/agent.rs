use crate::cli::{
    AgentCommands, AgentResearchCommands, AgentResearchKnowledgeCommands,
    AgentResearchProviderCommands, AgentResearchReportCommands, AgentResearchTaskCommands,
};
use bifrost_agent::config::{agent_home_dir, AgentConfigStore};
use bifrost_agent::research::config::{
    ResearchProviderConfig, ResearchProviderType, ResearchSource, WechatResearchConfig,
};
use bifrost_agent::research::provider::{
    FetchedDocument, ResearchFetchRequest, ResearchSearchRequest, ResearchSearchResponse,
};
use bifrost_agent::{ResearchConfig, ResearchRuntime};
use bifrost_core::Result;
use colored::Colorize;

pub fn handle_agent_command(action: AgentCommands) -> Result<()> {
    match action {
        AgentCommands::Research { action } => handle_research_command(action),
    }
}

fn handle_research_command(action: AgentResearchCommands) -> Result<()> {
    match action {
        AgentResearchCommands::Init {
            preset,
            web_provider,
            base_url,
            api_key,
            wechat_cdp_endpoint,
            yes: _,
        } => {
            let store = AgentConfigStore::new(&agent_home_dir());
            let mut config = store.load();
            let mut research = config
                .research
                .unwrap_or_else(bifrost_agent::research::default_enabled_config);
            research.enabled = true;
            bifrost_agent::research::apply_preset(&mut research, &preset);
            if let Some(base_url) = base_url {
                research.providers.insert(
                    web_provider.clone(),
                    ResearchProviderConfig {
                        provider_type: ResearchProviderType::GenericWebSearch,
                        base_url: Some(base_url),
                        api_key,
                        ..Default::default()
                    },
                );
                research.provider_order.retain(|id| id != &web_provider);
                research.provider_order.insert(0, web_provider);
            }
            if let Some(cdp_endpoint) = wechat_cdp_endpoint {
                research.wechat = Some(WechatResearchConfig {
                    enabled: true,
                    provider: Some("sogou_wechat_cdp".to_string()),
                    ..Default::default()
                });
                research.providers.insert(
                    "sogou_wechat_cdp".to_string(),
                    ResearchProviderConfig {
                        provider_type: ResearchProviderType::SogouWechatCdp,
                        cdp_endpoint: Some(cdp_endpoint),
                        ..Default::default()
                    },
                );
                let provider_id = "sogou_wechat_cdp".to_string();
                if !research.provider_order.contains(&provider_id) {
                    research.provider_order.push(provider_id);
                }
            }
            config.research = Some(research);
            store
                .save(&config)
                .map_err(bifrost_core::BifrostError::Config)?;
            println!("{}", "Research Pack initialized".bright_green());
            println!(
                "Config: {}",
                agent_home_dir().join("agent_config.json").display()
            );
            Ok(())
        }
        AgentResearchCommands::Provider { action } => match action {
            AgentResearchProviderCommands::Test { provider, query } => {
                let mut runtime_config = load_research_config()?;
                if let Some(provider_id) = &provider {
                    if !runtime_config.providers.contains_key(provider_id) {
                        return Err(bifrost_core::BifrostError::Config(format!(
                            "research provider '{}' is not configured",
                            provider_id
                        )));
                    }
                }
                let sources = provider
                    .as_ref()
                    .and_then(|provider_id| runtime_config.providers.get(provider_id))
                    .map(|provider_config| match provider_config.provider_type {
                        ResearchProviderType::SogouWechatCdp => vec![ResearchSource::Wechat],
                        _ => vec![ResearchSource::Web],
                    })
                    .unwrap_or_else(|| vec![ResearchSource::Web, ResearchSource::Wechat]);
                if let Some(provider) = provider {
                    runtime_config.providers.retain(|id, _| id == &provider);
                    runtime_config.provider_order = vec![provider];
                }
                let runtime = ResearchRuntime::from_config(runtime_config)
                    .map_err(|error| bifrost_core::BifrostError::Config(error.to_string()))?;
                let response: ResearchSearchResponse =
                    block_on(runtime.search(ResearchSearchRequest {
                        query,
                        sources,
                        provider_ids: Vec::new(),
                        freshness: None,
                        limit: Some(3),
                        fetch_content: false,
                        language: None,
                    }))?;
                print_json(&response)
            }
        },
        AgentResearchCommands::Search {
            query,
            limit,
            wechat,
            fetch_content,
        } => {
            let runtime = load_runtime()?;
            let mut sources = vec![ResearchSource::Web];
            if wechat {
                sources.push(ResearchSource::Wechat);
            }
            let response: ResearchSearchResponse =
                block_on(runtime.search(ResearchSearchRequest {
                    query,
                    sources,
                    provider_ids: Vec::new(),
                    freshness: None,
                    limit: Some(limit),
                    fetch_content,
                    language: None,
                }))?;
            print_json(&response)
        }
        AgentResearchCommands::Fetch { url, max_bytes } => {
            let runtime = load_runtime()?;
            let response: FetchedDocument = block_on(runtime.fetch(ResearchFetchRequest {
                url,
                format: "markdown".to_string(),
                max_bytes,
            }))?;
            print_json(&response)
        }
        AgentResearchCommands::Knowledge { action } => match action {
            AgentResearchKnowledgeCommands::Search { query, limit } => {
                let runtime = load_runtime()?;
                let results = runtime
                    .search_knowledge(&query, limit, None)
                    .map_err(|error| bifrost_core::BifrostError::Config(error.to_string()))?;
                print_json(&serde_json::json!({ "results": results }))
            }
        },
        AgentResearchCommands::Report { action } => match action {
            AgentResearchReportCommands::Latest { task_id } => {
                let report = latest_report(task_id.as_deref())?;
                println!("{}", report.display());
                Ok(())
            }
            AgentResearchReportCommands::Generate { task_id, query } => {
                let runtime = load_runtime()?;
                let response = runtime
                    .digest(task_id, None, query)
                    .map_err(|error| bifrost_core::BifrostError::Config(error.to_string()))?;
                print_json(&response)
            }
        },
        AgentResearchCommands::Task { action } => match action {
            AgentResearchTaskCommands::List => {
                let research = load_research_config()?;
                let tasks = research
                    .tasks
                    .iter()
                    .map(bifrost_agent::research::task::ResearchTaskView::from)
                    .collect::<Vec<_>>();
                print_json(&serde_json::json!({ "tasks": tasks }))
            }
            AgentResearchTaskCommands::Run { task_id } => {
                let runtime = load_runtime()?;
                let response: bifrost_agent::research::digest::ResearchDigestResponse =
                    block_on(runtime.run_task(&task_id))?;
                print_json(&response)
            }
        },
    }
}

fn load_research_config() -> Result<ResearchConfig> {
    let store = AgentConfigStore::new(&agent_home_dir());
    let config = store.load();
    config
        .research
        .filter(|research| research.enabled)
        .ok_or_else(|| bifrost_core::BifrostError::Config("Research Pack is not enabled".into()))
}

fn load_runtime() -> Result<ResearchRuntime> {
    ResearchRuntime::from_config(load_research_config()?)
        .map_err(|error| bifrost_core::BifrostError::Config(error.to_string()))
}

fn block_on<T, E>(future: impl std::future::Future<Output = std::result::Result<T, E>>) -> Result<T>
where
    E: std::fmt::Display,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| bifrost_core::BifrostError::Config(error.to_string()))?
        .block_on(future)
        .map_err(|error| bifrost_core::BifrostError::Config(error.to_string()))
}

fn print_json<T: serde::Serialize + ?Sized>(value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| bifrost_core::BifrostError::Config(error.to_string()))?;
    println!("{text}");
    Ok(())
}

fn latest_report(task_id: Option<&str>) -> Result<std::path::PathBuf> {
    let root = agent_home_dir().join("reports");
    let mut candidates = Vec::new();
    if let Some(task_id) = task_id {
        collect_reports(&root.join(task_id), &mut candidates);
    } else if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                collect_reports(&entry.path(), &mut candidates);
            }
        }
    }
    candidates.sort();
    candidates
        .pop()
        .ok_or_else(|| bifrost_core::BifrostError::Config("No research reports found".into()))
}

fn collect_reports(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("md") {
            out.push(path);
        }
    }
}
