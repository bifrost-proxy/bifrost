use bifrost_core::Protocol;
use bifrost_proxy::{ProxyConfig, ProxyServer, ResolvedRules, RuleValue, RulesResolver};
use bytes::Bytes;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

#[allow(dead_code)]
pub struct TestProxy {
    pub port: u16,
    pub host: String,
    pub ca_cert_der: Option<Vec<u8>>,
    pub server: ProxyServer,
    rules: Arc<TestRulesResolver>,
}

pub struct TestRulesResolver {
    rules: parking_lot::RwLock<Vec<TestRule>>,
}

struct TestRule {
    pattern: String,
    protocol: Protocol,
    value: String,
}

impl TestRulesResolver {
    pub fn new() -> Self {
        Self {
            rules: parking_lot::RwLock::new(Vec::new()),
        }
    }

    pub fn add_rule(&self, pattern: &str, protocol: Protocol, value: &str) {
        let mut rules = self.rules.write();
        rules.push(TestRule {
            pattern: pattern.to_string(),
            protocol,
            value: value.to_string(),
        });
    }

    pub fn clear(&self) {
        let mut rules = self.rules.write();
        rules.clear();
    }
}

impl RulesResolver for TestRulesResolver {
    fn resolve_with_context(
        &self,
        url: &str,
        _method: &str,
        _req_headers: &std::collections::HashMap<String, String>,
        _req_cookies: &std::collections::HashMap<String, String>,
    ) -> ResolvedRules {
        let rules = self.rules.read();
        let mut resolved = ResolvedRules::default();

        for rule in rules.iter() {
            if url.contains(&rule.pattern) || rule.pattern == "*" {
                match rule.protocol {
                    Protocol::Host => {
                        resolved.host = Some(rule.value.clone());
                    }
                    Protocol::Http | Protocol::Https => {
                        let prefix = if matches!(rule.protocol, Protocol::Http) {
                            "http://"
                        } else {
                            "https://"
                        };
                        if let Some(rest) = rule.value.strip_prefix(prefix) {
                            let host_port = rest.split('/').next().unwrap_or(rest);
                            let default_port = if matches!(rule.protocol, Protocol::Http) {
                                80
                            } else {
                                443
                            };
                            let normalized_host = if host_port.contains(':') {
                                host_port.to_string()
                            } else {
                                format!("{host_port}:{default_port}")
                            };
                            resolved.host = Some(normalized_host);
                            resolved.host_protocol = Some(rule.protocol);
                        }
                    }
                    Protocol::Ws => {
                        resolved.host = Some(rule.value.clone());
                        resolved.host_protocol = Some(Protocol::Ws);
                    }
                    Protocol::Wss => {
                        resolved.host = Some(rule.value.clone());
                        resolved.host_protocol = Some(Protocol::Wss);
                    }
                    Protocol::ReqHeaders => {
                        if let Some((key, value)) = rule.value.split_once('=') {
                            resolved
                                .req_headers
                                .push((key.to_string(), value.to_string()));
                        }
                    }
                    Protocol::ResHeaders => {
                        if let Some((key, value)) = rule.value.split_once('=') {
                            resolved
                                .res_headers
                                .push((key.to_string(), value.to_string()));
                        }
                    }
                    Protocol::ReqDelay => {
                        if let Ok(delay) = rule.value.parse() {
                            resolved.req_delay = Some(delay);
                        }
                    }
                    Protocol::ResDelay => {
                        if let Ok(delay) = rule.value.parse() {
                            resolved.res_delay = Some(delay);
                        }
                    }
                    Protocol::StatusCode => {
                        if let Ok(code) = rule.value.parse() {
                            resolved.status_code = Some(code);
                        }
                    }
                    Protocol::Method => {
                        resolved.method = Some(rule.value.clone());
                    }
                    Protocol::Ua => {
                        resolved.ua = Some(rule.value.clone());
                    }
                    Protocol::Referer => {
                        resolved.referer = Some(rule.value.clone());
                    }
                    Protocol::ReqCors => {
                        resolved.req_cors = bifrost_proxy::CorsConfig::enable_all();
                    }
                    Protocol::ResCors => {
                        resolved.res_cors = bifrost_proxy::CorsConfig::enable_all();
                    }
                    Protocol::ReqBody => {
                        resolved.req_body = Some(Bytes::from(rule.value.clone()));
                    }
                    Protocol::ResBody => {
                        resolved.res_body = Some(Bytes::from(rule.value.clone()));
                    }
                    _ => {
                        resolved.rules.push(RuleValue {
                            pattern: rule.pattern.clone(),
                            protocol: rule.protocol,
                            value: rule.value.clone(),
                            options: HashMap::new(),
                            rule_name: None,
                            raw: None,
                            line: None,
                            auto_tls_intercept: true,
                        });
                    }
                }
            }
        }

        resolved
    }
}

impl TestProxy {
    #[allow(dead_code)]
    pub fn port(&self) -> u16 {
        self.port
    }

