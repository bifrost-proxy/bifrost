use std::time::Duration;

use bifrost_command::SearchArgs;
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use tokio_stream::StreamExt;
use tracing::warn;

use super::{error_response, json_response, method_not_allowed, BoxBody};
use crate::query_service::AdminQueryService;
use crate::search::{SearchProgress, SearchRequest, SearchedRange, MAX_TARGET_RECORD_IDS};
use crate::state::SharedAdminState;

const SEARCH_HANDLER_TIMEOUT: Duration = Duration::from_secs(310);

pub async fn handle_search(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();

    let path = path.trim_end_matches('/');
    if path == "/api/search" {
        match method {
            Method::POST => execute_search(req, state).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/search/stream" {
        match method {
            Method::POST => execute_search_stream(req, state).await,
            _ => method_not_allowed(),
        }
    } else {
        error_response(StatusCode::NOT_FOUND, "Search endpoint not found")
    }
}

async fn execute_search(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let body_bytes = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read request body: {}", e),
            );
        }
    };

    let search_request: SearchRequest = match serde_json::from_slice(&body_bytes) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid search request: {}", e),
            );
        }
    };

    if let Some(response) = validate_target_record_ids(&search_request) {
        return response;
    }

    if search_request.keyword.trim().is_empty() && !search_request.filters.has_constraints() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Search keyword cannot be empty without any filters",
        );
    }

    let service = AdminQueryService::new(state.clone());
    let search_args = command_search_args_from_request(&search_request);
    let search_future = async move { service.search(&search_args).await };

    let search_result = match tokio::time::timeout(SEARCH_HANDLER_TIMEOUT, search_future).await {
        Ok(result) => result,
        Err(_) => {
            return error_response(
                StatusCode::GATEWAY_TIMEOUT,
                "Search timed out. Try narrowing your search with filters or a more specific keyword.",
            );
        }
    };

    match search_result {
        Ok(response) => json_response(&response),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Search failed: {}", e),
        ),
    }
}

#[derive(Debug, serde::Serialize)]
struct SearchStreamProgressPayload {
    total_searched: usize,
    total_matched: usize,
    next_cursor: Option<u64>,
    has_more_hint: bool,
    iterations: usize,
}

#[derive(Debug, serde::Serialize)]
struct SearchStreamDonePayload {
    total_searched: usize,
    total_matched: usize,
    next_cursor: Option<u64>,
    has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    partial_reason: Option<String>,
    search_id: String,
    searched_range: SearchedRange,
}

#[derive(Debug, serde::Serialize)]
struct SearchStreamErrorPayload {
    message: String,
}

