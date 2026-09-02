use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use tokio_stream::StreamExt;

use super::frames::{get_frame_detail, get_frames, subscribe_frames, unsubscribe_frames};
use super::{
    error_response, full_body, json_response, method_not_allowed, success_response, BoxBody,
};
use crate::body_store::BodyRef;
use crate::push::{SharedPushManager, MAX_ID_LEN, MAX_SUBSCRIBED_IDS};
use crate::query_service::AdminQueryService;
use crate::state::{AdminState, SharedAdminState};
use crate::traffic_db::{QueryParams, TrafficSummaryCompact};

mod sse_stream;
use sse_stream::subscribe_sse_stream;
mod batch;
use batch::batch_traffic;
mod body;
use body::{get_request_body, get_response_body, get_response_body_content, load_body_bytes_async};

fn empty_query_result() -> crate::traffic_db::QueryResult {
    crate::traffic_db::QueryResult {
        records: Vec::new(),
        next_cursor: None,
        prev_cursor: None,
        has_more: false,
        total: 0,
        server_sequence: 0,
    }
}

async fn join_clear_task<T>(
    task: tokio::task::JoinHandle<std::result::Result<T, String>>,
    label: &str,
) -> std::result::Result<T, String> {
    task.await
        .map_err(|e| format!("{label} clear task join failed: {e}"))?
}

fn enrich_compact_frame_info(summary: &mut TrafficSummaryCompact, state: &AdminState) {
    state.reconcile_socket_summary(summary);
}

