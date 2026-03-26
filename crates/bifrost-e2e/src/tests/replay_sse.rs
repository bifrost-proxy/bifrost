use crate::proxy::ProxyInstance;
use crate::runner::TestCase;
use bifrost_admin::{AdminState, QueryParams};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn admin_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .build()
        .map_err(|e| format!("Failed to create admin HTTP client: {}", e))
}

pub fn get_all_tests() -> Vec<TestCase> {
    vec![
        TestCase::standalone(
            "replay_sse_live_stream_keeps_tail_events",
            "Replay SSE 活跃详情流在连接关闭前后不应丢失尾部事件",
            "replay_sse",
            test_replay_sse_live_stream_keeps_tail_events,
        ),
        TestCase::standalone(
            "replay_sse_live_stream_keeps_done_event",
            "Replay SSE 活跃详情流应推送 OpenAI 风格的 [DONE] 尾事件",
            "replay_sse",
            test_replay_sse_live_stream_keeps_done_event,
        ),
    ]
}

async fn start_sse_mock_server(
    total_events: usize,
    tail_burst_events: usize,
    pre_tail_idle: Duration,
    hold_open_after_tail: Duration,
) -> Result<(u16, Arc<AtomicUsize>, tokio::task::JoinHandle<()>), String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind SSE mock server: {}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to get SSE mock server addr: {}", e))?
        .port();
    let request_count = Arc::new(AtomicUsize::new(0));
    let request_count_for_task = request_count.clone();

    let handle = tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        request_count_for_task.fetch_add(1, Ordering::SeqCst);

        let mut req_buf = [0u8; 4096];
        let _ = stream.read(&mut req_buf).await;

        let _ = stream
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n",
            )
            .await;

        let prefix_events = total_events.saturating_sub(tail_burst_events);
        for i in 1..=prefix_events {
            let event = format!("id: {i}\ndata: msg-{i}\n\n");
            let _ = stream.write_all(event.as_bytes()).await;
            let _ = stream.flush().await;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        tokio::time::sleep(pre_tail_idle).await;

        for i in (prefix_events + 1)..=total_events {
            let event = format!("id: {i}\ndata: msg-{i}\n\n");
            let _ = stream.write_all(event.as_bytes()).await;
        }
        let _ = stream.flush().await;
        tokio::time::sleep(hold_open_after_tail).await;
        let _ = stream.shutdown().await;
    });

    Ok((port, request_count, handle))
}

async fn start_openai_style_sse_mock_server(
    total_json_events: usize,
    pre_tail_idle: Duration,
    hold_open_after_done: Duration,
) -> Result<(u16, Arc<AtomicUsize>, tokio::task::JoinHandle<()>), String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind OpenAI SSE mock server: {}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to get OpenAI SSE mock server addr: {}", e))?
        .port();
    let request_count = Arc::new(AtomicUsize::new(0));
    let request_count_for_task = request_count.clone();

    let handle = tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        request_count_for_task.fetch_add(1, Ordering::SeqCst);

        let mut req_buf = [0u8; 4096];
        let _ = stream.read(&mut req_buf).await;

        let _ = stream
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n",
            )
            .await;

        for i in 1..=total_json_events {
            let payload = serde_json::json!({
                "id": "chatcmpl-test",
                "object": "chat.completion.chunk",
                "choices": [{
                    "index": 0,
                    "delta": { "content": format!("token-{i}") },
                    "finish_reason": serde_json::Value::Null,
                }]
            });
            let event = format!("data: {}\n\n", payload);
            let _ = stream.write_all(event.as_bytes()).await;
            let _ = stream.flush().await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        tokio::time::sleep(pre_tail_idle).await;

        let _ = stream.write_all(b"data: [DONE]\n\n").await;
        let _ = stream.flush().await;
        tokio::time::sleep(hold_open_after_done).await;
        let _ = stream.shutdown().await;
    });

    Ok((port, request_count, handle))
}

