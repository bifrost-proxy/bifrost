use bifrost_tls::{
    generate_root_ca, init_crypto_provider, DynamicCertGenerator, TlsConfig as BifrostTlsConfig,
};
use bytes::Bytes;
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use parking_lot::RwLock;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::net::UdpSocket;
use tokio_rustls::TlsAcceptor;

#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub timestamp: Instant,
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
}

#[derive(Clone)]
struct MockResponse {
    status: u16,
    body: String,
    headers: HashMap<String, String>,
}

impl Default for MockResponse {
    fn default() -> Self {
        Self {
            status: 200,
            body: "ok".to_string(),
            headers: HashMap::new(),
        }
    }
}

pub struct EnhancedMockServer {
    pub port: u16,
    pub addr: SocketAddr,
    requests: Arc<RwLock<Vec<RecordedRequest>>>,
    response: Arc<RwLock<MockResponse>>,
}

impl EnhancedMockServer {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(RwLock::new(Vec::new()));
        let response = Arc::new(RwLock::new(MockResponse::default()));

        let requests_clone = Arc::clone(&requests);
        let response_clone = Arc::clone(&response);

        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let io = TokioIo::new(stream);
                    let requests = Arc::clone(&requests_clone);
                    let response = Arc::clone(&response_clone);

                    tokio::spawn(async move {
                        let service = service_fn(|req: Request<hyper::body::Incoming>| {
                            let requests = Arc::clone(&requests);
                            let response = Arc::clone(&response);

                            async move {
                                let method = req.method().to_string();
                                let path = req.uri().path().to_string();
                                let query = req.uri().query().map(|s| s.to_string());

                                let headers: HashMap<String, String> = req
                                    .headers()
                                    .iter()
                                    .map(|(k, v)| {
                                        (k.to_string(), v.to_str().unwrap_or("").to_string())
                                    })
                                    .collect();

                                let body_bytes =
                                    match http_body_util::BodyExt::collect(req.into_body()).await {
                                        Ok(collected) => collected.to_bytes(),
                                        Err(_) => Bytes::new(),
                                    };
                                let body = if body_bytes.is_empty() {
                                    None
                                } else {
                                    Some(String::from_utf8_lossy(&body_bytes).to_string())
                                };

                                let recorded = RecordedRequest {
                                    timestamp: Instant::now(),
                                    method: method.clone(),
                                    path: path.clone(),
                                    query,
                                    headers: headers.clone(),
                                    body,
                                };
                                requests.write().push(recorded);

                                let resp = response.read();
                                let status =
                                    StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK);

                                let mut response_headers = resp.headers.clone();
                                response_headers
                                    .entry("Content-Type".to_string())
                                    .or_insert_with(|| "application/json".to_string());

                                let response_body = serde_json::json!({
                                    "status": "ok",
                                    "mock_response": resp.body,
                                    "received": {
                                        "method": method,
                                        "path": path,
                                        "headers": headers
                                    }
                                });

                                let mut builder = Response::builder().status(status);
                                for (k, v) in &response_headers {
                                    builder = builder.header(k.as_str(), v.as_str());
                                }

                                Ok::<_, hyper::Error>(
                                    builder
                                        .body(Full::new(Bytes::from(response_body.to_string())))
                                        .unwrap(),
                                )
                            }
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
            requests,
            response,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    pub fn get_requests(&self) -> Vec<RecordedRequest> {
        self.requests.read().clone()
    }

    pub fn last_request(&self) -> Option<RecordedRequest> {
        self.requests.read().last().cloned()
    }

    pub fn clear_requests(&self) {
        self.requests.write().clear();
    }

    pub fn request_count(&self) -> usize {
        self.requests.read().len()
    }

    pub fn set_response(&self, status: u16, body: &str) {
        let mut resp = self.response.write();
        resp.status = status;
        resp.body = body.to_string();
    }

    pub fn set_response_with_headers(
        &self,
        status: u16,
        body: &str,
        headers: HashMap<String, String>,
    ) {
        let mut resp = self.response.write();
        resp.status = status;
        resp.body = body.to_string();
        resp.headers = headers;
    }

    pub fn assert_request_received(&self) -> Result<RecordedRequest, String> {
        self.last_request()
            .ok_or_else(|| "No request received by mock server".to_string())
    }

    pub fn assert_header_received(&self, header: &str, expected: &str) -> Result<(), String> {
        let request = self.assert_request_received()?;
        let header_lower = header.to_lowercase();

        for (k, v) in &request.headers {
            if k.to_lowercase() == header_lower {
                if v == expected {
                    return Ok(());
                } else {
                    return Err(format!(
                        "Header '{}' expected '{}', got '{}'",
                        header, expected, v
                    ));
                }
            }
        }

        Err(format!(
            "Header '{}' not found in request. Available headers: {:?}",
            header,
            request.headers.keys().collect::<Vec<_>>()
        ))
    }

    pub fn assert_header_contains(&self, header: &str, substring: &str) -> Result<(), String> {
        let request = self.assert_request_received()?;
        let header_lower = header.to_lowercase();

        for (k, v) in &request.headers {
            if k.to_lowercase() == header_lower {
                if v.contains(substring) {
                    return Ok(());
                } else {
                    return Err(format!(
                        "Header '{}' does not contain '{}', value: '{}'",
                        header, substring, v
                    ));
                }
            }
        }

        Err(format!("Header '{}' not found in request", header))
    }

    pub fn assert_path(&self, expected: &str) -> Result<(), String> {
        let request = self.assert_request_received()?;
        if request.path == expected {
            Ok(())
        } else {
            Err(format!(
                "Path expected '{}', got '{}'",
                expected, request.path
            ))
        }
    }

    pub fn assert_method(&self, expected: &str) -> Result<(), String> {
        let request = self.assert_request_received()?;
        if request.method == expected {
            Ok(())
        } else {
            Err(format!(
                "Method expected '{}', got '{}'",
                expected, request.method
            ))
        }
    }
}

pub struct HttpsMockServer {
    pub port: u16,
    pub addr: SocketAddr,
    requests: Arc<RwLock<Vec<RecordedRequest>>>,
    response: Arc<RwLock<MockResponse>>,
}

pub struct HttpbinMockServer {
    pub http_port: u16,
    pub https_port: u16,
}

impl HttpbinMockServer {
    pub async fn start() -> Self {
        let http_port = spawn_httpbin_http_server().await;
        let https_port = spawn_httpbin_https_server("httpbin.org").await;

        Self {
            http_port,
            https_port,
        }
    }

    pub fn http_rules(&self) -> Vec<String> {
        vec![
            format!("http://httpbin.org/ http://127.0.0.1:{}", self.http_port),
            format!(
                "https://httpbin.org/ tlsIntercept:// https://127.0.0.1:{}",
                self.https_port
            ),
        ]
    }
}

impl HttpsMockServer {
    pub async fn start(domain: &str) -> Self {
        init_crypto_provider();
        let ca = Arc::new(generate_root_ca().expect("failed to generate test CA"));
        let cert_generator = DynamicCertGenerator::new(ca);
        let cert = cert_generator
            .generate_for_domain(domain)
            .expect("failed to generate test server certificate");
        let server_config = BifrostTlsConfig::build_server_config(&cert)
            .expect("failed to build HTTPS mock server config");

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(RwLock::new(Vec::new()));
        let response = Arc::new(RwLock::new(MockResponse::default()));
        let acceptor = TlsAcceptor::from(server_config);

        let requests_clone = Arc::clone(&requests);
        let response_clone = Arc::clone(&response);

        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let acceptor = acceptor.clone();
                    let requests = Arc::clone(&requests_clone);
                    let response = Arc::clone(&response_clone);

                    tokio::spawn(async move {
                        let Ok(stream) = acceptor.accept(stream).await else {
                            return;
                        };

                        let io = TokioIo::new(stream);
                        let service = service_fn(|req: Request<hyper::body::Incoming>| {
                            let requests = Arc::clone(&requests);
                            let response = Arc::clone(&response);

                            async move {
                                let method = req.method().to_string();
                                let path = req.uri().path().to_string();
                                let query = req.uri().query().map(|s| s.to_string());

                                let headers: HashMap<String, String> = req
                                    .headers()
                                    .iter()
                                    .map(|(k, v)| {
                                        (k.to_string(), v.to_str().unwrap_or("").to_string())
                                    })
                                    .collect();

                                let body_bytes =
                                    match http_body_util::BodyExt::collect(req.into_body()).await {
                                        Ok(collected) => collected.to_bytes(),
                                        Err(_) => Bytes::new(),
                                    };
                                let body = if body_bytes.is_empty() {
                                    None
                                } else {
                                    Some(String::from_utf8_lossy(&body_bytes).to_string())
                                };

                                requests.write().push(RecordedRequest {
                                    timestamp: Instant::now(),
                                    method: method.clone(),
                                    path: path.clone(),
                                    query,
                                    headers: headers.clone(),
                                    body,
                                });

                                let resp = response.read();
                                let status =
                                    StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK);

                                let mut response_headers = resp.headers.clone();
                                response_headers
                                    .entry("Content-Type".to_string())
                                    .or_insert_with(|| "application/json".to_string());

                                let response_body = serde_json::json!({
                                    "status": "ok",
                                    "mock_response": resp.body,
                                    "received": {
                                        "method": method,
                                        "path": path,
                                        "headers": headers
                                    }
                                });

                                let mut builder = Response::builder().status(status);
                                for (k, v) in &response_headers {
                                    builder = builder.header(k.as_str(), v.as_str());
                                }

                                Ok::<_, hyper::Error>(
                                    builder
                                        .body(Full::new(Bytes::from(response_body.to_string())))
                                        .unwrap(),
                                )
                            }
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
            requests,
            response,
        }
    }

    pub fn set_response(&self, status: u16, body: &str) {
        let mut resp = self.response.write();
        resp.status = status;
        resp.body = body.to_string();
    }

    pub fn request_count(&self) -> usize {
        self.requests.read().len()
    }

    pub fn assert_request_received(&self) -> Result<RecordedRequest, String> {
        self.requests
            .read()
            .last()
            .cloned()
            .ok_or_else(|| "No request received by HTTPS mock server".to_string())
    }
}

pub struct MockDnsServer {
    pub addr: SocketAddr,
    records: Arc<HashMap<String, IpAddr>>,
    queries: Arc<RwLock<Vec<String>>>,
}

impl MockDnsServer {
    pub async fn start(records: HashMap<String, IpAddr>) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let records = Arc::new(
            records
                .into_iter()
                .map(|(host, ip)| (host.to_ascii_lowercase(), ip))
                .collect::<HashMap<_, _>>(),
        );
        let queries = Arc::new(RwLock::new(Vec::new()));

        let records_clone = Arc::clone(&records);
        let queries_clone = Arc::clone(&queries);

        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            loop {
                let Ok((len, peer)) = socket.recv_from(&mut buf).await else {
                    break;
                };
                let packet = &buf[..len];
                let Some((host, qtype, question_end)) = parse_dns_question(packet) else {
                    continue;
                };

                queries_clone.write().push(host.clone());
                let ip = records_clone.get(&host).copied();
                let response = build_dns_response(packet, question_end, qtype, ip);
                let _ = socket.send_to(&response, peer).await;
            }
        });

        Self {
            addr,
            records,
            queries,
        }
    }

    pub fn server(&self) -> String {
        self.addr.to_string()
    }

    pub fn query_count_for(&self, host: &str) -> usize {
        let host = host.to_ascii_lowercase();
        self.queries
            .read()
            .iter()
            .filter(|q| q.as_str() == host)
            .count()
    }

    pub fn assert_query_received(&self, host: &str) -> Result<(), String> {
        let count = self.query_count_for(host);
        if count > 0 {
            Ok(())
        } else {
            Err(format!(
                "DNS query for '{}' not observed. Seen: {:?}",
                host,
                self.queries.read().clone()
            ))
        }
    }

    pub fn records_len(&self) -> usize {
        self.records.len()
    }
}

async fn spawn_httpbin_http_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            if let Ok((stream, _)) = listener.accept().await {
                let io = TokioIo::new(stream);
                tokio::spawn(async move {
                    let service = service_fn(handle_httpbin_request);
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        }
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    port
}

async fn spawn_httpbin_https_server(domain: &str) -> u16 {
    init_crypto_provider();
    let ca = Arc::new(generate_root_ca().expect("failed to generate test CA"));
    let cert_generator = DynamicCertGenerator::new(ca);
    let cert = cert_generator
        .generate_for_domain(domain)
        .expect("failed to generate httpbin mock certificate");
    let server_config = BifrostTlsConfig::build_server_config(&cert)
        .expect("failed to build httpbin mock TLS config");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = TlsAcceptor::from(server_config);

    tokio::spawn(async move {
        loop {
            if let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(stream) = acceptor.accept(stream).await else {
                        return;
                    };

                    let io = TokioIo::new(stream);
                    let service = service_fn(handle_httpbin_request);
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        }
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    port
}

async fn handle_httpbin_request(
    req: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let headers = collect_headers(req.headers());
    let host = headers
        .get("host")
        .cloned()
        .unwrap_or_else(|| "httpbin.org".to_string());
    let scheme = if headers
        .get("x-forwarded-proto")
        .map(|value| value.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
    {
        "https"
    } else {
        "http"
    };

    let body_bytes = match http_body_util::BodyExt::collect(req.into_body()).await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => Bytes::new(),
    };

    let response =
        build_httpbin_response(scheme, &host, &method, &path, &query, &headers, &body_bytes);

    Ok(response)
}

fn build_httpbin_response(
    scheme: &str,
    host: &str,
    method: &str,
    path: &str,
    query: &str,
    headers: &HashMap<String, String>,
    body: &[u8],
) -> Response<Full<Bytes>> {
    let full_url = if query.is_empty() {
        format!("{scheme}://{host}{path}")
    } else {
        format!("{scheme}://{host}{path}?{query}")
    };

    let query_json = parse_query_json(query);
    let body_text = String::from_utf8_lossy(body).to_string();

    let (status, extra_headers, payload): (u16, HashMap<String, String>, Value) = if path == "/ip" {
        (200, HashMap::new(), json!({ "origin": "127.0.0.1" }))
    } else if path == "/headers" {
        (
            200,
            HashMap::new(),
            json!({ "headers": normalize_httpbin_headers(headers) }),
        )
    } else if path == "/user-agent" {
        (
            200,
            HashMap::new(),
            json!({
                "user-agent": headers.get("user-agent").cloned().unwrap_or_default()
            }),
        )
    } else if path == "/cookies" {
        (
            200,
            HashMap::new(),
            json!({
                "cookies": parse_cookie_map(headers.get("cookie").map(String::as_str)),
            }),
        )
    } else if path.starts_with("/status/") {
        let status = path
            .trim_start_matches("/status/")
            .parse::<u16>()
            .unwrap_or(200);
        (status, HashMap::new(), json!({}))
    } else if path == "/get" || path == "/anything" {
        (
            200,
            HashMap::new(),
            json!({
                "args": query_json,
                "headers": normalize_httpbin_headers(headers),
                "origin": "127.0.0.1",
                "url": full_url,
            }),
        )
    } else if matches!(path, "/post" | "/put" | "/patch" | "/delete") {
        (
            200,
            HashMap::new(),
            json!({
                "args": query_json,
                "data": body_text,
                "headers": normalize_httpbin_headers(headers),
                "json": serde_json::from_slice::<Value>(body).ok(),
                "method": method,
                "origin": "127.0.0.1",
                "url": full_url,
            }),
        )
    } else {
        (
            200,
            HashMap::new(),
            json!({
                "args": query_json,
                "data": body_text,
                "headers": normalize_httpbin_headers(headers),
                "method": method,
                "origin": "127.0.0.1",
                "url": full_url,
            }),
        )
    };

    let mut builder = Response::builder().status(status);
    builder = builder.header("Content-Type", "application/json");
    for (key, value) in extra_headers {
        builder = builder.header(key.as_str(), value.as_str());
    }

    builder
        .body(Full::new(Bytes::from(payload.to_string())))
        .unwrap()
}

fn collect_headers(headers: &hyper::HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_str().unwrap_or("").to_string()))
        .collect()
}

fn normalize_httpbin_headers(headers: &HashMap<String, String>) -> Value {
    let mut map = Map::new();
    for (key, value) in headers {
        map.insert(canonical_header_name(key), Value::String(value.clone()));
    }
    Value::Object(map)
}

fn canonical_header_name(key: &str) -> String {
    key.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn parse_cookie_map(cookie_header: Option<&str>) -> Value {
    let mut map = Map::new();
    if let Some(cookie_header) = cookie_header {
        for part in cookie_header.split(';') {
            if let Some((key, value)) = part.trim().split_once('=') {
                map.insert(
                    key.trim().to_string(),
                    Value::String(value.trim().to_string()),
                );
            }
        }
    }
    Value::Object(map)
}

fn parse_query_json(query: &str) -> Value {
    let mut map = Map::new();
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        map.insert(key.into_owned(), Value::String(value.into_owned()));
    }
    Value::Object(map)
}

fn parse_dns_question(packet: &[u8]) -> Option<(String, u16, usize)> {
    if packet.len() < 17 {
        return None;
    }

    let qdcount = u16::from_be_bytes([packet[4], packet[5]]);
    if qdcount == 0 {
        return None;
    }

    let mut pos = 12usize;
    let mut labels = Vec::new();
    loop {
        let len = *packet.get(pos)? as usize;
        pos += 1;
        if len == 0 {
            break;
        }
        let label = packet.get(pos..pos + len)?;
        labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
        pos += len;
    }

    let qtype = u16::from_be_bytes([*packet.get(pos)?, *packet.get(pos + 1)?]);
    pos += 4; // qtype + qclass

    Some((labels.join("."), qtype, pos))
}

fn build_dns_response(
    packet: &[u8],
    question_end: usize,
    qtype: u16,
    ip: Option<IpAddr>,
) -> Vec<u8> {
    let question = &packet[12..question_end];
    let ipv4 = match ip {
        Some(IpAddr::V4(ipv4)) if qtype == 1 => Some(ipv4),
        _ => None,
    };

    let mut response = Vec::with_capacity(question_end + 32);
    response.extend_from_slice(&packet[0..2]); // transaction id
    response.extend_from_slice(&[0x81, 0x80]); // standard response, recursion available
    response.extend_from_slice(&packet[4..6]); // qdcount
    response.extend_from_slice(&(if ipv4.is_some() { 1u16 } else { 0u16 }).to_be_bytes());
    response.extend_from_slice(&0u16.to_be_bytes()); // nscount
    response.extend_from_slice(&0u16.to_be_bytes()); // arcount
    response.extend_from_slice(question);

    if let Some(ipv4) = ipv4 {
        response.extend_from_slice(&[0xC0, 0x0C]); // pointer to qname
        response.extend_from_slice(&1u16.to_be_bytes()); // A
        response.extend_from_slice(&1u16.to_be_bytes()); // IN
        response.extend_from_slice(&60u32.to_be_bytes()); // TTL
        response.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
        response.extend_from_slice(&ipv4.octets());
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_server_basic() {
        let server = EnhancedMockServer::start().await;
        assert!(server.port > 0);

        let client = reqwest::Client::new();
        let resp = client
            .get(server.url("/test"))
            .header("X-Custom", "value123")
            .send()
            .await
            .unwrap();

        assert!(resp.status().is_success());

        let request = server.last_request().unwrap();
        assert_eq!(request.path, "/test");
        assert_eq!(request.method, "GET");
        assert!(request.headers.contains_key("x-custom"));
    }

    #[tokio::test]
    async fn test_mock_server_custom_response() {
        let server = EnhancedMockServer::start().await;
        server.set_response(201, "created");

        let client = reqwest::Client::new();
        let resp = client.post(server.url("/create")).send().await.unwrap();

        assert_eq!(resp.status().as_u16(), 201);
    }

    #[tokio::test]
    async fn test_mock_server_assertions() {
        let server = EnhancedMockServer::start().await;

        let client = reqwest::Client::new();
        client
            .post(server.url("/api/users"))
            .header("Authorization", "Bearer token123")
            .send()
            .await
            .unwrap();

        server.assert_path("/api/users").unwrap();
        server.assert_method("POST").unwrap();
        server
            .assert_header_received("authorization", "Bearer token123")
            .unwrap();
    }
}
