use super::*;

fn test_provider() -> ImProviderConfig {
    ImProviderConfig {
        id: "weixin-main".to_string(),
        provider_type: ImProviderType::Weixin,
        display_name: "Weixin Main".to_string(),
        enabled: true,
        base_url: Some("http://127.0.0.1:12345/".to_string()),
        app_id: Some("mock-bot@im.bot".to_string()),
        secret_ref: Some("mock-token".to_string()),
        owner_open_id: Some("owner@im.wechat".to_string()),
        event_connection_enabled: true,
        event_types: vec!["message.receive".to_string()],
        agent_config: None,
        created_at: 0,
        updated_at: 0,
    }
}

fn test_target() -> ImTarget {
    ImTarget {
        id: "target-1".to_string(),
        provider_id: "weixin-main".to_string(),
        display_name: "Weixin User".to_string(),
        receive_id_type: "open_id".to_string(),
        receive_id: "user@im.wechat".to_string(),
        default_msg_type: "text".to_string(),
        enabled: true,
        created_at: 0,
        updated_at: 0,
    }
}

#[test]
fn normalize_login_account_uses_ilink_fields_and_redirected_base_url() {
    let account = WeixinProvider::normalize_login_account(
        serde_json::json!({
            "bot_token": "token-1",
            "ilink_bot_id": "bot-1@im.bot",
            "ilink_user_id": "user-1@im.wechat",
            "baseurl": "https://ilink.example.com/"
        }),
        "https://fallback.example.com",
    )
    .expect("normalize account");

    assert_eq!(account.account_id, "bot-1@im.bot");
    assert_eq!(account.user_id, "user-1@im.wechat");
    assert_eq!(account.base_url, "https://ilink.example.com");
    assert_eq!(account.bot_token, "token-1");
}

#[tokio::test]
async fn complete_login_uses_the_long_login_http_timeout() {
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind delayed login server");
    let port = listener.local_addr().expect("mock local addr").port();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let io = TokioIo::new(stream);
        let service = service_fn(|_req: Request<Incoming>| async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok::<_, hyper::Error>(
                Response::builder()
                    .status(200)
                    .body(Full::new(Bytes::from_static(
                        br#"{"status":"confirmed","bot_token":"token-1","ilink_bot_id":"bot-1@im.bot","ilink_user_id":"user-1@im.wechat"}"#,
                    )))
                    .unwrap(),
            )
        });
        let _ = http1::Builder::new().serve_connection(io, service).await;
    });

    let provider =
        WeixinProvider::with_http_timeouts(Duration::from_millis(50), Duration::from_millis(250));
    let account = provider
        .complete_login(
            "poll-key",
            Some(&format!("http://127.0.0.1:{port}")),
            1,
            Duration::ZERO,
        )
        .await
        .expect("login status request must outlive the default request timeout");

    assert_eq!(account.account_id, "bot-1@im.bot");
    assert_eq!(account.user_id, "user-1@im.wechat");
}

#[test]
fn normalize_update_converts_weixin_message_to_im_event() {
    let provider = test_provider();
    let event = WeixinProvider::normalize_update(
        &provider,
        "mock-bot@im.bot",
        serde_json::json!({
            "message_id": "msg-1",
            "from_user_id": "user-1@im.wechat",
            "text": "hello bifrost"
        }),
    );

    assert_eq!(event.provider_id, "weixin-main");
    assert_eq!(event.provider_type, ImProviderType::Weixin);
    assert_eq!(event.event_type, "message.receive");
    assert_eq!(event.source.chat_id.as_deref(), Some("user-1@im.wechat"));
    assert_eq!(event.source.user_id.as_deref(), Some("user-1@im.wechat"));
    assert_eq!(event.source.message_id.as_deref(), Some("msg-1"));
    assert_eq!(event.message.as_ref().unwrap().text, "hello bifrost");
}

