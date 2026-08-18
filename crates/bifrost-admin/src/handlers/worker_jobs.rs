use std::io::SeekFrom;

use hyper::{Method, Request, Response, StatusCode};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::{
    error_response, full_body, json_response, json_response_with_status, method_not_allowed,
    BoxBody,
};
use crate::worker_runtime::{
    global_worker_supervisor, worker_artifact, worker_job, worker_job_cancel_rejected,
    worker_job_cancel_target, worker_jobs, WorkerJobStatus, WorkerKind,
};

const DEFAULT_JOB_LIMIT: usize = 100;
const MAX_JOB_LIMIT: usize = 256;
const DEFAULT_ARTIFACT_READ_BYTES: u64 = 256 * 1024;
const MAX_ARTIFACT_READ_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    limit: Option<usize>,
    status: Option<String>,
    kind: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ArtifactQuery {
    offset: Option<u64>,
    limit: Option<u64>,
    tail: Option<u64>,
}

pub async fn handle_worker_jobs<B>(req: Request<B>, path: &str) -> Response<BoxBody> {
    let suffix = path
        .trim_start_matches("/api/worker-jobs")
        .trim_matches('/');
    if suffix.is_empty() {
        if req.method() != Method::GET {
            return method_not_allowed();
        }
        return list_worker_jobs(req.uri().query());
    }

    let segments = suffix.split('/').collect::<Vec<_>>();
    let job_id = segments[0];
    if job_id.is_empty() {
        return error_response(StatusCode::NOT_FOUND, "worker job not found");
    }
    match segments.as_slice() {
        [_] if req.method() == Method::GET => match worker_job(job_id) {
            Some(job) => json_response(&job),
            None => error_response(StatusCode::NOT_FOUND, "worker job not found"),
        },
        [_, "cancel"] if req.method() == Method::POST => cancel_worker_job(job_id).await,
        [_, "events"] if req.method() == Method::GET => match worker_job(job_id) {
            Some(job) => json_response(&job.events),
            None => error_response(StatusCode::NOT_FOUND, "worker job not found"),
        },
        [_, "artifacts"] if req.method() == Method::GET => match worker_job(job_id) {
            Some(job) => json_response(&job.artifacts),
            None => error_response(StatusCode::NOT_FOUND, "worker job not found"),
        },
        [_, "artifacts", artifact_id] if req.method() == Method::GET => {
            read_worker_artifact(job_id, artifact_id, req.uri().query()).await
        }
        [_] | [_, "cancel"] | [_, "events"] | [_, "artifacts"] | [_, "artifacts", _] => {
            method_not_allowed()
        }
        _ => error_response(StatusCode::NOT_FOUND, "worker job endpoint not found"),
    }
}

