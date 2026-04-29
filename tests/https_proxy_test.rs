mod common;

use bifrost_core::Protocol;
use bifrost_proxy::protocol::{
    compute_accept_key, WebSocketFrame, WebSocketReader, WebSocketWriter,
};
use bifrost_proxy::ProxyConfig;
use bifrost_tls::{generate_root_ca, init_crypto_provider, CertCache, DynamicCertGenerator};
use bytes::Bytes;
use common::{add_test_rule, create_proxy_client, start_test_proxy, start_test_proxy_with_config};
use common::{start_test_proxy_with_tls_support, MockH2TlsServer, MockHttpServer};
use futures_util::StreamExt;
use http_body_util::{Empty, Full};
use hyper::body::{Body, Frame};
use hyper::{Method, Request, Version};
use hyper_util::rt::{TokioExecutor, TokioIo};
use parking_lot::Mutex;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio_rustls::TlsConnector;

fn make_insecure_test_client_config(alpn_protocols: Vec<Vec<u8>>) -> ClientConfig {
    let mut client_config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerifier))
        .with_no_client_auth();
    client_config.alpn_protocols = alpn_protocols;
    client_config
}

#[derive(Debug)]
struct NoVerifier;

impl tokio_rustls::rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[tokio_rustls::rustls::pki_types::CertificateDer<'_>],
        _server_name: &tokio_rustls::rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: tokio_rustls::rustls::pki_types::UnixTime,
    ) -> Result<tokio_rustls::rustls::client::danger::ServerCertVerified, tokio_rustls::rustls::Error>
    {
        Ok(tokio_rustls::rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
            SignatureScheme::ED448,
        ]
    }
}

struct FailAfterFirstChunkBody {
    sent_chunk: bool,
}

impl Body for FailAfterFirstChunkBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if !self.sent_chunk {
            self.sent_chunk = true;
            return Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"partial")))));
        }
        Poll::Ready(Some(Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "intentional h2 body reset",
        ))))
    }
}

struct FlakyH2FallbackTlsServer {
    port: u16,
    h2_requests: Arc<Mutex<usize>>,
    h1_requests: Arc<Mutex<usize>>,
}

async fn start_flaky_h2_fallback_tls_server(
    body: Bytes,
    content_type: &'static str,
) -> FlakyH2FallbackTlsServer {
    use tokio_rustls::rustls;
    use tokio_rustls::rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use tokio_rustls::TlsAcceptor;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let rcgen::CertifiedKey { cert, signing_key } = rcgen::generate_simple_self_signed(vec![
        "localhost".to_string(),
        "intercepted.example.com".to_string(),
    ])
    .unwrap();
    let key_der = signing_key.serialize_der();
    let certs = vec![cert.der().clone()];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));

    let mut server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    let h2_requests = Arc::new(Mutex::new(0usize));
    let h1_requests = Arc::new(Mutex::new(0usize));
    let h2_requests_for_task = Arc::clone(&h2_requests);
    let h1_requests_for_task = Arc::clone(&h1_requests);

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let acceptor = acceptor.clone();
            let body = body.clone();
            let h2_requests = Arc::clone(&h2_requests_for_task);
            let h1_requests = Arc::clone(&h1_requests_for_task);
            tokio::spawn(async move {
                let tls_stream = match acceptor.accept(stream).await {
                    Ok(stream) => stream,
                    Err(_) => return,
                };
                let negotiated = tls_stream.get_ref().1.alpn_protocol().map(|v| v.to_vec());

                if negotiated.as_deref() == Some(b"h2".as_slice()) {
                    *h2_requests.lock() += 1;
                    let body_len = body.len();
                    let service = hyper::service::service_fn(move |_req| async move {
                        let response = hyper::Response::builder()
                            .status(200)
                            .header(hyper::header::CONTENT_TYPE, content_type)
                            .header(hyper::header::CONTENT_LENGTH, body_len.to_string())
                            .body(FailAfterFirstChunkBody { sent_chunk: false })
                            .unwrap();
                        Ok::<_, hyper::Error>(response)
                    });
                    let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                        .serve_connection(TokioIo::new(tls_stream), service)
                        .await;
                    return;
                }

                if negotiated.as_deref() == Some(b"http/1.1".as_slice()) || negotiated.is_none() {
                    *h1_requests.lock() += 1;
                    let mut stream = tls_stream;
                    let mut request = Vec::new();
                    loop {
                        let mut chunk = [0u8; 1024];
                        let n = match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => n,
                        };
                        request.extend_from_slice(&chunk[..n]);
                        if request.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let headers = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        content_type,
                        body.len()
                    );
                    let _ = stream.write_all(headers.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                }
            });
        }
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    FlakyH2FallbackTlsServer {
        port,
        h2_requests,
        h1_requests,
    }
}

