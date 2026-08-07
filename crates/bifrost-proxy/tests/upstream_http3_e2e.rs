#![cfg(feature = "http3")]

use std::sync::Arc;
use std::time::Duration;

use bifrost_proxy::{
    Http3Client, ProxyConfig, ProxyServer, ResolvedRules, RulesResolver as ProxyRulesResolver,
};
use bifrost_tls::{generate_root_ca, init_crypto_provider, DynamicCertGenerator, TlsConfig};
use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

// Both tests reserve ephemeral TCP/UDP ports before their servers bind them.
// Keep the scenarios serial so Windows cannot hand the released proxy port to
// the sibling test during that gap.
static UPSTREAM_HTTP3_E2E_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Default)]
struct StaticRulesResolver {
    enable_upstream_http3: bool,
}

impl ProxyRulesResolver for StaticRulesResolver {
    fn resolve_with_context(
        &self,
        _url: &str,
        _method: &str,
        _req_headers: &std::collections::HashMap<String, String>,
        _req_cookies: &std::collections::HashMap<String, String>,
    ) -> ResolvedRules {
        ResolvedRules {
            upstream_http3: self.enable_upstream_http3,
            ..ResolvedRules::default()
        }
    }
}

async fn pick_tcp_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn pick_udp_port() -> u16 {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    port
}

