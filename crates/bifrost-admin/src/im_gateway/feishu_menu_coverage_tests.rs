use super::*;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;

async fn application_response(
    status: StatusCode,
    body: &'static str,
    request_id: Option<&'static str>,
) -> reqwest::Response {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind one-shot Feishu fixture");
    let address = listener.local_addr().expect("one-shot fixture address");
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept fixture request");
        let service = service_fn(move |_request: Request<Incoming>| async move {
            let mut response = Response::builder()
                .status(status)
                .header("Content-Type", "application/json")
                .header("Connection", "close");
            if let Some(request_id) = request_id {
                response = response.header("x-tt-logid", request_id);
            }
            Ok::<_, hyper::Error>(
                response
                    .body(Full::new(Bytes::copy_from_slice(body.as_bytes())))
                    .unwrap(),
            )
        });
        http1::Builder::new()
            .serve_connection(TokioIo::new(stream), service)
            .await
            .expect("serve one-shot Feishu response");
    });
    let response = reqwest::Client::new()
        .get(format!("http://{address}/response"))
        .header("Connection", "close")
        .send()
        .await
        .expect("request one-shot Feishu response");
    task.await.expect("one-shot Feishu fixture");
    response
}

async fn spawn_reconcile_fixture(
    failure_stage: Option<&'static str>,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind reconcile fixture");
    let address = listener.local_addr().expect("reconcile fixture address");
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| async move {
                    let path = request.uri().path();
                    let stage = if path.ends_with("/ability") {
                        Some("ability")
                    } else if path.ends_with("/config") {
                        Some("config")
                    } else {
                        None
                    };
                    let (status, body) = if path.ends_with("/auth/v3/tenant_access_token/internal")
                    {
                        (
                            StatusCode::OK,
                            r#"{"code":0,"tenant_access_token":"token","expire":7200}"#,
                        )
                    } else if failure_stage == stage {
                        (
                            StatusCode::CONFLICT,
                            r#"{"code":23,"msg":"application is under review"}"#,
                        )
                    } else {
                        (StatusCode::OK, r#"{"code":0,"data":{}}"#)
                    };
                    Ok::<_, hyper::Error>(
                        Response::builder()
                            .status(status)
                            .header("Content-Type", "application/json")
                            .body(Full::new(Bytes::copy_from_slice(body.as_bytes())))
                            .unwrap(),
                    )
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (format!("http://{address}/open-apis"), task)
}

fn menu_provider(base_url: String) -> ImProviderConfig {
    ImProviderConfig {
        id: "feishu-menu-coverage".to_string(),
        provider_type: ImProviderType::Feishu,
        display_name: "Feishu Menu".to_string(),
        enabled: true,
        base_url: Some(base_url),
        app_id: Some("cli_menu".to_string()),
        secret_ref: Some("app-secret".to_string()),
        owner_open_id: Some("ou_owner".to_string()),
        event_connection_enabled: true,
        event_types: Vec::new(),
        agent_config: None,
        created_at: 0,
        updated_at: 0,
    }
}

fn closed_loopback_url() -> String {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("reserve closed loopback address");
    let address = listener.local_addr().expect("closed loopback address");
    drop(listener);
    format!("http://{address}")
}

#[test]
fn menu_validation_covers_disabled_duplicate_root_and_child_boundaries() {
    let mut disabled = bifrost_default_menu();
    disabled.enabled = false;
    assert_eq!(validate_menu(&disabled), Ok(()));

    let mut duplicate = bifrost_default_menu();
    duplicate.nodes[1].menu_id = duplicate.nodes[0].menu_id.clone();
    assert!(validate_menu(&duplicate).unwrap_err().contains("unique"));

    let mut invalid_root = bifrost_default_menu();
    invalid_root.nodes[0].menu_content_type = MENU_CONTENT_EVENT;
    invalid_root.nodes[0].event_key = Some("bifrost.status".to_string());
    assert!(validate_menu(&invalid_root)
        .unwrap_err()
        .contains("must be a submenu"));

    let mut too_many_children = bifrost_default_menu();
    too_many_children
        .nodes
        .push(action("fifth", "session", 50, "Fifth", "bifrost.help"));
    too_many_children
        .nodes
        .push(action("sixth", "session", 60, "Sixth", "bifrost.help"));
    assert!(validate_menu(&too_many_children)
        .unwrap_err()
        .contains("more than 5 children"));

    let mut invalid_child = bifrost_default_menu();
    invalid_child.nodes[1].menu_content_type = MENU_CONTENT_SUBMENU;
    assert!(validate_menu(&invalid_child)
        .unwrap_err()
        .contains("must be an event action"));

    let mut missing_key = bifrost_default_menu();
    missing_key.nodes[1].event_key = None;
    assert!(validate_menu(&missing_key)
        .unwrap_err()
        .contains("has no event_key"));
}

#[test]
fn menu_event_rejects_wrong_type_and_empty_id_and_defaults_timestamp() {
    let event = |event_type: &str, event_id: &str| {
        serde_json::json!({
            "header": {"event_id": event_id, "event_type": event_type},
            "event": {
                "operator": {"operator_id": {"open_id": "ou_owner"}},
                "event_key": "bifrost.help"
            }
        })
    };
    assert!(
        normalize_feishu_menu_event(&event("im.message.receive_v1", "evt"), "feishu-main")
            .is_none()
    );
    assert!(
        normalize_feishu_menu_event(&event(FEISHU_BOT_MENU_EVENT, "   "), "feishu-main").is_none()
    );

    let before = current_timestamp_ms();
    let normalized =
        normalize_feishu_menu_event(&event(FEISHU_BOT_MENU_EVENT, "evt-now"), "feishu-main")
            .unwrap();
    assert!(normalized.received_at >= before);
    assert_eq!(
        normalized.message.unwrap().raw_content.unwrap()["timestamp"],
        serde_json::Value::Null
    );
}

#[test]
fn state_store_reports_atomic_replace_failure_without_publishing_memory() {
    let temp = tempfile::tempdir().unwrap();
    let store = FeishuMenuStateStore::new(temp.path());
    std::fs::create_dir_all(&store.path).expect("create directory at state file path");
    let state = FeishuMenuState {
        provider_id: "feishu-main".to_string(),
        status: FeishuMenuApplyStatus::DraftApplied,
        updated_at: 7,
        ..FeishuMenuState::default()
    };

    let error = store.save(state).unwrap_err();
    assert!(error.contains("replace Feishu menu state"));
    assert_eq!(store.get("feishu-main"), None);
}

#[test]
fn provider_publish_validation_and_error_display_are_actionable() {
    let provider = FeishuProvider::new();
    let temp = tempfile::tempdir().unwrap();
    let store = FeishuMenuStateStore::new(temp.path());
    let provisioner = FeishuAppProvisioner::new(&provider, &store);

    let mut invalid_type = menu_provider("https://open.feishu.cn/open-apis".to_string());
    invalid_type.provider_type = ImProviderType::Webhook;
    assert_eq!(
        provisioner.preview(&invalid_type).unwrap_err().error,
        "invalid_provider"
    );

    let mut missing_app_id = menu_provider("https://open.feishu.cn/open-apis".to_string());
    missing_app_id.app_id = Some("  ".to_string());
    assert_eq!(
        provisioner.preview(&missing_app_id).unwrap_err().error,
        "missing_app_credentials"
    );

    assert_eq!(
        validate_publish_options(&FeishuMenuSyncOptions {
            publish: true,
            mobile_default_ability: "calendar".to_string(),
            pc_default_ability: "bot".to_string(),
        })
        .unwrap_err()
        .error,
        "invalid_publish_options"
    );
    assert!(validate_publish_options(&FeishuMenuSyncOptions {
        publish: true,
        mobile_default_ability: "gadget".to_string(),
        pc_default_ability: "web_app".to_string(),
    })
    .is_ok());

    let display = FeishuProvisionError {
        error: "publish_failed".to_string(),
        stage: "publish".to_string(),
        message: "rejected".to_string(),
        feishu_code: Some(42),
        http_status: Some(409),
        request_id: Some("req-42".to_string()),
    }
    .to_string();
    assert!(display.contains("code=42"));
    assert!(display.contains("request_id=req-42"));
}

#[tokio::test]
async fn application_response_errors_cover_invalid_and_generic_failures() {
    let invalid = application_response(StatusCode::BAD_GATEWAY, "not-json", Some("bad-json")).await;
    let error = parse_application_response(invalid, "config")
        .await
        .unwrap_err();
    assert_eq!(error.error, "config_response_invalid");
    assert_eq!(error.http_status, Some(502));
    assert_eq!(error.request_id.as_deref(), Some("bad-json"));

    for (stage, expected) in [
        ("ability", "ability_update_failed"),
        ("config", "event_update_failed"),
        ("publish", "publish_failed"),
        ("other", "provision_failed"),
    ] {
        let response = application_response(
            StatusCode::BAD_REQUEST,
            r#"{"code":9001,"message":"permission denied"}"#,
            None,
        )
        .await;
        let error = parse_application_response(response, stage)
            .await
            .unwrap_err();
        assert_eq!(error.error, expected);
        assert_eq!(error.message, "permission denied");
        assert_eq!(error.feishu_code, Some(9001));
    }

    let no_message =
        application_response(StatusCode::INTERNAL_SERVER_ERROR, r#"{"code":0}"#, None).await;
    let error = parse_application_response(no_message, "other")
        .await
        .unwrap_err();
    assert_eq!(error.message, "unknown Feishu application error");
    assert_eq!(error.feishu_code, None);
}

#[tokio::test]
async fn provisioner_reports_missing_secret_token_and_request_transport_failures() {
    let provider = FeishuProvider::new();
    let temp = tempfile::tempdir().unwrap();
    let store = FeishuMenuStateStore::new(temp.path());
    let provisioner = FeishuAppProvisioner::new(&provider, &store);

    let mut missing_secret = menu_provider(closed_loopback_url());
    missing_secret.secret_ref = Some("  ".to_string());
    assert_eq!(
        provisioner
            .reconcile(&missing_secret, &FeishuMenuSyncOptions::default())
            .await
            .unwrap_err()
            .error,
        "missing_app_credentials"
    );

    let token_failure = menu_provider(closed_loopback_url());
    assert_eq!(
        provisioner
            .reconcile(&token_failure, &FeishuMenuSyncOptions::default())
            .await
            .unwrap_err()
            .error,
        "token_failed"
    );

    let body = serde_json::json!({"test": true});
    let patch_error = provisioner
        .patch(&closed_loopback_url(), "token", &body, "ability")
        .await
        .unwrap_err();
    assert_eq!(patch_error.error, "ability_request_failed");
    let post_error = provisioner
        .post(&closed_loopback_url(), "token", &body, "publish")
        .await
        .unwrap_err();
    assert_eq!(post_error.error, "publish_request_failed");
}

#[tokio::test]
async fn reconcile_records_config_failure_and_reports_state_persist_failure() {
    let provider = FeishuProvider::new();
    let (base_url, server) = spawn_reconcile_fixture(Some("config")).await;
    let provider_config = menu_provider(base_url);
    let temp = tempfile::tempdir().unwrap();
    let store = FeishuMenuStateStore::new(temp.path());
    let error = FeishuAppProvisioner::new(&provider, &store)
        .reconcile(&provider_config, &FeishuMenuSyncOptions::default())
        .await
        .unwrap_err();
    assert_eq!(error.error, "app_under_review");
    assert_eq!(error.stage, "config");
    assert_eq!(
        store.get(&provider_config.id).unwrap().status,
        FeishuMenuApplyStatus::UnderReview
    );
    server.abort();

    let (base_url, server) = spawn_reconcile_fixture(None).await;
    let provider_config = menu_provider(base_url);
    let blocked = tempfile::tempdir().unwrap();
    let store = FeishuMenuStateStore::new(blocked.path());
    std::fs::create_dir_all(&store.path).expect("block state-file replacement");
    let error = FeishuAppProvisioner::new(&provider, &store)
        .reconcile(&provider_config, &FeishuMenuSyncOptions::default())
        .await
        .unwrap_err();
    assert_eq!(error.error, "state_persist_failed");
    assert_eq!(error.stage, "persist");
    assert_eq!(store.get(&provider_config.id), None);
    server.abort();
}
