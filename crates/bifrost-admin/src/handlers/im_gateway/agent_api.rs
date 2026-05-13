use super::*;

// ---------------------------------------------------------------------------

pub(super) async fn handle_agent(
    req: Request<Incoming>,
    service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let rest = rest.trim_end_matches('/');

    // GET /agent  |  PATCH /agent
    if rest.is_empty() {
        return match *req.method() {
            Method::GET => {
                let config = service.agent_config_store.load();
                json_response(&agent_config_response(config))
            }
            Method::PATCH => {
                let patch: serde_json::Value = match read_body_json(req).await {
                    Ok(v) => v,
                    Err(resp) => return resp,
                };
                let mut config = service.agent_config_store.load();
                apply_agent_config_patch(&mut config, &patch);
                match service.agent_config_store.save(&config) {
                    Ok(()) => json_response(&agent_config_response(config)),
                    Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
                }
            }
            _ => method_not_allowed(),
        };
    }

    // GET /agent/mcp-status — check configured MCP server availability
    if rest == "/mcp-status" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        let config = service.agent_config_store.load();
        let statuses = bifrost_agent::mcp::check_server_availability(&config.mcp_servers).await;
        return json_response(&serde_json::json!({ "servers": statuses }));
    }

    // GET /agent/providers — list all built-in model providers
    if rest == "/providers" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        let providers = bifrost_agent::list_builtin_providers();
        return json_response(&providers);
    }

    // GET /agent/tools — list all built-in agent tools
    if rest == "/tools" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        let tools = service.agent_tools.definitions();
        return json_response(&serde_json::json!({ "tools": tools }));
    }

    if let Some(skills_rest) = rest.strip_prefix("/skills") {
        return crate::handlers::agent_skills::handle_agent_skills(req, service, skills_rest).await;
    }

    // GET /agent/instructions — show loaded AGENTS.md sources
    if rest == "/instructions" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        let config = service.agent_config_store.load();
        let work_dir = config.resolve_work_dir();
        let home_dir = bifrost_agent::config::agent_home_dir();
        let agents_md_manager = bifrost_agent::agents_md::AgentsMdManager::new(&config);
        let content = agents_md_manager.user_instructions(
            &work_dir,
            Some(&home_dir),
            config.user_instructions.as_deref(),
        );
        return json_response(&serde_json::json!({
            "content": content,
            "work_dir": work_dir.display().to_string(),
        }));
    }

    // GET /agent/sessions
    if rest == "/sessions" {
        if req.method() == Method::GET {
            let sessions = service.agent_session_manager.list_sessions();
            return json_response(&serde_json::json!({ "sessions": sessions }));
        }
        if req.method() == Method::DELETE {
            service.agent_session_manager.clear_all_sessions();
            return json_response(
                &serde_json::json!({ "ok": true, "message": "all sessions cleared" }),
            );
        }
        return method_not_allowed();
    }

    // GET /agent/sessions/all — unified list of active + history sessions
    if rest == "/sessions/all" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        let active_sessions = service.agent_session_manager.list_sessions();
        let active_keys: std::collections::HashSet<String> = active_sessions
            .iter()
            .map(|s| s.session_key.clone())
            .collect();

        let data_dir = bifrost_agent::config::agent_home_dir();
        let files = bifrost_agent::persistence::list_conversations(&data_dir, None);

        // Determine retention cutoff based on persistence mode
        let agent_config = service.agent_config_store.load();
        let cutoff_ts: u64 = match agent_config
            .history
            .as_ref()
            .map(|h| h.persistence)
            .unwrap_or_default()
        {
            bifrost_agent::HistoryPersistence::Last90Days => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                now.saturating_sub(90 * 24 * 3600)
            }
            _ => 0, // no cutoff
        };

        // Build unified list
        let mut unified: Vec<serde_json::Value> = Vec::new();

        // Add active sessions
        for s in active_sessions {
            let duration_secs = s.last_active_at.saturating_sub(s.created_at);
            unified.push(serde_json::json!({
                "session_key": s.session_key,
                "status": "active",
                "source": s.source,
                "work_dir": s.work_dir,
                "turns": s.message_count,
                "tokens": s.total_tokens_used,
                "start_time": s.created_at,
                "last_active_time": s.last_active_at,
                "duration_secs": duration_secs,
                "compaction_count": s.compaction_count,
                "estimated_tokens": s.estimated_tokens,
                "title": s.title,
            }));
        }

        // Add history sessions (excluding those already active or expired)
        for p in files {
            let filename = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let (parsed_key, _timestamp) = parse_session_filename(filename);
            let summary = bifrost_agent::persistence::scan_session_summary(&p);
            // Prefer the original session key from JSONL content (handles sanitized filenames)
            let session_key = summary
                .session_key
                .as_deref()
                .unwrap_or(&parsed_key)
                .to_string();
            if active_keys.contains(&session_key) {
                continue; // skip duplicate
            }
            // Skip sessions older than the retention cutoff
            let last_time = if summary.end_time > 0 {
                summary.end_time
            } else {
                summary.start_time
            };
            if cutoff_ts > 0 && last_time < cutoff_ts {
                continue;
            }
            unified.push(serde_json::json!({
                "session_key": session_key,
                "status": "ended",
                "source": summary.source,
                "work_dir": summary.work_dir,
                "turns": (summary.user_turns as usize) + (summary.assistant_turns as usize),
                "tokens": summary.total_tokens,
                "start_time": summary.start_time,
                "last_active_time": summary.end_time,
                "duration_secs": summary.end_time.saturating_sub(summary.start_time),
                "history_path": p.display().to_string(),
                "title": summary.title,
            }));
        }

        // Sort by last_active_time descending (newest first)
        unified.sort_by(|a, b| {
            let t_a = a
                .get("last_active_time")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let t_b = b
                .get("last_active_time")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            t_b.cmp(&t_a)
        });

        let active_count = active_keys.len();
        let history_count = unified.len() - active_count;

        return json_response(&serde_json::json!({
            "sessions": unified,
            "total": unified.len(),
            "active_count": active_count,
            "history_count": history_count,
        }));
    }

    // GET /agent/sessions/history — list persisted session files
    if rest == "/sessions/history" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        let data_dir = bifrost_agent::config::agent_home_dir();
        let files = bifrost_agent::persistence::list_conversations(&data_dir, None);
        let history: Vec<serde_json::Value> = files
            .iter()
            .map(|p| {
                let filename = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let (parsed_key, timestamp) = parse_session_filename(filename);
                let summary = bifrost_agent::persistence::scan_session_summary(p);
                let session_key = summary
                    .session_key
                    .as_deref()
                    .unwrap_or(&parsed_key)
                    .to_string();
                serde_json::json!({
                    "path": p.display().to_string(),
                    "filename": filename,
                    "session_key": session_key,
                    "timestamp": timestamp,
                    "total_tokens": summary.total_tokens,
                    "user_turns": summary.user_turns,
                    "assistant_turns": summary.assistant_turns,
                    "tool_calls": summary.tool_calls,
                    "event_count": summary.event_count,
                    "work_dir": summary.work_dir,
                    "source": summary.source,
                    "start_time": summary.start_time,
                    "end_time": summary.end_time,
                    "duration_secs": summary.end_time.saturating_sub(summary.start_time),
                })
            })
            .collect();
        return json_response(&serde_json::json!({ "history": history, "total": history.len() }));
    }

    // GET/DELETE /agent/sessions/history/* — load or delete a specific persisted session
    if let Some(file_path) = rest.strip_prefix("/sessions/history/") {
        let file_path = urlencoding::decode(file_path)
            .unwrap_or_default()
            .to_string();
        let path = std::path::Path::new(&file_path);
        if req.method() == Method::GET {
            // Return full events with all details (tool calls, results, metadata, etc.)
            match bifrost_agent::persistence::load_conversation_events(path) {
                Ok(events) => {
                    let event_values: Vec<serde_json::Value> = events
                        .iter()
                        .map(|e| {
                            serde_json::json!({
                                "timestamp": e.timestamp,
                                "event_type": e.event_type,
                                "session_key": e.session_key,
                                "content": e.content,
                            })
                        })
                        .collect();
                    return json_response(
                        &serde_json::json!({ "events": event_values, "count": event_values.len() }),
                    );
                }
                Err(e) => {
                    return error_response(
                        StatusCode::NOT_FOUND,
                        &format!("failed to load session: {e}"),
                    );
                }
            }
        }
        if req.method() == Method::DELETE {
            match std::fs::remove_file(path) {
                Ok(()) => return json_response(&serde_json::json!({ "ok": true })),
                Err(e) => {
                    return error_response(
                        StatusCode::NOT_FOUND,
                        &format!("failed to delete: {e}"),
                    );
                }
            }
        }
        return method_not_allowed();
    }

    // GET/DELETE /agent/sessions/:key
    if let Some(session_key) = rest.strip_prefix("/sessions/") {
        let session_key = urlencoding::decode(session_key)
            .unwrap_or_default()
            .to_string();
        if req.method() == Method::GET {
            match service
                .agent_session_manager
                .get_session_detail(&session_key)
            {
                Some(detail) => return json_response(&detail),
                None => {
                    return error_response(StatusCode::NOT_FOUND, "session not found");
                }
            }
        }
        if req.method() == Method::DELETE {
            service.agent_session_manager.clear_session(&session_key);
            return json_response(&serde_json::json!({ "ok": true }));
        }
        return method_not_allowed();
    }

    // POST /agent/chat — internal test endpoint (bypasses Feishu)
    if rest == "/chat" {
        if req.method() != Method::POST {
            return method_not_allowed();
        }
        #[derive(Deserialize)]
        struct ChatRequest {
            message: String,
            #[serde(default)]
            images: Vec<ChatImageRequest>,
            #[serde(default)]
            session_key: Option<String>,
            #[serde(default)]
            system_prompt: Option<String>,
            #[serde(default)]
            work_dir: Option<String>,
            /// Inject a guide message into guide_channel before starting the turn.
            /// Used to test the scenario where a user sends a message while the
            /// agent is finishing its turn (guide_channel drain at turn end).
            #[serde(default)]
            guide_message: Option<String>,
            /// Inject multiple guide messages into guide_channel before starting
            /// the turn. Used to verify pending guide status and merge behavior.
            #[serde(default)]
            guide_messages: Vec<String>,
            /// Inject messages into the pending queue before starting the turn.
            /// Used to test queued message processing. Each message will be
            /// processed sequentially within the same `run_turn_with_mcp` call.
            #[serde(default)]
            queue_messages: Vec<String>,
        }
        #[derive(Deserialize)]
        struct ChatImageRequest {
            #[serde(default = "default_chat_image_mime_type")]
            mime_type: String,
            /// Base64 image bytes or a data URL.
            data: String,
        }
        pub(super) fn default_chat_image_mime_type() -> String {
            "image/png".to_string()
        }
        let body: ChatRequest = match read_body_json(req).await {
            Ok(v) => v,
            Err(resp) => return resp,
        };
        let config = service.agent_config_store.load();
        if !config.enabled {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "Agent is disabled");
        }
        let session_key = body
            .session_key
            .unwrap_or_else(|| "test-session".to_string());
        info!(
            session_key = %session_key,
            message_len = body.message.len(),
            has_system_prompt_override = body.system_prompt.is_some(),
            "invoking agent chat api"
        );

        // ── Session-free command fast path ──────────────────────────────
        if body.message.trim() == "/status" {
            if let Some(mut status) = service
                .agent_session_manager
                .get_active_turn_status(&session_key)
            {
                let pending_guides = service.queue_manager.guide_status(&session_key);
                status.pending_guide_messages = pending_guides.clone();
                return json_response(&serde_json::json!({
                    "success": true,
                    "response": bifrost_agent::format_active_turn_status_text(&status),
                    "active_status": status,
                    "pending_guide_messages": pending_guides,
                    "tool_calls": [],
                    "plan_steps": null
                }));
            }
            let detail = resolve_agent_api_status_detail(
                &service.agent_session_manager,
                &session_key,
                body.work_dir,
            );
            let response = build_agent_api_status_text(detail.as_ref(), &config);
            return json_response(&serde_json::json!({
                "success": true,
                "response": response,
                "tool_calls": [],
                "plan_steps": null
            }));
        }

        if body.message.trim() == "/stop" {
            let stopped = service.agent_session_manager.request_stop(&session_key);
            return json_response(&serde_json::json!({
                "success": true,
                "response": if stopped {
                    "已请求停止当前 Agent loop。"
                } else {
                    "当前没有正在执行的 Agent loop。"
                },
                "stopped": stopped,
                "tool_calls": [],
                "plan_steps": null
            }));
        }

        if let Some(response) =
            bifrost_agent::handle_session_free_command(&session_key, &body.message, &config)
        {
            return json_response(&serde_json::json!({
                "success": true,
                "response": response,
                "tool_calls": [],
                "plan_steps": null
            }));
        }

        // ── Busy check ─────────────────────────────────────────────────
        let mut session = match service
            .agent_session_manager
            .try_take_session_with_work_dir(&session_key, body.work_dir)
        {
            Some(s) => s,
            None => {
                return json_response(&serde_json::json!({
                    "success": true,
                    "response": "⏳ Agent 正在处理中，请稍后再试。\n\n提示: /stop 可立即停止当前 loop；/help、/remember、/memories、/forget 等命令即使在处理中也可立即响应。",
                    "tool_calls": [],
                    "plan_steps": null
                }))
            }
        };
        session.source = "api".to_string();
        // If guide messages are provided, inject into the shared guide channel
        // to simulate messages arriving before the next guide checkpoint.
        let mut has_guide_messages = false;
        if let Some(ref guide_msg) = body.guide_message {
            if !guide_msg.trim().is_empty() {
                service
                    .queue_manager
                    .inject_guide(&session_key, guide_msg.clone());
                has_guide_messages = true;
            }
        }
        for guide_msg in &body.guide_messages {
            if !guide_msg.trim().is_empty() {
                service
                    .queue_manager
                    .inject_guide(&session_key, guide_msg.clone());
                has_guide_messages = true;
            }
        }
        if has_guide_messages {
            session.guide_channel = Some(
                service
                    .queue_manager
                    .get_or_create_guide_channel(&session_key),
            );
        }
        // If queue_messages is provided, inject into session's pending_messages
        // to simulate queued messages arriving during agent processing.
        for msg in &body.queue_messages {
            if !msg.trim().is_empty() {
                session.pending_messages.push_back(msg.clone());
            }
        }
        // Initialize MCP from config for test endpoint (mirrors event loop behavior)
        let mut mcp_manager = ImMcpManager::new(&config.mcp_servers).await;
        let mcp_opt: Option<&mut ImMcpManager> = if mcp_manager.list_tools().is_empty() {
            None
        } else {
            Some(&mut mcp_manager)
        };
        // Create recorder for persistence (same logic as process_agent_chat)
        let mut recorder = if !config.is_ephemeral() {
            let should_persist = config
                .history
                .as_ref()
                .map(|h| h.persistence != bifrost_agent::config::HistoryPersistence::None)
                .unwrap_or(true);
            if should_persist {
                if session.recorder.is_some() {
                    session.recorder.take()
                } else {
                    let data_dir = bifrost_agent::config::agent_home_dir();
                    let max_bytes = config.history.as_ref().and_then(|h| h.max_bytes);
                    let mut rec = ConversationRecorder::new_with_max_bytes(
                        &data_dir,
                        &session_key,
                        max_bytes,
                    );
                    let _ = rec.record_session_start(
                        &session_key,
                        serde_json::json!({
                            "model": config.model,
                            "provider": config.model_provider,
                            "source": "api",
                            "base_instructions": bifrost_agent::prompt::resolve_base_instructions_text(&config, None),
                        }),
                    );
                    Some(rec)
                }
            } else {
                None
            }
        } else {
            None
        };
        let images: Vec<bifrost_agent::ChatImageInput> = body
            .images
            .iter()
            .take(MAX_AGENT_IMAGES_PER_MESSAGE)
            .filter(|image| !image.data.trim().is_empty())
            .map(|image| bifrost_agent::ChatImageInput {
                mime_type: image.mime_type.clone(),
                data: image.data.clone(),
            })
            .collect();
        if body.images.len() > MAX_AGENT_IMAGES_PER_MESSAGE {
            warn!(
                session_key = %session_key,
                image_count = body.images.len(),
                max_images = MAX_AGENT_IMAGES_PER_MESSAGE,
                "too many /agent/chat images in one request; truncating images"
            );
        }
        let turn_tools = service.build_agent_tool_registry(config.default_message_channel.clone());
        let result = bifrost_agent::session::run_turn_with_mcp_multimodal(
            &service.agent_client,
            &config,
            &mut session,
            &turn_tools,
            mcp_opt,
            &body.message,
            &images,
            body.system_prompt.as_deref(),
            recorder.as_mut(),
        )
        .await;
        mcp_manager.shutdown().await;
        if recorder.is_some() && !session.memory_cleared {
            session.recorder = recorder;
        }
        service.agent_session_manager.return_session(session);
        match result {
            Ok(turn_result) => {
                info!(
                    session_key = %session_key,
                    response_len = turn_result.response.len(),
                    tool_call_count = turn_result.tool_calls_log.len(),
                    "agent chat api completed"
                );
                json_response(&serde_json::json!({
                    "success": true,
                    "response": turn_result.response,
                    "tool_calls": turn_result.tool_calls_log,
                    "plan_steps": turn_result.plan_steps
                }))
            }
            Err(e) => {
                error!(
                    session_key = %session_key,
                    error = %e,
                    "agent chat api failed"
                );
                error_response(StatusCode::INTERNAL_SERVER_ERROR, &e)
            }
        }
    } else {
        error_response(StatusCode::NOT_FOUND, "Agent endpoint not found")
    }
}