    #[allow(dead_code)]
    pub fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn proxy_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

pub async fn start_test_proxy() -> TestProxy {
    start_test_proxy_with_config(ProxyConfig::default()).await
}

pub async fn start_test_proxy_with_config(mut config: ProxyConfig) -> TestProxy {
    let enable_tls_support = config.enable_tls_interception;
    start_test_proxy_with_config_inner(&mut config, enable_tls_support).await
}

#[allow(dead_code)]
pub async fn start_test_proxy_with_tls_support(mut config: ProxyConfig) -> TestProxy {
    start_test_proxy_with_config_inner(&mut config, true).await
}

async fn start_test_proxy_with_config_inner(
    config: &mut ProxyConfig,
    enable_tls_support: bool,
) -> TestProxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    config.port = addr.port();
    config.host = "127.0.0.1".to_string();

    let rules = Arc::new(TestRulesResolver::new());
    // 为 TLS interception 测试准备 CA/证书生成器。
    let tls_config = if enable_tls_support {
        bifrost_tls::init_crypto_provider();

        let ca = Arc::new(bifrost_tls::generate_root_ca().expect("Failed to generate test CA"));
        let cert_generator = Arc::new(bifrost_tls::DynamicCertGenerator::new(Arc::clone(&ca)));
        let sni_resolver = Arc::new(bifrost_tls::SniResolver::new(Arc::clone(&ca)));

        let ca_cert_der = ca
            .certificate_der()
            .expect("Failed to get CA cert DER")
            .as_ref()
            .to_vec();
        let ca_key_der = match ca.private_key_der() {
            bifrost_tls::rustls::pki_types::PrivateKeyDer::Pkcs8(k) => {
                k.secret_pkcs8_der().to_vec()
            }
            _ => vec![],
        };

        Arc::new(bifrost_proxy::TlsConfig {
            ca_cert: Some(ca_cert_der),
            ca_key: Some(ca_key_der),
            cert_generator: Some(cert_generator),
            sni_resolver: Some(sni_resolver),
        })
    } else {
        Arc::new(bifrost_proxy::TlsConfig::default())
    };

    let server = ProxyServer::new(config.clone())
        .with_rules(Arc::clone(&rules) as Arc<dyn RulesResolver>)
        .with_tls_config(Arc::clone(&tls_config));

    let serve_rules = Arc::clone(&rules);
    let serve_server = ProxyServer::new(config.clone())
        .with_rules(serve_rules as Arc<dyn RulesResolver>)
        .with_tls_config(Arc::clone(&tls_config));

    tokio::spawn(async move {
        let _ = serve_server.serve(listener).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    TestProxy {
        port: config.port,
        host: config.host.clone(),
        ca_cert_der: tls_config.ca_cert.clone(),
        server,
        rules,
    }
}

#[allow(dead_code)]
pub async fn start_test_proxy_with_socks5(socks5_port: u16) -> TestProxy {
    let config = ProxyConfig {
        socks5_port: Some(socks5_port),
        ..Default::default()
    };
    start_test_proxy_with_config(config).await
}

pub fn create_proxy_client(proxy: &TestProxy) -> reqwest::Client {
    let proxy_url = proxy.proxy_url();
    reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url).unwrap())
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap()
}

#[allow(dead_code)]
pub fn create_socks5_client(host: &str, port: u16) -> reqwest::Client {
    let proxy_url = format!("socks5://{}:{}", host, port);
    reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url).unwrap())
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap()
}

#[allow(dead_code)]
pub fn create_socks5_client_with_auth(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
) -> reqwest::Client {
    let proxy_url = format!("socks5://{}:{}@{}:{}", username, password, host, port);
    reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url).unwrap())
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap()
}

pub fn add_test_rule(proxy: &TestProxy, pattern: &str, protocol: Protocol, value: &str) {
    proxy.rules.add_rule(pattern, protocol, value);
}

pub fn clear_test_rules(proxy: &TestProxy) {
    proxy.rules.clear();
}

#[allow(dead_code)]
pub struct MockHttpServer {
    pub port: u16,
    pub addr: SocketAddr,
}

impl MockHttpServer {
    pub async fn start() -> Self {
        use http_body_util::Full;
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper::{Request, Response};
        use hyper_util::rt::TokioIo;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let io = TokioIo::new(stream);
                    tokio::spawn(async move {
                        let service =
                            service_fn(|req: Request<hyper::body::Incoming>| async move {
                                let path = req.uri().path().to_string();
                                let method = req.method().to_string();
                                let headers: Vec<String> = req
                                    .headers()
                                    .iter()
                                    .map(|(k, v)| format!("{}: {}", k, v.to_str().unwrap_or("")))
                                    .collect();

                                let body = format!(
                                    "{{\"path\":\"{}\",\"method\":\"{}\",\"headers\":[{}]}}",
                                    path,
                                    method,
                                    headers
                                        .iter()
                                        .map(|h| format!("\"{}\"", h))
                                        .collect::<Vec<_>>()
                                        .join(",")
                                );

                                Ok::<_, hyper::Error>(Response::new(Full::new(bytes::Bytes::from(
                                    body,
                                ))))
                            });

                        let _ = http1::Builder::new().serve_connection(io, service).await;
                    });
                }
            }
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        Self {
            port: addr.port(),
            addr,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }
}

