use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandCapability {
    Readonly,
    Mutating,
    SensitiveBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrafficListDirection {
    #[default]
    Backward,
    Forward,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SearchArgs {
    #[serde(default)]
    pub keyword: String,
    #[serde(default)]
    pub scope: SearchScope,
    #[serde(default)]
    pub filters: SearchFilters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_scan: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_range: Option<TimeRange>,
    #[serde(default, skip_serializing_if = "SearchInclude::is_default")]
    pub include: SearchInclude,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SearchInclude {
    #[serde(default)]
    pub request_body: bool,
    #[serde(default)]
    pub response_body: bool,
    #[serde(default)]
    pub request_headers: bool,
    #[serde(default)]
    pub response_headers: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_bytes: Option<usize>,
}

impl SearchInclude {
    pub fn is_default(&self) -> bool {
        !self.request_body
            && !self.response_body
            && !self.request_headers
            && !self.response_headers
            && self.max_body_bytes.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TimeRange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SearchFilters {
    #[serde(default)]
    pub protocols: Vec<String>,
    #[serde(default)]
    pub status_ranges: Vec<String>,
    #[serde(default)]
    pub content_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterCondition {
    pub field: String,
    pub operator: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficListArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<u64>,
    #[serde(default)]
    pub direction: TrafficListDirection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_min: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_max: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_rule_hit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_websocket: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_sse: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_tunnel: Option<bool>,
}

impl Default for TrafficListArgs {
    fn default() -> Self {
        Self {
            limit: None,
            cursor: None,
            direction: TrafficListDirection::Backward,
            method: None,
            status: None,
            status_min: None,
            status_max: None,
            protocol: None,
            host: None,
            url: None,
            path: None,
            content_type: None,
            client_ip: None,
            client_app: None,
            listener_port: None,
            has_rule_hit: None,
            is_websocket: None,
            is_sse: None,
            is_tunnel: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TrafficGetArgs {
    /// Single record id; empty when `ids` is provided (batch mode).
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub request_body: bool,
    #[serde(default)]
    pub response_body: bool,
    /// Batch mode: when non-empty the call returns one detail per id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<String>,
    /// Optional per-body truncation in bytes (applied to batch responses).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_bytes: Option<usize>,
    /// Batch mode: include request/response headers in each entry.
    #[serde(default)]
    pub request_headers: bool,
    #[serde(default)]
    pub response_headers: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TrafficClearArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "args", rename_all = "snake_case")]
pub enum CanonicalQueryCommand {
    Search(SearchArgs),
    TrafficList(TrafficListArgs),
    TrafficGet(TrafficGetArgs),
    TrafficClear(TrafficClearArgs),
}

impl CanonicalQueryCommand {
    pub fn command_id(&self) -> &'static str {
        match self {
            Self::Search(_) => "search.stream",
            Self::TrafficList(_) => "traffic.list",
            Self::TrafficGet(_) => "traffic.get",
            Self::TrafficClear(_) => "traffic.clear",
        }
    }

    pub fn capability(&self) -> CommandCapability {
        match self {
            Self::Search(_) | Self::TrafficList(_) => CommandCapability::Readonly,
            Self::TrafficGet(args) => {
                if args.request_body || args.response_body {
                    CommandCapability::SensitiveBody
                } else {
                    CommandCapability::Readonly
                }
            }
            Self::TrafficClear(_) => CommandCapability::Mutating,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_command_descriptor() {
        let command = CanonicalQueryCommand::Search(SearchArgs::default());
        assert_eq!(command.command_id(), "search.stream");
        assert_eq!(command.capability(), CommandCapability::Readonly);
    }

    #[test]
    fn test_traffic_get_with_body_is_sensitive() {
        let command = CanonicalQueryCommand::TrafficGet(TrafficGetArgs {
            id: "REQ-1".to_string(),
            request_body: true,
            response_body: false,
            ..Default::default()
        });
        assert_eq!(command.capability(), CommandCapability::SensitiveBody);
    }

    #[test]
    fn test_canonical_query_command_roundtrip() {
        let command = CanonicalQueryCommand::TrafficList(TrafficListArgs {
            limit: Some(20),
            direction: TrafficListDirection::Forward,
            host: Some("example.com".to_string()),
            ..TrafficListArgs::default()
        });
        let encoded = serde_json::to_string(&command).expect("encode");
        let decoded: CanonicalQueryCommand = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(decoded, command);
    }

    #[test]
    fn test_search_scope_default_enables_all() {
        // Exercises `default_true` and the manual `Default` impl.
        let scope = SearchScope::default();
        assert!(scope.all);
        assert!(!scope.request_body);
        assert!(!scope.response_body);
        assert!(!scope.request_headers);
        assert!(!scope.response_headers);
        assert!(!scope.url);
        assert!(!scope.websocket_messages);
        assert!(!scope.sse_events);
    }

    #[test]
    fn test_search_filters_constraints() {
        // Empty filters have no constraints.
        let empty = SearchFilters::default();
        assert!(!empty.has_constraints());

        // Each constraint-bearing field independently flips `has_constraints`.
        let with_protocols = SearchFilters {
            protocols: vec!["http".to_string()],
            ..SearchFilters::default()
        };
        assert!(with_protocols.has_constraints());

        let with_status = SearchFilters {
            status_ranges: vec!["2xx".to_string()],
            ..SearchFilters::default()
        };
        assert!(with_status.has_constraints());

        let with_content_type = SearchFilters {
            content_types: vec!["application/json".to_string()],
            ..SearchFilters::default()
        };
        assert!(with_content_type.has_constraints());

        let with_rule_hit = SearchFilters {
            has_rule_hit: Some(true),
            ..SearchFilters::default()
        };
        assert!(with_rule_hit.has_constraints());

        let with_conditions = SearchFilters {
            conditions: vec![FilterCondition {
                field: "host".to_string(),
                operator: "eq".to_string(),
                value: "example.com".to_string(),
            }],
            ..SearchFilters::default()
        };
        assert!(with_conditions.has_constraints());

        let with_client_ips = SearchFilters {
            client_ips: vec!["127.0.0.1".to_string()],
            ..SearchFilters::default()
        };
        assert!(with_client_ips.has_constraints());

        let with_client_apps = SearchFilters {
            client_apps: vec!["curl".to_string()],
            ..SearchFilters::default()
        };
        assert!(with_client_apps.has_constraints());

        let with_domains = SearchFilters {
            domains: vec!["example.com".to_string()],
            ..SearchFilters::default()
        };
        assert!(with_domains.has_constraints());
    }

    #[test]
    fn test_command_id_for_all_variants() {
        assert_eq!(
            CanonicalQueryCommand::Search(SearchArgs::default()).command_id(),
            "search.stream"
        );
        assert_eq!(
            CanonicalQueryCommand::TrafficList(TrafficListArgs::default()).command_id(),
            "traffic.list"
        );
        assert_eq!(
            CanonicalQueryCommand::TrafficGet(TrafficGetArgs::default()).command_id(),
            "traffic.get"
        );
        assert_eq!(
            CanonicalQueryCommand::TrafficClear(TrafficClearArgs::default()).command_id(),
            "traffic.clear"
        );
    }

    #[test]
    fn test_capability_for_all_variants() {
        // Readonly variants.
        assert_eq!(
            CanonicalQueryCommand::Search(SearchArgs::default()).capability(),
            CommandCapability::Readonly
        );
        assert_eq!(
            CanonicalQueryCommand::TrafficList(TrafficListArgs::default()).capability(),
            CommandCapability::Readonly
        );

        // TrafficGet without bodies is Readonly (the `else` branch).
        assert_eq!(
            CanonicalQueryCommand::TrafficGet(TrafficGetArgs::default()).capability(),
            CommandCapability::Readonly
        );

        // TrafficGet with response body is SensitiveBody.
        assert_eq!(
            CanonicalQueryCommand::TrafficGet(TrafficGetArgs {
                id: "REQ-2".to_string(),
                request_body: false,
                response_body: true,
                ..Default::default()
            })
            .capability(),
            CommandCapability::SensitiveBody
        );

        // TrafficClear is Mutating.
        assert_eq!(
            CanonicalQueryCommand::TrafficClear(TrafficClearArgs::default()).capability(),
            CommandCapability::Mutating
        );
    }
}
