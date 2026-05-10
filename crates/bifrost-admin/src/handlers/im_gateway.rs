use std::collections::{HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Instant;

use base64::Engine as _;
use bifrost_core::text::truncate_bytes_with_suffix;
use futures_util::FutureExt;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::handlers::{error_response, json_response, method_not_allowed, BoxBody};
use crate::im_gateway::event_router::ImEventRouter;
use crate::im_gateway::progress_card::ImAgentProgressRegistry;
use crate::im_gateway::provider::ImProvider;
use crate::im_gateway::types::{
    ImEvent, ImImageAttachment, ImMessageLog, ImProviderAgentConfig, ImProviderConfig, ImRoute,
    ImRouteAction, ImSchedule, ImTarget, MessageDirection, MessageStatus,
};
use crate::im_gateway::{
    ImAgentClient, ImAgentConfigStore, ImAgentSessionManager, ImAgentToolRegistry,
    ImConnectionManager, ImEventStore, ImMcpManager, ImMessageLogStore, ImProviderStore,
    ImRouteStore, ImRunStore, ImScheduleStore, ImTargetStore, SessionQueueManager,
};
use bifrost_agent::persistence::ConversationRecorder;
use bifrost_agent::{PlanStep, SessionDetail, ToolCallLog};

const IMAGE_ONLY_AGENT_PROMPT: &str = "请理解这张图片，并根据图片内容回答。";
const MAX_AGENT_IMAGES_PER_MESSAGE: usize = 6;

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
    pub progress_registry: Arc<ImAgentProgressRegistry>,
}

impl ImGatewayService {
    pub fn new(data_dir: &std::path::Path) -> Self {
        Self::new_with_agent_proxy_port(data_dir, None)
    }