async fn start_proxy_sse_request(
    proxy_port: u16,
    upstream_url: &str,
    counter: fn(&str) -> usize,
) -> Result<tokio::task::JoinHandle<Result<usize, String>>, String> {
    let upstream_url = upstream_url.to_string();

    Ok(tokio::spawn(async move {
        let mut stream = TcpStream::connect(("127.0.0.1", proxy_port))
            .await
            .map_err(|e| format!("Failed to connect to proxy: {}", e))?;

        let request = format!(
            "GET {upstream_url} HTTP/1.1\r\nHost: test.local\r\nAccept: text/event-stream\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| format!("Failed to write proxy SSE request: {}", e))?;

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .map_err(|e| format!("Failed to read proxied SSE response: {}", e))?;

        let response_text = String::from_utf8_lossy(&response);
        let (_, body) = response_text
            .split_once("\r\n\r\n")
            .ok_or_else(|| format!("Invalid HTTP response from proxy: {}", response_text))?;

        Ok(counter(body))
    }))
}

async fn collect_detail_sse_body(admin_base: &str, traffic_id: &str) -> Result<String, String> {
    let client = admin_http_client()?;
    let response = client
        .get(format!(
            "{}/traffic/{}/sse/stream?from=begin&batch=1",
            admin_base, traffic_id
        ))
        .send()
        .await
        .map_err(|e| format!("Detail SSE stream request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Detail SSE stream status: {}", response.status()));
    }

    response
        .text()
        .await
        .map_err(|e| format!("Failed to read detail SSE stream body: {}", e))
}

fn count_sse_frames(text: &str) -> usize {
    let normalized = text.replace("\r\n", "\n");
    normalized
        .split("\n\n")
        .filter(|chunk| chunk.lines().any(|line| line.starts_with("data: ")))
        .count()
}

fn count_raw_sse_events(text: &str) -> usize {
    text.lines()
        .filter(|line| line.trim_start().starts_with("data: msg-"))
        .count()
}

fn count_raw_data_events(text: &str) -> usize {
    text.lines()
        .filter(|line| line.trim_start().starts_with("data: "))
        .count()
}

fn count_live_detail_events_by_data_prefix(text: &str, prefix: &str) -> usize {
    let normalized = text.replace("\r\n", "\n");
    normalized
        .split("\n\n")
        .filter_map(|chunk| {
            chunk
                .lines()
                .find_map(|line| line.trim_start().strip_prefix("data: "))
        })
        .filter_map(|json| serde_json::from_str::<Value>(json).ok())
        .filter(|event| {
            event["data"]
                .as_str()
                .map(|data| data.starts_with(prefix))
                .unwrap_or(false)
        })
        .count()
}

fn live_detail_contains_finish_event(text: &str) -> bool {
    let normalized = text.replace("\r\n", "\n");
    normalized
        .split("\n\n")
        .filter_map(|chunk| {
            chunk
                .lines()
                .find_map(|line| line.trim_start().strip_prefix("data: "))
        })
        .filter_map(|json| serde_json::from_str::<Value>(json).ok())
        .any(|event| event["event"].as_str() == Some("finish"))
}

