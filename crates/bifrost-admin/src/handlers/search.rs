use std::time::Duration;

use bifrost_command::SearchArgs;
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use tokio_stream::StreamExt;

use super::{error_response, json_response, method_not_allowed, BoxBody};
use crate::query_service::AdminQueryService;
use crate::search::{SearchProgress, SearchRequest};
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
    search_id: String,
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
                        let _ = tx_results.blocking_send(Bytes::from(sse_event("result", &json)));
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
                        let _ =
                            tx_progress.blocking_send(Bytes::from(sse_event("progress", &json)));
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
                    search_id: response.search_id,
                };
                if let Ok(json) = serde_json::to_string(&done) {
                    let _ = tx.blocking_send(Bytes::from(sse_event("done", &json)));
                }
            }
            Err(error) => {
                let payload = SearchStreamErrorPayload {
                    message: error.to_string(),
                };
                if let Ok(json) = serde_json::to_string(&payload) {
                    let _ = tx.blocking_send(Bytes::from(sse_event("error", &json)));
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
            domains: request.filters.domains.clone(),
        },
        cursor: request.cursor,
        limit: request.limit,
        max_scan: request.max_scan,
        max_results: request.max_results,
    }
}
