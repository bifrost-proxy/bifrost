use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ScriptType {
    Request,
    Response,
    Decode,
    Parser,
}

impl std::fmt::Display for ScriptType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptType::Request => write!(f, "request"),
            ScriptType::Response => write!(f, "response"),
            ScriptType::Decode => write!(f, "decode"),
            ScriptType::Parser => write!(f, "parser"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ScriptLogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl std::fmt::Display for ScriptLogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptLogLevel::Debug => write!(f, "DEBUG"),
            ScriptLogLevel::Info => write!(f, "INFO"),
            ScriptLogLevel::Warn => write!(f, "WARN"),
            ScriptLogLevel::Error => write!(f, "ERROR"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptLogEntry {
    pub timestamp: u64,
    pub level: ScriptLogLevel,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptExecutionResult {
    pub script_name: String,
    pub script_type: ScriptType,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub duration_ms: u64,
    pub logs: Vec<ScriptLogEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_modifications: Option<TestRequestModifications>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_modifications: Option<TestResponseModifications>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decode_output: Option<DecodeOutput>,
}

/// decode 脚本的标准输出结构：
/// - `code == "0"` 表示成功，此时 `data` 作为解码后的文本
/// - 失败时 `msg` 为错误信息（前端用于展示原因）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodeOutput {
    pub data: String,
    pub code: String,
    pub msg: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRequestModifications {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResponseModifications {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedRuleInfo {
    pub pattern: String,
    pub protocol: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct ScriptContext {
    pub request_id: String,
    pub script_name: String,
    pub script_type: ScriptType,
    pub values: HashMap<String, String>,
    pub matched_rules: Vec<MatchedRuleInfo>,
}

#[derive(Debug, Clone, Default)]
pub struct RequestData {
    pub url: String,
    pub method: String,
    pub host: String,
    pub path: String,
    pub protocol: String,
    pub client_ip: String,
    pub client_app: Option<String>,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ResponseData {
    pub status: u16,
    pub status_text: String,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
    pub request: RequestData,
}

#[derive(Debug, Clone, Default)]
pub struct RequestModifications {
    pub method: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ResponseModifications {
    pub status: Option<u16>,
    pub status_text: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptInfo {
    pub name: String,
    pub script_type: ScriptType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptDetail {
    #[serde(flatten)]
    pub info: ScriptInfo,
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_type_display_and_serde() {
        let cases = [
            (ScriptType::Request, "request"),
            (ScriptType::Response, "response"),
            (ScriptType::Decode, "decode"),
            (ScriptType::Parser, "parser"),
        ];
        for (ty, expected) in cases {
            assert_eq!(ty.to_string(), expected);
            let json = serde_json::to_string(&ty).unwrap();
            assert_eq!(json.trim_matches('"'), expected);
            let de: ScriptType = serde_json::from_str(&json).unwrap();
            assert_eq!(de, ty);
        }
    }

    #[test]
    fn script_log_level_display_and_serde() {
        let cases = [
            (ScriptLogLevel::Debug, "DEBUG", "debug"),
            (ScriptLogLevel::Info, "INFO", "info"),
            (ScriptLogLevel::Warn, "WARN", "warn"),
            (ScriptLogLevel::Error, "ERROR", "error"),
        ];
        for (level, display, json_value) in cases {
            assert_eq!(level.to_string(), display);
            let json = serde_json::to_string(&level).unwrap();
            assert_eq!(json.trim_matches('"'), json_value);
            let de: ScriptLogLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(de, level);
        }
    }
}
