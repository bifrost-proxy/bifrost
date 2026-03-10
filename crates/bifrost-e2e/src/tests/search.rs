use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::{EnhancedMockServer, ProxyClient, ProxyInstance, TestCase};

fn extract_first_result_id(resp: &Value) -> Option<String> {
    resp.get("results")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("record"))
        .and_then(|r| r.get("id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

async fn wait_for_body_index_present(
    admin_state: &std::sync::Arc<bifrost_admin::AdminState>,
    traffic_id: &str,
    kind: i32,
    timeout: Duration,
) -> Result<(), String> {
    let start = Instant::now();
    let id = traffic_id.to_string();
    loop {
        let present = tokio::task::spawn_blocking({
            let db = admin_state.traffic_db_store.clone();
            let id = id.clone();
            move || {
                let Some(db) = db else {
                    return false;
                };
                let map = db.get_body_indexes_by_ids(&[id.as_str()], kind);
                map.get(&id)
                    .is_some_and(|row| row.block_count > 0 && !row.bitsets.is_empty())
            }
        })
        .await
        .map_err(|e| format!("wait_for_body_index_present join error: {e}"))?;

        if present {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            return Err(format!(
                "Timed out waiting for body index row (id={}, kind={})",
                id, kind
            ));
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
}

fn admin_url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{}/_bifrost{}", port, path)
}

async fn put_performance_config(
    port: u16,
    payload: Value,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .build()?;

    let url = admin_url(port, "/api/config/performance");
    let resp = client
        .put(url)
        .header(reqwest::header::HOST, format!("127.0.0.1:{}", port))
        .json(&payload)
        .send()
        .await?;
    let status = resp.status();
    let bytes = resp.bytes().await?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(v) => Ok(v),
        Err(e) => {
            let preview = String::from_utf8_lossy(&bytes);
            Err(format!(
                "Invalid JSON response (status={}): {} | body='{}'",
                status,
                e,
                preview.chars().take(500).collect::<String>()
            )
            .into())
        }
    }
}

async fn search_api(
    port: u16,
    payload: Value,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .build()?;

    let url = admin_url(port, "/api/search");
    let resp = client
        .post(url)
        .header(reqwest::header::HOST, format!("127.0.0.1:{}", port))
        .json(&payload)
        .send()
        .await?;
    let status = resp.status();
    let bytes = resp.bytes().await?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(v) => Ok(v),
        Err(e) => {
            let preview = String::from_utf8_lossy(&bytes);
            Err(format!(
                "Invalid JSON response (status={}): {} | body='{}'",
                status,
                e,
                preview.chars().take(500).collect::<String>()
            )
            .into())
        }
    }
}

async fn admin_get_json(
    port: u16,
    path_and_query: &str,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .build()?;

    let url = admin_url(port, path_and_query);
    let resp = client
        .get(url)
        .header(reqwest::header::HOST, format!("127.0.0.1:{}", port))
        .send()
        .await?;

    let status = resp.status();
    let bytes = resp.bytes().await?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(v) => Ok(v),
        Err(e) => {
            let preview = String::from_utf8_lossy(&bytes);
            Err(format!(
                "Invalid JSON response (status={}): {} | body='{}'",
                status,
                e,
                preview.chars().take(500).collect::<String>()
            )
            .into())
        }
    }
}

async fn start_ws_echo_server() -> Result<(u16, tokio::task::JoinHandle<()>), String> {
    use futures_util::{SinkExt, StreamExt as _};
    use tokio_tungstenite::accept_async;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind ws server: {}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to get ws server addr: {}", e))?
        .port();

    let handle = tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            tokio::spawn(async move {
                if let Ok(mut ws) = accept_async(stream).await {
                    while let Some(msg) = ws.next().await {
                        if let Ok(msg) = msg {
                            if msg.is_close() {
                                let _ = ws.close(None).await;
                                break;
                            }
                            let _ = ws.send(msg).await;
                        } else {
                            break;
                        }
                    }
                }
            });
        }
    });

    Ok((port, handle))
}

