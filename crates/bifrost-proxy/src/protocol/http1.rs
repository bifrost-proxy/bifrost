use std::collections::HashMap;

use bytes::{Bytes, BytesMut};

use super::{ContentType, ProtocolDetector};

#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: String,
    pub uri: String,
    pub version: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Bytes>,
}

impl HttpRequest {
    pub fn new(method: &str, uri: &str) -> Self {
        Self {
            method: method.to_string(),
            uri: uri.to_string(),
            version: "HTTP/1.1".to_string(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn with_header(mut self, key: &str, value: &str) -> Self {
        self.headers.push((key.to_string(), value.to_string()));
        self
    }

    pub fn with_body(mut self, body: impl Into<Bytes>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn host(&self) -> Option<&str> {
        self.header("host")
    }

    pub fn content_type(&self) -> Option<ContentType> {
        self.header("content-type").map(ContentType::from_header)
    }

    pub fn content_length(&self) -> Option<usize> {
        self.header("content-length").and_then(|v| v.parse().ok())
    }

    pub fn is_chunked(&self) -> bool {
        self.header("transfer-encoding")
            .map(|v| v.to_lowercase().contains("chunked"))
            .unwrap_or(false)
    }

    pub fn is_websocket_upgrade(&self) -> bool {
        ProtocolDetector::is_websocket_upgrade(&self.headers)
    }

    pub fn is_sse_request(&self) -> bool {
        ProtocolDetector::is_sse_request(&self.headers)
    }

    pub fn is_connect(&self) -> bool {
        self.method.eq_ignore_ascii_case("CONNECT")
    }

    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::new();

        buf.extend_from_slice(self.method.as_bytes());
        buf.extend_from_slice(b" ");
        buf.extend_from_slice(self.uri.as_bytes());
        buf.extend_from_slice(b" ");
        buf.extend_from_slice(self.version.as_bytes());
        buf.extend_from_slice(b"\r\n");

        for (key, value) in &self.headers {
            buf.extend_from_slice(key.as_bytes());
            buf.extend_from_slice(b": ");
            buf.extend_from_slice(value.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }

        if let Some(ref body) = self.body {
            if !self
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            {
                buf.extend_from_slice(b"Content-Length: ");
                buf.extend_from_slice(body.len().to_string().as_bytes());
                buf.extend_from_slice(b"\r\n");
            }
        }

        buf.extend_from_slice(b"\r\n");

        if let Some(ref body) = self.body {
            buf.extend_from_slice(body);
        }

        buf.freeze()
    }

    pub fn parse(data: &[u8]) -> Option<(Self, usize)> {
        let header_end = find_header_end(data)?;
        let header_str = std::str::from_utf8(&data[..header_end]).ok()?;

        let mut lines = header_str.lines();
        let first_line = lines.next()?;
        let parts: Vec<&str> = first_line.split_whitespace().collect();

        if parts.len() < 3 {
            return None;
        }

        let method = parts[0].to_string();
        let uri = parts[1].to_string();
        let version = parts[2].to_string();

        let mut headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim().to_string();
                let value = line[colon_pos + 1..].trim().to_string();
                headers.push((key, value));
            }
        }

        let header_total = header_end + 4;
        let mut request = HttpRequest {
            method,
            uri,
            version,
            headers,
            body: None,
        };

        let content_length = request.content_length();
        if let Some(len) = content_length {
            if len > 0 {
                if data.len() >= header_total + len {
                    request.body = Some(Bytes::copy_from_slice(
                        &data[header_total..header_total + len],
                    ));
                    return Some((request, header_total + len));
                } else {
                    return None;
                }
            }
        }

        Some((request, header_total))
    }
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub version: String,
    pub status_code: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Bytes>,
}

impl HttpResponse {
    pub fn new(status_code: u16, status_text: &str) -> Self {
        Self {
            version: "HTTP/1.1".to_string(),
            status_code,
            status_text: status_text.to_string(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn ok() -> Self {
        Self::new(200, "OK")
    }

    pub fn bad_request() -> Self {
        Self::new(400, "Bad Request")
    }

    pub fn not_found() -> Self {
        Self::new(404, "Not Found")
    }

    pub fn internal_error() -> Self {
        Self::new(500, "Internal Server Error")
    }

    pub fn bad_gateway() -> Self {
        Self::new(502, "Bad Gateway")
    }

    pub fn switching_protocols() -> Self {
        Self::new(101, "Switching Protocols")
    }

    pub fn connection_established() -> Self {
        Self::new(200, "Connection Established")
    }

    pub fn with_header(mut self, key: &str, value: &str) -> Self {
        self.headers.push((key.to_string(), value.to_string()));
        self
    }

    pub fn with_body(mut self, body: impl Into<Bytes>) -> Self {
        let body = body.into();
        self.headers
            .push(("Content-Length".to_string(), body.len().to_string()));
        self.body = Some(body);
        self
    }

    pub fn with_json_body(self, body: &str) -> Self {
        self.with_header("Content-Type", "application/json")
            .with_body(Bytes::copy_from_slice(body.as_bytes()))
    }

    pub fn with_text_body(self, body: &str) -> Self {
        self.with_header("Content-Type", "text/plain")
            .with_body(Bytes::copy_from_slice(body.as_bytes()))
    }

    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn content_type(&self) -> Option<ContentType> {
        self.header("content-type").map(ContentType::from_header)
    }

    pub fn content_length(&self) -> Option<usize> {
        self.header("content-length").and_then(|v| v.parse().ok())
    }

    pub fn is_chunked(&self) -> bool {
        self.header("transfer-encoding")
            .map(|v| v.to_lowercase().contains("chunked"))
            .unwrap_or(false)
    }

    pub fn is_sse_response(&self) -> bool {
        ProtocolDetector::is_sse_response(&self.headers)
    }

    pub fn is_upgrade(&self) -> bool {
        self.status_code == 101
    }

    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::new();

        buf.extend_from_slice(self.version.as_bytes());
        buf.extend_from_slice(b" ");
        buf.extend_from_slice(self.status_code.to_string().as_bytes());
        buf.extend_from_slice(b" ");
        buf.extend_from_slice(self.status_text.as_bytes());
        buf.extend_from_slice(b"\r\n");

        for (key, value) in &self.headers {
            buf.extend_from_slice(key.as_bytes());
            buf.extend_from_slice(b": ");
            buf.extend_from_slice(value.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }

        buf.extend_from_slice(b"\r\n");

        if let Some(ref body) = self.body {
            buf.extend_from_slice(body);
        }

        buf.freeze()
    }

    pub fn parse(data: &[u8]) -> Option<(Self, usize)> {
        let header_end = find_header_end(data)?;
        let header_str = std::str::from_utf8(&data[..header_end]).ok()?;

        let mut lines = header_str.lines();
        let first_line = lines.next()?;
        let parts: Vec<&str> = first_line.splitn(3, ' ').collect();

        if parts.len() < 2 {
            return None;
        }

        let version = parts[0].to_string();
        let status_code: u16 = parts[1].parse().ok()?;
        let status_text = parts.get(2).unwrap_or(&"").to_string();

        let mut headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim().to_string();
                let value = line[colon_pos + 1..].trim().to_string();
                headers.push((key, value));
            }
        }

        let header_total = header_end + 4;
        let mut response = HttpResponse {
            version,
            status_code,
            status_text,
            headers,
            body: None,
        };

        let content_length = response.content_length();
        if let Some(len) = content_length {
            if len > 0 {
                if data.len() >= header_total + len {
                    response.body = Some(Bytes::copy_from_slice(
                        &data[header_total..header_total + len],
                    ));
                    return Some((response, header_total + len));
                } else {
                    return None;
                }
            }
        }

        Some((response, header_total))
    }
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    (0..data.len().saturating_sub(3)).find(|&i| &data[i..i + 4] == b"\r\n\r\n")
}

pub fn parse_host_port(uri: &str) -> Option<(String, u16)> {
    let uri = if uri.contains("://") {
        uri.split("://").nth(1)?
    } else {
        uri
    };

    let host_port = uri.split('/').next()?;

    if let Some(colon_pos) = host_port.rfind(':') {
        let host = host_port[..colon_pos].to_string();
        let port: u16 = host_port[colon_pos + 1..].parse().ok()?;
        Some((host, port))
    } else {
        Some((host_port.to_string(), 80))
    }
}

pub fn build_absolute_uri(host: &str, port: u16, path: &str, is_https: bool) -> String {
    let scheme = if is_https { "https" } else { "http" };
    let default_port = if is_https { 443 } else { 80 };

    if port == default_port {
        format!("{}://{}{}", scheme, host, path)
    } else {
        format!("{}://{}:{}{}", scheme, host, port, path)
    }
}

pub fn headers_to_map(headers: &[(String, String)]) -> HashMap<String, String> {
    headers
        .iter()
        .map(|(k, v)| (k.to_lowercase(), v.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_request_new() {
        let req = HttpRequest::new("GET", "/path")
            .with_header("Host", "example.com")
            .with_header("Accept", "*/*");

        assert_eq!(req.method, "GET");
        assert_eq!(req.uri, "/path");
        assert_eq!(req.header("Host"), Some("example.com"));
        assert_eq!(req.header("Accept"), Some("*/*"));
    }

    #[test]
    fn test_http_request_encode() {
        let req = HttpRequest::new("GET", "/").with_header("Host", "example.com");

        let encoded = req.encode();
        let expected = "GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
        assert_eq!(String::from_utf8_lossy(&encoded), expected);
    }

    #[test]
    fn test_http_request_encode_with_body() {
        let req = HttpRequest::new("POST", "/api")
            .with_header("Host", "example.com")
            .with_body(Bytes::from("hello"));

        let encoded = req.encode();
        assert!(String::from_utf8_lossy(&encoded).contains("Content-Length: 5"));
        assert!(String::from_utf8_lossy(&encoded).ends_with("hello"));
    }

    #[test]
    fn test_http_request_parse() {
        let data = b"GET /path HTTP/1.1\r\nHost: example.com\r\nAccept: */*\r\n\r\n";
        let (req, consumed) = HttpRequest::parse(data).unwrap();

        assert_eq!(req.method, "GET");
        assert_eq!(req.uri, "/path");
        assert_eq!(req.version, "HTTP/1.1");
        assert_eq!(req.header("Host"), Some("example.com"));
        assert_eq!(req.header("Accept"), Some("*/*"));
        assert_eq!(consumed, data.len());
    }

    #[test]
    fn test_http_request_is_connect() {
        let req = HttpRequest::new("CONNECT", "example.com:443");
        assert!(req.is_connect());

        let req = HttpRequest::new("GET", "/");
        assert!(!req.is_connect());
    }

    #[test]
    fn test_http_request_is_websocket() {
        let req = HttpRequest::new("GET", "/ws")
            .with_header("Connection", "Upgrade")
            .with_header("Upgrade", "websocket");
        assert!(req.is_websocket_upgrade());

        let req = HttpRequest::new("GET", "/");
        assert!(!req.is_websocket_upgrade());
    }

    #[test]
    fn test_http_request_is_sse() {
        let req = HttpRequest::new("GET", "/events").with_header("Accept", "text/event-stream");
        assert!(req.is_sse_request());

        let req = HttpRequest::new("GET", "/");
        assert!(!req.is_sse_request());
    }

    #[test]
    fn test_http_response_new() {
        let resp = HttpResponse::ok()
            .with_header("Content-Type", "text/plain")
            .with_body(Bytes::from("Hello"));

        assert_eq!(resp.status_code, 200);
        assert_eq!(resp.status_text, "OK");
        assert_eq!(resp.header("Content-Type"), Some("text/plain"));
    }

    #[test]
    fn test_http_response_encode() {
        let resp = HttpResponse::ok()
            .with_header("Content-Type", "text/plain")
            .with_body(Bytes::from("Hello"));

        let encoded = resp.encode();
        assert!(String::from_utf8_lossy(&encoded).contains("HTTP/1.1 200 OK"));
        assert!(String::from_utf8_lossy(&encoded).contains("Content-Type: text/plain"));
        assert!(String::from_utf8_lossy(&encoded).contains("Content-Length: 5"));
        assert!(String::from_utf8_lossy(&encoded).ends_with("Hello"));
    }

    #[test]
    fn test_http_response_parse() {
        let data = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nHello";
        let (resp, consumed) = HttpResponse::parse(data).unwrap();

        assert_eq!(resp.version, "HTTP/1.1");
        assert_eq!(resp.status_code, 200);
        assert_eq!(resp.status_text, "OK");
        assert_eq!(resp.header("Content-Type"), Some("text/plain"));
        assert_eq!(resp.body, Some(Bytes::from("Hello")));
        assert_eq!(consumed, data.len());
    }

    #[test]
    fn test_http_response_is_upgrade() {
        let resp = HttpResponse::switching_protocols();
        assert!(resp.is_upgrade());

        let resp = HttpResponse::ok();
        assert!(!resp.is_upgrade());
    }

    #[test]
    fn coverage_90_request_and_response_convenience_paths() {
        let request = HttpRequest::new("POST", "/json")
            .with_header("Host", "example.test")
            .with_header("Content-Type", "application/json; charset=utf-8")
            .with_header("Transfer-Encoding", "gzip, Chunked")
            .with_header("Content-Length", "4");
        assert_eq!(request.host(), Some("example.test"));
        assert!(matches!(request.content_type(), Some(ContentType::Json)));
        assert_eq!(request.content_length(), Some(4));
        assert!(request.is_chunked());
        assert!(!HttpRequest::new("GET", "/").is_chunked());

        let raw = b"POST /body HTTP/1.1\r\nHost: example.test\r\nContent-Length: 4\r\n\r\ndata";
        let (parsed, consumed) = HttpRequest::parse(raw).unwrap();
        assert_eq!(parsed.body, Some(Bytes::from_static(b"data")));
        assert_eq!(consumed, raw.len());
        assert!(HttpRequest::parse(&raw[..raw.len() - 1]).is_none());

        for response in [
            HttpResponse::bad_request(),
            HttpResponse::not_found(),
            HttpResponse::internal_error(),
            HttpResponse::bad_gateway(),
            HttpResponse::connection_established(),
        ] {
            assert!(response.status_code >= 200);
        }
        let json = HttpResponse::ok().with_json_body("{\"ok\":true}");
        assert!(matches!(json.content_type(), Some(ContentType::Json)));
        assert_eq!(json.content_length(), Some(11));
        let text = HttpResponse::ok().with_text_body("hello");
        assert!(matches!(text.content_type(), Some(ContentType::PlainText)));
        let chunked = HttpResponse::ok().with_header("Transfer-Encoding", "chunked");
        assert!(chunked.is_chunked());
        assert!(!HttpResponse::ok().is_chunked());
    }

    #[test]
    fn test_parse_host_port() {
        assert_eq!(
            parse_host_port("example.com:8080"),
            Some(("example.com".to_string(), 8080))
        );
        assert_eq!(
            parse_host_port("example.com"),
            Some(("example.com".to_string(), 80))
        );
        assert_eq!(
            parse_host_port("http://example.com:8080/path"),
            Some(("example.com".to_string(), 8080))
        );
        assert_eq!(
            parse_host_port("https://example.com/path"),
            Some(("example.com".to_string(), 80))
        );
    }

    #[test]
    fn test_build_absolute_uri() {
        assert_eq!(
            build_absolute_uri("example.com", 80, "/path", false),
            "http://example.com/path"
        );
        assert_eq!(
            build_absolute_uri("example.com", 8080, "/path", false),
            "http://example.com:8080/path"
        );
        assert_eq!(
            build_absolute_uri("example.com", 443, "/path", true),
            "https://example.com/path"
        );
        assert_eq!(
            build_absolute_uri("example.com", 8443, "/path", true),
            "https://example.com:8443/path"
        );
    }

    #[test]
    fn test_headers_to_map() {
        let headers = vec![
            ("Content-Type".to_string(), "text/plain".to_string()),
            ("Accept".to_string(), "*/*".to_string()),
        ];
        let map = headers_to_map(&headers);

        assert_eq!(map.get("content-type"), Some(&"text/plain".to_string()));
        assert_eq!(map.get("accept"), Some(&"*/*".to_string()));
    }
}
