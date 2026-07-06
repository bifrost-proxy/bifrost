use hyper::header::HeaderValue;

pub fn is_allowed_origin(origin: &str) -> bool {
    let origin_lower = origin.to_ascii_lowercase();

    let host = origin_lower
        .strip_prefix("http://")
        .or_else(|| origin_lower.strip_prefix("https://"))
        .or_else(|| origin_lower.strip_prefix("tauri://"))
        .unwrap_or(&origin_lower);

    let host_without_port = if let Some(bracket_end) = host.find(']') {
        let after_bracket = &host[bracket_end + 1..];
        if let Some(rest) = after_bracket.strip_prefix(':') {
            if rest.chars().all(|c| c.is_ascii_digit()) {
                &host[..bracket_end + 1]
            } else {
                host
            }
        } else {
            host
        }
    } else if let Some(colon_pos) = host.rfind(':') {
        let after_colon = &host[colon_pos + 1..];
        if after_colon.chars().all(|c| c.is_ascii_digit()) {
            &host[..colon_pos]
        } else {
            host
        }
    } else {
        host
    };

    matches!(
        host_without_port,
        "localhost" | "127.0.0.1" | "[::1]" | "0.0.0.0" | "tauri.localhost" | "bifrost.local"
    )
}

pub fn allowed_origin_header_value(origin: &str) -> Option<HeaderValue> {
    if is_allowed_origin(origin) {
        HeaderValue::from_str(origin).ok()
    } else {
        None
    }
}

/// Returns `true` if a `Host` header value refers to a recognized local host.
///
/// This is the anti-DNS-rebinding check for the loopback auth bypass: a
/// browser tricked by DNS rebinding connects to `127.0.0.1` (so the peer looks
/// like loopback) but still sends the attacker's domain in the `Host` header.
/// By requiring the `Host` to be a known-local name we reject rebinding while
/// keeping the legitimate desktop UI (which always sends a local host) working.
pub fn is_allowed_host(host_header: &str) -> bool {
    let host_lower = host_header.trim().to_ascii_lowercase();

    // Strip an optional `:port` suffix, taking IPv6 brackets into account.
    let host_without_port = if let Some(bracket_end) = host_lower.find(']') {
        &host_lower[..bracket_end + 1]
    } else if host_lower.matches(':').count() == 1 {
        // Exactly one colon: a `host:port` pair (bracket-less IPv6 such as
        // `::1` has multiple colons and must NOT be treated as `host:port`,
        // otherwise the `:1` is wrongly stripped as a port leaving `::`).
        if let Some(colon_pos) = host_lower.rfind(':') {
            let after_colon = &host_lower[colon_pos + 1..];
            if !after_colon.is_empty() && after_colon.chars().all(|c| c.is_ascii_digit()) {
                &host_lower[..colon_pos]
            } else {
                host_lower.as_str()
            }
        } else {
            host_lower.as_str()
        }
    } else {
        host_lower.as_str()
    };

    matches!(
        host_without_port,
        "localhost"
            | "127.0.0.1"
            | "[::1]"
            | "::1"
            | "0.0.0.0"
            | "tauri.localhost"
            | "bifrost.local"
    )
}

pub fn apply_cors_headers(
    resp: &mut hyper::Response<super::handlers::BoxBody>,
    origin: Option<&str>,
) {
    let headers = resp.headers_mut();
    headers.remove("Access-Control-Allow-Origin");

    if let Some(origin) = origin {
        if let Some(value) = allowed_origin_header_value(origin) {
            headers.insert("Access-Control-Allow-Origin", value);
            headers.insert("Vary", HeaderValue::from_static("Origin"));
        }
    }
}

/// Returns `true` when an `Origin` header refers to the same host as the
/// request's `Host` header. Used to accept the admin UI when it is served from
/// a non-loopback bind address (e.g. a LAN IP with remote access enabled),
/// where the origin is not in the static loopback allowlist but still matches
/// the host the browser connected to.
pub fn origin_matches_host(origin: &str, host: &str) -> bool {
    if host.trim().is_empty() {
        return false;
    }
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    let Some(origin_host) = url.host_str() else {
        return false;
    };
    let origin_port = url.port_or_known_default();
    let host_lower = host.trim().to_ascii_lowercase();
    let origin_host_port = match origin_port {
        Some(port) => format!("{origin_host}:{port}").to_ascii_lowercase(),
        None => origin_host.to_ascii_lowercase(),
    };
    let origin_host_lower = origin_host.to_ascii_lowercase();

    host_lower == origin_host_port || host_lower == origin_host_lower
}

pub fn is_allowed_admin_origin_for_host(origin: &str, host: &str) -> bool {
    is_allowed_origin(origin) || origin_matches_host(origin, host)
}

