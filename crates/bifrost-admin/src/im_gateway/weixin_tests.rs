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
fn send_ready_survives_provider_restart_with_encrypted_context() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_provider();
    let target = test_target();
    let provider = WeixinProvider::new_with_data_dir(temp.path());
    assert!(!provider.send_ready(&config, &target));
    provider
        .context_store
        .as_ref()
        .unwrap()
        .put(
            WeixinProvider::account_id(&config),
            &target.receive_id,
            "context-token",
        )
        .unwrap();

    let restarted = WeixinProvider::new_with_data_dir(temp.path());
    assert!(restarted.send_ready(&config, &target));
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

#[tokio::test]
async fn start_login_accepts_qrcode_image_content_url() {
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind qr server");
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
                    .body(Full::new(Bytes::from_static(
                        br#"{"qrcode":"poll-key","qrcode_img_content":"https://example.com/qr.png"}"#,
                    )))
                    .unwrap(),
            )
        });
        let _ = http1::Builder::new().serve_connection(io, service).await;
    });

    let login = WeixinProvider::new()
        .start_login(Some(&format!("http://127.0.0.1:{port}")))
        .await
        .expect("start login");
    assert_eq!(login.poll_key, "poll-key");
    assert_eq!(login.scan_url, "https://example.com/qr.png");
    assert_eq!(login.expires_in_seconds, LOGIN_QR_EXPIRES_IN_SECONDS);
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
fn normalize_update_uses_opaque_media_keys_when_protocol_ids_are_missing() {
    let event = WeixinProvider::normalize_update(
        &test_provider(),
        "mock-bot@im.bot",
        serde_json::json!({
            "message_id": "media-without-item-ids",
            "from_user_id": "user-media@im.wechat",
            "item_list": [
                {
                    "type": 2,
                    "image_item": {
                        "media": {
                            "encrypt_query_param": "secret-image-query",
                            "full_url": "https://cdn.example.test/signed-image?token=secret"
                        }
                    }
                },
                {
                    "type": 4,
                    "file_item": {
                        "file_name": "report.pdf",
                        "media": {
                            "encrypt_query_param": "secret-file-query",
                            "full_url": "https://cdn.example.test/signed-file?token=secret"
                        }
                    }
                }
            ]
        }),
    );

    let message = event.message.expect("normalized message");
    let image_key = &message.images[0].file_key;
    let file_key = &message.files[0].file_key;
    assert!(image_key.starts_with("weixin-image-"), "{image_key}");
    assert!(file_key.starts_with("weixin-media-"), "{file_key}");
    assert!(!image_key.contains("secret"), "{image_key}");
    assert!(!file_key.contains("secret"), "{file_key}");
}

#[test]
fn normalize_update_extracts_file_video_and_untranscribed_voice_items() {
    let event = WeixinProvider::normalize_update(
        &test_provider(),
        "mock-bot@im.bot",
        serde_json::json!({
            "message_id": "media-bundle-1",
            "from_user_id": "user-media@im.wechat",
            "item_list": [
                {
                    "type": 4,
                    "msg_id": "file-1",
                    "file_item": {
                        "file_name": "report.pdf",
                        "len": "2048",
                        "media": {
                            "encrypt_query_param": "file-param",
                            "aes_key": "ZmlsZS1hZXMta2V5"
                        }
                    }
                },
                {
                    "type": 5,
                    "msg_id": "video-1",
                    "video_item": {
                        "video_size": 4096,
                        "play_length": 1200,
                        "media": {
                            "full_url": "https://novac2c.cdn.weixin.qq.com/c2c/video.enc",
                            "aes_key": "dmlkZW8tYWVzLWtleQ=="
                        }
                    }
                },
                {
                    "type": 3,
                    "msg_id": "voice-1",
                    "voice_item": {
                        "encode_type": 6,
                        "playtime": 900,
                        "media": {
                            "encrypt_query_param": "voice-param",
                            "aes_key": "dm9pY2UtYWVzLWtleQ=="
                        }
                    }
                }
            ]
        }),
    );

    let message = event.message.expect("normalized media message");
    assert_eq!(message.text, "【语音消息未转写，已作为音频附件提供】");
    assert_eq!(message.files.len(), 3);
    assert_eq!(message.files[0].media_kind, ImFileMediaKind::File);
    assert_eq!(message.files[0].name.as_deref(), Some("report.pdf"));
    assert_eq!(
        message.files[0].mime_type.as_deref(),
        Some("application/pdf")
    );
    assert_eq!(message.files[0].size_bytes, Some(2048));
    assert_eq!(message.files[1].media_kind, ImFileMediaKind::Video);
    assert_eq!(message.files[1].duration_ms, Some(1200));
    assert_eq!(message.files[2].media_kind, ImFileMediaKind::Voice);
    assert_eq!(message.files[2].codec.as_deref(), Some("silk"));
    assert_eq!(message.files[2].duration_ms, Some(900));
}

#[test]
fn file_item_parser_ignores_incomplete_and_duplicate_media_and_preserves_voice_spacing() {
    let mut files = Vec::new();
    WeixinProvider::push_file_from_item(&serde_json::json!({"type": 4}), &mut files);
    WeixinProvider::push_file_from_item(
        &serde_json::json!({"type": 4, "file_item": {"media": {}}}),
        &mut files,
    );
    let item = serde_json::json!({
        "type": 4,
        "msg_id": "same-file",
        "file_item": {
            "file_name": "report.pdf",
            "media": {"full_url": "https://novac2c.cdn.weixin.qq.com/report"}
        }
    });
    WeixinProvider::push_file_from_item(&item, &mut files);
    WeixinProvider::push_file_from_item(&item, &mut files);
    assert_eq!(files.len(), 1);

    let event = WeixinProvider::normalize_update(
        &test_provider(),
        "bot@im.bot",
        serde_json::json!({
            "message_id": "voice-with-text",
            "from_user_id": "user@im.wechat",
            "text": "please summarize",
            "item_list": [{
                "type": 3,
                "voice_item": {
                    "media": {"full_url": "https://novac2c.cdn.weixin.qq.com/voice"}
                }
            }]
        }),
    );
    assert!(event
        .message
        .as_ref()
        .unwrap()
        .text
        .contains("please summarize\n\n【语音消息未转写"));
}

#[test]
fn normalize_update_merges_voice_transcript_without_downloading_voice() {
    let event = WeixinProvider::normalize_update(
        &test_provider(),
        "mock-bot@im.bot",
        serde_json::json!({
            "message_id": "voice-transcript-1",
            "from_user_id": "user-voice@im.wechat",
            "item_list": [
                { "type": 1, "text_item": { "text": "请整理" } },
                {
                    "type": 3,
                    "voice_item": {
                        "text": "这是语音内容",
                        "media": {
                            "encrypt_query_param": "voice-param",
                            "aes_key": "dm9pY2UtYWVzLWtleQ=="
                        }
                    }
                }
            ]
        }),
    );

    let message = event.message.expect("normalized voice transcript");
    assert_eq!(message.text, "请整理\n\n【语音转写】\n这是语音内容");
    assert!(message.files.is_empty());
}

