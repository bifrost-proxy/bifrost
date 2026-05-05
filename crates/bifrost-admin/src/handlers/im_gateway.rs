use std::collections::{HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Instant;

use bifrost_core::text::truncate_bytes_with_suffix;
use futures_util::FutureExt;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::handlers::{error_response, json_response, method_not_allowed, BoxBody};
use crate::im_gateway::event_router::ImEventRouter;
use crate::im_gateway::provider::ImProvider;
use crate::im_gateway::types::{
    ImEvent, ImMessageLog, ImProviderConfig, ImRoute, ImRouteAction, ImSchedule, ImTarget,
    MessageDirection, MessageStatus,
};
use crate::im_gateway::{
    ImAgentClient, ImAgentConfigStore, ImAgentSessionManager, ImAgentToolRegistry,
    ImConnectionManager, ImEventStore, ImMcpManager, ImMessageLogStore, ImProviderStore,
    ImRouteStore, ImRunStore, ImScheduleStore, ImTargetStore, SessionQueueManager,
};
use bifrost_agent::persistence::ConversationRecorder;
use bifrost_agent::SessionDetail;

// ---------------------------------------------------------------------------
// ImGatewayService
// ---------------------------------------------------------------------------

pub struct ImGatewayService {
    pub provider_store: Arc<ImProviderStore>,
    pub target_store: Arc<ImTargetStore>,
    pub route_store: Arc<ImRouteStore>,
    pub schedule_store: Arc<ImScheduleStore>,
    pub event_store: Arc<ImEventStore>,
    pub run_store: Arc<ImRunStore>,
    pub message_log_store: Arc<ImMessageLogStore>,
    pub connection_manager: Arc<ImConnectionManager>,
    pub agent_config_store: Arc<ImAgentConfigStore>,
    pub agent_client: Arc<ImAgentClient>,
    pub agent_tools: Arc<ImAgentToolRegistry>,
    pub agent_session_manager: Arc<ImAgentSessionManager>,
    pub queue_manager: Arc<SessionQueueManager>,
}

impl ImGatewayService {
    pub fn new(data_dir: &std::path::Path) -> Self {
        // Install embedded system skills on startup (idempotent, fingerprint-checked)
        bifrost_agent::install_system_skills();

        // Store agent config under data_dir/agent/ for unified directory structure
        let agent_data_dir = data_dir.join("agent");
        let _ = std::fs::create_dir_all(&agent_data_dir);
        let agent_config_store = Arc::new(ImAgentConfigStore::new(&agent_data_dir));
        let agent_config = agent_config_store.load();
        let agent_tools = Arc::new(ImAgentToolRegistry::with_defaults(
            agent_config.get_shell_timeout_secs(),
        ));
        Self {
            provider_store: Arc::new(ImProviderStore::new(data_dir)),
            target_store: Arc::new(ImTargetStore::new(data_dir)),
            route_store: Arc::new(ImRouteStore::new(data_dir)),
            schedule_store: Arc::new(ImScheduleStore::new(data_dir)),
            event_store: Arc::new(ImEventStore::new(data_dir)),
            run_store: Arc::new(ImRunStore::new(data_dir)),
            message_log_store: Arc::new(ImMessageLogStore::new(data_dir)),
            connection_manager: Arc::new(ImConnectionManager::new()),
            agent_config_store,
            agent_client: Arc::new(ImAgentClient::new()),
            agent_tools,
            agent_session_manager: Arc::new(ImAgentSessionManager::new(
                agent_config.get_session_ttl_secs(),
            )),
            queue_manager: Arc::new(SessionQueueManager::new()),
        }
    }

    /// Auto-connect all configured providers that have a secret.
    /// If owner_open_id is not set, auto-detect it from the Feishu Application API.
    /// Called on Bifrost startup to restore active connections and send online notifications.
    pub async fn auto_connect_providers(self: &Arc<Self>) {
        let providers = self.provider_store.list();
        for mut provider in providers {
            // Only auto-connect providers that have a secret configured
            let has_secret = provider
                .secret_ref
                .as_deref()
                .is_some_and(|s| !s.is_empty());

            if !has_secret {
                info!(
                    provider_id = %provider.id,
                    "skipping auto-connect: missing secret"
                );
                continue;
            }

            // Auto-detect owner_open_id if not set
            let has_owner = provider
                .owner_open_id
                .as_deref()
                .is_some_and(|s| !s.is_empty());

            if !has_owner {
                let feishu = self.connection_manager.feishu_provider().clone();
                match feishu.fetch_bot_owner_open_id(&provider).await {
                    Ok(owner_id) => {
                        info!(
                            provider_id = %provider.id,
                            owner_open_id = %owner_id,
                            "auto-detected bot owner on startup"
                        );
                        provider.owner_open_id = Some(owner_id);
                        if let Err(e) = self.provider_store.update(provider.clone()) {
                            warn!(
                                provider_id = %provider.id,
                                error = %e,
                                "failed to persist auto-detected owner_open_id"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            provider_id = %provider.id,
                            error = %e,
                            "failed to auto-detect owner on startup, skipping auto-connect"
                        );
                        continue;
                    }
                }
            }

            let app_secret = provider.secret_ref.clone().unwrap_or_default();

            // Create event channel
            let (tx, rx) = mpsc::unbounded_channel::<ImEvent>();

            // Spawn the event processing loop
            let feishu = self.connection_manager.feishu_provider().clone();
            let provider_for_loop = provider.clone();
            let event_store = self.event_store.clone();
            let message_log_store = self.message_log_store.clone();
            let route_store = self.route_store.clone();
            let agent_config_store = self.agent_config_store.clone();
            let agent_client = self.agent_client.clone();
            let agent_tools = self.agent_tools.clone();
            let agent_session_manager = self.agent_session_manager.clone();
            let queue_manager = self.queue_manager.clone();
            tokio::spawn(async move {
                run_event_loop(
                    rx,
                    feishu,
                    provider_for_loop,
                    event_store,
                    message_log_store,
                    route_store,
                    agent_config_store,
                    agent_client,
                    agent_tools,
                    agent_session_manager,
                    queue_manager,
                )
                .await;
            });

            // Start the long connection
            match self
                .connection_manager
                .start_connection(&provider, &app_secret, tx)
                .await
            {
                Ok(()) => {
                    info!(provider_id = %provider.id, "auto-connected provider on startup");
                }
                Err(e) => {
                    error!(
                        provider_id = %provider.id,
                        error = %e,
                        "failed to auto-connect provider on startup"
                    );
                }
            }
        }

        // Kick off the background supervisor that periodically retries
        // providers whose long connection has fallen into a Disconnected /
        // Failed state. Fire-and-forget; the task stays alive for the
        // lifetime of the service.
        self.clone().spawn_reconnect_supervisor();
    }

    /// Spawn a background task that periodically re-scans all providers and
    /// attempts to reconnect any whose long connection is currently
    /// Disconnected or Failed.
    ///
    /// This acts as a last-resort safety net: the long-connection task
    /// itself retries internally with exponential backoff, but if its task
    /// ever exits (e.g. due to an unexpected shutdown signal or a panic
    /// caught elsewhere) the ConnectionManager would otherwise be stuck.
    fn spawn_reconnect_supervisor(self: Arc<Self>) {
        use crate::im_gateway::types::ConnectionState;
        const SUPERVISOR_INTERVAL_SECS: u64 = 60;
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(SUPERVISOR_INTERVAL_SECS));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // Skip the immediate tick — auto_connect_providers already ran.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let statuses = self.connection_manager.list_statuses();
                for (pid, st) in statuses {
                    match st.state {
                        ConnectionState::Disconnected | ConnectionState::Failed => {
                            let Some(provider) = self.provider_store.get(&pid) else {
                                debug!(provider_id = %pid, "supervisor: provider no longer configured, skipping");
                                continue;
                            };
                            let Some(app_secret) = provider.secret_ref.clone() else {
                                continue;
                            };
                            if app_secret.is_empty() {
                                continue;
                            }
                            info!(
                                provider_id = %pid,
                                prev_state = ?st.state,
                                "supervisor: attempting reconnect"
                            );
                            let (tx, rx) = mpsc::unbounded_channel::<ImEvent>();
                            let feishu = self.connection_manager.feishu_provider().clone();
                            let provider_for_loop = provider.clone();
                            let event_store = self.event_store.clone();
                            let message_log_store = self.message_log_store.clone();
                            let route_store = self.route_store.clone();
                            let agent_config_store = self.agent_config_store.clone();
                            let agent_client = self.agent_client.clone();
                            let agent_tools = self.agent_tools.clone();
                            let agent_session_manager = self.agent_session_manager.clone();
                            let queue_manager = self.queue_manager.clone();
                            tokio::spawn(async move {
                                run_event_loop(
                                    rx,
                                    feishu,
                                    provider_for_loop,
                                    event_store,
                                    message_log_store,
                                    route_store,
                                    agent_config_store,
                                    agent_client,
                                    agent_tools,
                                    agent_session_manager,
                                    queue_manager,
                                )
                                .await;
                            });
                            if let Err(e) = self
                                .connection_manager
                                .start_connection(&provider, &app_secret, tx)
                                .await
                            {
                                warn!(
                                    provider_id = %pid,
                                    error = %e,
                                    "supervisor: reconnect attempt failed, will retry next tick"
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
        });
    }
}

pub type SharedImGatewayService = Arc<ImGatewayService>;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn handle_im_gateway(
    req: Request<Incoming>,
    service: Option<SharedImGatewayService>,
    path: &str,
) -> Response<BoxBody> {
    let Some(service) = service else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "IM Gateway not configured");
    };

    let sub = path.strip_prefix("/api/im-gateway").unwrap_or(path);

    if let Some(rest) = sub.strip_prefix("/providers") {
        return handle_providers(req, &service, rest).await;
    }
    if let Some(rest) = sub.strip_prefix("/targets") {
        return handle_targets(req, &service, rest).await;
    }
    if sub == "/messages/send" || sub == "/messages/send/" {
        return handle_messages_send(req, &service).await;
    }
    if let Some(rest) = sub.strip_prefix("/routes") {
        return handle_routes(req, &service, rest).await;
    }
    if let Some(rest) = sub.strip_prefix("/agent") {
        return handle_agent(req, &service, rest).await;
    }
    if let Some(rest) = sub.strip_prefix("/schedules") {
        return handle_schedules(req, &service, rest).await;
    }
    if let Some(rest) = sub.strip_prefix("/history") {
        return handle_history(&req, &service, rest);
    }

    error_response(StatusCode::NOT_FOUND, "IM Gateway endpoint not found")
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

async fn handle_providers(
    req: Request<Incoming>,
    service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let rest = rest.trim_end_matches('/');

    // GET /providers  |  POST /providers
    if rest.is_empty() {
        return match *req.method() {
            Method::GET => {
                let providers = service.provider_store.list();
                let safe: Vec<_> = providers.iter().map(sanitize_provider).collect();
                json_response(&safe)
            }
            Method::POST => {
                let mut config: ImProviderConfig = match read_body_json(req).await {
                    Ok(v) => v,
                    Err(resp) => return resp,
                };
                let now = now_ms();
                if config.created_at == 0 {
                    config.created_at = now;
                }
                config.updated_at = now;
                match service.provider_store.add(config) {
                    Ok(()) => json_response(&serde_json::json!({"success": true})),
                    Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
                }
            }
            _ => method_not_allowed(),
        };
    }

    // Sub-paths: /:id, /:id/status, /:id/policy, /:id/policy/bind-shell
    if let Some(id_and_rest) = rest.strip_prefix('/') {
        // Check for /:id/policy/bind-shell
        if let Some(id) = extract_segment_before(id_and_rest, "/policy/bind-shell") {
            return handle_provider_policy_bind_shell(req, service, id).await;
        }
        // Check for /:id/policy
        if let Some(id) = extract_segment_before(id_and_rest, "/policy") {
            return handle_provider_policy(req, service, id).await;
        }
        // Check for /:id/status
        if let Some(id) = extract_segment_before(id_and_rest, "/status") {
            return handle_provider_status(&req, service, id);
        }
        // Check for /:id/connect
        if let Some(id) = extract_segment_before(id_and_rest, "/connect") {
            return handle_provider_connect(req, service, id).await;
        }
        // Check for /:id/disconnect
        if let Some(id) = extract_segment_before(id_and_rest, "/disconnect") {
            return handle_provider_disconnect(req, service, id).await;
        }
        // Check for /:id/messages
        if let Some(id) = extract_segment_before(id_and_rest, "/messages") {
            return handle_provider_messages(req, service, id).await;
        }
        // /:id
        let id = id_and_rest.split('/').next().unwrap_or(id_and_rest);
        if !id.is_empty() && !id.contains('/') {
            return handle_provider_by_id(req, service, id).await;
        }
    }

    error_response(StatusCode::NOT_FOUND, "Provider endpoint not found")
}

async fn handle_provider_by_id(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    match *req.method() {
        Method::GET => match service.provider_store.get(id) {
            Some(p) => json_response(&sanitize_provider(&p)),
            None => error_response(StatusCode::NOT_FOUND, "Provider not found"),
        },
        Method::PATCH => {
            let patch: serde_json::Value = match read_body_json(req).await {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let Some(mut existing) = service.provider_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Provider not found");
            };
            apply_provider_patch(&mut existing, &patch);
            match service.provider_store.update(existing) {
                Ok(()) => json_response(&serde_json::json!({"success": true})),
                Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        Method::DELETE => match service.provider_store.delete(id) {
            Ok(()) => json_response(&serde_json::json!({"success": true})),
            Err(e) => error_response(StatusCode::NOT_FOUND, &e.to_string()),
        },
        _ => method_not_allowed(),
    }
}

fn handle_provider_status(
    req: &Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }
    let status = service.connection_manager.get_status(id);
    match status {
        Some(s) => json_response(&s),
        None => {
            if service.provider_store.get(id).is_some() {
                json_response(&crate::im_gateway::types::ConnectionStatus::default())
            } else {
                error_response(StatusCode::NOT_FOUND, "Provider not found")
            }
        }
    }
}

/// POST /providers/:id/connect — start event long connection for a provider.
///
/// If `owner_open_id` is not configured, this will auto-detect it from the
/// Feishu Application Info API and persist it to the provider store.
async fn handle_provider_connect(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    let Some(mut provider) = service.provider_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };

    let app_secret = match provider.secret_ref.as_deref() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return error_response(StatusCode::BAD_REQUEST, "Provider has no secret configured"),
    };

    // Auto-detect owner_open_id if not set
    let feishu = service.connection_manager.feishu_provider().clone();
    if provider
        .owner_open_id
        .as_deref()
        .unwrap_or_default()
        .is_empty()
    {
        match feishu.fetch_bot_owner_open_id(&provider).await {
            Ok(owner_id) => {
                info!(
                    provider_id = id,
                    owner_open_id = %owner_id,
                    "auto-detected bot owner"
                );
                provider.owner_open_id = Some(owner_id);
                // Persist to store
                if let Err(e) = service.provider_store.update(provider.clone()) {
                    warn!(provider_id = id, error = %e, "failed to persist auto-detected owner_open_id");
                }
            }
            Err(e) => {
                warn!(
                    provider_id = id,
                    error = %e,
                    "failed to auto-detect owner, connection will proceed without owner filter"
                );
            }
        }
    }

    // Create event channel
    let (tx, rx) = mpsc::unbounded_channel::<ImEvent>();

    // Spawn the event processing loop
    let provider_for_loop = provider.clone();
    let event_store = service.event_store.clone();
    let message_log_store = service.message_log_store.clone();
    let route_store = service.route_store.clone();
    let agent_config_store = service.agent_config_store.clone();
    let agent_client = service.agent_client.clone();
    let agent_tools = service.agent_tools.clone();
    let agent_session_manager = service.agent_session_manager.clone();
    let queue_manager = service.queue_manager.clone();
    tokio::spawn(async move {
        run_event_loop(
            rx,
            feishu,
            provider_for_loop,
            event_store,
            message_log_store,
            route_store,
            agent_config_store,
            agent_client,
            agent_tools,
            agent_session_manager,
            queue_manager,
        )
        .await;
    });

    // Start the long connection
    match service
        .connection_manager
        .start_connection(&provider, &app_secret, tx)
        .await
    {
        Ok(()) => {
            info!(provider_id = id, "provider event connection started");
            json_response(&serde_json::json!({"success": true, "message": "Connection started"}))
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to start connection: {e}"),
        ),
    }
}

/// POST /providers/:id/disconnect — stop event long connection.
async fn handle_provider_disconnect(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    if service.provider_store.get(id).is_none() {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    }

    service.connection_manager.stop_connection(id);
    info!(provider_id = id, "provider event connection stopped");
    json_response(&serde_json::json!({"success": true, "message": "Connection stopped"}))
}

/// GET /providers/:id/messages — list message logs for a provider.
///   Query params: ?direction=inbound|outbound  &source=user|bot
/// DELETE /providers/:id/messages — clear message logs for a provider.
async fn handle_provider_messages(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if service.provider_store.get(id).is_none() {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    }

    match *req.method() {
        Method::GET => {
            let query = req.uri().query().unwrap_or("");
            let params = parse_query_params(query);
            let direction_filter = params.get("direction").map(|s| s.as_str());
            let source_filter = params.get("source").map(|s| s.as_str());

            let mut messages = service.message_log_store.list_by_provider(id);

            // Filter by direction
            if let Some(dir) = direction_filter {
                messages.retain(|m| match dir {
                    "inbound" => matches!(m.direction, MessageDirection::Inbound),
                    "outbound" => matches!(m.direction, MessageDirection::Outbound),
                    _ => true,
                });
            }

            // Filter by source: "user" = inbound from user, "bot" = outbound from bot
            if let Some(src) = source_filter {
                messages.retain(|m| match src {
                    "user" => matches!(m.direction, MessageDirection::Inbound),
                    "bot" => matches!(m.direction, MessageDirection::Outbound),
                    _ => true,
                });
            }

            json_response(&messages)
        }
        Method::DELETE => match service.message_log_store.clear_by_provider(id) {
            Ok(()) => {
                json_response(&serde_json::json!({"success": true, "message": "Messages cleared"}))
            }
            Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        },
        _ => method_not_allowed(),
    }
}

/// Build a session key that is unique per (provider, user) pair.
///
/// This ensures that the same user chatting through different bots gets
/// independent agent sessions, preventing cross-bot history contamination.
fn build_session_key(provider_id: &str, user_id: Option<&str>) -> String {
    let user = user_id.unwrap_or("unknown");
    format!("{provider_id}:{user}")
}

// ---------------------------------------------------------------------------
// Event Deduplication
// ---------------------------------------------------------------------------

/// Time-windowed event deduplication filter.
///
/// During reconnection, the Feishu server may re-deliver events that were
/// already processed. This filter uses a bounded queue of recently-seen
/// event_ids with a TTL to efficiently discard duplicates.
struct EventDedup {
    /// Ordered queue of (event_id, first_seen_at) for TTL expiry.
    window: VecDeque<(String, Instant)>,
    /// Maximum number of event_ids to retain.
    max_entries: usize,
    /// Events older than this duration are evicted.
    ttl: std::time::Duration,
}

impl EventDedup {
    fn new() -> Self {
        Self {
            window: VecDeque::with_capacity(512),
            max_entries: 2048,
            ttl: std::time::Duration::from_secs(300), // 5 minutes
        }
    }

    /// Returns `true` if this event_id is a duplicate (already seen within the
    /// TTL window). If not a duplicate, records it for future checks.
    fn is_duplicate(&mut self, event_id: &str) -> bool {
        self.evict_expired();

        // Check if already seen
        if self.window.iter().any(|(id, _)| id == event_id) {
            return true;
        }

        // Record new event
        if self.window.len() >= self.max_entries {
            self.window.pop_front();
        }
        self.window
            .push_back((event_id.to_string(), Instant::now()));
        false
    }

    fn evict_expired(&mut self) {
        let cutoff = Instant::now() - self.ttl;
        while let Some((_, ts)) = self.window.front() {
            if *ts < cutoff {
                self.window.pop_front();
            } else {
                break;
            }
        }
    }
}

/// Event processing loop: receives events from the long connection and processes them.
///
/// Security: Only processes messages from the bot owner (owner_open_id).
/// After owner check, matches routes and executes actions (script or agent chat).
#[allow(clippy::too_many_arguments)]
async fn run_event_loop(
    mut rx: mpsc::UnboundedReceiver<ImEvent>,
    feishu: Arc<crate::im_gateway::feishu::FeishuProvider>,
    provider: ImProviderConfig,
    event_store: Arc<ImEventStore>,
    message_log_store: Arc<ImMessageLogStore>,
    route_store: Arc<ImRouteStore>,
    agent_config_store: Arc<ImAgentConfigStore>,
    agent_client: Arc<ImAgentClient>,
    agent_tools: Arc<ImAgentToolRegistry>,
    agent_session_manager: Arc<ImAgentSessionManager>,
    queue_manager: Arc<SessionQueueManager>,
) {
    info!(
        provider_id = %provider.id,
        owner_open_id = ?provider.owner_open_id,
        "event processing loop started"
    );

    // Initialize MCP manager from agent config (TOML + JSON merged)
    let init_config = agent_config_store.load();

    // Cleanup expired session files on startup if retention policy is active
    if let Some(ref history) = init_config.history {
        if history.persistence == bifrost_agent::config::HistoryPersistence::Last90Days {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let cutoff = now.saturating_sub(90 * 24 * 3600);
            let data_dir = bifrost_agent::config::agent_home_dir();
            let removed = bifrost_agent::persistence::cleanup_expired_sessions(&data_dir, cutoff);
            if removed > 0 {
                info!(removed, "cleaned up expired session files (>90 days)");
            }
        }
    }

    let mut mcp_manager = ImMcpManager::new(&init_config.mcp_servers).await;
    let mcp_tool_count = mcp_manager.list_tools().len();
    if mcp_tool_count > 0 {
        info!(
            provider_id = %provider.id,
            mcp_tools = mcp_tool_count,
            "MCP manager initialized with tools"
        );
    }

    // Send online notification to owner on connect
    if let Some(ref owner_open_id) = provider.owner_open_id {
        let online_target = ImTarget {
            id: "__online_notify__".to_string(),
            provider_id: provider.id.clone(),
            display_name: "Owner".to_string(),
            enabled: true,
            receive_id_type: "open_id".to_string(),
            receive_id: owner_open_id.clone(),
            default_msg_type: "text".to_string(),
            created_at: 0,
            updated_at: 0,
        };

        let online_msg = "你好，Bifrost 助手上线了";
        let send_result = feishu
            .send_text(&provider, &online_target, online_msg)
            .await;

        // Record outbound message log
        let (status, message_id, error_msg) = match &send_result {
            Ok(r) => (MessageStatus::Success, r.message_id.clone(), None),
            Err(e) => (MessageStatus::Failed, None, Some(e.to_string())),
        };
        let log = ImMessageLog {
            id: uuid_short(),
            provider_id: provider.id.clone(),
            direction: MessageDirection::Outbound,
            status,
            timestamp: now_ms(),
            target_id: Some(owner_open_id.clone()),
            target_name: Some("Owner".to_string()),
            message_id,
            msg_type: Some("text".to_string()),
            content_preview: Some(online_msg.to_string()),
            trigger: Some("online".to_string()),
            error: error_msg,
            sender_open_id: None,
            event_id: None,
            reaction_added: None,
        };
        let _ = message_log_store.add(log);

        if let Err(e) = &send_result {
            error!(provider_id = %provider.id, error = %e, "failed to send online notification");
        } else {
            info!(provider_id = %provider.id, owner_open_id = %owner_open_id, "online notification sent");
        }
    }

    let mut dedup = EventDedup::new();

    while let Some(event) = rx.recv().await {
        // Deduplication: skip events we've already processed (e.g. re-delivered
        // by the server after a reconnection).
        if !event.event_id.is_empty() && dedup.is_duplicate(&event.event_id) {
            debug!(
                provider_id = %event.provider_id,
                event_id = %event.event_id,
                "dropping duplicate event"
            );
            continue;
        }

        // Security check: only process messages from the owner
        if let Some(ref owner_id) = provider.owner_open_id {
            let sender_id = event.source.user_id.as_deref().unwrap_or("");
            if sender_id != owner_id {
                info!(
                    provider_id = %event.provider_id,
                    event_id = %event.event_id,
                    sender_open_id = %sender_id,
                    owner_open_id = %owner_id,
                    "rejecting message from non-owner user"
                );
                let log = ImMessageLog {
                    id: uuid_short(),
                    provider_id: event.provider_id.clone(),
                    direction: MessageDirection::Inbound,
                    status: MessageStatus::Rejected,
                    timestamp: now_ms(),
                    target_id: None,
                    target_name: None,
                    message_id: event.source.message_id.clone(),
                    msg_type: event.message.as_ref().and_then(|m| m.raw_type.clone()),
                    content_preview: event.message.as_ref().map(|m| truncate_str(&m.text, 200)),
                    trigger: Some("websocket".to_string()),
                    error: Some(format!("rejected: sender {} is not owner", sender_id)),
                    sender_open_id: Some(sender_id.to_string()),
                    event_id: Some(event.event_id.clone()),
                    reaction_added: None,
                };
                let _ = message_log_store.add(log);
                continue;
            }
        }

        info!(
            provider_id = %event.provider_id,
            event_id = %event.event_id,
            event_type = %event.event_type,
            message_text = ?event.message.as_ref().map(|m| &m.text),
            "received inbound event from owner"
        );

        // Store the event in history
        if let Err(e) = event_store.add(event.clone()) {
            error!(error = %e, "failed to store event");
        }

        // Add "OK" reaction to acknowledge receipt
        let mut reaction_added = None;
        if let Some(ref message_id) = event.source.message_id {
            match feishu.add_reaction(&provider, message_id, "OK").await {
                Ok(()) => {
                    info!(message_id = %message_id, "added OK reaction to message");
                    reaction_added = Some(true);
                }
                Err(e) => {
                    error!(message_id = %message_id, error = %e, "failed to add OK reaction");
                    reaction_added = Some(false);
                }
            }
        }

        // Record inbound message log
        let log = ImMessageLog {
            id: uuid_short(),
            provider_id: event.provider_id.clone(),
            direction: MessageDirection::Inbound,
            status: MessageStatus::Success,
            timestamp: now_ms(),
            target_id: None,
            target_name: None,
            message_id: event.source.message_id.clone(),
            msg_type: event.message.as_ref().and_then(|m| m.raw_type.clone()),
            content_preview: event.message.as_ref().map(|m| truncate_str(&m.text, 200)),
            trigger: Some("websocket".to_string()),
            error: None,
            sender_open_id: event.source.user_id.clone(),
            event_id: Some(event.event_id.clone()),
            reaction_added,
        };
        if let Err(e) = message_log_store.add(log) {
            error!(error = %e, "failed to store inbound message log");
        }

        // --- Route matching & action execution ---
        let routes = route_store.list();
        let matches = ImEventRouter::match_routes(&event, &routes);
        if matches.is_empty() {
            // No route matched — try default agent chat if agent is enabled
            let agent_config = agent_config_store.load();
            if agent_config.enabled {
                if let Some(ref msg) = event.message {
                    if !msg.text.is_empty() {
                        let session_key =
                            build_session_key(&event.provider_id, event.source.user_id.as_deref());

                        // ── Guide/Queue mode: check if session is busy ──
                        if agent_session_manager.is_session_active(&session_key) {
                            handle_busy_message(
                                &msg.text,
                                &session_key,
                                BusyMessageContext {
                                    queue_manager: &queue_manager,
                                    feishu: &feishu,
                                    provider: &provider,
                                    event: &event,
                                    message_log_store: &message_log_store,
                                    agent_session_manager: &agent_session_manager,
                                },
                            )
                            .await;
                            continue;
                        }

                        // /status — handle directly without entering agent pipeline
                        if msg.text.trim() == "/status" {
                            let detail = agent_session_manager.get_session_detail(&session_key);
                            let reply = build_im_status_text(detail.as_ref());
                            send_agent_reply(
                                &feishu,
                                &provider,
                                &event,
                                &reply,
                                &message_log_store,
                            )
                            .await;
                            continue;
                        }

                        // Session is free — start processing with select! interleaving
                        run_agent_chat_with_interleave(
                            &mut rx,
                            &feishu,
                            &provider,
                            &event,
                            &agent_client,
                            &agent_config_store,
                            &agent_tools,
                            &agent_session_manager,
                            &queue_manager,
                            &session_key,
                            &msg.text,
                            None,
                            &mut mcp_manager,
                            &message_log_store,
                        )
                        .await;
                    }
                }
            }
            continue;
        }

        // Execute first matched route
        let route_match = &matches[0];
        info!(
            route_id = %route_match.route.id,
            route_name = %route_match.route.name,
            "executing matched route action"
        );

        match &route_match.route.action {
            ImRouteAction::RunScriptAndReply { .. } => {
                // Script execution (existing logic, kept as-is for this route type)
                info!(route_id = %route_match.route.id, "RunScriptAndReply action matched (execution handled by task executor)");
            }
            ImRouteAction::AgentChat {
                system_prompt,
                reply_target: _,
                ..
            } => {
                let message_text = route_match.message_text.as_deref().unwrap_or("");
                if message_text.is_empty() {
                    continue;
                }
                let session_key =
                    build_session_key(&event.provider_id, event.source.user_id.as_deref());

                // ── Guide/Queue mode: check if session is busy ──
                if agent_session_manager.is_session_active(&session_key) {
                    handle_busy_message(
                        message_text,
                        &session_key,
                        BusyMessageContext {
                            queue_manager: &queue_manager,
                            feishu: &feishu,
                            provider: &provider,
                            event: &event,
                            message_log_store: &message_log_store,
                            agent_session_manager: &agent_session_manager,
                        },
                    )
                    .await;
                    continue;
                }

                // /status — handle directly without entering agent pipeline
                if message_text.trim() == "/status" {
                    let detail = agent_session_manager.get_session_detail(&session_key);
                    let reply = build_im_status_text(detail.as_ref());
                    send_agent_reply(&feishu, &provider, &event, &reply, &message_log_store).await;
                    continue;
                }

                run_agent_chat_with_interleave(
                    &mut rx,
                    &feishu,
                    &provider,
                    &event,
                    &agent_client,
                    &agent_config_store,
                    &agent_tools,
                    &agent_session_manager,
                    &queue_manager,
                    &session_key,
                    message_text,
                    system_prompt.as_deref(),
                    &mut mcp_manager,
                    &message_log_store,
                )
                .await;
            }
        }
    }

    mcp_manager.shutdown().await;

    info!(
        provider_id = %provider.id,
        "event processing loop ended"
    );
}

// ---------------------------------------------------------------------------
// Guide/Queue mode: handle messages when session is busy
// ---------------------------------------------------------------------------

/// Handle an incoming message when the target session is already busy.
///
/// Behavior:
/// - `/status`: show session status (simplified when busy) or busy status
/// - `/q <text>`: push message to FIFO queue, reply with queue status
/// - `/rq <N>`: remove queued message #N, reply with updated queue status
/// - Otherwise (guide mode): inject message into the guide channel for mid-turn consumption
struct BusyMessageContext<'a> {
    queue_manager: &'a Arc<SessionQueueManager>,
    feishu: &'a Arc<crate::im_gateway::feishu::FeishuProvider>,
    provider: &'a ImProviderConfig,
    event: &'a ImEvent,
    message_log_store: &'a Arc<ImMessageLogStore>,
    agent_session_manager: &'a Arc<ImAgentSessionManager>,
}

async fn handle_busy_message(msg_text: &str, session_key: &str, ctx: BusyMessageContext<'_>) {
    let queue_manager = ctx.queue_manager;
    let feishu = ctx.feishu;
    let provider = ctx.provider;
    let event = ctx.event;
    let message_log_store = ctx.message_log_store;
    let agent_session_manager = ctx.agent_session_manager;
    let trimmed = msg_text.trim();

    // /status — show session status or busy indicator
    if trimmed == "/status" {
        // Try to get session detail from idle sessions
        if let Some(detail) = agent_session_manager.get_session_detail(session_key) {
            let real = detail
                .total_tokens_used
                .map(|t| t.to_string())
                .unwrap_or_else(|| "N/A".to_string());
            let reply = format!(
                "会话状态:\n- 消息数: {}\n- 估算 token: ~{}\n- API 累计 token: {}\n- 压缩次数: {}\n- 历史版本: {}\n- 状态: 空闲",
                detail.message_count, detail.estimated_tokens, real, detail.compaction_count, detail.history_version
            );
            send_agent_reply(feishu, provider, event, &reply, message_log_store).await;
        } else {
            // Session is currently being processed (taken out of the pool)
            let queue_items = queue_manager.queue_status(session_key);
            let queue_info = if queue_items.is_empty() {
                "无排队消息".to_string()
            } else {
                format!("{} 条排队消息", queue_items.len())
            };
            let reply = format!(
                "会话状态:\n- 状态: 🔵 正在处理中\n- 排队: {}\n\n请等待当前任务完成后再查询详细状态。",
                queue_info
            );
            send_agent_reply(feishu, provider, event, &reply, message_log_store).await;
        }
        return;
    }

    // /q <text> — queue mode
    if let Some(rest) = trimmed.strip_prefix("/q ") {
        let queue_text = rest.trim();
        if queue_text.is_empty() {
            send_agent_reply(
                feishu,
                provider,
                event,
                "用法: /q <消息内容>",
                message_log_store,
            )
            .await;
            return;
        }
        match queue_manager.push_queue(session_key, queue_text.to_string()) {
            Ok(items) => {
                let reply = format_queue_status("✅ 已加入排队", &items);
                send_agent_reply(feishu, provider, event, &reply, message_log_store).await;
            }
            Err(err) => {
                send_agent_reply(
                    feishu,
                    provider,
                    event,
                    &format!("❌ {err}"),
                    message_log_store,
                )
                .await;
            }
        }
        return;
    }

    // /rq <N> — remove queued message
    if let Some(rest) = trimmed.strip_prefix("/rq ") {
        let rest = rest.trim();
        match rest.parse::<u64>() {
            Ok(seq) => {
                if queue_manager.remove_queue(session_key, seq) {
                    let items = queue_manager.queue_status(session_key);
                    let reply = format_queue_status(&format!("🗑️ 已删除 #{seq}"), &items);
                    send_agent_reply(feishu, provider, event, &reply, message_log_store).await;
                } else {
                    send_agent_reply(
                        feishu,
                        provider,
                        event,
                        &format!("❌ 未找到排队消息 #{seq}"),
                        message_log_store,
                    )
                    .await;
                }
            }
            Err(_) => {
                send_agent_reply(
                    feishu,
                    provider,
                    event,
                    "用法: /rq <序号>（如 /rq 1）",
                    message_log_store,
                )
                .await;
            }
        }
        return;
    }

    // Other builtin commands that need session state — defer until session is free
    if matches!(
        trimmed,
        "/clear" | "/reset" | "/undo" | "/compact" | "/resume" | "/goal" | "/skill"
    ) || trimmed.starts_with("/undo ")
        || trimmed.starts_with("/goal ")
        || trimmed.starts_with("/skill ")
    {
        let reply = format!(
            "⏳ Agent 正在处理中，{} 命令需要等待当前任务完成后执行。\n\n\
             可用操作:\n\
             - /q <消息> — 排队消息\n\
             - /rq <序号> — 取消排队\n\
             - /status — 查看状态\n\
             - /help — 查看帮助",
            trimmed.split_whitespace().next().unwrap_or(trimmed)
        );
        send_agent_reply(feishu, provider, event, &reply, message_log_store).await;
        return;
    }

    // Default: guide mode — inject into the guide channel
    let previous = queue_manager.inject_guide(session_key, trimmed.to_string());
    let reply = if previous.is_some() {
        "🔀 已更新引导消息（替换前一条未处理的引导），将在当前工具调用完成后生效"
    } else {
        "🔀 已注入引导消息，将在当前工具调用完成后生效"
    };
    info!(
        session_key = %session_key,
        guide_msg_len = trimmed.len(),
        replaced_previous = previous.is_some(),
        "guide message injected via IM"
    );
    send_agent_reply(feishu, provider, event, reply, message_log_store).await;
}

/// Format queue status as a user-friendly string.
fn format_queue_status(
    header: &str,
    items: &[crate::im_gateway::queue_manager::QueueItem],
) -> String {
    let mut text = header.to_string();
    if items.is_empty() {
        text.push_str("\n\n📋 排队已清空");
    } else {
        text.push_str(&format!("\n\n📋 当前排队（{}条）：", items.len()));
        for item in items {
            let preview = truncate_str(&item.message, 60);
            text.push_str(&format!(
                "\n{}. [#{}] {}",
                items.iter().position(|i| i.seq == item.seq).unwrap_or(0) + 1,
                item.seq,
                preview
            ));
        }
    }
    text
}

/// Run agent chat with `tokio::select!` interleaving.
///
/// While the agent turn is executing, this function continues to receive events
/// from the channel and routes them through `handle_busy_message` (guide/queue).
/// After the turn completes, it drains the queue by processing queued messages
/// one by one.
#[allow(clippy::too_many_arguments)]
async fn run_agent_chat_with_interleave(
    rx: &mut mpsc::UnboundedReceiver<ImEvent>,
    feishu: &Arc<crate::im_gateway::feishu::FeishuProvider>,
    provider: &ImProviderConfig,
    initial_event: &ImEvent,
    agent_client: &Arc<ImAgentClient>,
    agent_config_store: &Arc<ImAgentConfigStore>,
    agent_tools: &Arc<ImAgentToolRegistry>,
    agent_session_manager: &Arc<ImAgentSessionManager>,
    queue_manager: &Arc<SessionQueueManager>,
    session_key: &str,
    initial_message: &str,
    system_prompt_override: Option<&str>,
    mcp_manager: &mut ImMcpManager,
    message_log_store: &Arc<ImMessageLogStore>,
) {
    // Set up the guide channel before starting the turn
    let guide_channel = queue_manager.get_or_create_guide_channel(session_key);

    let agent_config = agent_config_store.load();
    let mut current_msg = initial_message.to_string();

    // Queue drain loop: process initial message, then drain queued messages
    loop {
        // Clone into a local so the future borrows the local, not `current_msg`.
        let msg_for_turn = current_msg.clone();

        // Run agent chat with interleaved event processing
        let chat_future = AssertUnwindSafe(process_agent_chat(
            feishu,
            provider,
            initial_event,
            agent_client,
            &agent_config,
            agent_tools,
            agent_session_manager,
            session_key,
            &msg_for_turn,
            system_prompt_override,
            Some(mcp_manager),
            message_log_store,
            Some(guide_channel.clone()),
        ))
        .catch_unwind();

        // Use select! to interleave event processing with agent chat
        tokio::pin!(chat_future);
        loop {
            tokio::select! {
                result = &mut chat_future => {
                    // Chat completed (or panicked)
                    if let Err(panic_err) = result {
                        let panic_msg = panic_err
                            .downcast_ref::<String>()
                            .map(|s| s.as_str())
                            .or_else(|| panic_err.downcast_ref::<&str>().copied())
                            .unwrap_or("unknown panic");
                        error!(
                            session_key = %session_key,
                            panic = %panic_msg,
                            "process_agent_chat panicked, event loop continues"
                        );
                        agent_session_manager.release_active(session_key);
                        let _ = send_error_card_to_owner(
                            feishu,
                            provider,
                            &format!("Agent 内部错误 (panic): {}", truncate_str(panic_msg, 200)),
                        )
                        .await;
                    }
                    break;
                }
                Some(event) = rx.recv() => {
                    // Handle concurrent event while chat is running
                    handle_concurrent_event_during_chat(
                        &event,
                        provider,
                        session_key,
                        queue_manager,
                        feishu,
                        message_log_store,
                        agent_session_manager,
                        agent_config_store,
                    )
                    .await;
                }
            }
        }

        // After turn completes, first check for unconsumed guide message.
        // The guide_channel is only consumed inside the turn loop after tool calls.
        // If the model's last response was finish_reason=stop (no tool calls), the
        // guide message is never consumed. We must drain it here to avoid silent loss.
        let unconsumed_guide = guide_channel.lock().unwrap().take();
        if let Some(guide_msg) = unconsumed_guide {
            info!(
                session_key = %session_key,
                guide_msg_len = guide_msg.len(),
                "processing unconsumed guide message after turn completed"
            );
            current_msg = guide_msg;
            continue;
        }

        // Then check for queued messages
        match queue_manager.pop_queue(session_key) {
            Some(next_msg) => {
                let remaining = queue_manager.queue_status(session_key).len();
                info!(
                    session_key = %session_key,
                    queued_msg_len = next_msg.len(),
                    remaining_queue = remaining,
                    "processing next queued message"
                );
                // Notify user which queued message is being processed
                let preview = truncate_str(&next_msg, 80);
                let notice = if remaining > 0 {
                    format!("📋 正在处理排队消息: {preview}\n（剩余 {remaining} 条排队）")
                } else {
                    format!("📋 正在处理排队消息: {preview}")
                };
                send_agent_reply(feishu, provider, initial_event, &notice, message_log_store).await;
                current_msg = next_msg;
                // Continue the loop to process the next queued message
            }
            None => {
                // No more queued messages, clean up and exit
                queue_manager.clear_session(session_key);
                break;
            }
        }
    }
}

/// Handle an event that arrives during an active agent chat.
///
/// Performs the same security/logging/routing as the main loop, but for events
/// that come in while a chat is being processed. Messages for the active session
/// are routed through guide/queue mode.
#[allow(clippy::too_many_arguments)]
async fn handle_concurrent_event_during_chat(
    event: &ImEvent,
    provider: &ImProviderConfig,
    active_session_key: &str,
    queue_manager: &Arc<SessionQueueManager>,
    feishu: &Arc<crate::im_gateway::feishu::FeishuProvider>,
    message_log_store: &Arc<ImMessageLogStore>,
    agent_session_manager: &Arc<ImAgentSessionManager>,
    agent_config_store: &Arc<ImAgentConfigStore>,
) {
    // Security check: only process messages from the owner
    if let Some(ref owner_id) = provider.owner_open_id {
        let sender_id = event.source.user_id.as_deref().unwrap_or("");
        if sender_id != owner_id {
            debug!(
                event_id = %event.event_id,
                "rejecting concurrent event from non-owner"
            );
            return;
        }
    }

    let msg_text = match event.message.as_ref() {
        Some(m) if !m.text.is_empty() => &m.text,
        _ => return,
    };

    let session_key = build_session_key(&event.provider_id, event.source.user_id.as_deref());

    // Check if this event is for the currently active session
    if session_key == active_session_key {
        // Session-free commands are still instant
        let agent_config = agent_config_store.load();
        if let Some(response) =
            bifrost_agent::handle_session_free_command(&session_key, msg_text, &agent_config)
        {
            send_agent_reply(feishu, provider, event, &response, message_log_store).await;
            return;
        }
        // Route through guide/queue mode
        handle_busy_message(
            msg_text,
            &session_key,
            BusyMessageContext {
                queue_manager,
                feishu,
                provider,
                event,
                message_log_store,
                agent_session_manager,
            },
        )
        .await;
    } else {
        // Different session — check if it's also busy
        if agent_session_manager.is_session_active(&session_key) {
            handle_busy_message(
                msg_text,
                &session_key,
                BusyMessageContext {
                    queue_manager,
                    feishu,
                    provider,
                    event,
                    message_log_store,
                    agent_session_manager,
                },
            )
            .await;
        } else {
            // Session is free but we can't process it now (MCP is in use).
            // Queue it for later processing.
            let _ = queue_manager.push_queue(&session_key, msg_text.to_string());
            send_agent_reply(
                feishu,
                provider,
                event,
                "⏳ 消息已排队，将在当前任务完成后处理。",
                message_log_store,
            )
            .await;
        }
    }
}

/// Process an agent chat: run the full turn loop (with tool calls), send reply via Feishu, log the outbound message.
#[allow(clippy::too_many_arguments)]
async fn process_agent_chat(
    feishu: &Arc<crate::im_gateway::feishu::FeishuProvider>,
    provider: &ImProviderConfig,
    event: &ImEvent,
    agent_client: &Arc<ImAgentClient>,
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
    agent_tools: &Arc<ImAgentToolRegistry>,
    session_manager: &Arc<ImAgentSessionManager>,
    session_key: &str,
    user_message: &str,
    system_prompt_override: Option<&str>,
    mcp: Option<&mut ImMcpManager>,
    message_log_store: &Arc<ImMessageLogStore>,
    guide_channel: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,
) {
    info!(
        session_key = %session_key,
        user_message_len = user_message.len(),
        "invoking agent chat (turn loop)"
    );

    // ── Session-free command fast path ────────────────────────────────────
    // Commands like /help, /remember, /memories, /forget don't need session
    // state and can respond immediately even while a turn loop is running.
    if let Some(response) =
        bifrost_agent::handle_session_free_command(session_key, user_message, agent_config)
    {
        debug!(
            session_key = %session_key,
            "handled session-free command without taking session"
        );
        send_agent_reply(feishu, provider, event, &response, message_log_store).await;
        return;
    }

    // ── Busy check ───────────────────────────────────────────────────────
    // If another turn loop is already processing this session, reject early
    // instead of creating a duplicate empty session.
    let mut session = match session_manager.try_take_session(session_key) {
        Some(s) => s,
        None => {
            info!(
                session_key = %session_key,
                "session is busy, rejecting concurrent request"
            );
            let busy_msg =
                "⏳ Agent 正在处理中，请稍后再试。\n\n提示: /help、/remember、/memories、/forget 等命令即使在处理中也可立即响应。";
            send_agent_reply(feishu, provider, event, busy_msg, message_log_store).await;
            return;
        }
    };
    session.source = "feishu".to_string();
    session.guide_channel = guide_channel;

    // Set up plan update channel for real-time plan card rendering.
    // The turn loop pushes plan steps through this channel; a background task
    // sends (first time) or patches (subsequent) a single Feishu card.
    let (plan_tx, mut plan_rx) =
        tokio::sync::mpsc::unbounded_channel::<(Vec<bifrost_agent::PlanStep>, Option<String>)>();
    session.plan_sender = Some(plan_tx);
    {
        let feishu = feishu.clone();
        let provider = provider.clone();
        let target_open_id = provider
            .owner_open_id
            .as_deref()
            .or(event.source.user_id.as_deref())
            .unwrap_or("")
            .to_string();
        tokio::spawn(async move {
            let mut plan_card_msg_id: Option<String> = None;
            while let Some((steps, title)) = plan_rx.recv().await {
                let card = build_plan_card(&steps, title.as_deref());
                if let Some(ref msg_id) = plan_card_msg_id {
                    // Patch existing card
                    if let Err(e) = feishu.patch_card(&provider, msg_id, card).await {
                        tracing::warn!(error = %e, "failed to patch plan card");
                    }
                } else if !target_open_id.is_empty() {
                    // Send new card
                    let target = crate::im_gateway::types::ImTarget {
                        id: "__plan_card__".to_string(),
                        provider_id: provider.id.clone(),
                        display_name: "Plan Card".to_string(),
                        enabled: true,
                        receive_id_type: "open_id".to_string(),
                        receive_id: target_open_id.clone(),
                        default_msg_type: "interactive".to_string(),
                        created_at: 0,
                        updated_at: 0,
                    };
                    match feishu
                        .send_card(
                            &provider,
                            &target,
                            card,
                            crate::im_gateway::types::SendOptions::default(),
                        )
                        .await
                    {
                        Ok(r) => {
                            plan_card_msg_id = r.message_id;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to send plan card");
                        }
                    }
                }
            }
        });
    }

    // Create a conversation recorder for persistence if enabled
    let mut recorder = if !agent_config.is_ephemeral() {
        let should_persist = agent_config
            .history
            .as_ref()
            .map(|h| h.persistence != bifrost_agent::config::HistoryPersistence::None)
            .unwrap_or(true);
        if should_persist {
            // Reuse existing recorder from session, or create a new one
            if session.recorder.is_some() {
                session.recorder.take()
            } else {
                let data_dir = bifrost_agent::config::agent_home_dir();
                let max_bytes = agent_config.history.as_ref().and_then(|h| h.max_bytes);
                let mut rec =
                    ConversationRecorder::new_with_max_bytes(&data_dir, session_key, max_bytes);
                // Record session start metadata
                let _ = rec.record_session_start(
                    session_key,
                    serde_json::json!({
                        "model": agent_config.model,
                        "provider": agent_config.model_provider,
                        "source": "feishu",
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

    let result = crate::im_gateway::run_turn_with_mcp(
        agent_client,
        agent_config,
        &mut session,
        agent_tools,
        mcp,
        user_message,
        system_prompt_override,
        recorder.as_mut(),
    )
    .await;

    // If the turn failed, retry once: re-take MCP (already consumed), simplified call
    let result = match result {
        Ok(r) => Ok(r),
        Err(first_err) => {
            warn!(
                session_key = %session_key,
                error = %first_err,
                "agent turn failed, retrying once"
            );
            // Brief delay before retry
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            // Retry without MCP (already consumed) to simplify the call
            match crate::im_gateway::run_turn_with_mcp(
                agent_client,
                agent_config,
                &mut session,
                agent_tools,
                None, // MCP already consumed in first attempt
                user_message,
                system_prompt_override,
                recorder.as_mut(),
            )
            .await
            {
                Ok(r) => {
                    info!(session_key = %session_key, "agent turn retry succeeded");
                    Ok(r)
                }
                Err(retry_err) => {
                    error!(
                        session_key = %session_key,
                        first_error = %first_err,
                        retry_error = %retry_err,
                        "agent turn retry also failed"
                    );
                    Err(retry_err)
                }
            }
        }
    };

    // Put the recorder back into the session so it persists across turns.
    // Skip this if session was cleared during the turn (/clear drops the recorder
    // deliberately so a new file will be created for the fresh session).
    if recorder.is_some() && !session.memory_cleared {
        session.recorder = recorder;
    }

    // Extract session title before returning the session
    let session_title = session.title.clone();

    // Return session after turn completes
    session_manager.return_session(session);

    // Best-effort cleanup
    session_manager.cleanup_expired();

    // Separate main response and tool calls for card rendering
    let (main_response, tool_calls_panel, plan_steps) = match result {
        Ok(turn_result) => {
            // Log work_dir switch if it happened
            if let Some(ref new_dir) = turn_result.work_dir_switched {
                info!(
                    session_key = %session_key,
                    new_work_dir = %new_dir,
                    "session work directory switched via agent tool"
                );
            }
            // Build tool calls info for collapsible panel
            let panel = if !turn_result.tool_calls_log.is_empty() {
                let mut tool_md = String::new();
                for log in &turn_result.tool_calls_log {
                    let icon = if log.success { "✅" } else { "❌" };
                    tool_md.push_str(&format!("{} `{}`\n", icon, log.tool_name));
                    let result_preview = truncate_bytes_with_suffix(&log.result, 500, "...");
                    tool_md.push_str(&format!("```\n{}\n```\n", result_preview));
                }
                Some((turn_result.tool_calls_log.len(), tool_md))
            } else {
                None
            };
            let plan = turn_result.plan_steps;
            (turn_result.response, panel, plan)
        }
        Err(e) => {
            error!(
                session_key = %session_key,
                error = %e,
                "agent chat failed after retry"
            );
            (
                format!(
                    "⚠️ **Agent 执行失败**\n\n**错误原因**: {}\n\n请稍后重试，或发送 `/clear` 重置会话。",
                    truncate_str(&e, 300)
                ),
                None,
                None,
            )
        }
    };

    // Build an ephemeral target using owner's open_id or event sender
    let target_open_id = provider
        .owner_open_id
        .as_deref()
        .or(event.source.user_id.as_deref())
        .unwrap_or("");
    if target_open_id.is_empty() {
        error!("no target open_id to send agent reply");
        return;
    }

    let reply_target = crate::im_gateway::types::ImTarget {
        id: "__agent_reply__".to_string(),
        provider_id: provider.id.clone(),
        display_name: "Agent Reply".to_string(),
        enabled: true,
        receive_id_type: "open_id".to_string(),
        receive_id: target_open_id.to_string(),
        default_msg_type: "interactive".to_string(),
        created_at: 0,
        updated_at: 0,
    };

    // Build Feishu Card JSON 2.0: main response visible, tool calls in collapsible panel
    let main_response =
        crate::im_gateway::markdown_converter::convert_to_feishu_markdown(&main_response);
    let mut elements = vec![serde_json::json!({
        "tag": "markdown",
        "content": main_response,
        "element_id": "agent_reply"
    })];
    // Plan progress panel (between response and tool calls)
    if let Some(ref steps) = plan_steps {
        let completed = steps
            .iter()
            .filter(|s| matches!(s.status, bifrost_agent::PlanStepStatus::Completed))
            .count();
        let total = steps.len();
        let mut plan_md = String::new();
        for s in steps {
            plan_md.push_str(&format!("{} {}\n", s.status.emoji(), s.step));
        }
        elements.push(serde_json::json!({
            "tag": "collapsible_panel",
            "expanded": true,
            "background_color": "grey",
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": format!("📋 任务计划（{}/{}）", completed, total)
                }
            },
            "vertical_spacing": "2px",
            "padding": "4px 8px 4px 8px",
            "elements": [{
                "tag": "markdown",
                "content": plan_md
            }]
        }));
    }
    if let Some((count, ref tool_md)) = tool_calls_panel {
        elements.push(serde_json::json!({
            "tag": "collapsible_panel",
            "expanded": false,
            "background_color": "grey",
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": format!("🔧 工具调用记录（{}次）", count)
                }
            },
            "vertical_spacing": "2px",
            "padding": "4px 8px 4px 8px",
            "elements": [{
                "tag": "markdown",
                "content": tool_md
            }]
        }));
    }
    let rich_card_title = session_title.as_deref().unwrap_or("Bifrost AI");
    let card = serde_json::json!({
        "schema": "2.0",
        "config": {
            "width_mode": "fill",
            "update_multi": true
        },
        "header": {
            "template": "blue",
            "title": {
                "tag": "plain_text",
                "content": rich_card_title
            }
        },
        "body": {
            "elements": elements
        }
    });

    let send_result = feishu
        .send_card(
            provider,
            &reply_target,
            card,
            crate::im_gateway::types::SendOptions::default(),
        )
        .await;

    // Record outbound message log
    let (status, message_id, error_msg) = match &send_result {
        Ok(r) => (MessageStatus::Success, r.message_id.clone(), None),
        Err(e) => (MessageStatus::Failed, None, Some(e.to_string())),
    };
    let log = ImMessageLog {
        id: uuid_short(),
        provider_id: provider.id.clone(),
        direction: MessageDirection::Outbound,
        status,
        timestamp: now_ms(),
        target_id: Some("__agent_reply__".to_string()),
        target_name: Some("Agent Reply".to_string()),
        message_id,
        msg_type: Some("interactive".to_string()),
        content_preview: Some(truncate_str(&main_response, 200)),
        trigger: Some("agent".to_string()),
        error: error_msg,
        sender_open_id: None,
        event_id: Some(event.event_id.clone()),
        reaction_added: None,
    };
    if let Err(e) = message_log_store.add(log) {
        error!(error = %e, "failed to store agent outbound message log");
    }

    match send_result {
        Ok(_) => info!(session_key = %session_key, "agent reply sent successfully"),
        Err(e) => error!(session_key = %session_key, error = %e, "failed to send agent reply"),
    }
}

/// Send an agent reply text via Feishu card and log the outbound message.
///
/// Extracted helper to share between the main turn loop and session-free command fast path.
async fn send_agent_reply(
    feishu: &Arc<crate::im_gateway::feishu::FeishuProvider>,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_text: &str,
    message_log_store: &Arc<ImMessageLogStore>,
) {
    send_agent_reply_with_title(feishu, provider, event, reply_text, message_log_store, None).await;
}

/// Send an agent reply with a custom card title.
async fn send_agent_reply_with_title(
    feishu: &Arc<crate::im_gateway::feishu::FeishuProvider>,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_text: &str,
    message_log_store: &Arc<ImMessageLogStore>,
    title: Option<&str>,
) {
    let target_open_id = provider
        .owner_open_id
        .as_deref()
        .or(event.source.user_id.as_deref())
        .unwrap_or("");
    if target_open_id.is_empty() {
        error!("no target open_id to send agent reply");
        return;
    }

    let reply_target = crate::im_gateway::types::ImTarget {
        id: "__agent_reply__".to_string(),
        provider_id: provider.id.clone(),
        display_name: "Agent Reply".to_string(),
        enabled: true,
        receive_id_type: "open_id".to_string(),
        receive_id: target_open_id.to_string(),
        default_msg_type: "interactive".to_string(),
        created_at: 0,
        updated_at: 0,
    };

    let card_title = title.unwrap_or("Bifrost AI");
    let converted_text =
        crate::im_gateway::markdown_converter::convert_to_feishu_markdown(reply_text);
    let card = serde_json::json!({
        "schema": "2.0",
        "config": {
            "width_mode": "fill",
            "update_multi": true
        },
        "header": {
            "template": "blue",
            "title": {
                "tag": "plain_text",
                "content": card_title
            }
        },
        "body": {
            "elements": [
                {
                    "tag": "markdown",
                    "content": converted_text,
                    "element_id": "agent_reply"
                }
            ]
        }
    });

    let send_result = feishu
        .send_card(
            provider,
            &reply_target,
            card,
            crate::im_gateway::types::SendOptions::default(),
        )
        .await;

    let (status, message_id, error_msg) = match &send_result {
        Ok(r) => (MessageStatus::Success, r.message_id.clone(), None),
        Err(e) => (MessageStatus::Failed, None, Some(e.to_string())),
    };
    let log = ImMessageLog {
        id: uuid_short(),
        provider_id: provider.id.clone(),
        direction: MessageDirection::Outbound,
        status,
        timestamp: now_ms(),
        target_id: Some("__agent_reply__".to_string()),
        target_name: Some("Agent Reply".to_string()),
        message_id,
        msg_type: Some("interactive".to_string()),
        content_preview: Some(truncate_str(reply_text, 200)),
        trigger: Some("agent".to_string()),
        error: error_msg,
        sender_open_id: None,
        event_id: Some(event.event_id.clone()),
        reaction_added: None,
    };
    if let Err(e) = message_log_store.add(log) {
        error!(error = %e, "failed to store agent outbound message log");
    }

    match send_result {
        Ok(_) => debug!("agent reply sent successfully"),
        Err(e) => error!(error = %e, "failed to send agent reply"),
    }
}

/// Best-effort helper to send an error notification card to the provider owner.
async fn send_error_card_to_owner(
    feishu: &Arc<crate::im_gateway::feishu::FeishuProvider>,
    provider: &ImProviderConfig,
    error_message: &str,
) {
    let target_open_id = match provider.owner_open_id.as_deref() {
        Some(id) if !id.is_empty() => id,
        _ => return,
    };

    let target = crate::im_gateway::types::ImTarget {
        id: "__error_notify__".to_string(),
        provider_id: provider.id.clone(),
        display_name: "Error Notify".to_string(),
        enabled: true,
        receive_id_type: "open_id".to_string(),
        receive_id: target_open_id.to_string(),
        default_msg_type: "interactive".to_string(),
        created_at: 0,
        updated_at: 0,
    };

    let converted_error =
        crate::im_gateway::markdown_converter::convert_to_feishu_markdown(error_message);
    let card = serde_json::json!({
        "schema": "2.0",
        "config": { "width_mode": "fill" },
        "header": {
            "template": "red",
            "title": { "tag": "plain_text", "content": "Bifrost Agent Error" }
        },
        "body": {
            "elements": [{
                "tag": "markdown",
                "content": converted_error
            }]
        }
    });

    if let Err(e) = feishu
        .send_card(
            provider,
            &target,
            card,
            crate::im_gateway::types::SendOptions::default(),
        )
        .await
    {
        warn!(error = %e, "failed to send error card to owner");
    }
}

async fn handle_provider_policy(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    match *req.method() {
        Method::GET => {
            let Some(_provider) = service.provider_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Provider not found");
            };
            // Return policy placeholder — policy store will be integrated in future
            json_response(&serde_json::json!({
                "provider_id": id,
                "permissions": [],
                "script_policy_binding": null,
            }))
        }
        Method::PATCH => {
            let _patch: serde_json::Value = match read_body_json(req).await {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let Some(_provider) = service.provider_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Provider not found");
            };
            // Policy update placeholder
            json_response(&serde_json::json!({"success": true}))
        }
        _ => method_not_allowed(),
    }
}

async fn handle_provider_policy_bind_shell(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let _body: serde_json::Value = match read_body_json(req).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let Some(_provider) = service.provider_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found");
    };
    // Bind-shell placeholder
    json_response(&serde_json::json!({"success": true}))
}

// ---------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------

async fn handle_targets(
    req: Request<Incoming>,
    service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let rest = rest.trim_end_matches('/');

    // GET /targets  |  POST /targets
    if rest.is_empty() {
        return match *req.method() {
            Method::GET => {
                let targets = service.target_store.list();
                json_response(&targets)
            }
            Method::POST => {
                let mut target: ImTarget = match read_body_json(req).await {
                    Ok(v) => v,
                    Err(resp) => return resp,
                };
                let now = now_ms();
                if target.created_at == 0 {
                    target.created_at = now;
                }
                target.updated_at = now;
                match service.target_store.add(target) {
                    Ok(()) => json_response(&serde_json::json!({"success": true})),
                    Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
                }
            }
            _ => method_not_allowed(),
        };
    }

    // /:id
    if let Some(id_str) = rest.strip_prefix('/') {
        let id = id_str.split('/').next().unwrap_or(id_str);
        if !id.is_empty() && !id.contains('/') {
            return handle_target_by_id(req, service, id).await;
        }
    }

    error_response(StatusCode::NOT_FOUND, "Target endpoint not found")
}

async fn handle_target_by_id(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    match *req.method() {
        Method::PATCH => {
            let patch: serde_json::Value = match read_body_json(req).await {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let Some(mut existing) = service.target_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Target not found");
            };
            apply_target_patch(&mut existing, &patch);
            match service.target_store.update(existing) {
                Ok(()) => json_response(&serde_json::json!({"success": true})),
                Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        Method::DELETE => match service.target_store.delete(id) {
            Ok(()) => json_response(&serde_json::json!({"success": true})),
            Err(e) => error_response(StatusCode::NOT_FOUND, &e.to_string()),
        },
        _ => method_not_allowed(),
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SendMessageRequest {
    target_id: String,
    #[serde(default = "default_msg_type")]
    msg_type: String,
    content: serde_json::Value,
}

fn default_msg_type() -> String {
    "interactive".to_string()
}

async fn handle_messages_send(
    req: Request<Incoming>,
    service: &ImGatewayService,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    let body: SendMessageRequest = match read_body_json(req).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let Some(target) = service.target_store.get(&body.target_id) else {
        return error_response(StatusCode::NOT_FOUND, "Target not found");
    };

    let Some(provider) = service.provider_store.get(&target.provider_id) else {
        return error_response(StatusCode::NOT_FOUND, "Provider not found for target");
    };

    if !provider.enabled {
        return error_response(StatusCode::BAD_REQUEST, "Provider is disabled");
    }

    if !target.enabled {
        return error_response(StatusCode::BAD_REQUEST, "Target is disabled");
    }

    // Build content preview
    let content_preview = build_content_preview(&body.msg_type, &body.content);

    // Send via connection manager's feishu provider
    let feishu = service.connection_manager.feishu_provider();
    let content_str = serde_json::to_string(&body.content).unwrap_or_default();
    let result = if body.msg_type == "text" {
        feishu.send_text(&provider, &target, &content_str).await
    } else {
        feishu
            .send_card(&provider, &target, body.content.clone(), Default::default())
            .await
    };

    // Record outbound message log
    let (status, message_id, error_msg) = match &result {
        Ok(r) => (MessageStatus::Success, r.message_id.clone(), None),
        Err(e) => (MessageStatus::Failed, None, Some(e.to_string())),
    };
    let log = ImMessageLog {
        id: uuid_short(),
        provider_id: provider.id.clone(),
        direction: MessageDirection::Outbound,
        status,
        timestamp: now_ms(),
        target_id: Some(body.target_id.clone()),
        target_name: Some(target.display_name.clone()),
        message_id,
        msg_type: Some(body.msg_type.clone()),
        content_preview,
        trigger: Some("api".to_string()),
        error: error_msg,
        sender_open_id: None,
        event_id: None,
        reaction_added: None,
    };
    if let Err(e) = service.message_log_store.add(log) {
        error!(error = %e, "failed to store outbound message log");
    }

    match result {
        Ok(result) => json_response(&result),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to send message: {e}"),
        ),
    }
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

async fn handle_routes(
    req: Request<Incoming>,
    service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let rest = rest.trim_end_matches('/');

    // GET /routes  |  POST /routes
    if rest.is_empty() {
        return match *req.method() {
            Method::GET => {
                let routes = service.route_store.list();
                json_response(&routes)
            }
            Method::POST => {
                let mut route: ImRoute = match read_body_json(req).await {
                    Ok(v) => v,
                    Err(resp) => return resp,
                };
                let now = now_ms();
                if route.created_at == 0 {
                    route.created_at = now;
                }
                route.updated_at = now;
                match service.route_store.add(route) {
                    Ok(()) => json_response(&serde_json::json!({"success": true})),
                    Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
                }
            }
            _ => method_not_allowed(),
        };
    }

    // Sub-paths: /:id, /:id/pause, /:id/resume
    if let Some(id_and_rest) = rest.strip_prefix('/') {
        // /:id/pause
        if let Some(id) = extract_segment_before(id_and_rest, "/pause") {
            return handle_route_pause(req, service, id).await;
        }
        // /:id/resume
        if let Some(id) = extract_segment_before(id_and_rest, "/resume") {
            return handle_route_resume(req, service, id).await;
        }
        // /:id
        let id = id_and_rest.split('/').next().unwrap_or(id_and_rest);
        if !id.is_empty() && !id.contains('/') {
            return handle_route_by_id(req, service, id).await;
        }
    }

    error_response(StatusCode::NOT_FOUND, "Route endpoint not found")
}

async fn handle_route_by_id(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    match *req.method() {
        Method::PATCH => {
            let patch: serde_json::Value = match read_body_json(req).await {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let Some(mut existing) = service.route_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Route not found");
            };
            apply_route_patch(&mut existing, &patch);
            match service.route_store.update(existing) {
                Ok(()) => json_response(&serde_json::json!({"success": true})),
                Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        Method::DELETE => match service.route_store.delete(id) {
            Ok(()) => json_response(&serde_json::json!({"success": true})),
            Err(e) => error_response(StatusCode::NOT_FOUND, &e.to_string()),
        },
        _ => method_not_allowed(),
    }
}

async fn handle_route_pause(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(mut route) = service.route_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Route not found");
    };
    route.enabled = false;
    route.updated_at = now_ms();
    match service.route_store.update(route) {
        Ok(()) => json_response(&serde_json::json!({"success": true, "enabled": false})),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn handle_route_resume(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(mut route) = service.route_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Route not found");
    };
    route.enabled = true;
    route.updated_at = now_ms();
    match service.route_store.update(route) {
        Ok(()) => json_response(&serde_json::json!({"success": true, "enabled": true})),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Schedules
// ---------------------------------------------------------------------------

async fn handle_schedules(
    req: Request<Incoming>,
    service: &ImGatewayService,
    rest: &str,
) -> Response<BoxBody> {
    let rest = rest.trim_end_matches('/');

    // GET /schedules  |  POST /schedules
    if rest.is_empty() {
        return match *req.method() {
            Method::GET => {
                let schedules = service.schedule_store.list();
                json_response(&schedules)
            }
            Method::POST => {
                let mut schedule: ImSchedule = match read_body_json(req).await {
                    Ok(v) => v,
                    Err(resp) => return resp,
                };
                let now = now_ms();
                if schedule.created_at == 0 {
                    schedule.created_at = now;
                }
                schedule.updated_at = now;
                match service.schedule_store.add(schedule) {
                    Ok(()) => json_response(&serde_json::json!({"success": true})),
                    Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
                }
            }
            _ => method_not_allowed(),
        };
    }

    // Sub-paths: /:id, /:id/pause, /:id/resume, /:id/run, /:id/runs
    if let Some(id_and_rest) = rest.strip_prefix('/') {
        // /:id/pause
        if let Some(id) = extract_segment_before(id_and_rest, "/pause") {
            return handle_schedule_pause(req, service, id).await;
        }
        // /:id/resume
        if let Some(id) = extract_segment_before(id_and_rest, "/resume") {
            return handle_schedule_resume(req, service, id).await;
        }
        // /:id/run
        if let Some(id) = extract_segment_before(id_and_rest, "/run") {
            return handle_schedule_run(req, service, id).await;
        }
        // /:id/runs
        if let Some(id) = extract_segment_before(id_and_rest, "/runs") {
            return handle_schedule_runs(&req, service, id);
        }
        // /:id
        let id = id_and_rest.split('/').next().unwrap_or(id_and_rest);
        if !id.is_empty() && !id.contains('/') {
            return handle_schedule_by_id(req, service, id).await;
        }
    }

    error_response(StatusCode::NOT_FOUND, "Schedule endpoint not found")
}

async fn handle_schedule_by_id(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    match *req.method() {
        Method::PATCH => {
            let patch: serde_json::Value = match read_body_json(req).await {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let Some(mut existing) = service.schedule_store.get(id) else {
                return error_response(StatusCode::NOT_FOUND, "Schedule not found");
            };
            apply_schedule_patch(&mut existing, &patch);
            match service.schedule_store.update(existing) {
                Ok(()) => json_response(&serde_json::json!({"success": true})),
                Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        Method::DELETE => match service.schedule_store.delete(id) {
            Ok(()) => json_response(&serde_json::json!({"success": true})),
            Err(e) => error_response(StatusCode::NOT_FOUND, &e.to_string()),
        },
        _ => method_not_allowed(),
    }
}

async fn handle_schedule_pause(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(mut schedule) = service.schedule_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Schedule not found");
    };
    schedule.enabled = false;
    schedule.updated_at = now_ms();
    match service.schedule_store.update(schedule) {
        Ok(()) => json_response(&serde_json::json!({"success": true, "enabled": false})),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn handle_schedule_resume(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(mut schedule) = service.schedule_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Schedule not found");
    };
    schedule.enabled = true;
    schedule.updated_at = now_ms();
    match service.schedule_store.update(schedule) {
        Ok(()) => json_response(&serde_json::json!({"success": true, "enabled": true})),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn handle_schedule_run(
    req: Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let Some(schedule) = service.schedule_store.get(id) else {
        return error_response(StatusCode::NOT_FOUND, "Schedule not found");
    };

    // Look up the target and provider
    let Some(target) = service.target_store.get(&schedule.target_id) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!("Target '{}' not found", schedule.target_id),
        );
    };
    let Some(provider) = service.provider_store.get(&target.provider_id) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!("Provider '{}' not found", target.provider_id),
        );
    };

    // Execute the script
    let run_id = uuid_short();
    let request = crate::im_gateway::types::ImTaskExecutionRequest {
        provider_id: provider.id.clone(),
        trigger_source: crate::im_gateway::types::TriggerSource::ManualRun,
        policy_id: None,
        script_policy_binding: None,
        script: schedule.script.clone(),
        timeout_ms: schedule.timeout_ms,
        max_output_bytes: schedule.max_output_bytes,
    };

    let match_ctx = crate::im_gateway::task_executor::MatchContext::default();
    let task_run = crate::im_gateway::task_executor::ImTaskExecutor::execute(
        &request,
        run_id.clone(),
        None,
        Some(schedule.id.clone()),
        &match_ctx,
    )
    .await;

    // Persist the run record
    let _ = service.run_store.add(task_run.clone());

    // Send result to owner via the connected provider
    if let Some(ref owner_id) = provider.owner_open_id {
        let stdout = task_run.stdout_preview.as_deref().unwrap_or("(no output)");
        let status_icon = if task_run.status == crate::im_gateway::types::TaskRunStatus::Success {
            "✅"
        } else {
            "❌"
        };
        let msg = format!(
            "{} Schedule '{}' executed\nStatus: {:?}\nDuration: {}ms\nOutput:\n{}",
            status_icon,
            schedule.id,
            task_run.status,
            task_run.duration_ms.unwrap_or(0),
            stdout
        );
        // Build an ephemeral target using owner's open_id
        let owner_target = crate::im_gateway::types::ImTarget {
            id: "__owner__".to_string(),
            provider_id: provider.id.clone(),
            display_name: "Owner".to_string(),
            enabled: true,
            receive_id_type: "open_id".to_string(),
            receive_id: owner_id.clone(),
            default_msg_type: "text".to_string(),
            created_at: 0,
            updated_at: 0,
        };
        let feishu = service.connection_manager.feishu_provider();
        let content = serde_json::json!({"text": msg});
        let content_str = serde_json::to_string(&content).unwrap_or_default();
        let send_result = feishu
            .send_text(&provider, &owner_target, &content_str)
            .await;

        // Record outbound message log for the schedule-triggered send
        let (status, message_id, error_msg) = match &send_result {
            Ok(r) => (MessageStatus::Success, r.message_id.clone(), None),
            Err(e) => (MessageStatus::Failed, None, Some(e.to_string())),
        };
        let log = ImMessageLog {
            id: uuid_short(),
            provider_id: provider.id.clone(),
            direction: MessageDirection::Outbound,
            status,
            timestamp: now_ms(),
            target_id: Some("__owner__".to_string()),
            target_name: Some("Owner".to_string()),
            message_id,
            msg_type: Some("text".to_string()),
            content_preview: Some(truncate_str(&msg, 200)),
            trigger: Some(format!("schedule:{}", schedule.id)),
            error: error_msg,
            sender_open_id: None,
            event_id: None,
            reaction_added: None,
        };
        if let Err(e) = service.message_log_store.add(log) {
            error!(error = %e, "failed to store schedule outbound message log");
        }
    }

    json_response(&serde_json::json!({
        "success": true,
        "run_id": run_id,
        "schedule_id": schedule.id,
        "status": format!("{:?}", task_run.status),
        "duration_ms": task_run.duration_ms,
        "exit_code": task_run.exit_code,
    }))
}

fn handle_schedule_runs(
    req: &Request<Incoming>,
    service: &ImGatewayService,
    id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }
    let runs = service.run_store.list_by_schedule(id);
    json_response(&runs)
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

async fn handle_agent(
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
                json_response(&config)
            }
            Method::PATCH => {
                let patch: serde_json::Value = match read_body_json(req).await {
                    Ok(v) => v,
                    Err(resp) => return resp,
                };
                let mut config = service.agent_config_store.load();
                apply_agent_config_patch(&mut config, &patch);
                match service.agent_config_store.save(&config) {
                    Ok(()) => json_response(&config),
                    Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
                }
            }
            _ => method_not_allowed(),
        };
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
            config.instructions.as_deref(),
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
            session_key: Option<String>,
            #[serde(default)]
            system_prompt: Option<String>,
            #[serde(default)]
            work_dir: Option<String>,
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

        // ── Session-free command fast path ──────────────────────────────
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
                    "response": "⏳ Agent 正在处理中，请稍后再试。\n\n提示: /help、/remember、/memories、/forget 等命令即使在处理中也可立即响应。",
                    "tool_calls": [],
                    "plan_steps": null
                }));
            }
        };
        session.source = "api".to_string();
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
        let result = crate::im_gateway::run_turn_with_mcp(
            &service.agent_client,
            &config,
            &mut session,
            &service.agent_tools,
            mcp_opt,
            &body.message,
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
            Ok(turn_result) => json_response(&serde_json::json!({
                "success": true,
                "response": turn_result.response,
                "tool_calls": turn_result.tool_calls_log,
                "plan_steps": turn_result.plan_steps
            })),
            Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
        }
    } else {
        error_response(StatusCode::NOT_FOUND, "Agent endpoint not found")
    }
}

fn apply_agent_config_patch(
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
    if let Some(prompt) = patch
        .get("instructions")
        .or_else(|| patch.get("default_system_prompt"))
        .and_then(|v| v.as_str())
    {
        config.instructions = Some(prompt.to_string());
    }
    if let Some(max_hist) = patch.get("max_history_messages").and_then(|v| v.as_u64()) {
        config.max_history_messages = Some(u32::try_from(max_hist).unwrap_or(u32::MAX));
    }
    if let Some(ttl) = patch.get("session_ttl_secs").and_then(|v| v.as_u64()) {
        config.session_ttl_secs = Some(ttl);
    }
    if let Some(timeout) = patch.get("request_timeout_secs").and_then(|v| v.as_u64()) {
        config.request_timeout_secs = Some(timeout);
    }
    if let Some(shell_timeout) = patch.get("shell_timeout_secs").and_then(|v| v.as_u64()) {
        config.shell_timeout_secs = Some(shell_timeout);
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
        } else {
            if let Some(ref mut headers) = provider.http_headers {
                headers.remove("api-key");
            }
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

fn uses_api_key_header(provider_id: &str, patch: &serde_json::Value) -> bool {
    patch
        .get("by_azure")
        .and_then(|v| v.as_bool())
        .unwrap_or(matches!(provider_id, "aidp_crawl" | "azure"))
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

fn handle_history(
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

async fn read_body_json<T: serde::de::DeserializeOwned>(
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
fn extract_segment_before<'a>(path: &'a str, suffix: &str) -> Option<&'a str> {
    let without_trailing = path.trim_end_matches('/');
    if let Some(id) = without_trailing.strip_suffix(suffix) {
        if !id.is_empty() && !id.contains('/') {
            return Some(id);
        }
    }
    None
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

fn uuid_short() -> String {
    let id = uuid::Uuid::new_v4();
    id.to_string()[..8].to_string()
}

/// Build a Feishu Card 2.0 JSON for real-time plan progress display.
///
/// Used by the plan listener task: first call creates a new card via send_card,
/// subsequent calls update the same card via patch_card.
fn build_plan_card(
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
fn build_im_status_text(detail: Option<&SessionDetail>) -> String {
    match detail {
        Some(d) => {
            let real = d
                .total_tokens_used
                .map(|t| t.to_string())
                .unwrap_or_else(|| "N/A".to_string());
            format!(
                "会话状态:\n- 消息数: {}\n- 估算 token: ~{}\n- API 累计 token: {}\n- 压缩次数: {}\n- 历史版本: {}\n- 状态: 空闲",
                d.message_count, d.estimated_tokens, real, d.compaction_count, d.history_version
            )
        }
        None => {
            "会话状态:\n- 消息数: 0\n- 状态: 新会话\n\n提示: 发送消息即可开始对话。".to_string()
        }
    }
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{}...", truncated)
    }
}

/// Parse URL query string into key-value pairs.
fn parse_query_params(query: &str) -> std::collections::HashMap<String, String> {
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

fn build_content_preview(msg_type: &str, content: &serde_json::Value) -> Option<String> {
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
        _ => Some(format!("[{}]", msg_type)),
    }
}

/// Sanitize provider config for API response: never expose secret_ref in plaintext.
fn sanitize_provider(provider: &ImProviderConfig) -> serde_json::Value {
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
        "created_at": provider.created_at,
        "updated_at": provider.updated_at,
    })
}

fn apply_provider_patch(provider: &mut ImProviderConfig, patch: &serde_json::Value) {
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
    provider.updated_at = now_ms();
}

fn apply_target_patch(target: &mut ImTarget, patch: &serde_json::Value) {
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

fn apply_route_patch(route: &mut ImRoute, patch: &serde_json::Value) {
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

fn apply_schedule_patch(schedule: &mut ImSchedule, patch: &serde_json::Value) {
    if let Some(name) = patch.get("name").and_then(|v| v.as_str()) {
        schedule.name = name.to_string();
    }
    if let Some(enabled) = patch.get("enabled").and_then(|v| v.as_bool()) {
        schedule.enabled = enabled;
    }
    if let Some(target_id) = patch.get("target_id").and_then(|v| v.as_str()) {
        schedule.target_id = target_id.to_string();
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
    if let Some(script) = patch.get("script") {
        if let Ok(s) = serde_json::from_value(script.clone()) {
            schedule.script = s;
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
fn parse_session_filename(filename: &str) -> (String, u64) {
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