pub(super) fn agent_config_response(
    config: crate::im_gateway::agent::ImAgentConfig,
) -> serde_json::Value {
    let default_base_instructions = bifrost_agent::prompt::default_base_instructions();
    let effective_base_instructions = config
        .base_instructions
        .as_deref()
        .unwrap_or(default_base_instructions)
        .to_string();
    let mut value = serde_json::to_value(config).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "default_base_instructions".to_string(),
            serde_json::Value::String(default_base_instructions.to_string()),
        );
        obj.insert(
            "effective_base_instructions".to_string(),
            serde_json::Value::String(effective_base_instructions),
        );
    }
    value
}

pub(super) fn patch_optional_string(
    target: &mut Option<String>,
    patch: &serde_json::Value,
    keys: &[&str],
) {
    let Some(value) = keys.iter().find_map(|key| patch.get(*key)) else {
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

pub(super) fn apply_agent_config_patch(
    config: &mut crate::im_gateway::agent::ImAgentConfig,
    patch: &serde_json::Value,
) {
    if let Some(enabled) = patch.get("enabled").and_then(|v| v.as_bool()) {
        config.enabled = enabled;
    }
    if let Some(model) = patch.get("model").and_then(|v| v.as_str()) {
        config.model = Some(model.to_string());
    }
    if let Some(provider) = patch.get("model_provider").and_then(|v| v.as_str()) {
        config.model_provider = Some(provider.to_string());
    }
    if let Some(tokens) = patch.get("max_completion_tokens").and_then(|v| v.as_u64()) {
        config.max_completion_tokens = Some(u32::try_from(tokens).unwrap_or(u32::MAX));
    }
    if let Some(effort) = patch
        .get("model_reasoning_effort")
        .or_else(|| patch.get("reasoning_effort"))
        .and_then(|v| v.as_str())
    {
        config.model_reasoning_effort = Some(effort.to_string());
    }
    if let Some(summary) = patch
        .get("model_reasoning_summary")
        .or_else(|| patch.get("reasoning_summary"))
        .and_then(|v| v.as_str())
    {
        config.model_reasoning_summary = Some(summary.to_string());
    }
    if let Some(window) = patch.get("model_context_window").and_then(|v| v.as_i64()) {
        config.model_context_window = Some(window);
    }
    if let Some(compact) = patch
        .get("model_auto_compact_token_limit")
        .or_else(|| patch.get("compact_threshold_tokens"))
    {
        if compact.is_null() {
            // null → clear override, fall back to context_window × 90%
            config.model_auto_compact_token_limit = None;
        } else if let Some(v) = compact.as_i64() {
            config.model_auto_compact_token_limit = Some(v);
        }
    }
    patch_optional_string(&mut config.base_instructions, patch, &["base_instructions"]);
    patch_optional_string(
        &mut config.developer_instructions,
        patch,
        &["developer_instructions"],
    );
    patch_optional_string(&mut config.user_instructions, patch, &["user_instructions"]);
    if let Some(max_hist) = patch.get("max_history_messages").and_then(|v| v.as_u64()) {
        config.max_history_messages = Some(u32::try_from(max_hist).unwrap_or(u32::MAX));
    }
    if let Some(ttl) = patch.get("session_ttl_secs").and_then(|v| v.as_u64()) {
        config.session_ttl_secs = Some(ttl);
    }
    if let Some(timeout) = patch.get("request_timeout_secs").and_then(|v| v.as_u64()) {
        config.request_timeout_secs = Some(timeout);
    }
    if let Some(max_iter) = patch.get("max_turn_iterations").and_then(|v| v.as_u64()) {
        config.max_turn_iterations = Some(u32::try_from(max_iter).unwrap_or(u32::MAX));
    }
    if let Some(tool_limit) = patch
        .get("tool_output_token_limit")
        .and_then(|v| v.as_u64())
    {
        config.tool_output_token_limit = Some(tool_limit as usize);
    }
    if let Some(doc_max) = patch.get("project_doc_max_bytes").and_then(|v| v.as_u64()) {
        config.project_doc_max_bytes = Some(doc_max as usize);
    }
    if let Some(work_dir) = patch.get("work_dir").and_then(|v| v.as_str()) {
        config.work_dir = Some(work_dir.to_string());
    }

    // History & Session settings
    if let Some(ephemeral) = patch.get("ephemeral").and_then(|v| v.as_bool()) {
        config.ephemeral = ephemeral;
    }
    if let Some(history_obj) = patch.get("history").and_then(|v| v.as_object()) {
        let history = config.history.get_or_insert_with(Default::default);
        if let Some(persistence) = history_obj.get("persistence").and_then(|v| v.as_str()) {
            history.persistence = match persistence {
                "none" => bifrost_agent::HistoryPersistence::None,
                "last-90-days" => bifrost_agent::HistoryPersistence::Last90Days,
                _ => bifrost_agent::HistoryPersistence::SaveAll,
            };
        }
        if let Some(max_bytes) = history_obj.get("max_bytes").and_then(|v| v.as_u64()) {
            history.max_bytes = Some(max_bytes as usize);
        }
    }
    if let Some(memories_obj) = patch.get("memories").and_then(|v| v.as_object()) {
        let memories = config.memories.get_or_insert_with(Default::default);
        if let Some(v) = memories_obj
            .get("disable_on_external_context")
            .and_then(|v| v.as_bool())
        {
            memories.disable_on_external_context = Some(v);
        }
        if let Some(v) = memories_obj
            .get("generate_memories")
            .and_then(|v| v.as_bool())
        {
            memories.generate_memories = Some(v);
        }
        if let Some(v) = memories_obj.get("use_memories").and_then(|v| v.as_bool()) {
            memories.use_memories = Some(v);
        }
        if let Some(v) = memories_obj
            .get("max_raw_memories_for_consolidation")
            .and_then(|v| v.as_u64())
        {
            memories.max_raw_memories_for_consolidation = Some(v as usize);
        }
        if let Some(v) = memories_obj.get("max_unused_days").and_then(|v| v.as_i64()) {
            memories.max_unused_days = Some(v);
        }
        if let Some(v) = memories_obj
            .get("max_rollout_age_days")
            .and_then(|v| v.as_i64())
        {
            memories.max_rollout_age_days = Some(v);
        }
        if let Some(v) = memories_obj
            .get("max_rollouts_per_startup")
            .and_then(|v| v.as_u64())
        {
            memories.max_rollouts_per_startup = Some(v as usize);
        }
        if let Some(v) = memories_obj
            .get("min_rollout_idle_hours")
            .and_then(|v| v.as_i64())
        {
            memories.min_rollout_idle_hours = Some(v);
        }
        if let Some(v) = memories_obj
            .get("min_rate_limit_remaining_percent")
            .and_then(|v| v.as_i64())
        {
            memories.min_rate_limit_remaining_percent = Some(v);
        }
        if let Some(v) = memories_obj.get("extract_model").and_then(|v| v.as_str()) {
            memories.extract_model = Some(v.to_string());
        }
        if let Some(v) = memories_obj
            .get("consolidation_model")
            .and_then(|v| v.as_str())
        {
            memories.consolidation_model = Some(v.to_string());
        }
    }
    if let Some(timeout) = patch
        .get("background_terminal_max_timeout")
        .and_then(|v| v.as_u64())
    {
        config.background_terminal_max_timeout = Some(timeout);
    }
    if let Some(channel) = patch.get("default_message_channel") {
        if channel.is_null() {
            config.default_message_channel = None;
        } else if let Ok(binding) =
            serde_json::from_value::<bifrost_agent::ImMessageChannelBinding>(channel.clone())
        {
            config.default_message_channel = Some(binding);
        }
    }

    // Provider-level fields: apply to the active provider in model_providers
    let provider_id = config
        .model_provider
        .clone()
        .unwrap_or_else(|| "aidp_crawl".to_string());
    let provider = config
        .model_providers
        .entry(provider_id.clone())
        .or_insert_with(|| bifrost_agent::ModelProviderConfig {
            name: Some(provider_id.clone()),
            base_url: None,
            env_key: None,
            api_key: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_idle_timeout_ms: None,
            stream_max_retries: None,
        });
    if let Some(url) = patch.get("base_url").and_then(|v| v.as_str()) {
        provider.base_url = Some(url.to_string());
    }
    if let Some(key) = patch.get("api_key").and_then(|v| v.as_str()) {
        if key.is_empty() {
            provider.api_key = None;
            if let Some(headers) = provider.http_headers.as_mut() {
                headers.remove("api-key");
                if headers.is_empty() {
                    provider.http_headers = None;
                }
            }
        } else {
            provider.api_key = Some(key.to_string());
            if uses_api_key_header(&provider_id, patch) {
                provider
                    .http_headers
                    .get_or_insert_with(HashMap::new)
                    .insert("api-key".to_string(), key.to_string());
            }
        }
    }
    if let Some(env_key) = patch.get("env_key").and_then(|v| v.as_str()) {
        provider.env_key = Some(env_key.to_string());
    }
    if let Some(by_azure) = patch.get("by_azure").and_then(|v| v.as_bool()) {
        if by_azure {
            let headers = provider.http_headers.get_or_insert_with(HashMap::new);
            if !headers.contains_key("api-key") {
                headers.insert("api-key".to_string(), String::new());
            }
        } else if let Some(ref mut headers) = provider.http_headers {
            headers.remove("api-key");
        }
    }
    if let Some(retries) = patch.get("request_max_retries").and_then(|v| v.as_u64()) {
        provider.request_max_retries = Some(retries);
    }
    if let Some(timeout) = patch.get("stream_idle_timeout_ms").and_then(|v| v.as_u64()) {
        provider.stream_idle_timeout_ms = Some(timeout);
    }
    if let Some(retries) = patch.get("stream_max_retries").and_then(|v| v.as_u64()) {
        provider.stream_max_retries = Some(retries);
    }

    // MCP servers: full replacement via JSON object
    if let Some(mcp_obj) = patch.get("mcp_servers").and_then(|v| v.as_object()) {
        let mut mcp_servers = HashMap::new();
        for (name, server_val) in mcp_obj {
            if let Ok(server_config) =
                serde_json::from_value::<bifrost_agent::McpServerConfig>(server_val.clone())
            {
                mcp_servers.insert(name.clone(), server_config);
            }
        }
        config.mcp_servers = mcp_servers;
    }

    // Model providers: full replacement via JSON object
    if let Some(providers_obj) = patch.get("model_providers").and_then(|v| v.as_object()) {
        let mut model_providers = HashMap::new();
        for (name, provider_val) in providers_obj {
            if let Ok(provider_config) =
                serde_json::from_value::<bifrost_agent::ModelProviderConfig>(provider_val.clone())
            {
                model_providers.insert(name.clone(), provider_config);
            }
        }
        config.model_providers = model_providers;
    }
}

pub(super) fn uses_api_key_header(provider_id: &str, patch: &serde_json::Value) -> bool {
    patch
        .get("by_azure")
        .and_then(|v| v.as_bool())
        .unwrap_or(matches!(provider_id, "aidp_crawl" | "azure"))
}

// ---------------------------------------------------------------------------
