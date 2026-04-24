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
            has_rule_hit: None,
            is_websocket: None,
            is_sse: None,
            is_tunnel: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TrafficGetArgs {
    pub id: String,
    #[serde(default)]
    pub request_body: bool,
    #[serde(default)]
    pub response_body: bool,
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
}