async fn start_h3_origin(port: u16, expected_connections: usize, ready_tx: oneshot::Sender<()>) {
    init_crypto_provider();

    let ca = Arc::new(generate_root_ca().expect("failed to generate test CA"));
    let cert = DynamicCertGenerator::new(ca)
        .generate_for_domain("127.0.0.1")
        .expect("failed to generate test server certificate");

    let mut crypto =
        (*TlsConfig::build_server_config(&cert).expect("failed to build TLS config")).clone();
    crypto.alpn_protocols = vec![b"h3".to_vec()];
    crypto.max_early_data_size = u32::MAX;

    let quic_config = quinn::crypto::rustls::QuicServerConfig::try_from(crypto)
        .expect("failed to build QUIC server config");
    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_config));
    let transport = Arc::get_mut(&mut server_config.transport).unwrap();
    transport.keep_alive_interval(Some(Duration::from_secs(5)));
    transport.max_idle_timeout(Some(Duration::from_secs(30).try_into().unwrap()));

    let endpoint =
        quinn::Endpoint::server(server_config, format!("127.0.0.1:{port}").parse().unwrap())
            .expect("failed to bind H3 origin");

    ready_tx.send(()).unwrap();

    for _ in 0..expected_connections {
        let incoming = tokio::time::timeout(Duration::from_secs(10), endpoint.accept())
            .await
            .expect("timed out waiting for QUIC connection")
            .expect("expected one QUIC connection");
        let connection = incoming.await.expect("failed to accept QUIC connection");
        let mut h3_conn = h3::server::Connection::new(h3_quinn::Connection::new(connection))
            .await
            .expect("failed to create H3 server connection");

        let resolver = tokio::time::timeout(Duration::from_secs(10), h3_conn.accept())
            .await
            .expect("timed out waiting for H3 request")
            .expect("failed to accept H3 request")
            .expect("expected one H3 request");
        let (req, mut stream) = resolver
            .resolve_request()
            .await
            .expect("failed to resolve H3 request");

        assert_eq!(req.method(), hyper::Method::GET);
        assert_eq!(req.uri().path(), "/upstream-h3");

        let response = hyper::Response::builder()
            .status(200)
            .header("content-type", "text/plain")
            .header("content-length", "14")
            .header("x-bifrost-upstream", "h3")
            .body(())
            .unwrap();

        stream
            .send_response(response)
            .await
            .expect("failed to send H3 response headers");
        stream
            .send_data(Bytes::from_static(b"upstream-h3-ok"))
            .await
            .expect("failed to send H3 response body");
        stream.finish().await.expect("failed to finish H3 stream");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn send_proxy_request(proxy_port: u16, origin_port: u16) -> String {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{proxy_port}"))
        .await
        .expect("failed to connect to proxy");
    let request = format!(
        "GET https://127.0.0.1:{origin_port}/upstream-h3 HTTP/1.1\r\n\
         Host: 127.0.0.1:{origin_port}\r\n\
         User-Agent: bifrost-upstream-h3-test\r\n\
         Connection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("failed to write proxy request");

    let mut response = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("timed out waiting for proxy response")
            .expect("failed to read proxy response");
        if n == 0 {
            break;
        }
        response.extend_from_slice(&buf[..n]);
        if response
            .windows(b"upstream-h3-ok".len())
            .any(|w| w == b"upstream-h3-ok")
        {
            break;
        }
    }

    String::from_utf8_lossy(&response).into_owned()
}

async fn direct_h3_preflight(origin_port: u16) {
    let h3_client = Http3Client::new_with_options(true).expect("failed to create H3 client");
    let direct_req = hyper::Request::builder()
        .method("GET")
        .uri(format!("https://127.0.0.1:{origin_port}/upstream-h3"))
        .header("Host", format!("127.0.0.1:{origin_port}"))
        .body(Bytes::new())
        .unwrap();
    let direct_response = h3_client
        .request_to_addr(
            "127.0.0.1",
            format!("127.0.0.1:{origin_port}").parse().unwrap(),
            direct_req,
        )
        .await
        .expect("direct HTTP/3 preflight should succeed");
    assert_eq!(direct_response.status(), 200);
    assert_eq!(direct_response.body().as_ref(), b"upstream-h3-ok");
}

#[tokio::test]
async fn test_http_proxy_to_h3_origin_disabled_by_default() {
    let _guard = UPSTREAM_HTTP3_E2E_LOCK.lock().await;
    let test_future = async {
        init_crypto_provider();

        let origin_port = pick_udp_port();
        let proxy_port = pick_tcp_port().await;
        let (ready_tx, ready_rx) = oneshot::channel();

        let origin_handle = tokio::spawn(start_h3_origin(origin_port, 1, ready_tx));
        ready_rx.await.unwrap();

        direct_h3_preflight(origin_port).await;

        let proxy_handle = tokio::spawn(async move {
            ProxyServer::new(ProxyConfig {
                host: "127.0.0.1".to_string(),
                port: proxy_port,
                unsafe_ssl: true,
                verbose_logging: true,
                enable_socks: false,
                ..ProxyConfig::default()
            })
            .run()
            .await
            .expect("proxy server should run");
        });

        tokio::time::sleep(Duration::from_millis(150)).await;

        let response_text = send_proxy_request(proxy_port, origin_port).await;
        assert!(response_text.contains("502 Bad Gateway"), "{response_text}");

        proxy_handle.abort();
        origin_handle.await.expect("origin task should finish");
    };

    tokio::time::timeout(Duration::from_secs(20), test_future)
        .await
        .expect("upstream H3 disabled-by-default E2E timed out");
}

#[tokio::test]
async fn test_http_proxy_to_h3_origin_enabled_by_rule() {
    let _guard = UPSTREAM_HTTP3_E2E_LOCK.lock().await;
    let test_future = async {
        init_crypto_provider();

        let origin_port = pick_udp_port();
        let proxy_port = pick_tcp_port().await;
        let (ready_tx, ready_rx) = oneshot::channel();

        let origin_handle = tokio::spawn(start_h3_origin(origin_port, 2, ready_tx));
        ready_rx.await.unwrap();

        direct_h3_preflight(origin_port).await;

        let proxy_handle = tokio::spawn(async move {
            ProxyServer::new(ProxyConfig {
                host: "127.0.0.1".to_string(),
                port: proxy_port,
                unsafe_ssl: true,
                verbose_logging: true,
                enable_socks: false,
                ..ProxyConfig::default()
            })
            .with_rules(Arc::new(StaticRulesResolver {
                enable_upstream_http3: true,
            }))
            .run()
            .await
            .expect("proxy server with rule should run");
        });

        tokio::time::sleep(Duration::from_millis(150)).await;

        let response_text = send_proxy_request(proxy_port, origin_port).await;
        let response_text_lower = response_text.to_ascii_lowercase();
        assert!(response_text.contains("200 OK"), "{response_text}");
        assert!(
            response_text_lower.contains("x-bifrost-upstream: h3"),
            "{response_text}"
        );
        assert!(response_text.contains("upstream-h3-ok"), "{response_text}");

        proxy_handle.abort();
        origin_handle.await.expect("origin task should finish");
    };

    tokio::time::timeout(Duration::from_secs(20), test_future)
        .await
        .expect("upstream H3 rule-enabled E2E timed out");
}
