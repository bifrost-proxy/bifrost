use serde::{Deserialize, Serialize};

use crate::body_store::BodyRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameDirection {
    Send,
    Receive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameType {
    Text,
    Binary,
    Ping,
    Pong,
    Close,
    Continuation,
    Sse,
}

impl FrameType {
    pub fn from_opcode(opcode: u8) -> Self {
        match opcode {
            0x0 => FrameType::Continuation,
            0x1 => FrameType::Text,
            0x2 => FrameType::Binary,
            0x8 => FrameType::Close,
            0x9 => FrameType::Ping,
            0xA => FrameType::Pong,
            _ => FrameType::Binary,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocketStatus {
    pub is_open: bool,
    pub send_count: u64,
    pub receive_count: u64,
    pub send_bytes: u64,
    pub receive_bytes: u64,
    #[serde(default)]
    pub frame_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close_reason: Option<String>,
}

impl Default for SocketStatus {
    fn default() -> Self {
        Self {
            is_open: true,
            send_count: 0,
            receive_count: 0,
            send_bytes: 0,
            receive_bytes: 0,
            frame_count: 0,
            close_code: None,
            close_reason: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedRule {
    pub pattern: String,
    pub protocol: String,
    pub value: String,
    pub rule_name: Option<String>,
    pub raw: Option<String>,
    pub line: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RequestTiming {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_byte_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receive_ms: Option<u64>,
    pub total_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficRecord {
    pub id: String,
    #[serde(default)]
    pub sequence: u64,
    pub timestamp: u64,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub request_size: usize,
    pub response_size: usize,
    #[serde(default)]
    pub upload_bytes: usize,
    #[serde(default)]
    pub download_bytes: usize,
    pub duration_ms: u64,
    #[serde(default)]
    pub listener_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<RequestTiming>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_headers: Option<Vec<(String, String)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_response_headers: Option<Vec<(String, String)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body_ref: Option<BodyRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_body_ref: Option<BodyRef>,
    #[serde(default)]
    pub derived_response_body_ref: Option<BodyRef>,
    /// 原始（未 decode）请求体引用：用于 decode 失败回溯或对比。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_request_body_ref: Option<BodyRef>,
    /// 原始（未 decode）响应体引用：用于 decode 失败回溯或对比。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_response_body_ref: Option<BodyRef>,
    pub client_ip: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_path: Option<String>,
    pub host: String,
    pub path: String,
    pub protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_request_headers: Option<Vec<(String, String)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<Vec<(String, String)>>,
    #[serde(default)]
    pub is_tunnel: bool,
    #[serde(default)]
    pub has_rule_hit: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_rules: Option<Vec<MatchedRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_content_type: Option<String>,
    #[serde(default)]
    pub is_websocket: bool,
    #[serde(default)]
    pub is_sse: bool,
    #[serde(default)]
    pub is_h3: bool,
    #[serde(default)]
    pub is_replay: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket_status: Option<SocketStatus>,
    #[serde(default)]
    pub frame_count: usize,
    #[serde(default)]
    pub last_frame_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub req_script_results: Option<Vec<ScriptExecutionResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub res_script_results: Option<Vec<ScriptExecutionResult>>,
    /// decode（落库前解码）脚本执行结果：请求阶段
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decode_req_script_results: Option<Vec<ScriptExecutionResult>>,
    /// decode（落库前解码）脚本执行结果：响应阶段
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decode_res_script_results: Option<Vec<ScriptExecutionResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub devtools_client_req_id: Option<String>,
}

pub use bifrost_script::ScriptExecutionResult;

impl TrafficRecord {
    pub fn new(id: String, method: String, url: String) -> Self {
        let parsed_url = url::Url::parse(&url).ok();
        let host = parsed_url
            .as_ref()
            .and_then(|u| u.host_str())
            .unwrap_or("")
            .to_string();
        let path = parsed_url
            .as_ref()
            .map(|u| u.path().to_string())
            .unwrap_or_default();
        let protocol = parsed_url
            .as_ref()
            .map(|u| u.scheme().to_string())
            .unwrap_or_default();

        Self {
            id,
            sequence: 0,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            method,
            url,
            status: 0,
            content_type: None,
            request_size: 0,
            response_size: 0,
            upload_bytes: 0,
            download_bytes: 0,
            duration_ms: 0,
            listener_port: 0,
            timing: None,
            request_headers: None,
            original_response_headers: None,
            request_body_ref: None,
            response_body_ref: None,
            derived_response_body_ref: None,
            raw_request_body_ref: None,
            raw_response_body_ref: None,
            client_ip: String::new(),
            client_app: None,
            client_pid: None,
            client_path: None,
            host,
            path,
            protocol,
            actual_url: None,
            actual_host: None,
            original_request_headers: None,
            response_headers: None,
            is_tunnel: false,
            has_rule_hit: false,
            matched_rules: None,
            request_content_type: None,
            is_websocket: false,
            is_sse: false,
            is_h3: false,
            is_replay: false,
            socket_status: None,
            frame_count: 0,
            last_frame_id: 0,
            error_message: None,
            req_script_results: None,
            res_script_results: None,
            decode_req_script_results: None,
            decode_res_script_results: None,
            devtools_client_req_id: None,
        }
    }

    pub fn set_h3(&mut self) {
        self.is_h3 = true;
    }

    pub fn set_websocket(&mut self) {
        self.is_websocket = true;
        self.socket_status = Some(SocketStatus::default());
    }

    pub fn set_sse(&mut self) {
        self.is_sse = true;
        self.socket_status = Some(SocketStatus::default());
    }

    pub fn set_streaming(&mut self) {
        self.socket_status = Some(SocketStatus::default());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficSummary {
    pub id: String,
    #[serde(default)]
    pub sequence: u64,
    pub timestamp: u64,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub request_size: usize,
    pub response_size: usize,
    pub duration_ms: u64,
    #[serde(default)]
    pub listener_port: u16,
    pub host: String,
    pub path: String,
    pub protocol: String,
    pub client_ip: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_path: Option<String>,
    pub has_rule_hit: bool,
    pub matched_rule_count: usize,
    pub matched_protocols: Vec<String>,
    #[serde(default)]
    pub is_websocket: bool,
    #[serde(default)]
    pub is_sse: bool,
    #[serde(default)]
    pub is_h3: bool,
    #[serde(default)]
    pub is_tunnel: bool,
    #[serde(default)]
    pub frame_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket_status: Option<SocketStatus>,
    pub start_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<String>,
}

fn format_timestamp_ms(timestamp_ms: u64) -> String {
    use chrono::{Local, TimeZone};
    let secs = (timestamp_ms / 1000) as i64;
    let nanos = ((timestamp_ms % 1000) * 1_000_000) as u32;
    Local
        .timestamp_opt(secs, nanos)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
        .unwrap_or_else(|| "-".to_string())
}

impl From<&TrafficRecord> for TrafficSummary {
    fn from(record: &TrafficRecord) -> Self {
        let (matched_rule_count, matched_protocols) = if let Some(ref rules) = record.matched_rules
        {
            let protocols: Vec<String> = rules
                .iter()
                .map(|r| r.protocol.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            (rules.len(), protocols)
        } else {
            (0, Vec::new())
        };

        let start_time = format_timestamp_ms(record.timestamp);
        let end_time = if record.duration_ms > 0 {
            Some(format_timestamp_ms(record.timestamp + record.duration_ms))
        } else {
            None
        };

        Self {
            id: record.id.clone(),
            sequence: record.sequence,
            timestamp: record.timestamp,
            method: record.method.clone(),
            url: record.url.clone(),
            status: record.status,
            content_type: record.content_type.clone(),
            request_size: record.request_size,
            response_size: record.response_size,
            duration_ms: record.duration_ms,
            listener_port: record.listener_port,
            host: record.host.clone(),
            path: record.path.clone(),
            protocol: record.protocol.clone(),
            client_ip: record.client_ip.clone(),
            client_app: record.client_app.clone(),
            client_pid: record.client_pid,
            client_path: record.client_path.clone(),
            has_rule_hit: record.has_rule_hit,
            matched_rule_count,
            matched_protocols,
            is_websocket: record.is_websocket,
            is_sse: record.is_sse,
            is_h3: record.is_h3,
            is_tunnel: record.is_tunnel,
            frame_count: record.frame_count,
            socket_status: record.socket_status.clone(),
            start_time,
            end_time,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn test_traffic_record_new() {
        let record = TrafficRecord::new(
            "test-id".to_string(),
            "GET".to_string(),
            "https://example.com/api/test".to_string(),
        );

        assert_eq!(record.id, "test-id");
        assert_eq!(record.method, "GET");
        assert_eq!(record.host, "example.com");
        assert_eq!(record.path, "/api/test");
        assert_eq!(record.protocol, "https");
        assert_eq!(record.listener_port, 0);
    }

    #[test]
    fn test_traffic_summary_includes_sequence() {
        let mut record = TrafficRecord::new(
            "test-id".to_string(),
            "GET".to_string(),
            "https://example.com".to_string(),
        );
        record.sequence = 42;
        record.listener_port = 18888;
        let summary = TrafficSummary::from(&record);
        assert_eq!(summary.sequence, 42);
        assert_eq!(summary.listener_port, 18888);
    }

    #[test]
    fn derived_response_body_ref_is_present_in_serialized_detail() {
        let record = TrafficRecord::new(
            "REQ-test".to_string(),
            "GET".to_string(),
            "http://example.com/sse".to_string(),
        );

        let value = serde_json::to_value(&record).expect("serialize traffic record");
        assert!(matches!(value, Value::Object(_)));
        assert!(value.get("derived_response_body_ref").is_some());
        assert!(value["derived_response_body_ref"].is_null());
    }
}
