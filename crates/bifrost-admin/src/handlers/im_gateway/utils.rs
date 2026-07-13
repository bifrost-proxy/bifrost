use super::*;

// ---------------------------------------------------------------------------

pub(super) fn handle_history(
    req: &Request<Incoming>,
    service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }

    let rest = rest.trim_end_matches('/');
    match rest {
        "/events" | "/events/" => {
            let events = service.event_store.list();
            json_response(&events)
        }
        "/runs" | "/runs/" => {
            let runs = service.run_store.list();
            json_response(&runs)
        }
        _ => error_response(StatusCode::NOT_FOUND, "History endpoint not found"),
    }
}

pub(super) async fn handle_attachment(req: Request<Incoming>, rest: &str) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }

    let relative = rest.trim_start_matches('/');
    let relative = match urlencoding::decode(relative) {
        Ok(value) => value.to_string(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid attachment path"),
    };
    let Some(path) = resolve_attachment_path(&relative) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid attachment path");
    };
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(_) => return error_response(StatusCode::NOT_FOUND, "attachment not found"),
    };
    let content_type = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .header("Cache-Control", "private, max-age=3600")
        .body(full_body(bytes))
        .unwrap()
}

fn resolve_attachment_path(relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return None;
    }
    Some(
        bifrost_agent::config::agent_home_dir()
            .join("im_gateway")
            .join("attachments")
            .join(relative),
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(super) async fn read_body_json<T: serde::de::DeserializeOwned>(
    req: Request<Incoming>,
) -> std::result::Result<T, Response<BoxBody>> {
    let body = req.collect().await.map_err(|e| {
        error_response(
            StatusCode::BAD_REQUEST,
            &format!("Failed to read request body: {e}"),
        )
    })?;
    serde_json::from_slice(&body.to_bytes()).map_err(|e| {
        error_response(
            StatusCode::BAD_REQUEST,
            &format!("Invalid request body: {e}"),
        )
    })
}

pub(super) async fn request_agent_stop(
    manager: &bifrost_agent::AgentSessionManager,
    session_key: &str,
) -> bool {
    request_agent_stop_with_runs_root(
        manager,
        session_key,
        crate::im_gateway::external_cli::default_runs_root(),
    )
    .await
}

pub(super) async fn request_agent_stop_with_runs_root(
    manager: &bifrost_agent::AgentSessionManager,
    session_key: &str,
    runs_root: impl AsRef<Path>,
) -> bool {
    let internal_stopped = manager.request_stop(session_key);
    let external_worker_stopped =
        crate::im_gateway::external_cli::request_worker_session_stop(session_key).await;
    let external_stopped =
        crate::im_gateway::external_cli::request_session_stop(runs_root, session_key)
            .await
            .is_ok();
    internal_stopped || external_worker_stopped || external_stopped
}

/// Extract a path segment that appears before a known suffix.
/// E.g., `extract_segment_before("abc/status", "/status")` returns `Some("abc")`.
pub(super) fn extract_segment_before<'a>(path: &'a str, suffix: &str) -> Option<&'a str> {
    let without_trailing = path.trim_end_matches('/');
    if let Some(id) = without_trailing.strip_suffix(suffix) {
        if !id.is_empty() && !id.contains('/') {
            return Some(id);
        }
    }
    None
}

pub(super) fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

pub(super) fn uuid_short() -> String {
    let id = uuid::Uuid::new_v4();
    id.to_string()[..8].to_string()
}

fn clean_device_name(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
}

fn device_name_from_command(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .and_then(|value| clean_device_name(&value))
}

fn macos_computer_name() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        device_name_from_command("scutil", &["--get", "ComputerName"])
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

pub(super) fn current_device_name() -> String {
    ["BIFROST_DEVICE_NAME", "COMPUTERNAME"]
        .into_iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .and_then(|value| clean_device_name(&value))
        })
        .or_else(macos_computer_name)
        .or_else(|| {
            std::env::var("HOSTNAME")
                .ok()
                .and_then(|value| clean_device_name(&value))
        })
        .or_else(|| device_name_from_command("hostname", &[]))
        .unwrap_or_else(|| "unknown".to_string())
}