fn list_worker_jobs(query: Option<&str>) -> Response<BoxBody> {
    let query = query
        .and_then(|query| serde_urlencoded::from_str::<ListQuery>(query).ok())
        .unwrap_or_default();
    let limit = query
        .limit
        .unwrap_or(DEFAULT_JOB_LIMIT)
        .clamp(1, MAX_JOB_LIMIT);
    let status = match query.status.as_deref().map(parse_status).transpose() {
        Ok(status) => status,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    let kind = match query
        .kind
        .as_deref()
        .map(|value| {
            WorkerKind::parse(value).ok_or_else(|| format!("unknown worker kind '{value}'"))
        })
        .transpose()
    {
        Ok(kind) => kind,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    let jobs = worker_jobs()
        .into_iter()
        .filter(|job| status.is_none_or(|status| job.status == status))
        .filter(|job| kind.is_none_or(|kind| job.worker_kind == kind))
        .take(limit)
        .collect::<Vec<_>>();
    json_response(&jobs)
}

async fn cancel_worker_job(job_id: &str) -> Response<BoxBody> {
    let Some(job) = worker_job(job_id) else {
        return error_response(StatusCode::NOT_FOUND, "worker job not found");
    };
    let Some((worker_key, logical_job_id)) = worker_job_cancel_target(job_id) else {
        return json_response_with_status(
            StatusCode::CONFLICT,
            &serde_json::json!({
                "message": "worker job is already terminal",
                "job": job,
            }),
        );
    };

    if job.worker_kind == WorkerKind::RemoteExecution {
        match crate::worker_runtime::remote_execution::cancel_registered_execution(
            &worker_key,
            &job.request_id,
            &logical_job_id,
        )
        .await
        {
            Ok(true) => {
                return json_response_with_status(
                    StatusCode::ACCEPTED,
                    &serde_json::json!({
                        "accepted": true,
                        "jobId": job_id,
                        "workerKey": worker_key,
                    }),
                );
            }
            Ok(false) => {
                let error = "remote execution worker is no longer active".to_string();
                worker_job_cancel_rejected(job_id, error.clone());
                return json_response_with_status(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &serde_json::json!({
                        "accepted": false,
                        "jobId": job_id,
                        "workerKey": worker_key,
                        "error": error,
                    }),
                );
            }
            Err(error) => {
                worker_job_cancel_rejected(job_id, error.clone());
                return json_response_with_status(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &serde_json::json!({
                        "accepted": false,
                        "jobId": job_id,
                        "workerKey": worker_key,
                        "error": error,
                    }),
                );
            }
        }
    }

    if job.worker_kind == WorkerKind::ExternalCli {
        let stopped = match crate::im_gateway::external_cli::request_managed_session_stop(
            crate::im_gateway::external_cli::default_runs_root(),
            &logical_job_id,
        )
        .await
        {
            Ok(stopped) => stopped,
            Err(error) => {
                worker_job_cancel_rejected(job_id, error.clone());
                return json_response_with_status(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &serde_json::json!({
                        "accepted": false,
                        "jobId": job_id,
                        "workerKey": worker_key,
                        "error": error,
                    }),
                );
            }
        };
        if stopped {
            return json_response_with_status(
                StatusCode::ACCEPTED,
                &serde_json::json!({
                    "accepted": true,
                    "jobId": job_id,
                    "workerKey": worker_key,
                }),
            );
        }
        let error = "external CLI session is no longer active";
        worker_job_cancel_rejected(job_id, error);
        return json_response_with_status(
            StatusCode::SERVICE_UNAVAILABLE,
            &serde_json::json!({
                "accepted": false,
                "jobId": job_id,
                "workerKey": worker_key,
                "error": error,
            }),
        );
    }

    let Some(worker) = global_worker_supervisor().get(&worker_key).await else {
        let error = "worker is not available for cancellation";
        worker_job_cancel_rejected(job_id, error);
        return json_response_with_status(
            StatusCode::SERVICE_UNAVAILABLE,
            &serde_json::json!({
                "message": error,
                "workerKey": worker_key,
                "jobId": job_id,
            }),
        );
    };
    match worker
        .cancel_request(&job.request_id, &logical_job_id)
        .await
    {
        Ok(true) => json_response_with_status(
            StatusCode::ACCEPTED,
            &serde_json::json!({
                "accepted": true,
                "jobId": job_id,
                "workerKey": worker_key,
            }),
        ),
        Ok(false) => {
            let error = "worker request is no longer active".to_string();
            worker_job_cancel_rejected(job_id, error.clone());
            json_response_with_status(
                StatusCode::SERVICE_UNAVAILABLE,
                &serde_json::json!({
                    "accepted": false,
                    "jobId": job_id,
                    "workerKey": worker_key,
                    "error": error,
                }),
            )
        }
        Err(error) => {
            worker_job_cancel_rejected(job_id, error.clone());
            json_response_with_status(
                StatusCode::SERVICE_UNAVAILABLE,
                &serde_json::json!({
                    "accepted": false,
                    "jobId": job_id,
                    "workerKey": worker_key,
                    "error": error,
                }),
            )
        }
    }
}

async fn read_worker_artifact(
    job_id: &str,
    artifact_id: &str,
    query: Option<&str>,
) -> Response<BoxBody> {
    let Some(artifact) = worker_artifact(job_id, artifact_id) else {
        return error_response(StatusCode::NOT_FOUND, "worker artifact not found");
    };
    let query = query
        .and_then(|query| serde_urlencoded::from_str::<ArtifactQuery>(query).ok())
        .unwrap_or_default();
    let requested_limit = query
        .limit
        .or(query.tail)
        .unwrap_or(DEFAULT_ARTIFACT_READ_BYTES);
    if requested_limit == 0 || requested_limit > MAX_ARTIFACT_READ_BYTES {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!("artifact read limit must be between 1 and {MAX_ARTIFACT_READ_BYTES} bytes"),
        );
    }
    let total = artifact.size_bytes;
    let offset = query
        .tail
        .map(|tail| total.saturating_sub(tail.min(total)))
        .unwrap_or_else(|| query.offset.unwrap_or(0));
    if offset > total {
        return error_response(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "artifact offset exceeds size",
        );
    }
    let read_len = requested_limit.min(total.saturating_sub(offset));
    let canonical_root = match tokio::fs::canonicalize(artifact.root()).await {
        Ok(root) if root.is_dir() => root,
        Ok(_) => {
            return error_response(
                StatusCode::GONE,
                "worker artifact spool root is no longer a directory",
            )
        }
        Err(error) => {
            return error_response(
                StatusCode::GONE,
                &format!("worker artifact spool root is no longer available: {error}"),
            )
        }
    };
    let canonical_path = match tokio::fs::canonicalize(artifact.path()).await {
        Ok(path) if path.starts_with(&canonical_root) => path,
        Ok(_) => {
            return error_response(
                StatusCode::GONE,
                "worker artifact escaped its registered spool root",
            )
        }
        Err(error) => {
            return error_response(
                StatusCode::GONE,
                &format!("worker artifact is no longer available: {error}"),
            )
        }
    };
    let metadata = match tokio::fs::metadata(&canonical_path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => {
            return error_response(
                StatusCode::GONE,
                "worker artifact is no longer a regular file",
            )
        }
        Err(error) => {
            return error_response(
                StatusCode::GONE,
                &format!("worker artifact metadata is no longer available: {error}"),
            )
        }
    };
    if metadata.len() != total {
        return error_response(
            StatusCode::GONE,
            "worker artifact changed after it was registered",
        );
    }
    let mut file = match tokio::fs::File::open(&canonical_path).await {
        Ok(file) => file,
        Err(error) => {
            return error_response(
                StatusCode::GONE,
                &format!("worker artifact is no longer available: {error}"),
            )
        }
    };
    if let Err(error) = file.seek(SeekFrom::Start(offset)).await {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("seek worker artifact failed: {error}"),
        );
    }
    let mut bytes = vec![0_u8; read_len as usize];
    if let Err(error) = file.read_exact(&mut bytes).await {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("read worker artifact failed: {error}"),
        );
    }
    let end = offset.saturating_add(read_len);
    Response::builder()
        .status(StatusCode::OK)
        .header(
            "Content-Type",
            artifact
                .media_type
                .as_deref()
                .unwrap_or("application/octet-stream"),
        )
        .header("Accept-Ranges", "bytes")
        .header("X-Bifrost-Artifact-Offset", offset.to_string())
        .header("X-Bifrost-Artifact-End", end.to_string())
        .header("X-Bifrost-Artifact-Size", total.to_string())
        .body(full_body(bytes))
        .expect("worker artifact response")
}

