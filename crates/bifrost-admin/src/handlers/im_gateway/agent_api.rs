use super::*;
use bytes::Bytes;
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use tokio_stream::wrappers::BroadcastStream;

// ---------------------------------------------------------------------------

type HistorySessionEntry = (
    PathBuf,
    String,
    bifrost_agent::persistence::SessionFileSummary,
);

fn is_builtin_runner_value(value: &str) -> bool {
    matches!(value, "bifrost_agent" | "builtin" | "bifrost")
}

fn is_external_runner_metadata(runner_type: Option<&str>, runner_id: Option<&str>) -> bool {
    runner_type
        .map(|value| !is_builtin_runner_value(value))
        .unwrap_or(false)
        || runner_id
            .map(|value| !is_builtin_runner_value(value))
            .unwrap_or(false)
}

fn merge_history_runner_metadata(
    runner_type: Option<String>,
    runner_id: Option<String>,
    history: Option<&bifrost_agent::persistence::SessionFileSummary>,
) -> (Option<String>, Option<String>) {
    let Some(summary) = history else {
        return (runner_type, runner_id);
    };
    if is_external_runner_metadata(summary.runner_type.as_deref(), summary.runner_id.as_deref()) {
        (
            summary.runner_type.clone().or(runner_type),
            summary.runner_id.clone().or(runner_id),
        )
    } else {
        (runner_type, runner_id)
    }
}

fn enrich_detail_with_history_runner_metadata(detail: &mut bifrost_agent::SessionDetail) {
    let Some(history_path) = detail.history_path.as_deref() else {
        return;
    };
    let summary = bifrost_agent::persistence::scan_session_summary(Path::new(history_path));
    if !is_external_runner_metadata(summary.runner_type.as_deref(), summary.runner_id.as_deref()) {
        return;
    }
    let runner_type = summary.runner_type.or(detail.runner_type.take());
    let runner_id = summary.runner_id.or(detail.runner_id.take());
    let state_adapter = runner_type
        .as_deref()
        .or(runner_id.as_deref())
        .unwrap_or_default();
    if let Some(state) = crate::im_gateway::session_state::load_session_state(
        &detail.session_key,
        state_adapter,
        runner_id.as_deref(),
    ) {
        detail.external_conversation_id = state.external_conversation_id;
        detail.external_thread_id = state.external_thread_id;
    }
    detail.runner_type = runner_type;
    detail.runner_id = runner_id;
    if detail.agent_type.as_deref() == Some("Bifrost Agent") || detail.agent_type.is_none() {
        detail.agent_type = Some("external_cli".to_string());
    }
}

