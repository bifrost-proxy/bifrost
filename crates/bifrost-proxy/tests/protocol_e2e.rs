use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::timeout;

use bifrost_proxy::protocol::{
    ChunkedWriter, ConnectionKey, ConnectionPool, DetectionResult, HttpRequest, HttpResponse,
    PoolConfig, Priority, ProtocolDetector, SseEvent, SseReader, SseWriter, StreamForwarder,
    WebSocketFrame, WebSocketReader, WebSocketWriter,
};

async fn bind_localhost_listener() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").await.unwrap()
}

fn listener_port(listener: &TcpListener) -> u16 {
    listener.local_addr().unwrap().port()
}

mod websocket_e2e {
    use super::*;
    use bifrost_proxy::protocol::{compute_accept_key, generate_sec_websocket_key, Opcode};

    async fn start_websocket_echo_server(listener: TcpListener, ready_tx: oneshot::Sender<()>) {
        ready_tx.send(()).unwrap();

        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap();

                let sec_key = String::from_utf8_lossy(&buf[..n])
                    .lines()
                    .find(|line| line.to_lowercase().starts_with("sec-websocket-key:"))
                    .and_then(|line| line.split(':').nth(1))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();

                let accept_key = compute_accept_key(&sec_key);
                let response = format!(
                    "HTTP/1.1 101 Switching Protocols\r\n\
                     Upgrade: websocket\r\n\
                     Connection: Upgrade\r\n\
                     Sec-WebSocket-Accept: {}\r\n\r\n",
                    accept_key
                );
                stream.write_all(response.as_bytes()).await.unwrap();

                let (reader, writer) = stream.into_split();
                let mut ws_reader = WebSocketReader::new(reader);
                let mut ws_writer = WebSocketWriter::new(writer, false);

                while let Some(result) = ws_reader.next().await {
                    match result {
                        Ok(frame) => {
                            if frame.opcode == Opcode::Close {
                                let close_frame = WebSocketFrame::close(Some(1000), "");
                                ws_writer.write_frame(close_frame).await.ok();
                                break;
                            }
                            if frame.opcode == Opcode::Ping {
                                let pong = WebSocketFrame::pong(frame.payload);
                                ws_writer.write_frame(pong).await.ok();
                                continue;
                            }
                            let echo_frame = WebSocketFrame {
                                fin: frame.fin,
                                rsv1: frame.rsv1,
                                rsv2: frame.rsv2,
                                rsv3: frame.rsv3,
                                opcode: frame.opcode,
                                mask: None,
                                payload: frame.payload,
                            };
                            ws_writer.write_frame(echo_frame).await.ok();
                        }
                        Err(_) => break,
                    }
                }
            });
        }
    }

    #[tokio::test]
    async fn test_websocket_handshake_and_echo() {
        let listener = bind_localhost_listener().await;
        let port = listener_port(&listener);
        let (ready_tx, ready_rx) = oneshot::channel();

        let server_handle = tokio::spawn(start_websocket_echo_server(listener, ready_tx));
        ready_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();

        let sec_key = generate_sec_websocket_key();
        let request = format!(
            "GET / HTTP/1.1\r\n\
             Host: localhost:{}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: {}\r\n\
             Sec-WebSocket-Version: 13\r\n\r\n",
            port, sec_key
        );
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("101 Switching Protocols"));
        assert!(response.contains("Sec-WebSocket-Accept"));

        let (reader, writer) = stream.into_split();
        let mut ws_reader = WebSocketReader::new(reader);
        let mut ws_writer = WebSocketWriter::new(writer, true);

        let test_message = b"Hello, WebSocket!";
        let frame = WebSocketFrame::text(std::str::from_utf8(test_message).unwrap());
        ws_writer.write_frame(frame).await.unwrap();

        let echo_frame = timeout(Duration::from_secs(2), ws_reader.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(echo_frame.opcode, Opcode::Text);
        assert_eq!(&echo_frame.payload[..], test_message);

        let binary_data = vec![0x00, 0x01, 0x02, 0xFF, 0xFE];
        let binary_frame = WebSocketFrame::binary(Bytes::from(binary_data.clone()));
        ws_writer.write_frame(binary_frame).await.unwrap();

        let echo_binary = timeout(Duration::from_secs(2), ws_reader.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(echo_binary.opcode, Opcode::Binary);
        assert_eq!(echo_binary.payload.to_vec(), binary_data);

        let ping_frame = WebSocketFrame::ping(Bytes::from_static(b"ping"));
        ws_writer.write_frame(ping_frame).await.unwrap();

        let pong_frame = timeout(Duration::from_secs(2), ws_reader.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(pong_frame.opcode, Opcode::Pong);
        assert_eq!(&pong_frame.payload[..], b"ping");

        let close_frame = WebSocketFrame::close(Some(1000), "goodbye");
        ws_writer.write_frame(close_frame).await.unwrap();

        server_handle.abort();
    }

    #[tokio::test]
    async fn test_websocket_large_message_fragmentation() {
        let listener = bind_localhost_listener().await;
        let port = listener_port(&listener);
        let (ready_tx, ready_rx) = oneshot::channel();

        let server_handle = tokio::spawn(start_websocket_echo_server(listener, ready_tx));
        ready_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();

        let sec_key = generate_sec_websocket_key();
        let request = format!(
            "GET / HTTP/1.1\r\nHost: localhost\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: {}\r\n\
             Sec-WebSocket-Version: 13\r\n\r\n",
            sec_key
        );
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut buf = vec![0u8; 1024];
        let _ = stream.read(&mut buf).await.unwrap();

        let (reader, writer) = stream.into_split();
        let mut ws_reader = WebSocketReader::new(reader);
        let mut ws_writer = WebSocketWriter::new(writer, true);

        let large_message: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
        let frame = WebSocketFrame::binary(Bytes::from(large_message.clone()));
        ws_writer.write_frame(frame).await.unwrap();

        let echo_frame = timeout(Duration::from_secs(5), ws_reader.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(echo_frame.payload.len(), large_message.len());
        assert_eq!(echo_frame.payload.to_vec(), large_message);

        server_handle.abort();
    }
}

mod sse_e2e {
    use super::*;

    async fn start_sse_server(listener: TcpListener, ready_tx: oneshot::Sender<()>) {
        ready_tx.send(()).unwrap();

        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf).await.ok();

                let response_headers = "HTTP/1.1 200 OK\r\n\
                    Content-Type: text/event-stream\r\n\
                    Cache-Control: no-cache\r\n\
                    Connection: keep-alive\r\n\r\n";
                stream.write_all(response_headers.as_bytes()).await.unwrap();

                let mut writer = SseWriter::new(stream);

                let event1 = SseEvent {
                    id: Some("1".to_string()),
                    event: Some("message".to_string()),
                    data: "Hello SSE".to_string(),
                    retry: None,
                };
                writer.write_event(&event1).await.unwrap();

                tokio::time::sleep(Duration::from_millis(50)).await;

                let event2 = SseEvent {
                    id: Some("2".to_string()),
                    event: Some("update".to_string()),
                    data: "Data update".to_string(),
                    retry: Some(5000),
                };
                writer.write_event(&event2).await.unwrap();

                writer.write_comment("keepalive").await.unwrap();

                let event3 = SseEvent {
                    id: Some("3".to_string()),
                    event: None,
                    data: "Line 1\nLine 2\nLine 3".to_string(),
                    retry: None,
                };
                writer.write_event(&event3).await.unwrap();

                tokio::time::sleep(Duration::from_secs(10)).await;
            });
        }
    }

    #[tokio::test]
    async fn test_sse_event_stream() {
        let listener = bind_localhost_listener().await;
        let port = listener_port(&listener);
        let (ready_tx, ready_rx) = oneshot::channel();

        let server_handle = tokio::spawn(start_sse_server(listener, ready_tx));
        ready_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();

        let request =
            "GET /events HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n";
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap();

        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("200 OK"));
        assert!(response.contains("text/event-stream"));

        let (reader, _writer) = stream.into_split();
        let mut sse_reader = SseReader::new(reader);

        let mut events = Vec::new();
        for _ in 0..3 {
            if let Ok(Some(Ok(event))) = timeout(Duration::from_secs(2), sse_reader.next()).await {
                events.push(event);
            }
        }

        assert!(!events.is_empty(), "Should receive at least one event");

        if let Some(first_event) = events.first() {
            assert!(first_event.id.is_some() || !first_event.data.is_empty());
        }

        server_handle.abort();
    }

    #[tokio::test]
    async fn test_sse_event_parsing() {
        let raw_events = "id: 1\nevent: message\ndata: Hello\n\n";
        let event1 = SseEvent::parse(raw_events).unwrap();
        assert_eq!(event1.id, Some("1".to_string()));
        assert_eq!(event1.event, Some("message".to_string()));
        assert_eq!(event1.data, "Hello");

        let multiline = "id: 3\ndata: Line1\ndata: Line2\ndata: Line3\n\n";
        let event = SseEvent::parse(multiline).unwrap();
        assert_eq!(event.data, "Line1\nLine2\nLine3");
    }
}

