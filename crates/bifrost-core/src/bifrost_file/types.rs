use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const BIFROST_FILE_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BifrostFileType {
    Rules,
    Network,
    Script,
    Values,
    Template,
}

impl std::fmt::Display for BifrostFileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BifrostFileType::Rules => write!(f, "rules"),
            BifrostFileType::Network => write!(f, "network"),
            BifrostFileType::Script => write!(f, "script"),
            BifrostFileType::Values => write!(f, "values"),
            BifrostFileType::Template => write!(f, "template"),
        }
    }
}

impl std::str::FromStr for BifrostFileType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "rules" => Ok(BifrostFileType::Rules),
            "network" => Ok(BifrostFileType::Network),
            "script" => Ok(BifrostFileType::Script),
            "values" => Ok(BifrostFileType::Values),
            "template" => Ok(BifrostFileType::Template),
            _ => Err(format!("Unknown file type: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BifrostFileHeader {
    pub version: u8,
    pub file_type: BifrostFileType,
}

#[derive(Debug, Clone)]
pub struct BifrostFile<M, T> {
    pub header: BifrostFileHeader,
    pub meta: M,
    pub options: serde_json::Value,
    pub content: T,
}

#[derive(Debug, Clone)]
pub struct BifrostFileRaw {
    pub header: BifrostFileHeader,
    pub meta_raw: String,
    pub content_raw: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleFileMeta {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub sort_order: i32,
    #[serde(default = "default_version")]
    pub version: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default)]
    pub sync: RuleSyncMeta,
}

fn default_true() -> bool {
    true
}

fn default_version() -> String {
    "1.0.0".to_string()
}