async fn load_history_entries_blocking(
    data_dir: PathBuf,
    session_key: Option<String>,
    max_files: Option<usize>,
) -> Result<Vec<HistorySessionEntry>, String> {
    tokio::task::spawn_blocking(move || {
        let mut files =
            bifrost_agent::persistence::list_conversations(&data_dir, session_key.as_deref());
        if session_key.is_none() {
            if max_files.is_some() {
                let mut latest_by_parsed_key: std::collections::HashMap<String, PathBuf> =
                    std::collections::HashMap::new();
                for path in files {
                    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let (parsed_key, _timestamp) = parse_session_filename(filename);
                    latest_by_parsed_key
                        .entry(parsed_key)
                        .and_modify(|existing| {
                            if path > *existing {
                                *existing = path.clone();
                            }
                        })
                        .or_insert(path);
                }
                files = latest_by_parsed_key.into_values().collect();
                files.sort();
            }
            if let Some(max_files) = max_files {
                if files.len() > max_files {
                    files.drain(0..files.len() - max_files);
                }
            }
        }
        Ok::<Vec<HistorySessionEntry>, String>(
            files
                .into_iter()
                .map(|path| {
                    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let (parsed_key, _timestamp) = parse_session_filename(filename);
                    let summary = bifrost_agent::persistence::scan_session_summary(&path);
                    (path, parsed_key, summary)
                })
                .collect(),
        )
    })
    .await
    .map_err(|error| format!("history scan task failed: {error}"))?
}

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
        let mcp_http_network = service
            .agent_client
            .model_proxy_url()
            .map(|proxy_url| {
                bifrost_agent::mcp::McpHttpNetwork::with_proxy_and_ca(
                    proxy_url.to_string(),
                    Some(bifrost_storage::data_dir().join("certs").join("ca.crt")),
                )
            })
            .unwrap_or_else(bifrost_agent::mcp::McpHttpNetwork::direct);
        let statuses = bifrost_agent::mcp::check_server_availability_with_http_network(
            &config.mcp_servers,
            mcp_http_network,
        )
        .await;
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

    // GET /agent/sessions/events — push session list changes to every view.
    if rest == "/sessions/events" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        return agent_session_events_response(service);
    }

    // GET /agent/sessions/all — unified list of active + history sessions
    if rest == "/sessions/all" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        let query_params = parse_query_params(req.uri().query().unwrap_or_default());
        let list_limit = query_params
            .get("limit")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .map(|value| value.min(200));
        let history_scan_limit = list_limit.map(|limit| limit.saturating_mul(4).max(100));
        let active_sessions = service.agent_session_manager.list_sessions();
        let running_turns = service.agent_session_manager.list_active_turn_statuses();
        let active_info_by_key: std::collections::HashMap<String, bifrost_agent::SessionInfo> =
            active_sessions
                .iter()
                .map(|session| (session.session_key.clone(), session.clone()))
                .collect();
        let running_keys: std::collections::HashSet<String> = running_turns
            .iter()
            .map(|status| status.session_key.clone())
            .collect();
        let data_dir = bifrost_agent::config::agent_home_dir();
        let history_entries =
            match load_history_entries_blocking(data_dir, None, history_scan_limit).await {
                Ok(entries) => entries,
                Err(error) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("failed to scan agent sessions: {error}"),
                    );
                }
            };
        let mut history_by_key: std::collections::HashMap<
            String,
            (
                std::path::PathBuf,
                bifrost_agent::persistence::SessionFileSummary,
            ),
        > = std::collections::HashMap::new();
        for (p, parsed_key, summary) in &history_entries {
            let session_key = summary
                .session_key
                .as_deref()
                .unwrap_or(parsed_key)
                .to_string();
            if is_runner_call_session_key(&session_key) {
                continue;
            }
            let replace = history_by_key
                .get(&session_key)
                .map(|(_, existing)| summary.end_time > existing.end_time)
                .unwrap_or(true);
            if replace {
                history_by_key.insert(session_key, (p.clone(), summary.clone()));
            }
        }
        let active_keys: std::collections::HashSet<String> = active_sessions
            .iter()
            .map(|s| s.session_key.clone())
            .chain(running_turns.iter().map(|s| s.session_key.clone()))
            .collect();

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

        // Add running turn sessions. These are not present in list_sessions()
        // because the turn loop temporarily takes exclusive ownership.
        for s in running_turns {
            if is_runner_call_session_key(&s.session_key) {
                continue;
            }
            let info = active_info_by_key.get(&s.session_key);
            let history = history_by_key.get(&s.session_key);
            let (runner_type, runner_id) = merge_history_runner_metadata(
                s.runner_type
                    .or_else(|| info.and_then(|item| item.runner_type.clone())),
                s.runner_id
                    .or_else(|| info.and_then(|item| item.runner_id.clone())),
                history.map(|(_, summary)| summary),
            );
            let duration_secs = s.updated_at.saturating_sub(s.started_at);
            let queue_snapshot = crate::handlers::agent_chat::queue_snapshot_payload(
                &service.queue_manager,
                &s.session_key,
            );
            unified.push(serde_json::json!({
                "session_key": s.session_key,
                "status": "active",
                "running": true,
                "state": s.state,
                "source": info.map(|item| item.source.clone()).unwrap_or_else(|| "admin-api".to_string()),
                "work_dir": s.work_dir.or_else(|| info.and_then(|item| item.work_dir.clone())),
                "turns": s.message_count.max(info.map(|item| item.message_count).unwrap_or(0)),
                "tokens": s.total_tokens_used,
                "start_time": s.started_at,
                "last_active_time": s.updated_at,
                "duration_secs": duration_secs,
                "compaction_count": s.compaction_count,
                "estimated_tokens": s.estimated_context_tokens,
                "context_window_tokens": s.context_window_tokens,
                "context_usage_percent": s.context_usage_percent,
                "last_response_tokens": s.last_response_tokens,
                "agent_type": s.agent_type.or_else(|| info.and_then(|item| item.agent_type.clone())),
                "runner_type": runner_type,
                "runner_id": runner_id,
                "external_conversation_id": s.external_conversation_id,
                "external_thread_id": s.external_thread_id,
                "title": info.and_then(|item| item.title.clone()),
                "history_path": history.map(|(path, _)| path.display().to_string()),
                "has_timeline": history.is_some(),
                "timeline_event_count": history.map(|(_, summary)| summary.event_count).unwrap_or(0),
                "run_state": "running",
                "queue_length": queue_snapshot["queueLength"].clone(),
                "queue_items": queue_snapshot["queueItems"].clone(),
                "queueLength": queue_snapshot["queueLength"].clone(),
                "queueItems": queue_snapshot["queueItems"].clone(),
            }));
        }

        // Add idle in-memory sessions.
        for s in active_sessions {
            if is_runner_call_session_key(&s.session_key) {
                continue;
            }
            if running_keys.contains(&s.session_key) {
                continue;
            }
            let history = history_by_key.get(&s.session_key);
            let (runner_type, runner_id) = merge_history_runner_metadata(
                s.runner_type,
                s.runner_id,
                history.map(|(_, summary)| summary),
            );
            let duration_secs = s.last_active_at.saturating_sub(s.created_at);
            let queue_snapshot = crate::handlers::agent_chat::queue_snapshot_payload(
                &service.queue_manager,
                &s.session_key,
            );
            let has_active_turn = s.running;
            let run_state = active_session_list_run_state(has_active_turn, &s.run_state);
            unified.push(serde_json::json!({
                "session_key": s.session_key,
                "status": "active",
                "running": has_active_turn,
                "state": if has_active_turn { "running" } else { "idle" },
                "source": s.source,
                "work_dir": s.work_dir,
                "turns": s.message_count,
                "tokens": s.total_tokens_used,
                "start_time": s.created_at,
                "last_active_time": s.last_active_at,
                "duration_secs": duration_secs,
                "compaction_count": s.compaction_count,
                "estimated_tokens": s.estimated_tokens,
                "agent_type": s.agent_type,
                "runner_type": runner_type,
                "runner_id": runner_id,
                "external_conversation_id": s.external_conversation_id,
                "external_thread_id": s.external_thread_id,
                "title": s.title,
                "history_path": s.history_path.or_else(|| history.map(|(path, _)| path.display().to_string())),
                "has_timeline": s.has_timeline || history.is_some(),
                "timeline_event_count": if s.timeline_event_count > 0 {
                    s.timeline_event_count
                } else {
                    history
                        .map(|(_, summary)| summary.event_count as usize)
                        .unwrap_or(0)
                },
                "run_state": run_state,
                "queue_length": queue_snapshot["queueLength"].clone(),
                "queue_items": queue_snapshot["queueItems"].clone(),
                "queueLength": queue_snapshot["queueLength"].clone(),
                "queueItems": queue_snapshot["queueItems"].clone(),
            }));
        }

        // Add history sessions (excluding those already active or expired)
        for (p, parsed_key, summary) in history_entries {
            // Prefer the original session key from JSONL content (handles sanitized filenames)
            let session_key = summary
                .session_key
                .as_deref()
                .unwrap_or(&parsed_key)
                .to_string();
            if is_runner_call_session_key(&session_key) {
                continue;
            }
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
                "running": false,
                "state": "ended",
                "source": summary.source,
                "work_dir": summary.work_dir,
                "turns": (summary.user_turns as usize) + (summary.assistant_turns as usize),
                "tokens": summary.total_tokens,
                "start_time": summary.start_time,
                "last_active_time": summary.end_time,
                "duration_secs": summary.end_time.saturating_sub(summary.start_time),
                "history_path": p.display().to_string(),
                "has_timeline": true,
                "timeline_event_count": summary.event_count,
                "agent_type": if is_external_runner_metadata(
                    summary.runner_type.as_deref(),
                    summary.runner_id.as_deref(),
                ) {
                    Some("external_cli".to_string())
                } else {
                    None
                },
                "runner_type": summary.runner_type,
                "runner_id": summary.runner_id,
                "run_state": history_session_list_run_state(&summary),
                "title": summary.title.or(summary.first_user_message),
            }));
        }

        // Add external runner sessions (Codex / ChatGPT Web / other adapters)
        // that do not have an Agent JSONL history file. These runs are tracked
        // in the IM Gateway session-state store, so the Web UI can keep the
        // thread visible after the stream finishes or the page refreshes.
        for state in crate::im_gateway::session_state::list_session_states() {
            if is_runner_call_session_key(&state.session_key) {
                continue;
            }
            if active_keys.contains(&state.session_key) {
                continue;
            }
            if state.latest_run_id.is_none()
                && state.last_user_message.is_none()
                && state.last_response.is_none()
            {
                continue;
            }
            let last_active_time = state.updated_at / 1000;
            let first_message_time = state
                .messages
                .iter()
                .filter_map(|message| message.timestamp)
                .min()
                .unwrap_or(last_active_time);
            let turn_count = if state.messages.is_empty() {
                usize::from(state.last_user_message.is_some())
                    + usize::from(state.last_response.is_some())
            } else {
                state.messages.len()
            };
            let history = history_by_key.get(&state.session_key);
            let projection =
                persisted_session_projection(&state, history.map(|(_, summary)| summary));
            let last_active_time = if projection.prefer_history_time {
                history
                    .map(|(_, summary)| summary_end_time(summary))
                    .unwrap_or(last_active_time)
            } else {
                last_active_time
            };
            let history_path = state
                .history_path
                .clone()
                .or_else(|| history.map(|(path, _)| path.display().to_string()));
            let has_timeline = history_path.is_some();
            let timeline_event_count = history.map(|(_, summary)| summary.event_count).unwrap_or(0);
            unified.push(serde_json::json!({
                "session_key": state.session_key,
                "status": projection.status,
                "running": projection.running,
                "state": projection.state,
                "run_state": projection.run_state,
                "source": "admin-api",
                "work_dir": state.work_dir,
                "turns": turn_count,
                "tokens": serde_json::Value::Null,
                "start_time": first_message_time,
                "last_active_time": last_active_time,
                "duration_secs": last_active_time.saturating_sub(first_message_time),
                "agent_type": "external_cli",
                "runner_type": state.adapter,
                "runner_id": state.runner_id,
                "external_conversation_id": state.external_conversation_id,
                "external_thread_id": state.external_thread_id,
                "external_run_id": state.latest_run_id,
                "history_path": history_path,
                "has_timeline": has_timeline,
                "timeline_event_count": timeline_event_count,
                "title": state.title.or(state.last_user_message),
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
        dedupe_unified_sessions_by_key(&mut unified);
        if let Some(limit) = list_limit {
            unified.truncate(limit);
        }

        let active_count = unified
            .iter()
            .filter(|item| item.get("status").and_then(|value| value.as_str()) == Some("active"))
            .count();
        let history_count = unified.len().saturating_sub(active_count);

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
        let history_entries = match load_history_entries_blocking(data_dir, None, None).await {
            Ok(entries) => entries,
            Err(error) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("failed to scan agent sessions: {error}"),
                );
            }
        };
        let history: Vec<serde_json::Value> = history_entries
            .iter()
            .map(|(p, parsed_key, summary)| {
                let filename = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let (_filename_key, timestamp) = parse_session_filename(filename);
                let session_key = summary
                    .session_key
                    .as_deref()
                    .unwrap_or(parsed_key)
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
                    "agent_type": if is_external_runner_metadata(
                        summary.runner_type.as_deref(),
                        summary.runner_id.as_deref(),
                    ) {
                        Some("external_cli".to_string())
                    } else {
                        None
                    },
                    "runner_type": summary.runner_type.clone(),
                    "runner_id": summary.runner_id.clone(),
                    "title": summary.title.clone().or_else(|| summary.first_user_message.clone()),
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
        let data_dir = bifrost_agent::config::agent_home_dir();
        let path = match bifrost_agent::persistence::validate_conversation_path(
            &data_dir,
            Path::new(&file_path),
        ) {
            Ok(path) => path,
            Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
        };
        if req.method() == Method::GET {
            // Return full events with all details (tool calls, results, metadata, etc.)
            let query_params = parse_query_params(req.uri().query().unwrap_or_default());
            let limit = query_params
                .get("limit")
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .map(|value| value.min(500));
            let cursor = query_params
                .get("cursor")
                .and_then(|value| value.parse::<usize>().ok());
            let since = query_params
                .get("since")
                .and_then(|value| value.parse::<usize>().ok());
            let tail = query_params
                .get("tail")
                .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
                .unwrap_or(false);
            let paged_request = limit.is_some() || cursor.is_some() || since.is_some() || tail;
            let page_path = path.clone();
            let page_result = if paged_request {
                tokio::task::spawn_blocking(move || {
                    bifrost_agent::persistence::load_conversation_events_page(
                        &page_path,
                        bifrost_agent::persistence::ConversationEventPageOptions {
                            limit,
                            cursor,
                            tail,
                            since,
                        },
                    )
                })
                .await
                .map_err(|error| format!("history page task failed: {error}"))
                .and_then(|result| result)
            } else {
                tokio::task::spawn_blocking(move || {
                    bifrost_agent::persistence::load_conversation_events(&page_path).map(|events| {
                        let total_count = events.len();
                        bifrost_agent::persistence::ConversationEventPage {
                            events,
                            total_count,
                            start_index: 0,
                            end_index: total_count,
                            next_cursor: None,
                            has_more: false,
                        }
                    })
                })
                .await
                .map_err(|error| format!("history load task failed: {error}"))
                .and_then(|result| result)
            };
            match page_result {
                Ok(page) => {
                    let event_values: Vec<serde_json::Value> = page
                        .events
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
                    return json_response(&serde_json::json!({
                        "events": event_values,
                        "count": event_values.len(),
                        "total_count": page.total_count,
                        "start_index": page.start_index,
                        "end_index": page.end_index,
                        "next_cursor": page.next_cursor,
                        "has_more": page.has_more,
                    }));
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
            match std::fs::remove_file(&path) {
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
            let active_status = service
                .agent_session_manager
                .get_active_turn_status(&session_key);
            let detail = if let Some(detail) = service
                .agent_session_manager
                .get_session_detail(&session_key)
            {
                Some(detail)
            } else {
                let history_session_key = session_key.clone();
                match tokio::task::spawn_blocking(move || {
                    history_session_detail(&history_session_key)
                })
                .await
                {
                    Ok(detail) => detail,
                    Err(error) => {
                        warn!(
                            session_key = %session_key,
                            error = %error,
                            "history session detail task failed"
                        );
                        None
                    }
                }
            };
            if let Some(mut detail) = detail {
                enrich_detail_with_history_runner_metadata(&mut detail);
                return json_response(&session_detail_response(
                    detail,
                    &service.queue_manager,
                    active_status,
                ));
            }
            if let Some(detail) = external_runner_session_detail(&session_key).await {
                return json_response(&session_detail_response(
                    detail,
                    &service.queue_manager,
                    active_status,
                ));
            }
            if let Some(status) = active_status {
                return json_response(&active_status_session_detail(
                    status,
                    &service.queue_manager,
                ));
            }
            return error_response(StatusCode::NOT_FOUND, "session not found");
        }
        if req.method() == Method::DELETE {
            service.agent_session_manager.request_stop(&session_key);
            service.agent_session_manager.clear_session(&session_key);
            service.queue_manager.clear_session(&session_key);
            clear_persisted_agent_session_state(&session_key, None, None);
            return json_response(&serde_json::json!({
                "ok": true,
                "message": "session deleted"
            }));
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
            #[serde(default, alias = "collaborationMode")]
            collaboration_mode: Option<bifrost_agent::CollaborationMode>,
            #[serde(default)]
            work_dir: Option<String>,
            /// Optional JSONL history file to restore before continuing.
            #[serde(default)]
            history_path: Option<String>,
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
        let mut body: ChatRequest = match read_body_json(req).await {
            Ok(v) => v,
            Err(resp) => return resp,
        };
        let slash_mode = crate::im_gateway::agent_slash::parse_agent_slash_mode(&body.message);
        body.message = slash_mode.message;
        body.collaboration_mode = crate::im_gateway::agent_slash::merge_collaboration_mode(
            body.collaboration_mode,
            slash_mode.collaboration_mode,
        );
        if body.message.trim().is_empty() && body.images.is_empty() {
            return error_response(
                StatusCode::BAD_REQUEST,
                "message must not be empty after /plan",
            );
        }
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
                let pending_guides = merge_pending_guide_messages(
                    &status.pending_guide_messages,
                    service.queue_manager.guide_status(&session_key),
                );
                status.pending_guide_messages = pending_guides.clone();
                let status_context = status_context_from_agent_config(&config);
                return json_response(&serde_json::json!({
                    "success": true,
                    "response": bifrost_agent::format_active_turn_status_text_with_context(
                        &status,
                        &status_context
                    ),
                    "active_status": status,
                    "pending_guide_messages": pending_guides,
                    "tool_calls": [],
                    "plan_steps": null
                }));
            }
            let detail = if service
                .agent_session_manager
                .get_session_detail(&session_key)
                .is_none()
            {
                let has_requested_work_dir = body
                    .work_dir
                    .as_deref()
                    .is_some_and(|work_dir| !work_dir.trim().is_empty());
                if let Some(mut session) = service
                    .agent_session_manager
                    .try_take_session_with_work_dir(&session_key, body.work_dir.clone())
                {
                    session.source = "api".to_string();
                    session.mark_bifrost_agent_runtime();
                    let restored = restore_session_from_persisted_history(
                        &mut session,
                        &session_key,
                        crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
                        None,
                        config.history.as_ref().and_then(|h| h.max_bytes),
                    );
                    if restored.is_some() || has_requested_work_dir {
                        service.agent_session_manager.return_session(session);
                    } else {
                        service.agent_session_manager.release_active(&session_key);
                    }
                }
                service
                    .agent_session_manager
                    .get_session_detail(&session_key)
            } else {
                resolve_agent_api_status_detail(
                    &service.agent_session_manager,
                    &session_key,
                    body.work_dir,
                )
            };
            let response = build_agent_api_status_text(detail.as_ref(), &config);
            return json_response(&serde_json::json!({
                "success": true,
                "response": response,
                "tool_calls": [],
                "plan_steps": null
            }));
        }

        if body.message.trim() == "/stop" {
            let stopped = request_agent_stop(&service.agent_session_manager, &session_key).await;
            let repaired_stale_running = if stopped {
                0
            } else {
                repair_stale_persisted_running_states(&session_key)
            };
            return json_response(&serde_json::json!({
                "success": true,
                "response": if stopped {
                    "已请求停止当前 Agent loop。"
                } else if repaired_stale_running > 0 {
                    "当前没有正在执行的 Agent loop，已修复残留运行状态。"
                } else {
                    "当前没有正在执行的 Agent loop。"
                },
                "stopped": stopped,
                "repaired_stale_running": repaired_stale_running,
                "tool_calls": [],
                "plan_steps": null
            }));
        }

        if matches!(body.message.trim(), "/clear" | "/reset") {
            clear_builtin_im_agent_session(
                &service.agent_session_manager,
                &service.queue_manager,
                &session_key,
            )
            .await;
            return json_response(&serde_json::json!({
                "success": true,
                "response": "会话已重置，可以开始新的对话。",
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
        session.mark_bifrost_agent_runtime();
        let is_manual_compaction = body.message.trim() == "/compact";
        let requested_history_path = body
            .history_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(history_path) = requested_history_path {
            if session.history.is_empty() {
                if let Err(error) = restore_session_from_history_path(
                    &mut session,
                    history_path,
                    &session_key,
                    config.history.as_ref().and_then(|h| h.max_bytes),
                ) {
                    service.agent_session_manager.return_session(session);
                    return error_response(StatusCode::BAD_REQUEST, &error);
                }
            }
        } else {
            restore_session_from_persisted_history(
                &mut session,
                &session_key,
                crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
                None,
                config.history.as_ref().and_then(|h| h.max_bytes),
            );
        }
        service.agent_session_manager.update_active_session_preview(
            &session_key,
            if is_manual_compaction {
                session.title.clone()
            } else {
                session
                    .title
                    .clone()
                    .or_else(|| Some(body.message.trim().to_string()))
            },
            session.work_dir.clone(),
            Some("api".to_string()),
            Some(crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER.to_string()),
            None,
        );
        prime_builtin_worker_active_status(&service.agent_session_manager, &session, &config);
        consume_imported_contexts_for_builtin_agent(&mut session);
        // Initial guide messages are passed to the isolated worker request.
        // Avoid writing them into the main-process guide queue, otherwise the
        // same guide can leak into a later turn after the worker has finished.
        let history_path = session
            .recorder
            .as_ref()
            .map(|recorder| recorder.file_path().display().to_string())
            .or_else(|| body.history_path.clone());
        if !is_manual_compaction {
            remember_session_turn_started(
                &session_key,
                crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
                None,
                &body.message,
                history_path.clone(),
                session.work_dir.clone(),
            );
        }
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
        let mut worker_request = crate::im_gateway::agent_worker::build_run_request(
            session_key.clone(),
            body.message.clone(),
            images,
            &config,
            session.work_dir.clone(),
            history_path,
            Some("api".to_string()),
        );
        worker_request.system_prompt = body.system_prompt.clone();
        worker_request.collaboration_mode = body.collaboration_mode;
        worker_request.guide_messages = body
            .guide_message
            .clone()
            .into_iter()
            .chain(body.guide_messages.clone())
            .collect();
        worker_request.queued_messages = body.queue_messages.clone();
        let mut worker =
            crate::im_gateway::agent_worker::AgentWorkerClient::spawn_or_fallback(worker_request)
                .await;
        let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel::<
            crate::im_gateway::agent_worker::AgentWorkerStopRequest,
        >();
        let worker_pid = worker.child_id().unwrap_or(0);
        crate::im_gateway::agent_worker::register_active_worker(&session_key, worker_pid, stop_tx);
        let mut stop_ack = None;
        let result = loop {
            tokio::select! {
                maybe_stop = stop_rx.recv() => {
                    let _ = worker.terminate().await;
                    stop_ack = maybe_stop;
                    break Err("已收到 /stop，Agent worker 子进程已停止。".to_string());
                }
                event = worker.next_event() => {
                    match event {
                        Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Started { .. })) => {}
                        Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Progress { event })) => {
                            if let bifrost_agent::AgentTurnProgressEvent::Status(status) = event {
                                service.agent_session_manager.update_active_turn_status_from_worker(*status);
                            }
                        }
                        Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Finished { result })) => {
                            if let Some(history_path) = result.history_path.as_deref() {
                                let _ = restore_session_from_history_path(
                                    &mut session,
                                    history_path,
                                    &session_key,
                                    config.history.as_ref().and_then(|h| h.max_bytes),
                                );
                            }
                            break Ok(bifrost_agent::TurnResult::from(result));
                        }
                        Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Failed { error })) => break Err(error),
                        Ok(Some(crate::im_gateway::agent_worker::AgentWorkerEvent::Stopped)) => {
                            break Err("已收到 /stop，Agent worker 子进程已停止。".to_string());
                        }
                        Ok(None) => break Err("agent worker exited without result".to_string()),
                        Err(error) => break Err(format!("agent worker failed: {error}")),
                    }
                }
            }
        };
        crate::im_gateway::agent_worker::clear_active_worker(&session_key);
        if let Ok(turn_result) = &result {
            if let Some(new_dir) = turn_result.work_dir_switched.as_ref() {
                session.work_dir = Some(new_dir.clone());
            }
        }
        remember_session_state_from_agent_session(
            &session,
            crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
            None,
        );
        service.agent_session_manager.return_session(session);
        if let Some(stop_request) = stop_ack {
            stop_request.ack();
        }
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
                    "plan_steps": turn_result.plan_steps,
                    "proposed_plan": turn_result.proposed_plan
                }))
            }
            Err(e) => {
                if e.contains("已收到 /stop") {
                    info!(
                        session_key = %session_key,
                        response = %e,
                        "agent chat api stopped"
                    );
                    return json_response(&serde_json::json!({
                        "success": true,
                        "response": e,
                        "stopped": true,
                        "tool_calls": [],
                        "plan_steps": null
                    }));
                }
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

fn agent_session_events_response(service: &ImGatewayService) -> Response<BoxBody> {
    let connected = futures_util::stream::once(async {
        "retry: 30000\nevent: connected\ndata: {\"eventType\":\"connected\"}\n\n".to_string()
    });
    let updates =
        BroadcastStream::new(service.agent_session_manager.subscribe_session_events()).filter_map(
            |result| async move {
                match result {
                    Ok(event) => {
                        let event_name = event.event_type.clone();
                        let data = serde_json::to_string(&event).ok()?;
                        Some(format!("event: {event_name}\ndata: {data}\n\n"))
                    }
                    Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => {
                        Some(
                            "event: sessions_changed\ndata: {\"eventType\":\"sessions_changed\",\"reason\":\"lagged\"}\n\n"
                                .to_string(),
                        )
                    }
                }
            },
        );
    let body_stream = http_body_util::StreamBody::new(
        connected
            .chain(updates)
            .map(|chunk| Ok::<_, hyper::Error>(hyper::body::Frame::data(Bytes::from(chunk)))),
    );

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(BoxBody::new(body_stream))
        .unwrap()
}

fn consume_imported_contexts_for_builtin_agent(session: &mut bifrost_agent::AgentSession) {
    let contexts = match crate::im_gateway::session_state::take_imported_contexts(
        &session.session_key,
        crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
        None,
    ) {
        Ok(contexts) => contexts,
        Err(error) => {
            warn!(
                session_key = %session.session_key,
                error = %error,
                "failed to consume imported runner contexts for built-in agent"
            );
            Vec::new()
        }
    };
    let Some(rendered) = crate::im_gateway::session_state::render_imported_contexts(&contexts)
    else {
        return;
    };
    session
        .history
        .push(bifrost_agent::ChatMessage::developer(&rendered));
}

fn restore_session_from_history_path(
    session: &mut bifrost_agent::AgentSession,
    history_path: &str,
    expected_session_key: &str,
    max_bytes: Option<usize>,
) -> Result<(), String> {
    let data_dir = bifrost_agent::config::agent_home_dir();
    let path =
        bifrost_agent::persistence::validate_conversation_path(&data_dir, Path::new(history_path))?;
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
    session.memory_cleared = false;
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
            session.current_plan = runtime_state.current_plan;
            session.total_tokens_used = runtime_state.total_tokens_used;
            session.restore_token_snapshot(runtime_state.last_response_tokens);
            session.compaction_count = runtime_state.compaction_count;
            session.resolved_base_instructions = runtime_state.base_instructions;
            bifrost_agent::tools::goal::goal_runtime_apply(
                session,
                bifrost_agent::tools::goal::GoalRuntimeEvent::ThreadResumed,
            );
        }
        Err(error) => {
            warn!(history_path = %path.display(), error = %error, "restored chat history without runtime state");
            session.total_tokens_used = Some(summary.total_tokens);
            session.compaction_count = summary.compaction_count;
        }
    }
    if report.skipped_lines > 0 {
        warn!(
            history_path = %path.display(),
            skipped_lines = report.skipped_lines,
            "restored chat history with skipped JSONL lines"
        );
    }
    session.recorder = Some(ConversationRecorder::from_existing_file(path, max_bytes));
    Ok(())
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
    let resolved_work_dir = config.resolve_work_dir().display().to_string();
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
        obj.insert(
            "resolved_work_dir".to_string(),
            serde_json::Value::String(resolved_work_dir),
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
    if let Some(runner) = patch.get("runner") {
        if runner.is_null() {
            config.runner = None;
        } else if let Ok(value) =
            serde_json::from_value::<bifrost_agent::AgentRunnerMode>(runner.clone())
        {
            config.runner = Some(value);
        }
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
            wire_api: Some(bifrost_agent::ModelWireApi::ChatCompletions),
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

fn dedupe_unified_sessions_by_key(unified: &mut Vec<serde_json::Value>) {
    let mut seen_session_keys = std::collections::HashSet::new();
    unified.retain(|item| {
        let Some(session_key) = item.get("session_key").and_then(|v| v.as_str()) else {
            return true;
        };
        seen_session_keys.insert(session_key.to_string())
    });
}

fn is_runner_call_session_key(session_key: &str) -> bool {
    session_key.starts_with("runner-call:")
}

fn session_detail_response(
    detail: bifrost_agent::SessionDetail,
    queue_manager: &crate::im_gateway::SessionQueueManager,
    active_status: Option<bifrost_agent::ActiveTurnStatus>,
) -> serde_json::Value {
    let mut value = session_detail_with_active_status(detail, active_status);
    insert_queue_snapshot(&mut value, queue_manager);
    value
}

fn session_detail_with_active_status(
    detail: bifrost_agent::SessionDetail,
    active_status: Option<bifrost_agent::ActiveTurnStatus>,
) -> serde_json::Value {
    let Some(status) = active_status else {
        return serde_json::to_value(detail).unwrap_or_else(|_| serde_json::json!({}));
    };
    let mut value = serde_json::to_value(detail).unwrap_or_else(|_| serde_json::json!({}));
    overlay_active_status_on_session_detail(&mut value, status);
    value
}

fn active_status_session_detail(
    status: bifrost_agent::ActiveTurnStatus,
    queue_manager: &crate::im_gateway::SessionQueueManager,
) -> serde_json::Value {
    let active_status = serde_json::to_value(&status).unwrap_or_else(|_| serde_json::json!({}));
    let mut value = serde_json::json!({
        "session_key": status.session_key.clone(),
        "user_id": null,
        "message_count": status.message_count,
        "user_turn_count": status.user_turn_count,
        "created_at": status.started_at,
        "last_active_at": status.updated_at,
        "compaction_count": status.compaction_count,
        "total_tokens_used": status.total_tokens_used,
        "estimated_tokens": status.estimated_context_tokens,
        "context_window_tokens": status.context_window_tokens,
        "context_usage_percent": status.context_usage_percent,
        "last_response_tokens": status.last_response_tokens,
        "history_version": status.history_version,
        "work_dir": status.work_dir.clone(),
        "source": "admin-api",
        "agent_type": status.agent_type.clone(),
        "runner_type": status.runner_type.clone(),
        "runner_id": status.runner_id.clone(),
        "model": status.model.clone(),
        "model_provider": status.model_provider.clone(),
        "external_conversation_id": status.external_conversation_id.clone(),
        "external_thread_id": status.external_thread_id.clone(),
        "title": null,
        "messages": [],
        "goal_status": null,
        "goal_objective": null,
        "history_path": null,
        "has_timeline": false,
        "timeline_event_count": 0,
        "run_state": "running",
        "active_status": active_status,
    });
    insert_queue_snapshot(&mut value, queue_manager);
    value
}

fn active_session_list_run_state(running: bool, session_run_state: &str) -> String {
    if running {
        "running".to_string()
    } else if session_run_state == "running" {
        "completed".to_string()
    } else {
        session_run_state.to_string()
    }
}

fn history_session_list_run_state(
    summary: &bifrost_agent::persistence::SessionFileSummary,
) -> String {
    terminal_run_state_from_summary(summary)
        .unwrap_or("completed")
        .to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PersistedSessionProjection {
    running: bool,
    status: String,
    state: String,
    run_state: String,
    stale_running: bool,
    prefer_history_time: bool,
}

fn persisted_session_projection(
    state: &crate::im_gateway::session_state::ImAgentSessionState,
    history: Option<&bifrost_agent::persistence::SessionFileSummary>,
) -> PersistedSessionProjection {
    let raw_running = state.status.as_deref() == Some("running");
    let history_run_state = history.and_then(terminal_run_state_from_summary);
    if raw_running {
        if let Some(run_state) = history_run_state {
            return terminal_persisted_session_projection(run_state, true, true);
        }
        if state.adapter == crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER {
            return terminal_persisted_session_projection("completed", true, false);
        }
        if state.latest_run_id.is_none() {
            return terminal_persisted_session_projection("completed", true, false);
        }
        return PersistedSessionProjection {
            running: true,
            status: "active".to_string(),
            state: "running".to_string(),
            run_state: "running".to_string(),
            stale_running: false,
            prefer_history_time: false,
        };
    }

    if let Some(run_state) = history_run_state {
        return terminal_persisted_session_projection(run_state, false, true);
    }

    match state.status.as_deref() {
        Some("failed") => terminal_persisted_session_projection("failed", false, false),
        Some("stopped") => terminal_persisted_session_projection("stopped", false, false),
        Some("timed_out") => terminal_persisted_session_projection("timed_out", false, false),
        _ => terminal_persisted_session_projection("completed", false, false),
    }
}

fn terminal_persisted_session_projection(
    run_state: &str,
    stale_running: bool,
    prefer_history_time: bool,
) -> PersistedSessionProjection {
    let run_state = normalize_terminal_run_state(run_state);
    let state = if is_failed_terminal_run_state(run_state) {
        "failed"
    } else {
        "ended"
    };
    PersistedSessionProjection {
        running: false,
        status: "ended".to_string(),
        state: state.to_string(),
        run_state: run_state.to_string(),
        stale_running,
        prefer_history_time,
    }
}

fn terminal_run_state_from_summary(
    summary: &bifrost_agent::persistence::SessionFileSummary,
) -> Option<&str> {
    let run_state = summary.run_state.as_deref()?;
    if is_terminal_run_state(run_state) {
        Some(run_state)
    } else {
        None
    }
}

fn is_terminal_run_state(run_state: &str) -> bool {
    matches!(run_state, "completed" | "failed" | "stopped" | "timed_out")
}

fn is_failed_terminal_run_state(run_state: &str) -> bool {
    matches!(run_state, "failed" | "stopped" | "timed_out")
}

fn normalize_terminal_run_state(run_state: &str) -> &str {
    run_state
}

fn summary_end_time(summary: &bifrost_agent::persistence::SessionFileSummary) -> u64 {
    if summary.end_time > 0 {
        summary.end_time
    } else {
        summary.start_time
    }
}

fn repair_stale_persisted_running_states(session_key: &str) -> usize {
    let mut repaired = 0;
    for state in crate::im_gateway::session_state::list_session_states()
        .into_iter()
        .filter(|state| state.session_key == session_key)
        .filter(|state| state.status.as_deref() == Some("running"))
    {
        let history_summary = persisted_state_history_summary(&state);
        let projection = persisted_session_projection(&state, history_summary.as_ref());
        if !projection.stale_running {
            continue;
        }
        let next_status = status_from_run_state(&projection.run_state).to_string();
        match crate::im_gateway::session_state::upsert_session_state(
            &state.session_key,
            &state.adapter,
            state.runner_id.as_deref(),
            |stored| {
                stored.status = Some(next_status.clone());
            },
        ) {
            Ok(_) => repaired += 1,
            Err(error) => {
                warn!(
                    session_key = %state.session_key,
                    adapter = %state.adapter,
                    runner_id = ?state.runner_id,
                    error = %error,
                    "failed to repair stale persisted Agent running state"
                );
            }
        }
    }
    repaired
}

fn persisted_state_history_summary(
    state: &crate::im_gateway::session_state::ImAgentSessionState,
) -> Option<bifrost_agent::persistence::SessionFileSummary> {
    let data_dir = bifrost_agent::config::agent_home_dir();
    let path = state.history_path.as_deref().and_then(|path| {
        bifrost_agent::persistence::validate_conversation_path(
            &data_dir,
            std::path::Path::new(path),
        )
        .ok()
    });
    path.map(|path| bifrost_agent::persistence::scan_session_summary(&path))
}

fn status_from_run_state(run_state: &str) -> &'static str {
    match run_state {
        "failed" => "failed",
        "stopped" => "stopped",
        "timed_out" => "timed_out",
        _ => "ended",
    }
}

fn prime_builtin_worker_active_status(
    manager: &bifrost_agent::AgentSessionManager,
    session: &bifrost_agent::AgentSession,
    config: &bifrost_agent::AgentConfig,
) {
    let Some(mut status) = manager.get_active_turn_status(&session.session_key) else {
        return;
    };
    let estimated_context_tokens = session.effective_token_count();
    let context_window_tokens = config
        .model_context_window
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0);
    status.state = "running".to_string();
    status.current_loop_iteration = status.current_loop_iteration.max(1);
    status.max_loop_iterations = config.get_max_turn_iterations();
    status.last_response_tokens = session.last_response_tokens;
    status.total_tokens_used = session.total_tokens_used;
    status.estimated_context_tokens = estimated_context_tokens;
    status.context_window_tokens = context_window_tokens;
    status.context_usage_percent = context_window_tokens
        .map(|window| ((estimated_context_tokens as f64 / window as f64) * 1000.0).round() / 10.0);
    status.compaction_count = session.compaction_count;
    status.history_version = session.history_version;
    status.work_dir = session.work_dir.clone();
    status.message_count = session.history.len();
    status.user_turn_count = session.user_turn_count();
    status.agent_type = session.agent_type.clone();
    status.runner_type = session.runner_type.clone();
    status.runner_id = session.runner_id.clone();
    manager.update_active_turn_status_from_worker(status);
}

fn insert_queue_snapshot(
    value: &mut serde_json::Value,
    queue_manager: &crate::im_gateway::SessionQueueManager,
) {
    let Some(session_key) = value.get("session_key").and_then(|value| value.as_str()) else {
        return;
    };
    let queue_snapshot =
        crate::handlers::agent_chat::queue_snapshot_payload(queue_manager, session_key);
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "queue_length".to_string(),
            queue_snapshot["queueLength"].clone(),
        );
        object.insert(
            "queue_items".to_string(),
            queue_snapshot["queueItems"].clone(),
        );
        object.insert(
            "queueLength".to_string(),
            queue_snapshot["queueLength"].clone(),
        );
        object.insert(
            "queueItems".to_string(),
            queue_snapshot["queueItems"].clone(),
        );
    }
}