mod http1_e2e {
    use super::*;

    async fn start_http_server(listener: TcpListener, ready_tx: oneshot::Sender<()>) {
        ready_tx.send(()).unwrap();

        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap();

                let request_str = String::from_utf8_lossy(&buf[..n]);
                let first_line = request_str.lines().next().unwrap_or("");
                let parts: Vec<&str> = first_line.split_whitespace().collect();

                let (method, path) = if parts.len() >= 2 {
                    (parts[0], parts[1])
                } else {
                    ("GET", "/")
                };

                let response = match (method, path) {
                    ("GET", "/") => HttpResponse::ok()
                        .with_header("Content-Type", "text/plain")
                        .with_header("X-Custom-Header", "test-value")
                        .with_body("Hello, World!"),
                    ("POST", "/echo") => {
                        let body_start = request_str.find("\r\n\r\n").map(|p| p + 4);
                        let body = body_start.map(|s| &buf[s..n]).unwrap_or(&[]).to_vec();
                        HttpResponse::ok()
                            .with_header("Content-Type", "application/json")
                            .with_body(body)
                    }
                    ("GET", "/chunked") => {
                        let headers = "HTTP/1.1 200 OK\r\n\
                            Transfer-Encoding: chunked\r\n\
                            Content-Type: text/plain\r\n\r\n";
                        stream.write_all(headers.as_bytes()).await.unwrap();

                        let mut chunked_writer = ChunkedWriter::new(&mut stream);
                        chunked_writer.write_chunk(b"Hello, ").await.unwrap();
                        chunked_writer.write_chunk(b"Chunked ").await.unwrap();
                        chunked_writer.write_chunk(b"World!").await.unwrap();
                        chunked_writer.finish().await.unwrap();
                        return;
                    }
                    _ => HttpResponse::not_found().with_body("Not Found"),
                };

                stream.write_all(&response.encode()).await.ok();
            });
        }
    }

    #[tokio::test]
    async fn test_http_get_request() {
        let listener = bind_localhost_listener().await;
        let port = listener_port(&listener);
        let (ready_tx, ready_rx) = oneshot::channel();

        let server_handle = tokio::spawn(start_http_server(listener, ready_tx));
        ready_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();

        let request = HttpRequest::new("GET", "/")
            .with_header("Host", &format!("localhost:{}", port))
            .with_header("User-Agent", "test-client");
        stream.write_all(&request.encode()).await.unwrap();

        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap();

        let (response, _) = HttpResponse::parse(&buf[..n]).unwrap();
        assert_eq!(response.status_code, 200);
        assert!(response.body.is_some());
        let body_str = String::from_utf8_lossy(response.body.as_ref().unwrap());
        assert!(body_str.contains("Hello, World!"));

        let content_type = response
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("Content-Type"))
            .map(|(_, v)| v.as_str());
        assert_eq!(content_type, Some("text/plain"));

        server_handle.abort();
    }

    #[tokio::test]
    async fn test_http_post_with_body() {
        let listener = bind_localhost_listener().await;
        let port = listener_port(&listener);
        let (ready_tx, ready_rx) = oneshot::channel();

        let server_handle = tokio::spawn(start_http_server(listener, ready_tx));
        ready_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();

        let body = r#"{"message": "test data"}"#;
        let request = HttpRequest::new("POST", "/echo")
            .with_header("Host", &format!("localhost:{}", port))
            .with_header("Content-Type", "application/json")
            .with_body(body);
        stream.write_all(&request.encode()).await.unwrap();

        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap();

        let (response, _) = HttpResponse::parse(&buf[..n]).unwrap();
        assert_eq!(response.status_code, 200);
        assert!(response.body.is_some());
        let body_str = String::from_utf8_lossy(response.body.as_ref().unwrap());
        assert!(body_str.contains("test data"));

        server_handle.abort();
    }

    #[tokio::test]
    async fn test_http_chunked_transfer() {
        let listener = bind_localhost_listener().await;
        let port = listener_port(&listener);
        let (ready_tx, ready_rx) = oneshot::channel();

        let server_handle = tokio::spawn(start_http_server(listener, ready_tx));
        ready_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();

        let request =
            HttpRequest::new("GET", "/chunked").with_header("Host", &format!("localhost:{}", port));
        stream.write_all(&request.encode()).await.unwrap();

        let mut all_data = Vec::new();
        let mut buf = vec![0u8; 4096];

        loop {
            match timeout(Duration::from_millis(200), stream.read(&mut buf)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => all_data.extend_from_slice(&buf[..n]),
                Ok(Err(_)) => break,
                Err(_) => break,
            }
        }

        let response_str = String::from_utf8_lossy(&all_data);
        assert!(response_str.contains("200 OK"));
        assert!(response_str.contains("Transfer-Encoding: chunked"));

        assert!(
            response_str.contains("Hello")
                && response_str.contains("Chunked")
                && response_str.contains("World"),
            "Chunked response should contain all chunk data, got: {}",
            response_str
        );

        assert!(
            response_str.contains("0\r\n\r\n"),
            "Chunked response should end with final zero chunk"
        );

        server_handle.abort();
    }
}

