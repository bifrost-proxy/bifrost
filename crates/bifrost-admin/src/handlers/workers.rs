use std::time::Duration;

use hyper::{Method, Request, Response, StatusCode};
use serde::Serialize;

use super::{error_response, json_response, method_not_allowed, BoxBody};
use crate::worker_runtime::{
    execution_mode, execution_mode_env, global_worker_supervisor, WorkerKind, WorkerSupervisor,
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
    let supervisor = global_worker_supervisor();
    handle_workers_with_supervisor(req, path, &supervisor).await
}

async fn handle_workers_with_supervisor<B>(
    req: Request<B>,
    path: &str,
    supervisor: &WorkerSupervisor,
) -> Response<BoxBody> {
    let suffix = path.trim_start_matches("/api/workers").trim_matches('/');
    if suffix.is_empty() {
        return match *req.method() {
            Method::GET => json_response(&supervisor.snapshots().await),
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
            let snapshots = supervisor
                .snapshots()
                .await
                .into_iter()
                .filter(|snapshot| snapshot.worker_kind == kind)
                .collect::<Vec<_>>();
            json_response(&snapshots)
        }
        (&Method::POST, Some("start")) => {
            let results = supervisor.start_kind(kind).await;
            if results.is_empty() {
                if kind == WorkerKind::ExternalCli {
                    return json_response(&WorkerActionResponse {
                        worker_kind: kind,
                        action: "start",
                        affected: 0,
                        errors: Vec::new(),
                    });
                }
                return error_response(
                    StatusCode::CONFLICT,
                    "worker kind has no registered startup specification; use the capability once before starting it manually",
                );
            }
            action_response(kind, "start", results)
        }
        (&Method::POST, Some("stop")) => {
            let mut affected = supervisor.suspend_kind(kind, Duration::from_secs(5)).await;
            if kind == WorkerKind::ExternalCli {
                affected = affected.saturating_add(
                    crate::im_gateway::external_cli::stop_all_worker_sessions().await,
                );
            }
            json_response(&WorkerActionResponse {
                worker_kind: kind,
                action: "stop",
                affected,
                errors: Vec::new(),
            })
        }
        (&Method::POST, Some("restart")) => {
            let results = supervisor.restart_kind(kind).await;
            if results.is_empty() {
                return error_response(
                    StatusCode::CONFLICT,
                    "worker kind has no registered restart specification",
                );
            }
            action_response(kind, "restart", results)
        }
        (&Method::POST, Some("reset-circuit")) => {
            let affected = supervisor.reset_circuit_kind(kind).await;
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

    async fn request(method: Method, path: &str) -> Response<BoxBody> {
        handle_workers(
            Request::builder()
                .method(method)
                .uri(path)
                .body(())
                .unwrap(),
            path,
        )
        .await
    }

    #[tokio::test]
    async fn worker_routes_reject_invalid_methods_and_shapes() {
        assert_eq!(
            request(Method::POST, "/api/workers").await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            request(Method::POST, "/api/workers/modes").await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            request(Method::GET, "/api/workers/browser").await.status(),
            StatusCode::OK
        );
        assert_eq!(
            request(Method::GET, "/api/workers/browser/extra/path")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(Method::POST, "/api/workers/browser").await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            request(Method::GET, "/api/workers/browser/start")
                .await
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            request(Method::DELETE, "/api/workers/browser/reset-circuit")
                .await
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }

    #[tokio::test]
    async fn action_response_reports_success_partial_and_total_failure() {
        let success = action_response(
            WorkerKind::Asr,
            "start",
            vec![("asr:test".to_string(), Ok(()))],
        );
        assert_eq!(success.status(), StatusCode::OK);

        let partial = action_response(
            WorkerKind::ImGateway,
            "restart",
            vec![
                ("im:ok".to_string(), Ok(())),
                ("im:failed".to_string(), Err("boom".to_string())),
            ],
        );
        assert_eq!(partial.status(), StatusCode::OK);
        let body = partial.into_body().collect().await.unwrap().to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["affected"], 1);
        assert_eq!(value["errors"][0]["error"], "boom");

        let failed = action_response(
            WorkerKind::RemoteInvoke,
            "start",
            vec![("remote:test".to_string(), Err("offline".to_string()))],
        );
        assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn unregistered_worker_actions_return_stable_operator_responses() {
        async fn local_request(
            supervisor: &WorkerSupervisor,
            method: Method,
            path: &str,
        ) -> Response<BoxBody> {
            handle_workers_with_supervisor(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(())
                    .unwrap(),
                path,
                supervisor,
            )
            .await
        }

        let supervisor = WorkerSupervisor::new();
        let external_start =
            local_request(&supervisor, Method::POST, "/api/workers/external-cli/start").await;
        assert_eq!(external_start.status(), StatusCode::OK);
        let body = external_start
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["affected"], 0);
        assert!(value["errors"].as_array().unwrap().is_empty());

        assert_eq!(
            local_request(&supervisor, Method::POST, "/api/workers/browser/start")
                .await
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            local_request(&supervisor, Method::POST, "/api/workers/browser/restart",)
                .await
                .status(),
            StatusCode::CONFLICT
        );

        let reset = local_request(
            &supervisor,
            Method::POST,
            "/api/workers/browser/reset-circuit",
        )
        .await;
        assert_eq!(reset.status(), StatusCode::OK);
        let body = reset.into_body().collect().await.unwrap().to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["action"], "reset_circuit");
        assert_eq!(value["affected"], 0);

        let stop = local_request(&supervisor, Method::POST, "/api/workers/browser/stop").await;
        assert_eq!(stop.status(), StatusCode::OK);
        assert!(supervisor.is_kind_suspended(WorkerKind::Browser));
        supervisor.resume_kind(WorkerKind::Browser);

        let external_stop =
            local_request(&supervisor, Method::POST, "/api/workers/external-cli/stop").await;
        assert_eq!(external_stop.status(), StatusCode::OK);
        supervisor.resume_kind(WorkerKind::ExternalCli);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn registered_worker_kind_can_be_stopped_started_and_restarted() {
        async fn local_request(supervisor: &WorkerSupervisor, path: &str) -> Response<BoxBody> {
            handle_workers_with_supervisor(
                Request::builder()
                    .method(Method::POST)
                    .uri(path)
                    .body(())
                    .unwrap(),
                path,
                supervisor,
            )
            .await
        }

        let supervisor = WorkerSupervisor::new();
        let key = "asr:operator-actions";
        let tail = r#"
while IFS= read -r line; do
  case "$line" in
    *'"type":"shutdown"'*)
      printf '{"type":"goodbye","worker_instance_id":"fake-instance","reason":"operator action"}\n'
      exit 0
      ;;
  esac
done
"#;
        let mut spec = crate::worker_runtime::test_shell_worker_spec(key, WorkerKind::Asr, tail);
        spec.heartbeat_timeout = Duration::from_secs(5);
        supervisor.get_or_start(spec).await.unwrap();

        for action in ["stop", "start", "restart"] {
            let path = format!("/api/workers/asr/{action}");
            assert_eq!(
                local_request(&supervisor, &path).await.status(),
                StatusCode::OK,
                "worker action {action}"
            );
        }
        assert!(supervisor.unregister(key, Duration::from_secs(1)).await);
    }
}
