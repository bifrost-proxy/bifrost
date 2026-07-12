use bytes::Bytes;
use hyper::{header, Response, StatusCode};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
use tracing::{debug, warn};

use crate::ensure_crypto_provider;
use crate::server::{full_body, BoxBody, ResolvedRules};
use crate::utils::logging::RequestContext;
use crate::utils::url::build_redirect_uri;
use bifrost_core::TemplateEngine;

type HttpClient =
    Client<hyper_util::client::legacy::connect::HttpConnector, http_body_util::Empty<Bytes>>;

type HttpsClient = Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    http_body_util::Empty<Bytes>,
>;

static HTTP_CLIENT: OnceLock<HttpClient> = OnceLock::new();
static HTTPS_CLIENT: OnceLock<HttpsClient> = OnceLock::new();

fn get_http_client() -> &'static HttpClient {
    HTTP_CLIENT.get_or_init(|| Client::builder(TokioExecutor::new()).build_http())
}

fn get_https_client() -> &'static HttpsClient {
    HTTPS_CLIENT.get_or_init(|| {
        ensure_crypto_provider();

        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(config)
            .https_or_http()
            .enable_all_versions()
            .build();

        Client::builder(TokioExecutor::new()).build(https_connector)
    })
}

pub async fn generate_mock_response(
    rules: &ResolvedRules,
    request_uri: &hyper::Uri,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Option<Response<BoxBody>> {
    if rules.ignored.all {
        return None;
    }

    if let Some(status) = rules.status_code {
        if rules.mock_file.is_none()
            && rules.mock_rawfile.is_none()
            && rules.mock_template.is_none()
            && rules.location_href.is_none()
        {
            if verbose_logging {
                debug!("[{}] [MOCK] status code: {}", ctx.id_str(), status);
            }
            return Some(build_status_response(status, rules));
        }
    }

    if let Some(redirect_target) = &rules.redirect {
        if let Some(location) = build_redirect_uri(request_uri, redirect_target) {
            let status = rules.redirect_status.unwrap_or(302);
            if verbose_logging {
                debug!("[{}] [REDIRECT] {} -> {}", ctx.id_str(), status, location);
            }
            return Some(build_redirect_response(status, &location));
        }
    }

    if let Some(location) = &rules.location_href {
        if verbose_logging {
            debug!("[{}] [LOCATION_HREF] -> {}", ctx.id_str(), location);
        }
        let body = format!(
            r#"<!doctype html><html><head><meta charset="utf-8"></head><body><script>location.href = "{}";</script></body></html>"#,
            location
        );
        return Some(
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(full_body(body))
                .unwrap(),
        );
    }

    if let Some(file_path) = &rules.mock_file {
        if file_path.starts_with("http://") || file_path.starts_with("https://") {
            return load_remote_response(file_path, rules, verbose_logging, ctx).await;
        }
        if file_path.starts_with('(') && file_path.ends_with(')') {
            let content = &file_path[1..file_path.len() - 1];
            if verbose_logging {
                debug!(
                    "[{}] [FILE] inline content ({} bytes)",
                    ctx.id_str(),
                    content.len()
                );
            }
            let status = rules.status_code.unwrap_or(200);
            let status_code = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
            let mut builder = Response::builder()
                .status(status_code)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8");
            for (key, value) in &rules.res_headers {
                builder = builder.header(key.as_str(), value.as_str());
            }
            return Some(
                builder
                    .body(full_body(Bytes::from(content.to_string())))
                    .unwrap(),
            );
        }
        return load_file_response(file_path, rules, verbose_logging, ctx).await;
    }

    if let Some(file_path) = &rules.mock_rawfile {
        if file_path.starts_with('(') && file_path.ends_with(')') {
            return Some(build_inline_rawfile_response(
                &file_path[1..file_path.len() - 1],
                verbose_logging,
                ctx,
            ));
        }
        return load_rawfile_response(file_path, verbose_logging, ctx).await;
    }

    if let Some(template) = &rules.mock_template {
        return Some(
            build_template_response(template, rules, request_uri, verbose_logging, ctx).await,
        );
    }

    None
}

pub(crate) fn build_status_response(status: u16, rules: &ResolvedRules) -> Response<BoxBody> {
    let status_code = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    let body = rules
        .res_body
        .clone()
        .unwrap_or_else(|| Bytes::from(status_code.canonical_reason().unwrap_or("")));

    let mut builder = Response::builder().status(status_code);

    if rules.res_type.is_some() || rules.res_charset.is_some() {
        let base_ct = rules
            .res_type
            .as_deref()
            .map(|ct| ct.split(';').next().unwrap_or(ct).trim())
            .unwrap_or("text/plain");

        let content_type = if let Some(ref charset) = rules.res_charset {
            format!("{}; charset={}", base_ct, charset)
        } else {
            base_ct.to_string()
        };
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }

    if let Some(ref cache_value) = rules.cache {
        let cache_control = if let Ok(seconds) = cache_value.parse::<u64>() {
            if seconds == 0 {
                "no-cache, no-store, must-revalidate".to_string()
            } else {
                format!("max-age={}", seconds)
            }
        } else {
            cache_value.clone()
        };
        builder = builder.header(header::CACHE_CONTROL, cache_control);
    }

    for (key, value) in &rules.res_headers {
        builder = builder.header(key.as_str(), value.as_str());
    }

    builder.body(full_body(body)).unwrap()
}

fn build_redirect_response(status: u16, location: &str) -> Response<BoxBody> {
    let status_code = StatusCode::from_u16(status).unwrap_or(StatusCode::FOUND);
    Response::builder()
        .status(status_code)
        .header(header::LOCATION, location)
        .body(full_body(Bytes::new()))
        .unwrap()
}

async fn load_file_response(
    file_path: &str,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Option<Response<BoxBody>> {
    let normalized = normalize_file_path(file_path);
    let path = Path::new(&normalized);

    match tokio::fs::read(path).await {
        Ok(content) => {
            if verbose_logging {
                debug!(
                    "[{}] [FILE] loaded {} ({} bytes)",
                    ctx.id_str(),
                    file_path,
                    content.len()
                );
            }

            let content_type = guess_content_type(file_path);
            let status = rules.status_code.unwrap_or(200);
            let status_code = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);

            let mut builder = Response::builder()
                .status(status_code)
                .header(header::CONTENT_TYPE, content_type);

            for (key, value) in &rules.res_headers {
                builder = builder.header(key.as_str(), value.as_str());
            }

            Some(builder.body(full_body(content)).unwrap())
        }
        Err(e) => {
            warn!(
                "[{}] [FILE] failed to read {}: {}",
                ctx.id_str(),
                file_path,
                e
            );
            Some(build_error_response(404, "File not found"))
        }
    }
}

async fn load_remote_response(
    url: &str,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Option<Response<BoxBody>> {
    let uri: hyper::Uri = match url.parse() {
        Ok(u) => u,
        Err(e) => {
            warn!("[{}] [REMOTE] invalid URL {}: {}", ctx.id_str(), url, e);
            return Some(build_error_response(400, "Invalid URL"));
        }
    };

    let is_https = uri.scheme_str() == Some("https");

    let result = if is_https {
        load_https_content(uri.clone(), verbose_logging, ctx).await
    } else {
        load_http_content(uri.clone(), verbose_logging, ctx).await
    };

    match result {
        Ok(content) => {
            if verbose_logging {
                debug!(
                    "[{}] [REMOTE] fetched {} ({} bytes)",
                    ctx.id_str(),
                    url,
                    content.len()
                );
            }

            let content_type = guess_content_type_from_url(url);
            let status = rules.status_code.unwrap_or(200);
            let status_code = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);

            let mut builder = Response::builder()
                .status(status_code)
                .header(header::CONTENT_TYPE, content_type);

            for (key, value) in &rules.res_headers {
                builder = builder.header(key.as_str(), value.as_str());
            }

            Some(builder.body(full_body(content)).unwrap())
        }
        Err(e) => {
            warn!("[{}] [REMOTE] failed to fetch {}: {}", ctx.id_str(), url, e);
            Some(build_error_response(
                502,
                &format!("Failed to fetch remote URL: {}", e),
            ))
        }
    }
}

async fn load_http_content(
    uri: hyper::Uri,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    use http_body_util::BodyExt;

    let client = get_http_client();

    let req = hyper::Request::builder()
        .method("GET")
        .uri(&uri)
        .header("User-Agent", "bifrost-proxy")
        .body(http_body_util::Empty::<Bytes>::new())?;

    if verbose_logging {
        debug!("[{}] [REMOTE] fetching HTTP {}", ctx.id_str(), uri);
    }

    let response = tokio::time::timeout(Duration::from_secs(30), client.request(req))
        .await
        .map_err(|_| "Request timeout")??;

    let body = response.into_body();
    let collected = body.collect().await?;
    Ok(collected.to_bytes().to_vec())
}

async fn load_https_content(
    uri: hyper::Uri,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    use http_body_util::BodyExt;

    let client = get_https_client();

    let req = hyper::Request::builder()
        .method("GET")
        .uri(&uri)
        .header("User-Agent", "bifrost-proxy")
        .body(http_body_util::Empty::<Bytes>::new())?;

    if verbose_logging {
        debug!("[{}] [REMOTE] fetching HTTPS {}", ctx.id_str(), uri);
    }

    let response = tokio::time::timeout(Duration::from_secs(30), client.request(req))
        .await
        .map_err(|_| "Request timeout")??;

    let body = response.into_body();
    let collected = body.collect().await?;
    Ok(collected.to_bytes().to_vec())
}

async fn load_rawfile_response(
    file_path: &str,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Option<Response<BoxBody>> {
    let normalized = normalize_file_path(file_path);
    let path = Path::new(&normalized);

    match tokio::fs::read(path).await {
        Ok(content) => {
            if verbose_logging {
                debug!(
                    "[{}] [RAWFILE] loaded {} ({} bytes)",
                    ctx.id_str(),
                    file_path,
                    content.len()
                );
            }

            let content_type = guess_content_type(file_path);

            Some(
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, content_type)
                    .body(full_body(content))
                    .unwrap(),
            )
        }
        Err(e) => {
            warn!(
                "[{}] [RAWFILE] failed to read {}: {}",
                ctx.id_str(),
                file_path,
                e
            );
            Some(build_error_response(404, "File not found"))
        }
    }
}

async fn build_template_response(
    template: &str,
    rules: &ResolvedRules,
    request_uri: &hyper::Uri,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Response<BoxBody> {
    let host_string = request_uri
        .host()
        .map(str::to_string)
        .or_else(|| {
            url::Url::parse(&ctx.url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_string))
        })
        .unwrap_or_default();
    let host = if ctx.host.is_empty() {
        host_string.as_str()
    } else {
        ctx.host.as_str()
    };
    let path = if ctx.pathname.is_empty() {
        request_uri.path()
    } else {
        ctx.pathname.as_str()
    };
    let url_string = if ctx.url.is_empty() {
        if let Some(authority) = request_uri.authority() {
            format!(
                "http://{}{}",
                authority,
                request_uri
                    .path_and_query()
                    .map(|path| path.as_str())
                    .unwrap_or("/")
            )
        } else {
            request_uri.to_string()
        }
    } else {
        ctx.url.clone()
    };

    let template_content = if template.starts_with('(') && template.ends_with(')') {
        template[1..template.len() - 1].to_string()
    } else {
        let normalized = normalize_file_path(template);
        match tokio::fs::read_to_string(&normalized).await {
            Ok(content) => {
                if verbose_logging {
                    debug!(
                        "[{}] [TPL] loaded template file {} ({} bytes)",
                        ctx.id_str(),
                        normalized,
                        content.len()
                    );
                }
                content
            }
            Err(e) => {
                warn!(
                    "[{}] [TPL] failed to read template file {}: {}",
                    ctx.id_str(),
                    normalized,
                    e
                );
                return build_error_response(404, "Template file not found");
            }
        }
    };
    let rendered = TemplateEngine::expand_with_context(
        &template_content,
        &bifrost_core::RequestContext::builder()
            .url(&url_string)
            .host(host)
            .hostname(host)
            .path(path)
            .pathname(path)
            .method(&ctx.method)
            .client_ip(&ctx.client_ip)
            .build(),
        None,
        &rules.values,
    )
    .replace("{{host}}", host)
    .replace("{{url}}", &url_string)
    .replace("{{path}}", path)
    .replace("{{method}}", &ctx.method)
    .replace(
        "{{now}}",
        &std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis().to_string())
            .unwrap_or_default(),
    );

    if verbose_logging {
        debug!(
            "[{}] [TPL] rendered template ({} bytes)",
            ctx.id_str(),
            rendered.len()
        );
    }

    let status = rules.status_code.unwrap_or(200);
    let status_code = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);

    let content_type = rules.res_type.as_deref().unwrap_or("application/json");

    let mut builder = Response::builder()
        .status(status_code)
        .header(header::CONTENT_TYPE, content_type);

    for (key, value) in &rules.res_headers {
        builder = builder.header(key.as_str(), value.as_str());
    }

    builder.body(full_body(rendered)).unwrap()
}

