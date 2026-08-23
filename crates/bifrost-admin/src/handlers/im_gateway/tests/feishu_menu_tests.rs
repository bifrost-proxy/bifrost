use super::*;

use http_body_util::BodyExt;

#[derive(Clone, Debug)]
struct CapturedMenuRequest {
    method: String,
    path: String,
    authorization: Option<String>,
    body: serde_json::Value,
}

async fn spawn_feishu_menu_api(
    fail_publish: bool,
) -> (
    String,
    Arc<tokio::sync::Mutex<Vec<CapturedMenuRequest>>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake Feishu application API");
    let address = listener.local_addr().expect("fake Feishu address");
    let captured = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let server_captured = Arc::clone(&captured);
    let server = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let captured = Arc::clone(&server_captured);
            tokio::spawn(async move {
                let handler = service_fn(move |request: Request<Incoming>| {
                    let captured = Arc::clone(&captured);
                    async move {
                        let method = request.method().to_string();
                        let path = request.uri().path().to_string();
                        let authorization = request
                            .headers()
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string);
                        let bytes = request.into_body().collect().await?.to_bytes();
                        let body = if bytes.is_empty() {
                            serde_json::Value::Null
                        } else {
                            serde_json::from_slice(&bytes).expect("menu request JSON")
                        };
                        captured.lock().await.push(CapturedMenuRequest {
                            method,
                            path: path.clone(),
                            authorization,
                            body,
                        });

                        let (status, body, request_id) =
                            if path.ends_with("/auth/v3/tenant_access_token/internal") {
                                (
                                    StatusCode::OK,
                                    serde_json::json!({
                                        "code": 0,
                                        "tenant_access_token": "menu-token",
                                        "expire": 7200
                                    }),
                                    "token-request",
                                )
                            } else if path.contains("/applications/cli_menu_failure/")
                                && path.ends_with("/ability")
                            {
                                (
                                    StatusCode::BAD_GATEWAY,
                                    serde_json::json!({
                                        "code": 54321,
                                        "msg": "ability update rejected"
                                    }),
                                    "ability-failed-request",
                                )
                            } else if path.ends_with("/publish") && fail_publish {
                                (
                                    StatusCode::CONFLICT,
                                    serde_json::json!({
                                        "code": 12345,
                                        "msg": "unsupported PersonalAgent app type"
                                    }),
                                    "publish-failed-request",
                                )
                            } else if path.ends_with("/publish") {
                                (
                                    StatusCode::OK,
                                    serde_json::json!({
                                        "code": 0,
                                        "data": {
                                            "version_id": "version-id",
                                            "version": "1.0.1"
                                        }
                                    }),
                                    "publish-request",
                                )
                            } else if path.ends_with("/ability") {
                                (
                                    StatusCode::OK,
                                    serde_json::json!({"code": 0, "data": {}}),
                                    "ability-request",
                                )
                            } else if path.ends_with("/config") {
                                (
                                    StatusCode::OK,
                                    serde_json::json!({"code": 0, "data": {}}),
                                    "config-request",
                                )
                            } else {
                                (
                                    StatusCode::NOT_FOUND,
                                    serde_json::json!({"code": 404, "msg": "not found"}),
                                    "not-found",
                                )
                            };
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(status)
                                .header("content-type", "application/json")
                                .header("x-tt-logid", request_id)
                                .body(Full::new(Bytes::from(body.to_string())))
                                .expect("fake Feishu response"),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), handler)
                    .await;
            });
        }
    });
    (format!("http://{address}/open-apis"), captured, server)
}

fn menu_provider(base_url: String, id: &str) -> ImProviderConfig {
    let mut provider = test_provider();
    provider.id = id.to_string();
    provider.base_url = Some(base_url);
    provider.app_id = Some("cli_menu_http".to_string());
    provider.secret_ref = Some("menu-secret".to_string());
    provider.owner_open_id = Some("ou_owner".to_string());
    provider
}