fn overlay_active_status_on_session_detail(
    value: &mut serde_json::Value,
    status: bifrost_agent::ActiveTurnStatus,
) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let active_status = serde_json::to_value(&status).unwrap_or_else(|_| serde_json::json!({}));
    object.insert("run_state".to_string(), serde_json::json!("running"));
    object.insert(
        "message_count".to_string(),
        serde_json::json!(status.message_count),
    );
    object.insert(
        "user_turn_count".to_string(),
        serde_json::json!(status.user_turn_count),
    );
    object.insert(
        "last_active_at".to_string(),
        serde_json::json!(status.updated_at),
    );
    object.insert(
        "compaction_count".to_string(),
        serde_json::json!(status.compaction_count),
    );
    object.insert(
        "total_tokens_used".to_string(),
        serde_json::json!(status.total_tokens_used),
    );
    object.insert(
        "estimated_tokens".to_string(),
        serde_json::json!(status.estimated_context_tokens),
    );
    object.insert(
        "context_window_tokens".to_string(),
        serde_json::json!(status.context_window_tokens),
    );
    object.insert(
        "context_usage_percent".to_string(),
        serde_json::json!(status.context_usage_percent),
    );
    object.insert(
        "last_response_tokens".to_string(),
        serde_json::json!(status.last_response_tokens),
    );
    object.insert(
        "history_version".to_string(),
        serde_json::json!(status.history_version),
    );
    if status.work_dir.is_some() {
        object.insert(
            "work_dir".to_string(),
            serde_json::json!(status.work_dir.clone()),
        );
    }
    if status.agent_type.is_some() {
        object.insert(
            "agent_type".to_string(),
            serde_json::json!(status.agent_type.clone()),
        );
    }
    if status.runner_type.is_some() {
        object.insert(
            "runner_type".to_string(),
            serde_json::json!(status.runner_type.clone()),
        );
    }
    if status.runner_id.is_some() {
        object.insert(
            "runner_id".to_string(),
            serde_json::json!(status.runner_id.clone()),
        );
    }
    if status.model.is_some() {
        object.insert("model".to_string(), serde_json::json!(status.model.clone()));
    }
    if status.model_provider.is_some() {
        object.insert(
            "model_provider".to_string(),
            serde_json::json!(status.model_provider.clone()),
        );
    }
    if status.external_conversation_id.is_some() {
        object.insert(
            "external_conversation_id".to_string(),
            serde_json::json!(status.external_conversation_id.clone()),
        );
    }
    if status.external_thread_id.is_some() {
        object.insert(
            "external_thread_id".to_string(),
            serde_json::json!(status.external_thread_id.clone()),
        );
    }
    object.insert("active_status".to_string(), active_status);
}