fn parse_status(value: &str) -> Result<WorkerJobStatus, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "queued" => Ok(WorkerJobStatus::Queued),
        "running" => Ok(WorkerJobStatus::Running),
        "succeeded" | "success" => Ok(WorkerJobStatus::Succeeded),
        "failed" | "failure" => Ok(WorkerJobStatus::Failed),
        "cancelling" | "canceling" => Ok(WorkerJobStatus::Cancelling),
        "cancelled" | "canceled" => Ok(WorkerJobStatus::Cancelled),
        other => Err(format!("unknown worker job status '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;

    use super::*;

    #[test]
    fn parses_job_status_aliases() {
        assert_eq!(parse_status("success"), Ok(WorkerJobStatus::Succeeded));
        assert_eq!(parse_status("canceled"), Ok(WorkerJobStatus::Cancelled));
        assert!(parse_status("unknown").is_err());
    }

    #[tokio::test]
    async fn job_list_can_filter_by_kind_and_status() {
        let _guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();
        crate::worker_runtime::begin_worker_job(
            "asr:offline-jobs",
            WorkerKind::Asr,
            "list-request",
            Some("task-list"),
            "asr.run_directory_task",
        );
        crate::worker_runtime::mark_worker_job_running("list-request");
        let response = handle_worker_jobs(
            Request::builder()
                .method(Method::GET)
                .uri("/_bifrost/api/worker-jobs?kind=asr&status=running")
                .body(())
                .unwrap(),
            "/api/worker-jobs",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let jobs: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(jobs.as_array().unwrap().len(), 1);
        assert_eq!(jobs[0]["id"], "list-request");
    }

    #[tokio::test]
    async fn rejected_external_cli_cancel_restores_running_status() {
        let _guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();
        let job_id = format!("missing-external-cli-{}", uuid::Uuid::new_v4());
        crate::worker_runtime::begin_worker_job(
            &format!("external_cli:{job_id}"),
            WorkerKind::ExternalCli,
            &job_id,
            Some(&job_id),
            "external_cli.run",
        );
        crate::worker_runtime::mark_worker_job_running(&job_id);

        let response = handle_worker_jobs(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/_bifrost/api/worker-jobs/{job_id}/cancel"))
                .body(())
                .unwrap(),
            &format!("/api/worker-jobs/{job_id}/cancel"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let job = worker_job(&job_id).unwrap();
        assert_eq!(job.status, WorkerJobStatus::Running);
        assert_eq!(job.events.last().unwrap().event, "cancel_rejected");
    }

    #[tokio::test]
    async fn invalid_external_cli_cancel_reports_broker_error_and_restores_status() {
        let _guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();
        let job_id = format!("invalid-external-cli-{}", uuid::Uuid::new_v4());
        crate::worker_runtime::begin_worker_job(
            &format!("external_cli:{job_id}"),
            WorkerKind::ExternalCli,
            &job_id,
            Some(""),
            "external_cli.run",
        );
        crate::worker_runtime::mark_worker_job_running(&job_id);

        let path = format!("/api/worker-jobs/{job_id}/cancel");
        let response = request(Method::POST, &path, &path).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["accepted"], false);
        assert_eq!(payload["jobId"], job_id);
        assert_eq!(payload["error"], "session_key cannot be empty");

        let job = worker_job(&job_id).unwrap();
        assert_eq!(job.status, WorkerJobStatus::Running);
        assert_eq!(job.events.last().unwrap().event, "cancel_rejected");
    }

    #[tokio::test]
    async fn artifact_identifier_cannot_escape_the_job_registry() {
        let _guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();
        crate::worker_runtime::begin_worker_job(
            "asr:artifact-test",
            WorkerKind::Asr,
            "artifact-test",
            Some("artifact-test"),
            "asr.test",
        );
        let response = read_worker_artifact("artifact-test", "../../etc/passwd", None).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let _ = response.into_body().collect().await.unwrap();
    }

    #[tokio::test]
    async fn unknown_job_returns_not_found() {
        let _guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        let response = handle_worker_jobs(
            Request::builder()
                .method(Method::GET)
                .uri("/_bifrost/api/worker-jobs/missing")
                .body(())
                .unwrap(),
            "/api/worker-jobs/missing",
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let _ = response.into_body().collect().await.unwrap();
    }

    async fn request(method: Method, uri: &str, path: &str) -> Response<BoxBody> {
        handle_worker_jobs(
            Request::builder().method(method).uri(uri).body(()).unwrap(),
            path,
        )
        .await
    }

    fn begin_job(id: &str, kind: WorkerKind) {
        crate::worker_runtime::begin_worker_job(
            &format!("{}:{id}", kind.as_str()),
            kind,
            id,
            Some(id),
            "coverage.operation",
        );
    }

    #[tokio::test]
    async fn endpoint_validation_and_inactive_cancellation_matrix() {
        let _guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();

        assert_eq!(
            request(Method::POST, "/worker-jobs", "/api/worker-jobs")
                .await
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        for query in ["status=bogus", "kind=bogus"] {
            assert_eq!(
                request(
                    Method::GET,
                    &format!("/worker-jobs?{query}"),
                    "/api/worker-jobs",
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST
            );
        }
        assert_eq!(
            request(
                Method::GET,
                "/worker-jobs/missing/events",
                "/api/worker-jobs/missing/events",
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(
                Method::GET,
                "/worker-jobs/missing/artifacts",
                "/api/worker-jobs/missing/artifacts",
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(
                Method::POST,
                "/worker-jobs/missing/cancel",
                "/api/worker-jobs/missing/cancel",
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(
                Method::POST,
                "/worker-jobs/missing/events",
                "/api/worker-jobs/missing/events",
            )
            .await
            .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            request(
                Method::GET,
                "/worker-jobs/missing/extra/path",
                "/api/worker-jobs/missing/extra/path",
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );

        begin_job("terminal", WorkerKind::Asr);
        crate::worker_runtime::mark_worker_job_succeeded("terminal");
        assert_eq!(
            request(
                Method::POST,
                "/worker-jobs/terminal/cancel",
                "/api/worker-jobs/terminal/cancel",
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );

        begin_job("inactive-asr", WorkerKind::Asr);
        crate::worker_runtime::mark_worker_job_running("inactive-asr");
        assert_eq!(
            request(
                Method::POST,
                "/worker-jobs/inactive-asr/cancel",
                "/api/worker-jobs/inactive-asr/cancel",
            )
            .await
            .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );

        begin_job("inactive-remote-exec", WorkerKind::RemoteExecution);
        crate::worker_runtime::mark_worker_job_running("inactive-remote-exec");
        assert_eq!(
            request(
                Method::POST,
                "/worker-jobs/inactive-remote-exec/cancel",
                "/api/worker-jobs/inactive-remote-exec/cancel",
            )
            .await
            .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn active_worker_cancellation_reports_acknowledged_and_stale_requests() {
        let _guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();
        let key = format!("asr:cancel-handler-{}", uuid::Uuid::new_v4());
        let tail = r#"
wait_id=''
while IFS= read -r line; do
  case "$line" in
    *'"type":"request"'*)
      wait_id=$(printf '%s' "$line" | sed -n -e 's/.*"requestId":"\([^"]*\)".*/\1/p' -e 's/.*"request_id":"\([^"]*\)".*/\1/p')
      ;;
    *'"type":"cancel"'*)
      printf '{"type":"response","response":{"requestId":"%s","ok":false,"cancelled":true,"payload":null,"error":"cancel acknowledged"}}\n' "$wait_id"
      wait_id=''
      ;;
    *'"type":"shutdown"'*)
      printf '{"type":"goodbye","worker_instance_id":"fake-instance","reason":"shutdown acknowledged"}\n'
      exit 0
      ;;
  esac
done
"#;
        let mut spec = crate::worker_runtime::test_shell_worker_spec(&key, WorkerKind::Asr, tail);
        spec.request_timeout = std::time::Duration::from_secs(2);
        spec.heartbeat_timeout = std::time::Duration::from_secs(3);
        let supervisor = global_worker_supervisor();
        supervisor.resume_kind(WorkerKind::Asr);
        let worker = supervisor.get_or_start(spec).await.unwrap();
        let request_id = format!("cancel-request-{}", uuid::Uuid::new_v4());
        let request_job_id = request_id.clone();
        let pending_job_id = request_job_id.clone();
        let request_worker = worker.clone();
        let pending = tokio::spawn(async move {
            request_worker
                .request_with_id(
                    request_id,
                    Some(pending_job_id),
                    "operation.wait",
                    serde_json::Value::Null,
                    None,
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while worker_job(&request_job_id)
                .is_none_or(|job| job.status != WorkerJobStatus::Running)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let path = format!("/api/worker-jobs/{request_job_id}/cancel");
        assert_eq!(
            request(Method::POST, &path, &path).await.status(),
            StatusCode::ACCEPTED
        );
        assert_eq!(pending.await.unwrap().unwrap_err(), "cancel acknowledged");

        let stale_id = format!("stale-request-{}", uuid::Uuid::new_v4());
        crate::worker_runtime::begin_worker_job(
            &key,
            WorkerKind::Asr,
            &stale_id,
            Some(&stale_id),
            "operation.wait",
        );
        crate::worker_runtime::mark_worker_job_running(&stale_id);
        let stale_path = format!("/api/worker-jobs/{stale_id}/cancel");
        assert_eq!(
            request(Method::POST, &stale_path, &stale_path)
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            worker_job(&stale_id).unwrap().status,
            WorkerJobStatus::Running
        );
        supervisor
            .unregister(&key, std::time::Duration::from_secs(1))
            .await;
    }

    #[tokio::test]
    async fn artifact_reads_cover_ranges_and_filesystem_invalidation() {
        let _guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("spool");
        std::fs::create_dir(&root).unwrap();
        let path = root.join("result.txt");
        std::fs::write(&path, b"0123456789").unwrap();
        begin_job("artifact-matrix", WorkerKind::ExternalCli);
        let artifact = crate::worker_runtime::register_worker_artifact(
            "artifact-matrix",
            "result",
            &root,
            &path,
            Some("text/plain".to_string()),
        )
        .unwrap();

        let endpoint = format!(
            "/api/worker-jobs/artifact-matrix/artifacts/{}",
            artifact.artifact_id
        );
        let response = read_worker_artifact(
            "artifact-matrix",
            &artifact.artifact_id,
            Some("offset=2&limit=4"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "2345"
        );
        assert_eq!(
            read_worker_artifact("artifact-matrix", &artifact.artifact_id, Some("tail=3"),)
                .await
                .status(),
            StatusCode::OK
        );
        for query in ["limit=0", "limit=1048577"] {
            assert_eq!(
                read_worker_artifact("artifact-matrix", &artifact.artifact_id, Some(query))
                    .await
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
        assert_eq!(
            read_worker_artifact("artifact-matrix", &artifact.artifact_id, Some("offset=11"),)
                .await
                .status(),
            StatusCode::RANGE_NOT_SATISFIABLE
        );
        assert_eq!(
            request(Method::POST, &endpoint, &endpoint,).await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );

        std::fs::write(&path, b"changed-size").unwrap();
        assert_eq!(
            read_worker_artifact("artifact-matrix", &artifact.artifact_id, None)
                .await
                .status(),
            StatusCode::GONE
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            read_worker_artifact("artifact-matrix", &artifact.artifact_id, None)
                .await
                .status(),
            StatusCode::GONE
        );
        std::fs::create_dir(&path).unwrap();
        assert_eq!(
            read_worker_artifact("artifact-matrix", &artifact.artifact_id, None)
                .await
                .status(),
            StatusCode::GONE
        );
        std::fs::remove_dir(&path).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};

            let outside = temp.path().join("outside.txt");
            std::fs::write(&outside, b"0123456789").unwrap();
            symlink(&outside, &path).unwrap();
            assert_eq!(
                read_worker_artifact("artifact-matrix", &artifact.artifact_id, None)
                    .await
                    .status(),
                StatusCode::GONE
            );
            std::fs::remove_file(&path).unwrap();

            std::fs::write(&path, b"0123456789").unwrap();
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o000);
            std::fs::set_permissions(&path, permissions).unwrap();
            assert_eq!(
                read_worker_artifact("artifact-matrix", &artifact.artifact_id, None)
                    .await
                    .status(),
                StatusCode::GONE
            );
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o600);
            std::fs::set_permissions(&path, permissions).unwrap();
            std::fs::remove_file(&path).unwrap();
        }

        std::fs::remove_dir(&root).unwrap();
        std::fs::write(&root, b"not-a-directory").unwrap();
        assert_eq!(
            read_worker_artifact("artifact-matrix", &artifact.artifact_id, None)
                .await
                .status(),
            StatusCode::GONE
        );
        std::fs::remove_file(&root).unwrap();
        assert_eq!(
            read_worker_artifact("artifact-matrix", &artifact.artifact_id, None)
                .await
                .status(),
            StatusCode::GONE
        );
    }
}
