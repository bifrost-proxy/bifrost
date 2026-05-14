pub mod admin_audit;
mod admin_auth;
pub mod admin_auth_db;
mod app_icon;
pub mod asr_runtime;
mod async_traffic;
mod body_store;
pub mod client_trust_tracker;
pub mod connection_monitor;
pub mod connection_registry;
pub mod cors;
pub mod devtools;
mod frame_store;
mod handlers;
pub mod im_gateway;
pub mod ip_tls_pending;
mod metrics;
pub mod network;
pub mod notification_db;
mod port_rebind;
pub mod push;
mod query_service;
pub mod remote_invoke;
pub(crate) mod replay_body_rules;
pub mod replay_db;
pub mod replay_executor;
pub(crate) mod replay_response_rules;
pub mod request_rules;
mod resource_alerts;
pub mod resource_download;
mod router;
pub mod search;
mod security;
mod sse;
mod state;
mod static_files;
pub mod status_printer;
pub mod temp_ports;
mod traffic;
pub mod traffic_db;
mod version_check;
mod ws_payload_store;

#[cfg(test)]
mod tests;

pub use admin_auth::{
    get_admin_username, get_failed_login_count, has_admin_password, is_remote_access_enabled,
    record_failed_login, reset_failed_login_count, revoke_all_admin_sessions,
    set_admin_password_hash, set_admin_username, set_remote_access_enabled, validate_admin_jwt,
    validate_password_strength, AdminJwtClaims, MAX_LOGIN_ATTEMPTS, MIN_PASSWORD_LENGTH,
};
pub use app_icon::{create_app_icon_cache, AppIconCache, SharedAppIconCache};
pub use async_traffic::{
    start_async_traffic_processor, AsyncTrafficWriter, SharedAsyncTrafficWriter, TrafficCommand,
};
pub use body_store::{
    start_body_cleanup_task, BodyRef, BodyStore, BodyStreamWriter, SharedBodyStore,
};
pub use client_trust_tracker::{
    classify_tls_accept_error, ClientTlsTrustTracker, ClientTrustEvent, ClientTrustSummary,
    DomainFailureSummary, TlsAcceptFailureReason,
};
pub use connection_monitor::{
    start_connection_cleanup_task, ConnectionMonitor, SharedConnectionMonitor, WebSocketFrameRecord,
};
pub use connection_registry::{
    ConfigChangeEvent, ConnectionInfo, ConnectionRegistry, SharedConnectionRegistry,
};
pub use frame_store::{start_frame_cleanup_task, FrameStore, FrameStoreStats, SharedFrameStore};
pub use handlers::im_gateway::{ImGatewayService, SharedImGatewayService};
pub use handlers::scripts::ScriptManager;
pub use handlers::sync::handle_sync_login_callback;
pub use ip_tls_pending::{IpTlsPendingManager, PendingIpTls, PendingIpTlsEvent};
pub use metrics::{
    start_metrics_collector_task, MetricsCollector, MetricsSnapshot, TrafficType,
    TrafficTypeMetrics,
};
pub use notification_db::{
    count_notifications, count_unread, create_notification, list_notifications, mark_all_as_read,
    update_notification_status, CreateNotification, NotificationRecord,
};
pub use port_rebind::{
    PortRebindManager, PortRebindRequest, PortRebindResponse, SharedPortRebindManager,
};
pub use push::{start_push_tasks, PushManager, SharedPushManager};
pub use query_service::{
    search_request_from_command, traffic_list_params_from_command, AdminQueryService,
};
pub use router::AdminRouter;
pub use security::{is_cert_public_request, is_valid_admin_request, AdminSecurityConfig};
pub use sse::{
    assemble_openai_like_response_body_from_text, parse_sse_event, parse_sse_events_from_text,
    SseEvent, SseHub, MAX_OPENAI_LIKE_SSE_ASSEMBLY_INPUT_BYTES,
};
pub use state::{
    start_total_disk_cleanup_task, AdminState, RuntimeConfig, SharedAccessControl,
    SharedClientTrustTracker, SharedIpTlsPendingManager, SharedRuntimeConfig, SharedScriptManager,
    SharedSystemProxyManager, SharedValuesStorage,
};
pub use temp_ports::{
    RuleSetRef, SharedTemporaryPortManager, TemporaryPortActiveSummary, TemporaryPortBindRequest,
    TemporaryPortBinding, TemporaryPortError, TemporaryPortManager, TemporaryPortRuleItem,
    TemporaryPortStatus, TemporaryPortUpdateRequest,
};
pub use traffic::{
    FrameDirection, FrameType, MatchedRule, RequestTiming, SocketStatus, TrafficRecord,
};
pub use traffic_db::{
    start_db_cleanup_task, Direction, QueryParams, QueryResult, SharedTrafficDbStore,
    TrafficDbStats, TrafficDbStore, TrafficFlags, TrafficStoreEvent, TrafficSummaryCompact,
};
pub use ws_payload_store::{
    start_ws_payload_cleanup_task, SharedWsPayloadStore, WsPayloadStore, WsPayloadStoreConfigUpdate,
};

pub use handlers::remote_invoke::SharedRemoteInvokeWorker;
pub use remote_invoke::{
    identity::Identity as RemoteInvokeIdentity, types::RemoteInvokeConfig,
    worker::RemoteInvokeWorker,
};
pub use replay_db::{
    ReplayDbStore, ReplayGroup, ReplayHistory, ReplayRequest, ReplayRequestSummary, RuleConfig,
    RuleMode, SharedReplayDbStore, MAX_CONCURRENT_REPLAYS, MAX_HISTORY, MAX_REQUESTS,
};
pub use replay_executor::{
    ReplayError, ReplayExecuteRequest, ReplayExecuteResponse, ReplayExecutor, ReplayRequestData,
    SharedReplayExecutor,
};
pub use request_rules::{
    apply_all_request_rules, apply_host_rule, apply_url_rules, build_applied_rules, AppliedRequest,
    AppliedRules,
};
pub use search::{
    FilterCondition, MatchLocation, SearchEngine, SearchFilters, SearchRequest, SearchResponse,
    SearchResultItem, SearchScope,
};
pub use version_check::{SharedVersionChecker, VersionCheckResponse, VersionChecker};

pub const ADMIN_PATH_PREFIX: &str = "/_bifrost";
pub const CERT_PUBLIC_PATH_PREFIX: &str = "/_bifrost/public/cert";