async fn execute_search_stream(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    let body_bytes = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read request body: {}", e),
            );
        }
    };

    let search_request: SearchRequest = match serde_json::from_slice(&body_bytes) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid search request: {}", e),
            );
        }
    };

    if let Some(response) = validate_target_record_ids(&search_request) {
        return response;
    }

    if search_request.keyword.trim().is_empty() && !search_request.filters.has_constraints() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Search keyword cannot be empty without any filters",
        );
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(64);
    let service = AdminQueryService::new(state.clone());
    let search_args = command_search_args_from_request(&search_request);

    tokio::spawn(async move {
        let mut last_progress: Option<SearchProgress> = None;
        let tx_results = tx.clone();
        let tx_progress = tx.clone();
        let response = service
            .search_stream(
                &search_args,
                move |item| {
                    if let Ok(json) = serde_json::to_string(item) {
                        if tx_results
                            .blocking_send(Bytes::from(sse_event("result", &json)))
                            .is_err()
                        {
                            warn!("search stream result receiver disconnected");
                        }
                    }
                },
                move |p| {
                    let changed = last_progress
                        .as_ref()
                        .map(|prev| {
                            prev.total_searched != p.total_searched
                                || prev.total_matched != p.total_matched
                                || prev.cursor != p.cursor
                                || prev.iterations != p.iterations
                                || prev.has_more_hint != p.has_more_hint
                        })
                        .unwrap_or(true);
                    if !changed {
                        return;
                    }
                    last_progress = Some(p.clone());

                    let payload = SearchStreamProgressPayload {
                        total_searched: p.total_searched,
                        total_matched: p.total_matched,
                        next_cursor: p.cursor,
                        has_more_hint: p.has_more_hint,
                        iterations: p.iterations,
                    };

                    if let Ok(json) = serde_json::to_string(&payload) {
                        if tx_progress
                            .blocking_send(Bytes::from(sse_event("progress", &json)))
                            .is_err()
                        {
                            warn!("search stream progress receiver disconnected");
                        }
                    }
                },
            )
            .await;

        match response {
            Ok(response) => {
                let done = SearchStreamDonePayload {
                    total_searched: response.total_searched,
                    total_matched: response.total_matched,
                    next_cursor: response.next_cursor,
                    has_more: response.has_more,
                    partial_reason: response.partial_reason,
                    search_id: response.search_id,
                    searched_range: response.searched_range,
                };
                if let Ok(json) = serde_json::to_string(&done) {
                    if tx
                        .send(Bytes::from(sse_event("done", &json)))
                        .await
                        .is_err()
                    {
                        warn!("search stream done receiver disconnected");
                    }
                }
            }
            Err(error) => {
                let payload = SearchStreamErrorPayload {
                    message: error.to_string(),
                };
                if let Ok(json) = serde_json::to_string(&payload) {
                    if tx
                        .send(Bytes::from(sse_event("error", &json)))
                        .await
                        .is_err()
                    {
                        warn!("search stream error receiver disconnected");
                    }
                }
            }
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx)
        .map(|b| Ok::<_, hyper::Error>(hyper::body::Frame::data(b)));

    let body_stream = http_body_util::StreamBody::new(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(BoxBody::new(body_stream))
        .unwrap()
}

fn sse_event(event: &str, json_data: &str) -> String {
    // SSE 数据行必须以 data: 开头，事件以空行结束
    // 这里保证 json_data 不包含换行，避免破坏 SSE 帧。
    let data = json_data.replace('\n', "\\n");
    format!("event: {}\ndata: {}\n\n", event, data)
}

fn command_search_args_from_request(request: &SearchRequest) -> SearchArgs {
    SearchArgs {
        keyword: request.keyword.clone(),
        scope: bifrost_command::SearchScope {
            request_body: request.scope.request_body,
            response_body: request.scope.response_body,
            request_headers: request.scope.request_headers,
            response_headers: request.scope.response_headers,
            url: request.scope.url,
            websocket_messages: request.scope.websocket_messages,
            sse_events: request.scope.sse_events,
            all: request.scope.all,
        },
        filters: bifrost_command::SearchFilters {
            protocols: request.filters.protocols.clone(),
            status_ranges: request.filters.status_ranges.clone(),
            content_types: request.filters.content_types.clone(),
            has_rule_hit: request.filters.has_rule_hit,
            conditions: request
                .filters
                .conditions
                .iter()
                .map(|condition| bifrost_command::FilterCondition {
                    field: condition.field.clone(),
                    operator: condition.operator.clone(),
                    value: condition.value.clone(),
                })
                .collect(),
            client_ips: request.filters.client_ips.clone(),
            client_apps: request.filters.client_apps.clone(),
            account_names: request.filters.account_names.clone(),
            domains: request.filters.domains.clone(),
        },
        cursor: request.cursor,
        limit: request.limit,
        max_scan: request.max_scan,
        max_results: request.max_results,
        record_ids: request.record_ids.clone(),
        time_range: request
            .time_range
            .as_ref()
            .map(|tr| bifrost_command::TimeRange {
                since_ms: tr.since_ms,
                until_ms: tr.until_ms,
            }),
        include: bifrost_command::SearchInclude {
            request_body: request.include.request_body,
            response_body: request.include.response_body,
            request_headers: request.include.request_headers,
            response_headers: request.include.response_headers,
            max_body_bytes: request.include.max_body_bytes,
        },
    }
}

fn validate_target_record_ids(request: &SearchRequest) -> Option<Response<BoxBody>> {
    if request.record_ids.len() > MAX_TARGET_RECORD_IDS {
        return Some(error_response(
            StatusCode::BAD_REQUEST,
            &format!("record_ids cannot contain more than {MAX_TARGET_RECORD_IDS} entries"),
        ));
    }

    if request
        .record_ids
        .iter()
        .any(|id| id.is_empty() || id.len() > 128)
    {
        return Some(error_response(
            StatusCode::BAD_REQUEST,
            "record_ids must contain non-empty IDs no longer than 128 bytes",
        ));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpListener;

    use crate::state::AdminState;

    async fn spawn_search_server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind search test listener");
        let addr = listener.local_addr().expect("search listener addr");
        let state = std::sync::Arc::new(AdminState::new(0));

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let io = TokioIo::new(stream);
                let state = state.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| {
                        let state = state.clone();
                        async move {
                            let path = req.uri().path().to_string();
                            let response = handle_search(req, state, &path).await;
                            Ok::<_, hyper::Error>(response)
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        (format!("http://{addr}"), handle)
    }

    #[test]
    fn target_record_ids_are_bounded_and_validated() {
        let too_many = SearchRequest {
            keyword: "marker".to_string(),
            record_ids: (0..=MAX_TARGET_RECORD_IDS)
                .map(|index| format!("id-{index}"))
                .collect(),
            ..Default::default()
        };
        assert_eq!(
            validate_target_record_ids(&too_many)
                .expect("too many IDs must fail")
                .status(),
            StatusCode::BAD_REQUEST
        );

        let invalid = SearchRequest {
            keyword: "marker".to_string(),
            record_ids: vec![String::new()],
            ..Default::default()
        };
        assert_eq!(
            validate_target_record_ids(&invalid)
                .expect("empty ID must fail")
                .status(),
            StatusCode::BAD_REQUEST
        );

        let valid = SearchRequest {
            keyword: "marker".to_string(),
            record_ids: vec!["id-a".to_string(), "id-b".to_string()],
            ..Default::default()
        };
        assert!(validate_target_record_ids(&valid).is_none());
    }

    #[test]
    fn web_search_conversion_preserves_account_names_and_record_ids() {
        let request = SearchRequest {
            keyword: "marker".to_string(),
            filters: crate::search::SearchFilters {
                account_names: vec!["alice".to_string()],
                ..Default::default()
            },
            record_ids: vec!["id-a".to_string()],
            ..Default::default()
        };

        let converted = command_search_args_from_request(&request);
        assert_eq!(converted.filters.account_names, vec!["alice"]);
        assert_eq!(converted.record_ids, vec!["id-a"]);
    }

    #[tokio::test]
    async fn regular_and_stream_search_reject_oversized_target_record_ids() {
        let (base, handle) = spawn_search_server().await;
        let request = serde_json::json!({
            "keyword": "marker",
            "record_ids": (0..=MAX_TARGET_RECORD_IDS)
                .map(|index| format!("id-{index}"))
                .collect::<Vec<_>>(),
        });
        let client = reqwest::Client::new();

        for path in ["/api/search", "/api/search/stream"] {
            let response = client
                .post(format!("{base}{path}"))
                .json(&request)
                .send()
                .await
                .expect("send oversized targeted search");
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert!(response
                .text()
                .await
                .expect("read validation response")
                .contains("record_ids cannot contain more than"));
        }

        handle.abort();
    }
}