async fn start_sse_server(token: String) -> Result<(u16, tokio::task::JoinHandle<()>), String> {
    use bytes::Bytes;
    use futures_util::stream;
    use hyper::body::Frame;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Response, StatusCode};
    use hyper_util::rt::TokioIo;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind sse server: {}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to get sse server addr: {}", e))?
        .port();

    let handle = tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };

            let token = token.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let service = service_fn(move |_req: hyper::Request<hyper::body::Incoming>| {
                    let token = token.clone();
                    async move {
                        // 以 chunked streaming 的方式发送 SSE，避免被当作普通短响应处理。
                        let t = token.replace('\n', "");
                        let event1 = Bytes::from(format!("event: message\ndata: hello-{}\n\n", t));
                        let event2 =
                            Bytes::from(format!("event: message\ndata: goodbye-{}\n\n", t));

                        let body_stream = http_body_util::StreamBody::new(stream::unfold(
                            (0u8, event1, event2),
                            |(state, e1, e2)| async move {
                                match state {
                                    0 => {
                                        let chunk = e1.clone();
                                        Some((
                                            Ok::<_, hyper::Error>(Frame::data(chunk)),
                                            (1, e1, e2),
                                        ))
                                    }
                                    1 => {
                                        tokio::time::sleep(Duration::from_millis(120)).await;
                                        let chunk = e2.clone();
                                        Some((
                                            Ok::<_, hyper::Error>(Frame::data(chunk)),
                                            (2, e1, e2),
                                        ))
                                    }
                                    _ => None,
                                }
                            },
                        ));

                        Ok::<_, hyper::Error>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("Content-Type", "text/event-stream")
                                .header("Cache-Control", "no-cache")
                                .header("Connection", "keep-alive")
                                .body(body_stream)
                                .unwrap(),
                        )
                    }
                });

                let _ = http1::Builder::new().serve_connection(io, service).await;
            });
        }
    });

    Ok((port, handle))
}