fn build_inline_rawfile_response(
    raw: &str,
    verbose_logging: bool,
    ctx: &RequestContext,
) -> Response<BoxBody> {
    let decoded = raw.replace("\\r\\n", "\r\n");
    if verbose_logging {
        debug!(
            "[{}] [RAWFILE] inline response ({} bytes)",
            ctx.id_str(),
            decoded.len()
        );
    }

    if !decoded.starts_with("HTTP/") && !decoded.contains("\r\n\r\n") {
        return Response::builder()
            .status(StatusCode::OK)
            .body(full_body(decoded))
            .unwrap();
    }

    let (head, body) = decoded.split_once("\r\n\r\n").unwrap_or((&decoded, ""));
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or("HTTP/1.1 200 OK");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .and_then(|value| StatusCode::from_u16(value).ok())
        .unwrap_or(StatusCode::OK);

    let mut builder = Response::builder().status(status);
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            builder = builder.header(name.trim(), value.trim());
        }
    }

    builder.body(full_body(body.to_string())).unwrap()
}

fn build_error_response(status: u16, message: &str) -> Response<BoxBody> {
    let status_code = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    Response::builder()
        .status(status_code)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(full_body(message.to_string()))
        .unwrap()
}

pub fn guess_content_type(file_path: &str) -> String {
    mime_guess::from_path(file_path)
        .first()
        .map(|m| {
            let essence = m.essence_str().to_string();
            if is_text_mime(&essence) && m.get_param("charset").is_none() {
                format!("{}; charset=utf-8", essence)
            } else {
                essence
            }
        })
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

pub fn is_text_mime(content_type: &str) -> bool {
    let ct = content_type.split(';').next().unwrap_or("").trim();
    ct.starts_with("text/")
        || ct == "application/json"
        || ct == "application/xml"
        || ct == "application/javascript"
        || ct == "application/x-javascript"
        || ct.ends_with("+json")
        || ct.ends_with("+xml")
}

fn normalize_file_path(file_path: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        let path = file_path.replace('/', "\\");
        if path.len() >= 3 && path.as_bytes()[0] == b'\\' && path.as_bytes()[2] == b'\\' {
            let drive = path.as_bytes()[1];
            if drive.is_ascii_alphabetic() {
                return format!(
                    "{}:{}",
                    (drive as char).to_uppercase().next().unwrap(),
                    &path[2..]
                );
            }
        }
        path
    }
    #[cfg(not(target_os = "windows"))]
    {
        file_path.to_string()
    }
}