fn history_session_detail(session_key: &str) -> Option<bifrost_agent::SessionDetail> {
    let data_dir = bifrost_agent::config::agent_home_dir();
    let files = bifrost_agent::persistence::list_conversations(&data_dir, Some(session_key));
    let path = files
        .into_iter()
        .max_by_key(|path| bifrost_agent::persistence::scan_session_summary(path).end_time)?;
    let report = bifrost_agent::persistence::load_conversation_lossy(&path).ok()?;
    let summary = bifrost_agent::persistence::scan_session_summary(&path);
    let runtime_state = bifrost_agent::persistence::load_session_runtime_state(&path).ok();
    let mut message_timestamps: std::collections::VecDeque<u64> =
        bifrost_agent::persistence::load_conversation_events(&path)
            .ok()
            .map(|events| {
                events
                    .into_iter()
                    .filter(|event| {
                        event.event_type == bifrost_agent::persistence::event_types::USER_MESSAGE
                            || event.event_type
                                == bifrost_agent::persistence::event_types::ASSISTANT_MESSAGE
                    })
                    .map(|event| event.timestamp)
                    .collect()
            })
            .unwrap_or_default();
    let messages: Vec<bifrost_agent::session::SessionMessage> = report
        .messages
        .iter()
        .map(|message| bifrost_agent::session::SessionMessage {
            role: message.role.clone(),
            content: message.content.clone().unwrap_or_default(),
            timestamp: message_timestamps.pop_front(),
            content_parts: message
                .content_parts
                .as_ref()
                .and_then(|parts| serde_json::to_value(parts).ok()),
            tool_calls: message.tool_calls.as_ref().map(|calls| {
                calls
                    .iter()
                    .map(|call| serde_json::to_string(call).unwrap_or_else(|_| format!("{call:?}")))
                    .collect()
            }),
        })
        .collect();
    let resolved_key = summary
        .session_key
        .clone()
        .unwrap_or_else(|| session_key.to_string());
    let is_external_runner =
        is_external_runner_metadata(summary.runner_type.as_deref(), summary.runner_id.as_deref());
    Some(bifrost_agent::SessionDetail {
        session_key: resolved_key,
        user_id: None,
        message_count: messages.len(),
        user_turn_count: summary.user_turns as usize,
        created_at: summary.start_time,
        last_active_at: summary.end_time,
        compaction_count: runtime_state
            .as_ref()
            .map(|state| state.compaction_count)
            .unwrap_or(summary.compaction_count),
        total_tokens_used: runtime_state
            .as_ref()
            .and_then(|state| state.total_tokens_used)
            .or_else(|| (summary.total_tokens > 0).then_some(summary.total_tokens)),
        estimated_tokens: runtime_state
            .as_ref()
            .and_then(|state| state.last_response_tokens)
            .and_then(|tokens| u32::try_from(tokens).ok())
            .unwrap_or_default(),
        history_version: 0,
        work_dir: summary.work_dir,
        source: summary.source,
        agent_type: is_external_runner.then(|| "external_cli".to_string()),
        runner_type: summary.runner_type,
        runner_id: summary.runner_id,
        model: None,
        model_provider: None,
        external_conversation_id: None,
        external_thread_id: None,
        title: summary.title.or(summary.first_user_message),
        messages,
        goal_status: runtime_state
            .as_ref()
            .and_then(|state| state.current_goal.as_ref())
            .map(|goal| format!("{:?}", goal.status)),
        goal_objective: runtime_state
            .and_then(|state| state.current_goal.map(|goal| goal.objective)),
        history_path: Some(path.display().to_string()),
        has_timeline: true,
        timeline_event_count: summary.event_count as usize,
        run_state: summary.run_state.unwrap_or_else(|| "completed".to_string()),
    })
}