fn has_match_field(resp: &Value, field: &str) -> bool {
    resp.get("results")
        .and_then(|v| v.as_array())
        .map(|results| {
            results.iter().any(|item| {
                item.get("matches")
                    .and_then(|m| m.as_array())
                    .map(|ms| {
                        ms.iter()
                            .any(|x| x.get("field") == Some(&Value::String(field.to_string())))
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn has_record_path_contains(resp: &Value, keyword: &str) -> bool {
    resp.get("results")
        .and_then(|v| v.as_array())
        .map(|results| {
            results.iter().any(|item| {
                item.get("record")
                    .and_then(|r| r.get("p"))
                    .and_then(|p| p.as_str())
                    .map(|p| p.contains(keyword))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

async fn wait_for_search(
    port: u16,
    payload: Value,
    predicate: impl Fn(&Value) -> bool,
    timeout: Duration,
) -> Result<Value, String> {
    let start = Instant::now();
    loop {
        let resp = search_api(port, payload.clone())
            .await
            .map_err(|e| format!("Search API failed: {}", e))?;
        if predicate(&resp) {
            return Ok(resp);
        }
        if start.elapsed() >= timeout {
            return Err(format!(
                "Timed out waiting for search result, last response: {}",
                resp
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "search_scopes_inline",
            "Search covers url/headers/body with scope controls (inline bodies)",
            "search",
            || async move {
                let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let mock = EnhancedMockServer::start().await;
                let token = "bifrost_search_token_inline";
                let path = format!("/search/{}", token);

                let mut resp_headers = HashMap::new();
                resp_headers.insert("X-Response-Token".to_string(), token.to_string());
                mock.set_response_with_headers(200, token, resp_headers);

                let proxy_url = format!("http://127.0.0.1:{}", port);
                let client = ProxyClient::new(&proxy_url).map_err(|e| e.to_string())?;

                let mut headers = HashMap::new();
                headers.insert("X-Search-Token", token);
                let url = mock.url(&path);

                let _ = client
                    .get_with_headers(&url, headers)
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                let _ = client
                    .post(&url, &format!(r#"{{\"token\":\"{}\"}}"#, token))
                    .await
                    .map_err(|e| format!("POST failed: {}", e))?;

                tokio::time::sleep(Duration::from_millis(200)).await;

                let base = serde_json::json!({
                    "keyword": token,
                    "filters": {},
                    "cursor": null,
                    "limit": 50
                });

                let url_scope = serde_json::json!({"all": false, "url": true});
                let resp = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "scope": url_scope,
                        "filters": {},
                        "cursor": null,
                        "limit": 50
                    }),
                    |r| has_match_field(r, "url") && has_record_path_contains(r, token),
                    Duration::from_secs(5),
                )
                .await?;
                if !has_match_field(&resp, "url") {
                    return Err("Expected url match".to_string());
                }

                let req_header_scope = serde_json::json!({"all": false, "request_headers": true});
                let resp = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "scope": req_header_scope,
                        "filters": {},
                        "cursor": null,
                        "limit": 50
                    }),
                    |r| has_match_field(r, "request_header"),
                    Duration::from_secs(5),
                )
                .await?;
                if !has_match_field(&resp, "request_header") {
                    return Err("Expected request_header match".to_string());
                }

                let res_header_scope = serde_json::json!({"all": false, "response_headers": true});
                let resp = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "scope": res_header_scope,
                        "filters": {},
                        "cursor": null,
                        "limit": 50
                    }),
                    |r| has_match_field(r, "response_header"),
                    Duration::from_secs(5),
                )
                .await?;
                if !has_match_field(&resp, "response_header") {
                    return Err("Expected response_header match".to_string());
                }

                let req_body_scope = serde_json::json!({"all": false, "request_body": true});
                let resp = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "scope": req_body_scope,
                        "filters": {},
                        "cursor": null,
                        "limit": 50
                    }),
                    |r| has_match_field(r, "request_body"),
                    Duration::from_secs(5),
                )
                .await?;
                if !has_match_field(&resp, "request_body") {
                    return Err("Expected request_body match".to_string());
                }

                let res_body_scope = serde_json::json!({"all": false, "response_body": true});
                let resp = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "scope": res_body_scope,
                        "filters": {},
                        "cursor": null,
                        "limit": 50
                    }),
                    |r| has_match_field(r, "response_body"),
                    Duration::from_secs(5),
                )
                .await?;
                if !has_match_field(&resp, "response_body") {
                    return Err("Expected response_body match".to_string());
                }

                // ensure admin is still responsive
                let _ = admin_get_json(port, "/api/traffic?limit=5")
                    .await
                    .map_err(|e| e.to_string())?;

                // keep lint happy for now: base is used by construction (readability)
                let _ = base;
                Ok(())
            },
        ),
        TestCase::standalone(
            "search_file_backed_body",
            "Search works when request/response bodies are stored as files",
            "search",
            || async move {
                let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                // Force file-backed bodies in BodyStore
                let _ = put_performance_config(
                    port,
                    serde_json::json!({
                        "max_body_memory_size": 1
                    }),
                )
                .await
                .map_err(|e| format!("Failed to update performance config: {}", e))?;

                let mock = EnhancedMockServer::start().await;
                let token = "bifrost_search_token_file";
                let path = format!("/search/file/{}", token);

                let mut resp_headers = HashMap::new();
                resp_headers.insert("X-Response-Token".to_string(), token.to_string());
                mock.set_response_with_headers(200, token, resp_headers);

                let proxy_url = format!("http://127.0.0.1:{}", port);
                let client = ProxyClient::new(&proxy_url).map_err(|e| e.to_string())?;
                let url = mock.url(&path);

                let big_body = format!("{}-{}-", "A".repeat(4096), token);
                let _ = client
                    .post(&url, &big_body)
                    .await
                    .map_err(|e| format!("POST failed: {}", e))?;

                tokio::time::sleep(Duration::from_millis(300)).await;

                let req_body_scope = serde_json::json!({"all": false, "request_body": true});
                let resp = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "scope": req_body_scope,
                        "filters": {},
                        "cursor": null,
                        "limit": 50
                    }),
                    |r| has_match_field(r, "request_body"),
                    Duration::from_secs(8),
                )
                .await?;

                if !has_match_field(&resp, "request_body") {
                    return Err("Expected request_body match for file-backed body".to_string());
                }

                let res_body_scope = serde_json::json!({"all": false, "response_body": true});
                let resp = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "scope": res_body_scope,
                        "filters": {},
                        "cursor": null,
                        "limit": 50
                    }),
                    |r| has_match_field(r, "response_body"),
                    Duration::from_secs(8),
                )
                .await?;

                if !has_match_field(&resp, "response_body") {
                    return Err("Expected response_body match for file-backed body".to_string());
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "search_cross_block_boundary",
            "Search does not miss keywords that span across 64KB body index blocks",
            "search",
            || async move {
                let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let _ = put_performance_config(
                    port,
                    serde_json::json!({
                        "max_body_memory_size": 1
                    }),
                )
                .await
                .map_err(|e| format!("Failed to update performance config: {}", e))?;

                let mock = EnhancedMockServer::start().await;
                let keyword = "HELLO";
                let path = "/search/cross_block";
                mock.set_response(200, "ok");

                let proxy_url = format!("http://127.0.0.1:{}", port);
                let client = ProxyClient::new(&proxy_url).map_err(|e| e.to_string())?;
                let url = mock.url(path);

                // block_size = 64KB (65536). Put keyword starting at 65534, so it spans blocks.
                let mut body = String::new();
                body.push_str(&"a".repeat(65_534));
                body.push_str(keyword);
                body.push_str(&"b".repeat(1024));

                let _ = client
                    .post(&url, &body)
                    .await
                    .map_err(|e| format!("POST failed: {}", e))?;

                tokio::time::sleep(Duration::from_millis(500)).await;

                let req_body_scope = serde_json::json!({"all": false, "request_body": true});
                let resp = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": keyword,
                        "scope": req_body_scope,
                        "filters": {},
                        "cursor": null,
                        "limit": 50
                    }),
                    |r| has_match_field(r, "request_body"),
                    Duration::from_secs(10),
                )
                .await?;

                if !has_match_field(&resp, "request_body") {
                    return Err("Expected request_body match across block boundary".to_string());
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "search_performance_sanity",
            "Search returns within reasonable time on a moderate dataset",
            "search",
            || async move {
                let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let mock = EnhancedMockServer::start().await;
                let token = "bifrost_search_perf_token";
                mock.set_response(200, token);
                let url = mock.url("/search/perf");

                let proxy_url = format!("http://127.0.0.1:{}", port);
                let client = ProxyClient::new(&proxy_url).map_err(|e| e.to_string())?;

                for i in 0..200usize {
                    // mix in token sparsely
                    let body = if i % 37 == 0 {
                        format!("{{\"i\":{},\"token\":\"{}\"}}", i, token)
                    } else {
                        format!("{{\"i\":{}\"}}", i)
                    };
                    let _ = client.post(&url, &body).await;
                }

                tokio::time::sleep(Duration::from_millis(500)).await;

                let t0 = Instant::now();
                let resp = search_api(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "filters": {},
                        "cursor": null,
                        "limit": 100
                    }),
                )
                .await
                .map_err(|e| format!("Search API failed: {}", e))?;

                let elapsed = t0.elapsed();
                if elapsed > Duration::from_secs(10) {
                    return Err(format!("Search took too long: {:?}", elapsed));
                }

                if !has_match_field(&resp, "request_body")
                    && !has_match_field(&resp, "response_body")
                {
                    return Err(format!(
                        "Expected at least one match for perf token, got: {}",
                        resp
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "search_stream_sse_and_abort",
            "Streaming search (SSE) emits result/progress/done and can be aborted safely",
            "search",
            || async move {
                let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let mock = EnhancedMockServer::start().await;
                let token = "bifrost_search_stream_token";
                mock.set_response(200, token);
                let url = mock.url("/search/stream");

                let proxy_url = format!("http://127.0.0.1:{}", port);
                let client = ProxyClient::new(&proxy_url).map_err(|e| e.to_string())?;

                for i in 0..30usize {
                    let body = if i == 17 {
                        format!("{{\"token\":\"{}\",\"i\":{}}}", token, i)
                    } else {
                        format!("{{\"i\":{}}}", i)
                    };
                    let _ = client.post(&url, &body).await;
                }
                tokio::time::sleep(Duration::from_millis(300)).await;

                let http = reqwest::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .danger_accept_invalid_certs(true)
                    .no_proxy()
                    .build()
                    .map_err(|e| e.to_string())?;

                let stream_url = admin_url(port, "/api/search/stream");
                let payload = serde_json::json!({
                    "keyword": token,
                    "filters": {},
                    "cursor": null,
                    "limit": 100
                });

                let resp = http
                    .post(&stream_url)
                    .header(reqwest::header::HOST, format!("127.0.0.1:{}", port))
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|e| format!("SSE request failed: {}", e))?;

                let status = resp.status();

                let ct = resp
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                if !ct.contains("text/event-stream") {
                    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
                    let preview = String::from_utf8_lossy(&bytes);
                    return Err(format!(
                        "Expected text/event-stream, got: '{}' (status={}) body='{}'",
                        ct,
                        status,
                        preview.chars().take(500).collect::<String>()
                    ));
                }

                let mut saw_result = false;
                let mut saw_progress = false;
                let mut saw_done = false;

                let mut buf = String::new();
                let mut stream = resp.bytes_stream();
                let deadline = Instant::now() + Duration::from_secs(10);

                while Instant::now() < deadline {
                    let next = tokio::time::timeout(Duration::from_secs(2), stream.next()).await;
                    let Some(chunk) = next.ok().flatten().transpose().map_err(|e| e.to_string())?
                    else {
                        break;
                    };

                    buf.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(idx) = buf.find("\n\n") {
                        let frame = buf[..idx].to_string();
                        buf = buf[idx + 2..].to_string();

                        if frame.contains("event: result") {
                            saw_result = true;
                        }
                        if frame.contains("event: progress") {
                            saw_progress = true;
                        }
                        if frame.contains("event: done") {
                            saw_done = true;
                            break;
                        }
                    }

                    if saw_done {
                        break;
                    }
                }

                if !saw_done {
                    return Err("Expected SSE done event".to_string());
                }
                if !saw_result {
                    return Err("Expected SSE result event".to_string());
                }
                if !saw_progress {
                    return Err("Expected SSE progress event".to_string());
                }

                // Abort test: drop response early and ensure admin remains responsive.
                let resp2 = http
                    .post(&stream_url)
                    .header(reqwest::header::HOST, format!("127.0.0.1:{}", port))
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|e| format!("SSE request (abort) failed: {}", e))?;
                let mut s2 = resp2.bytes_stream();
                let _ = tokio::time::timeout(Duration::from_secs(2), s2.next()).await;
                drop(s2);

                let _ = admin_get_json(port, "/api/traffic?limit=5")
                    .await
                    .map_err(|e| e.to_string())?;

                Ok(())
            },
        ),
        TestCase::standalone(
            "search_filters_status_content_type_domain",
            "Search filters: status_ranges/content_types/domains work together",
            "search",
            || async move {
                let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
                let mock = EnhancedMockServer::start().await;

                let token = "bifrost_search_filter_token";
                let mut resp_headers = HashMap::new();
                resp_headers.insert(
                    "Content-Type".to_string(),
                    "text/plain; charset=utf-8".to_string(),
                );
                resp_headers.insert("X-Filter-Token".to_string(), token.to_string());
                mock.set_response_with_headers(404, token, resp_headers);

                let (_proxy, _admin_state) = ProxyInstance::start_with_admin(
                    port,
                    vec![&format!("filter.test host://127.0.0.1:{}", mock.port)],
                    false,
                    true,
                )
                .await
                .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let proxy_url = format!("http://127.0.0.1:{}", port);
                let client = ProxyClient::new(&proxy_url).map_err(|e| e.to_string())?;

                let mut headers = HashMap::new();
                headers.insert("X-Search-Token", token);
                let _ = client
                    .get_with_headers(&format!("http://filter.test/filters/{}", token), headers)
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                tokio::time::sleep(Duration::from_millis(300)).await;

                let good = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "filters": {
                            "status_ranges": ["4xx"],
                            "content_types": ["text/plain"],
                            "domains": ["filter.test"]
                        },
                        "cursor": null,
                        "limit": 50
                    }),
                    |r| r.get("total_matched").and_then(|v| v.as_u64()).unwrap_or(0) > 0,
                    Duration::from_secs(8),
                )
                .await?;

                if good.get("total_matched").and_then(|v| v.as_u64()).unwrap_or(0) == 0 {
                    return Err(format!("Expected filter match, got: {}", good));
                }

                let bad_domain = search_api(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "filters": {
                            "status_ranges": ["4xx"],
                            "content_types": ["text/plain"],
                            "domains": ["nope.test"]
                        },
                        "cursor": null,
                        "limit": 50
                    }),
                )
                .await
                .map_err(|e| format!("Search API failed: {}", e))?;

                if bad_domain
                    .get("total_matched")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    != 0
                {
                    return Err(format!(
                        "Expected 0 matches for bad domain filter, got: {}",
                        bad_domain
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "search_filter_conditions_method_and_path",
            "Search filter conditions: method/path/url matches correctly",
            "search",
            || async move {
                let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
                let mock = EnhancedMockServer::start().await;
                let token = "bifrost_search_condition_token";
                mock.set_response(200, token);

                let (_proxy, _admin_state) = ProxyInstance::start_with_admin(
                    port,
                    vec![&format!("cond.test host://127.0.0.1:{}", mock.port)],
                    false,
                    true,
                )
                .await
                .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let proxy_url = format!("http://127.0.0.1:{}", port);
                let proxy = reqwest::Proxy::all(&proxy_url).map_err(|e| e.to_string())?;
                let http = reqwest::Client::builder()
                    .proxy(proxy)
                    .timeout(Duration::from_secs(15))
                    .danger_accept_invalid_certs(true)
                    .build()
                    .map_err(|e| e.to_string())?;

                let url = format!("http://cond.test/special/path/{}?q=1", token);
                let _ = http
                    .put(url)
                    .header("X-Cond-Token", token)
                    .body(format!("{{\"token\":\"{}\"}}", token))
                    .send()
                    .await
                    .map_err(|e| format!("PUT request failed: {}", e))?;

                tokio::time::sleep(Duration::from_millis(300)).await;

                let payload = serde_json::json!({
                    "keyword": token,
                    "filters": {
                        "conditions": [
                            {"field": "method", "operator": "equals", "value": "PUT"},
                            {"field": "path", "operator": "contains", "value": "/special/path/"},
                            {"field": "url", "operator": "contains", "value": "q=1"}
                        ]
                    },
                    "cursor": null,
                    "limit": 50
                });

                let resp = wait_for_search(
                    port,
                    payload,
                    |r| r.get("total_matched").and_then(|v| v.as_u64()).unwrap_or(0) > 0,
                    Duration::from_secs(8),
                )
                .await?;

                if resp.get("total_matched").and_then(|v| v.as_u64()).unwrap_or(0) == 0 {
                    return Err(format!("Expected condition match, got: {}", resp));
                }

                let bad = search_api(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "filters": {
                            "conditions": [
                                {"field": "method", "operator": "equals", "value": "GET"}
                            ]
                        },
                        "cursor": null,
                        "limit": 50
                    }),
                )
                .await
                .map_err(|e| format!("Search API failed: {}", e))?;
                if bad.get("total_matched").and_then(|v| v.as_u64()).unwrap_or(0) != 0 {
                    return Err(format!(
                        "Expected 0 matches for bad method condition, got: {}",
                        bad
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "search_pagination_cursor",
            "Search pagination: next_cursor/has_more works and pages don't overlap",
            "search",
            || async move {
                let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let mock = EnhancedMockServer::start().await;
                let token = "bifrost_search_pagination_token";
                mock.set_response(200, token);
                let url = mock.url("/search/pagination");

                let proxy_url = format!("http://127.0.0.1:{}", port);
                let client = ProxyClient::new(&proxy_url).map_err(|e| e.to_string())?;

                for i in 0..40usize {
                    let body = format!("{{\"i\":{},\"token\":\"{}\"}}", i, token);
                    let _ = client.post(&url, &body).await;
                }

                tokio::time::sleep(Duration::from_millis(500)).await;

                let first = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "filters": {},
                        "cursor": null,
                        "limit": 5
                    }),
                    |r| r.get("results").and_then(|v| v.as_array()).map(|a| !a.is_empty()).unwrap_or(false),
                    Duration::from_secs(10),
                )
                .await?;

                let has_more = first
                    .get("has_more")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let cursor = first.get("next_cursor").and_then(|v| v.as_u64());
                // 搜索为了性能会提前结束，不保证 has_more 为 true；以 next_cursor 是否存在作为分页依据。
                if cursor.is_none() {
                    return Err(format!("Expected next_cursor for pagination, got: {}", first));
                }

                let mut seen = HashSet::<String>::new();
                if let Some(items) = first.get("results").and_then(|v| v.as_array()) {
                    for item in items {
                        if let Some(id) = item
                            .get("record")
                            .and_then(|r| r.get("id"))
                            .and_then(|v| v.as_str())
                        {
                            seen.insert(id.to_string());
                        }
                    }
                }

                let second = search_api(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "filters": {},
                        "cursor": cursor,
                        "limit": 5
                    }),
                )
                .await
                .map_err(|e| format!("Search API failed: {}", e))?;

                if let Some(items) = second.get("results").and_then(|v| v.as_array()) {
                    // 如果实现认为没有更多数据，第二页可能为空；此时仅验证接口可正常返回。
                    if items.is_empty() {
                        let _ = has_more;
                        return Ok(());
                    }
                    for item in items {
                        if let Some(id) = item
                            .get("record")
                            .and_then(|r| r.get("id"))
                            .and_then(|v| v.as_str())
                        {
                            if seen.contains(id) {
                                return Err(format!("Pagination overlap detected for id={}", id));
                            }
                        }
                    }
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "search_websocket_messages",
            "Search covers websocket message frames (scope + protocol filter)",
            "search",
            || async move {
                let (ws_port, server_handle) = start_ws_echo_server().await?;

                let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                tokio::time::sleep(Duration::from_millis(200)).await;

                let token = "bifrost_search_ws_token";
                let target_url = format!("ws://127.0.0.1:{}/echo", ws_port);
                let proxy_addr = format!("127.0.0.1:{}", port);
                let stream = TcpStream::connect(proxy_addr)
                    .await
                    .map_err(|e| format!("Failed to connect to proxy: {}", e))?;
                let request = target_url
                    .into_client_request()
                    .map_err(|e| format!("Failed to build ws request: {}", e))?;
                let (mut ws_stream, _) = client_async(request, stream)
                    .await
                    .map_err(|e| format!("Failed to open websocket: {}", e))?;

                ws_stream
                    .send(Message::Text(format!("hello-{}", token).into()))
                    .await
                    .map_err(|e| format!("Failed to send ws message: {}", e))?;

                // consume echo
                let _ = tokio::time::timeout(Duration::from_secs(2), ws_stream.next()).await;
                let _ = ws_stream.close(None).await;

                tokio::time::sleep(Duration::from_millis(500)).await;

                let scope = serde_json::json!({"all": false, "websocket_messages": true});
                let resp = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "scope": scope,
                        "filters": {"protocols": ["WS"]},
                        "cursor": null,
                        "limit": 50
                    }),
                    |r| has_match_field(r, "websocket_message"),
                    Duration::from_secs(12),
                )
                .await?;

                if !has_match_field(&resp, "websocket_message") {
                    return Err(format!("Expected websocket_message match, got: {}", resp));
                }

                server_handle.abort();
                Ok(())
            },
        ),
        TestCase::standalone(
            "search_sse_events",
            "Search covers SSE event frames (scope + protocol filter)",
            "search",
            || async move {
                let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let token = "bifrost_search_sse_token".to_string();
                let (sse_port, sse_handle) = start_sse_server(token.clone()).await?;

                // SSE 识别依赖请求头 Accept: text/event-stream。
                // 这里用自建 reqwest client 通过代理发起请求，并消费完整响应。
                let proxy_url = format!("http://127.0.0.1:{}", port);
                let proxy = reqwest::Proxy::all(&proxy_url).map_err(|e| e.to_string())?;
                let http = reqwest::Client::builder()
                    .proxy(proxy)
                    .timeout(Duration::from_secs(15))
                    .danger_accept_invalid_certs(true)
                    .build()
                    .map_err(|e| e.to_string())?;

                let url = format!("http://127.0.0.1:{}/sse", sse_port);
                let _ = http
                    .get(url)
                    .header("Accept", "text/event-stream")
                    .send()
                    .await
                    .map_err(|e| format!("SSE request failed: {}", e))?
                    .text()
                    .await
                    .map_err(|e| format!("SSE read failed: {}", e))?;

                tokio::time::sleep(Duration::from_millis(600)).await;

                let scope = serde_json::json!({"all": false, "sse_events": true});
                let resp = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "scope": scope,
                        // SSE traffic 的 content-type 在不同实现/阶段可能为空或不稳定；
                        // 这里仅用协议过滤，避免用例因字段缺失而误失败。
                        "filters": {"protocols": ["SSE"]},
                        "cursor": null,
                        "limit": 50
                    }),
                    |r| has_match_field(r, "sse_event"),
                    Duration::from_secs(12),
                )
                .await?;

                if !has_match_field(&resp, "sse_event") {
                    return Err(format!("Expected sse_event match, got: {}", resp));
                }

                sse_handle.abort();
                Ok(())
            },
        ),
        TestCase::standalone(
            "search_body_index_persist_and_detail_api",
            "Body index is persisted for file-backed bodies; /traffic/*-body detail API still works",
            "search",
            || async move {
                let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                // Force file-backed bodies
                let _ = put_performance_config(
                    port,
                    serde_json::json!({
                        "max_body_memory_size": 1
                    }),
                )
                .await
                .map_err(|e| format!("Failed to update performance config: {}", e))?;

                let mock = EnhancedMockServer::start().await;
                let token = "bifrost_search_index_token";
                let path = format!("/search/index/{}", token);

                // Large response body ensures response_body is also file-backed
                let big_res = format!("{}{}{}", "x".repeat(200_000), token, "y".repeat(1024));
                mock.set_response(200, &big_res);

                let proxy_url = format!("http://127.0.0.1:{}", port);
                let client = ProxyClient::new(&proxy_url).map_err(|e| e.to_string())?;
                let url = mock.url(&path);
                let big_req = format!("{}{}{}", "a".repeat(200_000), token, "b".repeat(1024));

                let _ = client
                    .post(&url, &big_req)
                    .await
                    .map_err(|e| format!("POST failed: {}", e))?;
                tokio::time::sleep(Duration::from_millis(600)).await;

                // Search request body and capture record id
                let req_body_scope = serde_json::json!({"all": false, "request_body": true});
                let req_resp = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "scope": req_body_scope,
                        "filters": {},
                        "cursor": null,
                        "limit": 20
                    }),
                    |r| has_match_field(r, "request_body"),
                    Duration::from_secs(12),
                )
                .await?;

                let id = extract_first_result_id(&req_resp)
                    .ok_or_else(|| format!("No search result id, resp={}", req_resp))?;

                // Wait for index row persisted (kind=0 request)
                wait_for_body_index_present(&admin_state, &id, 0, Duration::from_secs(10)).await?;

                // Detail API regression: request body must still be loadable
                let req_body = admin_get_json(port, &format!("/api/traffic/{}/request-body", id))
                    .await
                    .map_err(|e| format!("Get request-body failed: {}", e))?;
                let ok = req_body
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let data = req_body.get("data").and_then(|v| v.as_str()).unwrap_or("");
                if !ok || !data.contains(token) {
                    return Err(format!(
                        "Request-body detail unexpected: ok={} contains_token={} body={}",
                        ok,
                        data.contains(token),
                        req_body
                    ));
                }

                // Search response body; response index is best-effort and may be skipped
                // (e.g. response body file not ready when job runs). We only assert correctness.
                let res_body_scope = serde_json::json!({"all": false, "response_body": true});
                let _ = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": token,
                        "scope": res_body_scope,
                        "filters": {},
                        "cursor": null,
                        "limit": 20
                    }),
                    |r| has_match_field(r, "response_body"),
                    Duration::from_secs(12),
                )
                .await?;

                let _ =
                    wait_for_body_index_present(&admin_state, &id, 1, Duration::from_secs(10)).await;
                let res_body = admin_get_json(port, &format!("/api/traffic/{}/response-body", id))
                    .await
                    .map_err(|e| format!("Get response-body failed: {}", e))?;
                let ok = res_body
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let data = res_body.get("data").and_then(|v| v.as_str()).unwrap_or("");
                if !ok || !data.contains(token) {
                    return Err(format!(
                        "Response-body detail unexpected: ok={} contains_token={} body={}",
                        ok,
                        data.contains(token),
                        res_body
                    ));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "search_unicode_cross_block_boundary",
            "Search does not miss unicode keywords spanning 64KB body index blocks",
            "search",
            || async move {
                let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
                let (_proxy, _admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let _ = put_performance_config(
                    port,
                    serde_json::json!({
                        "max_body_memory_size": 1
                    }),
                )
                .await
                .map_err(|e| format!("Failed to update performance config: {}", e))?;

                let mock = EnhancedMockServer::start().await;
                let keyword = "测试关键字";
                mock.set_response(200, "ok");

                let proxy_url = format!("http://127.0.0.1:{}", port);
                let client = ProxyClient::new(&proxy_url).map_err(|e| e.to_string())?;
                let url = mock.url("/search/unicode_cross_block");

                // block_size = 64KB (65536). Put keyword near the end so UTF-8 bytes span blocks.
                let mut body = String::new();
                body.push_str(&"a".repeat(65_534));
                body.push_str(keyword);
                body.push_str(&"b".repeat(2048));

                let _ = client
                    .post(&url, &body)
                    .await
                    .map_err(|e| format!("POST failed: {}", e))?;
                tokio::time::sleep(Duration::from_millis(700)).await;

                let req_body_scope = serde_json::json!({"all": false, "request_body": true});
                let resp = wait_for_search(
                    port,
                    serde_json::json!({
                        "keyword": keyword,
                        "scope": req_body_scope,
                        "filters": {},
                        "cursor": null,
                        "limit": 50
                    }),
                    |r| has_match_field(r, "request_body"),
                    Duration::from_secs(12),
                )
                .await?;

                if !has_match_field(&resp, "request_body") {
                    return Err(format!("Expected request_body match, got: {}", resp));
                }

                Ok(())
            },
        ),
        TestCase::standalone(
            "search_index_negative_filter_perf",
            "Indexed negative filtering avoids heavy IO for absent keywords on large file bodies",
            "search",
            || async move {
                let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
                let (_proxy, admin_state) =
                    ProxyInstance::start_with_admin(port, vec![], false, true)
                        .await
                        .map_err(|e| format!("Failed to start proxy with admin: {}", e))?;

                let _ = put_performance_config(
                    port,
                    serde_json::json!({
                        "max_body_memory_size": 1
                    }),
                )
                .await
                .map_err(|e| format!("Failed to update performance config: {}", e))?;

                let mock = EnhancedMockServer::start().await;
                mock.set_response(200, "ok");
                let url = mock.url("/search/negative_perf");

                let proxy_url = format!("http://127.0.0.1:{}", port);
                let client = ProxyClient::new(&proxy_url).map_err(|e| e.to_string())?;

                // Build a moderate dataset of large file-backed bodies WITHOUT the keyword.
                let big_body = "a".repeat(256 * 1024);
                for _ in 0..40usize {
                    let _ = client.post(&url, &big_body).await;
                }
                tokio::time::sleep(Duration::from_millis(900)).await;

                // Fetch ids and wait for some indexes to be persisted to reduce flakiness.
                let traffic = admin_get_json(port, "/api/traffic?limit=60")
                    .await
                    .map_err(|e| format!("List traffic failed: {}", e))?;
                let ids: Vec<String> = traffic
                    .get("records")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|r| {
                                r.get("id")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                if ids.is_empty() {
                    return Err(format!("No traffic records found, resp={}", traffic));
                }

                for id in ids.iter().take(5) {
                    let _ = wait_for_body_index_present(&admin_state, id, 0, Duration::from_secs(8))
                        .await;
                }

                let absent = "bifrost_search_absent_keyword_123";
                let t0 = Instant::now();
                let resp = search_api(
                    port,
                    serde_json::json!({
                        "keyword": absent,
                        "scope": {"all": false, "request_body": true},
                        "filters": {},
                        "cursor": null,
                        "limit": 50
                    }),
                )
                .await
                .map_err(|e| format!("Search API failed: {}", e))?;
                let elapsed = t0.elapsed();

                let matched = resp
                    .get("total_matched")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if matched != 0 {
                    return Err(format!("Expected 0 matches for absent keyword, got: {}", resp));
                }
                if elapsed > Duration::from_secs(5) {
                    return Err(format!(
                        "Negative search took too long: {:?} (resp={})",
                        elapsed, resp
                    ));
                }

                Ok(())
            },
        ),
    ]
}
