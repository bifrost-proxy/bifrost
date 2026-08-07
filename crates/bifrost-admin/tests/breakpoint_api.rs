use bifrost_admin::breakpoint::{PendingBreakpoint, SharedBreakpointManager};
use bifrost_admin::{AdminRouter, AdminState, PushManager};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use std::sync::Arc;
use tokio::net::TcpListener;

fn pending(request_id: &str, phase: &str) -> PendingBreakpoint {
    PendingBreakpoint {
        request_id: request_id.to_string(),
        phase: phase.to_string(),
        method: (phase == "request").then(|| "POST".to_string()),
        url: (phase == "request").then(|| "http://example.test/".to_string()),
        status: (phase == "response").then_some(200),
        headers: vec![],
        body: Some("old".to_string()),
        body_omitted: false,
        body_size: Some(3),
        max_body_bytes: 1024,
        content_encoding: None,
        paused_at_ms: 1,
        deadline_at_ms: 2,
    }
}

async fn start_admin() -> (String, SharedBreakpointManager, tokio::task::JoinHandle<()>) {
    let state = Arc::new(AdminState::new(0));
    let manager = state.breakpoint_manager.clone();
    let push_manager = Arc::new(PushManager::new(state.clone()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                return;
            };
            let state = state.clone();
            let push_manager = push_manager.clone();
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| {
                    let state = state.clone();
                    let push_manager = push_manager.clone();
                    async move {
                        Ok::<_, hyper::Error>(
                            AdminRouter::handle(request, state, Some(push_manager), Some(peer))
                                .await,
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (format!("http://{address}/_bifrost"), manager, task)
}

async fn resume(client: &reqwest::Client, base: &str, json: &str) -> StatusCode {
    client
        .post(format!("{base}/api/breakpoint/resume"))
        .header("content-type", "application/json")
        .body(json.to_string())
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn breakpoint_routes_cover_settings_pending_and_errors() {
    let (base, _manager, server) = start_admin().await;
    let client = reqwest::Client::new();
    for (method, path, expected) in [
        (Method::GET, "/api/breakpoint/settings", StatusCode::OK),
        (
            Method::PUT,
            "/api/breakpoint/settings",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            Method::GET,
            "/api/breakpoint/resume",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (Method::GET, "/api/breakpoint/pending", StatusCode::OK),
        (
            Method::POST,
            "/api/breakpoint/pending",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            Method::GET,
            "/api/breakpoint/missing",
            StatusCode::NOT_FOUND,
        ),
    ] {
        let response = client
            .request(method, format!("{base}{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{path}");
    }
    server.abort();
}

#[tokio::test]
async fn breakpoint_settings_and_resume_validate_bodies_and_apply_edits() {
    let (base, manager, server) = start_admin().await;
    let client = reqwest::Client::new();
    let settings = client
        .post(format!("{base}/api/breakpoint/settings"))
        .json(&serde_json::json!({"enabled": true, "max_body_bytes": 99}))
        .send()
        .await
        .unwrap();
    assert_eq!(settings.status(), StatusCode::OK);
    assert!(manager.is_enabled());
    assert_eq!(manager.max_body_bytes(), 99);

    for json in [
        "not-json",
        r#"{"request_id":"id","phase":"request","headers":"bad"}"#,
        r#"{}"#,
        r#"{"request_id":"","phase":"request"}"#,
        r#"{"request_id":"id","phase":"other"}"#,
        r#"{"request_id":"id","phase":"request","headers":[["bad header","x"]]}"#,
        "{\"request_id\":\"id\",\"phase\":\"request\",\"headers\":[[\"x-ok\",\"bad\\nvalue\"]]}",
        r#"{"request_id":"id","phase":"request","status":200}"#,
        r#"{"request_id":"id","phase":"request","method":"bad method"}"#,
        r#"{"request_id":"id","phase":"request","url":"http://["}"#,
        r#"{"request_id":"id","phase":"request","url":"/relative"}"#,
        r#"{"request_id":"id","phase":"response","method":"GET"}"#,
        r#"{"request_id":"id","phase":"response","status":99}"#,
    ] {
        assert_eq!(
            resume(&client, &base, json).await,
            StatusCode::BAD_REQUEST,
            "{json}"
        );
    }
    assert_eq!(
        resume(
            &client,
            &base,
            r#"{"request_id":"missing","phase":"request"}"#
        )
        .await,
        StatusCode::NOT_FOUND
    );

    let request_rx = manager.pause_request(pending("edit", "request"), true);
    assert_eq!(
        resume(
            &client,
            &base,
            r#"{"request_id":"edit","phase":"request","method":"PUT","url":"https://example.test/new","headers":[["x-one","1"]],"body":"new"}"#,
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(request_rx.await.unwrap().method.as_deref(), Some("PUT"));

    let _phase_rx = manager.pause_request(pending("phase", "request"), true);
    assert_eq!(
        resume(
            &client,
            &base,
            r#"{"request_id":"phase","phase":"response"}"#
        )
        .await,
        StatusCode::CONFLICT
    );

    let response_rx = manager.pause_response(pending("response-edit", "response"), true);
    assert_eq!(
        resume(
            &client,
            &base,
            r#"{"request_id":"response-edit","phase":"response","status":418,"headers":[["set-cookie","a=1"],["set-cookie","b=2"]],"body":"teapot"}"#,
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(response_rx.await.unwrap().status, Some(418));
    server.abort();
}

#[tokio::test]
async fn breakpoint_settings_reject_invalid_json() {
    let (base, _manager, server) = start_admin().await;
    let response = reqwest::Client::new()
        .post(format!("{base}/api/breakpoint/settings"))
        .header("content-type", "application/json")
        .body("not-json")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    server.abort();
}