async fn wait_for_sse_record_id(admin_state: &Arc<AdminState>) -> Result<String, String> {
    let Some(db_store) = admin_state.traffic_db_store.clone() else {
        return Err("Traffic DB not configured".to_string());
    };

    for _ in 0..50 {
        let record = tokio::task::spawn_blocking({
            let db_store = db_store.clone();
            move || {
                let result = db_store.query(&QueryParams {
                    limit: Some(20),
                    ..Default::default()
                });
                result.records.into_iter().find(|record| record.is_sse())
            }
        })
        .await
        .map_err(|e| format!("spawn_blocking failed: {}", e))?;
        if let Some(record) = record {
            return Ok(record.id);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err("No active SSE traffic record found".to_string())
}

async fn fetch_recent_traffic_summaries(admin_base: &str) -> Result<Value, String> {
    admin_http_client()?
        .get(format!("{}/traffic?limit=20", admin_base))
        .send()
        .await
        .map_err(|e| format!("traffic list request failed: {}", e))?
        .json::<Value>()
        .await
        .map_err(|e| format!("traffic list decode failed: {}", e))
}

async fn test_replay_sse_live_stream_keeps_tail_events() -> Result<(), String> {
    const TOTAL_EVENTS: usize = 90;
    const TAIL_BURST_EVENTS: usize = 30;
    const HOLD_OPEN_AFTER_TAIL_MS: u64 = 2_000;

    let pre_tail_idle_ms = std::env::var("BIFROST_E2E_SSE_PRE_TAIL_IDLE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);

    let (upstream_port, request_count, upstream_handle) = start_sse_mock_server(
        TOTAL_EVENTS,
        TAIL_BURST_EVENTS,
        Duration::from_millis(pre_tail_idle_ms),
        Duration::from_millis(HOLD_OPEN_AFTER_TAIL_MS),
    )
    .await?;

    let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
    let (_proxy, admin_state) = ProxyInstance::start_with_admin(
        port,
        vec![&format!("test.local host://127.0.0.1:{}", upstream_port)],
        false,
        false,
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let admin_base = format!("http://127.0.0.1:{}/_bifrost/api", port);
    let upstream_url = "http://test.local/events".to_string();

    let proxied_request_handle =
        start_proxy_sse_request(port, &upstream_url, count_raw_sse_events).await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    if proxied_request_handle.is_finished() {
        let result = proxied_request_handle
            .await
            .map_err(|e| format!("Proxy SSE task join failed early: {}", e))??;
        return Err(format!(
            "Proxy SSE request finished before SSE traffic record appeared, event_count={}",
            result
        ));
    }
    let traffic_id = match wait_for_sse_record_id(&admin_state).await {
        Ok(id) => id,
        Err(err) => {
            let traffic_dump = fetch_recent_traffic_summaries(&admin_base).await?;
            return Err(format!(
                "{}; upstream_requests={}; traffic_dump={}",
                err,
                request_count.load(Ordering::SeqCst),
                traffic_dump
            ));
        }
    };
    let live_detail_body = collect_detail_sse_body(&admin_base, &traffic_id).await?;
    let live_detail_count = count_live_detail_events_by_data_prefix(&live_detail_body, "msg-");
    let replay_count = proxied_request_handle
        .await
        .map_err(|e| format!("Proxy SSE task join failed: {}", e))??;

    upstream_handle.abort();

    let response_body = admin_http_client()?
        .get(format!(
            "{}/traffic/{}/response-body",
            admin_base, traffic_id
        ))
        .send()
        .await
        .map_err(|e| format!("response-body request failed: {}", e))?
        .json::<Value>()
        .await
        .map_err(|e| format!("response-body json decode failed: {}", e))?;

    let raw_body = response_body["data"]
        .as_str()
        .ok_or("response-body data missing")?;
    let persisted_count = count_raw_sse_events(raw_body);

    if replay_count != TOTAL_EVENTS {
        return Err(format!(
            "Replay live stream count mismatch: expected {}, got {}",
            TOTAL_EVENTS, replay_count
        ));
    }

    if persisted_count != TOTAL_EVENTS {
        return Err(format!(
            "Persisted response-body count mismatch: expected {}, got {}",
            TOTAL_EVENTS, persisted_count
        ));
    }

    if live_detail_count != persisted_count {
        return Err(format!(
            "Detail live SSE stream lost tail events: live={}, persisted={}, traffic_id={}",
            live_detail_count, persisted_count, traffic_id
        ));
    }

    if !live_detail_contains_finish_event(&live_detail_body) {
        return Err(format!(
            "Detail live SSE stream missing synthetic finish event: traffic_id={}, body_tail={}",
            traffic_id,
            &live_detail_body[live_detail_body.len().saturating_sub(400)..]
        ));
    }

    Ok(())
}

async fn test_replay_sse_live_stream_keeps_done_event() -> Result<(), String> {
    const TOTAL_JSON_EVENTS: usize = 12;
    const TOTAL_EVENTS_WITH_DONE: usize = TOTAL_JSON_EVENTS + 1;
    const HOLD_OPEN_AFTER_DONE_MS: u64 = 2_000;

    let pre_tail_idle_ms = std::env::var("BIFROST_E2E_SSE_PRE_TAIL_IDLE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);

    let (upstream_port, request_count, upstream_handle) = start_openai_style_sse_mock_server(
        TOTAL_JSON_EVENTS,
        Duration::from_millis(pre_tail_idle_ms),
        Duration::from_millis(HOLD_OPEN_AFTER_DONE_MS),
    )
    .await?;

    let port = portpicker::pick_unused_port().ok_or("Failed to pick unused port")?;
    let (_proxy, admin_state) = ProxyInstance::start_with_admin(
        port,
        vec![&format!("test.local host://127.0.0.1:{}", upstream_port)],
        false,
        false,
    )
    .await
    .map_err(|e| format!("Failed to start proxy: {}", e))?;

    let admin_base = format!("http://127.0.0.1:{}/_bifrost/api", port);
    let upstream_url = "http://test.local/openai-events".to_string();

    let proxied_request_handle =
        start_proxy_sse_request(port, &upstream_url, count_raw_data_events).await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    if proxied_request_handle.is_finished() {
        let result = proxied_request_handle
            .await
            .map_err(|e| format!("Proxy SSE task join failed early: {}", e))??;
        return Err(format!(
            "Proxy OpenAI-style SSE request finished before SSE traffic record appeared, event_count={}",
            result
        ));
    }

    let traffic_id = match wait_for_sse_record_id(&admin_state).await {
        Ok(id) => id,
        Err(err) => {
            let traffic_dump = fetch_recent_traffic_summaries(&admin_base).await?;
            return Err(format!(
                "{}; upstream_requests={}; traffic_dump={}",
                err,
                request_count.load(Ordering::SeqCst),
                traffic_dump
            ));
        }
    };

    let live_detail_body = collect_detail_sse_body(&admin_base, &traffic_id).await?;
    let live_detail_count = count_sse_frames(&live_detail_body);
    let replay_count = proxied_request_handle
        .await
        .map_err(|e| format!("Proxy OpenAI-style SSE task join failed: {}", e))??;

    upstream_handle.abort();

    let response_body = admin_http_client()?
        .get(format!(
            "{}/traffic/{}/response-body",
            admin_base, traffic_id
        ))
        .send()
        .await
        .map_err(|e| format!("response-body request failed: {}", e))?
        .json::<Value>()
        .await
        .map_err(|e| format!("response-body json decode failed: {}", e))?;

    let raw_body = response_body["data"]
        .as_str()
        .ok_or("response-body data missing")?;
    let persisted_count = count_raw_data_events(raw_body);

    if replay_count != TOTAL_EVENTS_WITH_DONE {
        return Err(format!(
            "Replay OpenAI-style SSE count mismatch: expected {}, got {}",
            TOTAL_EVENTS_WITH_DONE, replay_count
        ));
    }

    if persisted_count != TOTAL_EVENTS_WITH_DONE {
        return Err(format!(
            "Persisted OpenAI-style response-body count mismatch: expected {}, got {}",
            TOTAL_EVENTS_WITH_DONE, persisted_count
        ));
    }

    if !raw_body.contains("data: [DONE]") {
        return Err("Persisted OpenAI-style response-body missing [DONE]".to_string());
    }

    if live_detail_count != persisted_count {
        return Err(format!(
            "Detail live OpenAI-style SSE stream lost tail events: live={}, persisted={}, traffic_id={}",
            live_detail_count, persisted_count, traffic_id
        ));
    }

    if !live_detail_body.contains("\"data\":\"[DONE]\"") {
        return Err(format!(
            "Detail live OpenAI-style SSE stream missing [DONE]: traffic_id={}, body_tail={}",
            traffic_id,
            &live_detail_body[live_detail_body.len().saturating_sub(400)..]
        ));
    }

    Ok(())
}
