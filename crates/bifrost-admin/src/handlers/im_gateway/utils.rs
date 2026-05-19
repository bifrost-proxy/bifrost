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

pub(super) fn build_online_notification_message(provider: &ImProviderConfig) -> String {
    let work_dir = provider
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
        .unwrap_or_else(|| "unknown".to_string());

    format!("你好，Bifrost 助手上线了\n工作目录：{work_dir}")
}

/// Build a Feishu Card 2.0 JSON for real-time plan progress display.
///
/// Used by the plan listener task: first call creates a new card via send_card,
/// subsequent calls update the same card via patch_card.
pub(super) fn build_plan_card(
    steps: &[bifrost_agent::PlanStep],
    session_title: Option<&str>,
) -> serde_json::Value {
    let completed = steps
        .iter()
        .filter(|s| matches!(s.status, bifrost_agent::PlanStepStatus::Completed))
        .count();
    let total = steps.len();

    let mut plan_md = String::new();
    for s in steps {
        plan_md.push_str(&format!("{} {}\n", s.status.emoji(), s.step));
    }

    let title = session_title.unwrap_or("Bifrost AI");

    serde_json::json!({
        "schema": "2.0",
        "config": {
            "width_mode": "fill",
            "update_multi": true
        },
        "header": {
            "template": "turquoise",
            "title": {
                "tag": "plain_text",
                "content": title
            },
            "subtitle": {
                "tag": "plain_text",
                "content": format!("📋 任务计划（{}/{}）", completed, total)
            }
        },
        "body": {
            "elements": [{
                "tag": "markdown",
                "content": plan_md
            }]
        }
    })
}

/// Build a status text for IM display.
/// Shows detailed status if session exists, otherwise shows a "new session" placeholder.
pub(super) fn build_im_status_text(detail: Option<&SessionDetail>) -> String {
    match detail {
        Some(d) => {
            let real = d
                .total_tokens_used
                .map(|t| t.to_string())
                .unwrap_or_else(|| "N/A".to_string());
            let goal_info = match (&d.goal_status, &d.goal_objective) {
                (Some(status), Some(objective)) => {
                    let obj_preview = truncate_str(objective, 80);
                    format!("\n- 目标状态: {status}\n- 目标: {obj_preview}")
                }
                _ => String::new(),
            };
            let work_dir = d.work_dir.as_deref().unwrap_or("N/A");
            format!(
                "会话状态:\n- 工作路径: {}\n- 消息数: {}\n- 估算 token: ~{}\n- API 累计 token: {}\n- 压缩次数: {}\n- 历史版本: {}\n- 状态: 空闲{}",
                work_dir,
                d.message_count,
                d.estimated_tokens,
                real,
                d.compaction_count,
                d.history_version,
                goal_info
            )
        }
        None => {
            "会话状态:\n- 消息数: 0\n- 状态: 新会话\n\n提示: 发送消息即可开始对话。".to_string()
        }
    }
}

pub(super) fn build_agent_api_status_text(
    detail: Option<&SessionDetail>,
    config: &bifrost_agent::config::AgentConfig,
) -> String {
    match detail {
        Some(d) => {
            let real = d
                .total_tokens_used
                .map(|t| t.to_string())
                .unwrap_or_else(|| "N/A".to_string());
            let context_window = config
                .model_context_window
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0)
                .unwrap_or(bifrost_agent::config::AgentConfig::DEFAULT_MODEL_CONTEXT_WINDOW as u32);
            let context_percent =
                ((d.estimated_tokens as f64 / context_window as f64) * 1000.0).round() / 10.0;
            let work_dir = d.work_dir.as_deref().unwrap_or("N/A");
            format!(
                "会话状态:\n- 工作路径: {}\n- 消息数: {}\n- 估算 token: ~{}\n- API 累计 token: {}\n- Context 用量: ~{} / {} ({:.1}%)\n- 压缩次数: {}\n- 历史版本: {}\n- MCP 工具数: 0",
                work_dir,
                d.message_count,
                d.estimated_tokens,
                real,
                d.estimated_tokens,
                context_window,
                context_percent,
                d.compaction_count,
                d.history_version,
            )
        }
        None => {
            "会话状态:\n- 消息数: 0\n- 状态: 新会话\n\n提示: 发送消息即可开始对话。".to_string()
        }
    }
}

pub(super) fn resolve_agent_api_status_detail(
    manager: &bifrost_agent::AgentSessionManager,
    session_key: &str,
    requested_work_dir: Option<String>,
) -> Option<SessionDetail> {
    let has_requested_work_dir = requested_work_dir
        .as_deref()
        .is_some_and(|work_dir| !work_dir.trim().is_empty());
    if !has_requested_work_dir && manager.get_session_detail(session_key).is_none() {
        return None;
    }

    match manager.try_take_session_with_work_dir(session_key, requested_work_dir) {
        Some(session) => {
            manager.return_session(session);
            manager.get_session_detail(session_key)
        }
        None => manager.get_session_detail(session_key),
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
    serde_json::from_value(payload)
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
