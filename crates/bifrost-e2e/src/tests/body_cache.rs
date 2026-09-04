use crate::curl::CurlCommand;
use crate::mock::EnhancedMockServer;
use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use bifrost_admin::{AdminState, QueryParams, TrafficRecord};
use bytes::Bytes;
use futures_util::stream;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Frame;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

async fn get_latest_record(admin_state: &Arc<AdminState>) -> Result<TrafficRecord, String> {
    let Some(db_store) = admin_state.traffic_db_store.clone() else {
        return Err("Traffic DB not configured".to_string());
    };

    tokio::task::spawn_blocking(move || {
        let result = db_store.query(&QueryParams {
            limit: Some(1),
            ..Default::default()
        });
        let id = result
            .records
            .first()
            .map(|r| r.id.clone())
            .ok_or_else(|| "No traffic records found".to_string())?;
        db_store
            .get_by_id(&id)
            .ok_or_else(|| "Failed to get traffic record detail".to_string())
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {}", e))?
}

async fn get_all_records(admin_state: &Arc<AdminState>) -> Result<Vec<TrafficRecord>, String> {
    let Some(db_store) = admin_state.traffic_db_store.clone() else {
        return Err("Traffic DB not configured".to_string());
    };

    tokio::task::spawn_blocking(move || {
        let result = db_store.query(&QueryParams {
            limit: Some(100),
            ..Default::default()
        });
        result
            .records
            .into_iter()
            .map(|summary| {
                db_store
                    .get_by_id(&summary.id)
                    .ok_or_else(|| format!("Failed to load traffic detail for {}", summary.id))
            })
            .collect::<Result<Vec<_>, _>>()
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {}", e))?
}

struct BinaryTrafficMockServer {
    port: u16,
}

impl BinaryTrafficMockServer {
    async fn start() -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("bind failed: {}", e))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("local_addr failed: {}", e))?
            .port();

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let service = service_fn(handle_binary_mock_request);
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(Self { port })
    }
}

type TestBody = http_body_util::combinators::BoxBody<Bytes, Infallible>;

async fn handle_binary_mock_request(
    req: Request<hyper::body::Incoming>,
) -> Result<Response<TestBody>, Infallible> {
    let path = req.uri().path();
    let response = match path {
        "/image.png" => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "image/png")
            .body(
                Full::new(Bytes::from_static(include_bytes!(
                    "../../../../web/public/favicon.png"
                )))
                .boxed(),
            )
            .unwrap(),
        "/download.bin" => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", (2 * 1024 * 1024).to_string())
            .body(Full::new(Bytes::from(vec![7u8; 2 * 1024 * 1024])).boxed())
            .unwrap(),
        "/speedtest.bin" => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/octet-stream")
            .body(chunked_body(128, 32 * 1024, 9))
            .unwrap(),
        "/live.ts" => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "video/mp2t")
            .body(chunked_body(96, 32 * 1024, 5))
            .unwrap(),
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from_static(b"not found")).boxed())
            .unwrap(),
    };

    Ok(response)
}

fn chunked_body(chunks: usize, chunk_size: usize, fill: u8) -> TestBody {
    let stream = stream::iter((0..chunks).map(move |_| {
        Ok::<Frame<Bytes>, Infallible>(Frame::data(Bytes::from(vec![fill; chunk_size])))
    }));
    StreamBody::new(stream).boxed()
}

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "body_cache_request_body_small",
            "请求体缓存 - 小请求体存储",
            "body_cache",
            test_request_body_small,
        ),
        TestCase::standalone(
            "body_cache_request_body_with_rule",
            "请求体缓存 - 带规则的请求体",
            "body_cache",
            test_request_body_with_rule,
        ),
        TestCase::standalone(
            "body_cache_request_body_post",
            "请求体缓存 - POST 请求体正确存储",
            "body_cache",
            test_request_body_post,
        ),
        TestCase::standalone(
            "binary_performance_mode_skips_binary_body_persistence",
            "二进制性能模式 - 下载/直播/测速保留 metadata 但不落 body",
            "body_cache",
            test_binary_performance_mode_skips_binary_body_persistence,
        ),
        TestCase::standalone(
            "binary_capture_mode_records_binary_traffic",
            "二进制性能模式关闭 - 下载/直播/测速恢复记录和落盘",
            "body_cache",
            test_binary_capture_mode_records_binary_traffic,
        ),
    ]
}

