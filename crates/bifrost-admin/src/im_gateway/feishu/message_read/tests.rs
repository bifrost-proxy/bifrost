use super::*;
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::sync::Arc;

async fn spawn_message_api(reply_body: &'static str) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback Feishu message fixture");
    let address = listener.local_addr().expect("fixture address");
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| async move {
                    let body = if request
                        .uri()
                        .path()
                        .ends_with("/auth/v3/tenant_access_token/internal")
                    {
                        r#"{"code":0,"tenant_access_token":"token","expire":7200}"#
                    } else {
                        reply_body
                    };
                    Ok::<_, hyper::Error>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("Content-Type", "application/json")
                            .body(Full::new(Bytes::from_static(body.as_bytes())))
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

fn config(base_url: String) -> ImProviderConfig {
    ImProviderConfig {
        id: "feishu-main".to_string(),
        provider_type: ImProviderType::Feishu,
        display_name: "Feishu".to_string(),
        enabled: true,
        base_url: Some(base_url),
        app_id: Some("cli_test".to_string()),
        secret_ref: Some("secret".to_string()),
        owner_open_id: None,
        event_connection_enabled: true,
        event_types: Vec::new(),
        agent_config: None,
        created_at: 0,
        updated_at: 0,
    }
}

#[tokio::test]
async fn fetch_message_reads_original_interactive_card_content() {
    let (base_url, task) = spawn_message_api(
        r#"{"code":0,"data":{"items":[{"message_id":"om_card","chat_id":"oc_group","msg_type":"interactive","sender":{"id":"ou_other_bot","sender_type":"app"},"body":{"content":"{\"schema\":\"2.0\",\"header\":{\"title\":{\"tag\":\"plain_text\",\"content\":\"分析结果\"}},\"body\":{\"elements\":[{\"tag\":\"markdown\",\"content\":\"测试全部通过\"}]}}"},"create_time":"1710000000000"}]}}"#,
    )
    .await;
    let message = FeishuProvider::new()
        .fetch_message(&config(base_url), "om_card")
        .await
        .expect("fetch referenced card");
    assert_eq!(message.chat_id, "oc_group");
    assert_eq!(message.sender_type.as_deref(), Some("app"));
    assert_eq!(message.text, "分析结果\n测试全部通过");
    task.abort();
}

#[tokio::test]
async fn fetch_message_restores_mentions_from_rest_string_ids() {
    let (base_url, task) = spawn_message_api(
        r#"{"code":0,"data":{"items":[{"message_id":"om_text","chat_id":"oc_group","msg_type":"text","sender":{"id":"ou_sender","sender_type":"user"},"mentions":[{"key":"@_user_1","id":"ou_alice","id_type":"open_id","name":"Alice"}],"body":{"content":"{\"text\":\"@_user_1 please review\"}"}}]}}"#,
    )
    .await;
    let message = FeishuProvider::new()
        .fetch_message(&config(base_url), "om_text")
        .await
        .expect("fetch referenced text mention");
    assert_eq!(message.text, "@_user_1 please review");
    assert_eq!(message.mentions.len(), 1);
    assert_eq!(message.mentions[0].open_id.as_deref(), Some("ou_alice"));
    assert_eq!(message.mentions[0].name.as_deref(), Some("Alice"));
    task.abort();
}

#[tokio::test]
async fn fetch_message_permission_error_explains_required_scopes_and_publish_step() {
    let (base_url, task) =
        spawn_message_api(r#"{"code":230027,"msg":"Lack of necessary permissions"}"#).await;
    let error = FeishuProvider::new()
        .fetch_message(&config(base_url), "om_denied")
        .await
        .expect_err("missing read scope must fail")
        .to_string();
    assert!(error.contains("im:message:readonly"));
    assert!(error.contains("im:message.group_msg"));
    assert!(error.contains("创建并发布新版本"));
    task.abort();
}

#[tokio::test]
async fn fetch_message_covers_validation_api_parse_and_not_found_errors() {
    let provider = FeishuProvider::new();
    let closed = config("http://127.0.0.1:9/open-apis".to_string());
    assert!(provider
        .fetch_message(&closed, "  ")
        .await
        .expect_err("blank message id")
        .to_string()
        .contains("message_id is empty"));

    for (body, expected) in [
        ("not-json", "response parse failed"),
        (r#"{"code":42,"msg":"remote failure"}"#, "code=42"),
        (r#"{"code":0,"data":{"items":[]}}"#, "not found"),
    ] {
        let body = Box::leak(body.to_string().into_boxed_str());
        let (base_url, task) = spawn_message_api(body).await;
        let error = provider
            .fetch_message(&config(base_url), "om_error")
            .await
            .expect_err("fixture must fail")
            .to_string();
        assert!(error.contains(expected), "unexpected error: {error}");
        task.abort();
    }

    let error = provider
        .fetch_message(&closed, "om_closed")
        .await
        .expect_err("closed loopback endpoint must fail")
        .to_string();
    assert!(
        error.contains("token request failed")
            || error.contains("referenced message request failed")
    );
}

#[test]
fn message_read_permission_detection_and_help_cover_text_and_missing_app() {
    assert!(is_message_read_permission_error(0, "permission denied"));
    assert!(is_message_read_permission_error(0, "缺少权限"));
    assert!(is_message_read_permission_error(0, "scope missing"));
    assert!(!is_message_read_permission_error(0, "not found"));
    let help = message_read_permission_help(None);
    assert!(!help.contains("App ID"));
    assert!(help.contains("im:message:readonly"));
}

#[tokio::test]
async fn fetch_message_retries_cardkit_id_with_visible_card_representation() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let server_reads = Arc::clone(&reads);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let reads = Arc::clone(&server_reads);
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| {
                    let reads = Arc::clone(&reads);
                    async move {
                        let body = if request
                            .uri()
                            .path()
                            .ends_with("/auth/v3/tenant_access_token/internal")
                        {
                            r#"{"code":0,"tenant_access_token":"token","expire":7200}"#
                        } else if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                            assert!(request
                                .uri()
                                .query()
                                .unwrap_or_default()
                                .contains("card_msg_content_type=user_card_content"));
                            r#"{"code":0,"data":{"items":[{"message_id":"om_cardkit","chat_id":"oc_group","msg_type":"interactive","body":{"content":"{\"card_id\":\"AAq9card\"}"}}]}}"#
                        } else {
                            assert!(!request
                                .uri()
                                .query()
                                .unwrap_or_default()
                                .contains("card_msg_content_type"));
                            r#"{"code":0,"data":{"items":[{"message_id":"om_cardkit","chat_id":"oc_group","msg_type":"interactive","body":{"content":"{\"schema\":\"2.0\",\"body\":{\"elements\":[{\"tag\":\"markdown\",\"content\":\"CardKit 最终结论\"}]}}"}}]}}"#
                        };
                        Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from_static(
                            body.as_bytes(),
                        ))))
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    let message = FeishuProvider::new()
        .fetch_message(&config(format!("http://{address}/open-apis")), "om_cardkit")
        .await
        .expect("fetch visible CardKit representation");
    assert_eq!(message.text, "CardKit 最终结论");
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    task.abort();
}

#[test]
fn card_text_extraction_keeps_visible_content_and_skips_actions() {
    let card = serde_json::json!({
        "schema": "2.0",
        "header": {"title": {"tag": "plain_text", "content": "分析结果"}},
        "body": {"elements": [
            {"tag": "markdown", "content": "结论：选择方案 A"},
            {"tag": "button", "text": {"tag": "plain_text", "content": "确认"}, "url": "https://secret.example"},
            {"tag": "collapsible_panel", "header": {"title": {"tag": "plain_text", "content": "证据"}}, "elements": [
                {"tag": "markdown", "content": "测试全部通过"}
            ]}
        ]}
    });
    let text = extract_card_text(&card);
    assert_eq!(text, "分析结果\n结论：选择方案 A\n证据\n测试全部通过");
    assert!(!text.contains("secret.example"));
}

#[test]
fn message_read_permission_help_is_actionable() {
    assert!(is_message_read_permission_error(
        230027,
        "Lack of necessary permissions"
    ));
    let help = message_read_permission_help(Some("cli_test"));
    assert!(help.contains("cli_test"));
    assert!(help.contains("im:message:readonly"));
    assert!(help.contains("im:message.group_msg"));
    assert!(help.contains("创建并发布新版本"));
}