fn online_notification_work_dir(provider: &ImProviderConfig) -> String {
    provider
        .agent_config
        .as_ref()
        .and_then(|config| config.work_dir.as_deref())
        .map(str::trim)
        .filter(|work_dir| !work_dir.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            std::env::current_dir()
                .map(|path| path.display().to_string())
                .ok()
        })
        .unwrap_or_else(|| "unknown".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OnlineNotificationAgentContext {
    pub(super) runner_type: String,
    pub(super) runner_id: Option<String>,
    pub(super) model: Option<String>,
    pub(super) model_provider: Option<String>,
    pub(super) model_reasoning_effort: Option<String>,
    pub(super) model_reasoning_summary: Option<String>,
    pub(super) session_key: String,
    pub(super) user_turn_count: u32,
}

pub(super) fn build_online_notification_agent_context(
    provider: &ImProviderConfig,
    base_agent_config: &crate::im_gateway::agent::ImAgentConfig,
    external_cli_config: &crate::im_gateway::external_cli::ExternalCliGatewayConfig,
    agent_session_manager: &bifrost_agent::AgentSessionManager,
) -> OnlineNotificationAgentContext {
    let effective_agent_config = effective_agent_config_for_provider(base_agent_config, provider);
    let runner_override = effective_agent_config
        .runner
        .as_ref()
        .and_then(|runner| runner.custom_runner_id());
    let (runner_type, runner_id, model_config) = {
        let external = crate::im_gateway::external_cli::effective_config_for_provider_and_runner(
            external_cli_config,
            Some(provider.id.as_str()),
            runner_override,
        );
        let model_config =
            crate::im_gateway::external_cli::resolve_external_cli_status_model_config(
                &external.settings.adapter,
                &external.settings.adapter_config,
            );
        (
            external.settings.adapter,
            Some(external.runner_id),
            Some(model_config),
        )
    };
    let session_key = build_session_key(&provider.id, provider.owner_open_id.as_deref());
    let user_turn_count = online_notification_user_turn_count(agent_session_manager, &session_key);

    OnlineNotificationAgentContext {
        runner_type,
        runner_id,
        model: model_config
            .as_ref()
            .and_then(|config| config.model.clone()),
        model_provider: model_config.as_ref().and_then(|config| {
            config
                .model_provider
                .clone()
                .or_else(|| config.model_source.clone())
        }),
        model_reasoning_effort: model_config
            .as_ref()
            .and_then(|config| config.reasoning_effort.clone()),
        model_reasoning_summary: model_config
            .as_ref()
            .and_then(|config| config.reasoning_summary.clone()),
        session_key,
        user_turn_count,
    }
}

fn online_notification_user_turn_count(
    agent_session_manager: &bifrost_agent::AgentSessionManager,
    session_key: &str,
) -> u32 {
    if let Some(detail) = agent_session_manager.get_session_detail(session_key) {
        return u32::try_from(detail.user_turn_count).unwrap_or(u32::MAX);
    }
    if let Some(active) = agent_session_manager.get_active_turn_status(session_key) {
        return u32::try_from(active.user_turn_count).unwrap_or(u32::MAX);
    }

    let data_dir = bifrost_agent::config::agent_home_dir();
    bifrost_agent::persistence::list_conversations(&data_dir, Some(session_key))
        .into_iter()
        .rev()
        .map(|path| bifrost_agent::persistence::scan_session_summary(&path).user_turns)
        .find(|count| *count > 0)
        .unwrap_or(0)
}

#[cfg(test)]
pub(super) fn build_online_notification_message_with_device_name(
    provider: &ImProviderConfig,
    device_name: &str,
) -> String {
    build_online_notification_message_with_context(provider, device_name, None)
}

pub(super) fn build_online_notification_message_with_context(
    provider: &ImProviderConfig,
    device_name: &str,
    agent_context: Option<&OnlineNotificationAgentContext>,
) -> String {
    let device_name = clean_device_name(device_name).unwrap_or_else(|| "unknown".to_string());
    let work_dir = online_notification_work_dir(provider);
    let runner_type = agent_context
        .map(|context| context.runner_type.as_str())
        .unwrap_or("external");
    let runner_id = agent_context
        .and_then(|context| context.runner_id.as_deref())
        .unwrap_or("N/A");
    let session_key = agent_context
        .map(|context| context.session_key.as_str())
        .unwrap_or("N/A");
    let model_text = agent_context
        .map(|context| {
            bifrost_agent::format_model_ref(
                context.model.as_deref(),
                context.model_provider.as_deref(),
            )
        })
        .unwrap_or_else(|| "N/A".to_string());
    let reasoning_effort_text = bifrost_agent::format_optional_status_text(
        agent_context.and_then(|context| context.model_reasoning_effort.as_deref()),
    );
    let reasoning_summary_text = bifrost_agent::format_optional_status_text(
        agent_context.and_then(|context| context.model_reasoning_summary.as_deref()),
    );
    let user_turn_count = agent_context
        .map(|context| context.user_turn_count)
        .unwrap_or(0);

    let mut message = format!(
        "**Bifrost is online**\n\n- **Provider**: {} (`{}`)\n- **Device**: {device_name}\n- **Workspace**: `{work_dir}`\n- **Runner Type**: `{runner_type}`\n- **Runner ID**: `{runner_id}`\n- **Model**: `{model_text}`\n- **Reasoning Effort**: `{reasoning_effort_text}`\n- **Reasoning Summary**: `{reasoning_summary_text}`\n- **Bound Session**: `{session_key}`\n- **Completed User Turns**: {user_turn_count}\n- **Status**: Ready",
        provider.display_name, provider.id
    );
    let runner_kind = agent_context
        .map(|context| ImHelpRunnerKind::from_runner_type(&context.runner_type))
        .unwrap_or_else(|| ImHelpRunnerKind::External {
            adapter: "codex".to_string(),
        });
    message.push_str("\n\n");
    message.push_str(&build_im_startup_help_for_runner(&runner_kind));
    message
}

pub(super) fn outbound_log_msg_type(
    provider: &ImProviderConfig,
    requested_msg_type: &str,
) -> String {
    if provider.provider_type == ImProviderType::Feishu && requested_msg_type == "text" {
        "interactive".to_string()
    } else {
        requested_msg_type.to_string()
    }
}

/// Build a Feishu Card 2.0 JSON for real-time plan progress display.
///
/// Used by the plan listener task: first call creates a new card via send_card,
/// subsequent calls update the same card via patch_card.
/// Shows detailed status if session exists, otherwise shows a "new session" placeholder.
pub(super) fn build_im_status_text(
    detail: Option<&SessionDetail>,
    context: &bifrost_agent::StatusRuntimeContext,
    default_work_dir: Option<&str>,
) -> String {
    match detail {
        Some(d) => {
            let real = d
                .total_tokens_used
                .map(bifrost_agent::format_status_metric_count)
                .unwrap_or_else(|| "N/A".to_string());
            let agent_type = d
                .agent_type
                .as_ref()
                .or(context.agent_type.as_ref())
                .map(String::as_str)
                .unwrap_or("External Runner Agent");
            let runner_type = d
                .runner_type
                .as_ref()
                .or(context.runner_type.as_ref())
                .map(String::as_str)
                .unwrap_or("external");
            let runner_id = d
                .runner_id
                .as_ref()
                .or(context.runner_id.as_ref())
                .map(String::as_str)
                .unwrap_or("N/A");
            let model_text = bifrost_agent::format_model_ref(
                d.model
                    .as_ref()
                    .or(context.model.as_ref())
                    .map(String::as_str),
                d.model_provider
                    .as_ref()
                    .or(context.model_provider.as_ref())
                    .map(String::as_str),
            );
            let reasoning_effort_text = bifrost_agent::format_optional_status_text(
                d.model_reasoning_effort
                    .as_ref()
                    .or(context.model_reasoning_effort.as_ref())
                    .map(String::as_str),
            );
            let reasoning_summary_text = bifrost_agent::format_optional_status_text(
                d.model_reasoning_summary
                    .as_ref()
                    .or(context.model_reasoning_summary.as_ref())
                    .map(String::as_str),
            );
            let conversation_ref = bifrost_agent::format_conversation_ref(
                d.external_thread_id
                    .as_ref()
                    .or(context.external_thread_id.as_ref())
                    .map(String::as_str),
                d.external_conversation_id
                    .as_ref()
                    .or(context.external_conversation_id.as_ref())
                    .map(String::as_str),
            );
            let goal_info = match (&d.goal_status, &d.goal_objective) {
                (Some(status), Some(objective)) => {
                    let obj_preview = truncate_str(objective, 80);
                    format!("\n- 目标状态: {status}\n- 目标: {obj_preview}")
                }
                _ => String::new(),
            };
            let work_dir = d.work_dir.as_deref().or(default_work_dir).unwrap_or("N/A");
            let context_management_text =
                bifrost_agent::format_context_management_status(d.message_count);
            format!(
                "会话状态:\n- 工作路径: {}\n- Agent 类型: {}\n- Runner 类型: {}\n- Runner ID: {}\n- 模型: {}\n- 思考强度: {}\n- 思考摘要: {}\n- 外部会话: {}\n- 历史对话轮次: {}\n- 消息数: {}\n- 估算 token: ~{}\n- API 累计 token: {}\n- 显式压缩次数: {}\n- 上下文管理: {}\n- 历史版本: {}\n- 状态: 空闲{}",
                work_dir,
                agent_type,
                runner_type,
                runner_id,
                model_text,
                reasoning_effort_text,
                reasoning_summary_text,
                conversation_ref,
                d.user_turn_count,
                d.message_count,
                bifrost_agent::format_status_metric_count(d.estimated_tokens.into()),
                real,
                d.compaction_count,
                context_management_text,
                d.history_version,
                goal_info
            )
        }
        None => {
            "会话状态:\n- 消息数: 0\n- 状态: 新会话\n\n提示: 发送消息即可开始对话。".to_string()
        }
    }
}

pub(super) fn status_context_from_agent_runner(
    runner: Option<&bifrost_agent::AgentRunnerMode>,
) -> bifrost_agent::StatusRuntimeContext {
    match runner {
        Some(bifrost_agent::AgentRunnerMode::Custom(runner_id)) => {
            status_context_from_external_runner(runner_id, runner_id)
        }
        _ => status_context_from_external_runner("Codex", "codex"),
    }
}

pub(super) fn status_context_from_agent_config(
    config: &bifrost_agent::config::AgentConfig,
) -> bifrost_agent::StatusRuntimeContext {
    let mut context = status_context_from_agent_runner(config.runner.as_ref());
    context.model = config.model.clone();
    context.model_provider = config.model_provider.clone();
    context.model_reasoning_effort = config.model_reasoning_effort.clone();
    context.model_reasoning_summary = config.model_reasoning_summary.clone();
    context
}

pub(super) fn status_context_from_external_runner(
    runner_id: &str,
    adapter: &str,
) -> bifrost_agent::StatusRuntimeContext {
    bifrost_agent::StatusRuntimeContext {
        agent_type: Some("External Runner Agent".to_string()),
        runner_type: Some(adapter.to_string()),
        runner_id: Some(runner_id.to_string()),
        model: None,
        model_provider: None,
        model_reasoning_effort: None,
        model_reasoning_summary: None,
        external_conversation_id: None,
        external_thread_id: None,
    }
}

pub(super) fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{}...", truncated)
    }
}