#[test]
fn normalize_update_extracts_ilink_item_list_text_and_numeric_message_id() {
    let provider = test_provider();
    let event = WeixinProvider::normalize_update(
        &provider,
        "mock-bot@im.bot",
        serde_json::json!({
            "message_id": 7459890488013826184u64,
            "from_user_id": "user-2@im.wechat",
            "message_type": 1,
            "item_list": [
                {
                    "type": 1,
                    "text_item": {
                        "text": "哈哈"
                    }
                }
            ]
        }),
    );

    assert_eq!(
        event.source.message_id.as_deref(),
        Some("7459890488013826184")
    );
    assert_eq!(event.message.as_ref().unwrap().text, "哈哈");
    assert_eq!(
        event.message.as_ref().unwrap().raw_type.as_deref(),
        Some("1")
    );
}

#[test]
fn normalize_update_extracts_weixin_reply_reference() {
    let provider = test_provider();
    let event = WeixinProvider::normalize_update(
        &provider,
        "mock-bot@im.bot",
        serde_json::json!({
            "message_id": 7481921678546968584u64,
            "from_user_id": "user-reply@im.wechat",
            "message_type": 1,
            "item_list": [
                {
                    "type": 1,
                    "text_item": {
                        "text": "这个链接对应哪篇文章？"
                    },
                    "ref_msg": {
                        "message_item": {
                            "type": 1,
                            "msg_id": 7481920452618855176u64,
                            "create_time_ms": 1783828843000u64,
                            "text_item": {
                                "text": "原回复 https://example.com/article"
                            }
                        }
                    }
                }
            ]
        }),
    );

    let message = event.message.expect("normalized message");
    assert_eq!(message.text, "这个链接对应哪篇文章？");
    assert_eq!(
        message.reply_to,
        Some(ImMessageReference {
            message_id: Some("7481920452618855176".to_string()),
            created_at_ms: Some(1783828843000),
            text: Some("原回复 https://example.com/article".to_string()),
        })
    );
}

#[test]
fn normalize_update_extracts_ilink_image_item_for_multimodal_agent() {
    let provider = test_provider();
    let event = WeixinProvider::normalize_update(
        &provider,
        "mock-bot@im.bot",
        serde_json::json!({
            "message_id": 7459903000000000000u64,
            "from_user_id": "user-image@im.wechat",
            "message_type": 1,
            "item_list": [
                {
                    "type": 2,
                    "msg_id": "img-msg-1",
                    "image_item": {
                        "aeskey": "00112233445566778899aabbccddeeff",
                        "media": {
                            "encrypt_query_param": "encrypted-param",
                            "full_url": "https://cdn.example.test/image.enc"
                        },
                        "thumb_width": 120
                    }
                }
            ]
        }),
    );

    let msg = event.message.as_ref().unwrap();
    assert_eq!(msg.text, "");
    assert_eq!(msg.images.len(), 1);
    assert_eq!(msg.images[0].file_key, "img-msg-1");
    assert_eq!(
        msg.images[0].download_url.as_deref(),
        Some("https://cdn.example.test/image.enc")
    );
    assert_eq!(
        msg.images[0].encrypted_query_param.as_deref(),
        Some("encrypted-param")
    );
    assert_eq!(
        msg.images[0].aes_key.as_deref(),
        Some("ABEiM0RVZneImaq7zN3u/w==")
    );
    assert_eq!(msg.images[0].mime_type.as_deref(), Some("image/png"));
}

#[test]
fn decrypt_aes_128_ecb_accepts_base64_raw_key() {
    use aes::cipher::BlockEncryptMut;

    type Aes128EcbEnc = ecb::Encryptor<aes::Aes128>;
    let key = [7u8; 16];
    let plaintext = b"fake-png-bytes";
    let ciphertext = Aes128EcbEnc::new_from_slice(&key)
        .unwrap()
        .encrypt_padded_vec_mut::<Pkcs7>(plaintext);
    let key_b64 = base64::engine::general_purpose::STANDARD.encode(key);

    let decrypted = WeixinProvider::decrypt_aes_128_ecb(&ciphertext, &key_b64, "unit").unwrap();

    assert_eq!(decrypted, plaintext);
}