async fn external_runner_session_detail(session_key: &str) -> Option<bifrost_agent::SessionDetail> {
    let state =
        crate::im_gateway::session_state::load_latest_session_state(session_key, None, None)?;
    let run_detail = match state.latest_run_id.as_deref() {
        Some(run_id) => {
            match crate::im_gateway::external_cli::read_run_detail(
                crate::im_gateway::external_cli::default_runs_root(),
                run_id,
            )
            .await
            {
                Ok(detail) => Some(detail),
                Err(error) => {
                    warn!(
                        session_key = %session_key,
                        run_id = %run_id,
                        error = %error,
                        "failed to load external runner detail from latest run"
                    );
                    None
                }
            }
        }
        None => None,
    };
    let history_path = state.history_path.clone();
    let timeline_detail = history_path
        .as_deref()
        .and_then(external_runner_timeline_messages);
    let (mut messages, timeline_event_count) =
        if let Some((messages, event_count)) = timeline_detail {
            (messages, event_count)
        } else {
            (Vec::new(), 0)
        };
    if messages.is_empty() && state.messages.is_empty() {
        if let Some(content) = state
            .last_user_message
            .clone()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            messages.push(bifrost_agent::session::SessionMessage {
                role: "user".to_string(),
                content,
                timestamp: Some(state.updated_at / 1000),
                content_parts: None,
                tool_calls: None,
            });
        }
        let response = run_detail
            .as_ref()
            .map(|detail| detail.response.clone())
            .or_else(|| state.last_response.clone())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if let Some(content) = response {
            messages.push(bifrost_agent::session::SessionMessage {
                role: "assistant".to_string(),
                content,
                timestamp: Some(state.updated_at / 1000),
                content_parts: None,
                tool_calls: None,
            });
        }
    } else if messages.is_empty() {
        messages.extend(state.messages.iter().map(|message| {
            bifrost_agent::session::SessionMessage {
                role: message.role.clone(),
                content: message.content.clone(),
                timestamp: message.timestamp,
                content_parts: message.content_parts.clone(),
                tool_calls: None,
            }
        }));
    }
    let updated_at = state.updated_at / 1000;
    let created_at = messages
        .iter()
        .filter_map(|message| message.timestamp)
        .min()
        .unwrap_or(updated_at);
    let user_turn_count = messages
        .iter()
        .filter(|message| message.role == "user")
        .count();
    let is_builtin_agent = state.adapter == crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER;
    let run_state = match state.status.as_deref() {
        Some("running") => "running",
        Some("failed" | "stopped" | "timed_out") => "failed",
        _ => "completed",
    }
    .to_string();
    let model = run_detail
        .as_ref()
        .and_then(|detail| detail.metadata.get("model").cloned());
    let model_provider = run_detail
        .as_ref()
        .and_then(|detail| detail.metadata.get("modelSource").cloned());
    let usage_total_tokens = run_detail
        .as_ref()
        .and_then(|detail| detail.metadata.get("usageTotalTokens"))
        .and_then(|value| value.trim().parse::<u64>().ok());
    let estimated_tokens = run_detail
        .as_ref()
        .and_then(|detail| detail.metadata.get("usageInputTokens"))
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or_default();
    Some(bifrost_agent::SessionDetail {
        session_key: state.session_key,
        user_id: None,
        message_count: messages.len(),
        user_turn_count,
        created_at,
        last_active_at: updated_at,
        compaction_count: 0,
        total_tokens_used: usage_total_tokens,
        estimated_tokens,
        history_version: 0,
        work_dir: state.work_dir,
        source: "admin-api".to_string(),
        agent_type: Some(if is_builtin_agent {
            "Bifrost Agent".to_string()
        } else {
            "external_cli".to_string()
        }),
        runner_type: Some(state.adapter),
        runner_id: state.runner_id,
        model,
        model_provider,
        external_conversation_id: state.external_conversation_id,
        external_thread_id: state.external_thread_id,
        title: state.title.or(state.last_user_message),
        messages,
        goal_status: None,
        goal_objective: None,
        has_timeline: history_path.is_some(),
        history_path,
        timeline_event_count,
        run_state,
    })
}

