use std::time::Duration;

use bifrost_admin::worker_runtime::{
    ManagedWorker, WorkerKind, WorkerLifecycleState, WorkerSpawnSpec,
};

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