/// Extract a diagnostic screenshot path from an error string.
///
/// The send layer appends `\n[diagnostic_screenshot:/path/to/file.png]` to errors
/// when a screenshot was captured. This function splits the marker out and returns
/// the clean error string plus an optional PathBuf.
pub(super) fn extract_diagnostic_screenshot_path(error: &str) -> (String, Option<PathBuf>) {
    const MARKER: &str = "[diagnostic_screenshot:";
    if let Some(idx) = error.find(MARKER) {
        let before = error[..idx].trim_end().to_string();
        let after = &error[idx + MARKER.len()..];
        let path_str = after.trim_end_matches(']').trim();
        if path_str.is_empty() {
            (before, None)
        } else {
            (before, Some(PathBuf::from(path_str)))
        }
    } else {
        (error.to_string(), None)
    }
}

/// Parse URL query string into key-value pairs.
pub(super) fn parse_query_params(query: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("").to_string();
        let val = parts.next().unwrap_or("").to_string();
        if !key.is_empty() {
            map.insert(key, val);
        }
    }
    map
}

pub(super) fn build_content_preview(msg_type: &str, content: &serde_json::Value) -> Option<String> {
    match msg_type {
        "text" => {
            let text = content.as_str().unwrap_or_default();
            Some(truncate_str(text, 200))
        }
        "interactive" => {
            // Try to extract header title from card JSON
            let title = content
                .get("header")
                .and_then(|h| h.get("title"))
                .and_then(|t| t.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("[card]");
            Some(truncate_str(title, 200))
        }
        "image" => content
            .get("image_key")
            .and_then(|value| value.as_str())
            .map(|image_key| truncate_str(&format!("[image:{image_key}]"), 200))
            .or_else(|| Some("[image]".to_string())),
        _ => Some(format!("[{}]", msg_type)),
    }
}

/// Sanitize provider config for API response: never expose secret_ref in plaintext.
pub(super) fn sanitize_provider(provider: &ImProviderConfig) -> serde_json::Value {
    serde_json::json!({
        "id": provider.id,
        "provider_type": provider.provider_type,
        "display_name": provider.display_name,
        "enabled": provider.enabled,
        "base_url": provider.base_url,
        "app_id": provider.app_id,
        "secret_configured": provider.secret_ref.is_some(),
        "owner_open_id": provider.owner_open_id,
        "event_connection_enabled": provider.event_connection_enabled,
        "event_types": provider.event_types,
        "agent_config": provider.agent_config,
        "created_at": provider.created_at,
        "updated_at": provider.updated_at,
    })
}

pub(super) fn parse_provider_create_payload(
    mut payload: serde_json::Value,
) -> std::result::Result<ImProviderConfig, serde_json::Error> {
    let fallback_display_name = payload
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|id| !id.trim().is_empty())
        .map(|id| id.to_string());
    let display_name_missing = payload
        .get("display_name")
        .and_then(|v| v.as_str())
        .is_none_or(|name| name.trim().is_empty());
    if display_name_missing {
        if let Some(display_name) = fallback_display_name {
            payload["display_name"] = serde_json::Value::String(display_name);
        }
    }
    if let Some(secret) = payload.get("app_secret").and_then(|v| v.as_str()) {
        if payload
            .get("secret_ref")
            .is_none_or(|existing| existing.is_null())
        {
            payload["secret_ref"] = serde_json::Value::String(secret.to_string());
        }
    }
    if let Some(obj) = payload.as_object_mut() {
        obj.remove("app_secret");
    }
    let mut config = serde_json::from_value(payload)?;
    normalize_provider_base_url(&mut config);
    Ok(config)
}

