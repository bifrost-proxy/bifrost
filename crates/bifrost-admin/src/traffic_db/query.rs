use serde::{Deserialize, Serialize};

use super::types::{TrafficFlags, TrafficSummaryCompact};

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[default]
    Backward,
    Forward,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextMatchMode {
    #[default]
    Contains,
    Equals,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct QueryParams {
    pub cursor: Option<u64>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub direction: Direction,

    pub method: Option<String>,
    pub status: Option<u16>,
    pub status_min: Option<u16>,
    pub status_max: Option<u16>,
    pub protocol: Option<String>,
    pub has_rule_hit: Option<bool>,
    pub is_websocket: Option<bool>,
    pub is_sse: Option<bool>,
    pub is_h3: Option<bool>,
    pub is_tunnel: Option<bool>,

    pub host_contains: Option<String>,
    pub url_contains: Option<String>,
    pub path_contains: Option<String>,
    pub client_app: Option<String>,
    #[serde(default)]
    pub client_app_match: TextMatchMode,
    pub client_app_empty: Option<bool>,
    pub client_ip: Option<String>,
    #[serde(default)]
    pub client_ip_match: TextMatchMode,
    pub client_ip_empty: Option<bool>,
    pub listener_port: Option<u16>,
    pub content_type: Option<String>,

    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,

    pub pending_ids: Option<Vec<String>>,
}

impl QueryParams {
    pub fn has_filters(&self) -> bool {
        self.method.is_some()
            || self.status.is_some()
            || self.status_min.is_some()
            || self.status_max.is_some()
            || self.protocol.is_some()
            || self.has_rule_hit.is_some()
            || self.is_websocket.is_some()
            || self.is_sse.is_some()
            || self.is_h3.is_some()
            || self.is_tunnel.is_some()
            || self.host_contains.is_some()
            || self.url_contains.is_some()
            || self.path_contains.is_some()
            || self.client_app.is_some()
            || self.client_app_empty.is_some()
            || self.client_ip.is_some()
            || self.client_ip_empty.is_some()
            || self.listener_port.is_some()
            || self.content_type.is_some()
            || self.since_ms.is_some()
            || self.until_ms.is_some()
    }

    pub fn build_where_clause(&self) -> (String, Vec<QueryValue>) {
        let mut conditions = Vec::new();
        let mut params: Vec<QueryValue> = Vec::new();

        if let Some(cursor) = self.cursor {
            match self.direction {
                Direction::Forward => {
                    conditions.push("sequence > ?".to_string());
                    params.push(QueryValue::Int(cursor as i64));
                }
                Direction::Backward => {
                    conditions.push("sequence < ?".to_string());
                    params.push(QueryValue::Int(cursor as i64));
                }
            }
        }

        if let Some(ref method) = self.method {
            conditions.push("method = ?".to_string());
            params.push(QueryValue::Text(method.to_uppercase()));
        }

        if let Some(status) = self.status {
            conditions.push("status = ?".to_string());
            params.push(QueryValue::Int(status as i64));
        }

        if let Some(min) = self.status_min {
            conditions.push("status >= ?".to_string());
            params.push(QueryValue::Int(min as i64));
        }

        if let Some(max) = self.status_max {
            conditions.push("status <= ?".to_string());
            params.push(QueryValue::Int(max as i64));
        }

        if let Some(ref protocol) = self.protocol {
            conditions.push("protocol = ?".to_string());
            params.push(QueryValue::Text(protocol.to_lowercase()));
        }

        if let Some(true) = self.has_rule_hit {
            conditions.push(format!("(flags & {}) != 0", TrafficFlags::HAS_RULE_HIT));
        }
        if let Some(false) = self.has_rule_hit {
            conditions.push(format!("(flags & {}) = 0", TrafficFlags::HAS_RULE_HIT));
        }

        if let Some(true) = self.is_websocket {
            conditions.push(format!("(flags & {}) != 0", TrafficFlags::IS_WEBSOCKET));
        }

        if let Some(true) = self.is_sse {
            conditions.push(format!("(flags & {}) != 0", TrafficFlags::IS_SSE));
        }

        if let Some(true) = self.is_h3 {
            conditions.push(format!("(flags & {}) != 0", TrafficFlags::IS_H3));
        }

        if let Some(true) = self.is_tunnel {
            conditions.push(format!("(flags & {}) != 0", TrafficFlags::IS_TUNNEL));
        }

        if let Some(ref host) = self.host_contains {
            conditions.push("host LIKE ?".to_string());
            params.push(QueryValue::Text(format!("%{}%", host)));
        }

        if let Some(ref url) = self.url_contains {
            conditions.push("url LIKE ?".to_string());
            params.push(QueryValue::Text(format!("%{}%", url)));
        }

        if let Some(ref path) = self.path_contains {
            conditions.push("path LIKE ?".to_string());
            params.push(QueryValue::Text(format!("%{}%", path)));
        }

        if let Some(is_empty) = self.client_app_empty {
            conditions.push(if is_empty {
                "COALESCE(client_app, '') = ''".to_string()
            } else {
                "COALESCE(client_app, '') != ''".to_string()
            });
        } else if let Some(ref app) = self.client_app {
            match self.client_app_match {
                TextMatchMode::Contains => {
                    conditions.push("client_app LIKE ?".to_string());
                    params.push(QueryValue::Text(format!("%{}%", app)));
                }
                TextMatchMode::Equals => {
                    conditions.push("client_app = ?".to_string());
                    params.push(QueryValue::Text(app.clone()));
                }
            }
        }

        if let Some(is_empty) = self.client_ip_empty {
            conditions.push(if is_empty {
                "COALESCE(client_ip, '') = ''".to_string()
            } else {
                "COALESCE(client_ip, '') != ''".to_string()
            });
        } else if let Some(ref ip) = self.client_ip {
            match self.client_ip_match {
                TextMatchMode::Contains => {
                    conditions.push("client_ip LIKE ?".to_string());
                    params.push(QueryValue::Text(format!("%{}%", ip)));
                }
                TextMatchMode::Equals => {
                    conditions.push("client_ip = ?".to_string());
                    params.push(QueryValue::Text(ip.clone()));
                }
            }
        }

        if let Some(port) = self.listener_port {
            conditions.push("listener_port = ?".to_string());
            params.push(QueryValue::Int(port as i64));
        }

        if let Some(ref ct) = self.content_type {
            conditions.push("content_type LIKE ?".to_string());
            params.push(QueryValue::Text(format!("%{}%", ct)));
        }

        if let Some(since_ms) = self.since_ms {
            conditions.push("timestamp >= ?".to_string());
            params.push(QueryValue::Int(since_ms));
        }

        if let Some(until_ms) = self.until_ms {
            conditions.push("timestamp <= ?".to_string());
            params.push(QueryValue::Int(until_ms));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };

        (where_clause, params)
    }

    pub fn build_select_sql(&self) -> (String, Vec<QueryValue>) {
        let (where_clause, params) = self.build_where_clause();

        let order = match self.direction {
            Direction::Forward => "ORDER BY sequence ASC",
            Direction::Backward => "ORDER BY sequence DESC",
        };

        let limit = self.limit.unwrap_or(100);

        let sql = format!(
            "SELECT sequence, id, timestamp, host, method, status, protocol, \
             url, path, content_type, request_size, response_size, upload_bytes, download_bytes, duration_ms, \
             listener_port, client_ip, client_app, client_pid, flags, frame_count, \
             socket_is_open, socket_send_count, socket_receive_count, \
             socket_send_bytes, socket_receive_bytes, socket_frame_count, \
             rule_count, rule_protocols, request_content_type \
             FROM traffic_records{} {} LIMIT {}",
            where_clause, order, limit
        );

        (sql, params)
    }

    pub fn build_count_sql(&self) -> (String, Vec<QueryValue>) {
        let (where_clause, params) = self.build_where_clause();
        let sql = format!("SELECT COUNT(*) FROM traffic_records{}", where_clause);
        (sql, params)
    }
}

#[derive(Debug, Clone)]
pub enum QueryValue {
    Int(i64),
    Text(String),
}

impl rusqlite::ToSql for QueryValue {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        match self {
            QueryValue::Int(i) => i.to_sql(),
            QueryValue::Text(s) => s.to_sql(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    pub records: Vec<TrafficSummaryCompact>,
    pub next_cursor: Option<u64>,
    pub prev_cursor: Option<u64>,
    pub has_more: bool,
    pub total: usize,
    pub server_sequence: u64,
}

#[cfg(test)]
mod tests {
    use super::{QueryParams, QueryValue, TextMatchMode};

    #[test]
    fn build_where_clause_supports_empty_client_app_filter() {
        let params = QueryParams {
            client_app_empty: Some(true),
            ..Default::default()
        };

        let (where_clause, values) = params.build_where_clause();
        assert!(where_clause.contains("COALESCE(client_app, '') = ''"));
        assert!(values.is_empty());
    }

    #[test]
    fn build_where_clause_supports_exact_client_app_filter() {
        let params = QueryParams {
            client_app: Some("Safari".to_string()),
            client_app_match: TextMatchMode::Equals,
            ..Default::default()
        };

        let (where_clause, values) = params.build_where_clause();
        assert!(where_clause.contains("client_app = ?"));
        assert!(matches!(values.first(), Some(QueryValue::Text(v)) if v == "Safari"));
    }

    #[test]
    fn build_where_clause_supports_listener_port_filter() {
        let params = QueryParams {
            listener_port: Some(50831),
            ..Default::default()
        };

        let (where_clause, values) = params.build_where_clause();
        assert!(where_clause.contains("listener_port = ?"));
        assert!(matches!(values.first(), Some(QueryValue::Int(v)) if *v == 50831));
    }

    #[test]
    fn build_where_clause_supports_timestamp_window_filter() {
        let params = QueryParams {
            since_ms: Some(1_700_000_000_000),
            until_ms: Some(1_700_000_060_000),
            ..Default::default()
        };

        let (where_clause, values) = params.build_where_clause();

        assert!(where_clause.contains("timestamp >= ?"));
        assert!(where_clause.contains("timestamp <= ?"));
        assert!(matches!(values.first(), Some(QueryValue::Int(v)) if *v == 1_700_000_000_000));
        assert!(matches!(values.get(1), Some(QueryValue::Int(v)) if *v == 1_700_000_060_000));
    }
}