#[tokio::test]
async fn download_message_image_resource_fetches_plain_full_url() {
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock image server");
    let port = listener.local_addr().expect("mock local addr").port();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let io = TokioIo::new(stream);
        let service = service_fn(|_req: Request<Incoming>| async move {
            Ok::<_, hyper::Error>(
                Response::builder()
                    .status(200)
                    .header("content-type", "image/png")
                    .body(Full::new(Bytes::from_static(b"plain-image-bytes")))
                    .unwrap(),
            )
        });
        let _ = http1::Builder::new().serve_connection(io, service).await;
    });

    let provider = WeixinProvider::new();
    let image = ImImageAttachment {
        file_key: "img-full-url".to_string(),
        source: ImImageSource::MessageResource,
        mime_type: None,
        data_base64: None,
        download_url: Some(format!("http://127.0.0.1:{port}/image.png")),
        encrypted_query_param: None,
        aes_key: None,
    };

    let (mime_type, bytes) = provider
        .download_message_image_resource(&test_provider(), &image)
        .await
        .expect("download image");

    assert_eq!(mime_type, "image/png");
    assert_eq!(bytes, b"plain-image-bytes");
}

#[tokio::test]
async fn send_image_uploads_original_bytes_to_cdn_and_sends_image_item() {
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Method, Request, Response};
    use hyper_util::rt::TokioIo;
    use std::sync::{Arc, Mutex};

    let getuploadurl_body = Arc::new(Mutex::new(None::<serde_json::Value>));
    let cdn_upload_body = Arc::new(Mutex::new(Vec::new()));
    let sendmessage_body = Arc::new(Mutex::new(None::<serde_json::Value>));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock weixin server");
    let port = listener.local_addr().expect("mock local addr").port();
    let getuploadurl_body_for_server = Arc::clone(&getuploadurl_body);
    let cdn_upload_body_for_server = Arc::clone(&cdn_upload_body);
    let sendmessage_body_for_server = Arc::clone(&sendmessage_body);

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let io = TokioIo::new(stream);
            let getuploadurl_body = Arc::clone(&getuploadurl_body_for_server);
            let cdn_upload_body = Arc::clone(&cdn_upload_body_for_server);
            let sendmessage_body = Arc::clone(&sendmessage_body_for_server);
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let getuploadurl_body = Arc::clone(&getuploadurl_body);
                    let cdn_upload_body = Arc::clone(&cdn_upload_body);
                    let sendmessage_body = Arc::clone(&sendmessage_body);
                    async move {
                        let path = req.uri().path().to_string();
                        let method = req.method().clone();
                        let body = req
                            .into_body()
                            .collect()
                            .await
                            .expect("collect request body")
                            .to_bytes();
                        if path.ends_with("/ilink/bot/getuploadurl") {
                            let json: serde_json::Value =
                                serde_json::from_slice(&body).expect("getuploadurl json");
                            *getuploadurl_body.lock().expect("lock getuploadurl") = Some(json);
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(200)
                                    .body(Full::new(Bytes::from(format!(
                                        r#"{{"ret":0,"upload_param":"upload-param","upload_full_url":"http://127.0.0.1:{port}/cdn-upload"}}"#
                                    ))))
                                    .unwrap(),
                            );
                        }
                        if path == "/cdn-upload" && method == Method::POST {
                            *cdn_upload_body.lock().expect("lock cdn body") = body.to_vec();
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(200)
                                    .header("x-encrypted-param", "download-param")
                                    .body(Full::new(Bytes::new()))
                                    .unwrap(),
                            );
                        }
                        if path == "/cdn-upload" {
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(405)
                                    .body(Full::new(Bytes::from_static(b"wrong method")))
                                    .unwrap(),
                            );
                        }
                        if path.ends_with("/ilink/bot/sendmessage") {
                            let json: serde_json::Value =
                                serde_json::from_slice(&body).expect("sendmessage json");
                            *sendmessage_body.lock().expect("lock sendmessage") = Some(json);
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(200)
                                    .body(Full::new(Bytes::from_static(b"{}")))
                                    .unwrap(),
                            );
                        }
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(404)
                                .body(Full::new(Bytes::from_static(b"not found")))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new().serve_connection(io, service).await;
            });
        }
    });

    let provider = WeixinProvider::new();
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let target = ImTarget {
        id: "target-1".to_string(),
        provider_id: config.id.clone(),
        display_name: "Weixin User".to_string(),
        receive_id_type: "open_id".to_string(),
        receive_id: "user@im.wechat".to_string(),
        default_msg_type: "image".to_string(),
        enabled: true,
        created_at: 0,
        updated_at: 0,
    };
    let uploaded = provider
        .upload_image(
            &config,
            "message",
            "cat.png",
            b"original-cat-bytes".to_vec(),
            Some("image/png"),
        )
        .await
        .expect("store outbound image");

    let result = provider
        .send_image(&config, &target, &uploaded.image_key, Some("uuid-image-1"))
        .await
        .expect("send image");

    assert_eq!(result.message_id.as_deref(), Some("uuid-image-1"));
    let getuploadurl_body = getuploadurl_body
        .lock()
        .expect("lock getuploadurl")
        .clone()
        .expect("getuploadurl called");
    assert_eq!(getuploadurl_body["media_type"], 1);
    assert_eq!(getuploadurl_body["to_user_id"], "user@im.wechat");
    assert_eq!(getuploadurl_body["rawsize"], 18);
    assert_eq!(
        getuploadurl_body["rawfilemd5"],
        "b035fd926a440dfa5717d5d3e32e9cbc"
    );
    assert_eq!(getuploadurl_body["no_need_thumb"], true);
    let aeskey = getuploadurl_body["aeskey"].as_str().expect("aes key");
    assert_eq!(aeskey.len(), 32);
    let uploaded_ciphertext = cdn_upload_body.lock().expect("lock cdn body").clone();
    assert!(!uploaded_ciphertext.is_empty());
    let decrypted =
        WeixinProvider::decrypt_aes_128_ecb(&uploaded_ciphertext, aeskey, "outbound").unwrap();
    assert_eq!(decrypted, b"original-cat-bytes");

    let sendmessage_body = sendmessage_body
        .lock()
        .expect("lock sendmessage")
        .clone()
        .expect("sendmessage called");
    assert_eq!(sendmessage_body["msg"]["to_user_id"], "user@im.wechat");
    assert_eq!(sendmessage_body["msg"]["client_id"], "uuid-image-1");
    assert_eq!(sendmessage_body["msg"]["item_list"][0]["type"], 2);
    assert_eq!(
        sendmessage_body["msg"]["item_list"][0]["image_item"]["media"]["encrypt_query_param"],
        "download-param"
    );
    assert_eq!(
        sendmessage_body["msg"]["item_list"][0]["image_item"]["media"]["encrypt_type"],
        1
    );
    assert_eq!(
        sendmessage_body["msg"]["item_list"][0]["image_item"]["aeskey"],
        aeskey
    );
}