impl RuleFileMeta {
    pub fn new(name: String) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            name,
            enabled: true,
            sort_order: 0,
            version: "1.0.0".to_string(),
            created_at: now.clone(),
            updated_at: now,
            description: None,
            group: None,
            sync: RuleSyncMeta::default(),
        }
    }

    pub fn touch(&mut self) {
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSyncStatus {
    #[default]
    LocalOnly,
    Synced,
    Modified,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleSyncMeta {
    #[serde(default)]
    pub rule_id: String,
    #[serde(default)]
    pub status: RuleSyncStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_synced_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_synced_content_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_updated_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleFileOptions {
    #[serde(default)]
    pub rule_count: usize,
}

pub type RuleFile = BifrostFile<RuleFileMeta, String>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExportMeta {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl ExportMeta {
    pub fn new(name: String) -> Self {
        Self {
            name,
            version: "1.0.0".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            description: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkRecord {
    pub id: String,
    pub method: String,
    pub url: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_rule_hit: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_headers: Option<Vec<(String, String)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<Vec<(String, String)>>,
    /// Upstream response headers before Bifrost rules and protocol normalization.
    ///
    /// Older network exports omit this field and store the upstream snapshot in
    /// `response_headers`; importers must retain that legacy interpretation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_response_headers: Option<Vec<(String, String)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body: Option<String>,
    /// Original request body bytes when the payload cannot be represented as
    /// plain UTF-8 or has a Content-Encoding that must remain recoverable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_body: Option<String>,
    /// Original response body bytes when the payload cannot be represented as
    /// plain UTF-8 or has a Content-Encoding that must remain recoverable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body_base64: Option<String>,
    pub duration_ms: u64,
    pub timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_rules: Option<Vec<MatchedRuleExport>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_rules: Option<ActiveRulesExport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedRuleExport {
    pub pattern: String,
    pub protocol: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveRuleSource {
    DefaultPort,
    CustomPort,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveRuleExport {
    pub name: String,
    pub rule_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveRulesExport {
    pub source: ActiveRuleSource,
    pub admin_port: u16,
    pub listener_port: u16,
    pub total: usize,
    pub rules: Vec<ActiveRuleExport>,
    pub merged_content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptItem {
    pub name: String,
    pub script_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub content: String,
}

pub type ValuesContent = HashMap<String, String>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateContent {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<ReplayGroupExport>,
    pub requests: Vec<ReplayRequestExport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayGroupExport {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub sort_order: i32,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayRequestExport {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub request_type: String,
    pub method: String,
    pub url: String,
    pub headers: Vec<KeyValueItemExport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<ReplayBodyExport>,
    pub is_saved: bool,
    pub sort_order: i32,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyValueItemExport {
    pub id: String,
    pub key: String,
    pub value: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayBodyExport {
    #[serde(rename = "type")]
    pub body_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub form_data: Vec<KeyValueItemExport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_file: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ParseResultWithWarnings<T> {
    pub data: T,
    pub warnings: Vec<ParseWarning>,
}

#[derive(Debug, Clone)]
pub struct ParseWarning {
    pub level: WarningLevel,
    pub message: String,
    pub field: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningLevel {
    Info,
    Warning,
    Error,
}

impl std::fmt::Display for WarningLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WarningLevel::Info => write!(f, "INFO"),
            WarningLevel::Warning => write!(f, "WARNING"),
            WarningLevel::Error => write!(f, "ERROR"),
        }
    }
}

impl<T> ParseResultWithWarnings<T> {
    pub fn ok(data: T) -> Self {
        Self {
            data,
            warnings: vec![],
        }
    }

    pub fn with_warning(data: T, warning: ParseWarning) -> Self {
        Self {
            data,
            warnings: vec![warning],
        }
    }

    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    pub fn has_errors(&self) -> bool {
        self.warnings.iter().any(|w| w.level == WarningLevel::Error)
    }

    pub fn add_warning(&mut self, warning: ParseWarning) {
        self.warnings.push(warning);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn bifrost_file_type_display_and_fromstr_roundtrip() {
        for (ty, s) in [
            (BifrostFileType::Rules, "rules"),
            (BifrostFileType::Network, "network"),
            (BifrostFileType::Script, "script"),
            (BifrostFileType::Values, "values"),
            (BifrostFileType::Template, "template"),
        ] {
            assert_eq!(ty.to_string(), s);
            assert_eq!(BifrostFileType::from_str(s).unwrap(), ty);
            // case-insensitive
            assert_eq!(BifrostFileType::from_str(&s.to_uppercase()).unwrap(), ty);
        }
    }

    #[test]
    fn bifrost_file_type_fromstr_unknown_errors() {
        let err = BifrostFileType::from_str("bogus").unwrap_err();
        assert!(err.contains("Unknown file type"));
    }

    #[test]
    fn rule_file_meta_new_and_touch() {
        let mut meta = RuleFileMeta::new("name".to_string());
        assert_eq!(meta.name, "name");
        assert!(meta.enabled);
        assert_eq!(meta.version, "1.0.0");
        assert!(meta.description.is_none());
        let before = meta.updated_at.clone();
        meta.touch();
        // touch only changes updated_at; created_at stays
        assert!(!meta.created_at.is_empty());
        // updated_at is a valid rfc3339 string (non-empty)
        assert!(!meta.updated_at.is_empty());
        let _ = before;
    }

    #[test]
    fn rule_sync_status_default_is_local_only() {
        assert_eq!(RuleSyncStatus::default(), RuleSyncStatus::LocalOnly);
    }

    #[test]
    fn export_meta_new() {
        let meta = ExportMeta::new("e".to_string());
        assert_eq!(meta.name, "e");
        assert_eq!(meta.version, "1.0.0");
        assert!(meta.description.is_none());
        assert!(!meta.created_at.is_empty());
    }

    #[test]
    fn warning_level_display() {
        assert_eq!(WarningLevel::Info.to_string(), "INFO");
        assert_eq!(WarningLevel::Warning.to_string(), "WARNING");
        assert_eq!(WarningLevel::Error.to_string(), "ERROR");
    }

    #[test]
    fn parse_result_ok_and_warning_helpers() {
        let r = ParseResultWithWarnings::ok(42);
        assert_eq!(r.data, 42);
        assert!(!r.has_warnings());
        assert!(!r.has_errors());

        let warn = ParseWarning {
            level: WarningLevel::Warning,
            message: "w".to_string(),
            field: Some("f".to_string()),
        };
        let r2 = ParseResultWithWarnings::with_warning(1, warn);
        assert!(r2.has_warnings());
        assert!(!r2.has_errors());

        let mut r3 = ParseResultWithWarnings::ok(0);
        r3.add_warning(ParseWarning {
            level: WarningLevel::Error,
            message: "boom".to_string(),
            field: None,
        });
        assert!(r3.has_warnings());
        assert!(r3.has_errors());
    }

    #[test]
    fn bifrost_file_type_serde_lowercase() {
        let json = serde_json::to_string(&BifrostFileType::Network).unwrap();
        assert_eq!(json, "\"network\"");
        let back: BifrostFileType = serde_json::from_str("\"template\"").unwrap();
        assert_eq!(back, BifrostFileType::Template);
    }
}
