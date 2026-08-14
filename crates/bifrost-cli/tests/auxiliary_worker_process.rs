use std::time::Duration;

use base64::Engine;
use bifrost_admin::remote_invoke::types::{CommandKind, RemoteCommand, ShellExecMode};
use bifrost_admin::remote_invoke::RemoteInvokeResponse;
use bifrost_admin::worker_runtime::{
    ManagedWorker, WorkerKind, WorkerLifecycleState, WorkerSpawnSpec,
};
use bifrost_storage::{RemoteShellPolicy, RemoteShellSet, RemoteShellStore};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_asr_worker_starts_handles_request_and_shuts_down() {
    let temp = tempfile::tempdir().expect("create worker test data dir");
    let mut spec = WorkerSpawnSpec::new(
        "test:asr-worker",
        WorkerKind::Asr,
        env!("CARGO_BIN_EXE_bifrost"),
        vec![
            "auxiliary-worker".to_string(),
            "--kind".to_string(),
            "asr".to_string(),
            "--data-dir".to_string(),
            temp.path().display().to_string(),
            "--admin-host".to_string(),
            "127.0.0.1".to_string(),
            "--admin-port".to_string(),
            "0".to_string(),
        ],
    );
    spec.startup_timeout = Duration::from_secs(15);
    spec.request_timeout = Duration::from_secs(5);
    spec.heartbeat_timeout = Duration::from_secs(30);
    spec.stderr_path = Some(temp.path().join("asr-worker.stderr.log"));

    let worker = ManagedWorker::spawn(spec)
        .await
        .expect("start real ASR worker process");
    assert_eq!(worker.kind(), WorkerKind::Asr);
    assert_eq!(worker.state(), WorkerLifecycleState::Ready);
    assert!(worker.pid().is_some());
    assert!(!worker.instance_id().is_empty());
    assert!(worker.is_healthy().await);

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
    {
        metadata["shell"] =
            serde_json::json!(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
    }
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
