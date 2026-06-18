use serde::{Deserialize, Serialize};

use crate::traffic_db::TrafficSummaryCompact;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SearchRequest {
    pub keyword: String,

    #[serde(default)]
    pub scope: SearchScope,

    #[serde(default)]
    pub filters: SearchFilters,

    pub cursor: Option<u64>,
    pub limit: Option<usize>,
    pub max_scan: Option<usize>,
    pub max_results: Option<usize>,

    #[serde(default)]
    pub time_range: Option<TimeRange>,

    /// Optional per-result attachments (body/headers) requested by the client.
    /// Kept off by default so existing callers continue to receive lean responses.
    #[serde(default)]
    pub include: SearchInclude,
}

/// Toggles for attaching bodies / headers to each `SearchResultItem`.
///
/// Backward compatible: when no toggles are set the engine skips body/header
/// hydration entirely. `max_body_bytes` defaults to 65536 (64 KiB) per body and
/// applies independently to request and response bodies.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SearchInclude {
    #[serde(default)]
    pub request_body: bool,
    #[serde(default)]
    pub response_body: bool,
    #[serde(default)]
    pub request_headers: bool,
    #[serde(default)]
    pub response_headers: bool,
    /// Maximum bytes returned per body; missing means use the engine default.
    #[serde(default)]
    pub max_body_bytes: Option<usize>,
}

impl SearchInclude {
    pub const DEFAULT_MAX_BODY_BYTES: usize = 64 * 1024;

    /// True when at least one body/header attachment is requested.
    pub fn any(&self) -> bool {
        self.request_body || self.response_body || self.request_headers || self.response_headers
    }

    pub fn body_limit(&self) -> usize {
        self.max_body_bytes.unwrap_or(Self::DEFAULT_MAX_BODY_BYTES)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TimeRange {
    #[serde(default)]
    pub since_ms: Option<i64>,
    #[serde(default)]
    pub until_ms: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchScope {
    #[serde(default)]
    pub request_body: bool,
    #[serde(default)]
    pub response_body: bool,
    #[serde(default)]
    pub request_headers: bool,
    #[serde(default)]
    pub response_headers: bool,
    #[serde(default)]
    pub url: bool,
    #[serde(default)]
    pub websocket_messages: bool,
    #[serde(default)]
    pub sse_events: bool,
    #[serde(default = "default_true")]
    pub all: bool,
}

fn default_true() -> bool {
    true
}

impl Default for SearchScope {
    fn default() -> Self {
        Self {
            request_body: false,
            response_body: false,
            request_headers: false,
            response_headers: false,
            url: false,
            websocket_messages: false,
            sse_events: false,
            all: true,
        }
    }
}

impl SearchScope {
    pub fn should_search_url(&self) -> bool {
        self.all || self.url
    }

    pub fn should_search_request_headers(&self) -> bool {
        self.all || self.request_headers
    }

    pub fn should_search_response_headers(&self) -> bool {
        self.all || self.response_headers
    }

    pub fn should_search_request_body(&self) -> bool {
        self.all || self.request_body
    }

    pub fn should_search_response_body(&self) -> bool {
        self.all || self.response_body
    }

    pub fn should_search_websocket_messages(&self) -> bool {
        self.all || self.websocket_messages
    }

    pub fn should_search_sse_events(&self) -> bool {
        self.all || self.sse_events
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SearchFilters {
    #[serde(default)]
    pub protocols: Vec<String>,
    #[serde(default)]
    pub status_ranges: Vec<String>,
    #[serde(default)]
    pub content_types: Vec<String>,
    pub has_rule_hit: Option<bool>,

    #[serde(default)]
    pub conditions: Vec<FilterCondition>,

    #[serde(default)]
    pub client_ips: Vec<String>,
    #[serde(default)]
    pub client_apps: Vec<String>,
    #[serde(default)]
    pub domains: Vec<String>,
}

impl SearchFilters {
    pub fn has_constraints(&self) -> bool {
        !self.protocols.is_empty()
            || !self.status_ranges.is_empty()
            || !self.content_types.is_empty()
            || self.has_rule_hit.is_some()
            || !self.conditions.is_empty()
            || !self.client_ips.is_empty()
            || !self.client_apps.is_empty()
            || !self.domains.is_empty()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FilterCondition {
    pub field: String,
    pub operator: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResultItem>,
    pub total_searched: usize,
    pub total_matched: usize,
    pub next_cursor: Option<u64>,
    pub has_more: bool,
    pub search_id: String,
    #[serde(default)]
    pub searched_range: SearchedRange,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SearchedRange {
    pub oldest_ts_ms: Option<i64>,
    pub newest_ts_ms: Option<i64>,
    pub scanned_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResultItem {
    pub record: TrafficSummaryCompact,
    pub matches: Vec<MatchLocation>,
    /// Bodies attached when `SearchInclude::{request_body,response_body}` is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bodies: Option<BodiesPayload>,
    /// Headers attached when `SearchInclude::{request_headers,response_headers}` is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HeadersPayload>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BodiesPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<BodyChunk>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<BodyChunk>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BodyChunk {
    /// Base64 (STANDARD) encoded bytes — binary-safe transport.
    pub bytes_b64: String,
    /// Original (pre-truncation) size in bytes, or post-truncation if unknown.
    pub size: usize,
    /// True when bytes were truncated to `max_body_bytes`.
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct HeadersPayload {
    #[serde(default)]
    pub request: Vec<(String, String)>,
    #[serde(default)]
    pub response: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatchLocation {
    pub field: String,
    pub preview: String,
    pub offset: usize,
}

#[cfg(test)]
mod include_serde_tests {
    use super::*;

    #[test]
    fn search_request_deserializes_without_include_field() {
        // Legacy callers that predate the include block must still work.
        let json = serde_json::json!({
            "keyword": "foo",
            "scope": {"all": true},
            "filters": {},
            "cursor": null,
            "limit": 10,
            "max_scan": null,
            "max_results": null,
            "time_range": null
        });
        let req: SearchRequest = serde_json::from_value(json).expect("deserialize");
        assert!(!req.include.any());
        assert_eq!(
            req.include.body_limit(),
            SearchInclude::DEFAULT_MAX_BODY_BYTES
        );
    }

    #[test]
    fn search_include_deserializes_partial_fields() {
        let json = serde_json::json!({"response_body": true, "max_body_bytes": 1024});
        let inc: SearchInclude = serde_json::from_value(json).expect("deserialize");
        assert!(inc.response_body);
        assert!(!inc.request_body);
        assert_eq!(inc.body_limit(), 1024);
        assert!(inc.any());
    }
}