fn guess_content_type_from_url(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    guess_content_type(path)
}

pub fn should_intercept_response(rules: &ResolvedRules) -> bool {
    rules.status_code.is_some()
        || rules.redirect.is_some()
        || rules.location_href.is_some()
        || rules.mock_file.is_some()
        || rules.mock_rawfile.is_some()
        || rules.mock_template.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    async fn body_text(response: Response<BoxBody>) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn temp_path(extension: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "bifrost-mock-test-{}-{}.{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            extension
        ))
    }

    #[test]
    fn test_guess_content_type_html() {
        assert_eq!(
            guess_content_type("/path/to/file.html"),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            guess_content_type("/path/to/file.htm"),
            "text/html; charset=utf-8"
        );
    }

    #[test]
    fn test_guess_content_type_js() {
        let ct = guess_content_type("/path/to/file.js");
        assert!(
            ct.contains("javascript"),
            "expected javascript content type, got: {}",
            ct
        );
    }

    #[test]
    fn test_guess_content_type_json() {
        assert_eq!(
            guess_content_type("/path/to/file.json"),
            "application/json; charset=utf-8"
        );
    }

    #[test]
    fn test_guess_content_type_image() {
        assert_eq!(guess_content_type("/path/to/file.png"), "image/png");
        assert_eq!(guess_content_type("/path/to/file.jpg"), "image/jpeg");
        assert_eq!(guess_content_type("/path/to/file.gif"), "image/gif");
    }

    #[test]
    fn test_guess_content_type_unknown() {
        assert_eq!(
            guess_content_type("/path/to/file"),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_build_redirect_response() {
        let response = build_redirect_response(302, "https://example.com/new");
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "https://example.com/new"
        );
    }

    #[test]
    fn test_build_status_response() {
        let rules = ResolvedRules::default();
        let response = build_status_response(404, &rules);
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_status_code_with_host_generates_direct_response() {
        let mut rules = ResolvedRules {
            host: Some("127.0.0.1:65535".to_string()),
            status_code: Some(418),
            ..ResolvedRules::default()
        };
        rules
            .res_headers
            .push(("X-Bifrost-Test".to_string(), "direct".to_string()));

        let uri: hyper::Uri = "http://example.test/api".parse().unwrap();
        let ctx = RequestContext::new();
        let response = generate_mock_response(&rules, &uri, false, &ctx)
            .await
            .expect("statusCode should generate direct response even when host is set");

        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(response.headers().get("x-bifrost-test").unwrap(), "direct");
    }

    #[test]
    fn test_should_intercept_response() {
        let mut rules = ResolvedRules::default();
        assert!(!should_intercept_response(&rules));

        rules.status_code = Some(200);
        assert!(should_intercept_response(&rules));

        rules.status_code = None;
        rules.mock_file = Some("/path/to/file".to_string());
        assert!(should_intercept_response(&rules));

        rules.mock_file = None;
        rules.redirect = Some("/new/path".to_string());
        assert!(should_intercept_response(&rules));
    }

    #[test]
    fn test_guess_content_type_from_url_strips_query() {
        let ct = guess_content_type_from_url("https://example.com/data.json?foo=bar");
        assert_eq!(ct, "application/json; charset=utf-8");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_normalize_file_path_passthrough_on_non_windows() {
        let path = "/tmp/bifrost/mock-response.txt";
        assert_eq!(normalize_file_path(path), path.to_string());
    }

    #[test]
    fn test_build_status_response_respects_type_charset_and_cache_seconds() {
        let rules = ResolvedRules {
            res_type: Some("text/html".to_string()),
            res_charset: Some("utf-8".to_string()),
            cache: Some("60".to_string()),
            ..Default::default()
        };

        let response = build_status_response(200, &rules);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(content_type, "text/html; charset=utf-8");

        let cache_control = response
            .headers()
            .get(header::CACHE_CONTROL)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cache_control, "max-age=60");
    }

    #[test]
    fn test_build_status_response_respects_custom_cache_string() {
        let rules = ResolvedRules {
            cache: Some("no-cache, no-store".to_string()),
            ..Default::default()
        };

        let response = build_status_response(200, &rules);
        let cache_control = response
            .headers()
            .get(header::CACHE_CONTROL)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cache_control, "no-cache, no-store");
    }

    #[tokio::test]
    async fn generate_mock_response_covers_ignore_redirect_and_location() {
        let uri: hyper::Uri = "http://example.test/base/path?x=1".parse().unwrap();
        let ctx = RequestContext::new();
        let mut rules = ResolvedRules::default();
        rules.ignored.all = true;
        assert!(generate_mock_response(&rules, &uri, true, &ctx)
            .await
            .is_none());

        rules.ignored.all = false;
        rules.redirect = Some("/next".to_string());
        rules.redirect_status = Some(307);
        let response = generate_mock_response(&rules, &uri, true, &ctx)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            response.headers()[header::LOCATION],
            "http://example.test/next"
        );

        rules.redirect = None;
        rules.location_href = Some("https://target.test/landing".to_string());
        let response = generate_mock_response(&rules, &uri, true, &ctx)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_text(response).await.contains("target.test/landing"));
    }

    #[tokio::test]
    async fn generate_mock_response_covers_inline_file_and_rawfile_formats() {
        let uri: hyper::Uri = "http://example.test/".parse().unwrap();
        let ctx = RequestContext::new();
        let rules = ResolvedRules {
            status_code: Some(0),
            mock_file: Some("(inline body)".to_string()),
            res_headers: vec![("X-Mock".to_string(), "inline".to_string())],
            ..Default::default()
        };
        let response = generate_mock_response(&rules, &uri, true, &ctx)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-mock"], "inline");
        assert_eq!(body_text(response).await, "inline body");

        let rules = ResolvedRules {
            mock_rawfile: Some(
                "(HTTP/1.1 201 Created\\r\\nX-Raw: yes\\r\\n\\r\\nraw body)".to_string(),
            ),
            ..Default::default()
        };
        let response = generate_mock_response(&rules, &uri, true, &ctx)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()["x-raw"], "yes");
        assert_eq!(body_text(response).await, "raw body");

        let rules = ResolvedRules {
            mock_rawfile: Some("(plain raw body)".to_string()),
            ..Default::default()
        };
        let response = generate_mock_response(&rules, &uri, false, &ctx)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_text(response).await, "plain raw body");
    }

    #[tokio::test]
    async fn generate_mock_response_covers_file_and_rawfile_success_and_failure() {
        let uri: hyper::Uri = "http://example.test/".parse().unwrap();
        let ctx = RequestContext::new();
        let file = temp_path("json");
        tokio::fs::write(&file, br#"{"file":true}"#).await.unwrap();

        let rules = ResolvedRules {
            status_code: Some(202),
            mock_file: Some(file.to_string_lossy().into_owned()),
            res_headers: vec![("X-File".to_string(), "yes".to_string())],
            ..Default::default()
        };
        let response = generate_mock_response(&rules, &uri, true, &ctx)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        assert_eq!(response.headers()["x-file"], "yes");
        assert!(body_text(response).await.contains("file"));

        let rules = ResolvedRules {
            mock_rawfile: Some(file.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let response = generate_mock_response(&rules, &uri, true, &ctx)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_text(response).await.contains("file"));
        tokio::fs::remove_file(&file).await.unwrap();

        for rules in [
            ResolvedRules {
                mock_file: Some(file.to_string_lossy().into_owned()),
                ..Default::default()
            },
            ResolvedRules {
                mock_rawfile: Some(file.to_string_lossy().into_owned()),
                ..Default::default()
            },
        ] {
            let response = generate_mock_response(&rules, &uri, false, &ctx)
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(body_text(response).await, "File not found");
        }
    }

    #[tokio::test]
    async fn template_response_covers_inline_file_context_and_missing_file() {
        let uri: hyper::Uri = "http://uri-host.test/from-uri?q=1".parse().unwrap();
        let ctx = RequestContext::new().with_request_info(
            "http://ctx-host.test/ctx-path".to_string(),
            "POST".to_string(),
            "ctx-host.test".to_string(),
            "/ctx-path".to_string(),
            "".to_string(),
            "127.0.0.1".to_string(),
        );
        let rules = ResolvedRules {
            status_code: Some(201),
            res_type: Some("text/plain".to_string()),
            mock_template: Some("({{host}}|{{path}}|{{method}}|{{url}}|{{now}})".to_string()),
            res_headers: vec![("X-Template".to_string(), "yes".to_string())],
            ..Default::default()
        };
        let response = generate_mock_response(&rules, &uri, true, &ctx)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/plain");
        assert_eq!(response.headers()["x-template"], "yes");
        let body = body_text(response).await;
        assert!(body.contains("ctx-host.test|/ctx-path|POST|http://ctx-host.test/ctx-path"));

        let file = temp_path("tpl");
        tokio::fs::write(&file, "file {{host}} {{path}}")
            .await
            .unwrap();
        let rules = ResolvedRules {
            mock_template: Some(file.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let response = generate_mock_response(&rules, &uri, true, &RequestContext::new())
            .await
            .unwrap();
        assert!(body_text(response)
            .await
            .contains("file uri-host.test /from-uri"));
        tokio::fs::remove_file(&file).await.unwrap();

        let response = generate_mock_response(&rules, &uri, false, &RequestContext::new())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_text(response).await, "Template file not found");
    }

    #[tokio::test]
    async fn remote_http_mock_covers_success_and_connection_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nremote body")
                .await
                .unwrap();
        });
        let uri: hyper::Uri = "http://example.test/".parse().unwrap();
        let ctx = RequestContext::new();
        let rules = ResolvedRules {
            status_code: Some(206),
            mock_file: Some(format!("http://{address}/data.json?download=1")),
            res_headers: vec![("X-Remote".to_string(), "yes".to_string())],
            ..Default::default()
        };
        let response = generate_mock_response(&rules, &uri, true, &ctx)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        assert_eq!(response.headers()["x-remote"], "yes");
        assert_eq!(body_text(response).await, "remote body");
        server.await.unwrap();

        let dead_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_address = dead_listener.local_addr().unwrap();
        drop(dead_listener);
        let rules = ResolvedRules {
            mock_file: Some(format!("http://{dead_address}/missing")),
            ..Default::default()
        };
        let response = generate_mock_response(&rules, &uri, false, &ctx)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert!(body_text(response)
            .await
            .contains("Failed to fetch remote URL"));
    }

    #[tokio::test]
    async fn coverage_90_remote_https_and_invalid_url_fail_closed() {
        let uri: hyper::Uri = "http://example.test/".parse().unwrap();
        let ctx = RequestContext::new();

        let invalid = ResolvedRules {
            mock_file: Some("http://[invalid".to_string()),
            ..Default::default()
        };
        let response = generate_mock_response(&invalid, &uri, true, &ctx)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unavailable = listener.local_addr().unwrap();
        drop(listener);
        let https = ResolvedRules {
            mock_file: Some(format!("https://{unavailable}/missing.json")),
            ..Default::default()
        };
        let response = generate_mock_response(&https, &uri, true, &ctx)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert!(body_text(response)
            .await
            .contains("Failed to fetch remote URL"));
    }

    #[test]
    fn status_and_content_type_helpers_cover_edge_values() {
        let rules = ResolvedRules {
            status_code: Some(0),
            res_body: Some(Bytes::from_static(b"custom")),
            res_charset: Some("utf-16".to_string()),
            cache: Some("0".to_string()),
            res_headers: vec![("X-Status".to_string(), "edge".to_string())],
            ..Default::default()
        };
        let response = build_status_response(0, &rules);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-16"
        );
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "no-cache, no-store, must-revalidate"
        );
        assert_eq!(response.headers()["x-status"], "edge");

        assert_eq!(
            build_redirect_response(0, "/fallback").status(),
            StatusCode::FOUND
        );
        assert_eq!(
            build_error_response(0, "bad").status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert!(is_text_mime("text/plain; charset=utf-8"));
        assert!(is_text_mime("application/problem+json"));
        assert!(is_text_mime("application/soap+xml"));
        assert!(!is_text_mime("application/octet-stream"));

        assert!(should_intercept_response(&ResolvedRules {
            location_href: Some("/a".to_string()),
            ..ResolvedRules::default()
        }));
        assert!(should_intercept_response(&ResolvedRules {
            mock_rawfile: Some("(raw)".to_string()),
            ..ResolvedRules::default()
        }));
        assert!(should_intercept_response(&ResolvedRules {
            mock_template: Some("(tpl)".to_string()),
            ..ResolvedRules::default()
        }));
    }
}