/// CSRF-equivalent guard for WebSocket upgrade requests.
///
/// WebSocket upgrades are `GET` requests, so they bypass the
/// `check_browser_write_guard` CSRF protection (which only runs for unsafe
/// methods) and they cannot carry the `X-Bifrost-CSRF` header. Without this
/// guard a malicious web page could open an authenticated socket to the admin
/// server — a Cross-Site WebSocket Hijacking (CSWSH) attack.
///
/// This mirrors the origin rules of `check_browser_write_guard`: when the
/// request carries any browser-controlled context (`Origin` / `Referer` /
/// `Sec-Fetch-*`) we reject untrusted cross-site and cross-origin upgrades.
/// Trusted desktop/local origins may be reported as `cross-site` by WebView
/// fetch metadata when they call the loopback admin backend. Native clients
/// (desktop app, CLI, mobile SDK) that do not send browser context headers are
/// gated by the auth layer instead, so they are left untouched.
///
/// Returns `None` when the upgrade is allowed, or `Some(reason)` describing why
/// it was rejected (suitable for structured logging).
pub fn websocket_origin_rejection(headers: &hyper::HeaderMap) -> Option<&'static str> {
    let header_value = |name: &str| -> Option<String> {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };

    let has_browser_context = header_value("origin").is_some()
        || header_value("referer").is_some()
        || header_value("sec-fetch-site").is_some()
        || header_value("sec-fetch-mode").is_some()
        || header_value("sec-fetch-dest").is_some();
    if !has_browser_context {
        return None;
    }

    if matches!(
        header_value("sec-fetch-site").as_deref(),
        Some("cross-site")
    ) && !header_value("origin")
        .as_deref()
        .map(|origin| {
            is_allowed_admin_origin_for_host(origin, &header_value("host").unwrap_or_default())
        })
        .unwrap_or(false)
    {
        return Some("cross_site");
    }

    if let Some(origin) = header_value("origin") {
        let host = header_value("host").unwrap_or_default();
        if !is_allowed_admin_origin_for_host(&origin, &host) {
            return Some("cross_origin");
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_localhost_origins() {
        assert!(is_allowed_origin("http://localhost"));
        assert!(is_allowed_origin("http://localhost:8800"));
        assert!(is_allowed_origin("http://localhost:3000"));
        assert!(is_allowed_origin("https://localhost:8800"));
        assert!(is_allowed_origin("http://127.0.0.1"));
        assert!(is_allowed_origin("http://127.0.0.1:8800"));
        assert!(is_allowed_origin("http://127.0.0.1:9900"));
        assert!(is_allowed_origin("https://127.0.0.1:8800"));
        assert!(is_allowed_origin("http://[::1]"));
        assert!(is_allowed_origin("http://[::1]:8800"));
        assert!(is_allowed_origin("http://0.0.0.0:8800"));
        assert!(is_allowed_origin("https://tauri.localhost"));
        assert!(is_allowed_origin("http://bifrost.local"));
        assert!(is_allowed_origin("http://bifrost.local:8800"));
        assert!(is_allowed_origin("tauri://localhost"));
    }

    #[test]
    fn blocks_external_origins() {
        assert!(!is_allowed_origin("http://evil.com"));
        assert!(!is_allowed_origin("https://attacker.example.com"));
        assert!(!is_allowed_origin("http://192.168.1.100:8800"));
        assert!(!is_allowed_origin("http://10.0.0.1:8800"));
        assert!(!is_allowed_origin("http://localhost.evil.com"));
        assert!(!is_allowed_origin("http://my-server.com"));
    }

    #[test]
    fn allowed_host_accepts_local_names() {
        assert!(is_allowed_host("localhost"));
        assert!(is_allowed_host("localhost:9900"));
        assert!(is_allowed_host("127.0.0.1"));
        assert!(is_allowed_host("127.0.0.1:8800"));
        assert!(is_allowed_host("[::1]"));
        assert!(is_allowed_host("[::1]:8800"));
        assert!(is_allowed_host("::1"));
        assert!(is_allowed_host("0.0.0.0:8800"));
        assert!(is_allowed_host("tauri.localhost"));
        assert!(is_allowed_host("bifrost.local"));
        assert!(is_allowed_host("BIFROST.LOCAL"));
    }

    #[test]
    fn allowed_host_rejects_rebinding_domains() {
        // DNS-rebinding: peer is loopback but Host is attacker-controlled.
        assert!(!is_allowed_host("evil.com"));
        assert!(!is_allowed_host("attacker.example.com:9900"));
        assert!(!is_allowed_host("localhost.evil.com"));
        assert!(!is_allowed_host("127.0.0.1.evil.com"));
        assert!(!is_allowed_host("192.168.1.100"));
        assert!(!is_allowed_host(""));
    }

    #[test]
    fn no_origin_returns_none() {
        assert!(allowed_origin_header_value("http://evil.com").is_none());
    }

    #[test]
    fn valid_origin_returns_header_value() {
        let val = allowed_origin_header_value("http://localhost:8800");
        assert!(val.is_some());
        assert_eq!(val.unwrap().to_str().unwrap(), "http://localhost:8800");
    }

    #[test]
    fn apply_cors_headers_adds_allowed_origin() {
        let mut resp = hyper::Response::builder()
            .status(200)
            .header("Access-Control-Allow-Origin", "*")
            .body(super::super::handlers::empty_body())
            .unwrap();

        apply_cors_headers(&mut resp, Some("http://localhost:8800"));

        assert_eq!(
            resp.headers()
                .get("Access-Control-Allow-Origin")
                .unwrap()
                .to_str()
                .unwrap(),
            "http://localhost:8800"
        );
        assert_eq!(
            resp.headers().get("Vary").unwrap().to_str().unwrap(),
            "Origin"
        );
    }

    #[test]
    fn apply_cors_headers_removes_wildcard_for_disallowed_origin() {
        let mut resp = hyper::Response::builder()
            .status(200)
            .header("Access-Control-Allow-Origin", "*")
            .body(super::super::handlers::empty_body())
            .unwrap();

        apply_cors_headers(&mut resp, Some("http://evil.com"));

        assert!(resp.headers().get("Access-Control-Allow-Origin").is_none());
    }

    #[test]
    fn apply_cors_headers_no_origin_header() {
        let mut resp = hyper::Response::builder()
            .status(200)
            .header("Access-Control-Allow-Origin", "*")
            .body(super::super::handlers::empty_body())
            .unwrap();

        apply_cors_headers(&mut resp, None);

        assert!(resp.headers().get("Access-Control-Allow-Origin").is_none());
    }

    fn ws_headers(pairs: &[(&str, &str)]) -> hyper::HeaderMap {
        let mut headers = hyper::HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                hyper::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                hyper::header::HeaderValue::from_str(value).unwrap(),
            );
        }
        headers
    }

    #[test]
    fn origin_matches_host_accepts_same_host_port() {
        assert!(origin_matches_host(
            "http://192.168.1.50:9000",
            "192.168.1.50:9000"
        ));
        assert!(origin_matches_host(
            "http://localhost:8800",
            "localhost:8800"
        ));
    }

    #[test]
    fn origin_matches_host_rejects_mismatch_or_garbage() {
        assert!(!origin_matches_host(
            "http://evil.example",
            "localhost:8800"
        ));
        assert!(!origin_matches_host("not a url", "localhost:8800"));
        assert!(!origin_matches_host("http://localhost:8800", ""));
    }

    #[test]
    fn ws_guard_allows_native_client_without_browser_context() {
        // No Origin / Referer / Sec-Fetch headers => native client => allowed.
        let headers = ws_headers(&[("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")]);
        assert_eq!(websocket_origin_rejection(&headers), None);
    }

    #[test]
    fn ws_guard_allows_same_origin_loopback() {
        let headers = ws_headers(&[
            ("origin", "http://localhost:8800"),
            ("host", "localhost:8800"),
            ("sec-fetch-site", "same-origin"),
        ]);
        assert_eq!(websocket_origin_rejection(&headers), None);
    }

    #[test]
    fn ws_guard_allows_allowlisted_loopback_origin() {
        let headers = ws_headers(&[
            ("origin", "http://127.0.0.1:9000"),
            ("host", "127.0.0.1:9000"),
        ]);
        assert_eq!(websocket_origin_rejection(&headers), None);
    }

    #[test]
    fn ws_guard_allows_trusted_desktop_origin_even_when_sec_fetch_is_cross_site() {
        let headers = ws_headers(&[
            ("origin", "tauri://localhost"),
            ("host", "127.0.0.1:9900"),
            ("sec-fetch-site", "cross-site"),
        ]);
        assert_eq!(websocket_origin_rejection(&headers), None);
    }

    #[test]
    fn ws_guard_rejects_untrusted_cross_site_via_sec_fetch() {
        let headers = ws_headers(&[
            ("origin", "http://evil.example.com"),
            ("host", "localhost:8800"),
            ("sec-fetch-site", "cross-site"),
        ]);
        assert_eq!(websocket_origin_rejection(&headers), Some("cross_site"));
    }

    #[test]
    fn ws_guard_rejects_cross_origin_attacker() {
        let headers = ws_headers(&[
            ("origin", "http://evil.example.com"),
            ("host", "localhost:8800"),
        ]);
        assert_eq!(websocket_origin_rejection(&headers), Some("cross_origin"));
    }

    #[test]
    fn ws_guard_accepts_matching_remote_host_origin() {
        // Remote-access admin served on a LAN IP: origin not in static
        // allowlist but matches the Host the browser connected to.
        let headers = ws_headers(&[
            ("origin", "http://192.168.1.50:9000"),
            ("host", "192.168.1.50:9000"),
        ]);
        assert_eq!(websocket_origin_rejection(&headers), None);
    }

    #[test]
    fn ws_guard_rejects_referer_only_cross_origin() {
        // Some browsers may omit Origin but send Referer; the guard still
        // engages because browser context is present, and with no Origin the
        // request is allowed (origin check is skipped) — but a cross-site
        // Sec-Fetch-Site must still be rejected.
        let headers = ws_headers(&[
            ("referer", "http://evil.example.com/page"),
            ("sec-fetch-site", "cross-site"),
        ]);
        assert_eq!(websocket_origin_rejection(&headers), Some("cross_site"));
    }
}
