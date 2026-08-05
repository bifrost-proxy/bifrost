use std::collections::HashMap;

use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};

use super::{error_response, json_response, method_not_allowed, BoxBody};
use crate::state::SharedAdminState;
use crate::traffic_db::{AppMetricsAggregate, HostMetricsAggregate};

pub async fn handle_metrics(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();

    match path {
        "/api/metrics" | "/api/metrics/" => match method {
            Method::GET => get_current_metrics(state).await,
            _ => method_not_allowed(),
        },
        "/api/metrics/history" => match method {
            Method::GET => get_metrics_history(req, state).await,
            _ => method_not_allowed(),
        },
        "/api/metrics/apps" => match method {
            Method::GET => get_app_metrics(state, include_summary(req.uri().query())).await,
            _ => method_not_allowed(),
        },
        "/api/metrics/hosts" => match method {
            Method::GET => get_host_metrics(state, include_summary(req.uri().query())).await,
            _ => method_not_allowed(),
        },
        _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
    }
}

async fn get_current_metrics(state: SharedAdminState) -> Response<BoxBody> {
    let metrics = state.metrics_collector.get_current();
    json_response(&metrics)
}

async fn get_metrics_history(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let query = req.uri().query().unwrap_or("");
    let limit = parse_limit(query);

    let history = state.metrics_collector.get_history(limit);
    json_response(&history)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppMetrics {
    pub app_name: String,
    pub requests: u64,
    pub active_connections: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub http_requests: u64,
    pub https_requests: u64,
    pub tunnel_requests: u64,
    pub ws_requests: u64,
    pub wss_requests: u64,
    pub h3_requests: u64,
    pub socks5_requests: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct MetricsAggregateSummary {
    pub total: usize,
    pub requests: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub total_traffic_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsAggregateResponse<T> {
    pub items: Vec<T>,
    pub summary: MetricsAggregateSummary,
}

fn summarize_metrics<T>(
    items: &[T],
    fields: impl Fn(&T) -> (u64, u64, u64),
) -> MetricsAggregateSummary {
    let mut summary = MetricsAggregateSummary {
        total: items.len(),
        ..Default::default()
    };
    for item in items {
        let (requests, bytes_sent, bytes_received) = fields(item);
        summary.requests = summary.requests.saturating_add(requests);
        summary.bytes_sent = summary.bytes_sent.saturating_add(bytes_sent);
        summary.bytes_received = summary.bytes_received.saturating_add(bytes_received);
    }
    summary.total_traffic_bytes = summary.bytes_sent.saturating_add(summary.bytes_received);
    summary
}

fn include_summary(query: Option<&str>) -> bool {
    query.is_some_and(|query| {
        query.split('&').any(|pair| {
            pair.split_once('=')
                .is_some_and(|(key, value)| key == "include_summary" && value == "true")
        })
    })
}

async fn get_app_metrics(state: SharedAdminState, with_summary: bool) -> Response<BoxBody> {
    let mut app_stats: HashMap<String, AppMetrics> = HashMap::new();

    if let Some(ref db_store) = state.traffic_db_store {
        let aggregates = db_store.aggregate_app_metrics();
        for aggregate in aggregates {
            let AppMetricsAggregate {
                app_name,
                requests,
                bytes_sent,
                bytes_received,
                http_requests,
                https_requests,
                tunnel_requests,
                ws_requests,
                wss_requests,
                h3_requests,
                socks5_requests,
            } = aggregate;
            app_stats.insert(
                app_name.clone(),
                AppMetrics {
                    app_name,
                    requests,
                    bytes_sent,
                    bytes_received,
                    http_requests,
                    https_requests,
                    tunnel_requests,
                    ws_requests,
                    wss_requests,
                    h3_requests,
                    socks5_requests,
                    active_connections: 0,
                },
            );
        }
    } else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "Traffic DB not available");
    }

    for (_, _, _, _, client_app) in state.connection_registry.list_connections_full() {
        let app_name = client_app.unwrap_or_else(|| "Unknown".to_string());
        let entry = app_stats
            .entry(app_name.clone())
            .or_insert_with(|| AppMetrics {
                app_name,
                ..Default::default()
            });
        entry.active_connections += 1;
    }

    let mut result: Vec<AppMetrics> = app_stats.into_values().collect();
    result.sort_by_key(|a| std::cmp::Reverse(a.requests));

    if with_summary {
        let summary = summarize_metrics(&result, |item| {
            (item.requests, item.bytes_sent, item.bytes_received)
        });
        json_response(&MetricsAggregateResponse {
            items: result,
            summary,
        })
    } else {
        json_response(&result)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HostMetrics {
    pub host: String,
    pub requests: u64,
    pub active_connections: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub http_requests: u64,
    pub https_requests: u64,
    pub tunnel_requests: u64,
    pub ws_requests: u64,
    pub wss_requests: u64,
    pub h3_requests: u64,
    pub socks5_requests: u64,
}

async fn get_host_metrics(state: SharedAdminState, with_summary: bool) -> Response<BoxBody> {
    let mut host_stats: HashMap<String, HostMetrics> = HashMap::new();

    if let Some(ref db_store) = state.traffic_db_store {
        let aggregates = db_store.aggregate_host_metrics();
        for aggregate in aggregates {
            let HostMetricsAggregate {
                host,
                requests,
                bytes_sent,
                bytes_received,
                http_requests,
                https_requests,
                tunnel_requests,
                ws_requests,
                wss_requests,
                h3_requests,
                socks5_requests,
            } = aggregate;
            host_stats.insert(
                host.clone(),
                HostMetrics {
                    host,
                    requests,
                    bytes_sent,
                    bytes_received,
                    http_requests,
                    https_requests,
                    tunnel_requests,
                    ws_requests,
                    wss_requests,
                    h3_requests,
                    socks5_requests,
                    active_connections: 0,
                },
            );
        }
    } else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "Traffic DB not available");
    }

    for (_, host, _, _, _) in state.connection_registry.list_connections_full() {
        let host = if host.is_empty() {
            "Unknown".to_string()
        } else {
            host
        };
        let entry = host_stats
            .entry(host.clone())
            .or_insert_with(|| HostMetrics {
                host,
                ..Default::default()
            });
        entry.active_connections += 1;
    }

    let mut result: Vec<HostMetrics> = host_stats.into_values().collect();
    result.sort_by_key(|a| std::cmp::Reverse(a.requests));

    if with_summary {
        let summary = summarize_metrics(&result, |item| {
            (item.requests, item.bytes_sent, item.bytes_received)
        });
        json_response(&MetricsAggregateResponse {
            items: result,
            summary,
        })
    } else {
        json_response(&result)
    }
}

fn parse_limit(query: &str) -> Option<usize> {
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            if key == "limit" {
                return value.parse().ok();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use http_body_util::BodyExt;

    use super::*;
    use crate::state::AdminState;
    use crate::traffic::TrafficRecord;
    use crate::traffic_db::TrafficDbStore;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bifrost-{}-{}", name, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn host_metrics_include_traffic_db_records() {
        let db_dir = temp_dir("metrics-hosts");
        let db_store = TrafficDbStore::new(db_dir.clone(), 5000, 0, None).unwrap();

        let state = Arc::new(AdminState::new(0).with_traffic_db_store(db_store));

        let mut record = TrafficRecord::new(
            "req-1".to_string(),
            "GET".to_string(),
            "https://example.com/a".to_string(),
        );
        record.status = 200;
        record.request_size = 10;
        record.response_size = 20;
        state.record_traffic(record);

        let resp = super::get_host_metrics(state, false).await;
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let metrics: Vec<HostMetrics> = serde_json::from_slice(&body).unwrap();

        let m = metrics.iter().find(|m| m.host == "example.com").unwrap();
        assert_eq!(m.requests, 1);
        assert_eq!(m.bytes_sent, 10);
        assert_eq!(m.bytes_received, 20);
        assert_eq!(m.https_requests, 1);

        std::fs::remove_dir_all(&db_dir).ok();
    }

    #[tokio::test]
    async fn app_metrics_include_traffic_db_records() {
        let db_dir = temp_dir("metrics-apps");
        let db_store = TrafficDbStore::new(db_dir.clone(), 5000, 0, None).unwrap();

        let state = Arc::new(AdminState::new(0).with_traffic_db_store(db_store));

        let mut record = TrafficRecord::new(
            "req-2".to_string(),
            "GET".to_string(),
            "https://example.com/b".to_string(),
        );
        record.status = 200;
        record.request_size = 7;
        record.response_size = 9;
        record.client_app = Some("TestApp".to_string());
        state.record_traffic(record);

        let resp = super::get_app_metrics(state, false).await;
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let metrics: Vec<AppMetrics> = serde_json::from_slice(&body).unwrap();

        let m = metrics.iter().find(|m| m.app_name == "TestApp").unwrap();
        assert_eq!(m.requests, 1);
        assert_eq!(m.bytes_sent, 7);
        assert_eq!(m.bytes_received, 9);
        assert_eq!(m.https_requests, 1);

        std::fs::remove_dir_all(&db_dir).ok();
    }

    #[tokio::test]
    async fn app_metrics_summary_is_calculated_by_server() {
        let db_dir = temp_dir("metrics-apps-summary");
        let db_store = TrafficDbStore::new(db_dir.clone(), 5000, 0, None).unwrap();
        let state = Arc::new(AdminState::new(0).with_traffic_db_store(db_store));

        for (id, app, upload, download) in [
            ("summary-1", "First App", 11, 13),
            ("summary-2", "Second App", 17, 19),
        ] {
            let mut record = TrafficRecord::new(
                id.to_string(),
                "GET".to_string(),
                format!("https://{id}.test/"),
            );
            record.client_app = Some(app.to_string());
            record.upload_bytes = upload;
            record.download_bytes = download;
            state.record_traffic(record);
        }

        let resp = super::get_app_metrics(state, true).await;
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let response: MetricsAggregateResponse<AppMetrics> = serde_json::from_slice(&body).unwrap();

        assert_eq!(response.summary.total, 2);
        assert_eq!(response.summary.requests, 2);
        assert_eq!(response.summary.bytes_sent, 28);
        assert_eq!(response.summary.bytes_received, 32);
        assert_eq!(response.summary.total_traffic_bytes, 60);

        std::fs::remove_dir_all(&db_dir).ok();
    }

    #[test]
    fn include_summary_requires_explicit_true_value() {
        assert!(include_summary(Some("include_summary=true")));
        assert!(include_summary(Some("other=1&include_summary=true")));
        assert!(!include_summary(Some("include_summary=false")));
        assert!(!include_summary(None));
    }
}
