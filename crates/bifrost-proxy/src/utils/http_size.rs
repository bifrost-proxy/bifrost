pub fn calculate_request_size(
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body_len: usize,
) -> usize {
    let mut size = 0;
    size += method.len();
    size += 1;
    size += uri.len();
    size += " HTTP/1.1\r\n".len();

    for (name, value) in headers {
        size += name.len();
        size += ": ".len();
        size += value.len();
        size += "\r\n".len();
    }

    size += "\r\n".len();
    size += body_len;

    size
}

pub fn calculate_response_size(
    status_code: u16,
    headers: &[(String, String)],
    body_len: usize,
) -> usize {
    calculate_response_headers_size(status_code, headers) + body_len
}

pub fn calculate_response_headers_size(status_code: u16, headers: &[(String, String)]) -> usize {
    let mut size = 0;
    size += "HTTP/1.1 ".len();
    size += format!("{}", status_code).len();
    size += " ".len();
    size += status_reason(status_code).len();
    size += "\r\n".len();

    for (name, value) in headers {
        size += name.len();
        size += ": ".len();
        size += value.len();
        size += "\r\n".len();
    }

    size += "\r\n".len();

    size
}

fn status_reason(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_request_size_matches_serialized_request() {
        let headers = vec![
            ("Host".to_string(), "example.com".to_string()),
            ("X-Test".to_string(), "abc".to_string()),
        ];
        let body = "12345";

        let calculated = calculate_request_size("GET", "/path", &headers, body.len());

        let mut serialized = String::new();
        serialized.push_str("GET /path HTTP/1.1\r\n");
        serialized.push_str("Host: example.com\r\n");
        serialized.push_str("X-Test: abc\r\n");
        serialized.push_str("\r\n");
        serialized.push_str(body);

        assert_eq!(calculated, serialized.len());
    }

    #[test]
    fn calculate_response_size_and_headers_size_are_consistent() {
        let headers = vec![
            ("Content-Type".to_string(), "text/plain".to_string()),
            ("X-Test".to_string(), "1".to_string()),
        ];
        let body = "hello";

        let headers_size = calculate_response_headers_size(200, &headers);
        let total_size = calculate_response_size(200, &headers, body.len());

        assert_eq!(total_size, headers_size + body.len());
    }

    #[test]
    fn status_reason_covers_known_and_unknown_codes() {
        assert_eq!(status_reason(200), "OK");
        assert_eq!(status_reason(404), "Not Found");
        assert_eq!(status_reason(101), "Switching Protocols");
        assert_eq!(status_reason(999), "Unknown");
    }

    #[test]
    fn coverage_90_status_reason_covers_every_supported_status() {
        for code in [
            100, 101, 200, 201, 202, 204, 206, 301, 302, 303, 304, 307, 308, 400, 401, 403, 404,
            405, 408, 409, 410, 413, 414, 415, 429, 500, 501, 502, 503, 504,
        ] {
            assert_ne!(status_reason(code), "Unknown", "status {code}");
            assert!(calculate_response_headers_size(code, &[]) > 12);
        }
    }
}
