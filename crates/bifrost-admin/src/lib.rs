#![recursion_limit = "512"]
// Admin handlers intentionally return complete HTTP/WebSocket errors so callers
// can forward status, headers, and bodies without a second error conversion.
// Boxing these cold-path values would complicate every handler boundary for no
// measured data-plane benefit.
#![allow(clippy::result_large_err)]

pub mod admin_audit;
mod admin_auth;
pub mod admin_auth_db;
mod app_icon;
pub mod asr_runtime;
mod async_traffic;
pub mod auth_inspect;
mod body_store;
pub mod breakpoint;
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
pub mod mobile_availability;
pub mod network;
pub mod notification_db;
pub mod openapi;
mod port_rebind;
pub mod push;
mod query_service;
pub mod remote_invoke;
pub mod replay;
pub(crate) mod replay_body_rules;
pub mod replay_db;
pub mod replay_executor;
pub(crate) mod replay_response_rules;
pub(crate) mod replay_scripts;
pub mod request_rules;
mod resource_alerts;
pub mod resource_download;
mod router;
pub mod rule_share_import;
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
pub mod worker_runtime;
mod ws_payload_store;

#[cfg(any(test, feature = "test-support"))]
pub mod test_env;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests;

pub use admin_auth::{
    get_admin_username, get_failed_login_count, has_admin_password, is_remote_access_enabled,
    record_failed_login, reset_failed_login_count, revoke_all_admin_sessions,
    set_admin_password_hash, set_admin_username, set_remote_access_enabled, validate_admin_jwt,
    validate_password_strength, AdminJwtClaims, MAX_LOGIN_ATTEMPTS, MIN_PASSWORD_LENGTH,
};
pub use app_icon::{create_app_icon_cache, run_app_icon_worker, AppIconCache, SharedAppIconCache};
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
pub use handlers::traffic::IncrementalContentDecoder;
pub mod asr_streaming {
    pub use crate::handlers::asr_streaming::{
        append_transcript_delta, call_asr_whole_file_endpoint, dedupe_increment,
        WholeFileTranscription,
    };
}
pub mod asr_cli_invoke {
    pub use crate::handlers::asr_cli_invoke::{parse_asr_cli_text, run_asr_cli};
}
pub use handlers::asr::shutdown_managed_asr_service;
pub use handlers::asr_jobs::run_asr_diarization_worker_stdio;
pub mod voice {
    pub use crate::handlers::voice::{
        apply_voice_vocabulary, discover_voice_sources, load_voice_vocabulary,
        save_voice_vocabulary, VoiceSource, VoiceSourceStatus, VoiceVocabulary,
        VoiceVocabularyTerm,
    };
    pub use crate::handlers::voice_stateful::{run_stateful_worker_stdio, StatefulVoiceConfig};
}
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
    SharedSystemProxyLifecycleHelperState, SharedSystemProxyManager, SharedTrayLaunchCallback,
    SharedValuesStorage, SystemProxyLifecycleHelperState,
};
pub use temp_ports::{
    RuleSetRef, SharedTemporaryPortManager, TemporaryPortActiveSummary, TemporaryPortBindRequest,
    TemporaryPortBinding, TemporaryPortError, TemporaryPortManager, TemporaryPortRuleItem,
    TemporaryPortStatus, TemporaryPortUpdateRequest,
};
pub use traffic::{
    FrameDirection, FrameType, MatchedRule, RequestTiming, SocketStatus, StoredBodyMetadata,
    TrafficBodyMetadata, TrafficRecord, TRAFFIC_BODY_METADATA_VERSION,
};
pub use traffic_db::{
    start_db_cleanup_task, Direction, QueryParams, QueryResult, SharedTrafficDbStore,
    TrafficDbStats, TrafficDbStore, TrafficFlags, TrafficStoreEvent, TrafficSummaryCompact,
};
pub use ws_payload_store::{
    start_ws_payload_cleanup_task, SharedWsPayloadStore, WsPayloadStore, WsPayloadStoreConfigUpdate,
};

pub use handlers::remote_invoke::SharedRemoteInvokeWorker;
pub use handlers::trust_probe::{
    handle_trust_probe_proxy_configured_request, is_active_trust_probe_target,
    TRUST_PROBE_PROXY_CONFIG_HOST, TRUST_PROBE_PROXY_CONFIG_PATH,
};
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
pub const MOBILE_PUBLIC_PATH_PREFIX: &str = "/_bifrost/public/mobile";
pub const PROXY_PUBLIC_PATH_PREFIX: &str = "/_bifrost/public/proxy";
pub const TRUST_PROBE_PUBLIC_PATH_PREFIX: &str = "/_bifrost/public/trust-probe";
pub const TRUST_PROBE_SHORT_PUBLIC_PATH: &str = "/_bifrost/tp";