pub(super) fn apply_provider_patch(provider: &mut ImProviderConfig, patch: &serde_json::Value) {
    if let Some(name) = patch.get("display_name").and_then(|v| v.as_str()) {
        provider.display_name = name.to_string();
    }
    if let Some(enabled) = patch.get("enabled").and_then(|v| v.as_bool()) {
        provider.enabled = enabled;
    }
    if let Some(url) = patch.get("base_url").and_then(|v| v.as_str()) {
        provider.base_url = Some(url.to_string());
    }
    if let Some(app_id) = patch.get("app_id").and_then(|v| v.as_str()) {
        provider.app_id = Some(app_id.to_string());
    }
    if let Some(secret) = patch.get("app_secret").and_then(|v| v.as_str()) {
        provider.secret_ref = Some(secret.to_string());
    }
    if let Some(conn) = patch
        .get("event_connection_enabled")
        .and_then(|v| v.as_bool())
    {
        provider.event_connection_enabled = conn;
    }
    if let Some(types) = patch.get("event_types").and_then(|v| v.as_array()) {
        provider.event_types = types
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(owner) = patch.get("owner_open_id").and_then(|v| v.as_str()) {
        provider.owner_open_id = Some(owner.to_string());
    }
    if let Some(agent_config_value) = patch.get("agent_config") {
        if agent_config_value.is_null() {
            provider.agent_config = None;
        } else if let Some(agent_config_obj) = agent_config_value.as_object() {
            let agent_config = provider.agent_config.get_or_insert(ImProviderAgentConfig {
                runner: None,
                work_dir: None,
                base_instructions: None,
                developer_instructions: None,
                user_instructions: None,
            });
            if let Some(runner_value) = agent_config_obj.get("runner") {
                if runner_value.is_null() {
                    agent_config.runner = None;
                } else if let Ok(runner) =
                    serde_json::from_value::<bifrost_agent::AgentRunnerMode>(runner_value.clone())
                {
                    agent_config.runner = Some(runner);
                }
            }
            if let Some(work_dir_value) = agent_config_obj.get("work_dir") {
                if work_dir_value.is_null() {
                    agent_config.work_dir = None;
                } else if let Some(work_dir) = work_dir_value.as_str() {
                    let work_dir = work_dir.trim();
                    agent_config.work_dir = (!work_dir.is_empty()).then(|| work_dir.to_string());
                }
            }
            apply_provider_agent_config_string(
                &mut agent_config.base_instructions,
                agent_config_obj.get("base_instructions"),
            );
            apply_provider_agent_config_string(
                &mut agent_config.developer_instructions,
                agent_config_obj.get("developer_instructions"),
            );
            apply_provider_agent_config_string(
                &mut agent_config.user_instructions,
                agent_config_obj.get("user_instructions"),
            );
            if agent_config.work_dir.is_none()
                && agent_config.runner.is_none()
                && agent_config.base_instructions.is_none()
                && agent_config.developer_instructions.is_none()
                && agent_config.user_instructions.is_none()
            {
                provider.agent_config = None;
            }
        }
    }
    normalize_provider_base_url(provider);
    normalize_provider_agent_config(provider);
    provider.updated_at = now_ms();
}

pub(super) fn persist_provider_agent_work_dir(
    provider_store: &Arc<ImProviderStore>,
    provider_id: &str,
    work_dir: &str,
) {
    let work_dir = work_dir.trim();
    if work_dir.is_empty() {
        return;
    }

    let Some(mut provider) = provider_store.get(provider_id) else {
        warn!(
            provider_id = %provider_id,
            work_dir = %work_dir,
            "failed to persist switched work_dir because provider was not found"
        );
        return;
    };

    let agent_config = provider.agent_config.get_or_insert(ImProviderAgentConfig {
        runner: None,
        work_dir: None,
        base_instructions: None,
        developer_instructions: None,
        user_instructions: None,
    });
    if agent_config.work_dir.as_deref() == Some(work_dir) {
        return;
    }
    agent_config.work_dir = Some(work_dir.to_string());
    normalize_provider_agent_config(&mut provider);
    if let Err(error) = provider_store.update(provider) {
        warn!(
            provider_id = %provider_id,
            work_dir = %work_dir,
            error = %error,
            "failed to persist switched provider work_dir"
        );
    }
}

pub(super) fn persist_provider_agent_runner(
    provider_store: &Arc<ImProviderStore>,
    provider_id: &str,
    runner: bifrost_agent::AgentRunnerMode,
) {
    let Some(mut provider) = provider_store.get(provider_id) else {
        warn!(
            provider_id = %provider_id,
            runner = ?runner,
            "failed to persist switched runner because provider was not found"
        );
        return;
    };

    let agent_config = provider.agent_config.get_or_insert(ImProviderAgentConfig {
        runner: None,
        work_dir: None,
        base_instructions: None,
        developer_instructions: None,
        user_instructions: None,
    });
    if agent_config.runner.as_ref() == Some(&runner) {
        return;
    }
    agent_config.runner = Some(runner);
    normalize_provider_agent_config(&mut provider);
    if let Err(error) = provider_store.update(provider) {
        warn!(
            provider_id = %provider_id,
            error = %error,
            "failed to persist switched provider runner"
        );
    }
}

pub(super) fn normalize_provider_agent_config(provider: &mut ImProviderConfig) {
    let Some(agent_config) = provider.agent_config.as_mut() else {
        return;
    };
    agent_config.work_dir = agent_config
        .work_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    agent_config.base_instructions = agent_config
        .base_instructions
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    agent_config.developer_instructions = agent_config
        .developer_instructions
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    agent_config.user_instructions = agent_config
        .user_instructions
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    if agent_config.work_dir.is_none()
        && agent_config.runner.is_none()
        && agent_config.base_instructions.is_none()
        && agent_config.developer_instructions.is_none()
        && agent_config.user_instructions.is_none()
    {
        provider.agent_config = None;
    }
}

pub(super) fn apply_provider_agent_config_string(
    target: &mut Option<String>,
    value: Option<&serde_json::Value>,
) {
    let Some(value) = value else {
        return;
    };
    if value.is_null() {
        *target = None;
        return;
    }
    if let Some(text) = value.as_str() {
        let text = text.trim();
        *target = (!text.is_empty()).then(|| text.to_string());
    }
}

pub(super) fn apply_target_patch(target: &mut ImTarget, patch: &serde_json::Value) {
    if let Some(name) = patch.get("display_name").and_then(|v| v.as_str()) {
        target.display_name = name.to_string();
    }
    if let Some(rid_type) = patch.get("receive_id_type").and_then(|v| v.as_str()) {
        target.receive_id_type = rid_type.to_string();
    }
    if let Some(rid) = patch.get("receive_id").and_then(|v| v.as_str()) {
        target.receive_id = rid.to_string();
    }
    if let Some(msg_type) = patch.get("default_msg_type").and_then(|v| v.as_str()) {
        target.default_msg_type = msg_type.to_string();
    }
    if let Some(enabled) = patch.get("enabled").and_then(|v| v.as_bool()) {
        target.enabled = enabled;
    }
    target.updated_at = now_ms();
}

pub(super) fn apply_route_patch(route: &mut ImRoute, patch: &serde_json::Value) {
    if let Some(name) = patch.get("name").and_then(|v| v.as_str()) {
        route.name = name.to_string();
    }
    if let Some(enabled) = patch.get("enabled").and_then(|v| v.as_bool()) {
        route.enabled = enabled;
    }
    if let Some(timeout) = patch.get("timeout_ms").and_then(|v| v.as_u64()) {
        route.timeout_ms = timeout;
    }
    if let Some(max_output) = patch.get("max_output_bytes").and_then(|v| v.as_u64()) {
        route.max_output_bytes = max_output;
    }
    if let Some(matcher) = patch.get("matcher") {
        if let Ok(m) = serde_json::from_value(matcher.clone()) {
            route.matcher = m;
        }
    }
    if let Some(action) = patch.get("action") {
        if let Ok(a) = serde_json::from_value(action.clone()) {
            route.action = a;
        }
    }
    if let Some(event_type) = patch.get("event_type") {
        if let Ok(et) = serde_json::from_value(event_type.clone()) {
            route.event_type = et;
        }
    }
    route.updated_at = now_ms();
}

pub(super) fn apply_schedule_patch(schedule: &mut ImSchedule, patch: &serde_json::Value) {
    if let Some(name) = patch.get("name").and_then(|v| v.as_str()) {
        schedule.name = name.to_string();
    }
    if let Some(enabled) = patch.get("enabled").and_then(|v| v.as_bool()) {
        schedule.enabled = enabled;
    }
    if let Some(channel) = patch.get("message_channel") {
        if channel.is_null() {
            schedule.message_channel = None;
        } else if let Ok(binding) = serde_json::from_value(channel.clone()) {
            schedule.message_channel = Some(binding);
        }
    }
    if let Some(timeout) = patch.get("timeout_ms").and_then(|v| v.as_u64()) {
        schedule.timeout_ms = timeout;
    }
    if let Some(max_output) = patch.get("max_output_bytes").and_then(|v| v.as_u64()) {
        schedule.max_output_bytes = max_output;
    }
    if let Some(trigger) = patch.get("trigger") {
        if let Ok(t) = serde_json::from_value(trigger.clone()) {
            schedule.trigger = t;
        }
    }
    if let Some(task_type) = patch.get("task_type") {
        if let Ok(t) = serde_json::from_value(task_type.clone()) {
            schedule.task_type = t;
        }
    }
    if let Some(script) = patch.get("script") {
        if let Ok(s) = serde_json::from_value(script.clone()) {
            schedule.script = s;
        }
    }
    if let Some(agent) = patch.get("agent") {
        if agent.is_null() {
            schedule.agent = None;
        } else if let Some(existing_agent) = schedule.agent.as_mut() {
            apply_schedule_agent_patch(existing_agent, agent);
        } else if let Ok(a) = serde_json::from_value(agent.clone()) {
            schedule.agent = Some(a);
        }
    }
    if let Some(concurrency) = patch.get("concurrency_policy") {
        if let Ok(c) = serde_json::from_value(concurrency.clone()) {
            schedule.concurrency_policy = c;
        }
    }
    if let Some(retry) = patch.get("retry") {
        if let Ok(r) = serde_json::from_value(retry.clone()) {
            schedule.retry = r;
        }
    }
    schedule.updated_at = now_ms();
}

fn apply_schedule_agent_patch(
    agent: &mut crate::im_gateway::types::ScheduleAgentTask,
    patch: &serde_json::Value,
) {
    if let Some(prompt) = patch.get("prompt").and_then(|v| v.as_str()) {
        agent.prompt = prompt.to_string();
    }
    if patch.get("runner_id").is_some() {
        agent.runner_id = optional_string_patch(patch.get("runner_id"));
    }
    if patch.get("initial_prompt").is_some() {
        agent.initial_prompt = optional_string_patch(patch.get("initial_prompt"));
    }
    if patch.get("session_key").is_some() {
        agent.session_key = optional_string_patch(patch.get("session_key"));
    }
    if patch.get("work_dir").is_some() {
        agent.work_dir = optional_string_patch(patch.get("work_dir"));
    }
    if patch.get("system_prompt").is_some() {
        agent.system_prompt = optional_string_patch(patch.get("system_prompt"));
    }
    if let Some(adapter_config) = patch.get("adapter_config") {
        if adapter_config.is_null() {
            agent.adapter_config = None;
        } else if let Ok(config) =
            merge_schedule_adapter_config_patch(agent.adapter_config.as_ref(), adapter_config)
        {
            agent.adapter_config = Some(config);
        }
    }
    if let Some(conversation_ref) = patch.get("conversation_ref") {
        if conversation_ref.is_null() {
            agent.conversation_ref = None;
        } else if let Ok(ref_value) = serde_json::from_value(conversation_ref.clone()) {
            agent.conversation_ref = Some(ref_value);
        }
    }
}

fn optional_string_patch(value: Option<&serde_json::Value>) -> Option<String> {
    value.and_then(|value| {
        if value.is_null() {
            None
        } else {
            value.as_str().map(ToString::to_string)
        }
    })
}

fn merge_schedule_adapter_config_patch(
    existing: Option<&crate::im_gateway::external_cli::ExternalCliAdapterConfig>,
    patch: &serde_json::Value,
) -> Result<crate::im_gateway::external_cli::ExternalCliAdapterConfig, serde_json::Error> {
    let mut merged = existing
        .map(serde_json::to_value)
        .transpose()?
        .unwrap_or_else(|| serde_json::json!({}));
    merge_json_object(&mut merged, patch);
    serde_json::from_value(merged)
}

fn merge_json_object(base: &mut serde_json::Value, patch: &serde_json::Value) {
    let Some(base_object) = base.as_object_mut() else {
        *base = serde_json::json!({});
        merge_json_object(base, patch);
        return;
    };
    let Some(patch_object) = patch.as_object() else {
        return;
    };
    for (key, value) in patch_object {
        base_object.insert(key.clone(), value.clone());
    }
}

pub(super) fn restore_session_from_persisted_history(
    session: &mut bifrost_agent::AgentSession,
    session_key: &str,
    adapter: &str,
    runner_id: Option<&str>,
    max_bytes: Option<usize>,
) -> Option<PathBuf> {
    if !session.history.is_empty() {
        return None;
    }

    let state =
        crate::im_gateway::session_state::load_session_state(session_key, adapter, runner_id)
            .or_else(|| {
                if runner_id.is_none() {
                    crate::im_gateway::session_state::load_latest_session_state(
                        session_key,
                        Some(adapter),
                        None,
                    )
                } else {
                    None
                }
            });
    let state_history_path = state
        .as_ref()
        .and_then(|state| state.history_path.as_deref())
        .map(PathBuf::from);
    let candidates = state_history_path
        .into_iter()
        .chain(if runner_id.is_none() {
            latest_history_path_for_session(session_key)
        } else {
            None
        })
        .collect::<Vec<_>>();

    for candidate in candidates {
        match restore_session_from_history_path(session, &candidate, session_key, max_bytes) {
            Ok(path) => {
                if let Some(state) = state.as_ref() {
                    session.remember_external_conversation_ref(
                        state.external_conversation_id.clone(),
                        state.external_thread_id.clone(),
                    );
                    if session.work_dir.is_none() {
                        session.work_dir = state.work_dir.clone();
                    }
                }
                return Some(path);
            }
            Err(error) => {
                warn!(
                    session_key = %session_key,
                    history_path = %candidate.display(),
                    error = %error,
                    "failed to restore persisted IM agent session history"
                );
            }
        }
    }
    None
}

pub(super) fn remember_session_state_from_agent_session(
    session: &bifrost_agent::AgentSession,
    adapter: &str,
    runner_id: Option<&str>,
) {
    let history_path = session
        .recorder
        .as_ref()
        .map(|recorder| recorder.file_path().display().to_string());
    if history_path.is_none()
        && session.external_conversation_id.is_none()
        && session.external_thread_id.is_none()
    {
        return;
    }
    if let Err(error) = crate::im_gateway::session_state::upsert_session_state(
        &session.session_key,
        adapter,
        runner_id,
        |state| {
            state.external_conversation_id = session.external_conversation_id.clone();
            state.external_thread_id = session.external_thread_id.clone();
            if let Some(history_path) = history_path.clone() {
                state.history_path = Some(history_path);
            }
            state.work_dir = session.work_dir.clone();
            state.title = session
                .title
                .clone()
                .or_else(|| latest_role_content(session, "user"));
            state.last_user_message = latest_role_content(session, "user");
            state.last_response = latest_role_content(session, "assistant");
            state.status = Some("ended".to_string());
        },
    ) {
        warn!(
            session_key = %session.session_key,
            adapter = %adapter,
            error = %error,
            "failed to persist IM agent session state"
        );
    }
}

fn latest_role_content(session: &bifrost_agent::AgentSession, role: &str) -> Option<String> {
    session
        .history
        .iter()
        .rev()
        .find(|message| message.role == role)
        .and_then(|message| message.content.as_deref())
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .map(ToString::to_string)
}

pub(super) fn remember_session_state_values(
    session_key: &str,
    adapter: &str,
    runner_id: Option<&str>,
    external_conversation_id: Option<String>,
    external_thread_id: Option<String>,
    history_path: Option<String>,
    work_dir: Option<String>,
) {
    if history_path.is_none() && external_conversation_id.is_none() && external_thread_id.is_none()
    {
        return;
    }
    if let Err(error) = crate::im_gateway::session_state::upsert_session_state(
        session_key,
        adapter,
        runner_id,
        |state| {
            if external_conversation_id.is_some() {
                state.external_conversation_id = external_conversation_id;
            }
            if external_thread_id.is_some() {
                state.external_thread_id = external_thread_id;
            }
            if history_path.is_some() {
                state.history_path = history_path;
            }
            if work_dir.is_some() {
                state.work_dir = work_dir;
            }
        },
    ) {
        warn!(
            session_key = %session_key,
            adapter = %adapter,
            error = %error,
            "failed to persist IM agent session state values"
        );
    }
}

pub(super) fn clear_persisted_agent_session_state(
    session_key: &str,
    adapter: Option<&str>,
    runner_id: Option<&str>,
) {
    let removed_states = match crate::im_gateway::session_state::clear_session_state_scope(
        session_key,
        adapter,
        runner_id,
    ) {
        Ok(states) => states,
        Err(error) => {
            warn!(
                session_key = %session_key,
                adapter = ?adapter,
                runner_id = ?runner_id,
                error = %error,
                "failed to clear persisted IM agent session state"
            );
            Vec::new()
        }
    };
    match clear_persisted_session_histories(session_key, adapter, &removed_states) {
        Ok(removed) if removed > 0 => {
            info!(
                session_key = %session_key,
                adapter = ?adapter,
                runner_id = ?runner_id,
                removed,
                "cleared persisted IM agent session histories"
            );
        }
        Ok(_) => {}
        Err(error) => {
            warn!(
                session_key = %session_key,
                adapter = ?adapter,
                runner_id = ?runner_id,
                error = %error,
                "failed to clear persisted IM agent session histories"
            );
        }
    }
}

fn latest_history_path_for_session(session_key: &str) -> Option<PathBuf> {
    let data_dir = bifrost_agent::config::agent_home_dir();
    let mut candidates =
        bifrost_agent::persistence::list_conversations(&data_dir, Some(session_key));
    candidates.sort_by_key(|path| {
        let summary = bifrost_agent::persistence::scan_session_summary(path);
        if summary.end_time > 0 {
            summary.end_time
        } else {
            summary.start_time
        }
    });
    candidates.pop()
}

fn clear_persisted_session_histories(
    session_key: &str,
    adapter: Option<&str>,
    removed_states: &[crate::im_gateway::session_state::ImAgentSessionState],
) -> Result<usize, String> {
    let session_key = session_key.trim();
    if session_key.is_empty() {
        return Ok(0);
    }
    let data_dir = bifrost_agent::config::agent_home_dir();
    let candidates: Vec<PathBuf> = if adapter.is_none() {
        bifrost_agent::persistence::list_conversations(&data_dir, Some(session_key))
    } else {
        removed_states
            .iter()
            .filter_map(|state| state.history_path.as_deref())
            .map(PathBuf::from)
            .collect()
    };
    let mut removed = 0;
    for candidate in candidates {
        let validated =
            match bifrost_agent::persistence::validate_conversation_path(&data_dir, &candidate) {
                Ok(path) => path,
                Err(error) => {
                    warn!(
                        session_key = %session_key,
                        history_path = %candidate.display(),
                        error = %error,
                        "skipping invalid persisted IM agent session history during clear"
                    );
                    continue;
                }
            };
        match std::fs::remove_file(&validated) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "remove session history {} failed: {error}",
                    validated.display()
                ));
            }
        }
    }
    Ok(removed)
}