pub async fn handle_traffic(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
    path: &str,
) -> Response<BoxBody> {
    let path = path.trim_end_matches('/');
    let method = req.method().clone();

    if path == "/api/traffic" {
        match method {
            Method::GET => list_traffic(req, state).await,
            Method::DELETE => clear_traffic(req, state, push_manager).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/traffic/query" {
        match method {
            Method::POST => query_traffic(req, state).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/traffic/updates" {
        match method {
            Method::GET => get_traffic_updates(req, state).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/traffic/statistics" {
        match method {
            Method::GET => get_traffic_statistics(state),
            _ => method_not_allowed(),
        }
    } else if path == "/api/traffic/batch" {
        match method {
            Method::GET => batch_traffic(req, state).await,
            _ => method_not_allowed(),
        }
    } else if let Some(rest) = path.strip_prefix("/api/traffic/") {
        let rest = rest.trim_end_matches('/');
        if let Some(id) = rest.strip_suffix("/request-body") {
            match method {
                Method::GET => get_request_body(state, id, req.uri().query()).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/response-body/content") {
            match method {
                Method::GET => get_response_body_content(state, id, req.uri().query()).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/response-body") {
            match method {
                Method::GET => get_response_body(state, id, req.uri().query()).await,
                _ => method_not_allowed(),
            }
        } else if let Some((id, after)) = rest.split_once("/sse/stream") {
            let after = after.trim().trim_matches('/');
            if !after.is_empty() {
                return error_response(StatusCode::BAD_REQUEST, "Invalid SSE stream path");
            }
            match method {
                Method::GET => subscribe_sse_stream(state, id, req.uri().query()).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/frames/stream") {
            match method {
                Method::GET => subscribe_frames(state, id).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/frames/unsubscribe") {
            match method {
                Method::DELETE => unsubscribe_frames(state, id).await,
                _ => method_not_allowed(),
            }
        } else if rest.contains("/frames/") {
            if let Some((id, frame_part)) = rest.split_once("/frames/") {
                if let Ok(frame_id) = frame_part.parse::<u64>() {
                    match method {
                        Method::GET => get_frame_detail(state, id, frame_id).await,
                        _ => method_not_allowed(),
                    }
                } else {
                    error_response(StatusCode::BAD_REQUEST, "Invalid frame ID")
                }
            } else {
                error_response(StatusCode::BAD_REQUEST, "Invalid path")
            }
        } else if let Some(id) = rest.strip_suffix("/frames") {
            match method {
                Method::GET => get_frames(state, id, req.uri().query()).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/auth-status") {
            match method {
                Method::GET => get_traffic_auth_status(state, id).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/export") {
            match method {
                Method::GET => get_traffic_export(state, id, req.uri().query()).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/replay") {
            match method {
                Method::POST => post_traffic_replay(req, state, id).await,
                _ => method_not_allowed(),
            }
        } else {
            match method {
                Method::GET => get_traffic_detail(state, rest).await,
                _ => method_not_allowed(),
            }
        }
    } else {
        error_response(StatusCode::NOT_FOUND, "Not Found")
    }
}

fn get_traffic_statistics(state: SharedAdminState) -> Response<BoxBody> {
    match state.traffic_db_store.as_ref() {
        Some(store) => json_response(&store.traffic_statistics()),
        None => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Traffic database not available",
        ),
    }
}

fn query_wants_raw(query: Option<&str>) -> bool {
    let Some(q) = query else {
        return false;
    };
    for part in q.split('&') {
        if let Some(v) = part.strip_prefix("raw=") {
            if v == "1" || v.eq_ignore_ascii_case("true") {
                return true;
            }
            return false;
        }
    }
    false
}

fn query_wants_base64(query: Option<&str>) -> bool {
    let Some(q) = query else {
        return false;
    };
    for part in q.split('&') {
        if let Some(v) = part
            .strip_prefix("encoding=")
            .or_else(|| part.strip_prefix("format="))
        {
            return v.eq_ignore_ascii_case("base64");
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{
        decode_query_value, parse_query_params_from_query_string, parse_updates_params,
        query_wants_base64, query_wants_raw,
    };
    use crate::push::{MAX_ID_LEN, MAX_SUBSCRIBED_IDS};
    use crate::traffic_db::{Direction, TextMatchMode};

    #[test]
    fn raw_body_query_flags_are_parsed_independently() {
        assert!(query_wants_raw(Some("raw=1&encoding=base64")));
        assert!(query_wants_raw(Some("raw=true")));
        assert!(!query_wants_raw(Some("raw=0&encoding=base64")));
        assert!(!query_wants_raw(Some("encoding=base64")));

        assert!(query_wants_base64(Some("raw=1&encoding=base64")));
        assert!(query_wants_base64(Some("format=base64")));
        assert!(!query_wants_base64(Some("raw=1&encoding=text")));
        assert!(!query_wants_base64(None));
    }

    #[test]
    fn decode_query_value_handles_plus_and_percent_encoding() {
        assert_eq!(decode_query_value("hello+world"), "hello world");
        assert_eq!(decode_query_value("a%2Bb+test"), "a+b test");
    }

    #[test]
    fn parse_updates_params_parses_fields_and_limits_pending_ids() {
        let long_id = "x".repeat(MAX_ID_LEN + 1);
        let mut parts = vec!["keep".to_string(), "".to_string(), long_id.clone()];
        for i in 0..(MAX_SUBSCRIBED_IDS + 2) {
            parts.push(format!("id-{i}"));
        }
        let query = format!(
            "after_id=req-1&after_seq=42&pending_ids={}&limit=200",
            parts.join(",")
        );

        let params = parse_updates_params(&query);

        assert_eq!(params.after_id.as_deref(), Some("req-1"));
        assert_eq!(params.after_seq, Some(42));
        assert_eq!(params.limit, Some(200));
        assert!(params.pending_ids.len() <= MAX_SUBSCRIBED_IDS);
        assert!(params.pending_ids.contains(&"keep".to_string()));
        assert!(!params.pending_ids.contains(&"".to_string()));
        assert!(!params.pending_ids.contains(&long_id));
    }

    #[test]
    fn parse_query_params_from_query_string_parses_basic_filters() {
        let query = "\
cursor=10\
&limit=200\
&direction=forward\
&method=GET\
&status=200\
&status_min=100\
&status_max=500\
&protocol=HTTP\
&has_rule_hit=true\
&is_websocket=false\
&is_sse=true\
&is_h3=false\
&is_tunnel=true\
&host=example.com\
&url_contains=%2Fapi%2Fv1\
&path_contains=%2Ffoo\
&client_app=Chrome\
&client_app_match=equals\
&client_app_empty=false\
&client_ip=1.2.3.4\
&client_ip_match=contains\
&client_ip_empty=true\
&listener_port=8080\
&content_type=text%2Fplain";

        let params = parse_query_params_from_query_string(query);

        assert_eq!(params.cursor, Some(10));
        assert_eq!(params.limit, Some(200));
        assert_eq!(params.direction, Direction::Forward);
        assert_eq!(params.method.as_deref(), Some("GET"));
        assert_eq!(params.status, Some(200));
        assert_eq!(params.status_min, Some(100));
        assert_eq!(params.status_max, Some(500));
        assert_eq!(params.protocol.as_deref(), Some("HTTP"));
        assert_eq!(params.has_rule_hit, Some(true));
        assert_eq!(params.is_websocket, Some(false));
        assert_eq!(params.is_sse, Some(true));
        assert_eq!(params.is_h3, Some(false));
        assert_eq!(params.is_tunnel, Some(true));
        assert_eq!(params.host_contains.as_deref(), Some("example.com"));
        assert_eq!(params.url_contains.as_deref(), Some("/api/v1"));
        assert_eq!(params.path_contains.as_deref(), Some("/foo"));
        assert_eq!(params.client_app.as_deref(), Some("Chrome"));
        assert_eq!(params.client_app_match, TextMatchMode::Equals);
        assert_eq!(params.client_app_empty, Some(false));
        assert_eq!(params.client_ip.as_deref(), Some("1.2.3.4"));
        assert_eq!(params.client_ip_match, TextMatchMode::Contains);
        assert_eq!(params.client_ip_empty, Some(true));
        assert_eq!(params.listener_port, Some(8080));
        assert_eq!(params.content_type.as_deref(), Some("text/plain"));
    }

    #[test]
    fn parse_query_params_from_query_string_defaults_limit_when_missing() {
        let params = parse_query_params_from_query_string("method=GET");
        assert_eq!(params.method.as_deref(), Some("GET"));
        assert_eq!(params.limit, Some(100));
    }

    #[test]
    fn raw_body_query_flags_use_first_raw_value() {
        // First raw flag wins and short-circuits.
        assert!(query_wants_raw(Some("raw=1&raw=0")));
        assert!(!query_wants_raw(Some("raw=0&raw=1")));
        assert!(!query_wants_raw(Some("raw=false&encoding=base64")));
    }

    #[test]
    fn parse_updates_params_uses_cursor_alias_and_ignores_invalid_numbers() {
        let params = parse_updates_params("after_seq=bogus&cursor=10&limit=abc");
        assert_eq!(params.after_seq, Some(10));
        assert!(params.limit.is_none());
    }

    #[test]
    fn parse_query_params_from_query_string_supports_host_url_and_path_synonyms() {
        let params = parse_query_params_from_query_string(
            "host_contains=example.com&host=final.test&url=/v1&path=/items",
        );

        // Last occurrence wins and all aliases map to the *_contains fields.
        assert_eq!(params.host_contains.as_deref(), Some("final.test"));
        assert_eq!(params.url_contains.as_deref(), Some("/v1"));
        assert_eq!(params.path_contains.as_deref(), Some("/items"));
    }
}

async fn query_traffic(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    if state.get_super_performance_mode() {
        return json_response(&empty_query_result());
    }

    let body_bytes = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read request body: {}", e),
            );
        }
    };

    let params: QueryParams = match serde_json::from_slice(&body_bytes) {
        Ok(p) => p,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid JSON body: {}", e),
            );
        }
    };

    let service = AdminQueryService::new(state);
    match service.query_traffic_params(params).await {
        Ok(result) => json_response(&result),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Query failed: {}", e),
        ),
    }
}

async fn list_traffic(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    if state.get_super_performance_mode() {
        return json_response(&empty_query_result());
    }
    let query = req.uri().query().unwrap_or("");
    let params = parse_query_params_from_query_string(query);
    let service = AdminQueryService::new(state);
    match service.query_traffic_params(params).await {
        Ok(result) => json_response(&result),
        Err(e) => {
            tracing::error!("[TRAFFIC_API] Query task failed: {}", e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Query failed: {}", e),
            )
        }
    }
}

async fn get_traffic_updates(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    if state.get_super_performance_mode() {
        let response = serde_json::json!({
            "new_records": [],
            "updated_records": [],
            "has_more": false,
            "server_total": 0,
            "server_sequence": 0
        });
        return json_response(&response);
    }
    let query = req.uri().query().unwrap_or("");
    let params = parse_updates_params(query);

    if let Some(ref db_store) = state.traffic_db_store {
        let limit = params.limit.unwrap_or(100);
        let cursor = params.after_seq;
        let pending_ids = params.pending_ids.clone();
        let db_clone = db_store.clone();
        let query_result = tokio::task::spawn_blocking(move || {
            if let Some(cursor) = cursor {
                let query_params = QueryParams {
                    cursor: Some(cursor),
                    limit: Some(limit),
                    direction: crate::traffic_db::Direction::Forward,
                    ..Default::default()
                };
                db_clone.query(&query_params)
            } else {
                db_clone.query_latest_window(limit)
            }
        })
        .await;

        let result = match query_result {
            Ok(r) => r,
            Err(e) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Query failed: {}", e),
                );
            }
        };

        let mut new_records: Vec<TrafficSummaryCompact> = result.records;
        for record in &mut new_records {
            enrich_compact_frame_info(record, &state);
        }

        let updated_records: Vec<TrafficSummaryCompact> = if !pending_ids.is_empty() {
            let ids: Vec<String> = pending_ids;
            let db_clone = db_store.clone();
            let ids_result = tokio::task::spawn_blocking(move || {
                let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
                db_clone.get_by_ids(&id_refs)
            })
            .await;

            match ids_result {
                Ok(mut summaries) => {
                    for summary in &mut summaries {
                        enrich_compact_frame_info(summary, &state);
                    }
                    summaries
                }
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        let response = serde_json::json!({
            "new_records": new_records,
            "updated_records": updated_records,
            "has_more": result.has_more,
            "server_total": result.total,
            "server_sequence": result.server_sequence
        });

        json_response(&response)
    } else {
        error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Traffic database not available",
        )
    }
}

#[derive(Debug, Default)]
struct UpdatesParams {
    after_id: Option<String>,
    after_seq: Option<u64>,
    pending_ids: Vec<String>,
    limit: Option<usize>,
}

fn decode_query_value(value: &str) -> String {
    let value_with_spaces = value.replace('+', " ");
    urlencoding::decode(&value_with_spaces)
        .unwrap_or_default()
        .to_string()
}

fn parse_updates_params(query: &str) -> UpdatesParams {
    let mut params = UpdatesParams::default();

    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            let value = decode_query_value(value);
            match key {
                "after_id" if !value.is_empty() => {
                    params.after_id = Some(value.to_string());
                }
                "after_seq" | "cursor" => {
                    params.after_seq = value.parse().ok();
                }
                "pending_ids" if !value.is_empty() => {
                    params.pending_ids = value
                        .split(',')
                        .take(MAX_SUBSCRIBED_IDS)
                        .filter_map(|s: &str| {
                            let id = s.to_string();
                            if id.is_empty() || id.len() > MAX_ID_LEN {
                                None
                            } else {
                                Some(id)
                            }
                        })
                        .collect();
                }
                "limit" => {
                    params.limit = value.parse().ok();
                }
                _ => {}
            }
        }
    }

    params
}

fn parse_query_params_from_query_string(query: &str) -> QueryParams {
    let mut params = QueryParams::default();

    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            let value = decode_query_value(value);
            match key {
                "cursor" => params.cursor = value.parse().ok(),
                "limit" => params.limit = value.parse().ok(),
                "direction" if value == "forward" => {
                    params.direction = crate::traffic_db::Direction::Forward;
                }
                "method" => params.method = Some(value),
                "status" => params.status = value.parse().ok(),
                "status_min" => params.status_min = value.parse().ok(),
                "status_max" => params.status_max = value.parse().ok(),
                "protocol" => params.protocol = Some(value),
                "has_rule_hit" => params.has_rule_hit = value.parse().ok(),
                "is_websocket" => params.is_websocket = value.parse().ok(),
                "is_sse" => params.is_sse = value.parse().ok(),
                "is_h3" => params.is_h3 = value.parse().ok(),
                "is_tunnel" => params.is_tunnel = value.parse().ok(),
                "host" | "host_contains" => params.host_contains = Some(value),
                "url" | "url_contains" => params.url_contains = Some(value),
                "path" | "path_contains" => params.path_contains = Some(value),
                "client_app" => params.client_app = Some(value),
                "client_app_match" => {
                    params.client_app_match = if value.eq_ignore_ascii_case("equals") {
                        crate::traffic_db::TextMatchMode::Equals
                    } else {
                        crate::traffic_db::TextMatchMode::Contains
                    };
                }
                "client_app_empty" => params.client_app_empty = value.parse().ok(),
                "account_name" => params.account_name = Some(value),
                "account_name_match" => {
                    params.account_name_match = if value.eq_ignore_ascii_case("equals") {
                        crate::traffic_db::TextMatchMode::Equals
                    } else {
                        crate::traffic_db::TextMatchMode::Contains
                    };
                }
                "account_name_empty" => params.account_name_empty = value.parse().ok(),
                "client_ip" => params.client_ip = Some(value),
                "client_ip_match" => {
                    params.client_ip_match = if value.eq_ignore_ascii_case("equals") {
                        crate::traffic_db::TextMatchMode::Equals
                    } else {
                        crate::traffic_db::TextMatchMode::Contains
                    };
                }
                "client_ip_empty" => params.client_ip_empty = value.parse().ok(),
                "listener_port" | "port" => params.listener_port = value.parse().ok(),
                "content_type" => params.content_type = Some(value),
                _ => {}
            }
        }
    }

    if params.limit.is_none() {
        params.limit = Some(100);
    }

    params
}

