pub mod access_control;
pub mod bifrost_file;
pub mod enhanced_proxy;
pub mod error;
pub mod file_access;
pub mod http_client;
pub mod limits;
pub mod logging;
pub mod matcher;
pub mod panic_handler;
pub mod process_alias;
pub mod process_start_time;
pub mod protocol;
pub mod rule;
pub mod rule_share;
pub mod shell_proxy;
pub mod syntax;
pub mod system_proxy;
pub mod system_proxy_launchd;
pub mod system_proxy_recovery;
pub mod text;
pub mod upgrade_progress;
pub mod version_check;

pub use access_control::{
    AccessControlConfig, AccessDecision, AccessMode, ClientAccessControl, PendingAuth,
    ProxyAuthRateLimiter, UserPassAccountConfig, UserPassAccountStatus, UserPassAuthConfig,
    UserPassAuthStatus,
};
pub use enhanced_proxy::{
    enhanced_proxy_should_capture, EnhancedProxyDesiredState, EnhancedProxyManager,
    EnhancedProxyPolicy, EnhancedProxyState, EnhancedProxyStatus,
};
pub use error::{BifrostError, Result};
pub use http_client::{
    apply_remote_relay_headers, direct_blocking_reqwest_client_builder,
    direct_reqwest_client_builder, direct_sse_reqwest_client_builder, direct_ureq_agent,
    direct_ureq_agent_builder, format_reqwest_error, github_blocking_reqwest_client_builder,
    github_reqwest_client_builder, load_reqwest_certificate,
    outbound_blocking_reqwest_client_builder, outbound_reqwest_client_builder,
    parse_remote_relay_headers, proxied_reqwest_client_builder, remote_relay_headers_from_env,
    remote_relay_reqwest_client_builder, remote_relay_sse_reqwest_client_builder,
    BIFROST_CA_BUNDLE_ENV, BIFROST_CA_DIR_ENV, BIFROST_UNSAFE_SSL_ENV, GITHUB_CA_BUNDLE_ENV,
    GITHUB_CA_DIR_ENV, GITHUB_UNSAFE_SSL_ENV, REMOTE_RELAY_CA_BUNDLE_ENV, REMOTE_RELAY_HEADERS_ENV,
    REMOTE_UNSAFE_SSL_ENV, UPGRADE_CA_BUNDLE_ENV, UPGRADE_CA_DIR_ENV, UPGRADE_UNSAFE_SSL_ENV,
};
pub use logging::{
    cleanup_bifrost_log_dir, init_logging, init_logging_with_config, reinit_logging_for_daemon,
    rotate_daemon_err_log, LogConfig, LogGuard, LogOutput, DEFAULT_LOG_DIR_MAX_BYTES,
    DEFAULT_LOG_RETENTION_DAYS,
};
pub use matcher::{
    factory::parse_pattern, DomainMatcher, IpMatcher, MatchResult, Matcher, RegexMatcher,
    WildcardMatcher,
};
pub use panic_handler::{install_panic_hook, spawn_with_panic_guard};
pub use process_alias::process_alias_executable;
pub use process_start_time::{
    current_process_start_time_ms, get_process_start_time_ms, start_times_match, StartTimeMatch,
    START_TIME_MATCH_TOLERANCE_MS,
};
pub use protocol::*;
pub use rule::{
    create_shared_store, expand_rule_references, expand_rule_references_strict,
    expand_rule_references_to_result, extract_inline_variables, parse_line, parse_rules,
    parse_rules_tolerant, rule_reference_name, validate_rule_syntax_report, validate_rules,
    validate_rules_with_context, CodeFix, CompositeValueStore, MemoryValueStore, ParseError,
    ParseErrorSeverity, ParseResult, RequestContext, RequestContextBuilder, ResolvedRule,
    ResolvedRules, Rule, RuleGroup, RuleGroupManager, RuleParser, RuleReferenceError,
    RuleSyntaxGuidance, RuleSyntaxReport, RulesResolver, ScriptReference, SharedValueStore,
    TemplateEngine, ValidationResult, ValueStore, VariableInfo,
};
pub use shell_proxy::{ShellProxyManager, ShellProxyStatus, ShellType};
pub use syntax::{
    get_all_protocols, get_filter_value_specs, get_pattern_types, get_syntax_info,
    get_template_variables, validate_filter_value, FilterValidationError, PatternInfo,
    ProtocolInfo, ProtocolValueSpec, SyntaxInfo, TemplateVariableInfo, ValueHint,
};
pub use system_proxy::{
    repair_system_proxy_lock_permissions, ProxyBackup, SystemProxyDisableOutcome,
    SystemProxyManager,
};
pub use system_proxy_launchd::{
    consume_stop_restore_suppression, consume_system_proxy_shutdown_mode, install_launchd_cleanup,
    install_launchd_cleanup_with_gui_auth, launchd_status, launchd_status_for_config,
    read_system_proxy_shutdown_mode, render_launchd_plist, uninstall_launchd_cleanup,
    uninstall_launchd_cleanup_with_gui_auth, write_system_proxy_shutdown_mode,
    SystemProxyLaunchdConfig, SystemProxyLaunchdMode, SystemProxyLaunchdRecoveryOutcome,
    SystemProxyLaunchdStatus, SystemProxyShutdownMode,
};
pub use system_proxy_recovery::{
    is_retryable_recovery_error, retry_with_policy, RECOVERY_RETRY_INTERVAL, RECOVERY_RETRY_WINDOW,
};