pub(super) fn restore_session_from_history_path(
    session: &mut bifrost_agent::AgentSession,
    history_path: &Path,
    expected_session_key: &str,
    max_bytes: Option<usize>,
) -> Result<PathBuf, String> {
    let data_dir = bifrost_agent::config::agent_home_dir();
    let path = bifrost_agent::persistence::validate_conversation_path(&data_dir, history_path)?;
    let report = bifrost_agent::persistence::load_conversation_lossy(&path)?;
    if let Some(restored_key) = report.session_key.as_deref() {
        if restored_key != expected_session_key {
            return Err("history session_key does not match the requested session_key".to_string());
        }
    }
    if report.messages.is_empty() {
        return Err("history file does not contain restorable chat messages".to_string());
    }

    let summary = bifrost_agent::persistence::scan_session_summary(&path);
    session.history = report.messages;
    session.history_version = session.history_version.saturating_add(1);
    session.last_response_tokens = None;
    session.last_response_history_len = None;
    session.history_cleared = false;
    session.title = summary.title;
    if session.work_dir.is_none() {
        session.work_dir = summary.work_dir;
    }
    if !summary.source.is_empty() {
        session.source = summary.source;
    }
    match bifrost_agent::persistence::load_session_runtime_state(&path) {
        Ok(runtime_state) => {
            session.current_goal = runtime_state.current_goal;
            session.total_tokens_used = runtime_state.total_tokens_used;
            session.restore_token_snapshot(runtime_state.last_response_tokens);
            session.compaction_count = runtime_state.compaction_count;
            bifrost_agent::tools::goal::goal_runtime_apply(
                session,
                bifrost_agent::tools::goal::GoalRuntimeEvent::ThreadResumed,
            );
        }
        Err(error) => {
            warn!(history_path = %path.display(), error = %error, "restored IM agent history without runtime state");
            session.total_tokens_used = Some(summary.total_tokens);
            session.compaction_count = summary.compaction_count;
        }
    }
    if report.skipped_lines > 0 {
        warn!(
            history_path = %path.display(),
            skipped_lines = report.skipped_lines,
            "restored IM agent history with skipped JSONL lines"
        );
    }
    session.recorder = Some(ConversationRecorder::from_existing_file(
        path.clone(),
        max_bytes,
    ));
    Ok(path)
}

/// Parse a session filename like `session-{key}-{timestamp}.jsonl`
/// into (session_key, timestamp).
pub(super) fn parse_session_filename(filename: &str) -> (String, u64) {
    let name = filename.strip_suffix(".jsonl").unwrap_or(filename);
    let name = name.strip_prefix("session-").unwrap_or(name);
    // Last segment after '-' is the timestamp
    if let Some(last_dash) = name.rfind('-') {
        let key = &name[..last_dash];
        let ts = name[last_dash + 1..].parse::<u64>().unwrap_or(0);
        (key.to_string(), ts)
    } else {
        (name.to_string(), 0)
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::resolve_attachment_path;

    #[test]
    fn attachment_path_resolution_stays_inside_attachment_root() {
        assert!(resolve_attachment_path("chatgpt_web/run/image.png").is_some());
        assert!(resolve_attachment_path("../secret.png").is_none());
        assert!(resolve_attachment_path("/tmp/secret.png").is_none());
    }
}