#[allow(dead_code)]
pub struct MockHttpsServer {
    pub port: u16,
    pub addr: SocketAddr,
}

#[allow(dead_code)]
pub struct MockH2TlsServer {
    pub port: u16,
    pub addr: SocketAddr,
    host_header_seen: Arc<parking_lot::Mutex<Option<bool>>>,
    notify: Arc<tokio::sync::Notify>,
}

#[allow(dead_code)]
impl MockH2TlsServer {
    pub async fn start() -> Self {
        use http_body_util::Full;
        use hyper::server::conn::http2;
        use hyper::service::service_fn;
        use hyper::{Request, Response, StatusCode};
        use hyper_util::rt::{TokioExecutor, TokioIo};
        use tokio_rustls::rustls;
        use tokio_rustls::rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
        use tokio_rustls::TlsAcceptor;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Self-signed cert is enough because tests run proxy with unsafe_ssl=true.
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
        server_config.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(server_config));

        let host_header_seen: Arc<parking_lot::Mutex<Option<bool>>> =
            Arc::new(parking_lot::Mutex::new(None));
        let notify = Arc::new(tokio::sync::Notify::new());

        let host_header_seen_for_task = Arc::clone(&host_header_seen);
        let notify_for_task = Arc::clone(&notify);
        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let acceptor = acceptor.clone();
                let host_header_seen = Arc::clone(&host_header_seen_for_task);
                let notify = Arc::clone(&notify_for_task);

                tokio::spawn(async move {
                    let tls_stream = match acceptor.accept(stream).await {
                        Ok(s) => s,
                        Err(_) => return,
                    };

                    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                        let host_header_seen = Arc::clone(&host_header_seen);
                        let notify = Arc::clone(&notify);
                        async move {
                            let has_host = req.headers().get(hyper::header::HOST).is_some();
                            *host_header_seen.lock() = Some(has_host);
                            notify.notify_waiters();

                            let mut resp = Response::new(Full::new(Bytes::from_static(b"ok")));
                            *resp.status_mut() = StatusCode::OK;
                            Ok::<_, hyper::Error>(resp)
                        }
                    });

                    let io = TokioIo::new(tls_stream);
                    let _ = http2::Builder::new(TokioExecutor::new())
                        .max_header_list_size(256 * 1024)
                        .serve_connection(io, service)
                        .await;
                });
            }
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        Self {
            port: addr.port(),
            addr,
            host_header_seen,
            notify,
        }
    }

    pub async fn wait_host_header_seen(&self) -> Option<bool> {
        // If the request already arrived, return immediately.
        if self.host_header_seen.lock().is_some() {
            return *self.host_header_seen.lock();
        }
        let _ = tokio::time::timeout(tokio::time::Duration::from_secs(5), self.notify.notified())
            .await
            .ok();
        *self.host_header_seen.lock()
    }
}

#[allow(dead_code)]
impl MockHttpsServer {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        Self {
            port: addr.port(),
            addr,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("https://127.0.0.1:{}{}", self.port, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_start_test_proxy() {
        let proxy = start_test_proxy().await;
        assert!(proxy.port > 0);
        assert_eq!(proxy.host, "127.0.0.1");
    }

    #[tokio::test]
    async fn test_mock_http_server() {
        let server = MockHttpServer::start().await;
        assert!(server.port > 0);
        assert_eq!(
            server.url("/test"),
            format!("http://127.0.0.1:{}/test", server.port)
        );
    }

    #[test]
    fn test_create_proxy_client() {
        let config = ProxyConfig::default();
        let rules = Arc::new(TestRulesResolver::new());
        let server =
            ProxyServer::new(config.clone()).with_rules(rules.clone() as Arc<dyn RulesResolver>);
        let proxy = TestProxy {
            port: 8080,
            host: "127.0.0.1".to_string(),
            ca_cert_der: None,
            server,
            rules,
        };
        let _client = create_proxy_client(&proxy);
    }

    #[test]
    fn test_add_and_clear_rules() {
        let config = ProxyConfig::default();
        let rules = Arc::new(TestRulesResolver::new());
        let server =
            ProxyServer::new(config.clone()).with_rules(rules.clone() as Arc<dyn RulesResolver>);
        let proxy = TestProxy {
            port: 8080,
            host: "127.0.0.1".to_string(),
            ca_cert_der: None,
            server,
            rules,
        };

        add_test_rule(&proxy, "example.com", Protocol::Host, "127.0.0.1");
        let resolved = proxy.rules.resolve("http://example.com", "GET");
        assert_eq!(resolved.host, Some("127.0.0.1".to_string()));

        clear_test_rules(&proxy);
        let resolved = proxy.rules.resolve("http://example.com", "GET");
        assert_eq!(resolved.host, None);
    }
}