async fn get_traffic_detail(state: SharedAdminState, id: &str) -> Response<BoxBody> {
    let service = AdminQueryService::new(state.clone());
    match service.get_traffic_record(id).await {
        Ok(mut record) => {
            if let Some(ref socket_status) = record.socket_status {
                let total = socket_status.send_bytes + socket_status.receive_bytes;
                if !socket_status.is_open {
                    if record.response_size == 0 && total > 0 {
                        record.response_size = total as usize;
                    }
                    let status = socket_status.clone();
                    let frame_count = record.frame_count;
                    let last_frame_id = record.last_frame_id;
                    let response_size = record.response_size;
                    let record_id = record.id.clone();
                    state.update_traffic_by_id(&record_id, move |record| {
                        record.response_size = response_size;
                        record.socket_status = Some(status.clone());
                        record.frame_count = frame_count;
                        record.last_frame_id = last_frame_id;
                    });
                }
            }
            json_response(&record)
        }
        Err(_) => error_response(
            StatusCode::NOT_FOUND,
            &format!("Traffic record '{}' not found", id),
        ),
    }
}

#[derive(Debug, serde::Deserialize)]
struct ClearTrafficRequest {
    ids: Option<Vec<String>>,
}

fn parse_clear_traffic_request_body(
    body: &[u8],
) -> std::result::Result<ClearTrafficRequest, String> {
    if body.is_empty() {
        return Ok(ClearTrafficRequest { ids: None });
    }
    serde_json::from_slice(body).map_err(|e| format!("Invalid JSON clear traffic request: {e}"))
}

