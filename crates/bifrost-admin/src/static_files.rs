use hyper::{header, HeaderMap, Response, StatusCode};
use rust_embed::{Embed, RustEmbed};

use crate::handlers::{full_body, BoxBody};

#[derive(RustEmbed)]
#[folder = "../../web/dist-gzip"]
#[prefix = ""]
struct Asset;

pub fn serve_static_file(path: &str, headers: &HeaderMap) -> Response<BoxBody> {
    if !accepts_gzip(headers) {
        return gzip_required_response();
    }

    let file_path = if path.is_empty() || path == "/" {
        "index.html"
    } else {
        path.trim_start_matches('/')
    };

    match <Asset as Embed>::get(file_path) {
        Some(content) => {
            let mime = mime_guess::from_path(file_path)
                .first_or_octet_stream()
                .to_string();
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", mime)
                .header("Content-Encoding", "gzip")
                .header("Vary", "Accept-Encoding")
                .header("Cache-Control", "public, max-age=3600")
                .body(full_body(content.data.to_vec()))
                .unwrap()
        }
        None => match <Asset as Embed>::get("index.html") {
            Some(content) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/html")
                .header("Content-Encoding", "gzip")
                .header("Vary", "Accept-Encoding")
                .body(full_body(content.data.to_vec()))
                .unwrap(),
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("Content-Type", "text/plain")
                .body(full_body("Not Found"))
                .unwrap(),
        },
    }
}

fn gzip_required_response() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::UPGRADE_REQUIRED)
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("Vary", "Accept-Encoding")
        .body(full_body(
            "Bifrost WebUI requires a gzip-capable browser. Please upgrade your browser.",
        ))
        .unwrap()
}

fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(encoding_allows_gzip)
}

fn encoding_allows_gzip(part: &str) -> bool {
    let mut segments = part.split(';').map(str::trim);
    let Some(encoding) = segments.next() else {
        return false;
    };
    if !encoding.eq_ignore_ascii_case("gzip") {
        return false;
    }

    for segment in segments {
        let Some((name, value)) = segment.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("q") {
            let Ok(q) = value.trim().parse::<f32>() else {
                continue;
            };
            if q <= 0.0 {
                return false;
            }
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use hyper::header::ACCEPT_ENCODING;

    async fn response_body(response: Response<BoxBody>) -> Vec<u8> {
        response
            .into_body()
            .collect()
            .await
            .expect("response body should collect")
            .to_bytes()
            .to_vec()
    }

    fn gzip_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, "br, gzip;q=1.0".parse().unwrap());
        headers
    }

    #[test]
    fn accepts_gzip_encoding_header() {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, "br, gzip;q=1.0".parse().unwrap());
        assert!(accepts_gzip(&headers));

        headers.insert(ACCEPT_ENCODING, "gzip;q=0".parse().unwrap());
        assert!(!accepts_gzip(&headers));

        headers.insert(ACCEPT_ENCODING, "gzip;q=0.0".parse().unwrap());
        assert!(!accepts_gzip(&headers));

        headers.insert(ACCEPT_ENCODING, "br, deflate".parse().unwrap());
        assert!(!accepts_gzip(&headers));
    }

    #[tokio::test]
    async fn serves_index_as_gzip_static_asset() {
        let response = serve_static_file("/", &gzip_headers());

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get("Content-Encoding").unwrap(), "gzip");
        assert_eq!(response.headers().get("Vary").unwrap(), "Accept-Encoding");
        assert_eq!(response.headers().get("Content-Type").unwrap(), "text/html");
        assert!(!response_body(response).await.is_empty());
    }

    #[tokio::test]
    async fn rejects_non_gzip_static_client() {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, "identity".parse().unwrap());
        let response = serve_static_file("/", &headers);

        assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
        assert_eq!(response.headers().get("Vary").unwrap(), "Accept-Encoding");
        let body = String::from_utf8(response_body(response).await).unwrap();
        assert!(body.contains("requires a gzip-capable browser"));
    }

    #[tokio::test]
    async fn falls_back_to_gzip_index_for_spa_routes() {
        let response = serve_static_file("/rules/not-a-real-asset", &gzip_headers());

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get("Content-Encoding").unwrap(), "gzip");
        assert_eq!(response.headers().get("Content-Type").unwrap(), "text/html");
        assert!(!response_body(response).await.is_empty());
    }
}