async fn test_request_body_small() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "response-ok");

    let port = portpicker::pick_unused_port().unwrap();
    let (_proxy, admin_state) = ProxyInstance::start_with_admin(
        port,
        vec![&format!("test.local host://127.0.0.1:{}", mock.port)],
        false,
        false,
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .method("POST")
    .data("small-request-body")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;
    result.assert_body_contains("response-ok")?;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let record = get_latest_record(&admin_state).await?;

    let body_ref = record
        .request_body_ref
        .ok_or("request_body_ref is None - body was not stored")?;

    let Some(body_store) = admin_state.body_store.as_ref() else {
        return Err("Body store not configured".to_string());
    };
    let data = body_store
        .read()
        .load(&body_ref)
        .ok_or("Failed to load request body")?;
    if !data.contains("small-request-body") {
        return Err(format!(
            "Expected 'small-request-body' in body, got: {}",
            data
        ));
    }

    Ok(())
}

async fn test_request_body_with_rule() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let port = portpicker::pick_unused_port().unwrap();
    let (_proxy, admin_state) = ProxyInstance::start_with_admin(
        port,
        vec![&format!(
            "test.local host://127.0.0.1:{} reqReplace://original=modified",
            mock.port
        )],
        false,
        false,
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .method("POST")
    .data("original-content")
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;

    let req = mock.last_request().ok_or("No request received by mock")?;
    let body = req.body.ok_or("No body in request")?;
    if !body.contains("modified-content") {
        return Err(format!(
            "Expected 'modified-content' in forwarded body, got: {}",
            body
        ));
    }

    tokio::time::sleep(Duration::from_millis(200)).await;

    let record = get_latest_record(&admin_state).await?;

    let body_ref = record
        .request_body_ref
        .ok_or("request_body_ref is None - body was not stored")?;

    let Some(body_store) = admin_state.body_store.as_ref() else {
        return Err("Body store not configured".to_string());
    };
    let data = body_store
        .read()
        .load(&body_ref)
        .ok_or("Failed to load request body")?;
    if !data.contains("modified-content") {
        return Err(format!(
            "Expected final forwarded body in store, got: {}",
            data
        ));
    }

    Ok(())
}