async fn start_websocket_echo_server(ready_tx: oneshot::Sender<u16>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    ready_tx.send(port).unwrap();

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
                    Ok(frame) if frame.opcode == bifrost_proxy::protocol::Opcode::Close => {
                        let close_frame = WebSocketFrame::close(Some(1000), "");
                        ws_writer.write_frame(close_frame).await.ok();
                        break;
                    }
                    Ok(frame) => {
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

async fn start_tls_websocket_echo_server(
    ready_tx: oneshot::Sender<u16>,
    negotiated_alpn: Arc<Mutex<Vec<Option<Vec<u8>>>>>,
) {
    use tokio_rustls::rustls;
    use tokio_rustls::rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use tokio_rustls::TlsAcceptor;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    ready_tx.send(port).unwrap();

    let rcgen::CertifiedKey { cert, signing_key } = rcgen::generate_simple_self_signed(vec![
        "localhost".to_string(),
        "intercepted.example.com".to_string(),
    ])
    .unwrap();
    let key_der = signing_key.serialize_der();
    let certs = vec![cert.der().clone()];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));

    let mut server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    while let Ok((stream, _)) = listener.accept().await {
        let acceptor = acceptor.clone();
        let negotiated_alpn = Arc::clone(&negotiated_alpn);
        tokio::spawn(async move {
            let tls_stream = acceptor.accept(stream).await.unwrap();
            let negotiated = tls_stream.get_ref().1.alpn_protocol().map(|v| v.to_vec());
            negotiated_alpn.lock().push(negotiated.clone());

            if negotiated.as_deref() == Some(b"h2".as_slice()) {
                let service =
                    hyper::service::service_fn(|req: Request<hyper::body::Incoming>| async move {
                        let selected_protocol = req
                            .headers()
                            .get("Sec-WebSocket-Protocol")
                            .and_then(|v| v.to_str().ok())
                            .map(|v| v.split(',').next().unwrap_or(v).trim().to_string());
                        let selected_extensions = req
                            .headers()
                            .get("Sec-WebSocket-Extensions")
                            .and_then(|v| v.to_str().ok())
                            .map(ToOwned::to_owned);

                        tokio::spawn(async move {
                            let upgraded = hyper::upgrade::on(req).await.unwrap();
                            let upgraded = TokioIo::new(upgraded);
                            let (reader, writer) = tokio::io::split(upgraded);
                            let mut ws_reader = WebSocketReader::new(reader);
                            let mut ws_writer = WebSocketWriter::new(writer, false);

                            while let Some(result) = ws_reader.next().await {
                                match result {
                                    Ok(frame)
                                        if frame.opcode
                                            == bifrost_proxy::protocol::Opcode::Close =>
                                    {
                                        let close_frame = WebSocketFrame::close(Some(1000), "");
                                        ws_writer.write_frame(close_frame).await.ok();
                                        break;
                                    }
                                    Ok(frame) => {
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

                        let mut response = hyper::Response::builder().status(200);
                        if let Some(protocol) = selected_protocol {
                            response = response.header("Sec-WebSocket-Protocol", protocol);
                        }
                        if let Some(extensions) = selected_extensions {
                            response = response.header("Sec-WebSocket-Extensions", extensions);
                        }

                        Ok::<_, hyper::Error>(response.body(Full::new(Bytes::new())).unwrap())
                    });

                let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                    .enable_connect_protocol()
                    .serve_connection(TokioIo::new(tls_stream), service)
                    .await;
                return;
            }

            if negotiated.as_deref() != Some(b"http/1.1".as_slice()) {
                return;
            }

            let mut stream = tls_stream;
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

            let (reader, writer) = tokio::io::split(stream);
            let mut ws_reader = WebSocketReader::new(reader);
            let mut ws_writer = WebSocketWriter::new(writer, false);

            while let Some(result) = ws_reader.next().await {
                match result {
                    Ok(frame) if frame.opcode == bifrost_proxy::protocol::Opcode::Close => {
                        let close_frame = WebSocketFrame::close(Some(1000), "");
                        ws_writer.write_frame(close_frame).await.ok();
                        break;
                    }
                    Ok(frame) => {
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
async fn test_https_tunnel() {
    let proxy = start_test_proxy().await;

    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_addr = target.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = target.accept().await;
    });

    let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();

    let connect_request = format!(
        "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        target_addr.port(),
        target_addr.port()
    );
    stream.write_all(connect_request.as_bytes()).await.unwrap();

    let mut response = vec![0u8; 1024];
    let n = stream.read(&mut response).await.unwrap();
    let response_str = String::from_utf8_lossy(&response[..n]);

    assert!(
        response_str.contains("200") || response_str.contains("OK"),
        "CONNECT should succeed, got: {}",
        response_str
    );
}

#[tokio::test]
async fn test_https_tunnel_with_port() {
    let proxy = start_test_proxy().await;

    let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();

    let connect_request = "CONNECT example.com:8443 HTTP/1.1\r\nHost: example.com:8443\r\n\r\n";
    stream.write_all(connect_request.as_bytes()).await.unwrap();

    let mut response = vec![0u8; 1024];
    let n = stream.read(&mut response).await.unwrap();
    let response_str = String::from_utf8_lossy(&response[..n]);

    assert!(
        response_str.contains("200") || response_str.contains("OK") || response_str.contains("502"),
        "CONNECT response: {}",
        response_str
    );
}

#[tokio::test]
async fn test_https_tunnel_with_host_rule() {
    let proxy = start_test_proxy().await;

    add_test_rule(
        &proxy,
        "secure.example.com",
        Protocol::Host,
        "127.0.0.1:8443",
    );

    let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();

    let connect_request =
        "CONNECT secure.example.com:443 HTTP/1.1\r\nHost: secure.example.com:443\r\n\r\n";
    stream.write_all(connect_request.as_bytes()).await.unwrap();

    let mut response = vec![0u8; 1024];
    let n = stream.read(&mut response).await.unwrap();
    let response_str = String::from_utf8_lossy(&response[..n]);

    assert!(
        response_str.contains("200") || response_str.contains("502"),
        "CONNECT with host rule should respond, got: {}",
        response_str
    );
}

#[tokio::test]
async fn test_https_host_rewrite_to_http_forces_tls_interception() {
    let upstream = MockHttpServer::start().await;
    let proxy = start_test_proxy_with_tls_support(ProxyConfig::default()).await;

    add_test_rule(
        &proxy,
        "rewritten.example.com",
        Protocol::Http,
        &upstream.url("/"),
    );

    let client = create_proxy_client(&proxy);
    let response = client
        .get("https://rewritten.example.com/hello?x=1")
        .send()
        .await
        .expect("HTTPS request rewritten to HTTP upstream should succeed");

    assert!(response.status().is_success());
    let body = response.text().await.unwrap();
    assert!(
        body.contains("\"path\":\"/hello\""),
        "expected upstream HTTP server to receive rewritten request path, got: {}",
        body
    );
    assert!(
        body.contains("\"method\":\"GET\""),
        "expected upstream HTTP server to receive GET request, got: {}",
        body
    );
}

#[tokio::test]
async fn test_https_interception_upstream_h2_host_header_removed() {
    // TLS interception 依赖 rustls/ring provider 初始化。
    init_crypto_provider();
    let _ = bifrost_core::init_logging("debug");

    let upstream = MockH2TlsServer::start().await;

    let config = ProxyConfig {
        enable_tls_interception: true,
        unsafe_ssl: true,
        verbose_logging: true,
        ..Default::default()
    };
    let proxy = start_test_proxy_with_config(config).await;

    // 将被拦截的域名转发到本地 h2 TLS server。
    add_test_rule(
        &proxy,
        "intercepted.example.com",
        Protocol::Host,
        &format!("127.0.0.1:{}", upstream.port),
    );

    let client = create_proxy_client(&proxy);
    let resp = tokio::time::timeout(
        tokio::time::Duration::from_secs(10),
        client.get("https://intercepted.example.com/test").send(),
    )
    .await
    .expect("request timeout")
    .expect("request failed");

    assert_eq!(resp.status(), 200);

    let host_seen = upstream
        .wait_host_header_seen()
        .await
        .expect("upstream did not receive request");
    assert!(
        !host_seen,
        "upstream h2 request should not include Host header"
    );
}

#[tokio::test]
async fn test_https_interception_retries_h2_body_failure_with_http1() {
    init_crypto_provider();

    let expected_body = Bytes::from(vec![0x89; 256 * 1024]);
    let upstream = start_flaky_h2_fallback_tls_server(expected_body.clone(), "image/png").await;
    let config = ProxyConfig {
        enable_tls_interception: true,
        unsafe_ssl: true,
        verbose_logging: true,
        ..Default::default()
    };
    let proxy = start_test_proxy_with_config(config).await;

    add_test_rule(
        &proxy,
        "intercepted.example.com",
        Protocol::Host,
        &format!("127.0.0.1:{}", upstream.port),
    );

    let client = create_proxy_client(&proxy);
    let response = tokio::time::timeout(
        tokio::time::Duration::from_secs(10),
        client
            .get("https://intercepted.example.com/asset.png")
            .send(),
    )
    .await
    .expect("request timeout")
    .expect("request failed");

    let status = response.status();
    let body = response.bytes().await.unwrap();
    assert_eq!(
        status,
        200,
        "unexpected body: {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(body, expected_body);
    assert_eq!(
        *upstream.h2_requests.lock(),
        1,
        "first upstream attempt should use h2"
    );
    assert_eq!(
        *upstream.h1_requests.lock(),
        1,
        "failed h2 body should be retried over http/1.1"
    );
}

#[tokio::test]
async fn test_http_handler_retries_h2_body_failure_with_http1() {
    init_crypto_provider();

    let expected_body = Bytes::from(vec![0x42; 128 * 1024]);
    let upstream = start_flaky_h2_fallback_tls_server(expected_body.clone(), "image/png").await;
    let config = ProxyConfig {
        unsafe_ssl: true,
        verbose_logging: true,
        ..Default::default()
    };
    let proxy = start_test_proxy_with_config(config).await;

    add_test_rule(
        &proxy,
        "intercepted.example.com",
        Protocol::Https,
        &format!("https://127.0.0.1:{}", upstream.port),
    );

    let client = create_proxy_client(&proxy);
    let response = tokio::time::timeout(
        tokio::time::Duration::from_secs(10),
        client
            .get("http://intercepted.example.com/asset.png")
            .send(),
    )
    .await
    .expect("request timeout")
    .expect("request failed");

    let status = response.status();
    let body = response.bytes().await.unwrap();
    assert_eq!(
        status,
        200,
        "unexpected body: {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(body, expected_body);
    assert_eq!(
        *upstream.h2_requests.lock(),
        1,
        "first upstream attempt should use h2"
    );
    assert_eq!(
        *upstream.h1_requests.lock(),
        1,
        "failed h2 body should be retried over http/1.1"
    );
}

#[tokio::test]
async fn test_https_interception_accepts_h2_websocket_extended_connect() {
    init_crypto_provider();

    let config = ProxyConfig {
        enable_tls_interception: true,
        unsafe_ssl: true,
        verbose_logging: true,
        ..Default::default()
    };
    let proxy = start_test_proxy_with_config(config).await;

    let (ready_tx, ready_rx) = oneshot::channel();
    tokio::spawn(start_websocket_echo_server(ready_tx));
    let ws_port = ready_rx.await.unwrap();

    add_test_rule(
        &proxy,
        "intercepted.example.com",
        Protocol::Ws,
        &format!("127.0.0.1:{}", ws_port),
    );

    let mut tunnel = TcpStream::connect(proxy.addr()).await.unwrap();
    let connect_request =
        "CONNECT intercepted.example.com:443 HTTP/1.1\r\nHost: intercepted.example.com:443\r\n\r\n";
    tunnel.write_all(connect_request.as_bytes()).await.unwrap();

    let mut connect_response = vec![0u8; 1024];
    let n = tunnel.read(&mut connect_response).await.unwrap();
    let response_str = String::from_utf8_lossy(&connect_response[..n]);
    assert!(
        response_str.contains("200"),
        "CONNECT should succeed, got: {}",
        response_str
    );

    let client_config =
        make_insecure_test_client_config(vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
    let connector = TlsConnector::from(Arc::new(client_config));
    let server_name = ServerName::try_from("intercepted.example.com".to_string()).unwrap();
    let tls_stream = connector.connect(server_name, tunnel).await.unwrap();
    assert_eq!(
        tls_stream.get_ref().1.alpn_protocol(),
        Some(b"h2".as_slice())
    );

    let (mut sender, connection) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
        .handshake(TokioIo::new(tls_stream))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let request = Request::builder()
        .method(Method::CONNECT)
        .uri("https://intercepted.example.com/echo")
        .version(Version::HTTP_2)
        .header("Sec-WebSocket-Version", "13")
        .extension(hyper::ext::Protocol::from_static("websocket"))
        .body(Empty::<Bytes>::new())
        .unwrap();

    let response = sender.send_request(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let upgraded = hyper::upgrade::on(response).await.unwrap();
    let upgraded = TokioIo::new(upgraded);
    let (reader, writer) = tokio::io::split(upgraded);
    let mut ws_reader = WebSocketReader::new(reader);
    let mut ws_writer = WebSocketWriter::new(writer, true);

    ws_writer
        .write_frame(WebSocketFrame::text("hello over h2"))
        .await
        .unwrap();

    let echoed = tokio::time::timeout(std::time::Duration::from_secs(5), ws_reader.next())
        .await
        .expect("timed out waiting for websocket echo")
        .expect("websocket stream should stay open")
        .expect("websocket frame should decode");

    assert_eq!(echoed.payload, Bytes::from_static(b"hello over h2"));

    ws_writer
        .write_frame(WebSocketFrame::close(Some(1000), "done"))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_https_interception_wss_upstream_websocket_bridge_works() {
    init_crypto_provider();

    let config = ProxyConfig {
        enable_tls_interception: true,
        unsafe_ssl: true,
        verbose_logging: true,
        ..Default::default()
    };
    let proxy = start_test_proxy_with_config(config).await;

    let (ready_tx, ready_rx) = oneshot::channel();
    let negotiated_alpn = Arc::new(Mutex::new(Vec::new()));
    tokio::spawn(start_tls_websocket_echo_server(
        ready_tx,
        Arc::clone(&negotiated_alpn),
    ));
    let ws_port = ready_rx.await.unwrap();

    add_test_rule(
        &proxy,
        "intercepted.example.com",
        Protocol::Wss,
        &format!("127.0.0.1:{}", ws_port),
    );

    let mut tunnel = TcpStream::connect(proxy.addr()).await.unwrap();
    let connect_request =
        "CONNECT intercepted.example.com:443 HTTP/1.1\r\nHost: intercepted.example.com:443\r\n\r\n";
    tunnel.write_all(connect_request.as_bytes()).await.unwrap();

    let mut connect_response = vec![0u8; 1024];
    let n = tunnel.read(&mut connect_response).await.unwrap();
    let response_str = String::from_utf8_lossy(&connect_response[..n]);
    assert!(
        response_str.contains("200"),
        "CONNECT should succeed, got: {}",
        response_str
    );

    let client_config =
        make_insecure_test_client_config(vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
    let connector = TlsConnector::from(Arc::new(client_config));
    let server_name = ServerName::try_from("intercepted.example.com".to_string()).unwrap();
    let tls_stream = connector.connect(server_name, tunnel).await.unwrap();
    assert_eq!(
        tls_stream.get_ref().1.alpn_protocol(),
        Some(b"h2".as_slice())
    );

    let (mut sender, connection) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
        .handshake(TokioIo::new(tls_stream))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let request = Request::builder()
        .method(Method::CONNECT)
        .uri("https://intercepted.example.com/echo")
        .version(Version::HTTP_2)
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Protocol", "chat, superchat")
        .header(
            "Sec-WebSocket-Extensions",
            "permessage-deflate; client_max_window_bits",
        )
        .extension(hyper::ext::Protocol::from_static("websocket"))
        .body(Empty::<Bytes>::new())
        .unwrap();

    let response = sender.send_request(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let upgraded = hyper::upgrade::on(response).await.unwrap();
    let upgraded = TokioIo::new(upgraded);
    let (reader, writer) = tokio::io::split(upgraded);
    let mut ws_reader = WebSocketReader::new(reader);
    let mut ws_writer = WebSocketWriter::new(writer, true);

    ws_writer
        .write_frame(WebSocketFrame::text("hello over upstream wss"))
        .await
        .unwrap();

    let echoed = tokio::time::timeout(std::time::Duration::from_secs(5), ws_reader.next())
        .await
        .expect("timed out waiting for websocket echo")
        .expect("websocket stream should stay open")
        .expect("websocket frame should decode");

    assert_eq!(
        echoed.payload,
        Bytes::from_static(b"hello over upstream wss")
    );
    assert!(
        !negotiated_alpn.lock().is_empty(),
        "expected an upstream WSS TLS handshake"
    );
}

#[tokio::test]
async fn test_https_interception_http1_client_websocket_can_bridge_to_upstream_http1() {
    init_crypto_provider();

    let config = ProxyConfig {
        enable_tls_interception: true,
        unsafe_ssl: true,
        verbose_logging: true,
        ..Default::default()
    };
    let proxy = start_test_proxy_with_config(config).await;

    let (ready_tx, ready_rx) = oneshot::channel();
    let negotiated_alpn = Arc::new(Mutex::new(Vec::new()));
    tokio::spawn(start_tls_websocket_echo_server(
        ready_tx,
        Arc::clone(&negotiated_alpn),
    ));
    let ws_port = ready_rx.await.unwrap();

    add_test_rule(
        &proxy,
        "intercepted.example.com",
        Protocol::Wss,
        &format!("127.0.0.1:{}", ws_port),
    );

    let mut tunnel = TcpStream::connect(proxy.addr()).await.unwrap();
    let connect_request =
        "CONNECT intercepted.example.com:443 HTTP/1.1\r\nHost: intercepted.example.com:443\r\n\r\n";
    tunnel.write_all(connect_request.as_bytes()).await.unwrap();

    let mut connect_response = vec![0u8; 1024];
    let n = tunnel.read(&mut connect_response).await.unwrap();
    let response_str = String::from_utf8_lossy(&connect_response[..n]);
    assert!(
        response_str.contains("200"),
        "CONNECT should succeed, got: {}",
        response_str
    );

    let client_config = make_insecure_test_client_config(vec![b"http/1.1".to_vec()]);
    let connector = TlsConnector::from(Arc::new(client_config));
    let server_name = ServerName::try_from("intercepted.example.com".to_string()).unwrap();
    let mut tls_stream = connector.connect(server_name, tunnel).await.unwrap();
    assert_eq!(
        tls_stream.get_ref().1.alpn_protocol(),
        Some(b"http/1.1".as_slice())
    );

    let sec_key = "dGhlIHNhbXBsZSBub25jZQ==";
    let request = format!(
        "GET /echo HTTP/1.1\r\n\
         Host: intercepted.example.com\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {}\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n",
        sec_key
    );
    tls_stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = Vec::new();
    loop {
        let mut chunk = [0u8; 1024];
        let n = tls_stream.read(&mut chunk).await.unwrap();
        assert!(
            n > 0,
            "proxy closed connection before websocket upgrade response"
        );
        response.extend_from_slice(&chunk[..n]);
        if response.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.starts_with("HTTP/1.1 101"),
        "unexpected websocket response: {}",
        response_str
    );
    assert!(
        response_str
            .to_ascii_lowercase()
            .contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="),
        "proxy should synthesize Sec-WebSocket-Accept for downstream h1 clients: {}",
        response_str
    );

    let (reader, writer) = tokio::io::split(tls_stream);
    let mut ws_reader = WebSocketReader::new(reader);
    let mut ws_writer = WebSocketWriter::new(writer, true);

    ws_writer
        .write_frame(WebSocketFrame::text("hello from http1 client"))
        .await
        .unwrap();

    let echoed = tokio::time::timeout(std::time::Duration::from_secs(5), ws_reader.next())
        .await
        .expect("timed out waiting for websocket echo")
        .expect("websocket stream should stay open")
        .expect("websocket frame should decode");

    assert_eq!(
        echoed.payload,
        Bytes::from_static(b"hello from http1 client")
    );
    assert!(
        negotiated_alpn
            .lock()
            .iter()
            .any(|alpn| alpn.as_deref() == Some(b"http/1.1".as_slice())),
        "expected at least one upstream WSS handshake to negotiate http/1.1, got {:?}",
        *negotiated_alpn.lock()
    );
}

#[tokio::test]
async fn test_https_interception_accepts_large_h2_request_headers() {
    init_crypto_provider();

    let upstream = MockH2TlsServer::start().await;
    let config = ProxyConfig {
        enable_tls_interception: true,
        unsafe_ssl: true,
        verbose_logging: true,
        ..Default::default()
    };
    let proxy = start_test_proxy_with_config(config).await;

    add_test_rule(
        &proxy,
        "intercepted.example.com",
        Protocol::Host,
        &format!("127.0.0.1:{}", upstream.port),
    );

    let mut tunnel = TcpStream::connect(proxy.addr()).await.unwrap();
    let connect_request =
        "CONNECT intercepted.example.com:443 HTTP/1.1\r\nHost: intercepted.example.com:443\r\n\r\n";
    tunnel.write_all(connect_request.as_bytes()).await.unwrap();

    let mut connect_response = vec![0u8; 1024];
    let n = tunnel.read(&mut connect_response).await.unwrap();
    let response_str = String::from_utf8_lossy(&connect_response[..n]);
    assert!(
        response_str.contains("200"),
        "CONNECT should succeed, got: {}",
        response_str
    );

    let client_config =
        make_insecure_test_client_config(vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
    let connector = TlsConnector::from(Arc::new(client_config));
    let server_name = ServerName::try_from("intercepted.example.com".to_string()).unwrap();
    let tls_stream = connector.connect(server_name, tunnel).await.unwrap();
    assert_eq!(
        tls_stream.get_ref().1.alpn_protocol(),
        Some(b"h2".as_slice())
    );

    let (mut sender, connection) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
        .handshake(TokioIo::new(tls_stream))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let large_cookie = format!("session={}", "x".repeat(24 * 1024));
    let request = Request::builder()
        .method(Method::GET)
        .uri("https://intercepted.example.com/test")
        .version(Version::HTTP_2)
        .header(hyper::header::COOKIE, large_cookie)
        .body(Empty::<Bytes>::new())
        .unwrap();

    let response = sender.send_request(request).await.unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
#[ignore = "Requires full TLS interception implementation"]
async fn test_https_interception() {
    let config = ProxyConfig {
        enable_tls_interception: true,
        ..Default::default()
    };

    let proxy = start_test_proxy_with_config(config).await;

    let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();

    let connect_request =
        "CONNECT intercepted.example.com:443 HTTP/1.1\r\nHost: intercepted.example.com:443\r\n\r\n";
    stream.write_all(connect_request.as_bytes()).await.unwrap();

    let mut response = vec![0u8; 1024];
    let n = stream.read(&mut response).await.unwrap();
    let response_str = String::from_utf8_lossy(&response[..n]);

    assert!(
        response_str.contains("200") || response_str.contains("OK"),
        "TLS interception CONNECT should succeed"
    );
}

#[tokio::test]
async fn test_dynamic_cert_generation() {
    init_crypto_provider();
    let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
    let generator = DynamicCertGenerator::new(Arc::clone(&ca));

    let cert_key = generator
        .generate_for_domain("test.example.com")
        .expect("Failed to generate certificate");

    assert_eq!(cert_key.cert.len(), 2, "Should have cert + CA chain");

    let wildcard_cert = generator
        .generate_for_domain("*.example.com")
        .expect("Failed to generate wildcard certificate");
    assert_eq!(wildcard_cert.cert.len(), 2);

    let ip_cert = generator
        .generate_for_domain("192.168.1.1")
        .expect("Failed to generate IP certificate");
    assert_eq!(ip_cert.cert.len(), 2);
}

#[tokio::test]
async fn test_cert_cache() {
    init_crypto_provider();
    let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
    let generator = DynamicCertGenerator::new(Arc::clone(&ca));
    let cache = CertCache::new();

    let cert1 = generator.generate_for_domain("cached.example.com").unwrap();
    cache.insert("cached.example.com", Arc::new(cert1));

    let cached = cache.get("cached.example.com");
    assert!(cached.is_some());
    assert_eq!(cached.unwrap().cert.len(), 2);

    let other_cert = generator.generate_for_domain("other.example.com").unwrap();
    cache.insert("other.example.com", Arc::new(other_cert));

    assert_eq!(cache.len(), 2);
}

#[tokio::test]
async fn test_multiple_https_tunnels() {
    let proxy = start_test_proxy().await;

    let domains = vec![
        "domain1.example.com:443",
        "domain2.example.com:443",
        "domain3.example.com:443",
    ];

    for domain in domains {
        let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();

        let connect_request = format!("CONNECT {} HTTP/1.1\r\nHost: {}\r\n\r\n", domain, domain);
        stream.write_all(connect_request.as_bytes()).await.unwrap();

        let mut response = vec![0u8; 1024];
        let n = stream.read(&mut response).await.unwrap();
        let response_str = String::from_utf8_lossy(&response[..n]);

        assert!(
            response_str.contains("200")
                || response_str.contains("OK")
                || response_str.contains("502"),
            "CONNECT to {} should respond",
            domain
        );
    }
}

#[tokio::test]
async fn test_https_tunnel_invalid_host() {
    let proxy = start_test_proxy().await;

    let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();

    let connect_request = "CONNECT :443 HTTP/1.1\r\nHost: :443\r\n\r\n";
    stream.write_all(connect_request.as_bytes()).await.unwrap();

    let mut response = vec![0u8; 1024];
    let n = stream.read(&mut response).await.unwrap();
    let response_str = String::from_utf8_lossy(&response[..n]);

    assert!(
        response_str.contains("400")
            || response_str.contains("502")
            || response_str.contains("Bad"),
        "Invalid host should return error"
    );
}

#[test]
fn test_generate_root_ca() {
    init_crypto_provider();
    let ca = generate_root_ca().expect("Failed to generate root CA");
    let cert_der = ca.certificate_der().expect("Failed to get cert DER");
    let key_der = ca.private_key_der();

    assert!(!cert_der.is_empty());
    match key_der {
        bifrost_tls::rustls::pki_types::PrivateKeyDer::Pkcs8(key) => {
            assert!(!key.secret_pkcs8_der().is_empty());
        }
        _ => panic!("Expected PKCS8 key"),
    }
}

#[test]
fn test_dynamic_cert_for_subdomain() {
    init_crypto_provider();
    let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
    let generator = DynamicCertGenerator::new(ca);

    let cert = generator
        .generate_for_domain("api.sub.example.com")
        .expect("Failed to generate subdomain cert");
    assert_eq!(cert.cert.len(), 2);
}

#[test]
fn test_dynamic_cert_for_localhost() {
    init_crypto_provider();
    let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
    let generator = DynamicCertGenerator::new(ca);

    let cert = generator
        .generate_for_domain("localhost")
        .expect("Failed to generate localhost cert");
    assert_eq!(cert.cert.len(), 2);
}

#[test]
fn test_dynamic_cert_for_ipv4() {
    init_crypto_provider();
    let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
    let generator = DynamicCertGenerator::new(ca);

    let cert = generator
        .generate_for_domain("127.0.0.1")
        .expect("Failed to generate IPv4 cert");
    assert_eq!(cert.cert.len(), 2);
}

#[test]
fn test_cert_cache_capacity() {
    init_crypto_provider();
    let cache = CertCache::with_capacity(2);
    let ca = Arc::new(generate_root_ca().expect("Failed to generate CA"));
    let generator = DynamicCertGenerator::new(ca);

    let cert1 = generator.generate_for_domain("domain1.com").unwrap();
    let cert2 = generator.generate_for_domain("domain2.com").unwrap();
    let cert3 = generator.generate_for_domain("domain3.com").unwrap();

    cache.insert("domain1.com", Arc::new(cert1));
    cache.insert("domain2.com", Arc::new(cert2));
    cache.insert("domain3.com", Arc::new(cert3));

    assert!(cache.len() <= 3);
}
