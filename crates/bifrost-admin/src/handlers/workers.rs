use std::time::Duration;

use hyper::{Method, Request, Response, StatusCode};
use serde::Serialize;

use super::{error_response, json_response, method_not_allowed, BoxBody};
use crate::worker_runtime::{
    execution_mode, execution_mode_env, global_worker_supervisor, WorkerKind,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerModeResponse {
    worker_kind: WorkerKind,
    execution_mode: &'static str,
    environment_variable: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerActionResponse {
    worker_kind: WorkerKind,
    action: &'static str,
    affected: usize,
    errors: Vec<WorkerActionError>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerActionError {
    key: String,
    error: String,
}

pub async fn handle_workers<B>(req: Request<B>, path: &str) -> Response<BoxBody> {
    let suffix = path.trim_start_matches("/api/workers").trim_matches('/');
    if suffix.is_empty() {
        return match *req.method() {
            Method::GET => json_response(&crate::worker_runtime::worker_snapshots().await),
            _ => method_not_allowed(),
        };
    }

    if suffix == "modes" {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        let modes = [
            WorkerKind::ExternalCli,
            WorkerKind::Browser,
            WorkerKind::Asr,
            WorkerKind::ImGateway,
            WorkerKind::RemoteInvoke,
            WorkerKind::RemoteExecution,
        ]
        .into_iter()
        .map(|kind| WorkerModeResponse {
            worker_kind: kind,
            execution_mode: execution_mode(kind).as_str(),
            environment_variable: execution_mode_env(kind),
        })
        .collect::<Vec<_>>();
        return json_response(&modes);
    }

    let mut segments = suffix.split('/');
    let Some(kind_segment) = segments.next() else {
        return error_response(StatusCode::NOT_FOUND, "Not Found");
    };
    let Some(kind) = WorkerKind::parse(kind_segment) else {
        return error_response(StatusCode::NOT_FOUND, "unknown worker kind");
    };
    let action = segments.next();
    if segments.next().is_some() {
        return error_response(StatusCode::NOT_FOUND, "Not Found");
    }

    match (req.method(), action) {
        (&Method::GET, None) => {
            let snapshots = crate::worker_runtime::worker_snapshots()
                .await
                .into_iter()
                .filter(|snapshot| snapshot.worker_kind == kind)
                .collect::<Vec<_>>();
            json_response(&snapshots)
        }
        (&Method::POST, Some("start")) => {
            let results = global_worker_supervisor().start_kind(kind).await;
            if results.is_empty() {
                return error_response(
                    StatusCode::CONFLICT,
                    "worker kind has no registered startup specification; use the capability once before starting it manually",
                );
            }
            action_response(kind, "start", results)
        }
        (&Method::POST, Some("stop")) => {
            let affected = global_worker_supervisor()
                .suspend_kind(kind, Duration::from_secs(5))
                .await;
            json_response(&WorkerActionResponse {
                worker_kind: kind,
                action: "stop",
                affected,
                errors: Vec::new(),
            })
        }
        (&Method::POST, Some("restart")) => {
            let results = global_worker_supervisor().restart_kind(kind).await;
            if results.is_empty() {
                return error_response(
                    StatusCode::CONFLICT,
                    "worker kind has no registered restart specification",
                );
            }
            action_response(kind, "restart", results)
        }
        (&Method::POST, Some("reset-circuit")) => {
            let affected = global_worker_supervisor().reset_circuit_kind(kind).await;
            json_response(&WorkerActionResponse {
                worker_kind: kind,
                action: "reset_circuit",
                affected,
                errors: Vec::new(),
            })
        }
        (&Method::POST, None) | (&Method::GET, Some(_)) => method_not_allowed(),
        _ => method_not_allowed(),
    }
}

fn action_response(
    kind: WorkerKind,
    action: &'static str,
    results: Vec<(String, Result<(), String>)>,
) -> Response<BoxBody> {
    let affected = results.iter().filter(|(_, result)| result.is_ok()).count();
    let errors = results
        .into_iter()
        .filter_map(|(key, result)| result.err().map(|error| WorkerActionError { key, error }))
        .collect::<Vec<_>>();
    let response = WorkerActionResponse {
        worker_kind: kind,
        action,
        affected,
        errors,
    };
    if response.affected == 0 && !response.errors.is_empty() {
        let body = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
        return Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header("Content-Type", "application/json")
            .body(super::full_body(body))
            .expect("worker action response");
    }
    json_response(&response)
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;

    use super::*;

    #[tokio::test]
    async fn worker_root_is_read_only_and_empty_by_default() {
        let response = handle_workers(
            Request::builder()
                .method(Method::GET)
                .uri("/_bifrost/api/workers")
                .body(())
                .unwrap(),
            "/api/workers",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(value.is_array());
    }

    #[tokio::test]
    async fn worker_modes_expose_all_rollback_environment_variables() {
        let response = handle_workers(
            Request::builder()
                .method(Method::GET)
                .uri("/_bifrost/api/workers/modes")
                .body(())
                .unwrap(),
            "/api/workers/modes",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let modes = value.as_array().unwrap();
        assert_eq!(modes.len(), 6);
        assert!(modes.iter().all(|mode| {
            mode["executionMode"] == "worker"
                && mode["environmentVariable"]
                    .as_str()
                    .is_some_and(|value| value.starts_with("BIFROST_"))
        }));
    }

    #[tokio::test]
    async fn unknown_worker_kind_is_rejected() {
        let response = handle_workers(
            Request::builder()
                .method(Method::GET)
                .uri("/_bifrost/api/workers/unknown")
                .body(())
                .unwrap(),
            "/api/workers/unknown",
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
