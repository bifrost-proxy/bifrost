use std::sync::Arc;

use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};

use crate::handlers::{error_response, json_response, method_not_allowed, BoxBody};
use crate::remote_invoke::types::GrantMode;
use crate::remote_invoke::worker::RemoteInvokeWorker;

pub type SharedRemoteInvokeWorker = Arc<RemoteInvokeWorker>;

pub async fn handle_remote_invoke(
    req: Request<Incoming>,
    worker: Option<SharedRemoteInvokeWorker>,
    path: &str,
) -> Response<BoxBody> {
    let Some(worker) = worker else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "Remote invoke not enabled");
    };

    let sub = path.strip_prefix("/api/remote-invoke").unwrap_or(path);

    if sub == "/status" || sub == "/status/" {
        return handle_status(&req, &worker);
    }
    if sub == "/identity" || sub == "/identity/" {
        return handle_identity(&req, &worker);
    }
    if sub == "/discovery/enter" || sub == "/discovery/enter/" {
        return handle_discovery_enter(req, &worker).await;
    }
    if sub == "/discovery/exit" || sub == "/discovery/exit/" {
        return handle_discovery_exit(req, &worker).await;
    }
    if sub == "/discovery/refresh" || sub == "/discovery/refresh/" {
        return handle_discovery_refresh(req, &worker).await;
    }
    if sub == "/pairings/pending" || sub == "/pairings/pending/" {
        return handle_pairings_pending(&req, &worker);
    }
    if let Some(rest) = sub.strip_prefix("/pairings/") {
        if let Some(pairing_id) = rest.strip_suffix("/approve") {
            return handle_pairing_approve(req, &worker, pairing_id).await;
        }
        if let Some(pairing_id) = rest.strip_suffix("/approve/") {
            return handle_pairing_approve(req, &worker, pairing_id).await;
        }
        if let Some(pairing_id) = rest.strip_suffix("/reject") {
            return handle_pairing_reject(req, &worker, pairing_id).await;
        }
        if let Some(pairing_id) = rest.strip_suffix("/reject/") {
            return handle_pairing_reject(req, &worker, pairing_id).await;
        }
    }

    error_response(StatusCode::NOT_FOUND, "Remote invoke endpoint not found")
}

fn handle_status(req: &Request<Incoming>, worker: &RemoteInvokeWorker) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }

    let state = worker.state();
    let discovery = worker.discovery_session();
    let pending_count = worker.pending_pairings().len();
    let active_calls = worker.active_call_ids();

    json_response(&serde_json::json!({
        "state": format!("{:?}", state),
        "discovery_session": discovery,
        "pending_pairings_count": pending_count,
        "active_call_ids": active_calls,
    }))
}

fn handle_identity(req: &Request<Incoming>, worker: &RemoteInvokeWorker) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }

    let identity = worker.identity();
    json_response(&serde_json::json!({
        "instance_id": identity.instance_id,
        "device_name": identity.device_name,
        "platform": identity.platform,
    }))
}

async fn handle_discovery_enter(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    match worker.enter_discovery_mode().await {
        Ok(session) => json_response(&serde_json::json!({
            "success": true,
            "session": session,
        })),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to enter discovery mode: {e}"),
        ),
    }
}

async fn handle_discovery_exit(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    match worker.exit_discovery_mode().await {
        Ok(()) => json_response(&serde_json::json!({
            "success": true,
        })),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to exit discovery mode: {e}"),
        ),
    }
}

async fn handle_discovery_refresh(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    match worker.refresh_pair_code().await {
        Ok(Some(session)) => json_response(&serde_json::json!({
            "success": true,
            "session": session,
        })),
        Ok(None) => error_response(StatusCode::BAD_REQUEST, "Not in discovery mode"),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to refresh pair code: {e}"),
        ),
    }
}

fn handle_pairings_pending(
    req: &Request<Incoming>,
    worker: &RemoteInvokeWorker,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }

    let pairings = worker.pending_pairings();
    json_response(&serde_json::json!({
        "pairings": pairings,
    }))
}

async fn handle_pairing_approve(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
    pairing_id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read request body: {e}"),
            );
        }
    };

    #[derive(serde::Deserialize)]
    struct ApproveBody {
        grant_mode: GrantMode,
    }

    let parsed: ApproveBody = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid request body: {e}"),
            );
        }
    };

    match worker.approve_pairing(pairing_id, parsed.grant_mode).await {
        Ok(result) => json_response(&serde_json::json!({
            "success": true,
            "data": result,
        })),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to approve pairing: {e}"),
        ),
    }
}

async fn handle_pairing_reject(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
    pairing_id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    match worker.reject_pairing(pairing_id).await {
        Ok(result) => json_response(&serde_json::json!({
            "success": true,
            "data": result,
        })),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to reject pairing: {e}"),
        ),
    }
}