async fn test_request_body_post() -> Result<(), String> {
    let mock = EnhancedMockServer::start().await;
    mock.set_response(200, "ok");

    let port = portpicker::pick_unused_port().unwrap();
    let (_proxy, admin_state) = ProxyInstance::start_with_admin(
        port,
        vec![&format!("test.local host://127.0.0.1:{}", mock.port)],
        false,
        false,
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let json_body = r#"{"name":"test","value":123}"#;
    let result = CurlCommand::with_proxy(
        &format!("http://127.0.0.1:{}", port),
        "http://test.local/api",
    )
    .method("POST")
    .header("Content-Type", "application/json")
    .data(json_body)
    .execute()
    .await
    .map_err(|e| format!("curl failed: {}", e))?;

    result.assert_success()?;

    let req = mock.last_request().ok_or("No request received by mock")?;
    let received_body = req.body.ok_or("No body in request")?;
    if received_body != json_body {
        return Err(format!(
            "Expected body '{}', got: '{}'",
            json_body, received_body
        ));
    }

    tokio::time::sleep(Duration::from_millis(200)).await;

    let record = get_latest_record(&admin_state).await?;

    let body_ref = record
        .request_body_ref
        .ok_or("request_body_ref is None - body was not stored")?;

    let Some(body_store) = admin_state.body_store.as_ref() else {
        return Err("Body store not configured".to_string());
    };
    let data = body_store
        .read()
        .load(&body_ref)
        .ok_or("Failed to load request body")?;
    if data != json_body {
        return Err(format!(
            "Expected '{}' in stored body, got: '{}'",
            json_body, data
        ));
    }

    Ok(())
}

async fn test_binary_performance_mode_skips_binary_body_persistence() -> Result<(), String> {
    let mock = BinaryTrafficMockServer::start().await?;
    let port = portpicker::pick_unused_port().unwrap();
    let (_proxy, admin_state) = ProxyInstance::start_with_admin(
        port,
        vec![&format!("media.local host://127.0.0.1:{}", mock.port)],
        false,
        false,
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    admin_state.set_binary_traffic_performance_mode(true);
    tokio::time::sleep(Duration::from_millis(100)).await;

    for path in ["/image.png", "/download.bin", "/live.ts", "/speedtest.bin"] {
        let result = CurlCommand::with_proxy(
            &format!("http://127.0.0.1:{}", port),
            &format!("http://media.local{}", path),
        )
        .verbose(false)
        .execute()
        .await
        .map_err(|e| format!("curl failed for {}: {}", path, e))?;
        result.assert_success()?;
    }

    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut records = get_all_records(&admin_state).await?;
    records.sort_by(|left, right| left.url.cmp(&right.url));
    let urls = records
        .iter()
        .map(|record| record.url.as_str())
        .collect::<Vec<_>>();
    let expected = vec![
        "http://media.local/download.bin",
        "http://media.local/image.png",
        "http://media.local/live.ts",
        "http://media.local/speedtest.bin",
    ];
    if urls != expected {
        return Err(format!(
            "Expected all binary metadata records in performance mode, got {:?}",
            urls
        ));
    }

    let image_record = records
        .iter()
        .find(|record| record.url == "http://media.local/image.png")
        .ok_or_else(|| "Expected image metadata record to be present".to_string())?;
    if image_record.response_body_ref.is_none() {
        return Err("Expected image response_body_ref to be present".to_string());
    }

    for record in records
        .iter()
        .filter(|record| record.url != "http://media.local/image.png")
    {
        if record.response_body_ref.is_some() {
            return Err(format!(
                "Expected performance-mode binary body to be skipped for {}",
                record.url
            ));
        }
    }

    let Some(body_store) = admin_state.body_store.as_ref() else {
        return Err("Body store not configured".to_string());
    };
    let stats = body_store.read().stats();
    if stats.file_count != 1 {
        return Err(format!(
            "Expected only 1 body cache file in performance mode, got {}",
            stats.file_count
        ));
    }

    Ok(())
}

async fn test_binary_capture_mode_records_binary_traffic() -> Result<(), String> {
    let mock = BinaryTrafficMockServer::start().await?;
    let port = portpicker::pick_unused_port().unwrap();
    let (_proxy, admin_state) = ProxyInstance::start_with_admin(
        port,
        vec![&format!("media.local host://127.0.0.1:{}", mock.port)],
        false,
        false,
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    admin_state.set_binary_traffic_performance_mode(false);
    tokio::time::sleep(Duration::from_millis(100)).await;

    for path in ["/download.bin", "/live.ts", "/speedtest.bin"] {
        let result = CurlCommand::with_proxy(
            &format!("http://127.0.0.1:{}", port),
            &format!("http://media.local{}", path),
        )
        .verbose(false)
        .execute()
        .await
        .map_err(|e| format!("curl failed for {}: {}", path, e))?;
        result.assert_success()?;
    }

    tokio::time::sleep(Duration::from_millis(300)).await;

    let records = get_all_records(&admin_state).await?;
    if records.len() != 3 {
        return Err(format!(
            "Expected 3 binary records when capture is enabled, got {}",
            records.len()
        ));
    }

    let mut urls = records
        .iter()
        .map(|record| record.url.as_str())
        .collect::<Vec<_>>();
    urls.sort_unstable();
    let expected = vec![
        "http://media.local/download.bin",
        "http://media.local/live.ts",
        "http://media.local/speedtest.bin",
    ];
    if urls != expected {
        return Err(format!("Unexpected recorded urls: {:?}", urls));
    }

    if records
        .iter()
        .any(|record| record.response_body_ref.is_none())
    {
        return Err("Expected all binary records to have response_body_ref".to_string());
    }

    let Some(body_store) = admin_state.body_store.as_ref() else {
        return Err("Body store not configured".to_string());
    };
    let stats = body_store.read().stats();
    if stats.file_count != 3 {
        return Err(format!(
            "Expected 3 body cache files when binary capture is enabled, got {}",
            stats.file_count
        ));
    }

    Ok(())
}
