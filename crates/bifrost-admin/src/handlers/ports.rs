use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};

use super::{error_response, json_response, success_response, BoxBody};
use crate::state::SharedAdminState;
use crate::temp_ports::{TemporaryPortBindRequest, TemporaryPortError, TemporaryPortUpdateRequest};

pub async fn handle_ports(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let Some(manager) = state.temporary_port_manager() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Temporary port manager is not configured",
        );
    };

    let suffix = path.strip_prefix("/api/ports").unwrap_or("");
    match (req.method(), suffix) {
        (&Method::GET, "" | "/") => json_response(&manager.list().await),
        (&Method::POST, "" | "/") => {
            let body = match read_json::<TemporaryPortBindRequest>(req).await {
                Ok(body) => body,
                Err(resp) => return resp,
            };
            match manager.bind(body).await {
                Ok(binding) => json_response(&binding),
                Err(error) => temporary_port_error(error),
            }
        }
        _ => handle_port_item(req, manager, suffix).await,
    }
}

async fn handle_port_item(
    req: Request<Incoming>,
    manager: crate::temp_ports::SharedTemporaryPortManager,
    suffix: &str,
) -> Response<BoxBody> {
    let Some(rest) = suffix.strip_prefix('/') else {
        return error_response(StatusCode::NOT_FOUND, "Port endpoint not found");
    };
    let mut parts = rest.split('/');
    let Some(port_raw) = parts.next() else {
        return error_response(StatusCode::NOT_FOUND, "Port endpoint not found");
    };
    let port = match port_raw.parse::<u16>() {
        Ok(port) => port,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid port"),
    };
    let tail = parts.next();
    if parts.next().is_some() {
        return error_response(StatusCode::NOT_FOUND, "Port endpoint not found");
    }

    match (req.method(), tail) {
        (&Method::GET, None) => match manager.show(port).await {
            Ok(binding) => json_response(&binding),
            Err(error) => temporary_port_error(error),
        },
        (&Method::PATCH, None) => {
            let body = match read_json::<TemporaryPortUpdateRequest>(req).await {
                Ok(body) => body,
                Err(resp) => return resp,
            };
            match manager.update(port, body).await {
                Ok(binding) => json_response(&binding),
                Err(error) => temporary_port_error(error),
            }
        }
        (&Method::DELETE, None) => match manager.destroy(port).await {
            Ok(_) => success_response(&format!("Temporary port {port} destroyed.")),
            Err(error) => temporary_port_error(error),
        },
        (&Method::GET, Some("active-summary")) => match manager.active_summary(port).await {
            Ok(summary) => json_response(&summary),
            Err(error) => temporary_port_error(error),
        },
        _ => error_response(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed"),
    }
}

async fn read_json<T: serde::de::DeserializeOwned>(
    req: Request<Incoming>,
) -> Result<T, Response<BoxBody>> {
    let body = req
        .into_body()
        .collect()
        .await
        .map_err(|e| error_response(StatusCode::BAD_REQUEST, &format!("Invalid body: {e}")))?
        .to_bytes();
    serde_json::from_slice::<T>(&body)
        .map_err(|e| error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {e}")))
}

fn temporary_port_error(error: TemporaryPortError) -> Response<BoxBody> {
    error_response(error.status, &error.message)
}
