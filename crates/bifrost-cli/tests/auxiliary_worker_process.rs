use std::time::Duration;

use base64::Engine;
use bifrost_admin::remote_invoke::types::{CommandKind, RemoteCommand, ShellExecMode};
use bifrost_admin::remote_invoke::RemoteInvokeResponse;
use bifrost_admin::worker_runtime::{
    ManagedWorker, WorkerKind, WorkerLifecycleState, WorkerSpawnSpec, WorkerSupervisor,
};
use bifrost_storage::{RemoteShellPolicy, RemoteShellSet, RemoteShellStore};

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auxiliary_worker_shell_e2e_runs_against_the_instrumented_binary() {
    let repository_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("resolve repository root");
    let script = repository_root.join("e2e-tests/tests/test_auxiliary_worker_isolation.sh");
    let mut command = tokio::process::Command::new("bash");
    command
        .arg(script)
        .current_dir(repository_root)
        .env("BIFROST_BIN", env!("CARGO_BIN_EXE_bifrost"))
        .env("SKIP_BUILD", "true")
        .kill_on_drop(true);

    let output = tokio::time::timeout(Duration::from_secs(240), command.output())
        .await
        .expect("auxiliary worker shell E2E timed out")
        .expect("execute auxiliary worker shell E2E");
    assert!(
        output.status.success(),
        "auxiliary worker shell E2E failed (status={}):\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn asr_worker_spec(key: &str, data_dir: &std::path::Path) -> WorkerSpawnSpec {
    let mut spec = WorkerSpawnSpec::new(
        key,
        WorkerKind::Asr,
        env!("CARGO_BIN_EXE_bifrost"),
        vec![
            "auxiliary-worker".to_string(),
            "--kind".to_string(),
            "asr".to_string(),
            "--data-dir".to_string(),
            data_dir.display().to_string(),
            "--admin-host".to_string(),
            "127.0.0.1".to_string(),
            "--admin-port".to_string(),
            "0".to_string(),
        ],
    );
    spec.startup_timeout = Duration::from_secs(15);
    spec.request_timeout = Duration::from_secs(5);
    spec.heartbeat_timeout = Duration::from_secs(30);
    spec.stderr_path = Some(data_dir.join("asr-worker.stderr.log"));
    spec
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_asr_worker_starts_handles_request_and_shuts_down() {
    let temp = tempfile::tempdir().expect("create worker test data dir");
    let spec = asr_worker_spec("test:asr-worker", temp.path());

    let worker = ManagedWorker::spawn(spec)
        .await
        .expect("start real ASR worker process");
    assert_eq!(worker.kind(), WorkerKind::Asr);
    assert_eq!(worker.state(), WorkerLifecycleState::Ready);
    assert!(worker.pid().is_some());
    assert!(!worker.instance_id().is_empty());
    assert!(worker.is_healthy().await);
    let snapshot = worker.snapshot(2, Some(10), Some(20));
    assert_eq!(snapshot.key, "test:asr-worker");
    assert_eq!(snapshot.restart_count, 2);
    assert_eq!(snapshot.backoff_until_ms, Some(10));
    assert_eq!(snapshot.circuit_open_until_ms, Some(20));

    let error = worker
        .request(
            "asr.unsupported_test_operation",
            serde_json::json!({}),
            Some(Duration::from_secs(5)),
        )
        .await
        .expect_err("unsupported operation should be returned by the worker");
    assert!(
        error.contains("unsupported ASR worker operation"),
        "{error}"
    );

    worker
        .shutdown(Duration::from_secs(5))
        .await
        .expect("shutdown real ASR worker process");
    assert_eq!(worker.state(), WorkerLifecycleState::Stopped);
    worker
        .shutdown(Duration::ZERO)
        .await
        .expect("repeated shutdown is idempotent");
    assert!(worker
        .request(
            "asr.unsupported_test_operation",
            serde_json::json!({}),
            Some(Duration::from_millis(50)),
        )
        .await
        .expect_err("stopped worker must reject requests")
        .contains("not ready"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_remote_invoke_worker_exposes_only_token_guarded_loopback_http() {
    let temp = tempfile::tempdir().expect("create Remote Invoke worker data dir");
    let http_token = "integration-http-token";
    let mut spec = WorkerSpawnSpec::new(
        "test:remote-invoke-worker",
        WorkerKind::RemoteInvoke,
        env!("CARGO_BIN_EXE_bifrost"),
        vec![
            "auxiliary-worker".to_string(),
            "--kind".to_string(),
            "remote_invoke".to_string(),
            "--data-dir".to_string(),
            temp.path().display().to_string(),
            "--admin-host".to_string(),
            "127.0.0.1".to_string(),
            "--admin-port".to_string(),
            "0".to_string(),
        ],
    );
    spec.env
        .insert("BIFROST_REMOTE_INVOKE_WORKER".to_string(), "1".to_string());
    spec.env.insert(
        "BIFROST_REMOTE_RELAY_URL".to_string(),
        "http://127.0.0.1:9".to_string(),
    );
    spec.env.insert(
        "BIFROST_REMOTE_SESSION_TOKEN".to_string(),
        "integration-session".to_string(),
    );
    spec.env.insert(
        "BIFROST_REMOTE_WORKER_HTTP_TOKEN".to_string(),
        http_token.to_string(),
    );
    spec.env.insert(
        "BIFROST_REMOTE_EXECUTION_BROKER_ADDR".to_string(),
        "127.0.0.1:9".to_string(),
    );
    spec.env.insert(
        "BIFROST_REMOTE_EXECUTION_BROKER_TOKEN".to_string(),
        "integration-broker-token".to_string(),
    );
    spec.env.insert(
        "BIFROST_REMOTE_EXECUTION_BROKER_RELAY".to_string(),
        "http://127.0.0.1:9".to_string(),
    );
    spec.startup_timeout = Duration::from_secs(15);
    spec.request_timeout = Duration::from_secs(10);
    spec.heartbeat_timeout = Duration::from_secs(30);
    spec.stderr_path = Some(temp.path().join("remote-invoke.stderr.log"));

    let worker = ManagedWorker::spawn(spec)
        .await
        .expect("start real Remote Invoke worker process");
    let endpoint = worker
        .request(
            "remote.endpoint",
            serde_json::Value::Null,
            Some(Duration::from_secs(5)),
        )
        .await
        .expect("read Remote Invoke worker HTTP endpoint");
    let port = endpoint["port"].as_u64().expect("endpoint port") as u16;
    assert!(port > 0);

    let runtime = worker
        .request(
            "remote.runtime_status",
            serde_json::Value::Null,
            Some(Duration::from_secs(5)),
        )
        .await
        .expect("read Remote Invoke runtime status");
    assert_eq!(runtime["relayUrl"], "http://127.0.0.1:9");
    assert!(runtime["activeCallIds"].as_array().unwrap().is_empty());

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build loopback HTTP client");
    let base = format!("http://127.0.0.1:{port}/api/remote-invoke");
    let forbidden = client
        .get(format!("{base}/status"))
        .send()
        .await
        .expect("request without worker token");
    assert_eq!(forbidden.status(), reqwest::StatusCode::FORBIDDEN);
    for path in ["status", "identity"] {
        let response = client
            .get(format!("{base}/{path}"))
            .header("x-bifrost-worker-token", http_token)
            .send()
            .await
            .expect("request token-guarded Remote Invoke endpoint");
        assert_eq!(response.status(), reqwest::StatusCode::OK, "{path}");
        assert!(response
            .json::<serde_json::Value>()
            .await
            .unwrap()
            .is_object());
    }
    let missing = client
        .get(format!("{base}/missing"))
        .header("x-bifrost-worker-token", http_token)
        .send()
        .await
        .expect("request missing Remote Invoke endpoint");
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);

    let error = worker
        .request(
            "remote.unsupported_test_operation",
            serde_json::Value::Null,
            Some(Duration::from_secs(5)),
        )
        .await
        .expect_err("unsupported Remote Invoke operation must fail");
    assert!(error.contains("unsupported Remote Invoke worker operation"));
    worker
        .shutdown(Duration::from_secs(5))
        .await
        .expect("shutdown Remote Invoke worker");
    assert_eq!(worker.state(), WorkerLifecycleState::Stopped);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_im_gateway_worker_owns_runtime_state_and_provider_failures() {
    let temp = tempfile::tempdir().expect("create IM Gateway worker data dir");
    let mut spec = WorkerSpawnSpec::new(
        "test:im-gateway-worker",
        WorkerKind::ImGateway,
        env!("CARGO_BIN_EXE_bifrost"),
        vec![
            "auxiliary-worker".to_string(),
            "--kind".to_string(),
            "im_gateway".to_string(),
            "--data-dir".to_string(),
            temp.path().display().to_string(),
            "--admin-host".to_string(),
            "127.0.0.1".to_string(),
            "--admin-port".to_string(),
            "0".to_string(),
        ],
    );
    spec.env
        .insert("BIFROST_IM_GATEWAY_WORKER".to_string(), "1".to_string());
    spec.env.insert(
        "BIFROST_IM_AGENT_BROKER_ADDR".to_string(),
        "127.0.0.1:9".to_string(),
    );
    spec.env.insert(
        "BIFROST_IM_AGENT_BROKER_TOKEN".to_string(),
        "integration-im-broker-token".to_string(),
    );
    spec.startup_timeout = Duration::from_secs(15);
    spec.request_timeout = Duration::from_secs(10);
    spec.heartbeat_timeout = Duration::from_secs(30);
    spec.stderr_path = Some(temp.path().join("im-gateway.stderr.log"));

    let worker = ManagedWorker::spawn(spec)
        .await
        .expect("start real IM Gateway worker process");
    let runtime = worker
        .request(
            "im.runtime_status",
            serde_json::Value::Null,
            Some(Duration::from_secs(5)),
        )
        .await
        .expect("read empty IM Gateway runtime status");
    assert!(runtime["providers"].as_array().unwrap().is_empty());

    for operation in [
        "im.connect_provider",
        "im.disconnect_provider",
        "im.provider_status",
    ] {
        let error = worker
            .request(
                operation,
                serde_json::json!({"providerId": "missing-provider"}),
                Some(Duration::from_secs(5)),
            )
            .await
            .expect_err("missing provider must fail inside isolated worker");
        assert!(error.contains("not found"), "{operation}: {error}");
    }

    let send_request_dir = temp.path().join("runtime/im-gateway-worker/requests");
    std::fs::create_dir_all(&send_request_dir).expect("create IM send request spool");
    let send_request_path = send_request_dir.join("send-integration.json");
    std::fs::write(
        &send_request_path,
        serde_json::to_vec(&serde_json::json!({
            "provider_id": "missing-provider",
            "msg_type": "text",
            "text": "must execute in the isolated IM worker"
        }))
        .unwrap(),
    )
    .expect("write IM send request spool");
    let send_response = worker
        .request(
            "im.send_message",
            serde_json::json!({"requestPath": send_request_path}),
            Some(Duration::from_secs(5)),
        )
        .await
        .expect("IM worker returns the provider error as an HTTP response envelope");
    assert_eq!(send_response["status"], 404);
    let response_body = base64::engine::general_purpose::STANDARD
        .decode(send_response["bodyBase64"].as_str().unwrap())
        .unwrap();
    let response_body = String::from_utf8(response_body).unwrap();
    assert!(
        response_body.contains("Provider 'missing-provider' not found"),
        "unexpected IM worker send response: {response_body}"
    );
    assert!(!send_request_path.exists());

    let upload_body_path = send_request_dir.join("upload-integration.bin");
    std::fs::write(&upload_body_path, b"isolated-upload").expect("write IM upload spool");
    let upload_response = worker
        .request(
            "im.upload_message",
            serde_json::json!({
                "bodyPath": upload_body_path,
                "providerId": "missing-provider",
                "kind": "file",
                "fileName": "isolated.txt",
                "mimeType": "text/plain",
                "imageType": "message"
            }),
            Some(Duration::from_secs(5)),
        )
        .await
        .expect("IM worker returns upload validation as an HTTP response envelope");
    assert_eq!(upload_response["status"], 404);
    assert!(!upload_body_path.exists());

    let bad_payload = worker
        .request(
            "im.provider_status",
            serde_json::json!({}),
            Some(Duration::from_secs(5)),
        )
        .await
        .expect_err("provider payload without providerId must fail");
    assert!(bad_payload.contains("parse IM provider worker request"));
    let unsupported = worker
        .request(
            "im.unsupported_test_operation",
            serde_json::Value::Null,
            Some(Duration::from_secs(5)),
        )
        .await
        .expect_err("unsupported IM Gateway operation must fail");
    assert!(unsupported.contains("unsupported IM Gateway worker operation"));

    worker
        .shutdown(Duration::from_secs(5))
        .await
        .expect("shutdown IM Gateway worker");
    assert_eq!(worker.state(), WorkerLifecycleState::Stopped);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervisor_manages_real_worker_lifecycle_and_rejects_bad_handshake() {
    let temp = tempfile::tempdir().expect("create supervisor test data dir");
    let supervisor = WorkerSupervisor::new();
    let spec = asr_worker_spec("test:supervised-asr", temp.path());

    assert!(supervisor.get("missing").await.is_none());
    assert!(!supervisor.stop("missing", Duration::ZERO).await);
    assert!(!supervisor.reset_circuit("missing").await);
    assert!(!supervisor.unregister("missing", Duration::ZERO).await);
    assert!(supervisor.restart_key("missing").await.is_err());

    let first = supervisor
        .get_or_start(spec.clone())
        .await
        .expect("start supervised ASR worker");
    let reused = supervisor
        .get_or_start(spec.clone())
        .await
        .expect("reuse healthy supervised worker");
    assert!(std::sync::Arc::ptr_eq(&first, &reused));
    assert!(std::sync::Arc::ptr_eq(
        &first,
        &supervisor.get("test:supervised-asr").await.unwrap()
    ));
    let snapshots = supervisor.snapshots().await;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].state, WorkerLifecycleState::Ready);

    assert!(
        supervisor
            .stop("test:supervised-asr", Duration::from_secs(5))
            .await
    );
    assert!(!supervisor.stop("test:supervised-asr", Duration::ZERO).await);
    assert!(supervisor.get("test:supervised-asr").await.is_none());
    assert_eq!(
        supervisor.snapshots().await[0].state,
        WorkerLifecycleState::Stopped
    );

    let restarted = supervisor
        .restart_key("test:supervised-asr")
        .await
        .expect("restart registered worker");
    assert!(restarted.is_healthy().await);
    assert_eq!(supervisor.reset_circuit_kind(WorkerKind::Asr).await, 1);
    assert_eq!(
        supervisor
            .suspend_kind(WorkerKind::Asr, Duration::from_secs(5))
            .await,
        1
    );
    assert!(supervisor.is_kind_suspended(WorkerKind::Asr));
    assert!(supervisor.get_or_start(spec.clone()).await.is_err());
    let started = supervisor.start_kind(WorkerKind::Asr).await;
    assert_eq!(started.len(), 1);
    assert!(started[0].1.is_ok());
    assert!(!supervisor.is_kind_suspended(WorkerKind::Asr));
    let restarted = supervisor.restart_kind(WorkerKind::Asr).await;
    assert_eq!(restarted.len(), 1);
    assert!(restarted[0].1.is_ok());
    assert_eq!(
        supervisor
            .stop_kind(WorkerKind::Asr, Duration::from_secs(5))
            .await,
        1
    );
    assert!(
        supervisor
            .unregister("test:supervised-asr", Duration::ZERO)
            .await
    );
    assert!(supervisor.snapshots().await.is_empty());

    let mut bad_spec = asr_worker_spec("test:bad-handshake", temp.path());
    bad_spec.kind = WorkerKind::Browser;
    let error = match ManagedWorker::spawn(bad_spec).await {
        Ok(worker) => {
            let _ = worker.shutdown(Duration::ZERO).await;
            panic!("worker kind mismatch must reject startup");
        }
        Err(error) => error,
    };
    assert!(error.contains("hello validation failed"), "{error}");
    supervisor.stop_all(Duration::ZERO).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_execution_worker_streams_stdout_and_exits_cleanly() {
    let temp = tempfile::tempdir().expect("create remote execution test data dir");
    let metadata = serde_json::json!({
        "exec_mode": "shell_text",
        "allowed_shell_patterns": ["^(?s:.*)$"],
        "max_timeout_ms": 10_000
    });
    #[cfg(windows)]
    let metadata = {
        let mut metadata = metadata;
        metadata["shell"] =
            serde_json::json!(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
        metadata
    };
    RemoteShellStore::with_file(temp.path().join("remote_shell.json"))
        .expect("create remote shell store")
        .save(&RemoteShellSet {
            schema_version: 1,
            version: 1,
            policies: vec![RemoteShellPolicy {
                id: "integration-shell".to_string(),
                name: "integration-shell".to_string(),
                description: None,
                enabled: true,
                profile_id: None,
                metadata,
            }],
            profiles: Vec::new(),
        })
        .expect("save remote shell policy");

    let mut spec = WorkerSpawnSpec::new(
        "test:remote-execution-worker",
        WorkerKind::RemoteExecution,
        env!("CARGO_BIN_EXE_bifrost"),
        vec![
            "auxiliary-worker".to_string(),
            "--kind".to_string(),
            "remote_execution".to_string(),
            "--data-dir".to_string(),
            temp.path().display().to_string(),
            "--admin-host".to_string(),
            "127.0.0.1".to_string(),
            "--admin-port".to_string(),
            "0".to_string(),
        ],
    );
    spec.startup_timeout = Duration::from_secs(15);
    spec.request_timeout = Duration::from_secs(15);
    spec.heartbeat_timeout = Duration::from_secs(30);
    spec.max_concurrency = 4;
    spec.stderr_path = Some(temp.path().join("remote-execution.stderr.log"));

    let worker = ManagedWorker::spawn(spec)
        .await
        .expect("start real Remote Execution worker process");
    let mut events = worker.subscribe_events();
    let execution_id = "integration-execution";
    worker
        .request(
            "remote_execution.prepare",
            serde_json::json!({ "executionId": execution_id }),
            Some(Duration::from_secs(5)),
        )
        .await
        .expect("prepare remote execution input");
    worker
        .request(
            "remote_execution.stdin_close",
            serde_json::json!({ "executionId": execution_id }),
            Some(Duration::from_secs(5)),
        )
        .await
        .expect("close remote execution stdin");

    #[cfg(windows)]
    let command_text = "[Console]::Write('remote-execution-ok')";
    #[cfg(not(windows))]
    let command_text = "printf remote-execution-ok";
    let command = RemoteCommand {
        kind: CommandKind::ShellExec,
        policy_id: Some("integration-shell".to_string()),
        exec_mode: Some(ShellExecMode::ShellText),
        command_text: Some(command_text.to_string()),
        timeout_ms: Some(10_000),
        ..Default::default()
    };
    let response_value = worker
        .request_with_id(
            "integration-run-request".to_string(),
            Some(execution_id.to_string()),
            "remote_execution.run",
            serde_json::json!({
                "command": command,
                "fileAccess": "none"
            }),
            Some(Duration::from_secs(15)),
        )
        .await
        .expect("execute remote shell command in isolated worker");
    let response: RemoteInvokeResponse =
        serde_json::from_value(response_value).expect("decode remote execution response");
    assert_eq!(response.exit_code, 0, "{:?}", response.stderr);

    let mut streamed = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.event == "remote_execution.stdout" && event.job_id.as_deref() == Some(execution_id)
        {
            let encoded = event.payload["dataBase64"]
                .as_str()
                .expect("stdout event dataBase64");
            streamed.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .expect("decode stdout event"),
            );
        }
    }
    assert_eq!(streamed, b"remote-execution-ok");

    worker
        .shutdown(Duration::from_secs(5))
        .await
        .expect("shutdown Remote Execution worker");
    assert_eq!(worker.state(), WorkerLifecycleState::Stopped);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_worker_bounds_queue_and_cancels_queued_and_running_requests() {
    let temp = tempfile::tempdir().expect("create queue test data dir");
    RemoteShellStore::with_file(temp.path().join("remote_shell.json"))
        .expect("create remote shell store")
        .save(&RemoteShellSet {
            schema_version: 1,
            version: 1,
            policies: vec![RemoteShellPolicy {
                id: "queue-shell".to_string(),
                name: "queue-shell".to_string(),
                description: None,
                enabled: true,
                profile_id: None,
                metadata: serde_json::json!({
                    "exec_mode": "shell_text",
                    "allowed_shell_patterns": ["^(?s:.*)$"],
                    "max_timeout_ms": 10_000
                }),
            }],
            profiles: Vec::new(),
        })
        .expect("save queue shell policy");

    let mut spec = WorkerSpawnSpec::new(
        "test:remote-execution-queue",
        WorkerKind::RemoteExecution,
        env!("CARGO_BIN_EXE_bifrost"),
        vec![
            "auxiliary-worker".to_string(),
            "--kind".to_string(),
            "remote_execution".to_string(),
            "--data-dir".to_string(),
            temp.path().display().to_string(),
            "--admin-host".to_string(),
            "127.0.0.1".to_string(),
            "--admin-port".to_string(),
            "0".to_string(),
        ],
    );
    spec.startup_timeout = Duration::from_secs(15);
    spec.request_timeout = Duration::from_secs(15);
    spec.heartbeat_timeout = Duration::from_secs(30);
    spec.max_concurrency = 1;
    spec.max_queue_depth = 1;
    spec.queue_wait_timeout = Duration::from_secs(5);
    spec.stderr_path = Some(temp.path().join("remote-execution-queue.stderr.log"));
    let worker = ManagedWorker::spawn(spec)
        .await
        .expect("start queue-bounded worker");

    for execution_id in ["queue-running", "queue-waiting"] {
        worker
            .request(
                "remote_execution.prepare",
                serde_json::json!({"executionId": execution_id}),
                Some(Duration::from_secs(5)),
            )
            .await
            .expect("prepare queued execution");
        worker
            .request(
                "remote_execution.stdin_close",
                serde_json::json!({"executionId": execution_id}),
                Some(Duration::from_secs(5)),
            )
            .await
            .expect("close queued execution input");
    }

    let command = RemoteCommand {
        kind: CommandKind::ShellExec,
        policy_id: Some("queue-shell".to_string()),
        exec_mode: Some(ShellExecMode::ShellText),
        command_text: Some("sleep 30".to_string()),
        timeout_ms: Some(10_000),
        ..Default::default()
    };
    let payload = serde_json::json!({"command": command, "fileAccess": "none"});
    let running_worker = worker.clone();
    let running = tokio::spawn(async move {
        running_worker
            .request_with_id(
                "queue-running-request".to_string(),
                Some("queue-running".to_string()),
                "remote_execution.run",
                payload,
                Some(Duration::from_secs(15)),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(150)).await;

    let waiting_worker = worker.clone();
    let waiting = tokio::spawn(async move {
        waiting_worker
            .request_with_id(
                "queue-waiting-request".to_string(),
                Some("queue-waiting".to_string()),
                "remote_execution.run",
                serde_json::json!({
                    "command": {
                        "kind": "shell.exec",
                        "policyId": "queue-shell",
                        "execMode": "shell_text",
                        "commandText": "printf never-runs",
                        "timeoutMs": 10000
                    },
                    "fileAccess": "none"
                }),
                Some(Duration::from_secs(15)),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let full = worker
        .request_with_id(
            "queue-full-request".to_string(),
            Some("queue-full".to_string()),
            "remote_execution.run",
            serde_json::json!({}),
            Some(Duration::from_secs(1)),
        )
        .await
        .expect_err("third request must be rejected by queue bound");
    assert!(full.contains("queue is full"), "{full}");
    assert!(!worker
        .cancel_request("missing-request", "missing-job")
        .await
        .unwrap());
    assert!(worker
        .cancel_request("queue-waiting-request", "queue-waiting")
        .await
        .unwrap());
    assert!(waiting
        .await
        .unwrap()
        .expect_err("queued request must be cancelled")
        .contains("cancelled while queued"));

    assert!(worker
        .cancel_request("queue-running-request", "queue-running")
        .await
        .unwrap());
    assert!(running
        .await
        .unwrap()
        .expect_err("running request must be cancelled")
        .contains("cancelled"));
    worker.cancel_job("missing-logical-job").await.unwrap();
    worker
        .shutdown(Duration::from_secs(5))
        .await
        .expect("shutdown queue-bounded worker");
}