    pub fn new_with_agent_proxy_port(data_dir: &std::path::Path, proxy_port: Option<u16>) -> Self {
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
        let ca_cert_path = data_dir.join("certs").join("ca.crt");
        let agent_client = proxy_port
            .map(|port| ImAgentClient::new_with_bifrost_proxy_and_ca(port, Some(&ca_cert_path)))
            .unwrap_or_default();
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
            agent_client: Arc::new(agent_client),
            agent_tools,
            agent_session_manager: Arc::new(ImAgentSessionManager::new(
                agent_config.get_session_ttl_secs(),
            )),
            queue_manager: Arc::new(SessionQueueManager::new()),
            progress_registry: Arc::new(ImAgentProgressRegistry::new()),
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
            let provider_store = self.provider_store.clone();
            let agent_config_store = self.agent_config_store.clone();
            let agent_client = self.agent_client.clone();
            let agent_tools = self.agent_tools.clone();
            let agent_session_manager = self.agent_session_manager.clone();
            let queue_manager = self.queue_manager.clone();
            let progress_registry = self.progress_registry.clone();
            tokio::spawn(async move {
                run_event_loop(
                    rx,
                    feishu,
                    provider_for_loop,
                    event_store,
                    message_log_store,
                    route_store,
                    provider_store,
                    agent_config_store,
                    agent_client,
                    agent_tools,
                    agent_session_manager,
                    queue_manager,
                    progress_registry,
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
                            let provider_store = self.provider_store.clone();
                            let agent_config_store = self.agent_config_store.clone();
                            let agent_client = self.agent_client.clone();
                            let agent_tools = self.agent_tools.clone();
                            let agent_session_manager = self.agent_session_manager.clone();
                            let queue_manager = self.queue_manager.clone();
                            let progress_registry = self.progress_registry.clone();
                            tokio::spawn(async move {
                                run_event_loop(
                                    rx,
                                    feishu,
                                    provider_for_loop,
                                    event_store,
                                    message_log_store,
                                    route_store,
                                    provider_store,
                                    agent_config_store,
                                    agent_client,
                                    agent_tools,
                                    agent_session_manager,
                                    queue_manager,
                                    progress_registry,
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
                let payload: serde_json::Value = match read_body_json(req).await {
                    Ok(v) => v,
                    Err(resp) => return resp,
                };
                let mut config = match parse_provider_create_payload(payload) {
                    Ok(v) => v,
                    Err(e) => {
                        return error_response(
                            StatusCode::BAD_REQUEST,
                            &format!("Invalid request body: {e}"),
                        );
                    }
                };
                let now = now_ms();
                if config.created_at == 0 {
                    config.created_at = now;
                }
                config.updated_at = now;
                normalize_provider_agent_config(&mut config);
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
    let provider_store = service.provider_store.clone();
    let agent_config_store = service.agent_config_store.clone();
    let agent_client = service.agent_client.clone();
    let agent_tools = service.agent_tools.clone();
    let agent_session_manager = service.agent_session_manager.clone();
    let queue_manager = service.queue_manager.clone();
    let progress_registry = service.progress_registry.clone();
    tokio::spawn(async move {
        run_event_loop(
            rx,
            feishu,
            provider_for_loop,
            event_store,
            message_log_store,
            route_store,
            provider_store,
            agent_config_store,
            agent_client,
            agent_tools,
            agent_session_manager,
            queue_manager,
            progress_registry,
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

fn effective_agent_config_for_provider(
    base: &crate::im_gateway::agent::ImAgentConfig,
    provider: &ImProviderConfig,
) -> crate::im_gateway::agent::ImAgentConfig {
    let mut config = base.clone();
    if let Some(agent_config) = provider.agent_config.as_ref() {
        if let Some(work_dir) = agent_config
            .work_dir
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            config.work_dir = Some(work_dir.to_string());
        }
        if let Some(instructions) = agent_config
            .base_instructions
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            config.base_instructions = Some(instructions.to_string());
        }
        if let Some(instructions) = agent_config
            .developer_instructions
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            config.developer_instructions = Some(instructions.to_string());
        }
        if let Some(instructions) = agent_config
            .user_instructions
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            config.user_instructions = Some(instructions.to_string());
        }
    }
    config
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
    provider_store: Arc<ImProviderStore>,
    agent_config_store: Arc<ImAgentConfigStore>,
    agent_client: Arc<ImAgentClient>,
    agent_tools: Arc<ImAgentToolRegistry>,
    agent_session_manager: Arc<ImAgentSessionManager>,
    queue_manager: Arc<SessionQueueManager>,
    progress_registry: Arc<ImAgentProgressRegistry>,
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

        let online_msg = build_online_notification_message(&provider);
        let send_result = feishu
            .send_text(&provider, &online_target, &online_msg)
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
        let provider = provider_store
            .get(&event.provider_id)
            .unwrap_or_else(|| provider.clone());

        if !provider.enabled {
            info!(
                provider_id = %event.provider_id,
                event_id = %event.event_id,
                "dropping inbound event because provider is disabled"
            );
            continue;
        }

        // Deduplication: per Feishu docs, use message_id for idempotency
        // ("如有幂等需求请使用 message_id 去重，不要依赖 event_id").
        // Falls back to event_id for non-message events.
        let dedup_key = event
            .source
            .message_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .unwrap_or(&event.event_id);

        if !dedup_key.is_empty() && dedup.is_duplicate(dedup_key) {
            debug!(
                provider_id = %event.provider_id,
                event_id = %event.event_id,
                message_id = ?event.source.message_id,
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
                    content_preview: event.message.as_ref().map(inbound_message_preview),
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
            message_type = ?event.message.as_ref().and_then(|m| m.raw_type.as_deref()),
            message_text_len = event.message.as_ref().map(|m| m.text.len()).unwrap_or(0),
            image_count = event.message.as_ref().map(|m| m.images.len()).unwrap_or(0),
            has_text = event.message.as_ref().is_some_and(|m| !m.text.trim().is_empty()),
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
            content_preview: event.message.as_ref().map(inbound_message_preview),
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
                    if !msg.text.trim().is_empty() || !msg.images.is_empty() {
                        let session_key =
                            build_session_key(&event.provider_id, event.source.user_id.as_deref());
                        let agent_message = agent_message_text(msg);

                        // ── Guide/Queue mode: check if session is busy ──
                        if agent_session_manager.is_session_active(&session_key) {
                            handle_busy_message(
                                &agent_message,
                                &session_key,
                                BusyMessageContext {
                                    queue_manager: &queue_manager,
                                    feishu: &feishu,
                                    provider: &provider,
                                    event: &event,
                                    message_log_store: &message_log_store,
                                    agent_session_manager: &agent_session_manager,
                                    progress_registry: &progress_registry,
                                },
                            )
                            .await;
                            continue;
                        }

                        // /status — handle directly without entering agent pipeline
                        if agent_message.trim() == "/status" {
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
                        let images =
                            resolve_event_images(&feishu, &provider, &event, &msg.images).await;
                        run_agent_chat_with_interleave(
                            &mut rx,
                            &feishu,
                            &provider,
                            &provider_store,
                            &event,
                            &agent_client,
                            &agent_config_store,
                            &agent_tools,
                            &agent_session_manager,
                            &queue_manager,
                            &progress_registry,
                            &session_key,
                            &agent_message,
                            images,
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
                let raw_message_text = route_match.message_text.as_deref().unwrap_or("");
                let has_images = event
                    .message
                    .as_ref()
                    .is_some_and(|message| !message.images.is_empty());
                if raw_message_text.trim().is_empty() && !has_images {
                    continue;
                }
                let message_text = event
                    .message
                    .as_ref()
                    .map(agent_message_text)
                    .unwrap_or_else(|| raw_message_text.to_string());
                let session_key =
                    build_session_key(&event.provider_id, event.source.user_id.as_deref());

                // ── Guide/Queue mode: check if session is busy ──
                if agent_session_manager.is_session_active(&session_key) {
                    handle_busy_message(
                        &message_text,
                        &session_key,
                        BusyMessageContext {
                            queue_manager: &queue_manager,
                            feishu: &feishu,
                            provider: &provider,
                            event: &event,
                            message_log_store: &message_log_store,
                            agent_session_manager: &agent_session_manager,
                            progress_registry: &progress_registry,
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

                let images = match event.message.as_ref() {
                    Some(message) => {
                        resolve_event_images(&feishu, &provider, &event, &message.images).await
                    }
                    None => Vec::new(),
                };
                run_agent_chat_with_interleave(
                    &mut rx,
                    &feishu,
                    &provider,
                    &provider_store,
                    &event,
                    &agent_client,
                    &agent_config_store,
                    &agent_tools,
                    &agent_session_manager,
                    &queue_manager,
                    &progress_registry,
                    &session_key,
                    &message_text,
                    images,
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
/// - `/stop`: request cooperative cancellation of the active turn loop
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
    progress_registry: &'a Arc<ImAgentProgressRegistry>,
}

async fn resolve_event_images(
    feishu: &Arc<crate::im_gateway::feishu::FeishuProvider>,
    provider: &ImProviderConfig,
    event: &ImEvent,
    images: &[ImImageAttachment],
) -> Vec<bifrost_agent::ChatImageInput> {
    let mut resolved = Vec::new();
    if images.len() > MAX_AGENT_IMAGES_PER_MESSAGE {
        warn!(
            provider_id = %provider.id,
            event_id = %event.event_id,
            image_count = images.len(),
            max_images = MAX_AGENT_IMAGES_PER_MESSAGE,
            "too many IM images in one message; truncating images for agent multimodal input"
        );
    }
    for image in images.iter().take(MAX_AGENT_IMAGES_PER_MESSAGE) {
        if let (Some(mime_type), Some(data)) = (&image.mime_type, &image.data_base64) {
            resolved.push(bifrost_agent::ChatImageInput {
                mime_type: mime_type.clone(),
                data: data.clone(),
            });
            continue;
        }

        let Some(message_id) = event.source.message_id.as_deref() else {
            warn!(
                provider_id = %provider.id,
                file_key = %image.file_key,
                "cannot download IM image because message_id is missing"
            );
            continue;
        };
        match feishu
            .download_message_image_resource(provider, message_id, &image.file_key)
            .await
        {
            Ok((mime_type, bytes)) => {
                info!(
                    provider_id = %provider.id,
                    message_id = %message_id,
                    file_key = %image.file_key,
                    mime_type = %mime_type,
                    byte_len = bytes.len(),
                    "downloaded IM image resource for agent multimodal input"
                );
                let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                resolved.push(bifrost_agent::ChatImageInput { mime_type, data });
            }
            Err(error) => {
                warn!(
                    provider_id = %provider.id,
                    message_id = %message_id,
                    file_key = %image.file_key,
                    error = %error,
                    "failed to download IM image resource"
                );
            }
        }
    }
    resolved
}

fn agent_message_text(message: &crate::im_gateway::types::ImEventMessage) -> String {
    let text = message.text.trim();
    if !text.is_empty() {
        text.to_string()
    } else if !message.images.is_empty() {
        IMAGE_ONLY_AGENT_PROMPT.to_string()
    } else {
        String::new()
    }
}

fn inbound_message_preview(message: &crate::im_gateway::types::ImEventMessage) -> String {
    if message.text.trim().is_empty() && !message.images.is_empty() {
        format!("[图片消息: {} 张]", message.images.len())
    } else {
        truncate_str(&message.text, 200)
    }
}

async fn handle_busy_message(msg_text: &str, session_key: &str, ctx: BusyMessageContext<'_>) {
    let queue_manager = ctx.queue_manager;
    let feishu = ctx.feishu;
    let provider = ctx.provider;
    let event = ctx.event;
    let message_log_store = ctx.message_log_store;
    let agent_session_manager = ctx.agent_session_manager;
    let progress_registry = ctx.progress_registry;
    let trimmed = msg_text.trim();

    // /status — show session status or busy indicator
    if trimmed == "/status" {
        // Try to get session detail from idle sessions
        if let Some(detail) = agent_session_manager.get_session_detail(session_key) {
            let reply = build_im_status_text(Some(&detail));
            send_agent_reply(feishu, provider, event, &reply, message_log_store).await;
        } else if let Some(mut status) = agent_session_manager.get_active_turn_status(session_key) {
            status.pending_guide_messages = queue_manager.guide_status(session_key);
            let queue_items = queue_manager.queue_status(session_key);
            let queue_info = if queue_items.is_empty() {
                "无排队消息".to_string()
            } else {
                format!("{} 条排队消息", queue_items.len())
            };
            let reply = format!(
                "{}\n- 排队: {}",
                bifrost_agent::format_active_turn_status_text(&status),
                queue_info
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
            let guide_info = format_pending_guide_status(&queue_manager.guide_status(session_key));
            let reply = format!(
                "会话状态:\n- 状态: 🔵 正在处理中\n- 排队: {}\n{}\n\n请等待当前任务完成后再查询详细状态。",
                queue_info, guide_info
            );
            send_agent_reply(feishu, provider, event, &reply, message_log_store).await;
        }
        return;
    }

    // /stop — cooperative cancellation of the active turn loop
    if trimmed == "/stop" {
        let stopped = agent_session_manager.request_stop(session_key);
        let reply = if stopped {
            "🛑 已请求停止当前 Agent loop。"
        } else {
            "当前没有正在执行的 Agent loop。"
        };
        send_agent_reply(feishu, provider, event, reply, message_log_store).await;
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
                let guide_pending = !queue_manager.guide_status(session_key).is_empty();
                let updated = progress_registry
                    .update_queue_state(
                        session_key,
                        items.clone(),
                        guide_pending,
                        Some(format!("已加入排队：{} 条", items.len())),
                    )
                    .await;
                if !updated {
                    let reply = format_queue_status("✅ 已加入排队", &items);
                    send_agent_reply(feishu, provider, event, &reply, message_log_store).await;
                }
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
                    let guide_pending = !queue_manager.guide_status(session_key).is_empty();
                    let updated = progress_registry
                        .update_queue_state(
                            session_key,
                            items.clone(),
                            guide_pending,
                            Some(format!("已删除排队消息 #{seq}")),
                        )
                        .await;
                    if !updated {
                        let reply = format_queue_status(&format!("🗑️ 已删除 #{seq}"), &items);
                        send_agent_reply(feishu, provider, event, &reply, message_log_store).await;
                    }
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
             - /stop — 立即停止当前 loop\n\
             - /help — 查看帮助",
            trimmed.split_whitespace().next().unwrap_or(trimmed)
        );
        send_agent_reply(feishu, provider, event, &reply, message_log_store).await;
        return;
    }

    // Default: guide mode — inject into the guide channel
    let pending_guide_count = queue_manager.inject_guide(session_key, trimmed.to_string());
    let reply = if pending_guide_count > 1 {
        format!(
            "🔀 已追加引导消息（当前 {} 条尚未进入 loop，将合并后生效）",
            pending_guide_count
        )
    } else {
        "🔀 已注入引导消息，将在当前工具调用完成后生效".to_string()
    };
    info!(
        session_key = %session_key,
        guide_msg_len = trimmed.len(),
        pending_guide_count = pending_guide_count,
        "guide message injected via IM"
    );
    let updated = progress_registry
        .update_queue_state(
            session_key,
            queue_manager.queue_status(session_key),
            true,
            Some(format!("已收到引导：{}", truncate_str(trimmed, 48))),
        )
        .await;
    if !updated {
        send_agent_reply(feishu, provider, event, &reply, message_log_store).await;
    }
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

fn format_pending_guide_status(guides: &[String]) -> String {
    if guides.is_empty() {
        return "- 引导消息: 无".to_string();
    }

    let mut text = format!("- 引导消息: {} 条尚未进入 loop", guides.len());
    for (idx, guide) in guides.iter().enumerate() {
        text.push_str(&format!("\n  {}. {}", idx + 1, truncate_str(guide, 80)));
    }
    text
}

async fn run_progress_event_coalescer(
    progress_registry: Arc<ImAgentProgressRegistry>,
    session_key: String,
    rx: &mut mpsc::UnboundedReceiver<bifrost_agent::AgentTurnProgressEvent>,
) {
    const STATUS_COALESCE_MS: u64 = 300;
    while let Some(first) = rx.recv().await {
        let mut immediate = progress_event_needs_immediate_flush(&first);
        let mut events = vec![first];
        while let Ok(event) = rx.try_recv() {
            immediate |= progress_event_needs_immediate_flush(&event);
            events.push(event);
        }
        if !immediate {
            let deadline = tokio::time::sleep(std::time::Duration::from_millis(STATUS_COALESCE_MS));
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    _ = &mut deadline => break,
                    maybe_event = rx.recv() => {
                        let Some(event) = maybe_event else {
                            break;
                        };
                        let mut batch_is_immediate = progress_event_needs_immediate_flush(&event);
                        events.push(event);
                        while let Ok(event) = rx.try_recv() {
                            let drained_is_immediate = progress_event_needs_immediate_flush(&event);
                            events.push(event);
                            if drained_is_immediate {
                                batch_is_immediate = true;
                                break;
                            }
                        }
                        if batch_is_immediate {
                            break;
                        }
                    }
                }
            }
        }
        progress_registry.apply_events(&session_key, events).await;
    }
}

fn progress_event_needs_immediate_flush(event: &bifrost_agent::AgentTurnProgressEvent) -> bool {
    matches!(
        event,
        bifrost_agent::AgentTurnProgressEvent::ToolStarted { .. }
            | bifrost_agent::AgentTurnProgressEvent::ToolFinished { .. }
            | bifrost_agent::AgentTurnProgressEvent::PlanUpdated { .. }
            | bifrost_agent::AgentTurnProgressEvent::TitleUpdated { .. }
            | bifrost_agent::AgentTurnProgressEvent::AssistantDelta { .. }
            | bifrost_agent::AgentTurnProgressEvent::AssistantFinal { .. }
            | bifrost_agent::AgentTurnProgressEvent::TurnFinished { .. }
            | bifrost_agent::AgentTurnProgressEvent::TurnFailed { .. }
    )
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
    provider_store: &Arc<ImProviderStore>,
    initial_event: &ImEvent,
    agent_client: &Arc<ImAgentClient>,
    agent_config_store: &Arc<ImAgentConfigStore>,
    agent_tools: &Arc<ImAgentToolRegistry>,
    agent_session_manager: &Arc<ImAgentSessionManager>,
    queue_manager: &Arc<SessionQueueManager>,
    progress_registry: &Arc<ImAgentProgressRegistry>,
    session_key: &str,
    initial_message: &str,
    initial_images: Vec<bifrost_agent::ChatImageInput>,
    system_prompt_override: Option<&str>,
    mcp_manager: &mut ImMcpManager,
    message_log_store: &Arc<ImMessageLogStore>,
) {
    // Set up the guide channel before starting the turn
    let guide_channel = queue_manager.get_or_create_guide_channel(session_key);

    let mut current_msg = initial_message.to_string();
    let mut current_images = initial_images;

    // Queue drain loop: process initial message, then drain queued messages
    loop {
        let current_provider = provider_store
            .get(&provider.id)
            .unwrap_or_else(|| provider.clone());
        let agent_config =
            effective_agent_config_for_provider(&agent_config_store.load(), &current_provider);
        // Clone into a local so the future borrows the local, not `current_msg`.
        let msg_for_turn = current_msg.clone();
        let images_for_turn = current_images.clone();
        current_images.clear();

        // Run agent chat with interleaved event processing
        let chat_future = AssertUnwindSafe(process_agent_chat(
            feishu,
            &current_provider,
            provider_store,
            initial_event,
            agent_client,
            &agent_config,
            agent_tools,
            agent_session_manager,
            progress_registry,
            session_key,
            &msg_for_turn,
            &images_for_turn,
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
                        &current_provider,
                        session_key,
                        queue_manager,
                        feishu,
                        message_log_store,
                        agent_session_manager,
                        progress_registry,
                        agent_config_store,
                        provider_store,
                    )
                    .await;
                }
            }
        }

        // After turn completes, drain any unconsumed guide message into the queue
        // so it's not lost when clear_session removes the guide slot.
        // This handles the race where a guide message arrives after the session's
        // turn-end checkpoint but before the select! loop breaks.
        let unconsumed_guides: Vec<String> = guide_channel.lock().unwrap().drain(..).collect();
        if let Some(unconsumed) = bifrost_agent::session::combine_guide_messages(unconsumed_guides)
        {
            if !unconsumed.trim().is_empty() {
                info!(
                    session_key = %session_key,
                    guide_msg_len = unconsumed.len(),
                    "draining unconsumed guide message into queue after turn"
                );
                let _ = queue_manager.push_queue(session_key, unconsumed);
            }
        }

        // After turn completes, check for queued messages.
        // Pop the next message from queue and process it as a new turn.
        // The session layer also supports `pending_messages` for inline queue
        // drain (used by `/agent/chat` API for testing), but in the IM event loop
        // we process one message per outer iteration to maintain interleave support.
        match queue_manager.pop_queue(session_key) {
            Some(next_msg) => {
                let remaining = queue_manager.queue_status(session_key).len();
                info!(
                    session_key = %session_key,
                    queued_msg_len = next_msg.len(),
                    remaining_queue = remaining,
                    "processing next queued message"
                );
                current_msg = next_msg;
            }
            None => {
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
    progress_registry: &Arc<ImAgentProgressRegistry>,
    agent_config_store: &Arc<ImAgentConfigStore>,
    provider_store: &Arc<ImProviderStore>,
) {
    let provider = provider_store
        .get(&event.provider_id)
        .unwrap_or_else(|| provider.clone());

    if !provider.enabled {
        debug!(
            event_id = %event.event_id,
            provider_id = %event.provider_id,
            "dropping concurrent event because provider is disabled"
        );
        return;
    }

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
        let agent_config =
            effective_agent_config_for_provider(&agent_config_store.load(), &provider);
        if let Some(response) =
            bifrost_agent::handle_session_free_command(&session_key, msg_text, &agent_config)
        {
            send_agent_reply(feishu, &provider, event, &response, message_log_store).await;
            return;
        }
        // Route through guide/queue mode
        handle_busy_message(
            msg_text,
            &session_key,
            BusyMessageContext {
                queue_manager,
                feishu,
                provider: &provider,
                event,
                message_log_store,
                agent_session_manager,
                progress_registry,
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
                    provider: &provider,
                    event,
                    message_log_store,
                    agent_session_manager,
                    progress_registry,
                },
            )
            .await;
        } else {
            // Session is free but we can't process it now (MCP is in use).
            // Queue it for later processing.
            let _ = queue_manager.push_queue(&session_key, msg_text.to_string());
            send_agent_reply(
                feishu,
                &provider,
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
    provider_store: &Arc<ImProviderStore>,
    event: &ImEvent,
    agent_client: &Arc<ImAgentClient>,
    agent_config: &crate::im_gateway::agent::ImAgentConfig,
    agent_tools: &Arc<ImAgentToolRegistry>,
    session_manager: &Arc<ImAgentSessionManager>,
    progress_registry: &Arc<ImAgentProgressRegistry>,
    session_key: &str,
    user_message: &str,
    images: &[bifrost_agent::ChatImageInput],
    system_prompt_override: Option<&str>,
    mcp: Option<&mut ImMcpManager>,
    message_log_store: &Arc<ImMessageLogStore>,
    guide_channel: Option<bifrost_agent::session::GuideChannel>,
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
    let mut session = match session_manager
        .try_take_session_with_work_dir(session_key, agent_config.work_dir.clone())
    {
        Some(s) => s,
        None => {
            info!(
                session_key = %session_key,
                "session is busy, rejecting concurrent request"
            );
            let busy_msg =
                "⏳ Agent 正在处理中，请稍后再试。\n\n提示: /stop 可立即停止当前 loop；/help、/remember、/memories、/forget 等命令即使在处理中也可立即响应。";
            send_agent_reply(feishu, provider, event, busy_msg, message_log_store).await;
            return;
        }
    };
    if session.history.is_empty() {
        if let Some(work_dir) = agent_config
            .work_dir
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if session.work_dir.as_deref() != Some(work_dir) {
                session.reinitialize_work_dir(work_dir.to_string());
            }
        } else if session.work_dir.is_none() {
            if let Some(work_dir) = agent_config.work_dir.clone() {
                session.reinitialize_work_dir(work_dir);
            }
        }
    } else if session.work_dir.is_none() {
        if let Some(work_dir) = agent_config.work_dir.clone() {
            session.reinitialize_work_dir(work_dir);
        }
    }
    session.source = "feishu".to_string();
    session.guide_channel = guide_channel;

    let target_open_id = provider
        .owner_open_id
        .as_deref()
        .or(event.source.user_id.as_deref())
        .unwrap_or("");
    let mut progress_enabled = false;
    let mut progress_tx_for_finish = None;
    let mut progress_task = None;
    if !target_open_id.is_empty() {
        let progress_target = crate::im_gateway::types::ImTarget {
            id: "__agent_progress__".to_string(),
            provider_id: provider.id.clone(),
            display_name: "Agent Progress".to_string(),
            enabled: true,
            receive_id_type: "open_id".to_string(),
            receive_id: target_open_id.to_string(),
            default_msg_type: "interactive".to_string(),
            created_at: 0,
            updated_at: 0,
        };
        match progress_registry
            .start_feishu(
                session_key,
                feishu.clone(),
                provider.clone(),
                progress_target,
                user_message,
            )
            .await
        {
            Ok(_) => {
                let (progress_tx, mut progress_rx) =
                    tokio::sync::mpsc::unbounded_channel::<bifrost_agent::AgentTurnProgressEvent>();
                session.progress_sender = Some(progress_tx.clone());
                progress_tx_for_finish = Some(progress_tx);
                let progress_registry = Arc::clone(progress_registry);
                let session_key_for_progress = session_key.to_string();
                progress_task = Some(tokio::spawn(async move {
                    run_progress_event_coalescer(
                        progress_registry,
                        session_key_for_progress,
                        &mut progress_rx,
                    )
                    .await;
                }));
                progress_enabled = true;
            }
            Err(error) => {
                warn!(
                    session_key = %session_key,
                    error = %error,
                    "failed to start IM streaming progress card; falling back to final reply card"
                );
            }
        }
    }

    // Set up plan update channel for real-time plan card rendering.
    // The turn loop pushes plan steps through this channel; a background task
    // sends (first time) or patches (subsequent) a single Feishu card.
    if !progress_enabled {
        let (plan_tx, mut plan_rx) =
            tokio::sync::mpsc::unbounded_channel::<(Vec<bifrost_agent::PlanStep>, Option<String>)>(
            );
        session.plan_sender = Some(plan_tx);
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
                        "base_instructions": bifrost_agent::prompt::resolve_base_instructions_text(agent_config, None),
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

    let result = bifrost_agent::session::run_turn_with_mcp_multimodal(
        agent_client,
        agent_config,
        &mut session,
        agent_tools,
        mcp,
        user_message,
        images,
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
            match bifrost_agent::session::run_turn_with_mcp_multimodal(
                agent_client,
                agent_config,
                &mut session,
                agent_tools,
                None, // MCP already consumed in first attempt
                user_message,
                images,
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

    // ── Goal-based auto-continuation loop ────────────────────────────────
    // If the turn completed successfully and the goal is still active,
    // send the intermediate response and automatically trigger another turn
    // with the continuation prompt. This prevents the session from appearing
    // "idle" while the agent has unfinished work.
    const MAX_GOAL_CONTINUATIONS: usize = 25;
    let mut result = result;
    let mut continuation_count = 0;

    while let Ok(ref turn_result) = result {
        if !turn_result.goal_needs_continuation || continuation_count >= MAX_GOAL_CONTINUATIONS {
            break;
        }
        continuation_count += 1;

        info!(
            session_key = %session_key,
            continuation = continuation_count,
            goal_objective = ?turn_result.goal_objective,
            "goal still active, auto-continuing"
        );

        // Send intermediate response when no streaming progress card is active.
        if !progress_enabled && !turn_result.response.is_empty() {
            send_agent_reply_with_plan(
                feishu,
                provider,
                event,
                &turn_result.response,
                turn_result.plan_steps.as_deref(),
                &turn_result.tool_calls_log,
                session.title.as_deref(),
                message_log_store,
            )
            .await;
        }

        // Get continuation prompt from goal system
        let continuation_msg = match bifrost_agent::tools::goal::get_continuation_prompt(&session) {
            Some(prompt) => prompt,
            None => break, // Goal no longer active after sending response
        };

        // Run another turn with the continuation prompt
        let cont_result = crate::im_gateway::run_turn_with_mcp(
            agent_client,
            agent_config,
            &mut session,
            agent_tools,
            None, // MCP already consumed
            &continuation_msg,
            system_prompt_override,
            recorder.as_mut(),
        )
        .await;

        match cont_result {
            Ok(r) => result = Ok(r),
            Err(e) => {
                warn!(
                    session_key = %session_key,
                    continuation = continuation_count,
                    error = %e,
                    "goal continuation turn failed, stopping"
                );
                break;
            }
        }
    }

    if continuation_count > 0 {
        info!(
            session_key = %session_key,
            total_continuations = continuation_count,
            "goal continuation loop completed"
        );
    }

    // Put the recorder back into the session so it persists across turns.
    // Skip this if session was cleared during the turn (/clear drops the recorder
    // deliberately so a new file will be created for the fresh session).
    if recorder.is_some() && !session.memory_cleared {
        session.recorder = recorder;
    }

    // Extract session title before returning the session
    let session_title = session.title.clone();
    session.progress_sender = None;
    session.plan_sender = None;

    // Return session after turn completes
    session_manager.return_session(session);

    // Best-effort cleanup
    session_manager.cleanup_expired();

    // Separate main response and tool calls for card rendering
    let mut progress_failed = false;
    let (main_response, tool_calls_panel, plan_steps) = match result {
        Ok(turn_result) => {
            let mut response = turn_result.response;
            // Log work_dir switch if it happened
            if let Some(ref new_dir) = turn_result.work_dir_switched {
                info!(
                    session_key = %session_key,
                    new_work_dir = %new_dir,
                    "session work directory switched via agent tool"
                );
                persist_provider_agent_work_dir(provider_store, &provider.id, new_dir);
                response.push_str(&format!("\n\n当前工作路径: `{new_dir}`"));
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
            (response, panel, plan)
        }
        Err(e) => {
            progress_failed = true;
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

    if progress_enabled {
        if let Some(tx) = progress_tx_for_finish.take() {
            let event = if progress_failed {
                bifrost_agent::AgentTurnProgressEvent::TurnFailed {
                    error: main_response.clone(),
                }
            } else {
                bifrost_agent::AgentTurnProgressEvent::TurnFinished {
                    content: main_response.clone(),
                }
            };
            let _ = tx.send(event);
            drop(tx);
        }
        if let Some(task) = progress_task.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
        }
        let progress_message_info = progress_registry
            .finish(session_key, Some(main_response.clone()), progress_failed)
            .await;

        let log = ImMessageLog {
            id: uuid_short(),
            provider_id: provider.id.clone(),
            direction: MessageDirection::Outbound,
            status: MessageStatus::Success,
            timestamp: now_ms(),
            target_id: Some(
                progress_message_info
                    .as_ref()
                    .map(|info| format!("__agent_progress__:{}", info.card_id))
                    .unwrap_or_else(|| "__agent_progress__".to_string()),
            ),
            target_name: Some("Agent Progress".to_string()),
            message_id: progress_message_info.and_then(|info| info.message_id),
            msg_type: Some("interactive".to_string()),
            content_preview: Some(truncate_str(&main_response, 200)),
            trigger: Some("agent_streaming".to_string()),
            error: None,
            sender_open_id: None,
            event_id: Some(event.event_id.clone()),
            reaction_added: None,
        };
        if let Err(e) = message_log_store.add(log) {
            error!(error = %e, "failed to store agent streaming outbound message log");
        }
        return;
    }

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

/// Send an agent reply with plan progress and tool calls panel (for goal continuation).
/// This sends a card similar to the final response rendering but can be called mid-continuation.
#[allow(clippy::too_many_arguments)]
async fn send_agent_reply_with_plan(
    feishu: &Arc<crate::im_gateway::feishu::FeishuProvider>,
    provider: &ImProviderConfig,
    event: &ImEvent,
    reply_text: &str,
    plan_steps: Option<&[PlanStep]>,
    tool_calls_log: &[ToolCallLog],
    title: Option<&str>,
    message_log_store: &Arc<ImMessageLogStore>,
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

    let converted_text =
        crate::im_gateway::markdown_converter::convert_to_feishu_markdown(reply_text);
    let mut elements = vec![serde_json::json!({
        "tag": "markdown",
        "content": converted_text,
        "element_id": "agent_reply"
    })];

    // Add plan progress panel if present
    if let Some(steps) = plan_steps {
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

    // Add tool calls panel if present
    if !tool_calls_log.is_empty() {
        let mut tool_md = String::new();
        for log in tool_calls_log {
            let icon = if log.success { "✅" } else { "❌" };
            tool_md.push_str(&format!("{} `{}`\n", icon, log.tool_name));
            let result_preview = truncate_str(&log.result, 500);
            tool_md.push_str(&format!("```\n{}\n```\n", result_preview));
        }
        elements.push(serde_json::json!({
            "tag": "collapsible_panel",
            "expanded": false,
            "background_color": "grey",
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": format!("🔧 工具调用记录（{}次）", tool_calls_log.len())
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

    let card_title = title.unwrap_or("Bifrost AI");
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
        trigger: Some("agent_continuation".to_string()),
        error: error_msg,
        sender_open_id: None,
        event_id: Some(event.event_id.clone()),
        reaction_added: None,
    };
    if let Err(e) = message_log_store.add(log) {
        error!(error = %e, "failed to store agent outbound message log");
    }

    match send_result {
        Ok(_) => debug!("agent reply with plan sent successfully"),
        Err(e) => error!(error = %e, "failed to send agent reply with plan"),
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
    #[serde(default)]
    provider_id: Option<String>,
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default = "default_msg_type")]
    msg_type: String,
    #[serde(default)]
    content: serde_json::Value,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    card: Option<serde_json::Value>,
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

    let resolved = match resolve_send_message_request(service, &body) {
        Ok(v) => v,
        Err((status, message)) => return error_response(status, &message),
    };

    if !resolved.provider.enabled {
        return error_response(StatusCode::BAD_REQUEST, "Provider is disabled");
    }

    if !resolved.target.enabled {
        return error_response(StatusCode::BAD_REQUEST, "Target is disabled");
    }

    // Build content preview
    let content_preview = build_content_preview(&body.msg_type, &resolved.content);

    // Send via connection manager's feishu provider
    let feishu = service.connection_manager.feishu_provider();
    let result = if body.msg_type == "text" {
        let text = resolved
            .content
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string(&resolved.content).unwrap_or_default());
        feishu
            .send_text(&resolved.provider, &resolved.target, &text)
            .await
    } else {
        feishu
            .send_card(
                &resolved.provider,
                &resolved.target,
                resolved.content.clone(),
                Default::default(),
            )
            .await
    };

    // Record outbound message log
    let (status, message_id, error_msg) = match &result {
        Ok(r) => (MessageStatus::Success, r.message_id.clone(), None),
        Err(e) => (MessageStatus::Failed, None, Some(e.to_string())),
    };
    let log = ImMessageLog {
        id: uuid_short(),
        provider_id: resolved.provider.id.clone(),
        direction: MessageDirection::Outbound,
        status,
        timestamp: now_ms(),
        target_id: Some(resolved.log_target_id),
        target_name: Some(resolved.log_target_name),
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

#[derive(Debug)]
struct ResolvedSendMessage {
    provider: ImProviderConfig,
    target: ImTarget,
    log_target_id: String,
    log_target_name: String,
    content: serde_json::Value,
}

fn resolve_send_message_request(
    service: &ImGatewayService,
    body: &SendMessageRequest,
) -> std::result::Result<ResolvedSendMessage, (StatusCode, String)> {
    let content = normalized_send_content(body)?;
    let target_id = body.target_id.as_deref().unwrap_or("__owner__");

    if matches!(target_id, "__owner__" | "owner") {
        let provider_id = body.provider_id.as_deref().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "provider_id is required when sending to owner".to_string(),
            )
        })?;
        let provider = service.provider_store.get(provider_id).ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Provider '{provider_id}' not found"),
            )
        })?;
        let owner_open_id = provider.owner_open_id.clone().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("Provider '{provider_id}' has no owner_open_id"),
            )
        })?;
        let target = ImTarget {
            id: "__owner__".to_string(),
            provider_id: provider.id.clone(),
            display_name: "Owner".to_string(),
            receive_id_type: "open_id".to_string(),
            receive_id: owner_open_id,
            default_msg_type: default_msg_type(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        };
        return Ok(ResolvedSendMessage {
            provider,
            target,
            log_target_id: "__owner__".to_string(),
            log_target_name: "Owner".to_string(),
            content,
        });
    }

    let target = service.target_store.get(target_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("Target '{target_id}' not found"),
        )
    })?;
    if let Some(provider_id) = body.provider_id.as_deref() {
        if target.provider_id != provider_id {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Target '{target_id}' does not belong to provider '{provider_id}'"),
            ));
        }
    }
    let provider = service
        .provider_store
        .get(&target.provider_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "Provider not found for target".to_string(),
            )
        })?;
    let log_target_id = target.id.clone();
    let log_target_name = target.display_name.clone();
    Ok(ResolvedSendMessage {
        provider,
        target,
        log_target_id,
        log_target_name,
        content,
    })
}

fn normalized_send_content(
    body: &SendMessageRequest,
) -> std::result::Result<serde_json::Value, (StatusCode, String)> {
    if !body.content.is_null() {
        return Ok(body.content.clone());
    }
    if let Some(text) = &body.text {
        return Ok(serde_json::Value::String(text.clone()));
    }
    if let Some(card) = &body.card {
        return Ok(card.clone());
    }
    Err((
        StatusCode::BAD_REQUEST,
        "content is required for messages/send".to_string(),
    ))
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
        fn default_chat_image_mime_type() -> String {
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
        let result = bifrost_agent::session::run_turn_with_mcp_multimodal(
            &service.agent_client,
            &config,
            &mut session,
            &service.agent_tools,
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

fn agent_config_response(config: crate::im_gateway::agent::ImAgentConfig) -> serde_json::Value {
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

fn patch_optional_string(target: &mut Option<String>, patch: &serde_json::Value, keys: &[&str]) {
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

fn build_online_notification_message(provider: &ImProviderConfig) -> String {
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

fn build_agent_api_status_text(
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

fn resolve_agent_api_status_detail(
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
        "agent_config": provider.agent_config,
        "created_at": provider.created_at,
        "updated_at": provider.updated_at,
    })
}

fn parse_provider_create_payload(
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
    if let Some(agent_config_value) = patch.get("agent_config") {
        if agent_config_value.is_null() {
            provider.agent_config = None;
        } else if let Some(agent_config_obj) = agent_config_value.as_object() {
            let agent_config = provider.agent_config.get_or_insert(ImProviderAgentConfig {
                work_dir: None,
                base_instructions: None,
                developer_instructions: None,
                user_instructions: None,
            });
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

fn persist_provider_agent_work_dir(
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

fn normalize_provider_agent_config(provider: &mut ImProviderConfig) {
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
        && agent_config.base_instructions.is_none()
        && agent_config.developer_instructions.is_none()
        && agent_config.user_instructions.is_none()
    {
        provider.agent_config = None;
    }
}

fn apply_provider_agent_config_string(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::im_gateway::types::ImProviderType;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct EnvGuard {
        old_data_dir: Option<String>,
    }

    impl EnvGuard {
        fn set_data_dir(data_dir: &std::path::Path) -> Self {
            let old_data_dir = std::env::var("BIFROST_DATA_DIR").ok();
            std::env::set_var("BIFROST_DATA_DIR", data_dir);
            Self { old_data_dir }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.old_data_dir.as_deref() {
                Some(value) => std::env::set_var("BIFROST_DATA_DIR", value),
                None => std::env::remove_var("BIFROST_DATA_DIR"),
            }
        }
    }

    #[test]
    fn online_notification_message_uses_provider_work_dir_override() {
        let mut provider = test_provider();
        provider.agent_config = Some(ImProviderAgentConfig {
            work_dir: Some("/custom/im-provider-workdir".to_string()),
            base_instructions: None,
            developer_instructions: None,
            user_instructions: None,
        });

        let message = build_online_notification_message(&provider);

        assert!(message.starts_with("你好，Bifrost 助手上线了"));
        assert!(message.contains("工作目录：/custom/im-provider-workdir"));
    }

    #[test]
    fn online_notification_message_falls_back_to_process_work_dir() {
        let cwd = std::env::current_dir()
            .expect("current dir")
            .display()
            .to_string();
        let provider = test_provider();

        let message = build_online_notification_message(&provider);

        assert!(message.starts_with("你好，Bifrost 助手上线了"));
        assert!(message.contains("工作目录："));
        assert!(message.contains(&cwd));
    }

    struct TestChatCompletionMock {
        port: u16,
        requests: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    impl TestChatCompletionMock {
        async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind mock chat server");
            let port = listener.local_addr().expect("mock local addr").port();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let requests_for_server = Arc::clone(&requests);

            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    let io = TokioIo::new(stream);
                    let requests = Arc::clone(&requests_for_server);
                    tokio::spawn(async move {
                        let service = service_fn(move |req: Request<Incoming>| {
                            let requests = Arc::clone(&requests);
                            async move {
                                let body_bytes = req
                                    .into_body()
                                    .collect()
                                    .await
                                    .map(|body| body.to_bytes())
                                    .unwrap_or_else(|_| Bytes::new());
                                let body: serde_json::Value =
                                    serde_json::from_slice(&body_bytes).unwrap_or_default();
                                requests.lock().expect("requests lock").push(body);
                                let response = serde_json::json!({
                                    "choices": [{
                                        "message": {
                                            "role": "assistant",
                                            "content": "IM_PROVIDER_CONFIG_OK"
                                        },
                                        "finish_reason": "stop"
                                    }],
                                    "usage": {
                                        "prompt_tokens": 10,
                                        "completion_tokens": 4,
                                        "total_tokens": 14
                                    }
                                });
                                Ok::<_, hyper::Error>(
                                    Response::builder()
                                        .status(StatusCode::OK)
                                        .header("Content-Type", "application/json")
                                        .body(Full::new(Bytes::from(response.to_string())))
                                        .unwrap(),
                                )
                            }
                        });
                        let _ = http1::Builder::new().serve_connection(io, service).await;
                    });
                }
            });

            Self { port, requests }
        }

        fn url(&self) -> String {
            format!("http://127.0.0.1:{}/chat/completions", self.port)
        }
    }

    fn request_messages_contain(body: &serde_json::Value, needle: &str) -> bool {
        body.get("messages")
            .and_then(|messages| messages.as_array())
            .map(|messages| {
                messages.iter().any(|message| {
                    let Some(content) = message.get("content") else {
                        return false;
                    };
                    if let Some(text) = content.as_str() {
                        return text.contains(needle);
                    }
                    content
                        .as_array()
                        .map(|parts| {
                            parts.iter().any(|part| {
                                part.get("text")
                                    .and_then(|value| value.as_str())
                                    .map(|text| text.contains(needle))
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    }

    fn request_contains_image_url(body: &serde_json::Value) -> bool {
        request_image_url_count(body) > 0
    }

    fn request_image_url_count(body: &serde_json::Value) -> usize {
        body.get("messages")
            .and_then(|messages| messages.as_array())
            .map(|messages| {
                messages
                    .iter()
                    .map(|message| {
                        message
                            .get("content")
                            .and_then(|content| content.as_array())
                            .map(|parts| {
                                parts
                                    .iter()
                                    .filter(|part| {
                                        part.get("type").and_then(|value| value.as_str())
                                            == Some("image_url")
                                            && part
                                                .pointer("/image_url/url")
                                                .and_then(|value| value.as_str())
                                                .is_some_and(|url| {
                                                    url.starts_with("data:image/png;base64,")
                                                })
                                    })
                                    .count()
                            })
                            .unwrap_or(0)
                    })
                    .sum()
            })
            .unwrap_or(0)
    }

    fn request_message_role(body: &serde_json::Value, idx: usize) -> Option<&str> {
        body.get("messages")?
            .as_array()?
            .get(idx)?
            .get("role")?
            .as_str()
    }

    fn test_provider() -> ImProviderConfig {
        ImProviderConfig {
            id: "feishu-main".to_string(),
            provider_type: ImProviderType::Feishu,
            display_name: "Feishu Main".to_string(),
            enabled: true,
            base_url: None,
            app_id: Some("cli_xxx".to_string()),
            secret_ref: None,
            owner_open_id: None,
            event_connection_enabled: true,
            event_types: Vec::new(),
            agent_config: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn send_message_request_resolves_owner_target_from_provider() {
        let temp_dir = tempfile::tempdir().expect("temp data dir");
        let service = ImGatewayService::new(temp_dir.path());
        let mut provider = test_provider();
        provider.owner_open_id = Some("ou-owner".to_string());
        service
            .provider_store
            .add(provider)
            .expect("provider should be saved");

        let body = SendMessageRequest {
            provider_id: Some("feishu-main".to_string()),
            target_id: Some("__owner__".to_string()),
            msg_type: "text".to_string(),
            content: serde_json::json!("hello"),
            text: None,
            card: None,
        };

        let resolved =
            resolve_send_message_request(&service, &body).expect("owner target should resolve");

        assert_eq!(resolved.provider.id, "feishu-main");
        assert_eq!(resolved.target.provider_id, "feishu-main");
        assert_eq!(resolved.target.receive_id_type, "open_id");
        assert_eq!(resolved.target.receive_id, "ou-owner");
        assert_eq!(resolved.log_target_id, "__owner__");
        assert_eq!(resolved.log_target_name, "Owner");
        assert_eq!(resolved.content, serde_json::json!("hello"));
    }

    #[test]
    fn send_message_request_rejects_owner_without_provider() {
        let temp_dir = tempfile::tempdir().expect("temp data dir");
        let service = ImGatewayService::new(temp_dir.path());
        let body = SendMessageRequest {
            provider_id: None,
            target_id: Some("__owner__".to_string()),
            msg_type: "text".to_string(),
            content: serde_json::json!("hello"),
            text: None,
            card: None,
        };

        let error = resolve_send_message_request(&service, &body)
            .expect_err("owner send should require provider");

        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert!(error.1.contains("provider_id is required"));
    }

    #[test]
    fn provider_agent_config_patch_sets_and_clears_overrides() {
        let mut provider = test_provider();

        apply_provider_patch(
            &mut provider,
            &serde_json::json!({
                "agent_config": {
                    "work_dir": " /tmp/bifrost-im ",
                    "base_instructions": " Provider prompt "
                }
            }),
        );

        let agent_config = provider.agent_config.as_ref().expect("agent_config");
        assert_eq!(agent_config.work_dir.as_deref(), Some("/tmp/bifrost-im"));
        assert_eq!(
            agent_config.base_instructions.as_deref(),
            Some("Provider prompt")
        );

        apply_provider_patch(
            &mut provider,
            &serde_json::json!({
                "agent_config": {
                    "work_dir": null,
                    "base_instructions": ""
                }
            }),
        );

        assert!(provider.agent_config.is_none());
    }

    #[test]
    fn provider_create_payload_maps_app_secret_without_exposing_it() {
        let provider = parse_provider_create_payload(serde_json::json!({
            "id": "feishu-main",
            "provider_type": "feishu",
            "display_name": "Feishu Main",
            "enabled": true,
            "app_id": "cli_xxx",
            "app_secret": "sk_test_secret",
            "event_connection_enabled": true,
            "event_types": []
        }))
        .expect("provider create payload should parse");

        assert_eq!(provider.secret_ref.as_deref(), Some("sk_test_secret"));
        assert_eq!(provider.display_name, "Feishu Main");

        let safe = sanitize_provider(&provider);
        assert_eq!(
            safe.get("secret_configured").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(safe.get("secret_ref").is_none());
        assert!(safe.get("app_secret").is_none());
        assert!(!safe.to_string().contains("sk_test_secret"));
    }

    #[test]
    fn provider_create_payload_defaults_missing_display_name_to_id() {
        let provider = parse_provider_create_payload(serde_json::json!({
            "id": "feishu-main",
            "provider_type": "feishu",
            "enabled": true,
            "app_id": "cli_xxx",
            "app_secret": "sk_test_secret",
            "event_connection_enabled": true,
            "event_types": []
        }))
        .expect("provider create payload should default display_name");

        assert_eq!(provider.display_name, "feishu-main");
        assert_eq!(provider.secret_ref.as_deref(), Some("sk_test_secret"));
    }

    #[test]
    fn provider_agent_config_overrides_base_agent_config() {
        let base = crate::im_gateway::agent::ImAgentConfig {
            work_dir: Some("/global".to_string()),
            base_instructions: Some("global prompt".to_string()),
            developer_instructions: Some("global developer".to_string()),
            user_instructions: Some("global user".to_string()),
            ..Default::default()
        };

        let mut provider = test_provider();
        provider.agent_config = Some(ImProviderAgentConfig {
            work_dir: Some("/provider".to_string()),
            base_instructions: Some("provider prompt".to_string()),
            developer_instructions: Some("provider developer".to_string()),
            user_instructions: Some("provider user".to_string()),
        });

        let effective = effective_agent_config_for_provider(&base, &provider);
        assert_eq!(effective.work_dir.as_deref(), Some("/provider"));
        assert_eq!(
            effective.base_instructions.as_deref(),
            Some("provider prompt")
        );
        assert_eq!(
            effective.developer_instructions.as_deref(),
            Some("provider developer")
        );
        assert_eq!(
            effective.user_instructions.as_deref(),
            Some("provider user")
        );
    }

    #[test]
    fn provider_switch_workdir_persists_provider_agent_override() {
        let temp_dir = tempfile::tempdir().expect("temp data dir");
        let store = Arc::new(ImProviderStore::new(temp_dir.path()));
        let mut provider = test_provider();
        provider.id = "persist-workdir-provider".to_string();
        provider.agent_config = Some(ImProviderAgentConfig {
            work_dir: Some("/old".to_string()),
            base_instructions: Some("keep provider prompt".to_string()),
            developer_instructions: None,
            user_instructions: None,
        });
        store.add(provider).expect("add provider");

        persist_provider_agent_work_dir(&store, "persist-workdir-provider", " /new/workdir ");

        let updated = store.get("persist-workdir-provider").expect("provider");
        let agent_config = updated.agent_config.expect("agent_config");
        assert_eq!(agent_config.work_dir.as_deref(), Some("/new/workdir"));
        assert_eq!(
            agent_config.base_instructions.as_deref(),
            Some("keep provider prompt")
        );
    }

    #[test]
    fn agent_api_status_detail_applies_work_dir_for_fresh_status_session() {
        let manager = bifrost_agent::AgentSessionManager::new(3600);

        let detail = resolve_agent_api_status_detail(
            &manager,
            "status-fresh-workdir",
            Some("/tmp/bifrost-status-workdir".to_string()),
        )
        .expect("requested work_dir should create status detail");

        assert_eq!(
            detail.work_dir.as_deref(),
            Some("/tmp/bifrost-status-workdir")
        );
        assert_eq!(detail.message_count, 0);
    }

    #[test]
    fn agent_api_status_detail_overrides_existing_idle_session_work_dir() {
        let manager = bifrost_agent::AgentSessionManager::new(3600);
        let session = manager
            .try_take_session_with_work_dir("status-existing-workdir", Some("/tmp/old".to_string()))
            .expect("initial session should be available");
        manager.return_session(session);

        let detail = resolve_agent_api_status_detail(
            &manager,
            "status-existing-workdir",
            Some("/tmp/new".to_string()),
        )
        .expect("existing status detail should remain available");

        assert_eq!(detail.work_dir.as_deref(), Some("/tmp/new"));
    }

    #[test]
    fn agent_api_status_detail_keeps_new_session_text_when_no_work_dir_requested() {
        let manager = bifrost_agent::AgentSessionManager::new(3600);

        let detail = resolve_agent_api_status_detail(&manager, "status-no-workdir", None);

        assert!(detail.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn im_event_loop_uses_provider_agent_config_for_agent_chat() {
        let temp_dir = tempfile::tempdir().expect("temp data dir");
        let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
        let mock = TestChatCompletionMock::start().await;
        let service = ImGatewayService::new(temp_dir.path());

        let mut base_config = service.agent_config_store.load();
        base_config.enabled = true;
        base_config.model = Some("mock-model".to_string());
        base_config.model_provider = Some("mock".to_string());
        base_config.work_dir = Some(std::env::current_dir().unwrap().display().to_string());
        base_config.base_instructions = Some("GLOBAL_BASE_SHOULD_NOT_APPEAR".to_string());
        base_config.developer_instructions = Some("GLOBAL_DEV_SHOULD_NOT_APPEAR".to_string());
        base_config.user_instructions = Some("GLOBAL_USER_SHOULD_NOT_APPEAR".to_string());
        base_config.max_turn_iterations = Some(1);
        base_config.model_providers.insert(
            "mock".to_string(),
            bifrost_agent::config::ModelProviderConfig {
                name: Some("Mock".to_string()),
                base_url: Some(mock.url()),
                env_key: None,
                api_key: None,
                http_headers: Some(HashMap::from([(
                    "Authorization".to_string(),
                    "Bearer test".to_string(),
                )])),
                env_http_headers: None,
                request_max_retries: None,
                stream_idle_timeout_ms: None,
                stream_max_retries: None,
            },
        );
        service
            .agent_config_store
            .save(&base_config)
            .expect("save base agent config");

        let mut provider = test_provider();
        provider.id = "new-im-provider-config".to_string();
        provider.owner_open_id = Some("owner-open-id".to_string());
        provider.base_url = Some("http://127.0.0.1:9".to_string());
        let mut provider_in_store = provider.clone();
        provider_in_store.agent_config = Some(ImProviderAgentConfig {
            work_dir: Some(std::env::current_dir().unwrap().display().to_string()),
            base_instructions: Some(
                "IM_PROVIDER_BASE_OK: answer IM_PROVIDER_CONFIG_OK".to_string(),
            ),
            developer_instructions: Some("IM_PROVIDER_DEV_OK".to_string()),
            user_instructions: Some("IM_PROVIDER_USER_OK".to_string()),
        });
        service
            .provider_store
            .add(provider_in_store)
            .expect("add current provider config to store");

        let (tx, rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(run_event_loop(
            rx,
            Arc::clone(service.connection_manager.feishu_provider()),
            provider.clone(),
            Arc::clone(&service.event_store),
            Arc::clone(&service.message_log_store),
            Arc::clone(&service.route_store),
            Arc::clone(&service.provider_store),
            Arc::clone(&service.agent_config_store),
            Arc::clone(&service.agent_client),
            Arc::clone(&service.agent_tools),
            Arc::clone(&service.agent_session_manager),
            Arc::clone(&service.queue_manager),
            Arc::clone(&service.progress_registry),
        ));

        tx.send(ImEvent {
            event_id: "evt-im-provider-agent-config".to_string(),
            provider_id: provider.id.clone(),
            provider_type: ImProviderType::Feishu,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource {
                chat_id: Some("chat-id".to_string()),
                user_id: Some("owner-open-id".to_string()),
                message_id: None,
            },
            message: Some(crate::im_gateway::types::ImEventMessage {
                text: "IM_PROVIDER_CHAT_MARKER 请只回复 IM_PROVIDER_CONFIG_OK".to_string(),
                mentions: Vec::new(),
                images: Vec::new(),
                raw_type: Some("text".to_string()),
            }),
            received_at: now_ms(),
            raw_digest: None,
        })
        .expect("send IM event");
        drop(tx);

        tokio::time::timeout(std::time::Duration::from_secs(10), handle)
            .await
            .expect("event loop timed out")
            .expect("event loop task panicked");

        let requests = mock.requests.lock().expect("requests lock");
        let request = requests.first().expect("mock received chat request");
        assert_eq!(request_message_role(request, 0), Some("system"));
        assert_eq!(request_message_role(request, 1), Some("developer"));
        assert_eq!(request_message_role(request, 2), Some("user"));
        assert!(request_messages_contain(request, "IM_PROVIDER_BASE_OK"));
        assert!(request_messages_contain(request, "IM_PROVIDER_DEV_OK"));
        assert!(request_messages_contain(request, "IM_PROVIDER_USER_OK"));
        assert!(request_messages_contain(request, "IM_PROVIDER_CHAT_MARKER"));
        assert!(!request_messages_contain(
            request,
            "GLOBAL_BASE_SHOULD_NOT_APPEAR"
        ));
        assert!(!request_messages_contain(
            request,
            "GLOBAL_DEV_SHOULD_NOT_APPEAR"
        ));
        assert!(!request_messages_contain(
            request,
            "GLOBAL_USER_SHOULD_NOT_APPEAR"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn im_event_loop_forwards_image_attachment_to_agent_chat() {
        let temp_dir = tempfile::tempdir().expect("temp data dir");
        let _env_guard = EnvGuard::set_data_dir(temp_dir.path());
        let mock = TestChatCompletionMock::start().await;
        let service = ImGatewayService::new(temp_dir.path());

        let mut base_config = service.agent_config_store.load();
        base_config.enabled = true;
        base_config.model = Some("mock-vision-model".to_string());
        base_config.model_provider = Some("mock".to_string());
        base_config.work_dir = Some(std::env::current_dir().unwrap().display().to_string());
        base_config.max_turn_iterations = Some(1);
        base_config.model_providers.insert(
            "mock".to_string(),
            bifrost_agent::config::ModelProviderConfig {
                name: Some("Mock".to_string()),
                base_url: Some(mock.url()),
                env_key: None,
                api_key: None,
                http_headers: Some(HashMap::from([(
                    "Authorization".to_string(),
                    "Bearer test".to_string(),
                )])),
                env_http_headers: None,
                request_max_retries: None,
                stream_idle_timeout_ms: None,
                stream_max_retries: None,
            },
        );
        service
            .agent_config_store
            .save(&base_config)
            .expect("save base agent config");

        let mut provider = test_provider();
        provider.id = "image-provider".to_string();
        provider.owner_open_id = Some("owner-open-id".to_string());
        provider.base_url = Some("http://127.0.0.1:9".to_string());
        service
            .provider_store
            .add(provider.clone())
            .expect("add provider");

        let (tx, rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(run_event_loop(
            rx,
            Arc::clone(service.connection_manager.feishu_provider()),
            provider.clone(),
            Arc::clone(&service.event_store),
            Arc::clone(&service.message_log_store),
            Arc::clone(&service.route_store),
            Arc::clone(&service.provider_store),
            Arc::clone(&service.agent_config_store),
            Arc::clone(&service.agent_client),
            Arc::clone(&service.agent_tools),
            Arc::clone(&service.agent_session_manager),
            Arc::clone(&service.queue_manager),
            Arc::clone(&service.progress_registry),
        ));

        tx.send(ImEvent {
            event_id: "evt-im-image-agent-chat".to_string(),
            provider_id: provider.id.clone(),
            provider_type: ImProviderType::Feishu,
            event_type: "message.receive".to_string(),
            source: crate::im_gateway::types::ImEventSource {
                chat_id: Some("chat-id".to_string()),
                user_id: Some("owner-open-id".to_string()),
                message_id: Some("om-image".to_string()),
            },
            message: Some(crate::im_gateway::types::ImEventMessage {
                text: "".to_string(),
                mentions: Vec::new(),
                images: (0..7)
                    .map(|idx| crate::im_gateway::types::ImImageAttachment {
                        file_key: format!("img-unit-{idx}"),
                        source: crate::im_gateway::types::ImImageSource::MessageResource,
                        mime_type: Some("image/png".to_string()),
                        data_base64: Some("iVBORw0KGgo=".to_string()),
                    })
                    .collect(),
                raw_type: Some("image".to_string()),
            }),
            received_at: now_ms(),
            raw_digest: None,
        })
        .expect("send IM image event");
        drop(tx);

        tokio::time::timeout(std::time::Duration::from_secs(10), handle)
            .await
            .expect("event loop timed out")
            .expect("event loop task panicked");

        let requests = mock.requests.lock().expect("requests lock");
        let request = requests.first().expect("mock received chat request");
        assert!(request_messages_contain(request, IMAGE_ONLY_AGENT_PROMPT));
        assert!(request_contains_image_url(request));
        assert_eq!(
            request_image_url_count(request),
            MAX_AGENT_IMAGES_PER_MESSAGE
        );
    }
}