mod protocol_detection_e2e {
    use super::*;

    #[tokio::test]
    async fn test_detect_http1_methods() {
        let test_cases = vec![
            (b"GET / HTTP/1.1\r\n".as_slice(), true, "GET"),
            (b"POST /api HTTP/1.1\r\n".as_slice(), true, "POST"),
            (b"PUT /resource HTTP/1.1\r\n".as_slice(), true, "PUT"),
            (b"DELETE /item HTTP/1.1\r\n".as_slice(), true, "DELETE"),
            (
                b"CONNECT example.com:443 HTTP/1.1\r\n".as_slice(),
                true,
                "CONNECT",
            ),
            (b"OPTIONS * HTTP/1.1\r\n".as_slice(), true, "OPTIONS"),
            (b"INVALID request".as_slice(), false, "INVALID"),
        ];

        for (data, should_match, method) in test_cases {
            let result = ProtocolDetector::detect_transport(data);
            match result {
                DetectionResult::Match(_) if should_match => {
                    println!("Correctly detected HTTP/1 {} request", method);
                }
                DetectionResult::NotMatch if !should_match => {
                    println!("Correctly rejected invalid request: {}", method);
                }
                _ if should_match => {
                    panic!("Failed to detect HTTP/1 {} request", method);
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn test_detect_http2_preface() {
        let h2_preface = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
        let result = ProtocolDetector::detect_transport(h2_preface);
        assert!(matches!(result, DetectionResult::Match(Priority::HIGHEST)));

        let partial = b"PRI *";
        let result = ProtocolDetector::detect_transport(partial);
        assert!(matches!(result, DetectionResult::Match(Priority::HIGHEST)));

        let invalid = b"INVALID";
        let result = ProtocolDetector::detect_transport(invalid);
        assert!(matches!(result, DetectionResult::NotMatch));
    }

    #[tokio::test]
    async fn test_detect_tls() {
        let tls_handshake = &[0x16, 0x03, 0x01, 0x00, 0x05];
        let result = ProtocolDetector::detect_transport(tls_handshake);
        assert!(matches!(result, DetectionResult::Match(_)));

        let non_tls = &[0x47, 0x45, 0x54, 0x20];
        let result = ProtocolDetector::detect_transport(non_tls);
        assert!(!matches!(
            result,
            DetectionResult::Match(Priority::HIGH)
                if ProtocolDetector::detect_transport(&[0x16, 0x03, 0x01])
                    == DetectionResult::Match(Priority::HIGH)
        ));
    }

    #[tokio::test]
    async fn test_detect_socks() {
        let socks5 = &[0x05, 0x01, 0x00];
        let result = ProtocolDetector::detect_transport(socks5);
        assert!(matches!(result, DetectionResult::Match(_)));

        let socks4 = &[0x04, 0x01, 0x00, 0x50];
        let result = ProtocolDetector::detect_transport(socks4);
        assert!(matches!(result, DetectionResult::Match(_)));
    }

    #[tokio::test]
    async fn test_detect_websocket_upgrade() {
        let headers = vec![
            ("Connection".to_string(), "Upgrade".to_string()),
            ("Upgrade".to_string(), "websocket".to_string()),
            (
                "Sec-WebSocket-Key".to_string(),
                "dGhlIHNhbXBsZSBub25jZQ==".to_string(),
            ),
        ];
        assert!(ProtocolDetector::is_websocket_upgrade(&headers));

        let non_ws_headers = vec![("Connection".to_string(), "keep-alive".to_string())];
        assert!(!ProtocolDetector::is_websocket_upgrade(&non_ws_headers));
    }

    #[tokio::test]
    async fn test_detect_sse_request() {
        let headers = vec![("Accept".to_string(), "text/event-stream".to_string())];
        assert!(ProtocolDetector::is_sse_request(&headers));

        let non_sse = vec![("Accept".to_string(), "application/json".to_string())];
        assert!(!ProtocolDetector::is_sse_request(&non_sse));
    }

    #[tokio::test]
    async fn test_detect_grpc() {
        let headers = vec![
            ("Content-Type".to_string(), "application/grpc".to_string()),
            ("te".to_string(), "trailers".to_string()),
        ];
        assert!(ProtocolDetector::is_grpc_request(&headers));

        let headers_proto = vec![
            (
                "Content-Type".to_string(),
                "application/grpc+proto".to_string(),
            ),
            ("te".to_string(), "trailers".to_string()),
        ];
        assert!(ProtocolDetector::is_grpc_request(&headers_proto));
    }
}

mod connection_pool_e2e {
    use super::*;

    async fn start_pool_test_server(listener: TcpListener, ready_tx: oneshot::Sender<()>) {
        ready_tx.send(()).unwrap();

        let connection_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };

            let count = connection_count.clone();
            tokio::spawn(async move {
                let conn_id = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

                loop {
                    let mut buf = vec![0u8; 4096];
                    let n = match stream.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };

                    let _ = n;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: text/plain\r\n\
                         X-Connection-Id: {}\r\n\
                         Content-Length: 2\r\n\r\n\
                         OK",
                        conn_id
                    );
                    if stream.write_all(response.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
        }
    }

    #[tokio::test]
    async fn test_connection_pool_reuse() {
        let listener = bind_localhost_listener().await;
        let port = listener_port(&listener);
        let (ready_tx, ready_rx) = oneshot::channel();

        let server_handle = tokio::spawn(start_pool_test_server(listener, ready_tx));
        ready_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let config = PoolConfig {
            max_idle_per_host: 5,
            max_total_connections: 10,
            idle_timeout: Duration::from_secs(30),
            max_age: Duration::from_secs(60),
            connect_timeout: Duration::from_secs(5),
        };
        let pool = ConnectionPool::new(config);
        let key = ConnectionKey::http("127.0.0.1", port);

        {
            let mut conn = pool.get(key.clone()).await.unwrap();
            let request = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
            conn.write_all(request).await.unwrap();

            let mut buf = vec![0u8; 1024];
            let _ = conn.read(&mut buf).await.unwrap();
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
        let stats = pool.stats();
        assert!(
            stats.connections_created >= 1,
            "Should have created at least 1 connection"
        );

        {
            let mut conn = pool.get(key.clone()).await.unwrap();
            let request = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
            conn.write_all(request).await.unwrap();

            let mut buf = vec![0u8; 1024];
            let _ = conn.read(&mut buf).await.unwrap();
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
        let stats = pool.stats();
        println!(
            "Connections created: {}, reused: {}",
            stats.connections_created, stats.connections_reused
        );

        server_handle.abort();
    }

    #[tokio::test]
    async fn test_connection_pool_concurrent() {
        let listener = bind_localhost_listener().await;
        let port = listener_port(&listener);
        let (ready_tx, ready_rx) = oneshot::channel();

        let server_handle = tokio::spawn(start_pool_test_server(listener, ready_tx));
        ready_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let pool = Arc::new(ConnectionPool::new(PoolConfig::default()));
        let key = ConnectionKey::http("127.0.0.1", port);

        let mut handles = Vec::new();
        for i in 0..10 {
            let pool = pool.clone();
            let key = key.clone();
            handles.push(tokio::spawn(async move {
                let mut conn = pool.get(key).await.unwrap();
                let request = format!("GET /{} HTTP/1.1\r\nHost: localhost\r\n\r\n", i);
                conn.write_all(request.as_bytes()).await.unwrap();

                let mut buf = vec![0u8; 1024];
                let _ = conn.read(&mut buf).await.unwrap();
                i
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let stats = pool.stats();
        assert!(
            stats.connections_created <= 10,
            "Should not create more than 10 connections"
        );
        println!(
            "Concurrent test - Created: {}, Reused: {}, Errors: {}",
            stats.connections_created, stats.connections_reused, stats.connection_errors
        );

        server_handle.abort();
    }

    #[tokio::test]
    async fn test_pool_stats() {
        let config = PoolConfig::high_performance();
        assert!(config.max_total_connections > PoolConfig::default().max_total_connections);

        let config = PoolConfig::low_memory();
        assert!(config.max_total_connections < PoolConfig::default().max_total_connections);
    }
}

mod stream_forwarding_e2e {
    use super::*;

    #[tokio::test]
    async fn test_bidirectional_forwarding() {
        let server_listener = bind_localhost_listener().await;
        let server_port = listener_port(&server_listener);
        let (ready_tx, ready_rx) = oneshot::channel();

        let server_handle = tokio::spawn(async move {
            let listener = server_listener;
            ready_tx.send(()).unwrap();

            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 1024];
                if let Ok(n) = stream.read(&mut buf).await {
                    let mut response = buf[..n].to_vec();
                    response.reverse();
                    stream.write_all(&response).await.ok();
                }
            }
        });

        ready_rx.await.unwrap();

        let proxy_listener = bind_localhost_listener().await;
        let proxy_port = listener_port(&proxy_listener);
        let (proxy_ready_tx, proxy_ready_rx) = oneshot::channel();

        let proxy_handle = tokio::spawn(async move {
            let listener = proxy_listener;
            proxy_ready_tx.send(()).unwrap();

            if let Ok((client_stream, _)) = listener.accept().await {
                let server_stream = TcpStream::connect(format!("127.0.0.1:{}", server_port))
                    .await
                    .unwrap();

                StreamForwarder::bidirectional(client_stream, server_stream, 8192)
                    .await
                    .ok();
            }
        });

        proxy_ready_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
            .await
            .unwrap();

        let message = b"Hello, Proxy!";
        client.write_all(message).await.unwrap();
        client.shutdown().await.ok();

        let mut response = vec![0u8; 1024];
        let n = timeout(Duration::from_secs(2), client.read(&mut response))
            .await
            .unwrap()
            .unwrap();

        let expected: Vec<u8> = message.iter().rev().copied().collect();
        assert_eq!(&response[..n], &expected[..]);

        server_handle.abort();
        proxy_handle.abort();
    }
}
