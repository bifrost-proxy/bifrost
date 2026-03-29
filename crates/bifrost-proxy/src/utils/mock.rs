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
        if rules.host.is_none()
            && rules.mock_file.is_none()
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
        return Some(build_template_response(
            template,
            rules,
            request_uri,
            verbose_logging,
            ctx,
        ));
    }

    None
}

fn build_status_response(status: u16, rules: &ResolvedRules) -> Response<BoxBody> {
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

fn build_template_response(
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

    let template = if template.starts_with('(') && template.ends_with(')') {
        &template[1..template.len() - 1]
    } else {
        template
    };
    let rendered = TemplateEngine::expand_with_context(
        template,
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

fn guess_content_type(file_path: &str) -> &'static str {
    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    ext_to_content_type(ext)
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

fn guess_content_type_from_url(url: &str) -> &'static str {
    let path = url.split('?').next().unwrap_or(url);
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    ext_to_content_type(ext)
}

fn ext_to_content_type(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "eot" => "application/vnd.ms-fontobject",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
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
        assert_eq!(
            guess_content_type("/path/to/file.js"),
            "application/javascript; charset=utf-8"
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
            guess_content_type("/path/to/file.xyz"),
            "application/octet-stream"
        );
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
}