fn external_runner_timeline_messages(
    history_path: &str,
) -> Option<(Vec<bifrost_agent::session::SessionMessage>, usize)> {
    let data_dir = bifrost_agent::config::agent_home_dir();
    let path = bifrost_agent::persistence::validate_conversation_path(
        &data_dir,
        std::path::Path::new(history_path),
    )
    .ok()?;
    let report = bifrost_agent::persistence::load_conversation_lossy(&path).ok()?;
    if report.messages.is_empty() {
        return None;
    }
    let events = bifrost_agent::persistence::load_conversation_events(&path).ok();
    let mut message_timestamps: std::collections::VecDeque<u64> = events
        .as_ref()
        .map(|events| {
            events
                .iter()
                .filter(|event| {
                    event.event_type == bifrost_agent::persistence::event_types::USER_MESSAGE
                        || event.event_type
                            == bifrost_agent::persistence::event_types::ASSISTANT_MESSAGE
                })
                .map(|event| event.timestamp)
                .collect()
        })
        .unwrap_or_default();
    let messages = report
        .messages
        .iter()
        .map(|message| bifrost_agent::session::SessionMessage {
            role: message.role.clone(),
            content: message.content.clone().unwrap_or_default(),
            timestamp: message_timestamps.pop_front(),
            content_parts: message
                .content_parts
                .as_ref()
                .and_then(|parts| serde_json::to_value(parts).ok()),
            tool_calls: message.tool_calls.as_ref().map(|calls| {
                calls
                    .iter()
                    .map(|call| serde_json::to_string(call).unwrap_or_else(|_| format!("{call:?}")))
                    .collect()
            }),
        })
        .collect();
    Some((
        messages,
        events.map(|events| events.len()).unwrap_or_default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    static AGENT_API_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn sessions_all_dedupes_by_session_key_after_sorting() {
        let mut sessions = vec![
            serde_json::json!({
                "session_key": "same-thread",
                "status": "ended",
                "last_active_time": 200,
                "history_path": "/tmp/newer.jsonl",
            }),
            serde_json::json!({
                "session_key": "same-thread",
                "status": "ended",
                "last_active_time": 100,
                "history_path": "/tmp/older.jsonl",
            }),
            serde_json::json!({
                "session_key": "other-thread",
                "status": "ended",
                "last_active_time": 50,
            }),
        ];

        dedupe_unified_sessions_by_key(&mut sessions);

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0]["session_key"], "same-thread");
        assert_eq!(sessions[0]["history_path"], "/tmp/newer.jsonl");
        assert_eq!(sessions[1]["session_key"], "other-thread");
    }

    #[test]
    fn sessions_all_idle_active_session_does_not_inherit_running_history_state() {
        let session = bifrost_agent::SessionInfo {
            session_key: "idle-active-with-running-history".to_string(),
            running: false,
            user_id: None,
            message_count: 1,
            user_turn_count: 1,
            created_at: 10,
            last_active_at: 20,
            compaction_count: 0,
            total_tokens_used: None,
            estimated_tokens: 0,
            history_version: 1,
            work_dir: None,
            source: "web".to_string(),
            agent_type: Some("Bifrost Agent".to_string()),
            runner_type: Some("bifrost_agent".to_string()),
            runner_id: None,
            external_conversation_id: None,
            external_thread_id: None,
            title: Some("Idle active".to_string()),
            history_path: Some("/tmp/history.jsonl".to_string()),
            has_timeline: true,
            timeline_event_count: 10,
            run_state: "idle".to_string(),
        };
        let _history = summary_with_run_state("running", 10, 20);

        assert_eq!(
            active_session_list_run_state(session.running, &session.run_state),
            "idle"
        );
        assert_eq!(active_session_list_run_state(false, "running"), "completed");
    }

    #[test]
    fn sessions_all_history_row_projects_non_terminal_run_state_to_completed() {
        let running_summary = summary_with_run_state("running", 10, 20);
        let waiting_summary = summary_with_run_state("waiting_for_tool", 10, 20);
        let completed_summary = summary_with_run_state("completed", 10, 20);

        assert_eq!(
            history_session_list_run_state(&running_summary),
            "completed"
        );
        assert_eq!(
            history_session_list_run_state(&waiting_summary),
            "completed"
        );
        assert_eq!(
            history_session_list_run_state(&completed_summary),
            "completed"
        );
    }

    #[test]
    fn runner_call_child_session_keys_are_internal() {
        assert!(is_runner_call_session_key(
            "runner-call:admin-chat-1:bifrost_agent"
        ));
        assert!(!is_runner_call_session_key("admin-chat-1"));
    }

    #[test]
    fn session_detail_overlay_uses_active_status_token_context_metrics() {
        let detail = bifrost_agent::SessionDetail {
            session_key: "active-token-sync".to_string(),
            user_id: None,
            message_count: 10,
            user_turn_count: 5,
            created_at: 1,
            last_active_at: 2,
            compaction_count: 1,
            total_tokens_used: Some(19_503_264),
            estimated_tokens: 23_448,
            history_version: 3,
            work_dir: Some("/tmp/old".to_string()),
            source: "feishu".to_string(),
            agent_type: None,
            runner_type: None,
            runner_id: None,
            model: None,
            model_provider: None,
            external_conversation_id: None,
            external_thread_id: None,
            title: Some("history title".to_string()),
            messages: Vec::new(),
            goal_status: None,
            goal_objective: None,
            history_path: Some("/tmp/history.jsonl".to_string()),
            has_timeline: true,
            timeline_event_count: 100,
            run_state: "completed".to_string(),
        };
        let status = bifrost_agent::ActiveTurnStatus {
            session_key: "active-token-sync".to_string(),
            state: "waiting_on_session".to_string(),
            started_at: 10,
            updated_at: 20,
            current_loop_iteration: 97,
            completed_loop_iterations: 96,
            max_loop_iterations: 1000,
            last_response_tokens: Some(181_900),
            total_tokens_used: Some(29_668_709),
            estimated_context_tokens: 186_727,
            context_window_tokens: Some(250_000),
            context_usage_percent: Some(74.7),
            compaction_count: 2,
            history_version: 11,
            work_dir: Some("/tmp/live".to_string()),
            message_count: 204,
            local_tool_count: 1,
            mcp_tool_count: 2,
            pending_guide_messages: Vec::new(),
            user_turn_count: 50,
            agent_type: Some("Bifrost Agent".to_string()),
            runner_type: Some("bifrost_agent".to_string()),
            runner_id: None,
            model: Some("gpt-5".to_string()),
            model_provider: Some("openai".to_string()),
            external_conversation_id: None,
            external_thread_id: None,
            turn_timing: None,
            turn_id: None,
        };

        let value = session_detail_with_active_status(detail, Some(status));

        assert_eq!(value["run_state"], "running");
        assert_eq!(value["total_tokens_used"], 29_668_709);
        assert_eq!(value["estimated_tokens"], 186_727);
        assert_eq!(value["context_window_tokens"], 250_000);
        assert_eq!(value["context_usage_percent"], 74.7);
        assert_eq!(value["last_response_tokens"], 181_900);
        assert_eq!(value["active_status"]["estimated_context_tokens"], 186_727);
        assert_eq!(value["history_path"], "/tmp/history.jsonl");
    }

    #[test]
    fn prime_builtin_worker_active_status_publishes_running_snapshot() {
        let manager = bifrost_agent::AgentSessionManager::new(3600);
        let mut session = manager.take_session_with_work_dir(
            "worker-prime-status",
            Some("/tmp/worker-prime-status".to_string()),
        );
        session.mark_bifrost_agent_runtime();
        session
            .history
            .push(bifrost_agent::ChatMessage::user("hello"));

        let config = bifrost_agent::AgentConfig {
            max_turn_iterations: Some(8),
            ..Default::default()
        };

        prime_builtin_worker_active_status(&manager, &session, &config);

        let status = manager
            .get_active_turn_status("worker-prime-status")
            .expect("active status");
        assert_eq!(status.state, "running");
        assert_eq!(status.current_loop_iteration, 1);
        assert_eq!(status.max_loop_iterations, 8);
        assert_eq!(status.work_dir.as_deref(), Some("/tmp/worker-prime-status"));
        assert_eq!(status.context_window_tokens, Some(250_000));
        assert_eq!(status.agent_type.as_deref(), Some("Bifrost Agent"));
        assert_eq!(status.runner_type.as_deref(), Some("bifrost_agent"));
        assert_eq!(status.user_turn_count, 1);
    }

    fn persisted_state(
        session_key: &str,
        adapter: &str,
        status: &str,
    ) -> crate::im_gateway::session_state::ImAgentSessionState {
        crate::im_gateway::session_state::ImAgentSessionState {
            session_key: session_key.to_string(),
            adapter: adapter.to_string(),
            status: Some(status.to_string()),
            ..crate::im_gateway::session_state::ImAgentSessionState::default()
        }
    }

    fn summary_with_run_state(
        run_state: &str,
        start_time: u64,
        end_time: u64,
    ) -> bifrost_agent::persistence::SessionFileSummary {
        bifrost_agent::persistence::SessionFileSummary {
            start_time,
            end_time,
            event_count: 4,
            source: "admin-api".to_string(),
            run_state: Some(run_state.to_string()),
            ..bifrost_agent::persistence::SessionFileSummary::default()
        }
    }

    #[test]
    fn stale_running_builtin_with_terminal_history_projects_completed() {
        let state = persisted_state(
            "stale-builtin-terminal",
            crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
            "running",
        );
        let summary = summary_with_run_state("completed", 10, 20);

        let projection = persisted_session_projection(&state, Some(&summary));

        assert!(!projection.running);
        assert_eq!(projection.status, "ended");
        assert_eq!(projection.state, "ended");
        assert_eq!(projection.run_state, "completed");
        assert!(projection.stale_running);
        assert!(projection.prefer_history_time);
    }

    #[test]
    fn stale_running_builtin_without_live_runtime_projects_completed() {
        let state = persisted_state(
            "stale-builtin-no-history",
            crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER,
            "running",
        );

        let projection = persisted_session_projection(&state, None);

        assert!(!projection.running);
        assert_eq!(projection.status, "ended");
        assert_eq!(projection.state, "ended");
        assert_eq!(projection.run_state, "completed");
        assert!(projection.stale_running);
        assert!(!projection.prefer_history_time);
    }

    #[test]
    fn external_runners_with_run_id_without_terminal_history_stay_running_for_remote_semantics() {
        for adapter in ["codex", "chatgpt_web", "custom_cli"] {
            let mut state = persisted_state("external-running", adapter, "running");
            state.latest_run_id = Some("run-123".to_string());

            let projection = persisted_session_projection(&state, None);

            assert!(projection.running, "{adapter}");
            assert_eq!(projection.status, "active", "{adapter}");
            assert_eq!(projection.state, "running", "{adapter}");
            assert_eq!(projection.run_state, "running", "{adapter}");
            assert!(!projection.stale_running, "{adapter}");
        }
    }

    #[test]
    fn external_runners_without_run_id_project_stale_running_to_completed() {
        for adapter in ["codex", "chatgpt_web", "custom_cli"] {
            let state = persisted_state("external-stale-running", adapter, "running");

            let projection = persisted_session_projection(&state, None);

            assert!(!projection.running, "{adapter}");
            assert_eq!(projection.status, "ended", "{adapter}");
            assert_eq!(projection.state, "ended", "{adapter}");
            assert_eq!(projection.run_state, "completed", "{adapter}");
            assert!(projection.stale_running, "{adapter}");
        }
    }

    #[test]
    fn external_running_with_terminal_history_projects_completed() {
        let state = persisted_state("external-terminal", "codex", "running");
        let summary = summary_with_run_state("completed", 10, 20);

        let projection = persisted_session_projection(&state, Some(&summary));

        assert!(!projection.running);
        assert_eq!(projection.status, "ended");
        assert_eq!(projection.state, "ended");
        assert_eq!(projection.run_state, "completed");
        assert!(projection.stale_running);
        assert!(projection.prefer_history_time);
    }

    #[test]
    fn external_running_without_explicit_history_path_ignores_unlinked_terminal_history() {
        let _lock = AGENT_API_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = AgentApiEnvGuard::new(dir.path());
        let session_key = "external-unlinked-terminal-history";
        let data_dir = bifrost_agent::config::agent_home_dir();
        let mut recorder =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, session_key);
        recorder
            .record_session_start(session_key, serde_json::json!({"source": "web"}))
            .expect("record start");
        recorder
            .record_user_message(session_key, "old unlinked user")
            .expect("record user");
        recorder
            .record_assistant_message(session_key, "old unlinked assistant")
            .expect("record assistant");
        recorder
            .record_run_state(session_key, "completed", Some("test"), Some("external"))
            .expect("record run state");

        let mut state = persisted_state(session_key, "codex", "running");
        state.latest_run_id = Some("run-current".to_string());
        let summary = persisted_state_history_summary(&state);

        assert!(
            summary.is_none(),
            "unlinked terminal history must not be discovered by session_key fallback"
        );
        let projection = persisted_session_projection(&state, summary.as_ref());
        assert!(projection.running);
        assert_eq!(projection.status, "active");
        assert_eq!(projection.state, "running");
        assert_eq!(projection.run_state, "running");
        assert!(!projection.stale_running);
    }

    struct AgentApiEnvGuard {
        _guard: crate::test_env::BifrostDataDirGuard,
    }

    impl AgentApiEnvGuard {
        fn new(data_dir: &std::path::Path) -> Self {
            Self {
                _guard: crate::test_env::BifrostDataDirGuard::set(data_dir),
            }
        }
    }

    #[test]
    fn external_runner_session_detail_uses_persisted_state() {
        let _lock = AGENT_API_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = AgentApiEnvGuard::new(dir.path());

        crate::im_gateway::session_state::remember_session_state(
            crate::im_gateway::session_state::ImAgentSessionState {
                session_key: "codex-ui-session".to_string(),
                adapter: "codex".to_string(),
                runner_id: Some("codex".to_string()),
                title: Some("Codex task".to_string()),
                last_user_message: Some("Codex task".to_string()),
                last_response: Some("Codex answer".to_string()),
                status: Some("succeeded".to_string()),
                updated_at: 1_770_000_000_000,
                ..crate::im_gateway::session_state::ImAgentSessionState::default()
            },
        )
        .expect("remember state");

        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let detail = runtime
            .block_on(external_runner_session_detail("codex-ui-session"))
            .expect("detail");
        assert_eq!(detail.session_key, "codex-ui-session");
        assert_eq!(detail.runner_type.as_deref(), Some("codex"));
        assert_eq!(detail.runner_id.as_deref(), Some("codex"));
        assert_eq!(detail.title.as_deref(), Some("Codex task"));
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[0].role, "user");
        assert_eq!(detail.messages[0].content, "Codex task");
        assert_eq!(detail.messages[1].role, "assistant");
        assert_eq!(detail.messages[1].content, "Codex answer");
    }

    #[test]
    fn stale_running_repair_marks_builtin_state_ended() {
        let _lock = AGENT_API_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = AgentApiEnvGuard::new(dir.path());
        let session_key = "stale-running-repair";
        let data_dir = bifrost_agent::config::agent_home_dir();
        let mut recorder =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, session_key);
        recorder
            .record_session_start(session_key, serde_json::json!({"source": "admin-api"}))
            .expect("record start");
        recorder
            .record_user_message(session_key, "repair me")
            .expect("record user");
        recorder
            .record_assistant_message(session_key, "done")
            .expect("record assistant");
        recorder
            .record_run_state(session_key, "completed", Some("test"), Some("builtin"))
            .expect("record run state");
        let history_path = recorder.file_path().display().to_string();

        crate::im_gateway::session_state::remember_session_state(
            crate::im_gateway::session_state::ImAgentSessionState {
                session_key: session_key.to_string(),
                adapter: crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER.to_string(),
                title: Some("Stale repair".to_string()),
                last_user_message: Some("repair me".to_string()),
                history_path: Some(history_path),
                status: Some("running".to_string()),
                updated_at: 1_780_274_116_096,
                ..crate::im_gateway::session_state::ImAgentSessionState::default()
            },
        )
        .expect("remember state");

        assert_eq!(repair_stale_persisted_running_states(session_key), 1);

        let repaired =
            crate::im_gateway::session_state::load_latest_session_state(session_key, None, None)
                .expect("repaired state");
        assert_eq!(repaired.status.as_deref(), Some("ended"));
    }

    #[test]
    fn external_runner_session_detail_prefers_canonical_timeline_messages() {
        let _lock = AGENT_API_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = AgentApiEnvGuard::new(dir.path());
        let session_key = "external-canonical-detail";
        let data_dir = bifrost_agent::config::agent_home_dir();
        let mut recorder =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, session_key);
        recorder
            .record_session_start(session_key, serde_json::json!({"source": "web"}))
            .expect("record start");
        recorder
            .record_user_message(session_key, "canonical user")
            .expect("record user");
        recorder
            .record_assistant_message(session_key, "canonical assistant")
            .expect("record assistant");
        let history_path = recorder.file_path().display().to_string();

        crate::im_gateway::session_state::remember_session_state(
            crate::im_gateway::session_state::ImAgentSessionState {
                session_key: session_key.to_string(),
                adapter: "chatgpt_web".to_string(),
                runner_id: Some("gpt".to_string()),
                title: Some("Canonical detail".to_string()),
                last_user_message: Some("stale user".to_string()),
                last_response: Some("stale assistant".to_string()),
                history_path: Some(history_path),
                status: Some("succeeded".to_string()),
                messages: vec![
                    crate::im_gateway::session_state::ImAgentSessionMessage {
                        role: "user".to_string(),
                        content: "stale user".to_string(),
                        timestamp: Some(1),
                        content_parts: None,
                    },
                    crate::im_gateway::session_state::ImAgentSessionMessage {
                        role: "assistant".to_string(),
                        content: "stale assistant".to_string(),
                        timestamp: Some(2),
                        content_parts: None,
                    },
                ],
                updated_at: 1_770_000_000_000,
                ..crate::im_gateway::session_state::ImAgentSessionState::default()
            },
        )
        .expect("remember state");

        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let detail = runtime
            .block_on(external_runner_session_detail(session_key))
            .expect("detail");

        assert_eq!(detail.runner_type.as_deref(), Some("chatgpt_web"));
        assert_eq!(detail.runner_id.as_deref(), Some("gpt"));
        assert_eq!(detail.message_count, 2);
        assert_eq!(detail.messages[0].content, "canonical user");
        assert_eq!(detail.messages[1].content, "canonical assistant");
        assert!(detail.timeline_event_count >= 3);
    }

    #[test]
    fn external_runner_session_detail_preserves_five_turns_in_one_thread() {
        let _lock = AGENT_API_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = AgentApiEnvGuard::new(dir.path());

        let mut messages = Vec::new();
        for turn in 1..=5 {
            messages.push(crate::im_gateway::session_state::ImAgentSessionMessage {
                role: "user".to_string(),
                content: format!("ChatGPT Web round {turn}"),
                timestamp: Some(1_770_000_000 + turn * 2),
                content_parts: None,
            });
            messages.push(crate::im_gateway::session_state::ImAgentSessionMessage {
                role: "assistant".to_string(),
                content: format!("ChatGPT Web answer {turn}"),
                timestamp: Some(1_770_000_000 + turn * 2 + 1),
                content_parts: None,
            });
        }

        crate::im_gateway::session_state::remember_session_state(
            crate::im_gateway::session_state::ImAgentSessionState {
                session_key: "chatgpt-web-five-turns".to_string(),
                adapter: "chatgpt_web".to_string(),
                runner_id: Some("web".to_string()),
                external_conversation_id: Some("conversation-123".to_string()),
                title: Some("ChatGPT Web round 1".to_string()),
                last_user_message: Some("ChatGPT Web round 5".to_string()),
                last_response: Some("ChatGPT Web answer 5".to_string()),
                status: Some("succeeded".to_string()),
                messages,
                updated_at: 1_770_000_020_000,
                ..crate::im_gateway::session_state::ImAgentSessionState::default()
            },
        )
        .expect("remember state");

        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let detail = runtime
            .block_on(external_runner_session_detail("chatgpt-web-five-turns"))
            .expect("detail");

        assert_eq!(detail.session_key, "chatgpt-web-five-turns");
        assert_eq!(detail.runner_type.as_deref(), Some("chatgpt_web"));
        assert_eq!(
            detail.external_conversation_id.as_deref(),
            Some("conversation-123")
        );
        assert_eq!(detail.message_count, 10);
        assert_eq!(detail.user_turn_count, 5);
        assert_eq!(detail.created_at, 1_770_000_002);
        assert_eq!(detail.last_active_at, 1_770_000_020);
        assert_eq!(detail.messages[0].content, "ChatGPT Web round 1");
        assert_eq!(detail.messages[9].content, "ChatGPT Web answer 5");
    }

    #[test]
    fn session_detail_prefers_latest_builtin_turn_state_over_stale_external_state() {
        let _lock = AGENT_API_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = AgentApiEnvGuard::new(dir.path());

        crate::im_gateway::session_state::remember_session_state(
            crate::im_gateway::session_state::ImAgentSessionState {
                session_key: "shared-thread".to_string(),
                adapter: "chatgpt_web".to_string(),
                runner_id: Some("web".to_string()),
                title: Some("Old web title".to_string()),
                last_user_message: Some("old web prompt".to_string()),
                last_response: Some("old web answer".to_string()),
                status: Some("succeeded".to_string()),
                updated_at: 1_770_000_000_000,
                ..crate::im_gateway::session_state::ImAgentSessionState::default()
            },
        )
        .expect("remember stale state");
        crate::im_gateway::session_state::remember_session_state(
            crate::im_gateway::session_state::ImAgentSessionState {
                session_key: "shared-thread".to_string(),
                adapter: crate::im_gateway::session_state::BUILTIN_AGENT_ADAPTER.to_string(),
                title: Some("Old web title".to_string()),
                last_user_message: Some("new builtin prompt".to_string()),
                status: Some("running".to_string()),
                updated_at: 1_770_000_001_000,
                ..crate::im_gateway::session_state::ImAgentSessionState::default()
            },
        )
        .expect("remember latest state");

        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let detail = runtime
            .block_on(external_runner_session_detail("shared-thread"))
            .expect("detail");
        assert_eq!(detail.runner_type.as_deref(), Some("bifrost_agent"));
        assert_eq!(detail.agent_type.as_deref(), Some("Bifrost Agent"));
        assert_eq!(detail.messages.len(), 1);
        assert_eq!(detail.messages[0].content, "new builtin prompt");
    }

    #[test]
    fn history_session_detail_uses_server_side_title_fallback() {
        let _lock = AGENT_API_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = AgentApiEnvGuard::new(dir.path());
        let sessions_dir = dir.path().join("agent/sessions/2026/05/25");
        std::fs::create_dir_all(&sessions_dir).expect("sessions dir");
        std::fs::write(
            sessions_dir.join("session-history-title-1779690000.jsonl"),
            r#"{"timestamp":1,"event_type":"session_start","session_key":"history-title","content":{"source":"admin-api","work_dir":"/tmp/work"}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"user_message","session_key":"history-title","content":{"message":"first user title"}}"#
                + "\n"
                + r#"{"timestamp":3,"event_type":"assistant_message","session_key":"history-title","content":{"message":"answer"}}"#
                + "\n",
        )
        .expect("write jsonl");

        let detail = history_session_detail("history-title").expect("history detail");
        assert_eq!(detail.title.as_deref(), Some("first user title"));
        assert_eq!(detail.message_count, 2);
        assert_eq!(detail.work_dir.as_deref(), Some("/tmp/work"));
    }

    #[test]
    fn history_session_detail_prefers_explicit_title() {
        let _lock = AGENT_API_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = AgentApiEnvGuard::new(dir.path());
        let sessions_dir = dir.path().join("agent/sessions/2026/05/25");
        std::fs::create_dir_all(&sessions_dir).expect("sessions dir");
        std::fs::write(
            sessions_dir.join("session-history-explicit-title-1779690000.jsonl"),
            r#"{"timestamp":1,"event_type":"user_message","session_key":"history-explicit-title","content":{"message":"first user title"}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"title_updated","session_key":"history-explicit-title","content":{"title":"Bifrost Edge"}}"#
                + "\n",
        )
        .expect("write jsonl");

        let detail = history_session_detail("history-explicit-title").expect("history detail");
        assert_eq!(detail.title.as_deref(), Some("Bifrost Edge"));
    }

    #[test]
    fn history_session_detail_preserves_external_runner_metadata() {
        let _lock = AGENT_API_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = AgentApiEnvGuard::new(dir.path());
        let sessions_dir = dir.path().join("agent/sessions/2026/05/25");
        std::fs::create_dir_all(&sessions_dir).expect("sessions dir");
        std::fs::write(
            sessions_dir.join("session-history-traex-runner-1779690000.jsonl"),
            r#"{"timestamp":1,"event_type":"session_start","session_key":"history-traex-runner","content":{"source":"admin-api","runtime":"external_cli","adapter":"traex","runner_id":"traex","work_dir":"/tmp/work"}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"user_message","session_key":"history-traex-runner","content":{"message":"first user title"}}"#
                + "\n",
        )
        .expect("write jsonl");

        let detail = history_session_detail("history-traex-runner").expect("history detail");
        assert_eq!(detail.runner_type.as_deref(), Some("traex"));
        assert_eq!(detail.runner_id.as_deref(), Some("traex"));
        assert_eq!(detail.work_dir.as_deref(), Some("/tmp/work"));
    }

    #[test]
    fn active_history_detail_uses_external_runner_binding_from_history_state() {
        let _lock = AGENT_API_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = AgentApiEnvGuard::new(dir.path());
        let sessions_dir = dir.path().join("agent/sessions/2026/05/25");
        std::fs::create_dir_all(&sessions_dir).expect("sessions dir");
        let history_path = sessions_dir.join("session-active-traex-runner-1779690000.jsonl");
        std::fs::write(
            &history_path,
            r#"{"timestamp":1,"event_type":"session_start","session_key":"active-traex-runner","content":{"source":"admin-api","runtime":"external_cli","adapter":"traex","runner_id":"traex","work_dir":"/tmp/work"}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"user_message","session_key":"active-traex-runner","content":{"message":"hello"}}"#
                + "\n",
        )
        .expect("write jsonl");
        crate::im_gateway::session_state::remember_session_state(
            crate::im_gateway::session_state::ImAgentSessionState {
                session_key: "active-traex-runner".to_string(),
                adapter: "traex".to_string(),
                runner_id: Some("traex".to_string()),
                external_thread_id: Some("thread-traex".to_string()),
                history_path: Some(history_path.display().to_string()),
                ..crate::im_gateway::session_state::ImAgentSessionState::default()
            },
        )
        .expect("remember state");

        let mut detail = bifrost_agent::SessionDetail {
            session_key: "active-traex-runner".to_string(),
            user_id: None,
            message_count: 0,
            user_turn_count: 0,
            created_at: 1,
            last_active_at: 2,
            compaction_count: 0,
            total_tokens_used: None,
            estimated_tokens: 0,
            history_version: 0,
            work_dir: None,
            source: "admin-api".to_string(),
            agent_type: Some("Bifrost Agent".to_string()),
            runner_type: Some("bifrost_agent".to_string()),
            runner_id: None,
            model: None,
            model_provider: None,
            external_conversation_id: None,
            external_thread_id: None,
            title: None,
            messages: Vec::new(),
            goal_status: None,
            goal_objective: None,
            history_path: Some(history_path.display().to_string()),
            has_timeline: true,
            timeline_event_count: 2,
            run_state: "idle".to_string(),
        };

        enrich_detail_with_history_runner_metadata(&mut detail);
        assert_eq!(detail.runner_type.as_deref(), Some("traex"));
        assert_eq!(detail.runner_id.as_deref(), Some("traex"));
        assert_eq!(detail.external_thread_id.as_deref(), Some("thread-traex"));
        assert_eq!(detail.agent_type.as_deref(), Some("external_cli"));
    }

    #[test]
    fn active_history_detail_uses_chatgpt_web_binding_from_history_state() {
        let _lock = AGENT_API_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = AgentApiEnvGuard::new(dir.path());
        let sessions_dir = dir.path().join("agent/sessions/2026/05/25");
        std::fs::create_dir_all(&sessions_dir).expect("sessions dir");
        let history_path = sessions_dir.join("session-active-web-runner-1779690000.jsonl");
        std::fs::write(
            &history_path,
            r#"{"timestamp":1,"event_type":"session_start","session_key":"active-web-runner","content":{"source":"admin-api","runtime":"external_cli","adapter":"chatgpt_web","runner_id":"web","work_dir":"/tmp/work"}}"#
                .to_string()
                + "\n"
                + r#"{"timestamp":2,"event_type":"assistant_message","session_key":"active-web-runner","content":{"message":"web answer"}}"#
                + "\n",
        )
        .expect("write jsonl");
        crate::im_gateway::session_state::remember_session_state(
            crate::im_gateway::session_state::ImAgentSessionState {
                session_key: "active-web-runner".to_string(),
                adapter: "chatgpt_web".to_string(),
                runner_id: Some("web".to_string()),
                external_conversation_id: Some("conversation-web".to_string()),
                history_path: Some(history_path.display().to_string()),
                ..crate::im_gateway::session_state::ImAgentSessionState::default()
            },
        )
        .expect("remember state");

        let mut detail = bifrost_agent::SessionDetail {
            session_key: "active-web-runner".to_string(),
            user_id: None,
            message_count: 0,
            user_turn_count: 0,
            created_at: 1,
            last_active_at: 2,
            compaction_count: 0,
            total_tokens_used: None,
            estimated_tokens: 0,
            history_version: 0,
            work_dir: None,
            source: "admin-api".to_string(),
            agent_type: Some("Bifrost Agent".to_string()),
            runner_type: Some("bifrost_agent".to_string()),
            runner_id: None,
            model: None,
            model_provider: None,
            external_conversation_id: None,
            external_thread_id: None,
            title: None,
            messages: Vec::new(),
            goal_status: None,
            goal_objective: None,
            history_path: Some(history_path.display().to_string()),
            has_timeline: true,
            timeline_event_count: 2,
            run_state: "idle".to_string(),
        };

        enrich_detail_with_history_runner_metadata(&mut detail);
        assert_eq!(detail.runner_type.as_deref(), Some("chatgpt_web"));
        assert_eq!(detail.runner_id.as_deref(), Some("web"));
        assert_eq!(
            detail.external_conversation_id.as_deref(),
            Some("conversation-web")
        );
        assert_eq!(detail.agent_type.as_deref(), Some("external_cli"));
    }
}

// ---------------------------------------------------------------------------
