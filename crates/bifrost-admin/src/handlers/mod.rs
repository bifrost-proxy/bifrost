pub mod agent_skills;
pub mod app_icon;
pub mod asr;
pub mod asr_cli_invoke;
pub mod asr_jobs;
mod asr_jobs_daily;
mod asr_jobs_source;
mod asr_jobs_timeline;
pub mod asr_streaming;
pub mod audit;
pub mod auth;
pub mod bifrost_file;
pub mod breakpoint;
pub mod capture;
pub mod cert;
pub mod config;
pub mod devtools;
pub mod env;
pub mod frames;
pub mod group;
pub mod group_rules;
pub mod im_gateway;
pub mod metrics;
pub mod mobile_devices;
pub mod notification;
pub mod ports;
pub mod power;
pub mod proxy;
pub mod remote_invoke;
pub mod replay;
mod replay_ws;
pub mod room;
pub mod rule_share_confirm;
pub mod rules;
pub mod scripts;
pub mod search;
pub mod speech;
pub mod swagger;
pub mod sync;
pub mod syntax;
pub mod system;
pub mod traffic;
pub mod trust_probe;
pub mod user;
pub mod values;
pub mod videos;
pub mod voice;
pub mod voice_stateful;
pub mod websocket;
pub mod whitelist;

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::{Response, StatusCode};
use serde::Serialize;

pub type BoxBody = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;
pub const ADMIN_CORS_ALLOW_HEADERS: &str =
    "Content-Type, Authorization, X-Client-Id, X-Bifrost-CSRF";
pub const PUBLIC_CORS_ALLOW_METHODS: &str = "GET, HEAD, OPTIONS";

pub fn full_body(body: impl Into<Bytes>) -> BoxBody {
    http_body_util::Full::new(body.into())
        .map_err(|e| match e {})
        .boxed()
}

pub fn empty_body() -> BoxBody {
    http_body_util::Empty::new().map_err(|e| match e {}).boxed()
}

pub fn json_response<T: Serialize>(data: &T) -> Response<BoxBody> {
    json_response_with_status(StatusCode::OK, data)
}

pub fn json_response_with_status<T: Serialize>(status: StatusCode, data: &T) -> Response<BoxBody> {
    match serde_json::to_string(data) {
        Ok(json) => Response::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .body(full_body(json))
            .unwrap(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("JSON serialization error: {}", e),
        ),
    }
}

pub fn error_response(status: StatusCode, message: &str) -> Response<BoxBody> {
    let body = serde_json::json!({
        "error": message,
        "status": status.as_u16()
    });
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(full_body(body.to_string()))
        .unwrap()
}

pub fn success_response(message: &str) -> Response<BoxBody> {
    let body = serde_json::json!({
        "success": true,
        "message": message
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(full_body(body.to_string()))
        .unwrap()
}

#[allow(dead_code)]
pub fn not_found() -> Response<BoxBody> {
    error_response(StatusCode::NOT_FOUND, "Not Found")
}

pub fn method_not_allowed() -> Response<BoxBody> {
    error_response(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed")
}

pub fn cors_preflight() -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header(
            "Access-Control-Allow-Methods",
            "GET, POST, PUT, DELETE, OPTIONS",
        )
        .header("Access-Control-Allow-Headers", ADMIN_CORS_ALLOW_HEADERS)
        .header("Access-Control-Max-Age", "86400")
        .body(empty_body())
        .unwrap()
}

pub fn public_response_builder(status: StatusCode) -> hyper::http::response::Builder {
    Response::builder()
        .status(status)
        .header("Access-Control-Allow-Methods", PUBLIC_CORS_ALLOW_METHODS)
        .header("Access-Control-Allow-Headers", ADMIN_CORS_ALLOW_HEADERS)
}

#[cfg(test)]
mod tests {
    use super::{
        cors_preflight, public_response_builder, ADMIN_CORS_ALLOW_HEADERS,
        PUBLIC_CORS_ALLOW_METHODS,
    };
    use hyper::StatusCode;

    #[test]
    fn cors_preflight_allows_desktop_client_header() {
        let response = cors_preflight();
        let headers = response.headers();
        let allow_headers = headers
            .get("Access-Control-Allow-Headers")
            .and_then(|value| value.to_str().ok());

        assert_eq!(allow_headers, Some(ADMIN_CORS_ALLOW_HEADERS));
        assert!(
            allow_headers
                .map(|value| value.contains("X-Client-Id"))
                .unwrap_or(false),
            "desktop requests require X-Client-Id to pass CORS preflight"
        );
    }

    #[test]
    fn public_response_builder_includes_cors_headers() {
        let response = public_response_builder(StatusCode::OK)
            .body(super::empty_body())
            .unwrap();
        let headers = response.headers();

        assert!(headers.get("Access-Control-Allow-Origin").is_none());
        assert_eq!(
            headers
                .get("Access-Control-Allow-Methods")
                .and_then(|value| value.to_str().ok()),
            Some(PUBLIC_CORS_ALLOW_METHODS)
        );
        assert_eq!(
            headers
                .get("Access-Control-Allow-Headers")
                .and_then(|value| value.to_str().ok()),
            Some(ADMIN_CORS_ALLOW_HEADERS)
        );
    }
}
