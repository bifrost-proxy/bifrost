use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

fn default_true() -> bool {
    true
}

fn default_sqlite_store() -> String {
    "sqlite".to_string()
}

fn default_retention_days() -> u32 {
    180
}

fn default_sources() -> Vec<ResearchSource> {
    vec![ResearchSource::Web]
}

fn default_limit() -> usize {
    10
}

fn default_max_results_per_query() -> usize {
    8
}

fn default_language() -> String {
    "zh-CN".to_string()
}

fn default_max_redirects() -> usize {
    5
}

fn default_max_response_bytes() -> usize {
    500_000
}

fn default_timeout_secs() -> u64 {
    20
}

fn default_user_agent() -> String {
    "BifrostResearch/0.1".to_string()
}

fn default_cdp_endpoint() -> String {
    "http://127.0.0.1:9222".to_string()
}

fn default_browser_user_data_dir() -> String {
    "~/.bifrost/web/edge-user-data".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ResearchConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(default)]
    pub providers: HashMap<String, ResearchProviderConfig>,
    #[serde(default)]
    pub provider_order: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wechat: Option<WechatResearchConfig>,
    #[serde(default)]
    pub cache: ResearchCacheConfig,
    #[serde(default)]
    pub defaults: ResearchDefaults,
    #[serde(default)]
    pub fetch_policy: ResearchFetchPolicy,
    #[serde(default)]
    pub tasks: Vec<ResearchTaskConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResearchProviderConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(rename = "type")]
    pub provider_type: ResearchProviderType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_key: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub env_headers: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_per_minute: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cdp_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_user_data_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<ResearchSiteKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub need_content: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub need_url: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub need_summary: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_formats: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_range: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_rewrite: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sites: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_hosts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_info_level: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub industry: Option<String>,
}

impl Default for ResearchProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider_type: ResearchProviderType::GenericWebSearch,
            base_url: None,
            api_key: None,
            env_key: None,
            headers: HashMap::new(),
            env_headers: HashMap::new(),
            search_url: None,
            fetch_url: None,
            request_template: None,
            rate_limit_per_minute: None,
            cdp_endpoint: None,
            browser_user_data_dir: None,
            site: None,
            search_type: None,
            count: None,
            need_content: None,
            need_url: None,
            need_summary: None,
            content_formats: None,
            time_range: None,
            query_rewrite: None,
            sites: None,
            block_hosts: None,
            auth_info_level: None,
            industry: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResearchProviderType {
    GenericWebSearch,
    VolcWebSearch,
    Tavily,
    Exa,
    CustomHttp,
    SogouWechatCdp,
    FixedSite,
    Mcp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ResearchSiteKind {
    Arxiv,
    HackerNews,
    GithubRepositories,
}

impl ResearchProviderConfig {
    pub fn cdp_endpoint_or_default(&self) -> String {
        self.cdp_endpoint
            .clone()
            .unwrap_or_else(default_cdp_endpoint)
    }

    pub fn browser_user_data_dir_or_default(&self) -> String {
        self.browser_user_data_dir
            .clone()
            .unwrap_or_else(default_browser_user_data_dir)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WechatResearchConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_wechat_mode")]
    pub mode: String,
    #[serde(default = "default_min_results_before_fallback")]
    pub min_results_before_fallback: usize,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default = "default_wechat_rate_limit")]
    pub rate_limit_per_minute: u32,
    #[serde(default = "default_max_pages_per_query")]
    pub max_pages_per_query: u32,
}

impl Default for WechatResearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: default_wechat_mode(),
            min_results_before_fallback: default_min_results_before_fallback(),
            provider: None,
            rate_limit_per_minute: default_wechat_rate_limit(),
            max_pages_per_query: default_max_pages_per_query(),
        }
    }
}

fn default_wechat_mode() -> String {
    "fallback".to_string()
}

fn default_min_results_before_fallback() -> usize {
    3
}

fn default_wechat_rate_limit() -> u32 {
    6
}

fn default_max_pages_per_query() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResearchCacheConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_sqlite_store")]
    pub store: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_path: Option<String>,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

impl Default for ResearchCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            store: default_sqlite_store(),
            db_path: None,
            retention_days: default_retention_days(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResearchDefaults {
    #[serde(default = "default_sources")]
    pub sources: Vec<ResearchSource>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub fetch_content: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

impl Default for ResearchDefaults {
    fn default() -> Self {
        Self {
            sources: default_sources(),
            limit: default_limit(),
            fetch_content: false,
            language: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResearchFetchPolicy {
    #[serde(default)]
    pub allow_private_ip: bool,
    #[serde(default)]
    pub allow_localhost: bool,
    #[serde(default = "default_max_redirects")]
    pub max_redirects: usize,
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
}

impl Default for ResearchFetchPolicy {
    fn default() -> Self {
        Self {
            allow_private_ip: false,
            allow_localhost: false,
            max_redirects: default_max_redirects(),
            max_response_bytes: default_max_response_bytes(),
            timeout_secs: default_timeout_secs(),
            user_agent: default_user_agent(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResearchTaskConfig {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub trigger: ResearchTaskTrigger,
    #[serde(default)]
    pub queries: Vec<String>,
    #[serde(default = "default_sources")]
    pub sources: Vec<ResearchSource>,
    #[serde(default = "default_max_results_per_query")]
    pub max_results_per_query: usize,
    #[serde(default)]
    pub fetch_content: bool,
    #[serde(default = "default_dedupe_days")]
    pub dedupe_days: u32,
    #[serde(default = "default_true")]
    pub summarize: bool,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<ResearchNotifyConfig>,
    #[serde(default)]
    pub concurrency_policy: ResearchConcurrencyPolicy,
    #[serde(default)]
    pub retry: ResearchRetryPolicy,
}

fn default_dedupe_days() -> u32 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResearchTaskTrigger {
    Cron { expr: String, timezone: String },
    Interval { every_ms: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResearchNotifyConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default)]
    pub include_summary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResearchConcurrencyPolicy {
    #[default]
    SkipIfRunning,
    Allow,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchRetryPolicy {
    #[serde(default = "default_retry_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_retry_backoff_ms")]
    pub backoff_ms: u64,
}

impl Default for ResearchRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: default_retry_max_attempts(),
            backoff_ms: default_retry_backoff_ms(),
        }
    }
}

fn default_retry_max_attempts() -> u32 {
    2
}

fn default_retry_backoff_ms() -> u64 {
    1_000
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ResearchSource {
    Web,
    Wechat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Day,
    Week,
    Month,
    Year,
    Any,
}