#[test]
fn outbound_video_requires_mime_and_file_signature_to_match() {
    let mp4 = b"\0\0\0\x18ftypisomfake";
    assert_eq!(
        WeixinProvider::classify_outbound_file(mp4, Some("video/mp4")),
        OutboundMediaKind::Video
    );
    assert_eq!(
        WeixinProvider::classify_outbound_file(mp4, Some("application/octet-stream")),
        OutboundMediaKind::File
    );
    assert_eq!(
        WeixinProvider::classify_outbound_file(b"not-a-video", Some("video/mp4")),
        OutboundMediaKind::File
    );
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

#[test]
fn parse_aes_key_accepts_hex_and_base64_encoded_hex() {
    let hex = "00112233445566778899aabbccddeeff";
    assert_eq!(
        WeixinProvider::parse_aes_key(hex, "hex").unwrap(),
        WeixinProvider::hex_to_bytes(hex).unwrap()
    );
    let encoded_hex = base64::engine::general_purpose::STANDARD.encode(hex.as_bytes());
    assert_eq!(
        WeixinProvider::parse_aes_key(&encoded_hex, "encoded-hex").unwrap(),
        WeixinProvider::hex_to_bytes(hex).unwrap()
    );
}

#[tokio::test]
async fn download_message_file_resource_decrypts_aes_payload() {
    use aes::cipher::BlockEncryptMut;
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;

    type Aes128EcbEnc = ecb::Encryptor<aes::Aes128>;
    let key = [9u8; 16];
    let plaintext = b"decrypted weixin document bytes";
    let ciphertext = Aes128EcbEnc::new_from_slice(&key)
        .unwrap()
        .encrypt_padded_vec_mut::<Pkcs7>(plaintext);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind encrypted file server");
    let port = listener.local_addr().expect("file server address").port();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept file download");
        let service = service_fn(move |_request: Request<Incoming>| {
            let ciphertext = ciphertext.clone();
            async move {
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(200)
                        .header("content-type", "application/pdf")
                        .body(Full::new(Bytes::from(ciphertext)))
                        .unwrap(),
                )
            }
        });
        let _ = http1::Builder::new()
            .serve_connection(TokioIo::new(stream), service)
            .await;
    });

    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let file = ImFileAttachment {
        file_key: "encrypted-document".to_string(),
        name: Some("document.pdf".to_string()),
        mime_type: Some("application/pdf".to_string()),
        download_url: Some(format!("http://127.0.0.1:{port}/document.enc")),
        aes_key: Some(base64::engine::general_purpose::STANDARD.encode(key)),
        ..Default::default()
    };

    let (mime_type, bytes) = provider
        .download_message_file_resource(&config, &file)
        .await
        .expect("download and decrypt file");
    assert_eq!(mime_type, "application/pdf");
    assert_eq!(bytes, plaintext);
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

    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
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

    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
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
    provider
        .context_store
        .as_ref()
        .unwrap()
        .put(
            WeixinProvider::account_id(&config),
            &target.receive_id,
            "image-context",
        )
        .unwrap();
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
async fn send_file_encrypts_cdn_bytes_and_sends_native_file_item() {
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use std::sync::{Arc, Mutex};

    let getuploadurl_body = Arc::new(Mutex::new(None::<serde_json::Value>));
    let cdn_upload_body = Arc::new(Mutex::new(Vec::new()));
    let sendmessage_body = Arc::new(Mutex::new(None::<serde_json::Value>));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock weixin file server");
    let port = listener.local_addr().expect("mock local addr").port();
    let upload_record = Arc::clone(&getuploadurl_body);
    let cdn_record = Arc::clone(&cdn_upload_body);
    let send_record = Arc::clone(&sendmessage_body);

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let upload_record = Arc::clone(&upload_record);
            let cdn_record = Arc::clone(&cdn_record);
            let send_record = Arc::clone(&send_record);
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| {
                    let upload_record = Arc::clone(&upload_record);
                    let cdn_record = Arc::clone(&cdn_record);
                    let send_record = Arc::clone(&send_record);
                    async move {
                        let path = request.uri().path().to_string();
                        let body = request
                            .into_body()
                            .collect()
                            .await
                            .expect("collect file request body")
                            .to_bytes();
                        let response = if path.ends_with("/ilink/bot/getuploadurl") {
                            *upload_record.lock().expect("lock upload record") =
                                Some(serde_json::from_slice(&body).expect("getuploadurl json"));
                            Response::builder().status(200).body(Full::new(Bytes::from(
                                format!(
                                    r#"{{"ret":0,"upload_param":"file-upload-param","upload_full_url":"http://127.0.0.1:{port}/cdn-file"}}"#
                                ),
                            )))
                        } else if path == "/cdn-file" {
                            *cdn_record.lock().expect("lock cdn record") = body.to_vec();
                            Response::builder()
                                .status(200)
                                .header("x-encrypted-param", "file-download-param")
                                .body(Full::new(Bytes::new()))
                        } else if path.ends_with("/ilink/bot/sendmessage") {
                            *send_record.lock().expect("lock send record") =
                                Some(serde_json::from_slice(&body).expect("sendmessage json"));
                            Response::builder()
                                .status(200)
                                .body(Full::new(Bytes::from_static(b"{}")))
                        } else {
                            Response::builder()
                                .status(404)
                                .body(Full::new(Bytes::from_static(b"not found")))
                        };
                        Ok::<_, hyper::Error>(response.unwrap())
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });

    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let target = test_target();
    provider
        .context_store
        .as_ref()
        .unwrap()
        .put(
            WeixinProvider::account_id(&config),
            &target.receive_id,
            "file-context",
        )
        .unwrap();
    let plaintext = b"final report bytes";
    let file_key = provider
        .upload_file(
            &config,
            "final-report.pdf",
            plaintext.to_vec(),
            Some("application/pdf"),
        )
        .await
        .expect("store outbound file");
    let channel_run_id = provider.begin_channel_run(&config, &target);
    let result = provider
        .send_file(&config, &target, &file_key, Some("uuid-file-1"))
        .await
        .expect("send file");

    assert_eq!(result.message_id.as_deref(), Some("uuid-file-1"));
    let upload = getuploadurl_body
        .lock()
        .expect("lock upload")
        .clone()
        .expect("getuploadurl called");
    assert_eq!(upload["media_type"], 3);
    assert_eq!(upload["rawsize"], plaintext.len());
    assert_eq!(upload["to_user_id"], target.receive_id);
    let aeskey = upload["aeskey"].as_str().expect("aes key");
    let ciphertext = cdn_upload_body.lock().expect("lock cdn").clone();
    assert_eq!(
        WeixinProvider::decrypt_aes_128_ecb(&ciphertext, aeskey, "outbound-file").unwrap(),
        plaintext
    );
    let sent = sendmessage_body
        .lock()
        .expect("lock send")
        .clone()
        .expect("sendmessage called");
    let item = &sent["msg"]["item_list"][0];
    assert_eq!(item["type"], 4);
    assert_eq!(item["file_item"]["file_name"], "final-report.pdf");
    assert_eq!(item["file_item"]["len"], plaintext.len().to_string());
    assert_eq!(sent["msg"]["run_id"], channel_run_id);
    assert_eq!(
        item["file_item"]["media"]["encrypt_query_param"],
        "file-download-param"
    );

    let no_context_dir = tempfile::tempdir().unwrap();
    let no_context_provider = WeixinProvider::new_with_data_dir(no_context_dir.path());
    let no_context_key = no_context_provider
        .upload_file(
            &config,
            "no-context.txt",
            b"no context".to_vec(),
            Some("text/plain"),
        )
        .await
        .unwrap();
    assert!(no_context_provider
        .send_file(&config, &target, &no_context_key, None)
        .await
        .unwrap_err()
        .to_string()
        .contains("not send-ready"));
}

