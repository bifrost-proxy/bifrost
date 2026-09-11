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
    #[serde(default)]
    pub order_by_time: bool,

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
    pub account_name: Option<String>,
    #[serde(default)]
    pub account_name_match: TextMatchMode,
    pub account_name_empty: Option<bool>,
    pub client_ip: Option<String>,
    #[serde(default)]
    pub client_ip_match: TextMatchMode,
    pub client_ip_empty: Option<bool>,
    pub listener_port: Option<u16>,
    pub content_type: Option<String>,

    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,

    pub record_ids: Option<Vec<String>>,
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
            || self.account_name.is_some()
            || self.account_name_empty.is_some()
            || self.client_ip.is_some()
            || self.client_ip_empty.is_some()
            || self.listener_port.is_some()
            || self.content_type.is_some()
            || self.since_ms.is_some()
            || self.until_ms.is_some()
            || self.record_ids.as_ref().is_some_and(|ids| !ids.is_empty())
    }

    pub fn build_where_clause(&self) -> (String, Vec<QueryValue>) {
        let mut conditions = Vec::new();
        let mut params: Vec<QueryValue> = Vec::new();

        if let Some(cursor) = self.cursor {
            let operator = match self.direction {
                Direction::Forward => ">",
                Direction::Backward => "<",
            };
            if self.order_by_time {
                conditions.push(format!(
                    "(timestamp, sequence) {operator} (SELECT timestamp, sequence FROM traffic_records WHERE sequence = ?)"
                ));
            } else {
                conditions.push(format!("sequence {operator} ?"));
            }
            params.push(QueryValue::Int(cursor as i64));
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

        for (filter, flag) in [
            (self.is_websocket, TrafficFlags::IS_WEBSOCKET),
            (self.is_sse, TrafficFlags::IS_SSE),
            (self.is_h3, TrafficFlags::IS_H3),
            (self.is_tunnel, TrafficFlags::IS_TUNNEL),
        ] {
            if let Some(enabled) = filter {
                let operator = if enabled { "!=" } else { "=" };
                conditions.push(format!("(flags & {flag}) {operator} 0"));
            }
        }

        if let Some(ref host) = self.host_contains {
            conditions.push("host LIKE ? ESCAPE '\\'".to_string());
            params.push(QueryValue::Text(contains_pattern(host)));
        }

        if let Some(ref url) = self.url_contains {
            conditions.push("url LIKE ? ESCAPE '\\'".to_string());
            params.push(QueryValue::Text(contains_pattern(url)));
        }

        if let Some(ref path) = self.path_contains {
            conditions.push("path LIKE ? ESCAPE '\\'".to_string());
            params.push(QueryValue::Text(contains_pattern(path)));
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
                    conditions.push("client_app LIKE ? ESCAPE '\\'".to_string());
                    params.push(QueryValue::Text(contains_pattern(app)));
                }
                TextMatchMode::Equals => {
                    conditions.push("client_app = ?".to_string());
                    params.push(QueryValue::Text(app.clone()));
                }
            }
        }

        if let Some(is_empty) = self.account_name_empty {
            conditions.push(if is_empty {
                "COALESCE(account_name, '') = ''".to_string()
            } else {
                "COALESCE(account_name, '') != ''".to_string()
            });
        } else if let Some(ref account_name) = self.account_name {
            match self.account_name_match {
                TextMatchMode::Contains => {
                    conditions.push("account_name LIKE ? ESCAPE '\\'".to_string());
                    params.push(QueryValue::Text(contains_pattern(account_name)));
                }
                TextMatchMode::Equals => {
                    conditions.push("account_name = ?".to_string());
                    params.push(QueryValue::Text(account_name.clone()));
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
                    conditions.push("client_ip LIKE ? ESCAPE '\\'".to_string());
                    params.push(QueryValue::Text(contains_pattern(ip)));
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
            conditions.push("content_type LIKE ? ESCAPE '\\'".to_string());
            params.push(QueryValue::Text(contains_pattern(ct)));
        }

        if let Some(since_ms) = self.since_ms {
            conditions.push("timestamp >= ?".to_string());
            params.push(QueryValue::Int(since_ms));
        }

        if let Some(until_ms) = self.until_ms {
            conditions.push("timestamp <= ?".to_string());
            params.push(QueryValue::Int(until_ms));
        }

        if let Some(record_ids) = self.record_ids.as_ref().filter(|ids| !ids.is_empty()) {
            conditions.push(format!(
                "id IN ({})",
                std::iter::repeat_n("?", record_ids.len())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            params.extend(record_ids.iter().cloned().map(QueryValue::Text));
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

        let order = match (self.order_by_time, self.direction) {
            (true, Direction::Forward) => "ORDER BY timestamp ASC, sequence ASC",
            (true, Direction::Backward) => "ORDER BY timestamp DESC, sequence DESC",
            (false, Direction::Forward) => "ORDER BY sequence ASC",
            (false, Direction::Backward) => "ORDER BY sequence DESC",
        };

        let limit = self.limit.unwrap_or(100);

        let sql = format!(
            "SELECT sequence, id, timestamp, host, method, status, protocol, \
             url, path, content_type, request_size, response_size, upload_bytes, download_bytes, duration_ms, \
             listener_port, client_ip, client_app, client_pid, flags, frame_count, \
             socket_is_open, socket_send_count, socket_receive_count, \
             socket_send_bytes, socket_receive_bytes, socket_frame_count, \
             rule_count, rule_protocols, request_content_type, account_name \
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

fn contains_pattern(value: &str) -> String {
    format!(
        "%{}%",
        value
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
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
    fn literal_contains_escapes_sql_wildcards_for_every_text_filter() {
        let value = "a_b%\\c";
        let params = QueryParams {
            host_contains: Some(value.to_string()),
            url_contains: Some(value.to_string()),
            path_contains: Some(value.to_string()),
            client_app: Some(value.to_string()),
            account_name: Some(value.to_string()),
            client_ip: Some(value.to_string()),
            content_type: Some(value.to_string()),
            ..Default::default()
        };
        let (clause, values) = params.build_where_clause();
        assert_eq!(clause.matches("ESCAPE").count(), 7);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        for pattern in values {
            let QueryValue::Text(pattern) = pattern else {
                panic!("expected text")
            };
            for (text, expected) in [(value, true), ("aXb%\\c", false), ("a_bZZ\\c", false)] {
                let matched: bool = conn
                    .query_row(
                        "SELECT ? LIKE ? ESCAPE '\\'",
                        rusqlite::params![text, pattern],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(matched, expected, "{text}");
            }
        }
    }

    #[test]
    fn boolean_filters_match_both_true_and_false() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        for enabled in [true, false] {
            let params = QueryParams {
                is_websocket: Some(enabled),
                is_sse: Some(enabled),
                is_h3: Some(enabled),
                is_tunnel: Some(enabled),
                ..Default::default()
            };
            let (clause, _) = params.build_where_clause();
            for flags in [0, u32::MAX] {
                let sql = format!("SELECT COUNT(*) FROM (SELECT {flags} AS flags){clause}");
                let matched: u32 = conn.query_row(&sql, [], |row| row.get(0)).unwrap();
                assert_eq!(matched, u32::from((flags != 0) == enabled));
            }
        }
    }

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
    fn build_where_clause_supports_exact_account_name_filter() {
        let params = QueryParams {
            account_name: Some("alice".to_string()),
            account_name_match: TextMatchMode::Equals,
            ..Default::default()
        };

        let (where_clause, values) = params.build_where_clause();
        assert!(where_clause.contains("account_name = ?"));
        assert!(matches!(values.first(), Some(QueryValue::Text(v)) if v == "alice"));
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

    #[test]
    fn build_where_clause_supports_record_id_filter_with_other_conditions() {
        let params = QueryParams {
            method: Some("post".to_string()),
            record_ids: Some(vec!["id-a".to_string(), "id-b".to_string()]),
            ..Default::default()
        };

        let (where_clause, values) = params.build_where_clause();
        assert!(where_clause.contains("method = ?"));
        assert!(where_clause.contains("id IN (?, ?)"));
        assert_eq!(values.len(), 3);
        assert!(params.has_filters());
    }
}