async fn clear_traffic(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => bytes::Bytes::new(),
    };

    let request = match parse_clear_traffic_request_body(&body) {
        Ok(request) => request,
        Err(e) => {
            tracing::warn!("[CLEAR_TRAFFIC] Failed to parse request body: {}", e);
            return error_response(StatusCode::BAD_REQUEST, &e);
        }
    };

    if let Some(ids) = request.ids {
        if !ids.is_empty() {
            return clear_traffic_by_ids(state, ids, push_manager).await;
        }
    }

    clear_all_traffic(state, push_manager).await
}

async fn clear_traffic_by_ids(
    state: SharedAdminState,
    ids: Vec<String>,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let active_connection_ids = state.connection_monitor.active_connection_ids();
    let active_set: std::collections::HashSet<&String> = active_connection_ids.iter().collect();

    let ids_to_delete: Vec<String> = ids
        .into_iter()
        .filter(|id| !active_set.contains(id))
        .collect();

    if ids_to_delete.is_empty() {
        return success_response("No traffic records to clear (all are active connections)");
    }

    let count = ids_to_delete.len();

    if let Some(ref db_store) = state.traffic_db_store {
        let db_store_clone = db_store.clone();
        let ids_for_db = ids_to_delete.clone();
        let delete_task = tokio::task::spawn_blocking(move || {
            db_store_clone.delete_by_ids(&ids_for_db);
            Ok(())
        });
        if let Err(e) = join_clear_task(delete_task, "traffic db delete-by-ids").await {
            tracing::error!(error = %e, "[CLEAR_TRAFFIC] Failed to delete traffic records by ids");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e);
        }
    }

    if let Some(ref body_store) = state.body_store {
        let body_store_clone = body_store.clone();
        let ids_for_body = ids_to_delete.clone();
        let delete_task = tokio::task::spawn_blocking(move || {
            body_store_clone
                .write()
                .delete_by_ids(&ids_for_body)
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = join_clear_task(delete_task, "body store delete-by-ids").await {
            tracing::warn!(error = %e, "Failed to delete bodies");
        }
    }

    if let Some(ref frame_store) = state.frame_store {
        let frame_store_clone = frame_store.clone();
        let ids_for_frame = ids_to_delete.clone();
        let delete_task = tokio::task::spawn_blocking(move || {
            frame_store_clone
                .delete_by_ids(&ids_for_frame)
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = join_clear_task(delete_task, "frame store delete-by-ids").await {
            tracing::warn!(error = %e, "Failed to delete frames");
        }
    }

    if let Some(ref ws_payload_store) = state.ws_payload_store {
        let ws_payload_store_clone = ws_payload_store.clone();
        let ids_for_payload = ids_to_delete.clone();
        let delete_task = tokio::task::spawn_blocking(move || {
            ws_payload_store_clone
                .delete_by_ids(&ids_for_payload)
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = join_clear_task(delete_task, "ws payload store delete-by-ids").await {
            tracing::warn!(error = %e, "Failed to delete ws payloads");
        }
    }

    if let Some(pm) = push_manager {
        pm.invalidate_overview_cache();
        pm.broadcast_traffic_deleted(ids_to_delete.clone());
    }

    tracing::info!("[CLEAR_TRAFFIC] Deleted {} traffic records", count);
    success_response(&format!("{} traffic records cleared successfully", count))
}

#[cfg(test)]
mod clear_traffic_request_tests {
    use super::parse_clear_traffic_request_body;

    #[test]
    fn malformed_clear_request_is_rejected() {
        let err = parse_clear_traffic_request_body(br#"{"ids":["one""#)
            .expect_err("malformed JSON must not fall back to clear-all");
        assert!(err.contains("Invalid JSON clear traffic request"));
    }

    #[test]
    fn empty_clear_request_keeps_clear_all_semantics() {
        let request = parse_clear_traffic_request_body(b"").expect("empty body is clear-all");
        assert!(request.ids.is_none());
    }

    #[test]
    fn ids_clear_request_parses_ids() {
        let request =
            parse_clear_traffic_request_body(br#"{"ids":["a","b"]}"#).expect("valid ids body");
        assert_eq!(
            request.ids.as_deref(),
            Some(&["a".to_string(), "b".to_string()][..])
        );
    }
}

async fn clear_all_traffic(
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    state.connection_monitor.clear();

    if let Some(ref db_store) = state.traffic_db_store {
        let db_store_clone = db_store.clone();
        let clear_task = tokio::task::spawn_blocking(move || {
            db_store_clone.clear();
            Ok(())
        });
        if let Err(e) = join_clear_task(clear_task, "traffic db clear-all").await {
            tracing::error!(error = %e, "[CLEAR_TRAFFIC] Failed to clear traffic records");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e);
        }
    }

    if let Some(ref body_store) = state.body_store {
        let body_store_clone = body_store.clone();
        let clear_task = tokio::task::spawn_blocking(move || {
            body_store_clone
                .write()
                .clear()
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = join_clear_task(clear_task, "body store clear-all").await {
            tracing::warn!(error = %e, "Failed to clear body store");
        }
    }

    if let Some(ref frame_store) = state.frame_store {
        let frame_store_clone = frame_store.clone();
        let clear_task = tokio::task::spawn_blocking(move || {
            frame_store_clone
                .clear()
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = join_clear_task(clear_task, "frame store clear-all").await {
            tracing::warn!(error = %e, "Failed to clear frame store");
        }
    }

    if let Some(ref ws_payload_store) = state.ws_payload_store {
        let ws_payload_store_clone = ws_payload_store.clone();
        let clear_task = tokio::task::spawn_blocking(move || {
            ws_payload_store_clone
                .clear()
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = join_clear_task(clear_task, "ws payload store clear-all").await {
            tracing::warn!(error = %e, "Failed to clear ws payload store");
        }
    }

    if let Some(pm) = push_manager {
        pm.invalidate_overview_cache();
        pm.notify_traffic_statistics_changed();
    }

    success_response("All traffic data cleared successfully")
}

/// GET /api/traffic/{id}/auth-status — JWT/Cookie 登录态诊断。
async fn get_traffic_auth_status(state: SharedAdminState, id: &str) -> Response<BoxBody> {
    use crate::auth_inspect::build_auth_summary;
    let service = AdminQueryService::new(state.clone());
    let record = match service.get_traffic_record(id).await {
        Ok(r) => r,
        Err(_) => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("Traffic record '{}' not found", id),
            )
        }
    };

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let mut merged: Vec<(String, String)> = Vec::new();
    if let Some(h) = record.request_headers.as_ref() {
        merged.extend(h.iter().cloned());
    }
    if let Some(h) = record.original_request_headers.as_ref() {
        merged.extend(h.iter().cloned());
    }
    if let Some(h) = record.response_headers.as_ref() {
        merged.extend(h.iter().cloned());
    }
    if let Some(h) = record.original_response_headers.as_ref() {
        merged.extend(h.iter().cloned());
    }

    let host = if record.host.is_empty() {
        record.actual_host.clone().unwrap_or_default()
    } else {
        record.host.clone()
    };

    let summary = build_auth_summary(&merged, &host, now_ms);
    json_response(&summary)
}

#[cfg(test)]
mod auth_status_tests {
    use crate::auth_inspect::AuthSummary;
    use base64::Engine as _;

    fn make_jwt(exp: i64, sub: &str) -> String {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\"}");
        let payload = serde_json::json!({"exp": exp, "sub": sub});
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        format!("{header}.{payload_b64}.sig")
    }

    #[test]
    fn auth_summary_serializes_with_expected_fields() {
        // 验证 AuthSummary 的字段命名（防止意外重命名破坏 API 契约）。
        let jwt = make_jwt(9_999_999_999, "u-1");
        let headers = vec![("Authorization".to_string(), format!("Bearer {jwt}"))];
        let s = crate::auth_inspect::build_auth_summary(&headers, "example.com", 1_700_000_000_000);
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("host").is_some());
        assert!(v.get("has_jwt").is_some());
        assert!(v.get("has_cookie").is_some());
        assert!(v.get("jwt_exp_ms").is_some());
        assert!(v.get("jwt_user_id").is_some());
        assert!(v.get("cookie_exp_ms").is_some());
        assert!(v.get("valid_at_ms").is_some());
        assert!(v.get("valid").is_some());

        // 反序列化往返。
        let parsed: AuthSummary = serde_json::from_value(v).unwrap();
        assert_eq!(parsed, s);
    }
}

// =============================================================================
// P2-6: traffic export / replay handlers
// =============================================================================

async fn get_traffic_record_async(
    state: &SharedAdminState,
    id: &str,
) -> Option<crate::traffic::TrafficRecord> {
    if let Some(ref db_store) = state.traffic_db_store {
        let db_clone = db_store.clone();
        let id_owned = id.to_string();
        tokio::task::spawn_blocking(move || db_clone.get_by_id(&id_owned))
            .await
            .ok()
            .flatten()
    } else {
        None
    }
}

fn parse_export_query(query: Option<&str>) -> crate::replay::ExportFormat {
    let mut format = crate::replay::ExportFormat::Curl;
    if let Some(q) = query {
        for part in q.split('&') {
            if let Some(v) = part.strip_prefix("format=") {
                if let Some(f) = crate::replay::ExportFormat::parse(v) {
                    format = f;
                }
            }
        }
    }
    format
}

async fn get_traffic_export(
    state: SharedAdminState,
    id: &str,
    query: Option<&str>,
) -> Response<BoxBody> {
    let format = parse_export_query(query);
    let record = match get_traffic_record_async(&state, id).await {
        Some(r) => r,
        None => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("Traffic record '{}' not found", id),
            )
        }
    };
    let body = if let Some(body_ref) = record
        .raw_request_body_ref
        .as_ref()
        .or(record.request_body_ref.as_ref())
    {
        load_body_bytes_async(&state, body_ref).await
    } else {
        None
    };
    let headers = record.request_headers.clone().unwrap_or_default();
    let opts = crate::replay::ExportOptions { format };
    let text = crate::replay::export_request(
        &record.method,
        &record.url,
        &headers,
        body.as_deref(),
        &opts,
    );
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("Cache-Control", "no-store")
        .body(full_body(text.into_bytes()))
        .unwrap()
}

async fn post_traffic_replay(
    req: Request<Incoming>,
    state: SharedAdminState,
    id: &str,
) -> Response<BoxBody> {
    let body_bytes = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => {
            return error_response(StatusCode::BAD_REQUEST, "Failed to read request body");
        }
    };
    let opts: crate::replay::ReplayOptions = if body_bytes.is_empty() {
        crate::replay::ReplayOptions::default()
    } else {
        match serde_json::from_slice(&body_bytes) {
            Ok(v) => v,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("Invalid JSON replay options: {e}"),
                );
            }
        }
    };
    let record = match get_traffic_record_async(&state, id).await {
        Some(r) => r,
        None => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("Traffic record '{}' not found", id),
            );
        }
    };
    let body = if let Some(body_ref) = record
        .raw_request_body_ref
        .as_ref()
        .or(record.request_body_ref.as_ref())
    {
        load_body_bytes_async(&state, body_ref)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let headers = record.request_headers.clone().unwrap_or_default();

    // refresh-auth：扫最近 N=200 个 traffic 记录，找同 host 的最新认证 header。
    let auth_candidates = if opts.refresh_auth_enabled() {
        let target_host = opts
            .auth_source_host
            .clone()
            .unwrap_or_else(|| record.host.clone())
            .to_ascii_lowercase();
        collect_auth_candidates(&state, &target_host, &record.id, 200).await
    } else {
        Vec::new()
    };

    let client = crate::replay::ReqwestClient;
    match crate::replay::replay_request(
        &client,
        &record.method,
        &record.url,
        &headers,
        &body,
        &opts,
        &auth_candidates,
    )
    .await
    {
        Ok(result) => json_response(&serde_json::json!({
            "success": true,
            "data": {
                "status": result.status,
                "duration_ms": result.duration_ms,
                "request": {
                    "method": record.method,
                    "url": record.url,
                },
                "response": {
                    "status": result.status,
                    "headers": result.headers,
                    "body_b64": result.body_b64,
                },
                "auth_refresh": result.auth_refresh,
                // 兼容旧响应字段：CLI human render 直接取这几个 key。
                "headers": result.headers,
                "body_b64": result.body_b64,
            },
        })),
        Err(e) => json_response(&serde_json::json!({
            "success": false,
            "error": e,
        })),
    }
}

async fn collect_auth_candidates(
    state: &SharedAdminState,
    target_host_lc: &str,
    exclude_id: &str,
    n: usize,
) -> Vec<crate::replay::AuthCandidate> {
    let Some(ref db_store) = state.traffic_db_store else {
        return Vec::new();
    };
    // 1. 取最近 N 条 compact 列表，按 host 粗筛。
    let db_clone = db_store.clone();
    let summaries = match tokio::task::spawn_blocking(move || db_clone.query_latest_window(n)).await
    {
        Ok(r) => r.records,
        Err(_) => return Vec::new(),
    };
    let target = target_host_lc.to_string();
    let exclude = exclude_id.to_string();
    let host_matches: Vec<(String, u64)> = summaries
        .into_iter()
        .filter(|s| s.id != exclude && s.h.to_ascii_lowercase() == target)
        .map(|s| (s.id, s.seq))
        .collect();

    // 2. 对每个粗筛命中拉完整记录，提取请求 header。
    let mut out: Vec<crate::replay::AuthCandidate> = Vec::new();
    for (id, seq) in host_matches {
        let db_clone = db_store.clone();
        let id_owned = id.clone();
        let full = match tokio::task::spawn_blocking(move || db_clone.get_by_id(&id_owned)).await {
            Ok(r) => r,
            Err(_) => continue,
        };
        let Some(rec) = full else { continue };
        let Some(headers) = rec.request_headers.clone() else {
            continue;
        };
        out.push(crate::replay::AuthCandidate {
            id,
            host: rec.host.clone(),
            headers,
            recency: seq,
        });
    }
    out
}