#[tokio::test]
async fn upload_file_routes_matching_video_to_native_video_item() {
    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
    let mp4 = b"\0\0\0\x18ftypisomvideo".to_vec();
    let key = provider
        .upload_file(&test_provider(), "clip.mp4", mp4.clone(), Some("video/mp4"))
        .await
        .expect("store outbound video");
    let pending = provider
        .pending_outbound_media
        .write()
        .remove(&key)
        .expect("pending video");
    assert_eq!(pending.kind, OutboundMediaKind::Video);
    let item = WeixinProvider::outbound_media_item(&UploadedOutboundMedia {
        pending,
        download_param: "video-download-param".to_string(),
        aeskey_hex: "00112233445566778899aabbccddeeff".to_string(),
        ciphertext_size: 32,
    });
    assert_eq!(item["type"], 5);
    assert_eq!(item["video_item"]["video_size"], 32);
    assert_eq!(
        item["video_item"]["media"]["encrypt_query_param"],
        "video-download-param"
    );
    assert_eq!(OutboundMediaKind::Video.upload_media_type(), 2);
}

#[tokio::test]
async fn outbound_media_reports_kind_upload_cdn_and_send_failures() {
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind outbound error server");
    let port = listener.local_addr().unwrap().port();
    let upload_attempt = Arc::new(AtomicUsize::new(0));
    let attempts = Arc::clone(&upload_attempt);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let attempts = Arc::clone(&attempts);
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| {
                    let attempts = Arc::clone(&attempts);
                    async move {
                        let path = request.uri().path().to_string();
                        let _ = request.into_body().collect().await;
                        let response = if path.ends_with("/ilink/bot/getuploadurl") {
                            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                            match attempt {
                                0 => Response::builder()
                                    .status(200)
                                    .body(Full::new(Bytes::from_static(
                                        br#"{"ret":-1,"errmsg":"upload denied"}"#,
                                    ))),
                                1 => Response::builder().status(200).body(Full::new(
                                    Bytes::from_static(br#"{"ret":0}"#),
                                )),
                                2 => Response::builder().status(200).body(Full::new(Bytes::from(
                                    format!(r#"{{"ret":0,"upload_param":"p","upload_full_url":"http://127.0.0.1:{port}/cdn-error"}}"#),
                                ))),
                                3 => Response::builder().status(200).body(Full::new(Bytes::from(
                                    format!(r#"{{"ret":0,"upload_param":"p","upload_full_url":"http://127.0.0.1:{port}/cdn-no-header"}}"#),
                                ))),
                                4 => Response::builder().status(200).body(Full::new(Bytes::from(
                                    format!(r#"{{"ret":0,"upload_param":"p","upload_full_url":"http://127.0.0.1:{port}/cdn-ok"}}"#),
                                ))),
                                _ => Response::builder().status(200).body(Full::new(Bytes::from(
                                    r#"{"ret":0,"upload_param":"p","upload_full_url":"http://127.0.0.1:1/unreachable"}"#,
                                ))),
                            }
                        } else if path == "/cdn-error" {
                            Response::builder()
                                .status(503)
                                .body(Full::new(Bytes::from_static(b"cdn unavailable")))
                        } else if path == "/cdn-no-header" {
                            Response::builder()
                                .status(200)
                                .body(Full::new(Bytes::new()))
                        } else if path == "/cdn-ok" {
                            Response::builder()
                                .status(200)
                                .header("x-encrypted-param", "download-param")
                                .body(Full::new(Bytes::new()))
                        } else if path.ends_with("/ilink/bot/sendmessage") {
                            Response::builder()
                                .status(200)
                                .body(Full::new(Bytes::from_static(
                                    br#"{"ret":-2,"errmsg":"send denied"}"#,
                                )))
                        } else {
                            Response::builder()
                                .status(404)
                                .body(Full::new(Bytes::new()))
                        };
                        Ok::<_, hyper::Error>(response.unwrap())
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });

    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let target = test_target();
    provider
        .store_context_for_test(&config, &target.receive_id, "media-context")
        .unwrap();

    let mismatch_key = provider
        .upload_file(&config, "mismatch.txt", b"x".to_vec(), Some("text/plain"))
        .await
        .unwrap();
    assert!(provider
        .upload_outbound_media_for_target(
            &config,
            &target,
            &mismatch_key,
            Some(OutboundMediaKind::Image),
        )
        .await
        .err()
        .expect("kind mismatch must fail")
        .to_string()
        .contains("kind mismatch"));

    for expected in [
        "getuploadurl failed",
        "missing upload_param",
        "CDN file upload error",
        "missing x-encrypted-param",
    ] {
        let key = provider
            .upload_file(
                &config,
                "report.txt",
                b"payload".to_vec(),
                Some("text/plain"),
            )
            .await
            .unwrap();
        let error = provider
            .upload_outbound_media_for_target(&config, &target, &key, Some(OutboundMediaKind::File))
            .await
            .err()
            .expect("scripted upload must fail")
            .to_string();
        assert!(error.contains(expected), "unexpected error: {error}");
    }

    let send_key = provider
        .upload_file(&config, "send.txt", b"payload".to_vec(), Some("text/plain"))
        .await
        .unwrap();
    assert!(provider
        .send_file(&config, &target, &send_key, None)
        .await
        .unwrap_err()
        .to_string()
        .contains("send file failed"));

    let unreachable_key = provider
        .upload_file(
            &config,
            "network.txt",
            b"payload".to_vec(),
            Some("text/plain"),
        )
        .await
        .unwrap();
    assert!(provider
        .upload_outbound_media_for_target(
            &config,
            &target,
            &unreachable_key,
            Some(OutboundMediaKind::File),
        )
        .await
        .err()
        .expect("unreachable CDN must fail")
        .to_string()
        .contains("CDN file upload failed"));
}

#[test]
fn pending_media_cache_evicts_expired_entries_and_enforces_item_limit() {
    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
    provider.pending_outbound_media.write().insert(
        "expired".to_string(),
        PendingOutboundMedia {
            kind: OutboundMediaKind::File,
            file_name: "old.txt".to_string(),
            bytes: vec![1],
            mime_type: Some("text/plain".to_string()),
            created_at_ms: now_ms().saturating_sub(PENDING_OUTBOUND_MEDIA_TTL_MS + 1),
        },
    );
    for index in 0..MAX_PENDING_OUTBOUND_MEDIA_ITEMS {
        provider
            .insert_pending_outbound_media(PendingOutboundMedia {
                kind: OutboundMediaKind::File,
                file_name: format!("{index}.txt"),
                bytes: vec![1],
                mime_type: Some("text/plain".to_string()),
                created_at_ms: now_ms(),
            })
            .expect("insert within pending item limit");
    }
    assert!(!provider
        .pending_outbound_media
        .read()
        .contains_key("expired"));
    let error = provider
        .insert_pending_outbound_media(PendingOutboundMedia {
            kind: OutboundMediaKind::File,
            file_name: "overflow.txt".to_string(),
            bytes: vec![1],
            mime_type: Some("text/plain".to_string()),
            created_at_ms: now_ms(),
        })
        .expect_err("pending item limit must be enforced")
        .to_string();
    assert!(error.contains("cache is full"));
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

    assert!(error.to_string().contains("outbound media key not found"));
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
fn long_poll_timeout_is_clamped_and_client_keeps_network_margin() {
    assert_eq!(
        WeixinProvider::normalize_long_poll_timeout_ms(1),
        MIN_LONG_POLL_TIMEOUT_MS
    );
    assert_eq!(
        WeixinProvider::normalize_long_poll_timeout_ms(999_999),
        MAX_LONG_POLL_TIMEOUT_MS
    );
    assert_eq!(
        WeixinProvider::long_poll_request_timeout_ms(DEFAULT_LONG_POLL_TIMEOUT_MS),
        DEFAULT_LONG_POLL_TIMEOUT_MS + LONG_POLL_NETWORK_MARGIN_MS
    );
}

#[tokio::test]
async fn poll_once_applies_dynamic_timeout_and_rejects_stale_authorization() {
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let calls = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind poll mock server");
    let port = listener.local_addr().expect("poll mock address").port();
    let calls_for_server = Arc::clone(&calls);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let calls = Arc::clone(&calls_for_server);
            tokio::spawn(async move {
                let service = service_fn(move |_request: Request<Incoming>| {
                    let call = calls.fetch_add(1, Ordering::SeqCst);
                    async move {
                        let body = match call {
                            0 => r#"{"ret":0,"longpolling_timeout_ms":1,"msgs":[]}"#,
                            1 => r#"{"ret":0,"longpolling_timeout_ms":999999,"msgs":[]}"#,
                            _ => r#"{"ret":-14,"errmsg":"stale token"}"#,
                        };
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(200)
                                .body(Full::new(Bytes::from_static(body.as_bytes())))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });

    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    provider.poll_once(&config).await.expect("first poll");
    assert_eq!(
        provider.runtime.read()[&WeixinProvider::account_runtime_key(&config)].long_poll_timeout_ms,
        MIN_LONG_POLL_TIMEOUT_MS
    );
    provider.poll_once(&config).await.expect("second poll");
    assert_eq!(
        provider.runtime.read()[&WeixinProvider::account_runtime_key(&config)].long_poll_timeout_ms,
        MAX_LONG_POLL_TIMEOUT_MS
    );
    let error = provider
        .poll_once(&config)
        .await
        .expect_err("stale authorization must stop polling")
        .to_string();
    assert!(error.contains("authentication required"));
    assert!(error.contains("ret=-14"));
}

#[tokio::test]
async fn poll_rejects_invalid_json_and_connection_requires_cursor_store() {
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind invalid poll server");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let service = service_fn(|_request: Request<Incoming>| async move {
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(200)
                        .body(Full::new(Bytes::from_static(b"not-json")))
                        .unwrap(),
                )
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(dir.path());
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    assert!(provider
        .poll_once(&config)
        .await
        .unwrap_err()
        .to_string()
        .contains("response parse failed"));

    let mut unavailable = WeixinProvider::new_with_data_dir(dir.path());
    unavailable.sync_cursor_store = None;
    let (sink, _events) = tokio::sync::mpsc::unbounded_channel();
    assert!(unavailable
        .connect_events_with_status(&config, sink.into(), None)
        .await
        .expect_err("missing cursor store must fail")
        .to_string()
        .contains("sync cursor store is unavailable"));
}

#[tokio::test]
async fn poll_rejects_non_success_http_status() {
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind HTTP error poll server");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let service = service_fn(|_request: Request<Incoming>| async move {
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(503)
                        .body(Full::new(Bytes::from_static(b"upstream unavailable")))
                        .unwrap(),
                )
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(dir.path());
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let error = provider.poll_once(&config).await.unwrap_err().to_string();
    assert!(error.contains("status=503"));
    assert!(error.contains("upstream unavailable"));
}

#[tokio::test]
async fn connection_recovers_after_sync_cursor_persist_failure() {
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind cursor persistence server");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let service = service_fn(|_request: Request<Incoming>| async move {
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(200)
                        .body(Full::new(Bytes::from_static(
                            br#"{"ret":0,"get_updates_buf":"recovered-cursor","msgs":[]}"#,
                        )))
                        .unwrap(),
                )
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        }
    });

    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
    let blocked_store_path = data_dir
        .path()
        .join("admin")
        .join("im_gateway_weixin_sync_cursors.json");
    std::fs::create_dir_all(&blocked_store_path).unwrap();

    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let (sink, _events) = tokio::sync::mpsc::unbounded_channel();
    let (status_tx, mut status_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = provider
        .connect_events_with_status(&config, sink.into(), Some(status_tx))
        .await
        .expect("start cursor persistence recovery connection");

    let reconnecting = tokio::time::timeout(Duration::from_secs(1), status_rx.recv())
        .await
        .expect("cursor persistence reconnecting timeout")
        .expect("cursor persistence reconnecting status");
    assert_eq!(reconnecting.state, ConnectionState::Reconnecting);
    assert!(reconnecting
        .error
        .as_deref()
        .is_some_and(|error| error.contains("replace weixin sync cursor store")));

    std::fs::remove_dir(&blocked_store_path).unwrap();
    let connected = tokio::time::timeout(Duration::from_secs(4), status_rx.recv())
        .await
        .expect("cursor persistence connected timeout")
        .expect("cursor persistence connected status");
    assert_eq!(connected.state, ConnectionState::Connected);
    assert_eq!(
        provider
            .sync_cursor_store
            .as_ref()
            .unwrap()
            .get(&config.id, WeixinProvider::account_id(&config))
            .as_deref(),
        Some("recovered-cursor")
    );
    let _ = handle.shutdown_tx.send(());
}

#[tokio::test]
async fn connection_retries_until_inbound_event_is_durably_accepted() {
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use std::sync::Arc;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind durable event server");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let service = service_fn(|_request: Request<Incoming>| async move {
                Ok::<_, hyper::Error>(
                    Response::builder()
                        .status(200)
                        .body(Full::new(Bytes::from_static(
                            br#"{"ret":0,"get_updates_buf":"durable-cursor","msgs":[{"message_id":"durable-event","from_user_id":"user@im.wechat","text":"hello"}]}"#,
                        )))
                        .unwrap(),
                )
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        }
    });

    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
    let event_store = Arc::new(crate::im_gateway::ImEventStore::new(data_dir.path()));
    let blocked_store_path = data_dir.path().join("admin").join("im_gateway_events.json");
    std::fs::create_dir_all(&blocked_store_path).unwrap();

    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let sink = crate::im_gateway::provider::EventSink::with_durable_store(
        sender,
        Arc::clone(&event_store),
    );
    let (status_tx, mut status_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = provider
        .connect_events_with_status(&config, sink, Some(status_tx))
        .await
        .expect("start durable event recovery connection");

    let reconnecting = tokio::time::timeout(Duration::from_secs(1), status_rx.recv())
        .await
        .expect("durable event reconnecting timeout")
        .expect("durable event reconnecting status");
    assert_eq!(reconnecting.state, ConnectionState::Reconnecting);
    assert!(reconnecting
        .error
        .as_deref()
        .is_some_and(|error| error.contains("im_gateway_events.json")));
    assert!(event_store.list().is_empty());
    assert!(provider
        .sync_cursor_store
        .as_ref()
        .unwrap()
        .get(&config.id, WeixinProvider::account_id(&config))
        .is_none());

    std::fs::remove_dir(&blocked_store_path).unwrap();
    let connected = tokio::time::timeout(Duration::from_secs(4), status_rx.recv())
        .await
        .expect("durable event connected timeout")
        .expect("durable event connected status");
    assert_eq!(connected.state, ConnectionState::Connected);
    let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("durable event timeout")
        .expect("durable event");
    assert_eq!(event.event_id, "durable-event");
    assert_eq!(event_store.list().len(), 1);
    assert_eq!(
        provider
            .sync_cursor_store
            .as_ref()
            .unwrap()
            .get(&config.id, WeixinProvider::account_id(&config))
            .as_deref(),
        Some("durable-cursor")
    );
    let _ = handle.shutdown_tx.send(());
}

#[tokio::test]
async fn connection_reports_closed_sink_after_transient_poll_error() {
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let calls = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind recovery poll server");
    let port = listener.local_addr().unwrap().port();
    let server_calls = Arc::clone(&calls);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let calls = Arc::clone(&server_calls);
            tokio::spawn(async move {
                let service = service_fn(move |_request: Request<Incoming>| {
                    let call = calls.fetch_add(1, Ordering::SeqCst);
                    async move {
                        let body = if call == 0 {
                            r#"{"ret":-1,"errmsg":"temporary"}"#
                        } else {
                            r#"{"ret":0,"msgs":[{"message_id":"recovered","from_user_id":"user@im.wechat","text":"hello"}]}"#
                        };
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(200)
                                .body(Full::new(Bytes::from_static(body.as_bytes())))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(dir.path());
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let (sink, events) = tokio::sync::mpsc::unbounded_channel();
    drop(events);
    let (status_tx, mut status_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = provider
        .connect_events_with_status(&config, sink.into(), Some(status_tx))
        .await
        .expect("start recovery connection");
    let reconnecting = tokio::time::timeout(Duration::from_secs(1), status_rx.recv())
        .await
        .expect("reconnecting status timeout")
        .expect("reconnecting status");
    assert_eq!(reconnecting.state, ConnectionState::Reconnecting);
    let disconnected = tokio::time::timeout(Duration::from_secs(4), status_rx.recv())
        .await
        .expect("disconnected status timeout")
        .expect("disconnected status");
    assert_eq!(disconnected.state, ConnectionState::Disconnected);
    assert!(calls.load(Ordering::SeqCst) >= 2);
    let _ = handle.shutdown_tx.send(());
}

#[tokio::test]
async fn connect_events_persists_cursor_only_after_enqueue_and_resumes_next_poll() {
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use std::sync::{Arc, Mutex};

    let request_bodies = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind cursor mock server");
    let port = listener.local_addr().expect("cursor mock address").port();
    let bodies_for_server = Arc::clone(&request_bodies);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let bodies = Arc::clone(&bodies_for_server);
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| {
                    let bodies = Arc::clone(&bodies);
                    async move {
                        let bytes = request
                            .into_body()
                            .collect()
                            .await
                            .expect("collect cursor request")
                            .to_bytes();
                        let body: serde_json::Value =
                            serde_json::from_slice(&bytes).expect("cursor request json");
                        let call = {
                            let mut bodies = bodies.lock().expect("lock cursor bodies");
                            bodies.push(body);
                            bodies.len()
                        };
                        let response = if call == 1 {
                            r#"{"ret":0,"get_updates_buf":"cursor-1","msgs":[{"message_id":"cursor-msg-1","from_user_id":"cursor-user","text":"hello","context_token":"cursor-context"}]}"#
                        } else {
                            r#"{"ret":-14,"errmsg":"stop after resume check"}"#
                        };
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(200)
                                .body(Full::new(Bytes::from_static(response.as_bytes())))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });

    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let (sink_tx, mut sink_rx) = tokio::sync::mpsc::unbounded_channel();
    let event_store = Arc::new(crate::im_gateway::ImEventStore::new(data_dir.path()));
    let sink = crate::im_gateway::provider::EventSink::with_durable_store(
        sink_tx,
        Arc::clone(&event_store),
    );
    let handle = provider
        .connect_events_with_status(&config, sink, None)
        .await
        .expect("start cursor poll");
    let event = tokio::time::timeout(Duration::from_secs(2), sink_rx.recv())
        .await
        .expect("event timeout")
        .expect("cursor event");
    assert_eq!(event.event_id, "cursor-msg-1");
    assert_eq!(event_store.list()[0].event_id, "cursor-msg-1");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if request_bodies.lock().expect("lock cursor bodies").len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("second cursor request");
    let bodies = request_bodies.lock().expect("lock cursor bodies").clone();
    assert_eq!(bodies[0]["get_updates_buf"], "");
    assert_eq!(bodies[1]["get_updates_buf"], "cursor-1");
    assert_eq!(
        provider
            .sync_cursor_store
            .as_ref()
            .unwrap()
            .get(&config.id, WeixinProvider::account_id(&config))
            .as_deref(),
        Some("cursor-1")
    );
    let _ = handle.shutdown_tx.send(());
}

#[tokio::test]
async fn stale_authorization_updates_connection_status_without_busy_retry() {
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let calls = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stale auth server");
    let port = listener.local_addr().expect("stale auth address").port();
    let calls_for_server = Arc::clone(&calls);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let calls = Arc::clone(&calls_for_server);
            tokio::spawn(async move {
                let service = service_fn(move |_request: Request<Incoming>| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async move {
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(200)
                                .body(Full::new(Bytes::from_static(
                                    br#"{"ret":-14,"errmsg":"stale"}"#,
                                )))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });

    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let (sink_tx, _sink_rx) = tokio::sync::mpsc::unbounded_channel();
    let (status_tx, mut status_rx) = tokio::sync::mpsc::unbounded_channel();
    let _handle = provider
        .connect_events_with_status(&config, sink_tx.into(), Some(status_tx))
        .await
        .expect("start stale auth poll");
    let status = tokio::time::timeout(Duration::from_secs(2), status_rx.recv())
        .await
        .expect("status timeout")
        .expect("authentication status");
    assert_eq!(status.state, ConnectionState::AuthenticationRequired);
    assert!(status
        .error
        .as_deref()
        .is_some_and(|error| error.contains("scan a new QR")));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
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
async fn im_long_reply_delivery_retries_failed_full_message_with_stable_child_ids() {
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
                            let client_id = json["msg"]["client_id"]
                                .as_str()
                                .expect("sendmessage client id")
                                .to_string();
                            let call_count = {
                                let mut bodies = bodies.lock().expect("lock bodies");
                                bodies.push(json);
                                bodies.len()
                            };
                            let response =
                                if client_id == "stable-long-1" || client_id.starts_with("fail-") {
                                    r#"{"ret":40003,"errmsg":"message too long"}"#.to_string()
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

    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let target = test_target();
    provider
        .context_store
        .as_ref()
        .unwrap()
        .put(
            WeixinProvider::account_id(&config),
            &target.receive_id,
            "text-context",
        )
        .unwrap();
    let long_text = format!(
        "{}\n{}",
        "最近一周大模型训练进展".repeat(260),
        "尾段必须保留"
    );
    let rendered_long_text = WeixinProvider::render_text_for_weixin(&long_text);

    let result = provider
        .send_text_with_client_id(&config, &target, &long_text, "stable-long-1")
        .await
        .expect("failed full message falls back to split send");

    assert_eq!(result.message_id.as_deref(), Some("chunk-2"));
    let bodies = sendmessage_bodies.lock().expect("lock bodies").clone();
    assert!(
        bodies.len() > 2,
        "expected full attempt plus split messages"
    );
    assert_eq!(
        bodies[0]["msg"]["item_list"][0]["text_item"]["text"],
        rendered_long_text
    );
    assert_eq!(bodies[0]["msg"]["client_id"], "stable-long-1");

    let mut recovered = String::new();
    let split_bodies = &bodies[1..];
    let split_total = split_bodies.len();
    for (idx, body) in split_bodies.iter().enumerate() {
        let text = body["msg"]["item_list"][0]["text_item"]["text"]
            .as_str()
            .expect("chunk text");
        let prefix = format!("[{}/{}]\n\n", idx + 1, split_total);
        assert!(text.starts_with(&prefix), "chunk must include order prefix");
        assert!(text.len() <= TEXT_RETRY_CHUNK_MAX_BYTES + 64);
        assert_eq!(
            body["msg"]["client_id"],
            format!("stable-long-1-part-{}", idx + 1)
        );
        assert_eq!(body["msg"]["context_token"], "text-context");
        recovered.push_str(&text[prefix.len()..]);
    }
    assert_eq!(recovered, rendered_long_text);

    let short_error = provider
        .send_text_with_client_id(&config, &target, "短消息", "fail-short")
        .await
        .expect_err("a short network failure cannot be split")
        .to_string();
    assert!(short_error.contains("message too long"));

    let chunk_error = provider
        .send_text_with_client_id(&config, &target, &long_text, "fail-long")
        .await
        .expect_err("a failed fallback chunk must preserve both errors")
        .to_string();
    assert!(chunk_error.contains("fallback chunk 1/"));
    assert!(chunk_error.contains("message too long"));
}

#[test]
fn weixin_text_renderer_promotes_single_line_breaks_and_preserves_paragraphs() {
    assert_eq!(
        WeixinProvider::render_text_for_weixin(
            "可用命令:\r\n/help  显示帮助\n/status  查看状态\n\nRunner 命令:\r/fast  切换模式"
        ),
        "可用命令:\n\n/help  显示帮助\n\n/status  查看状态\n\nRunner 命令:\n\n/fast  切换模式"
    );
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

#[test]
fn unavailable_context_store_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let blocked_data_dir = temp.path().join("blocked-data-dir");
    std::fs::write(&blocked_data_dir, b"not a directory").unwrap();
    let provider = WeixinProvider::new_with_data_dir(&blocked_data_dir);
    assert!(provider.context_store.is_none());
    assert!(provider
        .store_context_for_test(&test_provider(), "owner", "context")
        .is_err());
}

#[tokio::test]
async fn missing_context_blocks_short_and_long_full_text_before_network_send() {
    let temp = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(temp.path());
    let config = test_provider();
    let target = test_target();

    let short_error = provider
        .send_text_with_client_id(&config, &target, "hello", "short-client-id")
        .await
        .unwrap_err()
        .to_string();
    assert!(short_error.contains("not send-ready"));

    let long_error = provider
        .send_text_with_client_id(
            &config,
            &target,
            &"long text".repeat(TEXT_RETRY_CHUNK_MAX_CHARS),
            "chunked-client-id",
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(!long_error.contains("chunk"));
    assert!(long_error.contains("not send-ready"));
}

#[tokio::test]
async fn typing_and_tool_progress_fail_closed_across_remote_error_modes() {
    use bifrost_agent::{AgentTurnProgressEvent, ToolCallLog};
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let missing_context_dir = tempfile::tempdir().unwrap();
    let missing_context_provider = Arc::new(WeixinProvider::new_with_data_dir(
        missing_context_dir.path(),
    ));
    let missing_context = missing_context_provider
        .typing_ticket(&test_provider(), &test_target())
        .await
        .unwrap_err()
        .to_string();
    assert!(missing_context.contains("inbound context token"));
    let missing_progress_context = missing_context_provider
        .send_tool_progress(
            &test_provider(),
            &test_target(),
            WeixinToolProgress {
                channel_run_id: "run",
                client_msg_id: "client",
                tool_name: "read_file",
                tool_call_id: None,
                finished_status: None,
            },
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(missing_progress_context.contains("inbound context token"));
    let mut missing_context_session =
        crate::im_gateway::weixin_progress::WeixinProgressSession::start_for_test(
            Arc::clone(&missing_context_provider),
            test_provider(),
            test_target(),
            Duration::from_millis(10),
        )
        .await;
    missing_context_session
        .apply_events(vec![AgentTurnProgressEvent::AssistantDelta {
            content: "ignored by structured tool progress".to_string(),
        }])
        .await;
    missing_context_session.finish().await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind progress error server");
    let port = listener.local_addr().unwrap().port();
    let getconfig_calls = Arc::new(AtomicUsize::new(0));
    let sendtyping_calls = Arc::new(AtomicUsize::new(0));
    let getconfig_state = Arc::clone(&getconfig_calls);
    let sendtyping_state = Arc::clone(&sendtyping_calls);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let getconfig_state = Arc::clone(&getconfig_state);
            let sendtyping_state = Arc::clone(&sendtyping_state);
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| {
                    let getconfig_state = Arc::clone(&getconfig_state);
                    let sendtyping_state = Arc::clone(&sendtyping_state);
                    async move {
                        let path = request.uri().path().to_string();
                        let _ = request.into_body().collect().await;
                        let body = if path.ends_with("/ilink/bot/getconfig") {
                            match getconfig_state.fetch_add(1, Ordering::SeqCst) {
                                0 => br#"{"ret":-3,"errmsg":"config denied"}"#.as_slice(),
                                1 => br#"{"ret":0}"#.as_slice(),
                                _ => br#"{"ret":0,"typing_ticket":"ticket"}"#.as_slice(),
                            }
                        } else if path.ends_with("/ilink/bot/sendtyping") {
                            match sendtyping_state.fetch_add(1, Ordering::SeqCst) {
                                0 | 1 => br#"{"ret":-4,"errmsg":"typing denied"}"#.as_slice(),
                                2 => br#"{"ret":0}"#.as_slice(),
                                _ => br#"{"ret":-5,"errmsg":"typing expired"}"#.as_slice(),
                            }
                        } else if path.ends_with("/ilink/bot/sendmessage") {
                            br#"{"ret":-6,"errmsg":"progress denied"}"#.as_slice()
                        } else {
                            br#"{"ret":0}"#.as_slice()
                        };
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(200)
                                .body(Full::new(Bytes::copy_from_slice(body)))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });

    let data_dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(WeixinProvider::new_with_data_dir(data_dir.path()));
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let target = test_target();
    provider
        .store_context_for_test(&config, &target.receive_id, "progress-context")
        .unwrap();
    provider.typing_tickets.write().insert(
        WeixinProvider::user_runtime_key(&config, &target.receive_id),
        CachedTypingTicket {
            ticket: "expired-ticket".to_string(),
            expires_at_ms: now_ms().saturating_sub(1),
        },
    );

    assert!(provider
        .typing_ticket(&config, &target)
        .await
        .unwrap_err()
        .to_string()
        .contains("config denied"));
    assert!(provider
        .typing_ticket(&config, &target)
        .await
        .unwrap_err()
        .to_string()
        .contains("missing typing_ticket"));

    let mut rejected_start =
        crate::im_gateway::weixin_progress::WeixinProgressSession::start_for_test(
            Arc::clone(&provider),
            config.clone(),
            target.clone(),
            Duration::from_millis(10),
        )
        .await;
    rejected_start.finish().await;

    let mut failing_keepalive =
        crate::im_gateway::weixin_progress::WeixinProgressSession::start_for_test(
            Arc::clone(&provider),
            config.clone(),
            target.clone(),
            Duration::from_millis(10),
        )
        .await;
    failing_keepalive
        .apply_events(vec![
            AgentTurnProgressEvent::ToolStarted {
                tool_name: "read_file".to_string(),
                arguments: " a ".to_string(),
            },
            AgentTurnProgressEvent::ToolFinished {
                log: ToolCallLog {
                    tool_name: "read_file".to_string(),
                    arguments: "a".to_string(),
                    result: "failed".to_string(),
                    success: false,
                },
                duration_ms: 1,
            },
        ])
        .await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    failing_keepalive.finish().await;

    let network_dir = tempfile::tempdir().unwrap();
    let network_provider = WeixinProvider::new_with_data_dir(network_dir.path());
    let mut network_config = test_provider();
    network_config.base_url = Some("http://127.0.0.1:1".to_string());
    network_provider
        .store_context_for_test(&network_config, &target.receive_id, "network-context")
        .unwrap();
    assert!(network_provider
        .typing_ticket(&network_config, &target)
        .await
        .unwrap_err()
        .to_string()
        .contains("getconfig failed"));
    assert!(network_provider
        .send_typing_status(&network_config, &target, "ticket", 1)
        .await
        .unwrap_err()
        .to_string()
        .contains("sendtyping failed"));
    assert!(network_provider
        .send_tool_progress(
            &network_config,
            &target,
            WeixinToolProgress {
                channel_run_id: "run",
                client_msg_id: "client",
                tool_name: "read_file",
                tool_call_id: None,
                finished_status: None,
            },
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("send tool progress failed"));
}

#[tokio::test]
async fn inbound_media_validation_and_crypto_errors_are_actionable() {
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;

    let mut config = test_provider();
    assert!(
        WeixinProvider::validate_media_download_url(&config, "not a url")
            .unwrap_err()
            .to_string()
            .contains("URL is invalid")
    );
    assert!(
        WeixinProvider::validate_media_download_url(&config, "https://example.com/file")
            .unwrap_err()
            .to_string()
            .contains("host is not allowed")
    );
    assert!(WeixinProvider::parse_aes_key("%%%", "file")
        .unwrap_err()
        .to_string()
        .contains("base64 decode failed"));
    assert!(WeixinProvider::parse_aes_key("c2hvcnQ=", "file")
        .unwrap_err()
        .to_string()
        .contains("must decode to 16 raw bytes"));
    assert!(
        WeixinProvider::decrypt_aes_128_ecb(&[1, 2, 3], "MDEyMzQ1Njc4OWFiY2RlZg==", "file")
            .unwrap_err()
            .to_string()
            .contains("AES decrypt failed")
    );
    assert!(WeixinProvider::hex_to_bytes("f").is_none());
    for (value, expected) in [
        (1, "pcm"),
        (2, "adpcm"),
        (3, "feature"),
        (4, "speex"),
        (5, "amr"),
        (7, "mp3"),
        (8, "ogg-speex"),
        (99, "unknown"),
    ] {
        assert_eq!(WeixinProvider::voice_codec_name(value), expected);
    }

    let data_dir = tempfile::tempdir().unwrap();
    let provider = WeixinProvider::new_with_data_dir(data_dir.path());
    let missing_url = provider
        .download_and_decrypt_media(&config, None, None, None, "missing")
        .await
        .unwrap_err()
        .to_string();
    assert!(missing_url.contains("no download URL"));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind media error server");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let service = service_fn(|request: Request<Incoming>| async move {
                    let response = if request.uri().path() == "/status" {
                        Response::builder()
                            .status(503)
                            .body(Full::new(Bytes::from_static(b"unavailable")))
                    } else {
                        Response::builder()
                            .status(200)
                            .body(Full::new(Bytes::from(vec![0; MAX_INBOUND_MEDIA_BYTES + 1])))
                    };
                    Ok::<_, hyper::Error>(response.unwrap())
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let status_error = provider
        .download_and_decrypt_media(
            &config,
            Some(&format!("http://127.0.0.1:{port}/status")),
            None,
            None,
            "status",
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(status_error.contains("status=503"));
    let oversized = provider
        .download_and_decrypt_media(
            &config,
            Some(&format!("http://127.0.0.1:{port}/oversized")),
            None,
            None,
            "oversized",
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(oversized.contains("exceeds 100 MiB"), "{oversized}");

    config.base_url = Some("http://127.0.0.1:1".to_string());
    let network_error = provider
        .download_and_decrypt_media(
            &config,
            Some("http://127.0.0.1:1/file"),
            None,
            None,
            "network",
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(network_error.contains("download failed"));
}

#[tokio::test]
async fn structured_progress_typing_and_final_share_run_id_then_release_it() {
    use bifrost_agent::{AgentTurnProgressEvent, ToolCallLog};
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use std::sync::{Arc, Mutex};

    let requests = Arc::new(Mutex::new(Vec::<(String, serde_json::Value)>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind progress mock server");
    let port = listener.local_addr().expect("progress mock address").port();
    let requests_for_server = Arc::clone(&requests);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let requests = Arc::clone(&requests_for_server);
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| {
                    let requests = Arc::clone(&requests);
                    async move {
                        let path = request.uri().path().to_string();
                        let body = request
                            .into_body()
                            .collect()
                            .await
                            .expect("collect progress request")
                            .to_bytes();
                        let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
                        requests
                            .lock()
                            .expect("lock progress requests")
                            .push((path.clone(), json));
                        let response = if path.ends_with("/ilink/bot/getconfig") {
                            r#"{"ret":0,"typing_ticket":"typing-ticket-1"}"#
                        } else {
                            r#"{"ret":0}"#
                        };
                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(200)
                                .body(Full::new(Bytes::from_static(response.as_bytes())))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });

    let data_dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(WeixinProvider::new_with_data_dir(data_dir.path()));
    let mut config = test_provider();
    config.base_url = Some(format!("http://127.0.0.1:{port}"));
    let target = test_target();
    provider
        .store_context_for_test(&config, &target.receive_id, "progress-context")
        .unwrap();

    let mut session = crate::im_gateway::weixin_progress::WeixinProgressSession::start_for_test(
        Arc::clone(&provider),
        config.clone(),
        target.clone(),
        Duration::from_millis(20),
    )
    .await;
    let channel_run_id = session.channel_run_id().to_string();
    session
        .apply_events(vec![
            AgentTurnProgressEvent::ToolStarted {
                tool_name: "exec_command".to_string(),
                arguments: "echo first".to_string(),
            },
            AgentTurnProgressEvent::ToolStarted {
                tool_name: "exec_command".to_string(),
                arguments: "echo second".to_string(),
            },
            AgentTurnProgressEvent::ToolFinished {
                log: ToolCallLog {
                    tool_name: "exec_command".to_string(),
                    arguments: "echo second".to_string(),
                    result: "second".to_string(),
                    success: true,
                },
                duration_ms: 1,
            },
            AgentTurnProgressEvent::ToolFinished {
                log: ToolCallLog {
                    tool_name: "exec_command".to_string(),
                    arguments: "echo first".to_string(),
                    result: "first".to_string(),
                    success: true,
                },
                duration_ms: 1,
            },
        ])
        .await;
    provider
        .send_text_with_client_id(&config, &target, "final", "final-with-run")
        .await
        .expect("send final with active run");
    tokio::time::sleep(Duration::from_millis(55)).await;
    session.finish().await;
    provider.end_channel_run(&config, &target, &channel_run_id);
    provider
        .send_text_with_client_id(&config, &target, "after", "after-run")
        .await
        .expect("send after released run");
    let dropped_session =
        crate::im_gateway::weixin_progress::WeixinProgressSession::start_for_test(
            Arc::clone(&provider),
            config.clone(),
            target.clone(),
            Duration::from_millis(20),
        )
        .await;
    let dropped_run_id = dropped_session.channel_run_id().to_string();
    drop(dropped_session);
    tokio::time::sleep(Duration::from_millis(40)).await;
    provider.end_channel_run(&config, &target, &dropped_run_id);

    let registry = crate::im_gateway::progress_card::ImAgentProgressRegistry::new();
    let first_registry_session = registry
        .start_weixin(
            "weixin-registry",
            Arc::clone(&provider),
            config.clone(),
            target.clone(),
        )
        .await;
    assert!(!first_registry_session
        .lock()
        .await
        .channel_run_id()
        .is_empty());
    let replacement_registry_session = registry
        .start_weixin(
            "weixin-registry",
            Arc::clone(&provider),
            config.clone(),
            target.clone(),
        )
        .await;
    assert!(registry
        .weixin_channel_run_id("weixin-registry")
        .await
        .is_some());
    registry
        .apply_events(
            "weixin-registry",
            vec![AgentTurnProgressEvent::ToolFinished {
                log: ToolCallLog {
                    tool_name: "read_file".to_string(),
                    arguments: "  README.md  ".to_string(),
                    result: "done".to_string(),
                    success: false,
                },
                duration_ms: 2,
            }],
        )
        .await;
    assert!(
        registry
            .update_queue_state("weixin-registry", Vec::new(), false, None)
            .await
    );
    assert!(
        registry
            .update_runner_summary(
                "weixin-registry",
                crate::im_gateway::progress_card::ProgressRunnerSummary::default(),
            )
            .await
    );
    assert!(registry
        .finish("weixin-registry", Some("done".to_string()), false)
        .await
        .is_none());
    drop(replacement_registry_session);
    assert!(registry
        .weixin_channel_run_id("weixin-registry")
        .await
        .is_none());

    let requests = requests.lock().expect("lock captured requests").clone();
    let typing_statuses = requests
        .iter()
        .filter(|(path, _)| path.ends_with("/ilink/bot/sendtyping"))
        .filter_map(|(_, body)| body["status"].as_u64())
        .collect::<Vec<_>>();
    assert_eq!(typing_statuses.first(), Some(&1));
    assert_eq!(typing_statuses.last(), Some(&2));
    assert!(
        typing_statuses
            .iter()
            .filter(|status| **status == 2)
            .count()
            >= 4,
        "normal finish and dropped session must both cancel typing"
    );
    assert!(
        typing_statuses
            .iter()
            .filter(|status| **status == 1)
            .count()
            >= 2,
        "expected start plus keepalive typing events"
    );

    let messages = requests
        .iter()
        .filter(|(path, _)| path.ends_with("/ilink/bot/sendmessage"))
        .map(|(_, body)| body)
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 7);
    let item_types = messages[..4]
        .iter()
        .map(|body| body["msg"]["item_list"][0]["type"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(item_types, vec![11, 11, 12, 12]);
    assert!(messages[..5]
        .iter()
        .all(|body| body["msg"]["run_id"] == channel_run_id));
    assert!(messages[..4]
        .iter()
        .all(|body| body["msg"]["message_state"] == 2));
    assert_eq!(
        messages[2]["msg"]["item_list"][0]["tool_call_result_item"]["tool_call_id"],
        format!("{}-tool-0002", &channel_run_id[..8])
    );
    assert_eq!(
        messages[3]["msg"]["item_list"][0]["tool_call_result_item"]["tool_call_id"],
        format!("{}-tool-0001", &channel_run_id[..8])
    );
    assert!(messages[5]["msg"].get("run_id").is_none());
    let client_ids = messages
        .iter()
        .filter_map(|body| body["msg"]["client_id"].as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(client_ids.len(), messages.len());
}
