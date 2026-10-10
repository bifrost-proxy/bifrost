use std::{collections::HashMap, sync::Arc, time::Duration};

use bifrost_admin::{
    breakpoint::{BreakpointEdit, BreakpointSettings},
    AdminState,
};
use bifrost_core::Protocol;
use bifrost_proxy::{ProxyConfig, ProxyServer, ResolvedRules, RuleValue, RulesResolver};
use bifrost_tls::{generate_root_ca, init_crypto_provider, DynamicCertGenerator, TlsConfig};
use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::{BodyExt, StreamBody};
use hyper::{body::Frame, service::service_fn, HeaderMap, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

struct ResponseBreakpoint(&'static str, bool);
impl RulesResolver for ResponseBreakpoint {
    fn resolve_with_context(
        &self,
        _url: &str,
        _method: &str,
        _headers: &HashMap<String, String>,
        _cookies: &HashMap<String, String>,
    ) -> ResolvedRules {
        ResolvedRules {
            rules: vec![RuleValue {
                pattern: "127.0.0.1".into(),
                protocol: Protocol::Breakpoint,
                value: self.0.into(),
                options: HashMap::new(),
                rule_name: None,
                raw: None,
                line: None,
                auto_tls_intercept: false,
            }],
            upstream_unsafe_ssl: true,
            res_append: self.1.then(|| Bytes::from_static(b"append")),
            ..Default::default()
        }
    }
}

#[tokio::test]
async fn unchanged_breakpoint_preserves_actual_h2_trailers_on_http1_wire() {
    run_trailers_regression(false, false, false, false).await;
}

#[tokio::test]
async fn unchanged_breakpoint_preserves_actual_request_and_response_trailers() {
    run_trailers_regression(true, false, true, false).await;
}

#[tokio::test]
async fn stalled_h2_probe_does_not_repeat_capture_for_body_rules() {
    run_trailers_regression(false, true, false, false).await;
}

#[tokio::test]
async fn intercepted_h2_probe_preserves_actual_response_trailers() {
    run_trailers_regression(false, false, true, false).await;
}

#[tokio::test]
async fn intercepted_stalled_h2_probe_does_not_repeat_capture() {
    run_trailers_regression(false, true, true, false).await;
}

#[tokio::test]
async fn small_breakpoint_budget_preserves_body_rules_over_plain_h2() {
    run_trailers_regression(false, false, false, true).await;
}

#[tokio::test]
async fn small_breakpoint_budget_preserves_body_rules_over_intercepted_h2() {
    run_trailers_regression(false, false, true, true).await;
}

async fn run_trailers_regression(
    request_trailers: bool,
    stalled: bool,
    intercepted: bool,
    small_budget: bool,
) {
    tokio::time::timeout(Duration::from_secs(20), async {
        init_crypto_provider();
        let ca = Arc::new(generate_root_ca().unwrap());
        let certificate = DynamicCertGenerator::new(ca.clone()).generate_for_domain("127.0.0.1").unwrap();
        let mut tls = (*TlsConfig::build_server_config(&certificate).unwrap()).clone();
        tls.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_port = origin.local_addr().unwrap().port();
        let origin_hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_hits = origin_hits.clone();
        let (response_ready_tx, response_ready_rx) = tokio::sync::oneshot::channel();
        let origin_response_ready = Arc::new(std::sync::Mutex::new(Some(response_ready_tx)));
        let origin_task = tokio::spawn(async move {
            let (socket, _) = origin.accept().await.unwrap();
            let tls = acceptor.accept(socket).await.unwrap();
            let service = service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                let observed_hits = observed_hits.clone();
                let response_ready = origin_response_ready.clone();
                async move {
                observed_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert_eq!(request.version(), hyper::Version::HTTP_2);
                if request_trailers {
                    let collected = request.into_body().collect().await.unwrap();
                    let fields = collected.trailers().expect("actual upstream request trailers");
                    assert_eq!(fields.get_all("x-request-metadata").iter().map(|v| v.to_str().unwrap()).collect::<Vec<_>>(), vec!["first", "second"]);
                    assert_eq!(collected.to_bytes(), "req");
                }
                let mut trailers = HeaderMap::new();
                trailers.append("grpc-status", "0".parse().unwrap());
                trailers.append("x-metadata", "first".parse().unwrap());
                trailers.append("x-metadata", "second".parse().unwrap());
                let frames = if stalled {
                    futures_util::stream::iter(vec![Ok::<_, std::convert::Infallible>(Frame::data(Bytes::from_static(b"o")))])
                        .chain(futures_util::stream::pending()).boxed()
                } else {
                    futures_util::stream::iter(vec![Ok::<_, std::convert::Infallible>(Frame::data(Bytes::from_static(b"ok"))), Ok(Frame::trailers(trailers))]).boxed()
                };
                if let Some(sender) = response_ready.lock().unwrap().take() {
                    let _ = sender.send(tokio::time::Instant::now());
                }
                Ok::<_, std::convert::Infallible>(Response::builder()
                    .header("content-type", "application/grpc")
                    .header("content-length", "2")
                    .header("trailer", "grpc-status, x-metadata")
                    .body(StreamBody::new(frames)).unwrap())
            }});
            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new()).serve_connection(TokioIo::new(tls), service).await;
        });
        let reserve = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = reserve.local_addr().unwrap().port();
        drop(reserve);
        let state = Arc::new(AdminState::new(proxy_port));
        state.breakpoint_manager.update_settings(BreakpointSettings { enabled:true,max_body_bytes:if small_budget { 1 } else { 1024 } });
        let proxy_tls = Arc::new(bifrost_proxy::TlsConfig {
            ca_cert: Some(ca.certificate_pem().as_bytes().to_vec()),
            ca_key: Some(ca.key_pair.serialize_pem().into_bytes()),
            cert_generator: Some(Arc::new(DynamicCertGenerator::new(ca.clone()))),
            sni_resolver: None,
        });
        let proxy = ProxyServer::new(ProxyConfig {host:"127.0.0.1".into(),port:proxy_port,unsafe_ssl:true,enable_socks:false,..Default::default()})
            .with_tls_config(proxy_tls).with_admin_state_shared(state.clone()).with_rules(Arc::new(ResponseBreakpoint(if request_trailers { "both" } else { "response" }, stalled || small_budget)));
        let proxy_task = tokio::spawn(async move { proxy.run().await.unwrap(); });
        for _ in 0..100 {
            if TcpStream::connect(("127.0.0.1", proxy_port)).await.is_ok() { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let received = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let client_received = received.clone();
        let client = tokio::spawn(async move {
            let mut socket = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
            if intercepted {
                socket.write_all(format!("CONNECT 127.0.0.1:{origin_port} HTTP/1.1\r\nHost: 127.0.0.1:{origin_port}\r\n\r\n").as_bytes()).await.unwrap();
                let mut connect_response = Vec::new();
                let mut byte = [0u8; 1];
                while !connect_response.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).await.unwrap();
                    connect_response.push(byte[0]);
                }
                assert!(String::from_utf8(connect_response).unwrap().contains("200"));
                let mut roots = rustls::RootCertStore::empty();
                roots.add(ca.certificate_der().unwrap()).unwrap();
                let mut config = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
                config.alpn_protocols = vec![b"h2".to_vec()];
                let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect("127.0.0.1".try_into().unwrap(), socket).await.unwrap();
                assert_eq!(tls.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
                let (mut sender, connection) = hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls)).await.unwrap();
                let driver = tokio::spawn(async move { let _ = connection.await; });
                let mut trailers = HeaderMap::new();
                trailers.append("x-request-metadata", "first".parse().unwrap());
                trailers.append("x-request-metadata", "second".parse().unwrap());
                let mut frames = Vec::<Result<Frame<Bytes>, std::convert::Infallible>>::new();
                if request_trailers { frames.push(Ok(Frame::data(Bytes::from_static(b"req")))); }
                if request_trailers { frames.push(Ok(Frame::trailers(trailers))); }
                let request = hyper::Request::builder().method(if request_trailers { "POST" } else { "GET" })
                    .uri(format!("https://127.0.0.1:{origin_port}/trailers"))
                    .header("host", format!("127.0.0.1:{origin_port}"))
                    .header("content-length", if request_trailers { "3" } else { "0" })
                    .body(StreamBody::new(futures_util::stream::iter(frames))).unwrap();
                let response = sender.send_request(request).await.unwrap();
                client_received.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert_eq!(response.status(), if stalled { 204 } else { 200 });
                let collected = response.into_body().collect().await.unwrap();
                if stalled {
                    assert!(collected.trailers().is_none());
                    assert!(collected.to_bytes().is_empty());
                } else {
                let trailers = collected.trailers().expect("actual HTTP/2 response trailers");
                assert_eq!(trailers["grpc-status"], "0");
                assert_eq!(trailers.get_all("x-metadata").iter().map(|v| v.to_str().unwrap()).collect::<Vec<_>>(), vec!["first", "second"]);
                assert_eq!(collected.to_bytes(), if small_budget { "okappend" } else { "ok" });
                }
                driver.abort();
                None
            } else {
                socket.write_all(format!("GET https://127.0.0.1:{origin_port}/trailers HTTP/1.1\r\nHost: 127.0.0.1:{origin_port}\r\nTE: trailers\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
                let mut response = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let len = socket.read(&mut chunk).await.unwrap();
                    if len == 0 { break; }
                    client_received.fetch_add(len, std::sync::atomic::Ordering::SeqCst);
                    response.extend_from_slice(&chunk[..len]);
                }
                Some(String::from_utf8(response).unwrap().to_ascii_lowercase())
            }
        });
        if request_trailers {
            let pause = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(pause) = state.breakpoint_manager.pending().into_iter().next() { break pause; }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.unwrap();
            assert_eq!(pause.phase, "request");
            assert_eq!(pause.body.as_deref(), Some("req"));
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(origin_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
            assert_eq!(received.load(std::sync::atomic::Ordering::SeqCst), 0);
            assert!(state.breakpoint_manager.resume(&pause.request_id, "request", BreakpointEdit::default()).is_ok());
        }
        // Measure capture latency after the real upstream prepares its response,
        // excluding client connection and TLS setup while retaining the 3.5s bound.
        let pause_deadline = if stalled {
            let ready_at = tokio::time::timeout(Duration::from_secs(5), response_ready_rx).await.unwrap().unwrap();
            ready_at + Duration::from_millis(3500)
        } else {
            tokio::time::Instant::now() + Duration::from_secs(5)
        };
        let pause = tokio::time::timeout_at(pause_deadline, async {
            loop {
                if let Some(pause) = state.breakpoint_manager.pending().into_iter().next() { break pause; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        if stalled {
            assert!(tokio::time::Instant::now() <= pause_deadline, "pause must be observed within 3.5s of actual upstream response readiness");
            assert!(pause.body_omitted, "a stalled probe must remain header-only");
        } else if small_budget {
            assert!(pause.body_omitted, "body editing must still obey the one-byte breakpoint budget");
            assert!(pause.body.is_none());
        } else {
            assert_eq!(pause.body.as_deref(), Some("ok"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!client.is_finished(), "response must stay blocked until resume");
        assert_eq!(received.load(std::sync::atomic::Ordering::SeqCst), 0, "no headers or body may escape before resume");
        assert!(state.breakpoint_manager.resume(&pause.request_id, "response", BreakpointEdit { status: stalled.then_some(204), ..Default::default() }).is_ok());
        let response = client.await.unwrap();
        proxy_task.abort();
        origin_task.abort();
        if let Some(response) = response {
        if stalled {
            assert!(response.contains("204 no content"), "{response}");
            assert!(response.split_once("\r\n\r\n").unwrap().1.is_empty(), "{response}");
            return;
        }
        assert!(response.contains("200 ok"), "{response}");
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(headers.contains("transfer-encoding: chunked"), "{response}");
        assert!(!headers.contains("content-length:"), "{response}");
        assert!(body.contains(if small_budget { "okappend" } else { "ok" }), "{response}");
        assert!(body.contains("grpc-status: 0"), "{response}");
        assert!(body.contains("x-metadata: first"), "{response}");
        assert!(body.contains("x-metadata: second"), "{response}");
        }
    }).await.expect("bounded real proxy HTTP/2 trailers regression timed out");
}
