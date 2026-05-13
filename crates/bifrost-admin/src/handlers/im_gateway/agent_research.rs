use super::*;
use bytes::Bytes;
use futures_util::{stream, StreamExt as _};

pub(super) async fn handle_agent_research(
    req: Request<Incoming>,
    service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let rest = rest.trim_end_matches('/');

    if rest == "/config" {
        return match *req.method() {
            Method::GET => {
                let config = service.agent_config_store.load();
                json_response(&config.research.map(normalize_research_config))
            }
            Method::PATCH => {
                let patch: serde_json::Value = match read_body_json(req).await {
                    Ok(value) => value,
                    Err(resp) => return resp,
                };
                let research = match serde_json::from_value::<bifrost_agent::ResearchConfig>(patch)
                {
                    Ok(value) => value,
                    Err(error) => {
                        return error_response(
                            StatusCode::BAD_REQUEST,
                            &format!("invalid research config: {error}"),
                        )
                    }
                };
                let mut config = service.agent_config_store.load();
                config.research = Some(normalize_research_config(research));
                match service.agent_config_store.save(&config) {
                    Ok(()) => json_response(&config.research),
                    Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
                }
            }
            _ => method_not_allowed(),
        };
    }

    if rest == "/providers" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        let config = service.agent_config_store.load();
        let providers = config
            .research
            .as_ref()
            .map(|research| {
                research
                    .providers
                    .iter()
                    .map(|(id, provider)| {
                        serde_json::json!({
                            "id": id,
                            "enabled": provider.enabled,
                            "type": provider.provider_type,
                            "base_url": provider.base_url,
                            "search_url": provider.search_url,
                            "fetch_url": provider.fetch_url,
                            "has_api_key": provider.api_key.as_deref().is_some_and(|v| !v.is_empty()) || provider.env_key.as_deref().is_some_and(|v| !v.is_empty()),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let capabilities = research_capabilities(config.research.as_ref()).await;
        return json_response(&serde_json::json!({
            "providers": providers,
            "capabilities": capabilities,
        }));
    }

    if rest == "/capabilities" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        let config = service.agent_config_store.load();
        return json_response(&serde_json::json!({
            "capabilities": research_capabilities(config.research.as_ref()).await,
        }));
    }

    if rest == "/search/stream" {
        if req.method() != Method::POST {
            return method_not_allowed();
        }
        let body: bifrost_agent::research::provider::ResearchSearchRequest =
            match read_body_json(req).await {
                Ok(value) => value,
                Err(resp) => return resp,
            };
        if body.query.trim().is_empty() {
            return error_response(StatusCode::BAD_REQUEST, "research query is required");
        }
        let runtime = match runtime_from_service(service) {
            Ok(runtime) => Arc::new(runtime),
            Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
        };
        let rx = match runtime.search_stream_channel(body).await {
            Ok(rx) => rx,
            Err(error) => return error_response(StatusCode::BAD_GATEWAY, &error.to_string()),
        };
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx)
            .map(|event| {
                let line = serde_json::json!({
                    "type": "provider_result",
                    "provider_id": event.provider_id,
                    "results": event.results,
                    "error": event.error,
                });
                let json = serde_json::to_string(&line).unwrap_or_else(|_| {
                    "{\"type\":\"error\",\"error\":\"serialize event\"}".to_string()
                });
                Ok::<_, hyper::Error>(hyper::body::Frame::data(Bytes::from(format!("{json}\n"))))
            })
            .chain(stream::once(async {
                Ok::<_, hyper::Error>(hyper::body::Frame::data(Bytes::from(
                    "{\"type\":\"done\"}\n",
                )))
            }));
        let body = http_body_util::StreamBody::new(stream);
        return Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/x-ndjson")
            .header("Cache-Control", "no-cache")
            .body(BodyExt::boxed(body))
            .unwrap();
    }

    if rest == "/search" {
        if req.method() != Method::POST {
            return method_not_allowed();
        }
        let body: bifrost_agent::research::provider::ResearchSearchRequest =
            match read_body_json(req).await {
                Ok(value) => value,
                Err(resp) => return resp,
            };
        if body.query.trim().is_empty() {
            return error_response(StatusCode::BAD_REQUEST, "research query is required");
        }
        let runtime = match runtime_from_service(service) {
            Ok(runtime) => runtime,
            Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
        };
        return match runtime.search(body).await {
            Ok(response) => json_response(&response),
            Err(error) => error_response(StatusCode::BAD_GATEWAY, &error.to_string()),
        };
    }

    if rest == "/providers/test" {
        if req.method() != Method::POST {
            return method_not_allowed();
        }
        #[derive(Deserialize)]
        struct ProviderTestRequest {
            provider_id: Option<String>,
            query: Option<String>,
            limit: Option<usize>,
        }
        let body: ProviderTestRequest = match read_body_json(req).await {
            Ok(value) => value,
            Err(resp) => return resp,
        };
        let runtime = match runtime_from_service(service) {
            Ok(runtime) => runtime,
            Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
        };
        let query = body
            .query
            .unwrap_or_else(|| "bifrost research test".to_string());
        let provider_ids = body.provider_id.clone().into_iter().collect::<Vec<_>>();
        let sources = body
            .provider_id
            .as_ref()
            .and_then(|provider_id| runtime.config().providers.get(provider_id))
            .map(|provider| match provider.provider_type {
                bifrost_agent::research::config::ResearchProviderType::SogouWechatCdp => {
                    vec![bifrost_agent::research::config::ResearchSource::Wechat]
                }
                _ => vec![bifrost_agent::research::config::ResearchSource::Web],
            })
            .unwrap_or_else(|| {
                vec![
                    bifrost_agent::research::config::ResearchSource::Web,
                    bifrost_agent::research::config::ResearchSource::Wechat,
                ]
            });
        return match runtime
            .search(bifrost_agent::research::provider::ResearchSearchRequest {
                query,
                sources,
                provider_ids,
                freshness: None,
                limit: Some(body.limit.unwrap_or(3)),
                fetch_content: false,
                language: None,
            })
            .await
        {
            Ok(response) => json_response(&response),
            Err(error) => error_response(StatusCode::BAD_GATEWAY, &error.to_string()),
        };
    }

    if rest == "/items" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        let query = query_param(req.uri().query(), "query").unwrap_or_default();
        let limit = query_param(req.uri().query(), "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(20);
        let runtime = match runtime_from_service(service) {
            Ok(runtime) => runtime,
            Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
        };
        return match runtime.search_knowledge(&query, limit, None) {
            Ok(results) => json_response(&serde_json::json!({ "results": results })),
            Err(error) => error_response(StatusCode::BAD_REQUEST, &error.to_string()),
        };
    }

    if rest == "/reports" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        return json_response(&serde_json::json!({ "reports": list_reports(service) }));
    }

    if let Some(rest) = rest.strip_prefix("/reports/") {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        let parts = rest.split('/').collect::<Vec<_>>();
        if parts.len() != 2 {
            return error_response(StatusCode::BAD_REQUEST, "expected /reports/:task_id/:date");
        }
        let path = service
            .agent_data_dir
            .join("reports")
            .join(safe_segment(parts[0]))
            .join(format!("{}.md", safe_segment(parts[1])));
        return match std::fs::read_to_string(&path) {
            Ok(content) => json_response(&serde_json::json!({
                "path": path.display().to_string(),
                "content": content,
            })),
            Err(_) => error_response(StatusCode::NOT_FOUND, "report not found"),
        };
    }

    if rest == "/tasks" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        let config = service.agent_config_store.load();
        let tasks = config
            .research
            .as_ref()
            .map(|research| {
                research
                    .tasks
                    .iter()
                    .map(bifrost_agent::research::task::ResearchTaskView::from)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        return json_response(&serde_json::json!({ "tasks": tasks }));
    }

    if let Some(id) = rest
        .strip_prefix("/tasks/")
        .and_then(|value| value.strip_suffix("/run"))
    {
        if req.method() != Method::POST {
            return method_not_allowed();
        }
        let runtime = match runtime_from_service(service) {
            Ok(runtime) => runtime,
            Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
        };
        return match runtime.run_task(id).await {
            Ok(report) => json_response(&report),
            Err(error) => error_response(StatusCode::BAD_REQUEST, &error.to_string()),
        };
    }

    error_response(StatusCode::NOT_FOUND, "Research endpoint not found")
}

fn runtime_from_service(
    service: &ImGatewayService,
) -> Result<bifrost_agent::ResearchRuntime, String> {
    let config = service.agent_config_store.load();
    let Some(research) = config.research else {
        return Err("research is not configured".to_string());
    };
    let research = normalize_research_config(research);
    bifrost_agent::ResearchRuntime::from_config_with_home(research, service.agent_data_dir.clone())
        .map_err(|error| format!("research is not available: {error}"))
}

fn normalize_research_config(
    mut research: bifrost_agent::ResearchConfig,
) -> bifrost_agent::ResearchConfig {
    if research.enabled {
        let preset = research
            .preset
            .clone()
            .unwrap_or_else(|| "personal-cn".to_string());
        bifrost_agent::research::apply_preset(&mut research, &preset);
    }
    research
}

async fn research_capabilities(
    research: Option<&bifrost_agent::ResearchConfig>,
) -> Vec<serde_json::Value> {
    use bifrost_agent::research::config::{ResearchProviderType, ResearchSiteKind};

    let mut capabilities = Vec::new();
    let builtins = [
        (
            "arxiv",
            "arXiv",
            "web",
            Some(ResearchSiteKind::Arxiv),
            ResearchProviderType::FixedSite,
            "https://export.arxiv.org/api/query?search_query=all:{query}",
            true,
            None,
        ),
        (
            "hacker_news",
            "Hacker News",
            "web",
            Some(ResearchSiteKind::HackerNews),
            ResearchProviderType::FixedSite,
            "https://hn.algolia.com/api/v1/search_by_date?query={query}",
            true,
            None,
        ),
        (
            "github_repositories",
            "GitHub Repositories",
            "web",
            Some(ResearchSiteKind::GithubRepositories),
            ResearchProviderType::FixedSite,
            "https://api.github.com/search/repositories?q={query}",
            true,
            None,
        ),
        (
            "generic_web_search",
            "Generic Web Search",
            "web",
            None,
            ResearchProviderType::GenericWebSearch,
            "{base_url}",
            true,
            None,
        ),
        (
            "volc_web_search",
            "Volc Web Search",
            "web",
            None,
            ResearchProviderType::VolcWebSearch,
            "https://open.feedcoopapi.com/search_api/web_search",
            true,
            Some("ARK_TOKEN"),
        ),
        (
            "sogou_wechat_cdp",
            "Sogou WeChat",
            "wechat",
            None,
            ResearchProviderType::SogouWechatCdp,
            "https://weixin.sogou.com/weixin?type=2&p=44351200&ie=utf8&query={query}",
            true,
            None,
        ),
        (
            "tavily",
            "Tavily",
            "web",
            None,
            ResearchProviderType::Tavily,
            "{base_url}",
            true,
            Some("TAVILY_API_KEY"),
        ),
        (
            "exa",
            "Exa",
            "web",
            None,
            ResearchProviderType::Exa,
            "{base_url}",
            true,
            Some("EXA_API_KEY"),
        ),
        (
            "custom_http",
            "Custom HTTP",
            "web",
            None,
            ResearchProviderType::CustomHttp,
            "{base_url}",
            true,
            None,
        ),
        (
            "mcp",
            "MCP Source Bridge",
            "web",
            None,
            ResearchProviderType::Mcp,
            "",
            false,
            None,
        ),
    ];

    for (id, label, source, site, provider_type, search_url_template, supported, env_key) in
        builtins
    {
        let provider = research.and_then(|research| research.providers.get(id));
        let builtin_env_configured = env_key.is_some_and(|key| std::env::var(key).is_ok());
        let configured = provider.is_some() || builtin_env_configured;
        let enabled = provider.is_some_and(|provider| provider.enabled);
        let authorization_status = if !supported {
            "reserved"
        } else if builtin_env_configured {
            "configured"
        } else {
            authorization_status(provider, &provider_type)
        };
        let authorized = matches!(authorization_status, "configured" | "not_required");
        let (login_status, logged_in) = if provider_type == ResearchProviderType::SogouWechatCdp {
            match provider {
                Some(provider) if provider.enabled => {
                    if cdp_endpoint_reachable(&provider.cdp_endpoint_or_default()).await {
                        ("browser_connected", true)
                    } else {
                        ("browser_not_connected", false)
                    }
                }
                Some(_) => ("disabled", false),
                None => ("not_configured", false),
            }
        } else {
            ("not_required", true)
        };
        capabilities.push(serde_json::json!({
            "id": id,
            "label": label,
            "source": source,
            "supported": supported,
            "configured": configured,
            "enabled": enabled,
            "type": provider.as_ref().map(|provider| &provider.provider_type).unwrap_or(&provider_type),
            "site": provider.as_ref().and_then(|provider| provider.site.as_ref()).or(site.as_ref()),
            "authorized": authorized,
            "authorization_status": authorization_status,
            "logged_in": logged_in,
            "login_status": login_status,
            "search_url_template": search_url_template,
        }));
    }

    if let Some(research) = research {
        for (id, provider) in &research.providers {
            if capabilities
                .iter()
                .any(|item| item.get("id").and_then(|value| value.as_str()) == Some(id.as_str()))
            {
                continue;
            }
            let authorization_status =
                authorization_status(Some(provider), &provider.provider_type);
            capabilities.push(serde_json::json!({
                "id": id,
                "label": id,
                "source": if matches!(provider.provider_type, ResearchProviderType::SogouWechatCdp) { "wechat" } else { "web" },
                "supported": true,
                "configured": true,
                "enabled": provider.enabled,
                "type": provider.provider_type,
                "site": provider.site.as_ref(),
                "authorized": matches!(authorization_status, "configured" | "not_required"),
                "authorization_status": authorization_status,
                "logged_in": true,
                "login_status": "not_required",
                "search_url_template": provider.search_url.as_deref().or(provider.base_url.as_deref()),
            }));
        }
    }

    capabilities
}

fn authorization_status(
    provider: Option<&bifrost_agent::research::config::ResearchProviderConfig>,
    provider_type: &bifrost_agent::research::config::ResearchProviderType,
) -> &'static str {
    use bifrost_agent::research::config::ResearchProviderType;

    if matches!(
        provider_type,
        ResearchProviderType::FixedSite | ResearchProviderType::SogouWechatCdp
    ) {
        return "not_required";
    }
    let Some(provider) = provider else {
        return "not_configured";
    };
    if provider
        .api_key
        .as_deref()
        .is_some_and(|value| !value.is_empty())
        || provider
            .env_key
            .as_deref()
            .is_some_and(|value| !value.is_empty() && std::env::var(value).is_ok())
    {
        "configured"
    } else if matches!(
        provider_type,
        ResearchProviderType::GenericWebSearch
            | ResearchProviderType::CustomHttp
            | ResearchProviderType::Mcp
    ) {
        "not_required"
    } else {
        "missing"
    }
}

async fn cdp_endpoint_reachable(endpoint: &str) -> bool {
    let Ok(url) = url::Url::parse(endpoint) else {
        return false;
    };
    let host = url
        .host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase());
    if !matches!(
        host.as_deref(),
        Some("127.0.0.1") | Some("::1") | Some("localhost")
    ) {
        return false;
    }
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(800))
        .build()
    else {
        return false;
    };
    let endpoint = endpoint.trim_end_matches('/');
    client
        .get(format!("{endpoint}/json/version"))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

fn safe_segment(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .collect::<String>()
}

fn list_reports(service: &ImGatewayService) -> Vec<serde_json::Value> {
    let root = service.agent_data_dir.join("reports");
    let mut reports = Vec::new();
    let Ok(task_dirs) = std::fs::read_dir(&root) else {
        return reports;
    };
    for task_dir in task_dirs.flatten() {
        if !task_dir.path().is_dir() {
            continue;
        }
        let task_id = task_dir.file_name().to_string_lossy().to_string();
        let Ok(files) = std::fs::read_dir(task_dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|v| v.to_str()) != Some("md") {
                continue;
            }
            let date = path
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or_default()
                .to_string();
            reports.push(serde_json::json!({
                "task_id": task_id,
                "date": date,
                "path": path.display().to_string(),
            }));
        }
    }
    reports.sort_by(|left, right| {
        right
            .get("date")
            .and_then(|v| v.as_str())
            .cmp(&left.get("date").and_then(|v| v.as_str()))
    });
    reports
}
