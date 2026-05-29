pub mod access_control;
pub mod bifrost_file;
pub mod error;
pub mod file_access;
pub mod http_client;
pub mod limits;
pub mod logging;
pub mod matcher;
pub mod panic_handler;
pub mod protocol;
pub mod rule;
pub mod shell_proxy;
pub mod syntax;
pub mod system_proxy;
pub mod text;
pub mod version_check;

pub use access_control::{
    AccessControlConfig, AccessDecision, AccessMode, ClientAccessControl, PendingAuth,
    ProxyAuthRateLimiter, UserPassAccountConfig, UserPassAccountStatus, UserPassAuthConfig,
    UserPassAuthStatus,
};
pub use error::{BifrostError, Result};
pub use http_client::{
    direct_blocking_reqwest_client_builder, direct_reqwest_client_builder, direct_ureq_agent,
    direct_ureq_agent_builder, load_reqwest_certificate, proxied_reqwest_client_builder,
};
pub use logging::{
    init_logging, init_logging_with_config, reinit_logging_for_daemon, rotate_daemon_err_log,
    LogConfig, LogGuard, LogOutput,
};
pub use matcher::{
    factory::parse_pattern, DomainMatcher, IpMatcher, MatchResult, Matcher, RegexMatcher,
    WildcardMatcher,
};
pub use panic_handler::{install_panic_hook, spawn_with_panic_guard};
pub use protocol::*;
pub use rule::{
    create_shared_store, extract_inline_variables, parse_line, parse_rules, parse_rules_tolerant,
    validate_rules, validate_rules_with_context, CodeFix, CompositeValueStore, MemoryValueStore,
    ParseError, ParseErrorSeverity, ParseResult, RequestContext, RequestContextBuilder,
    ResolvedRule, ResolvedRules, Rule, RuleGroup, RuleGroupManager, RuleParser, RulesResolver,
    ScriptReference, SharedValueStore, TemplateEngine, ValidationResult, ValueStore, VariableInfo,
};
pub use shell_proxy::{ShellProxyManager, ShellProxyStatus, ShellType};
pub use syntax::{
    get_all_protocols, get_filter_value_specs, get_pattern_types, get_syntax_info,
    get_template_variables, validate_filter_value, FilterValidationError, PatternInfo,
    ProtocolInfo, ProtocolValueSpec, SyntaxInfo, TemplateVariableInfo, ValueHint,
};
pub use system_proxy::{ProxyBackup, SystemProxyManager};