#[tokio::test]
async fn send_image_returns_config_error_for_unknown_image_key() {
    let provider = WeixinProvider::new();
    let target = ImTarget {
        id: "target-1".to_string(),
        provider_id: "weixin-main".to_string(),
        display_name: "Weixin User".to_string(),
        receive_id_type: "open_id".to_string(),
        receive_id: "user@im.wechat".to_string(),
        default_msg_type: "image".to_string(),
        enabled: true,
        created_at: 0,
        updated_at: 0,
    };

    let error = provider
        .send_image(&test_provider(), &target, "missing-key", None)
        .await
        .expect_err("missing key should fail");

    assert!(error.to_string().contains("outbound image key not found"));
}

#[test]
fn send_error_message_detects_non_zero_ilink_code() {
    let error = WeixinProvider::send_error_message(&serde_json::json!({
        "errcode": 40001,
        "errmsg": "invalid token"
    }));

    assert_eq!(error.as_deref(), Some("errcode=40001: invalid token"));
    assert!(WeixinProvider::send_error_message(&serde_json::json!({"errcode": 0})).is_none());
}

#[test]
fn send_error_message_detects_non_zero_ilink_ret() {
    let error = WeixinProvider::send_error_message(&serde_json::json!({
        "ret": -2,
        "errmsg": "invalid payload"
    }));

    assert_eq!(error.as_deref(), Some("ret=-2: invalid payload"));
    assert!(WeixinProvider::send_error_message(&serde_json::json!({"ret": 0})).is_none());
}

