use hyper::{Method, Request, Response, StatusCode};

use super::{error_response, json_response, method_not_allowed, BoxBody};
use crate::state::SharedAdminState;

pub async fn handle_diagnostics<B>(
    req: Request<B>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    match path {
        "/api/diagnostics/process-resolver" => match *req.method() {
            Method::GET => match state.process_resolver_diagnostics() {
                Some(diagnostics) => json_response(&diagnostics.snapshot()),
                None => error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Process resolver diagnostics are not configured",
                ),
            },
            _ => method_not_allowed(),
        },
        _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use http_body_util::BodyExt;
    use hyper::Request;

    use super::*;
    use crate::state::AdminState;

    #[tokio::test]
    async fn process_resolver_diagnostics_returns_shared_snapshot() {
        let state = Arc::new(AdminState::new(0));
        let diagnostics = Arc::new(bifrost_core::ProcessResolverDiagnostics::default());
        diagnostics.record_lookup_result(true);
        diagnostics.record_snapshot_refresh(25, 4, 30, false);
        state.set_process_resolver_diagnostics(diagnostics);

        let request = Request::builder()
            .method(Method::GET)
            .uri("/_bifrost/api/diagnostics/process-resolver")
            .body(())
            .unwrap();
        let response =
            handle_diagnostics(request, state, "/api/diagnostics/process-resolver").await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let snapshot: bifrost_core::ProcessResolverDiagnosticsSnapshot =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(snapshot.lookup_requests_total, 1);
        assert_eq!(snapshot.snapshot_refreshes_total, 1);
        assert_eq!(snapshot.scanned_pids_total, 4);
    }

    #[tokio::test]
    async fn process_resolver_diagnostics_rejects_writes_and_missing_state() {
        let state = Arc::new(AdminState::new(0));
        let post = Request::builder()
            .method(Method::POST)
            .uri("/_bifrost/api/diagnostics/process-resolver")
            .body(())
            .unwrap();
        assert_eq!(
            handle_diagnostics(
                post,
                Arc::clone(&state),
                "/api/diagnostics/process-resolver"
            )
            .await
            .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );

        let get = Request::builder()
            .method(Method::GET)
            .uri("/_bifrost/api/diagnostics/process-resolver")
            .body(())
            .unwrap();
        assert_eq!(
            handle_diagnostics(get, state, "/api/diagnostics/process-resolver")
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