async fn menu_request(
    service: SharedImGatewayService,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> reqwest::Response {
    let (address, server) = spawn_im_gateway_http(service).await;
    let mut request = reqwest::Client::new()
        .request(method, format!("http://{address}{path}"))
        .header("connection", "close");
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.expect("Feishu menu admin request");
    server.await.expect("Feishu menu admin server");
    response
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn feishu_menu_admin_api_previews_syncs_publishes_and_reports_status() {
    let _test_guard = IM_GATEWAY_TEST_ENV_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let _loopback_guard = EnvVarGuard::set("BIFROST_E2E_ALLOW_FEISHU_LOOPBACK_BASE_URL", "1");
    let temp = tempfile::tempdir().expect("temp data dir");
    let _data_guard = EnvGuard::set_data_dir(temp.path());
    let (base_url, captured, feishu_server) = spawn_feishu_menu_api(false).await;
    let service = Arc::new(ImGatewayService::new(temp.path()));
    service
        .provider_store
        .add(menu_provider(base_url, "feishu-menu"))
        .expect("save Feishu menu provider");
    let base_path = "/api/im-gateway/providers/feishu-menu/feishu/menu";

    let preview = menu_request(
        Arc::clone(&service),
        reqwest::Method::GET,
        &format!("{base_path}/preview"),
        None,
    )
    .await;
    assert_eq!(preview.status(), reqwest::StatusCode::OK);
    let preview: serde_json::Value = preview.json().await.expect("preview JSON");
    assert_eq!(preview["preset"], "bifrost-default-v1");
    assert_eq!(
        preview["ability"]["bot"]["bot_menus"]
            .as_array()
            .unwrap()
            .len(),
        13
    );
    assert_eq!(captured.lock().await.len(), 0, "preview must be local");

    let initial_status = menu_request(
        Arc::clone(&service),
        reqwest::Method::GET,
        &format!("{base_path}/status"),
        None,
    )
    .await;
    assert_eq!(initial_status.status(), reqwest::StatusCode::OK);
    let initial_status: serde_json::Value = initial_status.json().await.expect("status JSON");
    assert_eq!(initial_status["state"]["status"], "not_applied");
    assert_eq!(captured.lock().await.len(), 0, "status must be local");

    let draft = menu_request(
        Arc::clone(&service),
        reqwest::Method::POST,
        &format!("{base_path}/sync"),
        Some(serde_json::json!({"publish": false})),
    )
    .await;
    assert_eq!(draft.status(), reqwest::StatusCode::OK);
    let draft: serde_json::Value = draft.json().await.expect("draft JSON");
    assert_eq!(draft["ability_updated"], true);
    assert_eq!(draft["event_subscription_updated"], true);
    assert_eq!(draft["published"], false);

    let publish = menu_request(
        Arc::clone(&service),
        reqwest::Method::POST,
        &format!("{base_path}/sync"),
        Some(serde_json::json!({"publish": true})),
    )
    .await;
    assert_eq!(publish.status(), reqwest::StatusCode::OK);
    let publish: serde_json::Value = publish.json().await.expect("publish JSON");
    assert_eq!(publish["ability_updated"], false);
    assert_eq!(publish["event_subscription_updated"], false);
    assert_eq!(publish["published"], true);
    assert_eq!(publish["version_id"], "version-id");

    let skipped = menu_request(
        Arc::clone(&service),
        reqwest::Method::POST,
        &format!("{base_path}/sync"),
        Some(serde_json::json!({"publish": true})),
    )
    .await;
    assert_eq!(skipped.status(), reqwest::StatusCode::OK);
    let skipped: serde_json::Value = skipped.json().await.expect("skipped JSON");
    assert_eq!(skipped["skipped"], true);

    let status = menu_request(
        Arc::clone(&service),
        reqwest::Method::GET,
        &format!("{base_path}/status"),
        None,
    )
    .await;
    let status: serde_json::Value = status.json().await.expect("applied status JSON");
    assert_eq!(status["state"]["status"], "published");
    assert_eq!(status["state"]["version_id"], "version-id");

    let requests = captured.lock().await.clone();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests[0].path,
        "/open-apis/auth/v3/tenant_access_token/internal"
    );
    assert_eq!(requests[1].method, "PATCH");
    assert!(requests[1].path.ends_with("/ability"));
    assert_eq!(
        requests[1].authorization.as_deref(),
        Some("Bearer menu-token")
    );
    assert_eq!(requests[1].body["bot"]["bot_menu_enable"], true);
    assert_eq!(requests[2].method, "PATCH");
    assert!(requests[2].path.ends_with("/config"));
    assert_eq!(
        requests[2].body,
        serde_json::json!({"event": {"add_events": ["application.bot.menu_v6"]}})
    );
    assert_eq!(requests[3].method, "POST");
    assert!(requests[3].path.ends_with("/publish"));
    assert_eq!(requests[3].body["mobile_default_ability"], "bot");
    assert_eq!(requests[3].body["pc_default_ability"], "bot");
    feishu_server.abort();
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn feishu_menu_admin_api_enforces_provider_method_and_error_boundaries() {
    let _test_guard = IM_GATEWAY_TEST_ENV_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let _loopback_guard = EnvVarGuard::set("BIFROST_E2E_ALLOW_FEISHU_LOOPBACK_BASE_URL", "1");
    let temp = tempfile::tempdir().expect("temp data dir");
    let _data_guard = EnvGuard::set_data_dir(temp.path());
    let (base_url, captured, feishu_server) = spawn_feishu_menu_api(true).await;
    let service = Arc::new(ImGatewayService::new(temp.path()));
    service
        .provider_store
        .add(menu_provider(base_url, "feishu-error"))
        .expect("save Feishu provider");
    let mut webhook = test_provider();
    webhook.id = "webhook-menu".to_string();
    webhook.provider_type = ImProviderType::Webhook;
    service.provider_store.add(webhook).expect("save webhook");

    let missing = menu_request(
        Arc::clone(&service),
        reqwest::Method::GET,
        "/api/im-gateway/providers/missing/feishu/menu/preview",
        None,
    )
    .await;
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);

    let invalid_provider = menu_request(
        Arc::clone(&service),
        reqwest::Method::GET,
        "/api/im-gateway/providers/webhook-menu/feishu/menu/preview",
        None,
    )
    .await;
    assert_eq!(invalid_provider.status(), reqwest::StatusCode::BAD_REQUEST);
    let invalid_provider: serde_json::Value = invalid_provider.json().await.unwrap();
    assert_eq!(invalid_provider["error"], "invalid_provider");

    let wrong_method = menu_request(
        Arc::clone(&service),
        reqwest::Method::POST,
        "/api/im-gateway/providers/feishu-error/feishu/menu/preview",
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(
        wrong_method.status(),
        reqwest::StatusCode::METHOD_NOT_ALLOWED
    );

    let invalid_options = menu_request(
        Arc::clone(&service),
        reqwest::Method::POST,
        "/api/im-gateway/providers/feishu-error/feishu/menu/sync",
        Some(serde_json::json!({
            "publish": true,
            "mobile_default_ability": "arbitrary"
        })),
    )
    .await;
    assert_eq!(invalid_options.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(
        captured.lock().await.len(),
        0,
        "validation must precede I/O"
    );

    let publish = menu_request(
        Arc::clone(&service),
        reqwest::Method::POST,
        "/api/im-gateway/providers/feishu-error/feishu/menu/sync",
        Some(serde_json::json!({"publish": true})),
    )
    .await;
    assert_eq!(publish.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let publish: serde_json::Value = publish.json().await.expect("publish error JSON");
    assert_eq!(publish["error"], "unsupported_app_type");
    assert_eq!(publish["stage"], "publish");
    assert_eq!(publish["request_id"], "publish-failed-request");
    assert_eq!(captured.lock().await.len(), 4);
    feishu_server.abort();
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn feishu_menu_handler_covers_invalid_json_unknown_action_and_connect_results() {
    let _test_guard = IM_GATEWAY_TEST_ENV_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let _loopback_guard = EnvVarGuard::set("BIFROST_E2E_ALLOW_FEISHU_LOOPBACK_BASE_URL", "1");
    let temp = tempfile::tempdir().expect("temp data dir");
    let _data_guard = EnvGuard::set_data_dir(temp.path());
    let (base_url, _captured, feishu_server) = spawn_feishu_menu_api(false).await;
    let service = Arc::new(ImGatewayService::new(temp.path()));
    let provider = menu_provider(base_url, "feishu-handler");
    service
        .provider_store
        .add(provider.clone())
        .expect("save Feishu provider");

    let invalid_json = menu_request(
        Arc::clone(&service),
        reqwest::Method::POST,
        "/api/im-gateway/providers/feishu-handler/feishu/menu/sync",
        Some(serde_json::json!({"publish": "yes"})),
    )
    .await;
    assert_eq!(invalid_json.status(), reqwest::StatusCode::BAD_REQUEST);

    let draft = reconcile_feishu_menu_for_connect(&service, &provider, false, "unit").await;
    assert_eq!(draft["success"], true);
    assert_eq!(draft["result"]["published"], false);

    let mut webhook = test_provider();
    webhook.provider_type = ImProviderType::Webhook;
    assert_eq!(
        reconcile_feishu_menu_for_connect(&service, &webhook, false, "unit").await,
        serde_json::Value::Null
    );

    let mut invalid = provider;
    invalid.app_id = None;
    let failed = reconcile_feishu_menu_for_connect(&service, &invalid, false, "unit").await;
    assert_eq!(failed["success"], false);
    assert_eq!(failed["error"]["error"], "missing_app_credentials");
    feishu_server.abort();
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn startup_auto_connect_reconciles_historical_feishu_menu_once() {
    let _test_guard = IM_GATEWAY_TEST_ENV_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let _loopback_guard = EnvVarGuard::set("BIFROST_E2E_ALLOW_FEISHU_LOOPBACK_BASE_URL", "1");
    let temp = tempfile::tempdir().expect("temp data dir");
    let _data_guard = EnvGuard::set_data_dir(temp.path());
    let (base_url, captured, feishu_server) = spawn_feishu_menu_api(false).await;
    let provider = menu_provider(base_url, "feishu-startup-menu");

    let first_service = Arc::new(ImGatewayService::new(temp.path()));
    first_service
        .provider_store
        .add(provider.clone())
        .expect("persist historical Feishu provider");
    first_service.auto_connect_providers().await;

    let state = first_service
        .feishu_menu_state_store
        .get(&provider.id)
        .expect("startup reconcile state");
    assert_eq!(
        state.status,
        crate::im_gateway::feishu_menu::FeishuMenuApplyStatus::DraftApplied
    );
    let first_requests = captured.lock().await.clone();
    assert_eq!(
        first_requests
            .iter()
            .filter(|request| request.path.ends_with("/ability"))
            .count(),
        1
    );
    assert_eq!(
        first_requests
            .iter()
            .filter(|request| request.path.ends_with("/config"))
            .count(),
        1
    );
    assert!(
        first_requests
            .iter()
            .all(|request| !request.path.ends_with("/publish")),
        "startup recovery must never publish an imported application"
    );
    first_service
        .connection_manager
        .stop_connection_and_wait(&provider.id)
        .await;

    // Recreate the service from the same data directory to model a real
    // process restart. The persisted desired digest must suppress another
    // ability/config PATCH while the long connection is still restored.
    let restarted_service = Arc::new(ImGatewayService::new(temp.path()));
    restarted_service.auto_connect_providers().await;
    let second_requests = captured.lock().await.clone();
    assert_eq!(
        second_requests
            .iter()
            .filter(|request| request.path.ends_with("/ability"))
            .count(),
        1
    );
    assert_eq!(
        second_requests
            .iter()
            .filter(|request| request.path.ends_with("/config"))
            .count(),
        1
    );
    assert!(restarted_service
        .connection_manager
        .get_status(&provider.id)
        .is_some());
    restarted_service
        .connection_manager
        .stop_connection_and_wait(&provider.id)
        .await;
    feishu_server.abort();
}

#[tokio::test(flavor = "current_thread")]
pub(super) async fn startup_menu_reconcile_failure_does_not_block_connection_restore() {
    let _test_guard = IM_GATEWAY_TEST_ENV_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let _loopback_guard = EnvVarGuard::set("BIFROST_E2E_ALLOW_FEISHU_LOOPBACK_BASE_URL", "1");
    let temp = tempfile::tempdir().expect("temp data dir");
    let _data_guard = EnvGuard::set_data_dir(temp.path());
    let (base_url, captured, feishu_server) = spawn_feishu_menu_api(false).await;
    let mut provider = menu_provider(base_url, "feishu-startup-menu-failure");
    provider.app_id = Some("cli_menu_failure".to_string());
    let service = Arc::new(ImGatewayService::new(temp.path()));
    service
        .provider_store
        .add(provider.clone())
        .expect("persist historical Feishu provider");

    service.auto_connect_providers().await;

    let state = service
        .feishu_menu_state_store
        .get(&provider.id)
        .expect("failed startup reconcile state");
    assert_eq!(
        state.status,
        crate::im_gateway::feishu_menu::FeishuMenuApplyStatus::Failed
    );
    assert_eq!(
        state.last_error_kind.as_deref(),
        Some("ability_update_failed")
    );
    assert!(
        captured
            .lock()
            .await
            .iter()
            .any(|request| request.path.ends_with("/ability")),
        "startup must attempt menu provisioning before restoring transport"
    );
    assert!(
        service
            .connection_manager
            .get_status(&provider.id)
            .is_some(),
        "menu provisioning failure must not prevent transport restoration"
    );
    service
        .connection_manager
        .stop_connection_and_wait(&provider.id)
        .await;
    feishu_server.abort();
}

#[test]
pub(super) fn reconnect_supervisor_does_not_reconcile_feishu_menu() {
    let service_source = include_str!("../service.rs");
    let supervisor = service_source
        .split_once("pub(super) fn spawn_reconnect_supervisor")
        .expect("reconnect supervisor definition")
        .1
        .split_once("fn queued_enabled_schedule")
        .expect("reconnect supervisor boundary")
        .0;
    assert!(
        !supervisor.contains("reconcile_feishu_menu_for_connect"),
        "transport reconnects must not mutate Feishu application configuration"
    );
}
