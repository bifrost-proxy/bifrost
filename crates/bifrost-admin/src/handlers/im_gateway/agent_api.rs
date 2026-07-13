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

fn is_external_runner_metadata(runner_type: Option<&str>, runner_id: Option<&str>) -> bool {
    runner_type
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
        || runner_id
            .map(|value| !value.trim().is_empty())
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
    if detail.agent_type.is_none() {
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
            let external_detail =
                if is_external_runner_metadata(runner_type.as_deref(), runner_id.as_deref()) {
                    external_runner_session_detail(&s.session_key).await
                } else {
                    None
                };
            let total_tokens_used = s.total_tokens_used.or_else(|| {
                external_detail
                    .as_ref()
                    .and_then(|detail| detail.total_tokens_used)
            });
            let estimated_tokens = if s.estimated_context_tokens > 0 {
                s.estimated_context_tokens
            } else {
                external_detail
                    .as_ref()
                    .map(|detail| detail.estimated_tokens)
                    .unwrap_or_default()
            };
            let duration_secs = s.updated_at.saturating_sub(s.started_at);
            let queue_snapshot = queue_snapshot_payload(&service.queue_manager, &s.session_key);
            unified.push(serde_json::json!({
                "session_key": s.session_key,
                "status": "active",
                "running": true,
                "state": s.state,
                "source": info.map(|item| item.source.clone()).unwrap_or_else(|| "admin-api".to_string()),
                "work_dir": s.work_dir.or_else(|| info.and_then(|item| item.work_dir.clone())),
                "turns": s.message_count.max(info.map(|item| item.message_count).unwrap_or(0)),
                "tokens": total_tokens_used,
                "start_time": s.started_at,
                "last_active_time": s.updated_at,
                "duration_secs": duration_secs,
                "compaction_count": s.compaction_count,
                "estimated_tokens": estimated_tokens,
                "context_window_tokens": s.context_window_tokens,
                "context_usage_percent": s.context_usage_percent,
                "last_response_tokens": s.last_response_tokens,
                "agent_type": s.agent_type.or_else(|| info.and_then(|item| item.agent_type.clone())),
                "runner_type": runner_type,
                "runner_id": runner_id,
                "model": s.model.or_else(|| external_detail.as_ref().and_then(|detail| detail.model.clone())).or_else(|| info.and_then(|item| item.model.clone())),
                "model_provider": s.model_provider.or_else(|| external_detail.as_ref().and_then(|detail| detail.model_provider.clone())).or_else(|| info.and_then(|item| item.model_provider.clone())),
                "model_reasoning_effort": s.model_reasoning_effort.or_else(|| external_detail.as_ref().and_then(|detail| detail.model_reasoning_effort.clone())).or_else(|| info.and_then(|item| item.model_reasoning_effort.clone())),
                "model_reasoning_summary": s.model_reasoning_summary.or_else(|| external_detail.as_ref().and_then(|detail| detail.model_reasoning_summary.clone())).or_else(|| info.and_then(|item| item.model_reasoning_summary.clone())),
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
            let external_detail =
                if is_external_runner_metadata(runner_type.as_deref(), runner_id.as_deref()) {
                    external_runner_session_detail(&s.session_key).await
                } else {
                    None
                };
            let total_tokens_used = s.total_tokens_used.or_else(|| {
                external_detail
                    .as_ref()
                    .and_then(|detail| detail.total_tokens_used)
            });
            let estimated_tokens = if s.estimated_tokens > 0 {
                s.estimated_tokens
            } else {
                external_detail
                    .as_ref()
                    .map(|detail| detail.estimated_tokens)
                    .unwrap_or_default()
            };
            let duration_secs = s.last_active_at.saturating_sub(s.created_at);
            let queue_snapshot = queue_snapshot_payload(&service.queue_manager, &s.session_key);
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
                "tokens": total_tokens_used,
                "start_time": s.created_at,
                "last_active_time": s.last_active_at,
                "duration_secs": duration_secs,
                "compaction_count": s.compaction_count,
                "estimated_tokens": estimated_tokens,
                "agent_type": s.agent_type,
                "runner_type": runner_type,
                "runner_id": runner_id,
                "model": s.model.or_else(|| external_detail.as_ref().and_then(|detail| detail.model.clone())),
                "model_provider": s.model_provider.or_else(|| external_detail.as_ref().and_then(|detail| detail.model_provider.clone())),
                "model_reasoning_effort": s.model_reasoning_effort.or_else(|| external_detail.as_ref().and_then(|detail| detail.model_reasoning_effort.clone())),
                "model_reasoning_summary": s.model_reasoning_summary.or_else(|| external_detail.as_ref().and_then(|detail| detail.model_reasoning_summary.clone())),
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
            let is_external_runner = is_external_runner_metadata(
                summary.runner_type.as_deref(),
                summary.runner_id.as_deref(),
            );
            let external_detail = if is_external_runner {
                external_runner_session_detail(&session_key).await
            } else {
                None
            };
            let total_tokens_used = if summary.total_tokens > 0 {
                summary.total_tokens
            } else {
                external_detail
                    .as_ref()
                    .and_then(|detail| detail.total_tokens_used)
                    .unwrap_or_default()
            };
            let estimated_tokens = external_detail
                .as_ref()
                .map(|detail| detail.estimated_tokens)
                .filter(|tokens| *tokens > 0);
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
                "tokens": total_tokens_used,
                "estimated_tokens": estimated_tokens,
                "start_time": summary.start_time,
                "last_active_time": summary.end_time,
                "duration_secs": summary.end_time.saturating_sub(summary.start_time),
                "history_path": p.display().to_string(),
                "has_timeline": true,
                "timeline_event_count": summary.event_count,
                "agent_type": if is_external_runner {
                    Some("external_cli".to_string())
                } else {
                    None
                },
                "runner_type": summary.runner_type,
                "runner_id": summary.runner_id,
                "model": external_detail.as_ref().and_then(|detail| detail.model.clone()),
                "model_provider": external_detail.as_ref().and_then(|detail| detail.model_provider.clone()),
                "model_reasoning_effort": external_detail.as_ref().and_then(|detail| detail.model_reasoning_effort.clone()),
                "model_reasoning_summary": external_detail.as_ref().and_then(|detail| detail.model_reasoning_summary.clone()),
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
                merge_external_runner_detail_metadata(&session_key, &mut detail).await;
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

    error_response(StatusCode::NOT_FOUND, "Agent endpoint not found")
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

pub(super) fn agent_config_response(
    config: crate::im_gateway::agent::ImAgentConfig,
) -> serde_json::Value {
    let default_base_instructions = "";
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
        let mut value = serde_json::to_value(detail).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(object) = value.as_object_mut() {
            object.insert("running".to_string(), serde_json::json!(false));
            object
                .entry("state".to_string())
                .or_insert_with(|| serde_json::json!("idle"));
            object
                .entry("run_state".to_string())
                .or_insert_with(|| serde_json::json!("idle"));
        }
        return value;
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
        "model_reasoning_effort": status.model_reasoning_effort.clone(),
        "model_reasoning_summary": status.model_reasoning_summary.clone(),
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

fn insert_queue_snapshot(
    value: &mut serde_json::Value,
    queue_manager: &crate::im_gateway::SessionQueueManager,
) {
    let Some(session_key) = value.get("session_key").and_then(|value| value.as_str()) else {
        return;
    };
    let snapshot = queue_snapshot_payload(queue_manager, session_key);
    if let Some(object) = value.as_object_mut() {
        for key in ["queue_length", "queueLength"] {
            object.insert(key.to_string(), snapshot["queueLength"].clone());
        }
        for key in ["queue_items", "queueItems"] {
            object.insert(key.to_string(), snapshot["queueItems"].clone());
        }
    }
}

fn queue_snapshot_payload(
    queue_manager: &crate::im_gateway::SessionQueueManager,
    session_key: &str,
) -> serde_json::Value {
    let items = queue_manager.queue_status(session_key);
    serde_json::json!({ "queueLength": items.len(), "queueItems": items })
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
    if status.model_reasoning_effort.is_some() {
        object.insert(
            "model_reasoning_effort".to_string(),
            serde_json::json!(status.model_reasoning_effort.clone()),
        );
    }
    if status.model_reasoning_summary.is_some() {
        object.insert(
            "model_reasoning_summary".to_string(),
            serde_json::json!(status.model_reasoning_summary.clone()),
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
        model_reasoning_effort: None,
        model_reasoning_summary: None,
        external_conversation_id: None,
        external_thread_id: None,
        metadata: None,
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

async fn merge_external_runner_detail_metadata(
    session_key: &str,
    detail: &mut bifrost_agent::SessionDetail,
) {
    let Some(external_detail) = external_runner_session_detail(session_key).await else {
        return;
    };
    append_system_session_messages(&mut detail.messages, &external_detail.messages);
    detail
        .messages
        .sort_by_key(|message| message.timestamp.unwrap_or(u64::MAX));
    detail.message_count = detail.messages.len();
    if detail
        .metadata
        .as_ref()
        .map(std::collections::BTreeMap::is_empty)
        .unwrap_or(true)
    {
        detail.metadata = external_detail.metadata;
    }
    if detail.external_conversation_id.is_none() {
        detail.external_conversation_id = external_detail.external_conversation_id;
    }
    if detail.external_thread_id.is_none() {
        detail.external_thread_id = external_detail.external_thread_id;
    }
    if detail.agent_type.is_none() {
        detail.agent_type = external_detail.agent_type;
    }
    if detail.runner_type.is_none() {
        detail.runner_type = external_detail.runner_type;
    }
    if detail.runner_id.is_none() {
        detail.runner_id = external_detail.runner_id;
    }
    if detail.model.is_none() {
        detail.model = external_detail.model;
    }
    if detail.model_provider.is_none() {
        detail.model_provider = external_detail.model_provider;
    }
    if detail.model_reasoning_effort.is_none() {
        detail.model_reasoning_effort = external_detail.model_reasoning_effort;
    }
    if detail.model_reasoning_summary.is_none() {
        detail.model_reasoning_summary = external_detail.model_reasoning_summary;
    }
    if detail.total_tokens_used.unwrap_or_default() == 0 {
        detail.total_tokens_used = external_detail.total_tokens_used;
    }
    if detail.estimated_tokens == 0 {
        detail.estimated_tokens = external_detail.estimated_tokens;
    }
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
    if !messages.is_empty() {
        append_system_display_messages(&mut messages, &state.messages);
    }
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
    messages.sort_by_key(|message| message.timestamp.unwrap_or(u64::MAX));
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
    let run_state = match state.status.as_deref() {
        Some("running") => "running",
        Some("failed" | "stopped" | "timed_out") => "failed",
        _ => "completed",
    }
    .to_string();
    let state_metadata = crate::im_gateway::session_state::metadata_from_state(&state);
    let model = state.model_override.clone().or_else(|| {
        run_detail
            .as_ref()
            .and_then(|detail| detail.metadata.get("model").cloned())
    });
    let model_provider = run_detail.as_ref().and_then(|detail| {
        detail
            .metadata
            .get("modelProvider")
            .or_else(|| detail.metadata.get("modelSource"))
            .cloned()
    });
    let usage_total_tokens = run_detail
        .as_ref()
        .and_then(|detail| detail.metadata.get("usageTotalTokens"))
        .and_then(|value| value.trim().parse::<u64>().ok());
    let estimated_tokens = run_detail
        .as_ref()
        .and_then(|detail| detail.metadata.get("usageInputTokens"))
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or_default();
    let mut metadata = run_detail
        .as_ref()
        .map(|detail| detail.metadata.clone())
        .unwrap_or_default();
    for (key, value) in state_metadata {
        metadata.entry(key).or_insert(value);
    }
    let metadata = (!metadata.is_empty()).then_some(metadata);
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
        agent_type: Some("external_cli".to_string()),
        runner_type: Some(state.adapter),
        runner_id: state.runner_id,
        model,
        model_provider,
        model_reasoning_effort: state.reasoning_effort_override.clone().or_else(|| {
            run_detail
                .as_ref()
                .and_then(|detail| detail.metadata.get("modelReasoningEffort").cloned())
        }),
        model_reasoning_summary: run_detail
            .as_ref()
            .and_then(|detail| detail.metadata.get("modelReasoningSummary").cloned()),
        external_conversation_id: state.external_conversation_id,
        external_thread_id: state.external_thread_id,
        metadata,
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

fn append_system_display_messages(
    output: &mut Vec<bifrost_agent::session::SessionMessage>,
    state_messages: &[crate::im_gateway::session_state::ImAgentSessionMessage],
) {
    for message in state_messages
        .iter()
        .filter(|message| message.role == "system")
    {
        append_state_display_message(output, message);
    }
}

fn append_system_session_messages(
    output: &mut Vec<bifrost_agent::session::SessionMessage>,
    state_messages: &[bifrost_agent::session::SessionMessage],
) {
    for message in state_messages
        .iter()
        .filter(|message| message.role == "system")
    {
        append_session_display_message(output, message);
    }
}

fn append_state_display_message(
    output: &mut Vec<bifrost_agent::session::SessionMessage>,
    message: &crate::im_gateway::session_state::ImAgentSessionMessage,
) {
    if output.iter().any(|existing| {
        existing.role == message.role
            && existing.content == message.content
            && existing.timestamp == message.timestamp
    }) {
        return;
    }
    output.push(bifrost_agent::session::SessionMessage {
        role: message.role.clone(),
        content: message.content.clone(),
        timestamp: message.timestamp,
        content_parts: message.content_parts.clone(),
        tool_calls: None,
    });
}

fn append_session_display_message(
    output: &mut Vec<bifrost_agent::session::SessionMessage>,
    message: &bifrost_agent::session::SessionMessage,
) {
    if output.iter().any(|existing| {
        existing.role == message.role
            && existing.content == message.content
            && existing.timestamp == message.timestamp
    }) {
        return;
    }
    output.push(message.clone());
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
            agent_type: Some("External Runner Agent".to_string()),
            runner_type: Some("codex".to_string()),
            runner_id: Some("Codex".to_string()),
            model: None,
            model_provider: None,
            model_reasoning_effort: None,
            model_reasoning_summary: None,
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
    fn session_detail_without_active_status_reports_explicit_idle_state() {
        let detail = bifrost_agent::SessionDetail {
            session_key: "idle-detail".to_string(),
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
            agent_type: Some("External Runner Agent".to_string()),
            runner_type: Some("codex".to_string()),
            runner_id: Some("Codex".to_string()),
            model: None,
            model_provider: None,
            model_reasoning_effort: None,
            model_reasoning_summary: None,
            external_conversation_id: None,
            external_thread_id: None,
            metadata: None,
            title: Some("Idle detail".to_string()),
            messages: Vec::new(),
            goal_status: None,
            goal_objective: None,
            history_path: Some("/tmp/history.jsonl".to_string()),
            has_timeline: true,
            timeline_event_count: 1,
            run_state: "idle".to_string(),
        };

        let value = session_detail_with_active_status(detail, None);

        assert_eq!(value["running"], false);
        assert_eq!(value["state"], "idle");
        assert_eq!(value["run_state"], "idle");
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
            model_reasoning_effort: None,
            model_reasoning_summary: None,
            external_conversation_id: None,
            external_thread_id: None,
            metadata: None,
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
            agent_type: Some("External Runner Agent".to_string()),
            runner_type: Some("codex".to_string()),
            runner_id: Some("Codex".to_string()),
            model: Some("gpt-5".to_string()),
            model_provider: Some("openai".to_string()),
            model_reasoning_effort: Some("high".to_string()),
            model_reasoning_summary: Some("auto".to_string()),
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
    fn external_runner_session_detail_merges_system_display_messages() {
        let _lock = AGENT_API_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = AgentApiEnvGuard::new(dir.path());
        let session_key = "external-system-display-detail";
        let data_dir = bifrost_agent::config::agent_home_dir();
        let mut recorder =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, session_key);
        recorder
            .record_session_start(session_key, serde_json::json!({"source": "web"}))
            .expect("record start");
        recorder
            .record_user_message(session_key, "first prompt")
            .expect("record user");
        recorder
            .record_assistant_message(session_key, "first answer")
            .expect("record assistant");
        let history_path = recorder.file_path().display().to_string();

        crate::im_gateway::session_state::remember_session_state(
            crate::im_gateway::session_state::ImAgentSessionState {
                session_key: session_key.to_string(),
                adapter: "traex".to_string(),
                runner_id: Some("Traex".to_string()),
                history_path: Some(history_path),
                status: Some("succeeded".to_string()),
                messages: vec![crate::im_gateway::session_state::ImAgentSessionMessage {
                    role: "system".to_string(),
                    content: "已将 Traex Runner 的 session 模型设置为 Kimi-K2.6。".to_string(),
                    timestamp: Some(1_770_000_003),
                    content_parts: None,
                }],
                updated_at: 1_770_000_004_000,
                ..crate::im_gateway::session_state::ImAgentSessionState::default()
            },
        )
        .expect("remember state");

        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let detail = runtime
            .block_on(external_runner_session_detail(session_key))
            .expect("detail");

        assert_eq!(detail.message_count, 3);
        assert_eq!(detail.user_turn_count, 1);
        assert!(detail
            .messages
            .iter()
            .any(|message| { message.role == "user" && message.content == "first prompt" }));
        assert!(detail
            .messages
            .iter()
            .any(|message| { message.role == "assistant" && message.content == "first answer" }));
        assert!(detail
            .messages
            .iter()
            .any(|message| { message.role == "system" && message.content.contains("Kimi-K2.6") }));
    }

    #[test]
    fn session_detail_metadata_merge_preserves_external_system_display_messages() {
        let _lock = AGENT_API_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = AgentApiEnvGuard::new(dir.path());
        let session_key = "external-system-display-merge";
        let data_dir = bifrost_agent::config::agent_home_dir();
        let mut recorder =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, session_key);
        recorder
            .record_session_start(session_key, serde_json::json!({"source": "web"}))
            .expect("record start");
        recorder
            .record_user_message(session_key, "existing prompt")
            .expect("record user");
        recorder
            .record_assistant_message(session_key, "existing answer")
            .expect("record assistant");
        let history_path = recorder.file_path().display().to_string();

        crate::im_gateway::session_state::remember_session_state(
            crate::im_gateway::session_state::ImAgentSessionState {
                session_key: session_key.to_string(),
                adapter: "traex".to_string(),
                runner_id: Some("Traex".to_string()),
                history_path: Some(history_path),
                model_override: Some("Kimi-K2.6".to_string()),
                model_override_source: Some("session slash command".to_string()),
                status: Some("succeeded".to_string()),
                messages: vec![crate::im_gateway::session_state::ImAgentSessionMessage {
                    role: "system".to_string(),
                    content: "切换模型为 Kimi-K2.6".to_string(),
                    timestamp: Some(1_770_000_003),
                    content_parts: None,
                }],
                updated_at: 1_770_000_004_000,
                ..crate::im_gateway::session_state::ImAgentSessionState::default()
            },
        )
        .expect("remember state");

        let mut detail = history_session_detail(session_key).expect("history detail");
        assert_eq!(detail.message_count, 2);
        assert!(!detail
            .messages
            .iter()
            .any(|message| message.role == "system"));

        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        runtime.block_on(merge_external_runner_detail_metadata(
            session_key,
            &mut detail,
        ));

        assert_eq!(detail.message_count, 3);
        assert_eq!(detail.model.as_deref(), Some("Kimi-K2.6"));
        assert!(detail.metadata.as_ref().is_some_and(|metadata| metadata
            .get("modelOverride")
            .is_some_and(|model| model == "Kimi-K2.6")));
        assert!(detail
            .messages
            .iter()
            .any(|message| message.role == "system" && message.content == "切换模型为 Kimi-K2.6"));
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
            agent_type: Some("External Runner Agent".to_string()),
            runner_type: Some("codex".to_string()),
            runner_id: Some("Codex".to_string()),
            model: None,
            model_provider: None,
            model_reasoning_effort: None,
            model_reasoning_summary: None,
            external_conversation_id: None,
            external_thread_id: None,
            metadata: None,
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
        assert_eq!(detail.agent_type.as_deref(), Some("External Runner Agent"));
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
            agent_type: Some("External Runner Agent".to_string()),
            runner_type: Some("codex".to_string()),
            runner_id: Some("Codex".to_string()),
            model: None,
            model_provider: None,
            model_reasoning_effort: None,
            model_reasoning_summary: None,
            external_conversation_id: None,
            external_thread_id: None,
            metadata: None,
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
        assert_eq!(detail.agent_type.as_deref(), Some("External Runner Agent"));
    }

    #[test]
    fn is_external_runner_metadata_requires_a_nonempty_type_or_id() {
        assert!(!is_external_runner_metadata(None, None));
        assert!(!is_external_runner_metadata(Some(""), Some("  ")));
        assert!(is_external_runner_metadata(Some("chatgpt_web"), None));
        assert!(is_external_runner_metadata(None, Some("web-main")));
        assert!(is_external_runner_metadata(
            Some("chatgpt_web"),
            Some("bifrost")
        ));
    }

    #[test]
    fn merge_history_runner_metadata_prefers_external_history() {
        let summary = bifrost_agent::persistence::SessionFileSummary {
            runner_type: Some("chatgpt_web".to_string()),
            runner_id: Some("web-main".to_string()),
            ..Default::default()
        };

        let (r_type, r_id) = merge_history_runner_metadata(
            Some("codex".to_string()),
            Some("Codex".to_string()),
            Some(&summary),
        );
        assert_eq!(r_type.as_deref(), Some("chatgpt_web"));
        assert_eq!(r_id.as_deref(), Some("web-main"));

        let (r_type2, r_id2) = merge_history_runner_metadata(
            Some("chatgpt_web".to_string()),
            Some("web-main".to_string()),
            None,
        );
        assert_eq!(r_type2.as_deref(), Some("chatgpt_web"));
        assert_eq!(r_id2.as_deref(), Some("web-main"));

        // No history -> original values preserved.
        let (r_type3, r_id3) =
            merge_history_runner_metadata(Some("chatgpt_web".into()), None, None);
        assert_eq!(r_type3.as_deref(), Some("chatgpt_web"));
        assert!(r_id3.is_none());
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod coverage_boost {
    use super::*;

    use crate::test_env::BifrostDataDirGuard;
    use crate::test_support::TestAdminState;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::service::service_fn;
    use hyper::{
        body::Incoming, client::conn::http1 as client_http1, server::conn::http1 as server_http1,
    };
    use hyper::{Method, Request, StatusCode};
    use hyper_util::rt::TokioIo;
    use serde_json::json;
    use tempfile::TempDir;

    /// Send a request to `/agent` handlers over an in-memory HTTP/1.1 connection.
    async fn agent_request_raw(
        service: SharedImGatewayService,
        method: Method,
        path_and_query: &str,
        body: Option<&str>,
    ) -> (StatusCode, String) {
        let (client_io, server_io) = tokio::io::duplex(16 * 1024);
        let service_clone = service.clone();
        let path = path_and_query.to_string();

        // Server side: route requests under /agent to `handle_agent`.
        let server = tokio::spawn(async move {
            let io = TokioIo::new(server_io);
            let svc = service_fn(move |req: Request<Incoming>| {
                let service = service_clone.clone();
                async move {
                    let path = req.uri().path().to_string();
                    let rest = path.strip_prefix("/agent").unwrap_or(path.as_str());
                    let resp = handle_agent(req, &service, rest).await;
                    Ok::<_, hyper::Error>(resp)
                }
            });
            server_http1::Builder::new().serve_connection(io, svc).await
        });

        // Client side: single-request HTTP/1.1 connection.
        let (mut sender, conn) = client_http1::handshake::<_, Full<Bytes>>(TokioIo::new(client_io))
            .await
            .expect("handshake");
        let conn_task = tokio::spawn(conn);

        let body_bytes = body.map(|s| s.as_bytes().to_vec()).unwrap_or_default();
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(body_bytes)))
            .unwrap();

        let resp = sender.send_request(req).await.expect("send request");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(bytes.to_vec()).unwrap_or_default();

        drop(sender);
        let _ = conn_task.await;
        let _ = server.await;

        (status, text)
    }

    async fn agent_request_json(
        service: SharedImGatewayService,
        method: Method,
        path_and_query: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let body_string = body.map(|v| serde_json::to_string(&v).expect("encode json"));
        let (status, text) =
            agent_request_raw(service, method, path_and_query, body_string.as_deref()).await;
        let json = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
        (status, json)
    }

    struct BifrostTestDir {
        _dir: TempDir,
        _guard: BifrostDataDirGuard,
    }

    fn new_temp_bifrost_dir() -> BifrostTestDir {
        let dir = TempDir::new().expect("tempdir");
        let guard = BifrostDataDirGuard::set(dir.path());
        BifrostTestDir {
            _dir: dir,
            _guard: guard,
        }
    }

    #[test]
    fn agent_config_response_enriches_with_effective_fields() {
        let config = crate::im_gateway::agent::ImAgentConfig {
            work_dir: Some("/tmp/agent-workdir".to_string()),
            ..Default::default()
        };

        let value = agent_config_response(config.clone());

        let default_instructions = "";
        assert_eq!(
            value["default_base_instructions"].as_str(),
            Some(default_instructions)
        );
        assert_eq!(
            value["effective_base_instructions"].as_str(),
            Some(default_instructions)
        );
        let resolved = config.resolve_work_dir().display().to_string();
        assert_eq!(value["resolved_work_dir"].as_str(), Some(resolved.as_str()));
    }

    #[test]
    fn patch_optional_string_handles_null_and_whitespace() {
        let mut target = Some("original".to_string());

        // Missing key -> unchanged
        let patch = json!({});
        patch_optional_string(&mut target, &patch, &["base_instructions"]);
        assert_eq!(target.as_deref(), Some("original"));

        // Null -> cleared
        let patch = json!({"base_instructions": null});
        patch_optional_string(&mut target, &patch, &["base_instructions"]);
        assert!(target.is_none());

        // Whitespace -> cleared
        let patch = json!({"base_instructions": "   "});
        patch_optional_string(&mut target, &patch, &["base_instructions"]);
        assert!(target.is_none());

        // Non-empty -> trimmed value
        let patch = json!({"base_instructions": "  hi  "});
        patch_optional_string(&mut target, &patch, &["base_instructions"]);
        assert_eq!(target.as_deref(), Some("hi"));
    }

    #[test]
    fn uses_api_key_header_respects_by_azure_override_and_defaults() {
        assert!(uses_api_key_header("aidp_crawl", &json!({})));
        assert!(uses_api_key_header("azure", &json!({})));
        assert!(!uses_api_key_header("openai", &json!({})));

        assert!(uses_api_key_header("openai", &json!({"by_azure": true})));
        assert!(!uses_api_key_header(
            "aidp_crawl",
            &json!({"by_azure": false})
        ));
    }

    #[test]
    fn apply_agent_config_patch_updates_core_fields() {
        let mut config = crate::im_gateway::agent::ImAgentConfig::default();
        let patch = json!({
            "enabled": true,
            "runner": "custom-runner",
            "model": "gpt-4-mini",
            "model_provider": "azure",
            "max_completion_tokens": 1024u64,
            "model_reasoning_effort": "medium",
            "model_reasoning_summary": "concise",
            "model_context_window": 200000,
            "model_auto_compact_token_limit": 12345,
            "base_instructions": " Base ",
            "developer_instructions": " Dev ",
            "user_instructions": " User ",
            "session_ttl_secs": 3600u64,
            "request_timeout_secs": 30u64,
            "max_turn_iterations": 12u64
        });

        apply_agent_config_patch(&mut config, &patch);

        assert!(config.enabled);
        match config.runner {
            Some(bifrost_agent::AgentRunnerMode::Custom(ref id)) => {
                assert_eq!(id, "custom-runner")
            }
            other => panic!("unexpected runner: {:?}", other),
        }
        assert_eq!(config.model.as_deref(), Some("gpt-4-mini"));
        assert_eq!(config.model_provider.as_deref(), Some("azure"));
        assert_eq!(config.max_completion_tokens, Some(1024));
        assert_eq!(config.model_reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(config.model_reasoning_summary.as_deref(), Some("concise"));
        assert_eq!(config.model_context_window, Some(200000));
        assert_eq!(config.model_auto_compact_token_limit, Some(12345));
        assert_eq!(config.base_instructions.as_deref(), Some("Base"));
        assert_eq!(config.developer_instructions.as_deref(), Some("Dev"));
        assert_eq!(config.user_instructions.as_deref(), Some("User"));
        assert_eq!(config.session_ttl_secs, Some(3600));
        assert_eq!(config.request_timeout_secs, Some(30));
        assert_eq!(config.max_turn_iterations, Some(12));
    }

    #[test]
    fn apply_agent_config_patch_clears_auto_compact_and_runner() {
        let mut config = crate::im_gateway::agent::ImAgentConfig {
            model_auto_compact_token_limit: Some(10),
            runner: Some(bifrost_agent::AgentRunnerMode::Custom("old".into())),
            ..Default::default()
        };

        let patch = json!({
            "model_auto_compact_token_limit": null,
            "runner": null
        });

        apply_agent_config_patch(&mut config, &patch);

        assert!(config.model_auto_compact_token_limit.is_none());
        assert!(config.runner.is_none());
    }

    #[test]
    fn apply_agent_config_patch_updates_history_and_memories() {
        let mut config = crate::im_gateway::agent::ImAgentConfig::default();
        let patch = json!({
            "ephemeral": true,
            "history": {
                "persistence": "last-90-days",
                "max_bytes": 8192u64
            },
            "memories": {
                "disable_on_external_context": true,
                "generate_memories": false,
                "use_memories": true,
                "max_raw_memories_for_consolidation": 123u64,
                "max_unused_days": 10,
                "max_rollout_age_days": 20,
                "max_rollouts_per_startup": 5u64,
                "min_rollout_idle_hours": 48,
                "min_rate_limit_remaining_percent": 30,
                "extract_model": "gpt-extract",
                "consolidation_model": "gpt-consolidate"
            }
        });

        apply_agent_config_patch(&mut config, &patch);

        assert!(config.ephemeral);
        let history = config.history.expect("history config");
        assert_eq!(
            history.persistence,
            bifrost_agent::HistoryPersistence::Last90Days
        );
        assert_eq!(history.max_bytes, Some(8192usize));

        let memories = config.memories.expect("memories config");
        assert_eq!(memories.disable_on_external_context, Some(true));
        assert_eq!(memories.generate_memories, Some(false));
        assert_eq!(memories.use_memories, Some(true));
        assert_eq!(memories.max_raw_memories_for_consolidation, Some(123));
        assert_eq!(memories.max_unused_days, Some(10));
        assert_eq!(memories.max_rollout_age_days, Some(20));
        assert_eq!(memories.max_rollouts_per_startup, Some(5));
        assert_eq!(memories.min_rollout_idle_hours, Some(48));
        assert_eq!(memories.min_rate_limit_remaining_percent, Some(30));
        assert_eq!(memories.extract_model.as_deref(), Some("gpt-extract"));
        assert_eq!(
            memories.consolidation_model.as_deref(),
            Some("gpt-consolidate")
        );
    }

    #[test]
    fn apply_agent_config_patch_updates_provider_and_api_key_header() {
        let mut config = crate::im_gateway::agent::ImAgentConfig {
            model_provider: Some("aidp_crawl".to_string()),
            ..Default::default()
        };

        let patch = json!({
            "base_url": "https://example.com",
            "api_key": "secret",
            "env_key": "OPENAI_API_KEY",
            "by_azure": true,
            "request_max_retries": 2u64,
            "stream_idle_timeout_ms": 5000u64,
            "stream_max_retries": 3u64
        });

        apply_agent_config_patch(&mut config, &patch);

        let provider = config.model_providers.get("aidp_crawl").expect("provider");
        assert_eq!(provider.base_url.as_deref(), Some("https://example.com"));
        assert_eq!(provider.api_key.as_deref(), Some("secret"));
        assert_eq!(provider.env_key.as_deref(), Some("OPENAI_API_KEY"));
        assert_eq!(provider.request_max_retries, Some(2));
        assert_eq!(provider.stream_idle_timeout_ms, Some(5000));
        assert_eq!(provider.stream_max_retries, Some(3));

        let headers = provider.http_headers.as_ref().expect("headers");
        assert_eq!(headers.get("api-key").map(String::as_str), Some("secret"));

        // Clearing the API key should also clear the header map.
        let patch_clear = json!({"api_key": ""});
        let mut config2 = config.clone();
        apply_agent_config_patch(&mut config2, &patch_clear);
        let provider2 = config2.model_providers.get("aidp_crawl").expect("provider");
        assert!(provider2.api_key.is_none());
        assert!(provider2
            .http_headers
            .as_ref()
            .map(|m| m.is_empty())
            .unwrap_or(true));
    }

    #[test]
    fn apply_agent_config_patch_replaces_mcp_servers_and_model_providers() {
        let mut config = crate::im_gateway::agent::ImAgentConfig::default();
        let patch = json!({
            "mcp_servers": {
                "docs": {"url": "https://example.com/mcp", "enabled": true}
            },
            "model_providers": {
                "openai": {
                    "name": "OpenAI",
                    "base_url": "https://api.openai.com",
                    "wire_api": "chat_completions",
                    "env_key": "OPENAI_API_KEY"
                }
            }
        });

        apply_agent_config_patch(&mut config, &patch);

        assert!(config.mcp_servers.contains_key("docs"));
        let provider = config
            .model_providers
            .get("openai")
            .expect("openai provider");
        assert_eq!(provider.name.as_deref(), Some("OpenAI"));
        assert_eq!(provider.base_url.as_deref(), Some("https://api.openai.com"));
        assert_eq!(provider.env_key.as_deref(), Some("OPENAI_API_KEY"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn load_history_entries_filters_by_session_key() {
        let _env = new_temp_bifrost_dir();
        let data_dir = bifrost_agent::config::agent_home_dir();

        let mut recorder1 =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, "session-one");
        recorder1
            .record_session_start("session-one", json!({"source": "admin-api"}))
            .expect("start one");
        recorder1
            .record_user_message("session-one", "hello")
            .expect("msg one");

        let mut recorder2 =
            bifrost_agent::persistence::ConversationRecorder::new(&data_dir, "session-two");
        recorder2
            .record_session_start("session-two", json!({"source": "admin-api"}))
            .expect("start two");
        recorder2
            .record_user_message("session-two", "hi")
            .expect("msg two");

        let entries =
            load_history_entries_blocking(data_dir.clone(), Some("session-one".into()), None)
                .await
                .expect("entries");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1, "session-one");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn load_history_entries_respects_max_files_limit() {
        let _env = new_temp_bifrost_dir();
        let data_dir = bifrost_agent::config::agent_home_dir();

        for key in ["session-a", "session-b", "session-c"] {
            let mut recorder =
                bifrost_agent::persistence::ConversationRecorder::new(&data_dir, key);
            recorder
                .record_session_start(key, json!({"source": "admin-api"}))
                .expect("start");
            recorder.record_user_message(key, "hello").expect("msg");
        }

        let entries = load_history_entries_blocking(data_dir.clone(), None, Some(2))
            .await
            .unwrap();
        assert!(entries.len() <= 2);
    }

    #[test]
    fn agent_session_events_response_sets_sse_headers() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let resp = agent_session_events_response(&service);
        assert_eq!(resp.status(), StatusCode::OK);
        let headers = resp.headers();
        assert_eq!(
            headers.get("Content-Type").and_then(|v| v.to_str().ok()),
            Some("text/event-stream")
        );
        assert_eq!(
            headers.get("Cache-Control").and_then(|v| v.to_str().ok()),
            Some("no-cache")
        );
        assert_eq!(
            headers.get("Connection").and_then(|v| v.to_str().ok()),
            Some("keep-alive")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_root_get_returns_config_payload() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, body) = agent_request_json(service, Method::GET, "/agent", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.get("default_base_instructions").is_some());
        assert!(body.get("effective_base_instructions").is_some());
        assert!(body.get("resolved_work_dir").is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_root_patch_rejects_invalid_json() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, text) =
            agent_request_raw(service, Method::PATCH, "/agent", Some("{not-json")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["status"], 400);
        assert!(body["error"]
            .as_str()
            .unwrap_or("")
            .contains("Invalid request body"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_root_patch_updates_config_and_persists() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();
        let patch = json!({
            "enabled": false,
            "model": "unit-test-model",
            "session_ttl_secs": 123u64
        });

        let (status, body) =
            agent_request_json(service.clone(), Method::PATCH, "/agent", Some(patch)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["enabled"], false);
        assert_eq!(body["model"], "unit-test-model");
        assert_eq!(body["session_ttl_secs"], 123);

        let config = service.agent_config_store.load();
        assert!(!config.enabled);
        assert_eq!(config.model.as_deref(), Some("unit-test-model"));
        assert_eq!(config.session_ttl_secs, Some(123));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_root_method_not_allowed_for_post() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, body) = agent_request_json(service, Method::POST, "/agent", None).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(body["status"], 405);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_instructions_get_returns_content_and_work_dir() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, body) =
            agent_request_json(service, Method::GET, "/agent/instructions", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["content"].as_str().is_some());
        assert!(body["work_dir"].as_str().is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_sessions_get_and_delete_roundtrip() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, body) =
            agent_request_json(service.clone(), Method::GET, "/agent/sessions", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["sessions"].is_array());

        let (status_del, body_del) =
            agent_request_json(service, Method::DELETE, "/agent/sessions", None).await;
        assert_eq!(status_del, StatusCode::OK);
        assert_eq!(body_del["ok"], true);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_sessions_post_method_not_allowed() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, body) =
            agent_request_json(service, Method::POST, "/agent/sessions", None).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(body["status"], 405);
    }
}

#[cfg(test)]
mod coverage_boost_v2 {
    use super::*;

    use crate::test_support::TestAdminState;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::service::service_fn;
    use hyper::{
        body::Incoming, client::conn::http1 as client_http1, server::conn::http1 as server_http1,
    };
    use hyper::{Method, Request, StatusCode};
    use hyper_util::rt::TokioIo;
    use serde_json::json;

    /// Send a request to `/agent` handlers over an in-memory HTTP/1.1 connection.
    async fn agent_request_raw(
        service: SharedImGatewayService,
        method: Method,
        path_and_query: &str,
        body: Option<&str>,
    ) -> (StatusCode, String) {
        let (client_io, server_io) = tokio::io::duplex(16 * 1024);
        let service_clone = service.clone();
        let path = path_and_query.to_string();

        // Server side: route requests under /agent to `handle_agent`.
        let server = tokio::spawn(async move {
            let io = TokioIo::new(server_io);
            let svc = service_fn(move |req: Request<Incoming>| {
                let service = service_clone.clone();
                async move {
                    let path = req.uri().path().to_string();
                    let rest = path.strip_prefix("/agent").unwrap_or(path.as_str());
                    let resp = handle_agent(req, &service, rest).await;
                    Ok::<_, hyper::Error>(resp)
                }
            });
            server_http1::Builder::new().serve_connection(io, svc).await
        });

        // Client side: single-request HTTP/1.1 connection.
        let (mut sender, conn) = client_http1::handshake::<_, Full<Bytes>>(TokioIo::new(client_io))
            .await
            .expect("handshake");
        let conn_task = tokio::spawn(conn);

        let body_bytes = body.map(|s| s.as_bytes().to_vec()).unwrap_or_default();
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(body_bytes)))
            .unwrap();

        let resp = sender.send_request(req).await.expect("send request");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(bytes.to_vec()).unwrap_or_default();

        drop(sender);
        let _ = conn_task.await;
        let _ = server.await;

        (status, text)
    }

    async fn agent_request_json(
        service: SharedImGatewayService,
        method: Method,
        path_and_query: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let body_string = body.map(|v| serde_json::to_string(&v).expect("encode json"));
        let (status, text) =
            agent_request_raw(service, method, path_and_query, body_string.as_deref()).await;
        let json = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
        (status, json)
    }

    // ---------------------------------------------------------------------
    // /agent/sessions/all

    #[tokio::test(flavor = "current_thread")]
    async fn agent_sessions_all_empty_returns_zero_counts() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, body) =
            agent_request_json(service, Method::GET, "/agent/sessions/all", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total"].as_u64(), Some(0));
        assert_eq!(body["active_count"].as_u64(), Some(0));
        assert_eq!(body["history_count"].as_u64(), Some(0));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_sessions_all_supports_limit_query_param() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, body) =
            agent_request_json(service, Method::GET, "/agent/sessions/all?limit=1", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["total"].as_u64().unwrap_or(0) <= 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_sessions_all_method_not_allowed_for_post() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, body) =
            agent_request_json(service, Method::POST, "/agent/sessions/all", None).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(body["status"], 405);
    }

    // ---------------------------------------------------------------------
    // /agent/sessions/history

    #[tokio::test(flavor = "current_thread")]
    async fn agent_sessions_history_empty_returns_zero() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, body) =
            agent_request_json(service, Method::GET, "/agent/sessions/history", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["history"].as_array().unwrap().is_empty());
        assert_eq!(body["total"].as_u64(), Some(0));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_sessions_history_post_method_not_allowed() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, body) =
            agent_request_json(service, Method::POST, "/agent/sessions/history", None).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(body["status"], 405);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_sessions_history_lists_created_conversation() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();
        let data_dir = bifrost_agent::config::agent_home_dir();

        let key = "history-v2-list";
        let mut recorder = bifrost_agent::persistence::ConversationRecorder::new(&data_dir, key);
        recorder
            .record_session_start(key, json!({"source": "admin-api"}))
            .expect("start");
        recorder
            .record_user_message(key, "hello")
            .expect("user message");

        let (status, body) =
            agent_request_json(service, Method::GET, "/agent/sessions/history", None).await;
        assert_eq!(status, StatusCode::OK);
        let history = body["history"].as_array().expect("array");
        assert!(history.iter().any(|item| item["session_key"] == key));
    }

    // ---------------------------------------------------------------------
    // /agent/sessions/history/{path}

    #[tokio::test(flavor = "current_thread")]
    async fn agent_history_events_unpaged_returns_events() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();
        let data_dir = bifrost_agent::config::agent_home_dir();

        let key = "history-v2-events";
        let mut recorder = bifrost_agent::persistence::ConversationRecorder::new(&data_dir, key);
        recorder
            .record_session_start(key, json!({"source": "admin-api"}))
            .expect("start");
        recorder.record_user_message(key, "hello").expect("user");
        let file_path = recorder.file_path();
        let encoded = urlencoding::encode(file_path.to_str().unwrap());
        let url = format!("/agent/sessions/history/{encoded}");

        let (status, body) = agent_request_json(service, Method::GET, &url, None).await;
        assert_eq!(status, StatusCode::OK);
        let events = body["events"].as_array().expect("events array");
        assert!(!events.is_empty());
        assert_eq!(
            body["total_count"].as_u64().unwrap_or(0),
            body["count"].as_u64().unwrap_or(0)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_history_events_tail_query_uses_paging() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();
        let data_dir = bifrost_agent::config::agent_home_dir();

        let key = "history-v2-tail";
        let mut recorder = bifrost_agent::persistence::ConversationRecorder::new(&data_dir, key);
        recorder
            .record_session_start(key, json!({"source": "admin-api"}))
            .expect("start");
        recorder.record_user_message(key, "hello").expect("user");
        recorder
            .record_assistant_message(key, "world")
            .expect("assistant");
        let file_path = recorder.file_path();
        let encoded = urlencoding::encode(file_path.to_str().unwrap());
        let url = format!("/agent/sessions/history/{encoded}?limit=1&tail=1");

        let (status, body) = agent_request_json(service, Method::GET, &url, None).await;
        assert_eq!(status, StatusCode::OK);
        let count = body["count"].as_u64().unwrap_or(0);
        assert!(count <= 1);
        assert!(body["total_count"].as_u64().unwrap_or(0) >= count);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_history_events_invalid_path_returns_bad_request() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let encoded = urlencoding::encode("../outside.jsonl");
        let url = format!("/agent/sessions/history/{encoded}");
        let (status, body) = agent_request_json(service, Method::GET, &url, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["status"], 400);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_history_delete_removes_file_and_returns_ok() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();
        let data_dir = bifrost_agent::config::agent_home_dir();

        let key = "history-v2-delete";
        let mut recorder = bifrost_agent::persistence::ConversationRecorder::new(&data_dir, key);
        recorder
            .record_session_start(key, json!({"source": "admin-api"}))
            .expect("start");
        let file_path = recorder.file_path();
        let encoded = urlencoding::encode(file_path.to_str().unwrap());
        let url = format!("/agent/sessions/history/{encoded}");

        let (status, body) = agent_request_json(service.clone(), Method::DELETE, &url, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert!(!file_path.exists());
    }

    // ---------------------------------------------------------------------
    // /agent/sessions/:key

    #[tokio::test(flavor = "current_thread")]
    async fn agent_session_detail_not_found_returns_404() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, body) =
            agent_request_json(service, Method::GET, "/agent/sessions/does-not-exist", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["status"], 404);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_session_detail_uses_history_when_runtime_missing() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();
        let data_dir = bifrost_agent::config::agent_home_dir();

        let key = "detail-from-history-v2";
        let mut recorder = bifrost_agent::persistence::ConversationRecorder::new(&data_dir, key);
        recorder
            .record_session_start(key, json!({"source": "admin-api"}))
            .expect("start");
        recorder.record_user_message(key, "hello").expect("user");

        let url = "/agent/sessions/detail-from-history-v2";
        let (status, body) = agent_request_json(service, Method::GET, url, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["session_key"], key);
        assert_eq!(body["message_count"].as_u64(), Some(1));
        assert!(body["history_path"].as_str().is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_session_delete_returns_ok_even_when_missing() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, body) =
            agent_request_json(service, Method::DELETE, "/agent/sessions/missing", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
    }

    // ---------------------------------------------------------------------
    // /agent/sessions/events

    #[tokio::test(flavor = "current_thread")]
    async fn agent_sessions_events_post_method_not_allowed() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let (status, body) =
            agent_request_json(service, Method::POST, "/agent/sessions/events", None).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(body["status"], 405);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_sessions_events_stream_starts_with_connected_event() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let mut resp = agent_session_events_response(&service);
        assert_eq!(resp.status(), StatusCode::OK);
        let first_frame = resp
            .body_mut()
            .frame()
            .await
            .expect("frame")
            .expect("data frame");
        let bytes = first_frame.into_data().expect("data");
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("event: connected"));
    }

    // ---------------------------------------------------------------------
    // Helper functions coverage

    #[test]
    fn summary_end_time_prefers_end_when_present() {
        let mut summary = bifrost_agent::persistence::SessionFileSummary {
            start_time: 10,
            end_time: 20,
            ..Default::default()
        };
        assert_eq!(summary_end_time(&summary), 20);
        summary.end_time = 0;
        assert_eq!(summary_end_time(&summary), 10);
    }

    #[test]
    fn insert_queue_snapshot_is_noop_without_session_key() {
        let harness = TestAdminState::builder().build();
        let service = harness.im_gateway_service();

        let mut value = json!({"foo": "bar"});
        insert_queue_snapshot(&mut value, &service.queue_manager);
        assert_eq!(value["foo"], "bar");
    }

    #[test]
    fn active_session_list_run_state_handles_all_cases() {
        assert_eq!(active_session_list_run_state(true, "idle"), "running");
        assert_eq!(active_session_list_run_state(false, "running"), "completed");
        assert_eq!(active_session_list_run_state(false, "failed"), "failed");
    }

    #[test]
    fn history_session_list_run_state_uses_terminal_state_when_available() {
        let mut summary = bifrost_agent::persistence::SessionFileSummary {
            run_state: Some("failed".to_string()),
            ..Default::default()
        };
        assert_eq!(history_session_list_run_state(&summary), "failed");
        summary.run_state = Some("non_terminal".to_string());
        assert_eq!(history_session_list_run_state(&summary), "completed");
    }
}