#[test]
fn split_text_for_retry_preserves_multibyte_content() {
    let text = format!("开头\n{}{}", "训练基础设施".repeat(260), "\n结尾");
    let chunks = WeixinProvider::split_text_for_retry(&text);

    assert!(chunks.len() > 1);
    assert_eq!(chunks.concat(), text);
    assert!(chunks
        .iter()
        .all(|chunk| chunk.chars().count() <= TEXT_RETRY_CHUNK_MAX_CHARS));
    assert!(chunks
        .iter()
        .all(|chunk| chunk.len() <= TEXT_RETRY_CHUNK_MAX_BYTES));
}

#[tokio::test]
async fn im_long_reply_delivery_send_text_retries_failed_long_message_as_split_messages() {
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use std::sync::{Arc, Mutex};

    let sendmessage_bodies = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock weixin server");
    let port = listener.local_addr().expect("mock local addr").port();
    let bodies_for_server = Arc::clone(&sendmessage_bodies);

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let io = TokioIo::new(stream);
            let bodies = Arc::clone(&bodies_for_server);
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let bodies = Arc::clone(&bodies);
                    async move {
                        let path = req.uri().path().to_string();
                        let body = req
                            .into_body()
                            .collect()
                            .await
                            .expect("collect request body")
                            .to_bytes();
                        if path.ends_with("/ilink/bot/sendmessage") {
                            let json: serde_json::Value =
                                serde_json::from_slice(&body).expect("sendmessage json");
                            let call_count = {
                                let mut bodies = bodies.lock().expect("lock bodies");
                                bodies.push(json);
                                bodies.len()
                            };
                            let response = if call_count == 1 {
                                r#"{"ret":-2,"errmsg":"unknown error"}"#.to_string()
                            } else {
                                format!(r#"{{"ret":0,"message_id":"chunk-{call_count}"}}"#)
                            };
                            return Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(200)
                                    .body(Full::new(Bytes::from(response)))
                                    .unwrap(),
                            );
                        }
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(404)
                                .body(Full::new(Bytes::from_static(b"not found")))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new().serve_connection(io, service).await;
            });
        }
    });

    let provider = WeixinProvider::new();
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let long_text = format!(
        "{}\n{}",
        "最近一周大模型训练进展".repeat(260),
        "尾段必须保留"
    );

    let result = provider
        .send_text(&config, &test_target(), &long_text)
        .await
        .expect("split retry should recover long sendmessage failure");

    assert_eq!(result.message_id.as_deref(), Some("chunk-2"));
    let bodies = sendmessage_bodies.lock().expect("lock bodies").clone();
    assert!(
        bodies.len() > 2,
        "expected original send plus split retries"
    );
    assert_eq!(
        bodies[0]["msg"]["item_list"][0]["text_item"]["text"],
        long_text
    );

    let mut recovered = String::new();
    let split_total = bodies.len() - 1;
    for (idx, body) in bodies.iter().skip(1).enumerate() {
        let text = body["msg"]["item_list"][0]["text_item"]["text"]
            .as_str()
            .expect("chunk text");
        let prefix = format!("[{}/{}]\n", idx + 1, split_total);
        assert!(text.starts_with(&prefix), "chunk must include order prefix");
        assert!(text.len() <= TEXT_RETRY_CHUNK_MAX_BYTES + 64);
        recovered.push_str(&text[prefix.len()..]);
    }
    assert_eq!(recovered, long_text);
}

#[test]
fn card_to_text_omits_header_when_body_has_content() {
    let text = WeixinProvider::card_to_text(&serde_json::json!({
        "header": {
            "title": {
                "content": "Bifrost AI"
            }
        },
        "body": {
            "elements": [{
                "tag": "markdown",
                "content": "最终回复内容"
            }]
        }
    }));

    assert_eq!(text, "最终回复内容");
}

#[tokio::test]
async fn validate_config_requires_completed_qr_login() {
    let provider = WeixinProvider::new();
    let mut config = test_provider();
    config.secret_ref = None;
    let validation = provider.validate_config(&config).await.expect("validate");
    assert!(!validation.valid);
    assert!(validation
        .errors
        .iter()
        .any(|error| error.contains("QR login")));
}
